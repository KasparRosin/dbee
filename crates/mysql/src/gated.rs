//! How a server reads MySQL's executable comments, which dbee's lexer reads as code whatever the
//! server: `/*!` and MariaDB's `/*M!`, each with a version or none. A server skips one as a plain
//! comment when its version is above the server's, MySQL skips every `/*M!`, and MariaDB skips a
//! `/*!` with a MySQL 5.7 to 9.x version (50700 to 99999). The read-only guard reads a statement
//! as the server will, so a comment the server skips can hide no word from it. A comment the
//! server skips ends, on the server, after a comment nested in it, so one that holds `/*` has no
//! reading dbee trusts.

use std::ops::Range;

use dbee_sql::Dialect;
use dbee_sql::lexer::{TokenKind, tokenize_with};

use crate::flavor::Flavor;

/// The most readings of one statement dbee works out: two for each comment it cannot tell how the
/// server reads, at most four such comments.
const MAX_READINGS: usize = 16;

/// The first MySQL version that may read a sixth digit of a version (MySQL 26.10 does, 8.0 does
/// not; the ones between were not measured).
const SIX_DIGITS_FROM: u32 = 80100;

/// Why a statement has no reading dbee trusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unread {
    /// It would take more than [`MAX_READINGS`].
    TooMany,
    /// A comment the server skips holds `/*`.
    Nested,
}

/// How the server takes one executable comment: `Some(n)` runs it, the first `n` bytes of its
/// start being the mark (the digits after them are code), `None` skips it.
type Way = Option<usize>;

/// The ways `server` may take an executable comment that starts with `start` (`/*!` or `/*M!` and
/// every digit after it): one, or two for a MySQL after 8.0 and six digits.
fn ways(flavor: Flavor, server: u32, start: &str) -> Vec<Way> {
    let mariadb_mark = start.starts_with("/*M!");
    let prefix = if mariadb_mark { 4 } else { 3 };
    let digits = start.len().saturating_sub(prefix);
    let version = |len: usize| start.get(prefix..prefix.saturating_add(len)).and_then(|v| v.parse::<u32>().ok());
    match flavor {
        Flavor::MySql if mariadb_mark => vec![None],
        Flavor::MySql if digits < 5 => vec![Some(prefix)],
        Flavor::MySql => {
            let five = version(5).filter(|v| *v <= server).map(|_| prefix + 5);
            let six = version(6).filter(|v| *v <= server).map(|_| prefix + 6);
            if digits >= 6 && server >= SIX_DIGITS_FROM { vec![five, six] } else { vec![five] }
        }
        Flavor::MariaDb if digits < 5 => vec![Some(prefix)],
        Flavor::MariaDb => {
            let len = if digits >= 6 { 6 } else { 5 };
            let runs = version(len).is_some_and(|v| v <= server && (mariadb_mark || !(50_700..=99_999).contains(&v)));
            vec![runs.then_some(prefix + len)]
        }
    }
}

/// `text` with the bytes of `range` (ASCII at both ends) turned into spaces, so every other byte
/// keeps its place.
fn blank(text: &str, range: Range<usize>) -> String {
    let (Some(before), Some(after)) = (text.get(..range.start), text.get(range.end..)) else { return text.to_owned() };
    format!("{before}{}{after}", " ".repeat(range.len()))
}

/// The first executable comment's start in `text`, and the end that closes it, if any.
fn first_comment(text: &str, backslash_escapes: bool) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let tokens = tokenize_with(Dialect::MySql, text, backslash_escapes);
    let marks: Vec<&dbee_sql::Token> = tokens.iter().filter(|t| t.kind == TokenKind::ExecutableMark).collect();
    let k = marks.iter().position(|t| text.get(t.range.clone()).is_some_and(|s| s.starts_with("/*")))?;
    let start = marks.get(k)?.range.clone();
    let end = marks.get(k + 1).filter(|t| text.get(t.range.clone()) == Some("*/")).map(|t| t.range.clone());
    Some((start, end))
}

/// `text` with its first executable comment, which starts at `start` and is closed at `end`, read
/// the `way` the server takes it: skipped, it is blanked to the first `*/` after its start, where
/// the server's comment ends; run, its marks are blanked and the text between stays.
fn read_one(text: &str, start: Range<usize>, end: Option<Range<usize>>, way: Way) -> String {
    match way {
        None => blank(text, start.start..skipped_end(text, &start)),
        Some(mark) => {
            let text = end.map_or_else(|| text.to_owned(), |end| blank(text, end));
            blank(&text, start.start..start.start.saturating_add(mark).min(start.end))
        }
    }
}

/// Where a skipped comment that starts at `start` ends: after the first `*/`, or at the end.
fn skipped_end(text: &str, start: &Range<usize>) -> usize {
    text.get(start.end..).and_then(|rest| rest.find("*/")).map_or(text.len(), |at| start.end + at + 2)
}

/// `sql` as `server` reads it, its strings with backslash escapes as `backslash_escapes` says:
/// each executable comment it skips blanked out, and the marks of each it runs blanked, so the
/// text between reads as the code it is. More than one reading when a comment can be taken two
/// ways.
pub(crate) fn as_read(sql: &str, flavor: Flavor, server: u32, backslash_escapes: bool) -> Result<Vec<String>, Unread> {
    if !sql.contains("/*!") && !sql.contains("/*M!") {
        return Ok(vec![sql.to_owned()]);
    }
    let (mut done, mut todo) = (Vec::new(), vec![sql.to_owned()]);
    while let Some(text) = todo.pop() {
        let Some((start, end)) = first_comment(&text, backslash_escapes) else {
            done.push(text);
            continue;
        };
        let start_text = text.get(start.clone()).unwrap_or_default();
        for way in ways(flavor, server, start_text) {
            if way.is_none() && text.get(start.end..skipped_end(&text, &start)).is_some_and(|skipped| skipped.contains("/*")) {
                return Err(Unread::Nested);
            }
            todo.push(read_one(&text, start.clone(), end.clone(), way));
        }
        if done.len().saturating_add(todo.len()) > MAX_READINGS {
            return Err(Unread::TooMany);
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_skips_what_it_was_measured_to_skip() {
        let (my57, my80, my2610, maria1011, maria13) =
            ((Flavor::MySql, 50744), (Flavor::MySql, 80046), (Flavor::MySql, 261000), (Flavor::MariaDb, 101119), (Flavor::MariaDb, 130002));
        // Measured with `SELECT 1 /*!99999 + 100 */, 1 /*M!999999 + 100 */, 1 /*M! + 100 */` and the others.
        for (server, start, want) in [
            (my57, "/*!99999", vec![None]),
            (my80, "/*!99999", vec![None]),
            (my2610, "/*!99999", vec![Some(8)]),
            (maria1011, "/*!99999", vec![None]),
            (maria13, "/*!99999", vec![None]),
            (my80, "/*M!999999", vec![None]),
            (maria13, "/*M!999999", vec![None]),
            (my57, "/*M!", vec![None]),
            (my80, "/*M!100100", vec![None]),
            (maria1011, "/*M!100100", vec![Some(10)]),
            (maria1011, "/*M!", vec![Some(4)]),
            (my80, "/*!", vec![Some(3)]),
            (my80, "/*!1234", vec![Some(3)]),
            (my80, "/*!50001", vec![Some(8)]),
            (maria1011, "/*!50001", vec![Some(8)]),
            (my80, "/*!500001", vec![Some(8)]),
            (my2610, "/*!500001", vec![Some(8), None]),
            (maria1011, "/*!500001", vec![None]),
            (maria1011, "/*!100100", vec![Some(9)]),
        ] {
            assert_eq!(ways(server.0, server.1, start), want, "{server:?} {start}");
        }
    }

    #[test]
    fn a_statement_reads_as_the_server_reads_it() {
        let read = |sql: &str, flavor, server| as_read(sql, flavor, server, true).unwrap_or_default();
        assert_eq!(read("/*M!999999 SELECT */ START TRANSACTION", Flavor::MySql, 80046), ["                     START TRANSACTION"]);
        assert_eq!(read("/*M! SELECT */ START", Flavor::MariaDb, 101119), ["     SELECT    START"]);
        assert_eq!(read("SELECT 1 /*!50001 + 1 */", Flavor::MySql, 80046), ["SELECT 1          + 1   "]);
        assert_eq!(read("SELECT 1 /*!99999 '*/' */", Flavor::MySql, 80046), ["SELECT 1             ' */"]);
        assert_eq!(read("SELECT 1 /*!500001 */", Flavor::MySql, 261000).len(), 2);
        assert_eq!(read("SELECT 1", Flavor::MySql, 80046), ["SELECT 1"]);
        let many = "/*!500001 */ ".repeat(5);
        assert_eq!(as_read(&many, Flavor::MySql, 261000, true), Err(Unread::TooMany));
        // With backslash escapes off, `'\'` is a whole string, so the comment after it is one.
        assert_eq!(read(r"SELECT '\' /*!99999 x */", Flavor::MySql, 80046), [r"SELECT '\' /*!99999 x */"]);
        assert_eq!(as_read(r"SELECT '\' /*!99999 x */", Flavor::MySql, 80046, false), Ok(vec![r"SELECT '\'              ".to_owned()]));
    }

    /// A comment the server skips ends, on the server, after a comment nested in it (measured on
    /// all five servers): a statement where a skipped one holds `/*` has no reading dbee trusts.
    #[test]
    fn a_skipped_comment_that_holds_a_comment_has_no_reading() {
        for (sql, flavor, server) in [
            ("/*!99999 /* */ SELECT */ START TRANSACTION READ WRITE", Flavor::MySql, 80046),
            ("/*!999999 /* */ SELECT */ START TRANSACTION READ WRITE", Flavor::MySql, 261000),
            ("/*M!999999 /* */ SELECT */ START TRANSACTION READ WRITE", Flavor::MariaDb, 130002),
            ("/*M!999999 /* */ SELECT */ START TRANSACTION READ WRITE", Flavor::MySql, 80046),
            ("/*!99999 /*!50000 x */ SELECT */ START TRANSACTION READ WRITE", Flavor::MySql, 50744),
            ("/*!99999 /*!50000 x */ SELECT */ START TRANSACTION READ WRITE", Flavor::MariaDb, 101119),
            ("SELECT 1 /*!99999 + 1 /* x */ + 2 */", Flavor::MariaDb, 101119),
        ] {
            assert_eq!(as_read(sql, flavor, server, true), Err(Unread::Nested), "{flavor:?} {server}: {sql}");
        }
        // One the server runs may hold a comment: its text is code, and the comment a comment.
        assert!(as_read("SELECT 1 /*!50000 + 1 /* x */ */", Flavor::MySql, 80046, true).is_ok());
    }
}

//! The statements dbee does not send: on a read-only connection, those that can turn read-only
//! off or write in spite of it; on any connection, those that would make the server read the
//! statements after them otherwise than dbee shows and splits them. Each engine's rules, in its
//! own dialect file, read a statement's [`Code`].

use std::ops::Range;

use crate::Dialect;
use crate::lexer::{Reading, Token, TokenKind, code, statements_with, tokenize, tokenize_with};

/// A statement dbee does not send, and why, in words for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// On a read-only connection: it can turn read-only off, or write in spite of it.
    ReadOnly(&'static str),
    /// It would make the server read the statements after it otherwise than dbee shows them.
    Misread(&'static str),
}

impl Refusal {
    /// What the user reads.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::ReadOnly(m) | Refusal::Misread(m) => m,
        }
    }
}

pub(crate) const CHANGES_MODE: &str =
    "this statement changes read-only mode, which only the connection's settings change here. To write, turn Read-only off in them.";

pub(crate) const READ_WRITE: &str =
    "this statement makes a transaction read-write, so dbee does not run it here. To write, turn Read-only off in the connection's settings.";

/// The values that turn a setting on.
pub(crate) const ON: &[&str] = &["on", "true", "yes", "1"];

pub(crate) const CHAIN: &str = "AND CHAIN starts the next transaction in the mode of the one it ends, so dbee does not run it here. End the \
                                transaction, then begin the next one.";

/// One statement as the server reads it: its tokens without whitespace, comments and the marks of
/// an executable comment.
pub(crate) struct Code<'a> {
    sql: &'a str,
    tokens: Vec<&'a Token>,
}

impl<'a> Code<'a> {
    fn of(sql: &'a str, tokens: &'a [Token], range: &Range<usize>) -> Code<'a> {
        let tokens = tokens.iter().filter(|t| t.range.start >= range.start && t.range.end <= range.end && code(t)).collect();
        Code { sql, tokens }
    }

    /// Token `i` as a word: a keyword or an identifier, or a quoted identifier without its quotes
    /// (a `U&` one spelled out), in lowercase.
    pub(crate) fn word(&self, i: usize) -> Option<String> {
        let t = self.tokens.get(i)?;
        let text = self.sql.get(t.range.clone())?;
        match t.kind {
            TokenKind::Keyword | TokenKind::Ident => Some(text.to_lowercase()),
            TokenKind::QuotedIdent => self.quoted(i, text).map(|name| name.to_lowercase()),
            _ => None,
        }
    }

    /// What the quoted name or string `text` at token `i` holds: without its quotes, each doubled
    /// quote one, and a `U&` one's escapes spelled out (with the `UESCAPE` character after it).
    fn quoted(&self, i: usize, text: &str) -> Option<String> {
        let unicode = text.strip_prefix(['U', 'u']).and_then(|t| t.strip_prefix('&'));
        let text = unicode.unwrap_or(text);
        let quote = text.chars().next()?;
        let inner = text.get(quote.len_utf8()..)?;
        let inner = inner.strip_suffix(quote).unwrap_or(inner).replace(&format!("{quote}{quote}"), &quote.to_string());
        if unicode.is_none() {
            return Some(inner);
        }
        let escape = match (self.is(i + 1, "uescape"), self.tokens.get(i + 2).and_then(|t| self.sql.get(t.range.clone()))) {
            (true, Some(c)) => c.strip_prefix('\'').and_then(|c| c.strip_suffix('\'')).and_then(|c| c.chars().next()).unwrap_or('\\'),
            _ => '\\',
        };
        Some(unescape(&inner, escape).unwrap_or(inner))
    }

    /// Token `i` is the word `w`.
    pub(crate) fn is(&self, i: usize, w: &str) -> bool {
        self.word(i).is_some_and(|x| x == w)
    }

    /// The statement starts with one of `words`.
    pub(crate) fn starts(&self, words: &[&str]) -> bool {
        self.word(0).is_some_and(|x| words.contains(&x.as_str()))
    }

    /// Some token is one of `words`.
    pub(crate) fn names(&self, words: &[&str]) -> bool {
        (0..self.tokens.len()).any(|i| self.word(i).is_some_and(|x| words.contains(&x.as_str())))
    }

    /// The words `a` and `b` follow each other.
    pub(crate) fn pair(&self, a: &str, b: &str) -> bool {
        (1..self.tokens.len()).any(|i| self.is(i - 1, a) && self.is(i, b))
    }

    /// Some string literal holds `part`, in any case (a `U&` one spelled out).
    pub(crate) fn string_holds(&self, part: &str) -> bool {
        (0..self.tokens.len()).any(|i| {
            let Some(t) = self.tokens.get(i).filter(|t| t.kind == TokenKind::String) else { return false };
            let text = self.sql.get(t.range.clone()).unwrap_or_default();
            let unicode = text.starts_with(['U', 'u']).then(|| self.quoted(i, text)).flatten();
            unicode.as_deref().unwrap_or(text).to_lowercase().contains(part)
        })
    }

    /// Token `i`'s text.
    pub(crate) fn text(&self, i: usize) -> Option<&'a str> {
        self.tokens.get(i).and_then(|t| self.sql.get(t.range.clone()))
    }

    /// Token `i` as a name: a word ([`Code::word`]), or a `"…"` string's text, which MySQL's
    /// `ANSI_QUOTES` makes a name, in lowercase.
    pub(crate) fn name(&self, i: usize) -> Option<String> {
        let text = self.text(i)?;
        match self.tokens.get(i)?.kind {
            TokenKind::String if text.starts_with('"') => self.quoted(i, text).map(|name| name.to_lowercase()),
            _ => self.word(i),
        }
    }

    /// Token `i` as a value: a name ([`Code::name`]) or a `'…'` string's text, in lowercase.
    pub(crate) fn literal(&self, i: usize) -> Option<String> {
        match self.text(i)? {
            text if text.starts_with('\'') => self.quoted(i, text).map(|value| value.to_lowercase()),
            _ => self.name(i),
        }
    }

    /// The tokens that name `name` ([`Code::name`]).
    pub(crate) fn naming(&self, name: &str) -> Vec<usize> {
        (0..self.tokens.len()).filter(|&i| self.name(i).is_some_and(|n| n == name)).collect()
    }

    /// Where each assignment of a `SET` starts: right after `SET`, or after a `,` outside
    /// parentheses. None in another statement.
    pub(crate) fn assignments(&self) -> Vec<usize> {
        if !self.is(0, "set") {
            return Vec::new();
        }
        let depths = self.depths();
        (1..self.tokens.len()).filter(|&i| i == 1 || (self.text(i - 1) == Some(",") && depths.get(i - 1) == Some(&0))).collect()
    }

    /// Token `i` is the keyword `w`, unquoted.
    fn keyword(&self, i: usize, w: &str) -> bool {
        self.tokens.get(i).is_some_and(|t| matches!(t.kind, TokenKind::Keyword | TokenKind::Ident)) && self.is(i, w)
    }

    /// How deep in parentheses each token is.
    fn depths(&self) -> Vec<usize> {
        let mut depth = 0usize;
        (0..self.tokens.len())
            .map(|i| match self.text(i) {
                Some("(") => {
                    depth += 1;
                    depth - 1
                }
                Some(")") => {
                    depth = depth.saturating_sub(1);
                    depth
                }
                _ => depth,
            })
            .collect()
    }

    /// The token after the first unquoted keyword `w` outside parentheses, as an unquoted keyword
    /// in lowercase (`None` inside when it is something else); `None` when there is no such `w`.
    pub(crate) fn after_top_level(&self, w: &str) -> Option<Option<String>> {
        let depths = self.depths();
        let at = (0..self.tokens.len()).find(|&i| depths.get(i) == Some(&0) && self.keyword(i, w))?;
        Some(self.tokens.get(at + 1).filter(|t| matches!(t.kind, TokenKind::Keyword | TokenKind::Ident)).and_then(|_| self.word(at + 1)))
    }

    /// A `SET` that changes a setting for the whole server: one of `scopes` right after `SET` or a
    /// `,` outside parentheses, or a `@@scope.name` set with `=` or `:=`.
    pub(crate) fn sets_scope(&self, scopes: &[&str]) -> bool {
        let depths = self.depths();
        (0..self.tokens.len()).filter(|&i| self.word(i).is_some_and(|x| scopes.contains(&x.as_str()))).any(|i| {
            let before = i.checked_sub(1);
            let opens = before.is_some_and(|b| (b == 0 && self.is(0, "set")) || (self.text(b) == Some(",") && depths.get(b) == Some(&0)));
            // `:=` is two tokens, and `=` joins the operator characters after it.
            let named = before.and_then(|b| self.text(b)).is_some_and(|t| t.ends_with("@@"))
                && self.text(i + 1) == Some(".")
                && self.text(i + 3).is_some_and(|t| t.starts_with('=') || t == ":");
            opens || named
        })
    }

    /// The value after token `at` and `=` or `TO`: a word, a number, or a string without its
    /// quotes, in lowercase.
    fn value_at(&self, at: usize) -> Option<String> {
        let sign = self.tokens.get(at.checked_add(1)?)?;
        if !(self.is(at + 1, "to") || self.sql.get(sign.range.clone()) == Some("=")) {
            return None;
        }
        let value = self.tokens.get(at.checked_add(2)?)?;
        let text = self.sql.get(value.range.clone())?;
        let text = if value.kind == TokenKind::String { text.trim_matches('\'') } else { text };
        Some(text.to_lowercase())
    }

    /// The value after the word `name` and `=` or `TO`, as [`Code::value_at`] reads it.
    pub(crate) fn value_of(&self, name: &str) -> Option<String> {
        self.value_at((0..self.tokens.len()).find(|&i| self.is(i, name))?)
    }

    /// Some setting of `names` is set to anything but one token that turns it on ([`ON`]), alone
    /// before a `,` or the statement's end; or named where dbee reads no value.
    pub(crate) fn sets_off(&self, names: &[&str]) -> bool {
        (0..self.tokens.len()).filter(|&i| self.word(i).is_some_and(|x| names.contains(&x.as_str()))).any(|i| {
            let alone = self.tokens.get(i + 3).is_none() || self.text(i + 3) == Some(",");
            !(alone && self.value_at(i).is_some_and(|v| ON.contains(&v.as_str())))
        })
    }
}

/// The text a `U&` name or string spells: `escape` and four hex digits, or `escape+` and six, is
/// that character, and `escape` twice is `escape`; `None` when an escape is not one.
fn unescape(body: &str, escape: char) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != escape {
            out.push(c);
            continue;
        }
        let rest = chars.as_str();
        if rest.starts_with(escape) {
            chars.next();
            out.push(escape);
            continue;
        }
        let (digits, len) = match rest.strip_prefix('+') {
            Some(six) => (six.get(..6)?, 7),
            None => (rest.get(..4)?, 4),
        };
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out.push(char::from_u32(u32::from_str_radix(digits, 16).ok()?)?);
        for _ in 0..len {
            chars.next();
        }
    }
    Some(out)
}

impl Dialect {
    /// Why dbee does not send `sql`, one statement or several, on a connection of this engine that
    /// is read-only or not; `None` when it may.
    pub fn refusal(self, sql: &str, read_only: bool) -> Option<Refusal> {
        self.refusal_with(sql, read_only, Reading::of(self))
    }

    /// [`Dialect::refusal`], with quotes read as `reading` says ([`tokenize_with`]): as the
    /// session reads them.
    pub fn refusal_with(self, sql: &str, read_only: bool, reading: Reading) -> Option<Refusal> {
        let refuse = self.rules().refuse;
        let tokens = tokenize_with(self, sql, reading);
        statements_with(self, sql, reading).iter().find_map(|range| refuse(&Code::of(sql, &tokens, range), read_only))
    }

    /// `sql` has a plain string literal (`'…'`) that holds a backslash, which a server reads as the
    /// lexer does only while such backslashes are plain characters.
    pub fn backslash_in_plain_string(self, sql: &str) -> bool {
        tokenize(self, sql)
            .iter()
            .filter(|t| t.kind == TokenKind::String)
            .filter_map(|t| sql.get(t.range.clone()))
            .any(|s| s.starts_with('\'') && s.contains('\\'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(d: Dialect, sql: &str, read_only: bool) -> Option<Refusal> {
        d.refusal(sql, read_only)
    }

    #[test]
    fn postgres_refuses_what_can_end_read_only() {
        let pg = Dialect::Postgres;
        for sql in [
            "DO $$ BEGIN COMMIT; END $$",
            "/* x */ do $$ $$",
            "CALL p()",
            "SET default_transaction_read_only = off",
            "set session \"DEFAULT_TRANSACTION_READ_ONLY\" to false",
            "SET LOCAL transaction_read_only = off",
            "RESET default_transaction_read_only",
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ WRITE",
            "BEGIN READ WRITE",
            "START TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ WRITE",
            "RESET ALL",
            "DISCARD ALL",
            "COMMIT AND CHAIN",
            "rollback and chain",
            "END AND CHAIN",
            "SELECT pg_catalog.set_config('default_transaction_read_only', 'off', false)",
            "SELECT lo_from_bytea(0, 'x')",
            "SELECT \"lo_create\"(0)",
            "SELECT 1; DO $$ $$",
        ] {
            assert!(matches!(refused(pg, sql, true), Some(Refusal::ReadOnly(_))), "{sql}");
            assert_eq!(refused(pg, sql, false), None, "{sql}");
        }
        for sql in [
            "SELECT 1",
            "BEGIN",
            "BEGIN ISOLATION LEVEL READ COMMITTED",
            "BEGIN READ ONLY",
            "SET TRANSACTION READ ONLY",
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            "COMMIT",
            "ROLLBACK AND NO CHAIN",
            "SET search_path = app",
            "SET LOCAL search_path TO pg_catalog, pg_temp; SELECT 1",
            "SELECT pg_catalog.set_config('search_path', '', false)",
            "SELECT current_setting('default_transaction_read_only')",
            "SHOW transaction_read_only",
            "SELECT 'do', 'call' AS \"read write\"",
            "DISCARD PLANS",
            "-- DO\nSELECT 1",
        ] {
            assert_eq!(refused(pg, sql, true), None, "{sql}");
        }
    }

    #[test]
    fn postgres_refuses_turning_standard_conforming_strings_off_everywhere() {
        let pg = Dialect::Postgres;
        for sql in [
            "SET standard_conforming_strings = off",
            "SET standard_conforming_strings TO 'off'",
            "SET SESSION standard_conforming_strings TO DEFAULT",
            "RESET standard_conforming_strings",
            "SELECT set_config('standard_conforming_strings', 'off', false)",
        ] {
            for read_only in [false, true] {
                assert!(matches!(refused(pg, sql, read_only), Some(Refusal::Misread(_))), "{sql}");
            }
        }
        for sql in ["SET standard_conforming_strings = on", "set standard_conforming_strings to 'on'", "SET standard_conforming_strings TO true"] {
            assert_eq!(refused(pg, sql, false), None, "{sql}");
        }
    }

    #[test]
    fn mysql_refuses_what_can_end_read_only() {
        let my = Dialect::MySql;
        for sql in [
            "START TRANSACTION READ WRITE",
            "start transaction with consistent snapshot, read write",
            "/*!40101 START TRANSACTION READ WRITE */",
            "START TRANSACTION /*! READ WRITE */",
            "SET SESSION TRANSACTION READ WRITE",
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ WRITE",
            "SET @@SESSION.transaction_read_only = 0",
            "SET @@session.`tx_read_only` := 0",
            "SET GLOBAL transaction_read_only = 0",
            "SET STATEMENT max_statement_time = 1 FOR SELECT 1",
            "CALL p()",
            "PREPARE s FROM 'SELECT 1'",
            "EXECUTE s",
            "EXECUTE IMMEDIATE 'SELECT 1'",
            "COMMIT AND CHAIN",
            "ROLLBACK WORK AND CHAIN",
        ] {
            assert!(matches!(refused(my, sql, true), Some(Refusal::ReadOnly(_))), "{sql}");
            assert_eq!(refused(my, sql, false), None, "{sql}");
        }
        for sql in [
            "SELECT 1",
            "START TRANSACTION",
            "START TRANSACTION READ ONLY",
            "BEGIN",
            "SET NAMES utf8mb4",
            "/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */",
            "SET @statement = 1",
            "SELECT @@SESSION.transaction_read_only",
            "SHOW WARNINGS",
            "COMMIT AND NO CHAIN",
            "/* CALL p() */ SELECT 1",
        ] {
            assert_eq!(refused(my, sql, true), None, "{sql}");
        }
    }

    /// A read-only MySQL session keeps reading statements in UTF-8: a `SET` of the client's
    /// character set to anything else is refused, however it is spelled; reading it, and a
    /// character set inside an expression, are not.
    #[test]
    fn mysql_keeps_the_character_set_utf8_on_read_only() {
        let my = Dialect::MySql;
        for sql in [
            "SET NAMES sjis",
            "set names 'gbk'",
            "SET NAMES \"big5\" COLLATE big5_chinese_ci",
            "SET NAMES DEFAULT",
            "SET NAMES utf8mb4 sjis",
            "/*!40101 SET NAMES cp932 */",
            "SET CHARACTER SET gbk",
            "SET CHARSET sjis",
            "SET @a = 1, NAMES sjis",
            "SET @a = (SELECT 1), CHARACTER SET gb18030",
            "SET character_set_client = sjis",
            "SET @@character_set_client = 'big5'",
            "SET SESSION character_set_client = DEFAULT",
            "SET LOCAL character_set_client = @x",
            "SET character_set_client=@x",
            "SET @@local.character_set_client := 'cp932'",
            "SET @@SESSION.`character_set_client` = 'sjis'",
            "SET @@SESSION.\"character_set_client\" = 'sjis'",
            "SET @a = 1, character_set_client = 'gbk'",
            "SET character_set_client = 'utf8mb4' COLLATE utf8mb4_bin",
        ] {
            let why = refused(my, sql, true).map(Refusal::message).unwrap_or_default();
            assert!(why.contains("character set"), "{sql}: {why}");
            assert_eq!(refused(my, sql, false), None, "{sql}");
        }
        for sql in [
            "SET NAMES utf8mb4",
            "SET NAMES 'utf8mb4' COLLATE 'utf8mb4_bin'",
            "set names utf8mb3",
            "SET NAMES `utf8`, @a = 1",
            "SET CHARACTER SET utf8mb4",
            "SET CHARSET \"utf8\"",
            "SET @@SESSION.character_set_client = 'UTF8MB4'",
            "SET character_set_client = utf8mb4, @a = 1",
            "SET @old = @@character_set_client",
            "SET @names = 'sjis', @charset = 'gbk'",
            "SET @a = CAST('x' AS CHAR CHARACTER SET sjis), @b = CONVERT('y' USING gbk)",
            "SELECT @@character_set_client = 'sjis'",
            "SHOW CHARACTER SET LIKE 'sjis'",
        ] {
            assert_eq!(refused(my, sql, true), None, "{sql}");
        }
    }

    /// Names spelled with Unicode escapes, `UESCAPE` too, are the names they spell.
    #[test]
    fn postgres_reads_names_spelled_with_unicode_escapes() {
        let pg = Dialect::Postgres;
        for sql in [
            r#"SET U&"default_transaction_read_onl\0079" = off"#,
            r#"SET U&"default_transaction_read_onl!0079" UESCAPE '!' = off"#,
            r#"SELECT U&"lo_from_byte\0061"(0, 'x')"#,
            r#"SELECT pg_catalog.U&"lo_from_byte\+000061"(0, 'x')"#,
            r#"SELECT set_config(U&'default_transaction_read\005Fonly', 'off', false)"#,
        ] {
            assert!(matches!(refused(pg, sql, true), Some(Refusal::ReadOnly(_))), "{sql}");
        }
        assert!(matches!(refused(pg, r#"SET U&"standard_conforming_string\0073" = off"#, false), Some(Refusal::Misread(_))));
        assert_eq!(refused(pg, r#"SELECT U&"d\0061ta" FROM t"#, true), None);
    }

    /// Statements that write outside the database: the server's files and settings.
    #[test]
    fn read_only_refuses_writes_outside_the_database() {
        for (d, sql) in [
            (Dialect::Postgres, "ALTER SYSTEM SET work_mem = '4MB'"),
            (Dialect::Postgres, "COPY (SELECT 1) TO '/tmp/x'"),
            (Dialect::Postgres, "COPY t TO PROGRAM 'cat'"),
            (Dialect::MySql, "SELECT 1 INTO OUTFILE '/tmp/x'"),
            (Dialect::MySql, "SELECT 1 FROM t INTO DUMPFILE '/tmp/x'"),
            (Dialect::MySql, "SET GLOBAL max_connections = 10"),
            (Dialect::MySql, "SET @@global.max_connections = 10"),
            (Dialect::MySql, "SET PERSIST max_connections = 10"),
            (Dialect::MySql, "SET PERSIST_ONLY max_connections = 10"),
            (Dialect::MySql, "SET @@persist.max_connections := 10"),
            (Dialect::MySql, "SET @@GLOBAL.max_connections=-1"),
            (Dialect::MySql, "SET @a = 1, GLOBAL max_connections = 10"),
            (Dialect::Postgres, r#"COPY (SELECT "to" stdout FROM (VALUES (1)) AS v("to")) TO PROGRAM 'cat'"#),
            (Dialect::Postgres, r#"COPY (WITH "to" AS (SELECT 1 AS x) SELECT * FROM "to" stdout) TO PROGRAM 'cat'"#),
            (Dialect::Postgres, r#"COPY t TO "stdout""#),
        ] {
            assert!(matches!(refused(d, sql, true), Some(Refusal::ReadOnly(_))), "{d:?}: {sql}");
            assert_eq!(refused(d, sql, false), None, "{d:?}: {sql}");
        }
        for (d, sql) in [
            (Dialect::Postgres, "COPY t TO STDOUT"),
            (Dialect::Postgres, "ALTER TABLE t ADD COLUMN x int"),
            (Dialect::MySql, "SELECT 1 INTO @a"),
            (Dialect::MySql, "SET @global = 1"),
            (Dialect::MySql, "SET SESSION sql_mode = ''"),
            (Dialect::MySql, "SET SESSION sql_mode = @@GLOBAL.sql_mode"),
            (Dialect::MySql, "SET @x = @@global.max_connections, @y = 1"),
            (Dialect::MySql, "SET @persist = 1"),
            (Dialect::Postgres, r#"COPY (SELECT 1 AS "to") TO STDOUT"#),
            (Dialect::Postgres, "COPY t (a, b) TO STDOUT WITH (FORMAT csv)"),
            (Dialect::Postgres, "COPY t FROM STDIN"),
        ] {
            assert_eq!(refused(d, sql, true), None, "{d:?}: {sql}");
        }
        let global = refused(Dialect::MySql, "SET GLOBAL max_connections = 10", true).map(Refusal::message).unwrap_or_default();
        assert!(global.contains("every session"), "{global}");
    }

    /// A `SET` that turns read-only on, with no `READ WRITE` in it, changes nothing that matters.
    #[test]
    fn a_set_that_turns_read_only_on_is_let_through() {
        for (d, sql) in [
            (Dialect::Postgres, "SET default_transaction_read_only = on"),
            (Dialect::Postgres, "SET transaction_read_only TO true"),
            (Dialect::Postgres, "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY"),
            (Dialect::Postgres, "SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            (Dialect::MySql, "SET SESSION TRANSACTION READ ONLY"),
            (Dialect::MySql, "SET @@SESSION.transaction_read_only = 1"),
            (Dialect::MySql, "SET SESSION tx_read_only = ON"),
            (Dialect::MySql, "SET @@transaction_read_only = 1, @a = 2"),
        ] {
            assert_eq!(refused(d, sql, true), None, "{d:?}: {sql}");
        }
        for (d, sql) in [
            (Dialect::Postgres, "SET default_transaction_read_only TO DEFAULT"),
            (Dialect::Postgres, "RESET transaction_read_only"),
            (Dialect::Postgres, "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE"),
            (Dialect::MySql, "SET @@transaction_read_only = 1, @@transaction_read_only = 0"),
            (Dialect::MySql, "SET SESSION TRANSACTION READ ONLY, READ WRITE"),
            (Dialect::MySql, "SET @@transaction_read_only = 1 - 1"),
            (Dialect::MySql, "SET @@transaction_read_only = TRUE - 1"),
            (Dialect::MySql, "SET SESSION tx_read_only = 1 AND 0"),
            (Dialect::MySql, "SET @@transaction_read_only = ON = OFF"),
            (Dialect::Postgres, "SET default_transaction_read_only = on off"),
        ] {
            assert!(matches!(refused(d, sql, true), Some(Refusal::ReadOnly(_))), "{d:?}: {sql}");
        }
    }

    #[test]
    fn turning_standard_conforming_strings_off_says_what_to_do() {
        let Some(Refusal::Misread(m)) = refused(Dialect::Postgres, "SET standard_conforming_strings = off", false) else { panic!("refused") };
        assert!(m.contains("E'"), "{m}");
    }

    #[test]
    fn sqlite_leaves_it_to_the_authorizer() {
        for sql in ["ATTACH 'x' AS y", "VACUUM INTO 'x'", "PRAGMA query_only = 0"] {
            assert_eq!(refused(Dialect::Sqlite, sql, true), None, "{sql}");
        }
    }

    #[test]
    fn backslashes_in_plain_strings_are_found() {
        let pg = Dialect::Postgres;
        assert!(pg.backslash_in_plain_string(r"SELECT 'a\'"));
        assert!(pg.backslash_in_plain_string(r"SELECT 1 WHERE x = 'C:\path'"));
        for sql in [r"SELECT E'a\\b'", r"SELECT $$a\$$", r#"SELECT "a\b""#, r"SELECT 1 -- a\b", "SELECT 'plain'", r"SELECT U&'d\0061'"] {
            assert!(!pg.backslash_in_plain_string(sql), "{sql}");
        }
    }
}

//! The statements dbee does not send: on a read-only connection, those that can turn read-only
//! off or write in spite of it; on any connection, those that would make the server read the
//! statements after them otherwise than dbee shows and splits them. Each engine's rules, in its
//! own dialect file, read a statement's [`Code`].

use std::ops::Range;

use crate::Dialect;
use crate::lexer::{Token, TokenKind, code, statements, tokenize};

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

    /// Token `i` as a word: a keyword or an identifier, or a quoted identifier without its quotes,
    /// in lowercase.
    pub(crate) fn word(&self, i: usize) -> Option<String> {
        let t = self.tokens.get(i)?;
        let text = self.sql.get(t.range.clone())?;
        match t.kind {
            TokenKind::Keyword | TokenKind::Ident => Some(text.to_lowercase()),
            TokenKind::QuotedIdent => {
                let text = text.strip_prefix(['U', 'u']).and_then(|t| t.strip_prefix('&')).unwrap_or(text);
                let quote = text.chars().next()?;
                let inner = text.get(quote.len_utf8()..)?;
                let inner = inner.strip_suffix(quote).unwrap_or(inner);
                Some(inner.replace(&format!("{quote}{quote}"), &quote.to_string()).to_lowercase())
            }
            _ => None,
        }
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

    /// Some string literal holds `part`, in any case.
    pub(crate) fn string_holds(&self, part: &str) -> bool {
        self.tokens
            .iter()
            .filter(|t| t.kind == TokenKind::String)
            .any(|t| self.sql.get(t.range.clone()).is_some_and(|s| s.to_lowercase().contains(part)))
    }

    /// The value after the word `name` and `=` or `TO`: a word, a number, or a string without its
    /// quotes, in lowercase.
    pub(crate) fn value_of(&self, name: &str) -> Option<String> {
        let at = (0..self.tokens.len()).find(|&i| self.is(i, name))?;
        let sign = self.tokens.get(at.checked_add(1)?)?;
        if !(self.is(at + 1, "to") || self.sql.get(sign.range.clone()) == Some("=")) {
            return None;
        }
        let value = self.tokens.get(at.checked_add(2)?)?;
        let text = self.sql.get(value.range.clone())?;
        let text = if value.kind == TokenKind::String { text.trim_matches('\'') } else { text };
        Some(text.to_lowercase())
    }
}

impl Dialect {
    /// Why dbee does not send `sql`, one statement or several, on a connection of this engine that
    /// is read-only or not; `None` when it may.
    pub fn refusal(self, sql: &str, read_only: bool) -> Option<Refusal> {
        let refuse = self.rules().refuse;
        let tokens = tokenize(self, sql);
        statements(self, sql).iter().find_map(|range| refuse(&Code::of(sql, &tokens, range), read_only))
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

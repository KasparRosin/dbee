//! The SQLite result codes dbee acts on, and a rusqlite error as a [`DbError`].

use std::ffi::c_int;

use dbee_driver::{DbError, ServerError};
use rusqlite::ffi;

/// What a result code makes of the error, other than a [`DbError::Server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Becomes {
    Cancelled,
    ReadOnly,
}

/// The result codes dbee acts on, matched exactly. Every other code is the server's error with its
/// extended code: `SQLITE_BUSY` (the file is locked past the busy timeout) among them, and the
/// extended codes of `SQLITE_READONLY` (the file moved, a lock it cannot take), which say more
/// than that the connection is read-only.
const CODES: &[(c_int, Becomes)] = &[(ffi::SQLITE_INTERRUPT, Becomes::Cancelled), (ffi::SQLITE_READONLY, Becomes::ReadOnly)];

/// What the text shows before the code: `no such column: x (SQLite error 1)`.
const CODE_LABEL: &str = "SQLite error";

pub(crate) const PARAMETERS: &str = "the statement has parameters (such as ?), which dbee does not fill";

/// The hint of `SQLITE_AUTH`, which only the authorizer of an isolated run's connection gives.
const OWN_FILE_ONLY: &str = "This isolated run lets SQL open no other file, so ATTACH and VACUUM are refused.";

/// `e`, from running the statement at byte `start` of `sql`, the text the caller ran; a syntax
/// error's offset becomes a character position in `sql`.
pub(crate) fn in_text(e: rusqlite::Error, sql: &str, start: usize) -> DbError {
    match e {
        rusqlite::Error::SqlInputError { error, msg, offset, .. } => from_code(error.extended_code, msg, position(sql, start, offset)),
        other => from_sqlite(other),
    }
}

/// `e`, where no statement text gives a position.
pub(crate) fn from_sqlite(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(error, message) => {
            from_code(error.extended_code, message.unwrap_or_else(|| ffi::code_to_str(error.extended_code).to_owned()), None)
        }
        rusqlite::Error::SqlInputError { error, msg, .. } => from_code(error.extended_code, msg, None),
        rusqlite::Error::MultipleStatement => DbError::Other("SQLite reads more than one statement where dbee reads one".into()),
        rusqlite::Error::InvalidParameterCount(..) => DbError::Other(PARAMETERS.into()),
        other => DbError::Other(other.to_string()),
    }
}

/// The error of result code `extended` (an extended code, or a primary one) with SQLite's
/// `message`.
fn from_code(extended: c_int, message: String, position: Option<u32>) -> DbError {
    match CODES.iter().find(|(code, _)| *code == extended).map(|(_, becomes)| *becomes) {
        Some(Becomes::Cancelled) => DbError::Cancelled,
        Some(Becomes::ReadOnly) => DbError::ReadOnly { message },
        None => DbError::Server(Box::new(ServerError {
            code: extended.to_string(),
            code_label: Some(CODE_LABEL),
            message,
            detail: None,
            hint: (primary(extended) == ffi::SQLITE_AUTH).then(|| OWN_FILE_ONLY.to_owned()),
            position,
            meaning: None,
        })),
    }
}

/// The primary result code of an extended one: its low byte.
fn primary(code: c_int) -> c_int {
    code & 0xFF
}

/// `e` is the server's error with primary result code `code`.
pub(crate) fn is_code(e: &DbError, code: c_int) -> bool {
    matches!(e, DbError::Server(s) if s.code.parse::<c_int>().is_ok_and(|c| primary(c) == code))
}

/// The 1-based character position of byte `offset` of the statement at byte `start` of `sql`;
/// `None` when SQLite's offset is not a character's start in it.
fn position(sql: &str, start: usize, offset: c_int) -> Option<u32> {
    let at = start.checked_add(usize::try_from(offset).ok()?)?;
    let chars = sql.get(..at)?.chars().count();
    u32::try_from(chars.checked_add(1)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(extended: c_int, message: &str) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(ffi::Error::new(extended), Some(message.into()))
    }

    #[test]
    fn result_codes_dbee_acts_on() {
        assert_eq!(from_sqlite(failure(ffi::SQLITE_INTERRUPT, "interrupted")), DbError::Cancelled);
        assert_eq!(from_sqlite(failure(ffi::SQLITE_READONLY, "attempt to write")), DbError::ReadOnly { message: "attempt to write".into() });
        // An extended code says more than "read-only": the file moved, a lock it cannot take.
        let moved = from_sqlite(failure(ffi::SQLITE_READONLY_DBMOVED, "attempt to write a readonly database"));
        assert_eq!((moved.code(), moved.to_string().as_str()), ("server", "attempt to write a readonly database (SQLite error 1032)"));
        let busy = from_sqlite(failure(ffi::SQLITE_BUSY, "database is locked"));
        assert_eq!((busy.code(), busy.to_string().as_str()), ("server", "database is locked (SQLite error 5)"));
        let unique = from_sqlite(failure(ffi::SQLITE_CONSTRAINT_UNIQUE, "UNIQUE constraint failed: t.a"));
        assert_eq!(unique.to_string(), "UNIQUE constraint failed: t.a (SQLite error 2067)");
        assert!(
            is_code(&unique, ffi::SQLITE_CONSTRAINT) && !is_code(&unique, ffi::SQLITE_ERROR) && !is_code(&DbError::Cancelled, ffi::SQLITE_INTERRUPT)
        );
        // Without SQLite's message, its text for the code.
        let bare = from_sqlite(rusqlite::Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_FULL), None));
        assert_eq!(bare.to_string(), "database or disk is full (SQLite error 13)");
        // Refused by the authorizer of a run whose SQL may open no other file: the hint says why.
        let hint = |e: DbError| if let DbError::Server(s) = e { s.hint } else { None };
        let refused = hint(from_sqlite(failure(ffi::SQLITE_AUTH, "not authorized")));
        assert_eq!(refused.as_deref(), Some("This isolated run lets SQL open no other file, so ATTACH and VACUUM are refused."));
        assert_eq!(hint(unique), None);
    }

    #[test]
    fn errors_of_the_library_itself() {
        assert_eq!(from_sqlite(rusqlite::Error::MultipleStatement).code(), "other");
        assert_eq!(from_sqlite(rusqlite::Error::InvalidParameterCount(0, 1)), DbError::Other(PARAMETERS.into()));
        assert_eq!(from_sqlite(rusqlite::Error::InvalidColumnIndex(3)).code(), "other");
    }

    #[test]
    fn positions_count_characters_from_the_texts_start() {
        let input = |sql: &str, offset| rusqlite::Error::SqlInputError {
            error: ffi::Error::new(ffi::SQLITE_ERROR),
            msg: "no such column: nope".into(),
            sql: sql.into(),
            offset,
        };
        let at = |e: DbError| match e {
            DbError::Server(s) => s.position,
            other => panic!("{other:?}"),
        };
        assert_eq!(at(in_text(input("SELECT nope", 7), "SELECT nope", 0)), Some(8));
        // é is two bytes and one character; the statement starts after `SELECT 1; `.
        let sql = "SELECT 1; SELECT 'é', nope";
        assert_eq!(at(in_text(input("SELECT 'é', nope", 13), sql, 10)), Some(23));
        // An offset inside a character, past the end or negative gives no position.
        assert_eq!(at(in_text(input("é", 1), "é", 0)), None);
        assert_eq!(at(in_text(input("x", 9), "x", 0)), None);
        assert_eq!(at(in_text(input("x", -1), "x", 0)), None);
        assert_eq!(at(from_sqlite(input("SELECT nope", 7))), None);
    }
}

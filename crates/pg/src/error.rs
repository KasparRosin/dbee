//! The mapping from tokio-postgres errors to [`DbError`].

use dbee_driver::{DbError, MAX_MESSAGE, Meaning, ServerError, kept_text};
use tokio_postgres::error::{DbError as PgServerError, ErrorPosition, Severity};

use crate::watch::Refusal;

/// What a Postgres error code is called in the error text.
const SQLSTATE: &str = "SQLSTATE";

/// When an error happened: while connecting (startup, login, the session setup) or on an open
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Connect,
    Session,
}

// tokio-postgres keeps its error kinds private; `Display` of an error is exactly its kind's text
// (tokio-postgres 0.7.18, `error/mod.rs`), and the cause is its `source()`.
const KIND_IO: &str = "error communicating with the server";
const KIND_TLS: &str = "error performing TLS handshake";
const KIND_CONNECT: &str = "error connecting to server";
const KIND_TIMEOUT: &str = "timeout waiting for server";
const KIND_AUTH: &str = "authentication error";
const KIND_CONFIG: &str = "invalid configuration";
const KIND_UNEXPECTED: &str = "unexpected message from server";
const KIND_PARSE: &str = "error parsing response from server";

/// Map a tokio-postgres error. The result never quotes the statement text.
pub(crate) fn map(e: &tokio_postgres::Error, phase: Phase) -> DbError {
    if let Some(db) = e.as_db_error() {
        return from_server(db, phase);
    }
    if let Some(refusal) = Refusal::of(e) {
        return refusal.error();
    }
    let kind = e.to_string();
    let cause = std::error::Error::source(e).map(ToString::to_string);
    let because = || cause.clone().unwrap_or_else(|| kind.clone());
    if e.is_closed() {
        return match phase {
            Phase::Session => DbError::ConnectionLost,
            Phase::Connect => DbError::Connect("the server closed the connection".into()),
        };
    }
    match (kind.as_str(), phase) {
        (KIND_TLS, _) => DbError::Tls(because()),
        (KIND_IO, Phase::Session) => DbError::ConnectionLost,
        (KIND_IO | KIND_CONNECT | KIND_TIMEOUT, _) => DbError::Connect(because()),
        (KIND_AUTH, _) => DbError::Auth { message: because() },
        (KIND_CONFIG, _) if cause.as_deref() == Some("password missing") => {
            DbError::Auth { message: "the server asks for a password, and none was given".into() }
        }
        (KIND_UNEXPECTED | KIND_PARSE, Phase::Connect) => {
            DbError::Connect(format!("the server did not answer like a Postgres server ({})", because()))
        }
        _ => DbError::Other(match cause {
            Some(cause) => format!("{kind}: {cause}"),
            None => kind,
        }),
    }
}

/// The server's error, its message, detail and hint each cut to [`MAX_MESSAGE`].
fn from_server(db: &PgServerError, phase: Phase) -> DbError {
    let code = kept_text(db.code().code(), MAX_MESSAGE);
    let code = code.as_str();
    let message = kept_text(db.message(), MAX_MESSAGE);
    match code {
        "28P01" | "28000" => return DbError::Auth { message },
        "57014" => return DbError::Cancelled,
        "25006" => return DbError::ReadOnly { message },
        _ => {}
    }
    // FATAL and PANIC end the session (a killed backend, a server shutting down, an idle timeout).
    // While connecting they are the normal way to refuse, so they keep their message there.
    if phase == Phase::Session && matches!(db.parsed_severity(), Some(Severity::Fatal | Severity::Panic)) {
        return DbError::ConnectionLost;
    }
    let position = match db.position() {
        Some(ErrorPosition::Original(p)) => Some(*p),
        // A position in a query the server generated internally means nothing to the user.
        Some(ErrorPosition::Internal { .. }) | None => None,
    };
    let meaning = match code {
        "25P02" => Some(Meaning::TransactionAborted),
        _ => None,
    };
    DbError::Server(Box::new(ServerError {
        code: code.to_owned(),
        code_label: Some(SQLSTATE),
        message,
        detail: db.detail().map(|d| kept_text(d, MAX_MESSAGE)),
        hint: db.hint().map(|h| kept_text(h, MAX_MESSAGE)),
        position,
        meaning,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_kind_text_is_what_we_match() {
        // The one tokio-postgres error that can be built outside it: proves that `Display` is the
        // bare kind text the mapping compares with.
        let e = tokio_postgres::Error::__private_api_timeout();
        assert_eq!(e.to_string(), KIND_TIMEOUT);
        assert_eq!(map(&e, Phase::Connect), DbError::Connect(KIND_TIMEOUT.into()));
        assert_eq!(map(&e, Phase::Session).code(), "connect");
    }
}

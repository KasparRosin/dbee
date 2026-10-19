//! [`PgError`] and the mapping from tokio-postgres errors.

use dbee_ssh::SshError;
use tokio_postgres::error::{DbError, ErrorPosition, Severity};

/// Why a connection or a statement failed.
///
/// The text of [`PgError::Server`], [`PgError::Auth`] and [`PgError::ReadOnly`] comes from the
/// server and can quote values (a duplicate key, a bad literal), so it is shown to the user and
/// never logged: logs carry [`PgError::code`] only. No variant ever holds a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgError {
    /// The server reported an error. `position` is the 1-based character position in the
    /// statement text, when the server gave one.
    Server { code: String, message: String, detail: Option<String>, hint: Option<String>, position: Option<u32> },
    /// The server refused the login (SQLSTATE `28P01` or `28000`), or asked for a password and
    /// none was given.
    Auth { message: String },
    /// The server could not be reached, or did not answer like a Postgres server.
    Connect(String),
    /// TLS could not be set up: no TLS on the server, a certificate that is not trusted, or an
    /// unusable CA file.
    Tls(String),
    /// The SSH tunnel failed.
    Ssh(SshError),
    /// The connection broke after it was open (server restart, killed backend, network gone).
    ConnectionLost,
    /// The statement was cancelled (SQLSTATE `57014`).
    Cancelled,
    /// A write on a read-only connection (SQLSTATE `25006`).
    ReadOnly { message: String },
    /// Statement `statement` (0-based) of an apply did not change exactly one row.
    RowNotFound { statement: usize },
    /// Anything else.
    Other(String),
}

impl PgError {
    /// A stable id for the UI, the control channel and logs.
    pub fn code(&self) -> &'static str {
        match self {
            PgError::Server { .. } => "server",
            PgError::Auth { .. } => "auth",
            PgError::Connect(_) => "connect",
            PgError::Tls(_) => "tls",
            PgError::Ssh(_) => "ssh",
            PgError::ConnectionLost => "connectionLost",
            PgError::Cancelled => "cancelled",
            PgError::ReadOnly { .. } => "readOnly",
            PgError::RowNotFound { .. } => "rowNotFound",
            PgError::Other(_) => "other",
        }
    }
}

impl std::fmt::Display for PgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PgError::Server { code, message, detail, hint, .. } => {
                write!(f, "{message} (SQLSTATE {code})")?;
                if let Some(detail) = detail {
                    write!(f, "\nDetail: {detail}")?;
                }
                if let Some(hint) = hint {
                    write!(f, "\nHint: {hint}")?;
                }
                Ok(())
            }
            PgError::Auth { message } => write!(f, "the server refused the login: {message}"),
            PgError::Connect(m) => write!(f, "could not connect to the server: {m}"),
            PgError::Tls(m) => write!(f, "could not set up TLS: {m}"),
            PgError::Ssh(e) => write!(f, "{e}"),
            PgError::ConnectionLost => write!(f, "the connection to the server was lost"),
            PgError::Cancelled => write!(f, "the statement was cancelled"),
            PgError::ReadOnly { message } => write!(f, "the connection is read-only: {message}"),
            PgError::RowNotFound { statement } => write!(
                f,
                "change {} did not match exactly one row (it may have been changed or deleted since it was loaded), so nothing was written",
                statement.saturating_add(1)
            ),
            PgError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for PgError {}

impl From<SshError> for PgError {
    fn from(e: SshError) -> Self {
        PgError::Ssh(e)
    }
}

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
pub(crate) fn map(e: &tokio_postgres::Error, phase: Phase) -> PgError {
    if let Some(db) = e.as_db_error() {
        return from_db(db, phase);
    }
    let kind = e.to_string();
    let cause = std::error::Error::source(e).map(ToString::to_string);
    let because = || cause.clone().unwrap_or_else(|| kind.clone());
    if e.is_closed() {
        return match phase {
            Phase::Session => PgError::ConnectionLost,
            Phase::Connect => PgError::Connect("the server closed the connection".into()),
        };
    }
    match (kind.as_str(), phase) {
        (KIND_TLS, _) => PgError::Tls(because()),
        (KIND_IO, Phase::Session) => PgError::ConnectionLost,
        (KIND_IO | KIND_CONNECT | KIND_TIMEOUT, _) => PgError::Connect(because()),
        (KIND_AUTH, _) => PgError::Auth { message: because() },
        (KIND_CONFIG, _) if cause.as_deref() == Some("password missing") => {
            PgError::Auth { message: "the server asks for a password, and none was given".into() }
        }
        (KIND_UNEXPECTED | KIND_PARSE, Phase::Connect) => {
            PgError::Connect(format!("the server did not answer like a Postgres server ({})", because()))
        }
        _ => PgError::Other(match cause {
            Some(cause) => format!("{kind}: {cause}"),
            None => kind,
        }),
    }
}

fn from_db(db: &DbError, phase: Phase) -> PgError {
    let code = db.code().code();
    let message = db.message().to_owned();
    match code {
        "28P01" | "28000" => return PgError::Auth { message },
        "57014" => return PgError::Cancelled,
        "25006" => return PgError::ReadOnly { message },
        _ => {}
    }
    // FATAL and PANIC end the session (a killed backend, a server shutting down, an idle timeout).
    // While connecting they are the normal way to refuse, so they keep their message there.
    if phase == Phase::Session && matches!(db.parsed_severity(), Some(Severity::Fatal | Severity::Panic)) {
        return PgError::ConnectionLost;
    }
    let position = match db.position() {
        Some(ErrorPosition::Original(p)) => Some(*p),
        // A position in a query the server generated internally means nothing to the user.
        Some(ErrorPosition::Internal { .. }) | None => None,
    };
    PgError::Server { code: code.to_owned(), message, detail: db.detail().map(str::to_owned), hint: db.hint().map(str::to_owned), position }
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
        assert_eq!(map(&e, Phase::Connect), PgError::Connect(KIND_TIMEOUT.into()));
        assert_eq!(map(&e, Phase::Session).code(), "connect");
    }

    #[test]
    fn row_not_found_counts_from_one() {
        let e = PgError::RowNotFound { statement: 0 };
        assert!(e.to_string().starts_with("change 1 "), "{e}");
        assert!(PgError::RowNotFound { statement: usize::MAX }.to_string().contains(&usize::MAX.to_string()));
    }

    #[test]
    fn server_text_has_detail_and_hint() {
        let e = PgError::Server {
            code: "23505".into(),
            message: "duplicate key".into(),
            detail: Some("Key (id)=(1) already exists.".into()),
            hint: Some("h".into()),
            position: None,
        };
        assert_eq!(e.to_string(), "duplicate key (SQLSTATE 23505)\nDetail: Key (id)=(1) already exists.\nHint: h");
    }
}

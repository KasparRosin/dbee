//! [`DbError`]: why a connection or a statement failed, the same type for every driver.

use dbee_sql::Refusal;
use dbee_ssh::SshError;

/// What a server error means, for the code that acts on it, so none of it matches an engine's
/// own codes. A new meaning is a new variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meaning {
    /// A statement failed earlier in the transaction, which stays open and refuses every
    /// statement until it is rolled back.
    TransactionAborted,
}

/// An error the server reported, with its own `code` (Postgres's SQLSTATE, say), which the text
/// shows after `code_label` when the driver gives one. `position` is the 1-based character
/// position in the statement text, when the server gave one; `meaning` is set when the driver
/// knows what the error means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    pub code: String,
    pub code_label: Option<&'static str>,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub position: Option<u32>,
    pub meaning: Option<Meaning>,
}

impl ServerError {
    /// The message with the code after it, labelled when the driver gives a label:
    /// `column "x" does not exist (SQLSTATE 42703)`.
    pub fn message_with_code(&self) -> String {
        match self.code_label {
            Some(label) => format!("{} ({label} {})", self.message, self.code),
            None => format!("{} ({})", self.message, self.code),
        }
    }
}

/// Why a connection or a statement failed.
///
/// The text of [`DbError::Server`], [`DbError::Auth`] and [`DbError::ReadOnly`] comes from the
/// server and can quote values (a duplicate key, a bad literal), so it is shown to the user and
/// never logged: logs carry [`DbError::code`] only. No variant ever holds a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbError {
    /// The server reported an error.
    Server(Box<ServerError>),
    /// The server refused the login, or asked for a password and none was given.
    Auth { message: String },
    /// The server could not be reached, or did not answer like the server it should be.
    Connect(String),
    /// TLS could not be set up: no TLS on the server, a certificate that is not trusted, or an
    /// unusable CA file.
    Tls(String),
    /// The SSH tunnel failed.
    Ssh(SshError),
    /// The connection broke after it was open (server restart, killed session, network gone).
    ConnectionLost,
    /// The statement was cancelled.
    Cancelled,
    /// A write on a read-only connection.
    ReadOnly { message: String },
    /// Statement `statement` (0-based) of an apply did not change exactly one row.
    RowNotFound { statement: usize },
    /// The run may not reach it: a file outside an isolated run's allowed folders.
    NotAllowed(String),
    /// Anything else.
    Other(String),
}

impl DbError {
    /// A stable id for the UI, the control channel and logs.
    pub fn code(&self) -> &'static str {
        match self {
            DbError::Server(_) => "server",
            DbError::Auth { .. } => "auth",
            DbError::Connect(_) => "connect",
            DbError::Tls(_) => "tls",
            DbError::Ssh(_) => "ssh",
            DbError::ConnectionLost => "connectionLost",
            DbError::Cancelled => "cancelled",
            DbError::ReadOnly { .. } => "readOnly",
            DbError::RowNotFound { .. } => "rowNotFound",
            DbError::NotAllowed(_) => "config",
            DbError::Other(_) => "other",
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Server(e) => {
                write!(f, "{}", e.message_with_code())?;
                if let Some(detail) = &e.detail {
                    write!(f, "\nDetail: {detail}")?;
                }
                if let Some(hint) = &e.hint {
                    write!(f, "\nHint: {hint}")?;
                }
                Ok(())
            }
            DbError::Auth { message } => write!(f, "the server refused the login: {message}"),
            DbError::Connect(m) => write!(f, "could not connect to the server: {m}"),
            DbError::Tls(m) => write!(f, "could not set up TLS: {m}"),
            DbError::Ssh(e) => write!(f, "{e}"),
            DbError::ConnectionLost => write!(f, "the connection was lost"),
            DbError::Cancelled => write!(f, "the statement was cancelled"),
            DbError::ReadOnly { message } => write!(f, "the connection is read-only: {message}"),
            DbError::RowNotFound { statement } => write!(
                f,
                "change {} did not match exactly one row (it may have been changed or deleted since it was loaded), so nothing was written",
                statement.saturating_add(1)
            ),
            DbError::NotAllowed(m) | DbError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<SshError> for DbError {
    fn from(e: SshError) -> Self {
        DbError::Ssh(e)
    }
}

/// A statement dbee does not send: on a read-only connection the user's mode, else something
/// that would make the server misread what follows.
impl From<Refusal> for DbError {
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::ReadOnly(message) => DbError::ReadOnly { message: message.into() },
            Refusal::Misread(message) => DbError::Other(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(code: &str, code_label: Option<&'static str>) -> ServerError {
        ServerError { code: code.into(), code_label, message: "m".into(), detail: None, hint: None, position: None, meaning: None }
    }

    fn boxed(e: ServerError) -> DbError {
        DbError::Server(Box::new(e))
    }

    #[test]
    fn server_code_suffix() {
        assert_eq!(boxed(server("42703", Some("SQLSTATE"))).to_string(), "m (SQLSTATE 42703)");
        assert_eq!(boxed(server("SQLITE_BUSY", None)).to_string(), "m (SQLITE_BUSY)");
        // Five characters say nothing on their own: only the driver's label names a SQLSTATE.
        assert_eq!(boxed(server("00942", None)).to_string(), "m (00942)");
        assert_eq!(
            (server("42703", Some("SQLSTATE")).message_with_code(), server("SQLITE_BUSY", None).message_with_code()),
            ("m (SQLSTATE 42703)".to_owned(), "m (SQLITE_BUSY)".to_owned())
        );
    }

    #[test]
    fn server_text_has_detail_and_hint() {
        let e = boxed(ServerError {
            message: "duplicate key".into(),
            detail: Some("Key (id)=(1) already exists.".into()),
            hint: Some("h".into()),
            ..server("23505", Some("SQLSTATE"))
        });
        assert_eq!(e.to_string(), "duplicate key (SQLSTATE 23505)\nDetail: Key (id)=(1) already exists.\nHint: h");
        let DbError::Server(s) = &e else { panic!("a server error") };
        assert_eq!(s.message_with_code(), "duplicate key (SQLSTATE 23505)", "without the detail and the hint");
    }

    #[test]
    fn a_lost_connection_names_no_server() {
        assert_eq!(DbError::ConnectionLost.to_string(), "the connection was lost");
    }

    #[test]
    fn row_not_found_counts_from_one() {
        let e = DbError::RowNotFound { statement: 0 };
        assert!(e.to_string().starts_with("change 1 "), "{e}");
        assert!(DbError::RowNotFound { statement: usize::MAX }.to_string().contains(&usize::MAX.to_string()));
    }

    #[test]
    fn error_codes() {
        let all = [
            (boxed(server("42703", Some("SQLSTATE"))), "server"),
            (DbError::Auth { message: "m".into() }, "auth"),
            (DbError::Connect("m".into()), "connect"),
            (DbError::Tls("m".into()), "tls"),
            (DbError::Ssh(dbee_ssh::SshError::NoKey), "ssh"),
            (DbError::ConnectionLost, "connectionLost"),
            (DbError::Cancelled, "cancelled"),
            (DbError::ReadOnly { message: "m".into() }, "readOnly"),
            (DbError::RowNotFound { statement: 0 }, "rowNotFound"),
            (DbError::NotAllowed("m".into()), "config"),
            (DbError::Other("m".into()), "other"),
        ];
        for (e, code) in all {
            assert_eq!(e.code(), code);
            assert!(!e.to_string().is_empty());
        }
        let aborted = boxed(ServerError { meaning: Some(Meaning::TransactionAborted), ..server("25P02", Some("SQLSTATE")) });
        assert_eq!((aborted.code(), aborted.to_string().as_str()), ("server", "m (SQLSTATE 25P02)"));
        assert_eq!(DbError::from(dbee_ssh::SshError::NoKey), DbError::Ssh(dbee_ssh::SshError::NoKey));
    }

    #[test]
    fn errors_stay_under_clippys_large_error_limit() {
        let (db, apply) = (std::mem::size_of::<DbError>(), std::mem::size_of::<crate::ApplyError>());
        println!("DbError {db} bytes, ApplyError {apply} bytes");
        assert!(db < 128 && apply < 128, "DbError {db}, ApplyError {apply}");
    }
}

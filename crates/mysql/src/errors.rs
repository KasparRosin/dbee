//! The MySQL and MariaDB error numbers dbee acts on, and an ERR packet as a [`DbError`].

use dbee_driver::{DbError, MAX_MESSAGE, Meaning, ServerError, kept_bytes};
use mysql_common::constants::CapabilityFlags;
use mysql_common::io::ParseBuf;
use mysql_common::packets::ErrPacket;

use crate::flavor::Flavor;

const ER_ACCESS_DENIED_ERROR: u16 = 1045;
const ER_SERVER_SHUTDOWN: u16 = 1053;
const ER_QUERY_INTERRUPTED: u16 = 1317;
const ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION: u16 = 1792;
/// MariaDB's: the connection was killed.
const ER_CONNECTION_KILLED: u16 = 1927;
/// MySQL's: the connection was idle past `wait_timeout`.
const ER_CLIENT_INTERACTION_TIMEOUT: u16 = 4031;
/// The server has no session variable of that name.
pub(crate) const ER_UNKNOWN_SYSTEM_VARIABLE: u16 = 1193;

/// What an error number makes of the error, other than a plain [`DbError::Server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Becomes {
    Auth,
    /// The server's error, marked [`Meaning::Interrupted`]: a `KILL QUERY`, dbee's or another
    /// session's.
    Interrupted,
    ReadOnly,
    /// The server ends the session.
    ConnectionLost,
}

/// The error numbers dbee acts on, and the flavor they mean that on (`None`: both; the numbers
/// from 1900 and 4000 differ between the two).
const ERRNOS: &[(u16, Option<Flavor>, Becomes)] = &[
    (ER_ACCESS_DENIED_ERROR, None, Becomes::Auth),
    (ER_SERVER_SHUTDOWN, None, Becomes::ConnectionLost),
    (ER_QUERY_INTERRUPTED, None, Becomes::Interrupted),
    (ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION, None, Becomes::ReadOnly),
    (ER_CONNECTION_KILLED, Some(Flavor::MariaDb), Becomes::ConnectionLost),
    (ER_CLIENT_INTERACTION_TIMEOUT, Some(Flavor::MySql), Becomes::ConnectionLost),
];

/// `e` is the server's error `errno`.
pub(crate) fn is_errno(e: &DbError, errno: u16) -> bool {
    matches!(e, DbError::Server(s) if s.code == errno.to_string())
}

/// The ERR packet `payload` (header `0xFF` included) as a [`DbError`], its message cut to
/// [`MAX_MESSAGE`].
pub(crate) fn from_err_packet(payload: &[u8], caps: CapabilityFlags, flavor: Flavor) -> DbError {
    match ParseBuf(payload).parse::<ErrPacket>(caps) {
        Ok(ErrPacket::Error(e)) => from_errno(e.error_code(), kept_bytes(e.message_ref(), MAX_MESSAGE), flavor),
        Ok(ErrPacket::Progress(_)) => DbError::Other("the server sent a progress report dbee did not ask for".into()),
        Err(_) => DbError::Other("the server sent a malformed error packet".into()),
    }
}

/// Error `errno` with the server's `message`. The SQLSTATE is left out: MySQL gives `HY000` for
/// most errors, and the error number is what its users look up.
fn from_errno(errno: u16, message: String, flavor: Flavor) -> DbError {
    let known = ERRNOS.iter().find(|(n, on, _)| *n == errno && on.is_none_or(|on| on == flavor));
    let meaning = match known.map(|(_, _, becomes)| *becomes) {
        Some(Becomes::Auth) => return DbError::Auth { message },
        Some(Becomes::ReadOnly) => return DbError::ReadOnly { message },
        Some(Becomes::ConnectionLost) => return DbError::ConnectionLost,
        Some(Becomes::Interrupted) => Some(Meaning::Interrupted),
        None => None,
    };
    DbError::Server(Box::new(ServerError {
        code: errno.to_string(),
        code_label: Some(flavor.code_label()),
        message,
        detail: None,
        hint: None,
        position: None,
        meaning,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_packet(errno: u16, message: &str) -> Vec<u8> {
        [&[0xFF][..], &errno.to_le_bytes(), b"#HY000", message.as_bytes()].concat()
    }

    #[test]
    fn errnos_dbee_acts_on() {
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41;
        let map = |errno, flavor| from_err_packet(&err_packet(errno, "the message"), caps, flavor);
        assert_eq!(map(1045, Flavor::MySql), DbError::Auth { message: "the message".into() });
        // The server's interrupt is its error, marked: only the engine makes it a cancel.
        let interrupted = map(1317, Flavor::MariaDb);
        assert!(matches!(&interrupted, DbError::Server(s) if s.code == "1317" && s.meaning == Some(Meaning::Interrupted)), "{interrupted:?}");
        assert_eq!((interrupted.code(), interrupted.to_string().as_str()), ("server", "the message (MariaDB error 1317)"));
        assert!(matches!(map(1146, Flavor::MySql), DbError::Server(s) if s.meaning.is_none()));
        assert_eq!(map(1792, Flavor::MySql), DbError::ReadOnly { message: "the message".into() });
        assert_eq!(map(1146, Flavor::MySql).to_string(), "the message (MySQL error 1146)");
        assert_eq!(map(1146, Flavor::MariaDb).to_string(), "the message (MariaDB error 1146)");
        // The server ends the session; numbers past 1900 mean other things on the other flavor.
        assert_eq!((map(1053, Flavor::MySql), map(1053, Flavor::MariaDb)), (DbError::ConnectionLost, DbError::ConnectionLost));
        assert_eq!((map(4031, Flavor::MySql), map(1927, Flavor::MariaDb)), (DbError::ConnectionLost, DbError::ConnectionLost));
        assert_eq!((map(4031, Flavor::MariaDb).code(), map(1927, Flavor::MySql).code()), ("server", "server"));
    }

    #[test]
    fn is_errno_reads_the_servers_number() {
        let e = from_err_packet(&err_packet(1193, "Unknown system variable"), CapabilityFlags::CLIENT_PROTOCOL_41, Flavor::MySql);
        assert!(is_errno(&e, ER_UNKNOWN_SYSTEM_VARIABLE) && !is_errno(&e, 1194));
        assert!(!is_errno(&DbError::Cancelled, ER_QUERY_INTERRUPTED));
        let interrupted = from_err_packet(&err_packet(1317, "interrupted"), CapabilityFlags::CLIENT_PROTOCOL_41, Flavor::MySql);
        assert!(is_errno(&interrupted, ER_QUERY_INTERRUPTED));
    }

    #[test]
    fn an_error_before_the_handshake_has_no_sqlstate() {
        let packet = [&[0xFF][..], &1040u16.to_le_bytes(), b"Too many connections"].concat();
        let e = from_err_packet(&packet, CapabilityFlags::empty(), Flavor::MySql);
        assert_eq!(e.to_string(), "Too many connections (MySQL error 1040)");
    }

    #[test]
    fn a_malformed_error_packet_is_other() {
        for bad in [&[0xFF][..], &[0xFF, 1]] {
            assert_eq!(from_err_packet(bad, CapabilityFlags::CLIENT_PROTOCOL_41, Flavor::MySql).code(), "other", "{bad:?}");
        }
    }
}

//! What runs around each statement on a connection: the statements dbee does not send
//! ([`Dialect::refusal`]), the check that `standard_conforming_strings` is still on before a
//! statement whose `'…'` strings hold a backslash, and, on a connection dbee made read-only, the
//! check after each statement that it still is. Before an Apply's statements, the check that
//! `client_encoding` is still UTF8 ([`utf8`]), and again before its `COMMIT` (`apply.rs`).
//!
//! The read-only check reads both settings with `SHOW`, which takes no snapshot, so a transaction
//! the user began can still set its isolation level after it. When the check cannot run (the
//! transaction failed, whose setting changes end with it, or a cancel stopped the check) it runs
//! before the next statement instead.

use std::sync::atomic::Ordering;

use dbee_driver::{DbError, Limits, Meaning};
use dbee_sql::Dialect;

use crate::{PgConnection, query};

/// Both `on` while the session and its transaction are read-only.
const READ_MODE: &str = "SHOW default_transaction_read_only; SHOW transaction_read_only";

const TURNED_OFF: &str = "the statement turned read-only off, so dbee rolled back the open transaction and turned read-only on again. To write, turn \
                          Read-only off in the connection's settings.";

const TURNED_OFF_BEFORE: &str = "an earlier statement turned read-only off, so dbee rolled back the open transaction, turned read-only on again \
                                 and did not run this statement. Run it again; to write, turn Read-only off in the connection's settings.";

const NOT_RESTORED: &str = "read-only was turned off and dbee could not turn it on again, so it closed the connection. Reconnect to go on.";

const CONFORMING_OFF: &str = "standard_conforming_strings was off in this session, so the server would read the backslashes in this statement's \
                              strings differently from how dbee shows them. dbee turned it on again and did not run the statement; run it again.";

/// What reading the mode back found.
enum Mode {
    ReadOnly,
    Changed,
    /// A cancel stopped the read.
    Stopped,
    /// The transaction failed: nothing runs in it but its end, which undoes its setting changes.
    Aborted,
    Failed(DbError),
}

/// Before a statement runs, with the connection's lock held.
pub(crate) async fn before(conn: &PgConnection, sql: &str) -> Result<(), DbError> {
    if let Some(refused) = Dialect::Postgres.refusal(sql, conn.read_only) {
        return Err(refused.into());
    }
    if conn.read_only && conn.check_due.load(Ordering::Acquire) {
        match mode(conn).await {
            Mode::ReadOnly => conn.check_due.store(false, Ordering::Release),
            Mode::Changed => return Err(restore(conn, TURNED_OFF_BEFORE).await),
            Mode::Stopped => return Err(DbError::Cancelled),
            Mode::Aborted => {}
            Mode::Failed(e) => return Err(e),
        }
    }
    if Dialect::Postgres.backslash_in_plain_string(sql) && !conforming(conn).await? {
        conn.client().batch_execute("SET standard_conforming_strings = on").await.map_err(|e| conn.error(&e))?;
        return Err(DbError::Other(CONFORMING_OFF.into()));
    }
    Ok(())
}

/// After a statement ran, with the lock still held: on a read-only connection, the mode read back.
pub(crate) async fn after<T>(conn: &PgConnection, ran: Result<T, DbError>) -> Result<T, DbError> {
    if !conn.read_only || ran.as_ref().err() == Some(&DbError::ConnectionLost) {
        return ran;
    }
    match mode(conn).await {
        Mode::ReadOnly => {
            conn.check_due.store(false, Ordering::Release);
            ran
        }
        Mode::Changed => Err(restore(conn, TURNED_OFF).await),
        Mode::Stopped | Mode::Aborted => {
            conn.check_due.store(true, Ordering::Release);
            ran
        }
        Mode::Failed(e) => Err(e),
    }
}

async fn mode(conn: &PgConnection) -> Mode {
    match read(conn, READ_MODE).await {
        Ok(cells) => {
            let on: Vec<bool> = cells.iter().map(|cell| cell.as_deref() == Some("on")).collect();
            if on == [true, true] { Mode::ReadOnly } else { Mode::Changed }
        }
        Err(DbError::Cancelled) => Mode::Stopped,
        Err(DbError::Server(s)) if s.meaning == Some(Meaning::TransactionAborted) => Mode::Aborted,
        Err(other) => Mode::Failed(other),
    }
}

/// A setting or two the guard reads ([`query::own_read`]): short values, one row each.
async fn read(conn: &PgConnection, sql: &str) -> Result<Vec<Option<String>>, DbError> {
    query::own_read(conn.client(), sql, Limits { max_rows: 2, max_bytes: 4 * 1024 }, |e| conn.error(e)).await
}

/// Rolls back what is open and turns read-only on again; the error to report, `said` when it
/// worked. When it did not, the connection is closed.
async fn restore(conn: &PgConnection, said: &str) -> DbError {
    let client = conn.client();
    let set = async {
        client.batch_execute("ROLLBACK").await?;
        client.batch_execute("SET default_transaction_read_only = on").await
    };
    if set.await.is_ok() && matches!(mode(conn).await, Mode::ReadOnly) {
        conn.check_due.store(false, Ordering::Release);
        log::info!("pg: a statement turned read-only off; turned it on again");
        return DbError::ReadOnly { message: said.into() };
    }
    conn.mark_closed();
    DbError::Other(NOT_RESTORED.into())
}

const ENCODING_CHANGED: &str = "client_encoding was not UTF8 in this session (a function can change it), so the server would have read \
                                these changes as other text. dbee set it back to UTF8 and wrote nothing; apply again.";

pub(crate) const ENCODING_CHANGED_DURING: &str = "client_encoding changed during the Apply (a trigger on the table can change it), so the \
                                                  server would have read the later changes as other text. dbee rolled them back, set it \
                                                  back to UTF8 and wrote nothing; apply again. Should it change again, ask the table's \
                                                  owner about its triggers.";

/// The session's `client_encoding` is UTF8, in which dbee writes every value.
pub(crate) async fn is_utf8(conn: &PgConnection) -> Result<bool, DbError> {
    Ok(read(conn, "SHOW client_encoding").await?.iter().any(|cell| cell.as_deref() == Some("UTF8")))
}

/// Sets the session's `client_encoding` back to UTF8.
pub(crate) async fn set_utf8(conn: &PgConnection) -> Result<(), DbError> {
    conn.client().batch_execute("SET client_encoding = 'UTF8'").await.map_err(|e| conn.error(&e))?;
    log::info!("pg: client_encoding had changed; set it back to UTF8");
    Ok(())
}

/// Before an Apply's statements, with the lock held: the session's `client_encoding` is still
/// UTF8; when a function changed it, it is set back, and the Apply writes nothing.
pub(crate) async fn utf8(conn: &PgConnection) -> Result<(), DbError> {
    if is_utf8(conn).await? {
        return Ok(());
    }
    set_utf8(conn).await?;
    Err(DbError::Other(ENCODING_CHANGED.into()))
}

/// The session still has `standard_conforming_strings` on.
async fn conforming(conn: &PgConnection) -> Result<bool, DbError> {
    Ok(read(conn, "SHOW standard_conforming_strings").await?.iter().any(|cell| cell.as_deref() == Some("on")))
}

//! Writing staged changes: every statement in one transaction, or none.

use crate::query::{Limits, run_locked};
use crate::{PgConnection, PgError};

/// Why an apply wrote nothing: the 0-based `index` of the statement that failed and why.
///
/// The text never includes statement text or values; [`PgError`]'s own text can quote a server
/// message, so callers that log use `error.code()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyError {
    pub index: usize,
    pub error: PgError,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "change {} failed, nothing was written: {}", self.index.saturating_add(1), self.error)
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Marks the connection closed if dropped before [`finish`](Self::finish): the apply future was
/// cancelled between `BEGIN` and its end, so the server may still hold an open transaction and
/// a later statement on this connection would run inside it. A closed connection is reported as
/// lost and the engine reconnects, which discards the transaction.
struct OpenTransaction<'a> {
    conn: &'a PgConnection,
    armed: bool,
}

impl OpenTransaction<'_> {
    fn finish(&mut self) {
        self.armed = false;
    }
}

impl Drop for OpenTransaction<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.conn.mark_closed();
        }
    }
}

const NO_ROWS: Limits = Limits { max_rows: 0, max_bytes: 0 };

/// Run `statements` in one transaction and return the rows they changed in total.
///
/// Each statement must change exactly one row, else it is [`PgError::RowNotFound`] for its
/// index. Any failure rolls back; a `ROLLBACK` that itself fails is [`PgError::ConnectionLost`].
/// The connection lock is held from `BEGIN` to the end. An empty list sends nothing.
pub async fn apply(conn: &PgConnection, statements: &[String]) -> Result<u64, ApplyError> {
    if statements.is_empty() {
        return Ok(0);
    }
    let _lock = conn.lock().await;
    let mut open = OpenTransaction { conn, armed: true };
    let last = statements.len() - 1;
    let outcome = write_all(conn, statements).await;
    let (index, error) = match outcome {
        Ok(()) => match run_locked(conn, "COMMIT", NO_ROWS).await {
            Ok(_) => {
                open.finish();
                return Ok(statements.len() as u64);
            }
            Err(e) => (last, e),
        },
        Err((index, e)) => (index, e),
    };
    match run_locked(conn, "ROLLBACK", NO_ROWS).await {
        Ok(_) => {
            open.finish();
            Err(ApplyError { index, error })
        }
        Err(_) => {
            conn.mark_closed();
            open.finish();
            Err(ApplyError { index, error: PgError::ConnectionLost })
        }
    }
}

/// `BEGIN` and every statement; the error carries the failing statement's index (`BEGIN`'s is 0).
async fn write_all(conn: &PgConnection, statements: &[String]) -> Result<(), (usize, PgError)> {
    run_locked(conn, "BEGIN", NO_ROWS).await.map_err(|e| (0, e))?;
    for (i, sql) in statements.iter().enumerate() {
        let result = run_locked(conn, sql, NO_ROWS).await.map_err(|e| (i, e))?;
        if result.rows_affected != Some(1) {
            return Err((i, PgError::RowNotFound { statement: i }));
        }
    }
    Ok(())
}

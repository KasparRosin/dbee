//! Writing staged changes: every statement in one transaction, or none.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::query::{Limits, run_locked};
use crate::{PgConnection, PgError};

/// Whether an apply may still be cancelled, shared by the apply and whoever cancels it.
///
/// A cancel counts only while the apply's statements before `COMMIT` run: [`ApplyGate::cancel`]
/// marks it, and the apply then sends no further statement and no `COMMIT` but rolls back (the
/// caller also sends a cancel request, which stops a statement that waits, on a lock say). Once
/// the apply has decided to commit, `cancel` refuses and the apply goes on to its end, so a cancel
/// never meets the `COMMIT` and the outcome stays known.
#[derive(Debug, Default)]
pub struct ApplyGate(AtomicU8);

const WRITING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;

impl ApplyGate {
    /// Marks the apply cancelled. True while its statements before `COMMIT` run (also when it was
    /// cancelled already): the caller then sends the cancel request. False once the apply decided
    /// to commit: nothing may be sent then.
    pub fn cancel(&self) -> bool {
        match self.0.compare_exchange(WRITING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => true,
            Err(state) => state == CANCELLED,
        }
    }

    /// The apply decided to commit: `COMMIT` is on its way, or done.
    pub fn committing(&self) -> bool {
        self.0.load(Ordering::SeqCst) == COMMITTING
    }

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst) == CANCELLED
    }

    /// Decides to commit; false when the apply was cancelled first.
    fn commit(&self) -> bool {
        self.0.compare_exchange(WRITING, COMMITTING, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }
}

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
        let n = self.index.saturating_add(1);
        match self.error {
            PgError::ConnectionLost => write!(f, "the connection was lost while writing change {n}; the outcome is unknown, so re-read the rows"),
            PgError::RowNotFound { .. } => write!(f, "{}", self.error),
            PgError::Cancelled => write!(f, "the Apply was cancelled at change {n}, so nothing was written"),
            _ => write!(f, "change {n} failed, so nothing was written: {}", self.error),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Marks the connection closed if dropped before [`finish`](Self::finish): the apply future was
/// cancelled between `BEGIN` and its end, so the server may still hold an open transaction.
/// Every later operation on the connection is refused with `ConnectionLost`; the socket and the
/// server-side transaction stay open until the `PgConnection` is dropped, so the engine must
/// drop it and reconnect (which discards the transaction).
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
/// index, and each must be exactly one SQL statement (checked before anything is sent). A
/// failing `COMMIT` (a deferred constraint, say) is reported at the last statement's index. A
/// `ConnectionLost` during `COMMIT` leaves the outcome unknown. Any failure rolls back; a `ROLLBACK` that itself fails is [`PgError::ConnectionLost`].
/// The connection lock is held from `BEGIN` to the end. An empty list sends nothing.
pub async fn apply(conn: &PgConnection, statements: &[String]) -> Result<u64, ApplyError> {
    apply_gated(conn, statements, &ApplyGate::default()).await
}

/// [`apply`] that `gate` can cancel until it decides to commit ([`ApplyGate`]). A cancelled apply
/// rolls back and fails with [`PgError::Cancelled`] at the statement it stopped at (the last one
/// when every statement had run).
pub async fn apply_gated(conn: &PgConnection, statements: &[String], gate: &ApplyGate) -> Result<u64, ApplyError> {
    if statements.is_empty() {
        return Ok(0);
    }
    for (i, sql) in statements.iter().enumerate() {
        if dbee_sql::lexer::statements(dbee_sql::Dialect::Postgres, sql).len() != 1 {
            return Err(ApplyError { index: i, error: PgError::Other("a change must be exactly one SQL statement".into()) });
        }
    }
    let _lock = conn.lock().await.map_err(|error| ApplyError { index: 0, error })?;
    let mut open = OpenTransaction { conn, armed: true };
    let last = statements.len() - 1;
    let outcome = write_all(conn, statements, gate).await;
    let (index, error) = match outcome {
        Ok(()) if !gate.commit() => (last, PgError::Cancelled),
        Ok(()) => match run_locked(conn, "COMMIT", NO_ROWS).await {
            Ok(_) => {
                open.finish();
                return Ok(statements.len() as u64);
            }
            Err(e) => (last, e),
        },
        Err((index, e)) => (index, e),
    };
    if roll_back(conn).await {
        open.finish();
        Err(ApplyError { index, error })
    } else {
        conn.mark_closed();
        open.finish();
        Err(ApplyError { index, error: PgError::ConnectionLost })
    }
}

/// `ROLLBACK`; false when it failed. A cancel request can arrive after the statement it was
/// meant for ended, during the `ROLLBACK`: one more `ROLLBACK` then ends the transaction.
async fn roll_back(conn: &PgConnection) -> bool {
    for _ in 0..2 {
        match run_locked(conn, "ROLLBACK", NO_ROWS).await {
            Ok(_) => return true,
            Err(PgError::Cancelled) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `BEGIN` and every statement, until `gate` is cancelled; the error carries the failing
/// statement's index (`BEGIN`'s is 0).
async fn write_all(conn: &PgConnection, statements: &[String], gate: &ApplyGate) -> Result<(), (usize, PgError)> {
    run_locked(conn, "BEGIN", NO_ROWS).await.map_err(|e| (0, e))?;
    for (i, sql) in statements.iter().enumerate() {
        if gate.cancelled() {
            return Err((i, PgError::Cancelled));
        }
        let result = run_locked(conn, sql, NO_ROWS).await.map_err(|e| (i, e))?;
        if result.rows_affected != Some(1) {
            return Err((i, PgError::RowNotFound { statement: i }));
        }
    }
    Ok(())
}

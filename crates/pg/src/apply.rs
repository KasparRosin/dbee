//! Writing staged changes: every statement in one transaction, or none.

use dbee_driver::{ApplyError, ApplyGate, DbError, Limits};

use crate::PgConnection;
use crate::query::run_locked;

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
/// Each statement must change exactly one row, else it is [`DbError::RowNotFound`] for its
/// index, and each must be exactly one SQL statement (checked before anything is sent). A
/// failing `COMMIT` (a deferred constraint, say) is reported at the last statement's index. A
/// `ConnectionLost` during `COMMIT` leaves the outcome unknown. Any failure rolls back; a `ROLLBACK` that itself fails is [`DbError::ConnectionLost`].
/// The connection lock is held from `BEGIN` to the end. An empty list sends nothing.
pub async fn apply(conn: &PgConnection, statements: &[String]) -> Result<u64, ApplyError> {
    apply_gated(conn, statements, &ApplyGate::default()).await
}

/// [`apply`] that `gate` can cancel until it decides to commit ([`ApplyGate`]). A cancelled apply
/// rolls back and fails with [`DbError::Cancelled`] at the statement it stopped at (the last one
/// when every statement had run).
pub async fn apply_gated(conn: &PgConnection, statements: &[String], gate: &ApplyGate) -> Result<u64, ApplyError> {
    if statements.is_empty() {
        return Ok(0);
    }
    for (i, sql) in statements.iter().enumerate() {
        if dbee_sql::lexer::statements(dbee_sql::Dialect::Postgres, sql).len() != 1 {
            return Err(ApplyError { index: i, error: DbError::Other("a change must be exactly one SQL statement".into()) });
        }
    }
    let _lock = conn.lock().await.map_err(|error| ApplyError { index: 0, error })?;
    let mut open = OpenTransaction { conn, armed: true };
    let last = statements.len() - 1;
    let outcome = write_all(conn, statements, gate).await;
    let (index, error) = match outcome {
        Ok(()) if !gate.commit() => (last, DbError::Cancelled),
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
        Err(ApplyError { index, error: DbError::ConnectionLost })
    }
}

/// `ROLLBACK`; false when it failed. A cancel request can arrive after the statement it was
/// meant for ended, during the `ROLLBACK`: one more `ROLLBACK` then ends the transaction.
async fn roll_back(conn: &PgConnection) -> bool {
    for _ in 0..2 {
        match run_locked(conn, "ROLLBACK", NO_ROWS).await {
            Ok(_) => return true,
            Err(DbError::Cancelled) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `BEGIN` and every statement, until `gate` is cancelled; the error carries the failing
/// statement's index (`BEGIN`'s is 0).
async fn write_all(conn: &PgConnection, statements: &[String], gate: &ApplyGate) -> Result<(), (usize, DbError)> {
    run_locked(conn, "BEGIN", NO_ROWS).await.map_err(|e| (0, e))?;
    for (i, sql) in statements.iter().enumerate() {
        if gate.cancelled() {
            return Err((i, DbError::Cancelled));
        }
        let result = run_locked(conn, sql, NO_ROWS).await.map_err(|e| (i, e))?;
        if result.rows_affected != Some(1) {
            return Err((i, DbError::RowNotFound { statement: i }));
        }
    }
    Ok(())
}

//! Writing staged changes: every statement in one transaction, or none, by the shared algorithm
//! in `dbee-driver` ([`dbee_driver::apply::apply`]).

use dbee_driver::{ApplyError, ApplyGate, ApplySession, DbError, Limits};
use dbee_sql::Dialect;
use tokio::sync::MutexGuard;

use crate::query::run_locked;
use crate::{PgConnection, guard};

/// The connection with its lock held, for one apply. Marking it lost refuses every later
/// operation with `ConnectionLost`; the socket and the server-side transaction stay open until
/// the `PgConnection` is dropped.
struct Locked<'a> {
    conn: &'a PgConnection,
    _guard: MutexGuard<'a, ()>,
}

impl ApplySession for Locked<'_> {
    const BEGIN: &'static str = "BEGIN";

    async fn execute(&mut self, sql: &str) -> Result<Option<u64>, DbError> {
        run_locked(self.conn, sql, NO_ROWS).await.map(|r| r.rows_affected)
    }

    fn mark_lost(&self) {
        self.conn.mark_closed();
    }
}

/// The connection for one apply, its lock taken and its `client_encoding` UTF8 ([`guard::utf8`]).
async fn locked(conn: &PgConnection) -> Result<Locked<'_>, DbError> {
    let guard = conn.lock().await?;
    guard::utf8(conn).await?;
    Ok(Locked { conn, _guard: guard })
}

const NO_ROWS: Limits = Limits { max_rows: 0, max_bytes: 0 };

/// Run `statements` in one transaction and return the rows they changed in total, as
/// [`dbee_driver::apply::apply`] describes. The connection lock is held from `BEGIN` to the
/// end. An empty list sends nothing.
pub async fn apply(conn: &PgConnection, statements: &[String]) -> Result<u64, ApplyError> {
    apply_gated(conn, statements, &ApplyGate::default()).await
}

/// [`apply`] that `gate` can cancel until it decides to commit ([`ApplyGate`]). A cancelled apply
/// rolls back and fails with [`DbError::Cancelled`] at the statement it stopped at (the last one
/// when every statement had run).
pub async fn apply_gated(conn: &PgConnection, statements: &[String], gate: &ApplyGate) -> Result<u64, ApplyError> {
    dbee_driver::apply::apply(Dialect::Postgres, statements, gate, locked(conn)).await
}

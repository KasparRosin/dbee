//! Writing staged changes: every statement in one transaction, or none, by the shared algorithm
//! in `dbee-driver` ([`dbee_driver::apply::apply`]).

use dbee_driver::{ApplyError, ApplyGate, ApplySession, DbError, Limits, QUERY_LIMITS};
use dbee_sql::Dialect;
use tokio::sync::MutexGuard;

use crate::connect::{MySqlConnection, Session};

/// No rows are kept: an apply's statements change rows, and return none.
const NO_ROWS: Limits = Limits { max_rows: 0, ..QUERY_LIMITS };

/// The connection's session, locked for one apply.
struct Locked<'a> {
    conn: &'a MySqlConnection,
    session: MutexGuard<'a, Session>,
}

impl ApplySession for Locked<'_> {
    const BEGIN: &'static str = "START TRANSACTION";

    async fn execute(&mut self, sql: &str) -> Result<Option<u64>, DbError> {
        self.conn.statement(&mut self.session, sql, NO_ROWS).await.map(|r| r.rows_affected)
    }

    fn mark_lost(&self) {
        self.conn.mark_closed();
    }
}

impl MySqlConnection {
    /// Run `statements` in one transaction and return the rows they changed in total, as
    /// [`dbee_driver::apply::apply`] describes; `gate` can cancel it until it decides to commit.
    /// A statement's rows are those it matched, as dbee logs in with `CLIENT_FOUND_ROWS`, so an
    /// update to the value a row already holds counts. The session is held from `START
    /// TRANSACTION` to the end; an empty list sends nothing.
    pub async fn apply(&self, statements: &[String], gate: &ApplyGate) -> Result<u64, ApplyError> {
        let locked = async { Ok(Locked { conn: self, session: self.lock().await? }) };
        dbee_driver::apply::apply(Dialect::MySql, statements, gate, locked).await
    }
}

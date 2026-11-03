//! Writing staged changes: every statement in one transaction, or none, by the shared algorithm
//! in `dbee-driver` ([`dbee_driver::apply::apply`]), one blocking call per statement, so the gate
//! is read between them.

use dbee_driver::{ApplyError, ApplyGate, ApplySession, DbError, Limits};
use dbee_sql::Dialect;
use tokio::sync::MutexGuard;

use crate::connect::SqliteConnection;
use crate::query::{Columns, run_text};

/// No rows are kept: an apply's statements change rows, and return none.
const NO_ROWS: Limits = Limits { max_rows: 0, max_bytes: 0 };

/// The connection with its turn held, for one apply.
struct Locked<'a> {
    conn: &'a SqliteConnection,
    _turn: MutexGuard<'a, ()>,
}

impl ApplySession for Locked<'_> {
    /// Takes the file's write lock at once, so a locked file fails the apply before any change.
    const BEGIN: &'static str = "BEGIN IMMEDIATE";

    async fn execute(&mut self, sql: &str) -> Result<Option<u64>, DbError> {
        let sql = sql.to_owned();
        self.conn
            .blocking(move |c, running| {
                // SQLite ends the transaction itself when an interrupt stops a write; nothing to undo.
                if sql == "ROLLBACK" && c.is_autocommit() {
                    return Ok(None);
                }
                run_text(c, running, &sql, NO_ROWS, Columns::Unnamed).map(|r| r.rows_affected)
            })
            .await
    }

    fn mark_lost(&self) {
        self.conn.mark_closed();
    }
}

impl SqliteConnection {
    /// Run `statements` in one transaction and return the rows they changed in total, as
    /// [`dbee_driver::apply::apply`] describes; `gate` can cancel it until it decides to commit.
    /// A statement's rows are those SQLite's `changes()` counts, which an update to the value a
    /// row already holds counts too. The turn is held from `BEGIN IMMEDIATE` to the end; a file
    /// another connection holds locked fails after the busy timeout. An empty list sends nothing.
    pub async fn apply(&self, statements: &[String], gate: &ApplyGate) -> Result<u64, ApplyError> {
        let locked = async { Ok(Locked { conn: self, _turn: self.lock().await? }) };
        dbee_driver::apply::apply(Dialect::Sqlite, statements, gate, locked).await
    }
}

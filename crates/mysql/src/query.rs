//! Running SQL text: one `COM_QUERY`, then the text result sets, with values as the server
//! sends them.

use dbee_core::{Column, ResultSet};
use dbee_driver::{DbError, Limits, QUERY_LIMITS, TransactionState};

use crate::connect::{MySqlConnection, Session};

/// Marks the connection closed when dropped before [`Closes::disarm`]: a statement dropped
/// half-way leaves unread answers on the wire.
struct Closes<'a>(Option<&'a MySqlConnection>);

impl Closes<'_> {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for Closes<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.0 {
            conn.mark_closed();
        }
    }
}

/// An error the server reported leaves the session in step, and so does a result whose columns
/// did not fit, which was read to its end; any other leaves it unusable.
fn session_survives(e: &DbError) -> bool {
    matches!(e, DbError::Server(_) | DbError::Auth { .. } | DbError::Cancelled | DbError::ReadOnly { .. } | DbError::TooLarge { .. })
}

impl MySqlConnection {
    /// Run `sql` as one `COM_QUERY`, holding the connection lock.
    ///
    /// The result holds the last row-returning result set's columns, typed from their column
    /// definitions, and rows; `rows_affected` is the affected (matched) rows of the last OK
    /// packet, `None` when every result returned rows. The first error ends the run. Rows past
    /// `limits` are still read (so the connection stays in step) and counted in `rows_seen`, but
    /// not kept, and `complete` becomes false. The byte cap counts what the columns and the rows
    /// kept hold ([`dbee_driver::cell_cost`]); columns that do not fit end the run in
    /// [`DbError::TooLarge`], once its answer is read to the end. A statement that dbee's
    /// `KILL QUERY` targeted and that still returned rows (MySQL's `SELECT SLEEP(…)` returns 1
    /// when killed) is [`DbError::Cancelled`]; one that a KILL ended in an error is the server's
    /// interrupt (1317), whoever sent it. What dbee refuses is not sent, and a read-only
    /// connection's mode is read back ([`crate::guard`]).
    pub async fn run(&self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        let mut session = self.lock().await?;
        self.before(&mut session, sql).await?;
        let ran = self.statement(&mut session, sql, limits).await;
        self.after(&mut session, ran).await
    }

    /// [`MySqlConnection::run`], with the columns untyped (table pages take them from the
    /// catalog).
    pub async fn run_untyped(&self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        let mut result = self.run(sql, limits).await?;
        for column in &mut result.columns {
            *column = Column::untyped(&column.name);
        }
        Ok(result)
    }

    /// The first cell of the first row as an unsigned integer (a count).
    pub async fn scalar_u64(&self, sql: &str) -> Result<u64, DbError> {
        let result = self.run_untyped(sql, Limits { max_rows: 1, ..QUERY_LIMITS }).await?;
        let cell = result.rows.first().and_then(|row| row.first()).and_then(|c| c.as_deref());
        cell.and_then(|text| text.trim().parse::<u64>().ok()).ok_or_else(|| DbError::Other("the query did not return a count".into()))
    }

    /// Whether the session is in a transaction: as the last command's OK or EOF packet said, else
    /// (an error's packet says nothing of it) as the server answers a ping. `Unknown` on a closed
    /// connection, and when the ping fails, which closes it.
    pub async fn transaction(&self) -> TransactionState {
        let Ok(mut session) = self.lock().await else { return TransactionState::Unknown };
        let in_transaction = match session.in_transaction {
            Some(known) => known,
            None => {
                let closes = Closes(Some(self));
                let pinged = session.ping().await;
                closes.disarm();
                match pinged {
                    Ok(known) => known,
                    Err(_) => {
                        self.mark_closed();
                        return TransactionState::Unknown;
                    }
                }
            }
        };
        if in_transaction { TransactionState::Open } else { TransactionState::Idle }
    }

    /// Run one statement on `session`, which the caller locked, as the cancel's statement
    /// counter's next statement. An error other than the server's closes the connection.
    pub(crate) async fn statement(&self, session: &mut Session, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        let n = self.statements.begin().await;
        let closes = Closes(Some(self));
        let result = session.query(sql, limits).await;
        let killed = self.statements.end(n);
        closes.disarm();
        match result {
            Ok(r) if killed && !r.columns.is_empty() => Err(DbError::Cancelled),
            Ok(r) => Ok(r),
            Err(e) => {
                if !session_survives(&e) {
                    self.mark_closed();
                }
                Err(e)
            }
        }
    }
}

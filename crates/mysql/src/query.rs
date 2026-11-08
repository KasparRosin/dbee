//! Running SQL text: one `COM_QUERY`, then the text result sets, with values as the server
//! sends them.

use dbee_core::{Column, ResultSet};
use dbee_driver::{DbError, Limits, QUERY_LIMITS};

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

/// An error the server reported leaves the session in step; any other leaves it unusable.
fn session_survives(e: &DbError) -> bool {
    matches!(e, DbError::Server(_) | DbError::Auth { .. } | DbError::Cancelled | DbError::ReadOnly { .. })
}

impl MySqlConnection {
    /// Run `sql` as one `COM_QUERY`, holding the connection lock.
    ///
    /// The result holds the last row-returning result set's columns, typed from their column
    /// definitions, and rows; `rows_affected` is the affected (matched) rows of the last OK
    /// packet, `None` when every result returned rows. The first error ends the run. Rows past
    /// `limits` are still read (so the connection stays in step) and counted in `rows_seen`, but
    /// not kept, and `complete` becomes false. The byte cap counts the column names and the
    /// cell text kept. A statement that a cancel's `KILL QUERY` targeted and that still returned
    /// rows (`SELECT SLEEP(…)` returns 1 when killed) is [`DbError::Cancelled`]. What dbee
    /// refuses is not sent, and a read-only connection's mode is read back ([`crate::guard`]).
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

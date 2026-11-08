//! What runs around each statement on a connection dbee made read-only: the statements dbee does
//! not send ([`Dialect::refusal`]), and the session's read-only variable read back after each
//! statement; when a statement turned it off, dbee rolls back, turns it on again and says so.
//!
//! On MySQL reading a variable clears what `SHOW WARNINGS` lists, so after a statement that left
//! warnings or an error the mode is read before the next statement that is not `SHOW WARNINGS`,
//! `SHOW ERRORS` or `GET DIAGNOSTICS`, which would clear them anyway. So is a read a cancel
//! stopped. Both checks read the statement as this server will ([`gated::as_read`]): a version
//! comment it skips hides no word from them.

use std::sync::atomic::Ordering;

use dbee_driver::{DbError, Limits};
use dbee_sql::Dialect;
use dbee_sql::lexer::{TokenKind, tokenize};

use crate::connect::{MySqlConnection, Session};
use crate::gated;

const TURNED_OFF: &str = "the statement turned read-only off, so dbee rolled back the open transaction and turned read-only on again. To write, \
                          turn Read-only off in the connection's settings.";

const TURNED_OFF_BEFORE: &str = "an earlier statement turned read-only off, so dbee rolled back the open transaction, turned read-only on again \
                                 and did not run this statement. Run it again; to write, turn Read-only off in the connection's settings.";

const NOT_RESTORED: &str = "read-only was turned off and dbee could not turn it on again, so it closed the connection. Reconnect to go on.";

const ONE_ROW: Limits = Limits { max_rows: 1, max_bytes: 1024 };

const TOO_MANY_VERSIONS: &str = "this statement has more version comments than dbee can tell how the server reads, so it does not run it here. \
                                 Split the statement, or turn Read-only off in the connection's settings.";

/// What reading the mode back found.
enum Mode {
    ReadOnly,
    Changed,
    /// A cancel stopped the read.
    Stopped,
    /// The server refused the read: the mode is unknown.
    Unread(DbError),
    Failed(DbError),
}

/// `sql` reads the warnings or errors of the statement before it, and leaves them in place.
fn diagnostic(sql: &str) -> bool {
    let words: Vec<String> = tokenize(Dialect::MySql, sql)
        .into_iter()
        .filter(|t| matches!(t.kind, TokenKind::Keyword | TokenKind::Ident))
        .take(3)
        .filter_map(|t| sql.get(t.range).map(str::to_ascii_lowercase))
        .collect();
    matches!(
        words.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
        ["show", "warnings" | "errors" | "count", ..] | ["get", "diagnostics", ..] | ["get", "current" | "stacked", "diagnostics"]
    )
}

impl MySqlConnection {
    /// `sql` as this server reads it, in each way it may ([`gated::as_read`]).
    fn as_read(&self, sql: &str) -> Result<Vec<String>, DbError> {
        gated::as_read(sql, self.flavor(), self.info().version_num).ok_or_else(|| DbError::ReadOnly { message: TOO_MANY_VERSIONS.into() })
    }

    /// Before a statement runs, with the session locked.
    pub(crate) async fn before(&self, session: &mut Session, sql: &str) -> Result<(), DbError> {
        if !self.read_only {
            return Ok(());
        }
        let readings = self.as_read(sql)?;
        if let Some(refused) = readings.iter().find_map(|r| Dialect::MySql.refusal(r, true)) {
            return Err(refused.into());
        }
        if self.check_due.load(Ordering::Acquire) && !readings.iter().all(|r| diagnostic(r)) {
            match self.mode(session).await {
                Mode::ReadOnly => self.check_due.store(false, Ordering::Release),
                Mode::Changed => return Err(self.restore(session, TURNED_OFF_BEFORE).await),
                Mode::Stopped => return Err(DbError::Cancelled),
                Mode::Unread(e) | Mode::Failed(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// After a statement ran, with the session still locked.
    pub(crate) async fn after<T>(&self, session: &mut Session, sql: &str, ran: Result<T, DbError>) -> Result<T, DbError> {
        if !self.read_only || ran.as_ref().err() == Some(&DbError::ConnectionLost) {
            return ran;
        }
        if self.as_read(sql).is_ok_and(|readings| readings.iter().all(|r| diagnostic(r))) {
            return ran;
        }
        if session.conditions {
            self.check_due.store(true, Ordering::Release);
            return ran;
        }
        match self.mode(session).await {
            Mode::ReadOnly => {
                self.check_due.store(false, Ordering::Release);
                ran
            }
            Mode::Changed => Err(self.restore(session, TURNED_OFF).await),
            Mode::Stopped | Mode::Unread(_) => {
                self.check_due.store(true, Ordering::Release);
                ran
            }
            Mode::Failed(e) => Err(e),
        }
    }

    async fn mode(&self, session: &mut Session) -> Mode {
        let sql = format!("SELECT @@SESSION.{}", session.read_only_variable);
        match self.statement(session, &sql, ONE_ROW).await {
            Ok(r) if r.rows.first().and_then(|row| row.first()).is_some_and(|v| v.as_deref() == Some("1")) => Mode::ReadOnly,
            Ok(_) => Mode::Changed,
            Err(DbError::Cancelled) => Mode::Stopped,
            Err(e @ DbError::Server(_)) => Mode::Unread(e),
            Err(e) => Mode::Failed(e),
        }
    }

    /// Rolls back what is open and turns read-only on again; the error to report, `said` when it
    /// worked. When it did not, the connection is closed.
    async fn restore(&self, session: &mut Session, said: &str) -> DbError {
        let mut set = self.statement(session, "ROLLBACK", ONE_ROW).await.map(drop);
        if set.is_ok() {
            set = self.statement(session, "SET SESSION TRANSACTION READ ONLY", ONE_ROW).await.map(drop);
        }
        if set.is_ok() && matches!(self.mode(session).await, Mode::ReadOnly) {
            self.check_due.store(false, Ordering::Release);
            log::info!("mysql: a statement turned read-only off; turned it on again");
            return DbError::ReadOnly { message: said.into() };
        }
        self.mark_closed();
        DbError::Other(NOT_RESTORED.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_statements_are_told_apart() {
        for sql in [
            "SHOW WARNINGS",
            "show errors limit 5",
            "SHOW COUNT(*) WARNINGS",
            "GET DIAGNOSTICS @n = NUMBER",
            "get current diagnostics condition 1 @m = MESSAGE_TEXT",
            "/* x */ SHOW WARNINGS",
        ] {
            assert!(diagnostic(sql), "{sql}");
        }
        for sql in ["SELECT 1", "SHOW TABLES", "SHOW VARIABLES LIKE 'warnings'", "SELECT 'show warnings'", ""] {
            assert!(!diagnostic(sql), "{sql}");
        }
    }
}

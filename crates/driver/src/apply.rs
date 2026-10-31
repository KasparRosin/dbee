//! Writing staged changes, every engine alike: the all-or-nothing algorithm over an engine's
//! [`ApplySession`], the gate a cancel goes through, and why an apply wrote nothing.

use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};

use dbee_sql::Dialect;

use crate::DbError;

/// Whether an apply may still be cancelled, shared by the apply and whoever cancels it.
///
/// A cancel counts only while the apply's statements before `COMMIT` run: [`ApplyGate::cancel`]
/// marks it, and the apply then sends no further statement and no `COMMIT` but rolls back (the
/// caller also cancels through the connection's [`Cancel`](crate::Cancel), which stops a
/// statement that waits, on a lock say). Once the apply has decided to commit, `cancel` refuses
/// and the apply goes on to its end, so a cancel never meets the `COMMIT` and the outcome stays
/// known.
#[derive(Debug, Default)]
pub struct ApplyGate(AtomicU8);

const WRITING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;

impl ApplyGate {
    /// Marks the apply cancelled. True while its statements before `COMMIT` run (also when it was
    /// cancelled already): the caller then cancels the running statement. False once the apply
    /// decided to commit: nothing may be sent then.
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

    /// The apply was cancelled: the driver sends no further statement.
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst) == CANCELLED
    }

    /// The driver decides to commit, right before it sends `COMMIT`; false when the apply was
    /// cancelled first, and the driver then rolls back.
    pub fn commit(&self) -> bool {
        self.0.compare_exchange(WRITING, COMMITTING, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }
}

/// Why an apply wrote nothing: the 0-based `index` of the statement that failed and why.
///
/// The text never includes statement text or values; [`DbError`]'s own text can quote a server
/// message, so callers that log use `error.code()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyError {
    pub index: usize,
    pub error: DbError,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.index.saturating_add(1);
        match self.error {
            DbError::ConnectionLost => write!(f, "the connection was lost while writing change {n}; the outcome is unknown, so re-read the rows"),
            DbError::RowNotFound { .. } => write!(f, "{}", self.error),
            DbError::Cancelled => write!(f, "the Apply was cancelled at change {n}, so nothing was written"),
            _ => write!(f, "change {n} failed, so nothing was written: {}", self.error),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// An engine's part in [`apply`]: its session, held for the whole apply (the connection's lock
/// taken), on which statements run one at a time.
pub trait ApplySession: Send {
    /// The statement that opens the transaction: `BEGIN`, `START TRANSACTION`.
    const BEGIN: &'static str;

    /// Run one statement; the rows it changed, when the server said.
    fn execute(&mut self, sql: &str) -> impl Future<Output = Result<Option<u64>, DbError>> + Send;

    /// Refuse every later operation on the connection with [`DbError::ConnectionLost`]: the
    /// server may still hold the transaction open, so the caller must drop the connection and
    /// reconnect (which discards the transaction).
    fn mark_lost(&self);
}

/// The session of an apply that has not ended: dropped while armed (the apply future was
/// dropped between `BEGIN` and its end), it marks the connection lost.
struct Open<S: ApplySession> {
    session: S,
    armed: bool,
}

impl<S: ApplySession> Drop for Open<S> {
    fn drop(&mut self) {
        if self.armed {
            self.session.mark_lost();
        }
    }
}

/// Run `statements` in one transaction on the session `lock` gives, and return the rows they
/// changed in total.
///
/// Each statement must change exactly one row, else it is [`DbError::RowNotFound`] for its
/// index, and each must be exactly one SQL statement in `dialect` (checked before `lock` is
/// awaited, so before anything is sent). `gate` can cancel the apply until it decides to commit
/// ([`ApplyGate`]): a cancelled apply rolls back and fails with [`DbError::Cancelled`] at the
/// statement it stopped at (the last one when every statement had run). A failing `COMMIT` (a
/// deferred constraint, say) is reported at the last statement's index; a `ConnectionLost`
/// during `COMMIT` leaves the outcome unknown. Any failure rolls back; a `ROLLBACK` that itself
/// fails is [`DbError::ConnectionLost`], and the connection is marked lost. The session is held
/// from the transaction's start to the end. An empty list sends nothing.
pub async fn apply<S: ApplySession>(
    dialect: Dialect,
    statements: &[String],
    gate: &ApplyGate,
    lock: impl Future<Output = Result<S, DbError>>,
) -> Result<u64, ApplyError> {
    if statements.is_empty() {
        return Ok(0);
    }
    for (i, sql) in statements.iter().enumerate() {
        if dbee_sql::lexer::statements(dialect, sql).len() != 1 {
            return Err(ApplyError { index: i, error: DbError::Other("a change must be exactly one SQL statement".into()) });
        }
    }
    let session = lock.await.map_err(|error| ApplyError { index: 0, error })?;
    let mut open = Open { session, armed: true };
    let last = statements.len() - 1;
    let outcome = write_all(&mut open.session, statements, gate).await;
    let (index, error) = match outcome {
        Ok(()) if !gate.commit() => (last, DbError::Cancelled),
        Ok(()) => match open.session.execute("COMMIT").await {
            Ok(_) => {
                open.armed = false;
                return Ok(statements.len() as u64);
            }
            Err(e) => (last, e),
        },
        Err((index, e)) => (index, e),
    };
    if roll_back(&mut open.session).await {
        open.armed = false;
        Err(ApplyError { index, error })
    } else {
        open.session.mark_lost();
        open.armed = false;
        Err(ApplyError { index, error: DbError::ConnectionLost })
    }
}

/// `ROLLBACK`; false when it failed. A cancel can arrive after the statement it was meant for
/// ended, during the `ROLLBACK`: one more `ROLLBACK` then ends the transaction.
async fn roll_back<S: ApplySession>(session: &mut S) -> bool {
    for _ in 0..2 {
        match session.execute("ROLLBACK").await {
            Ok(_) => return true,
            Err(DbError::Cancelled) => {}
            Err(_) => return false,
        }
    }
    false
}

/// The transaction's start and every statement, until `gate` is cancelled; the error carries
/// the failing statement's index (the start's is 0).
async fn write_all<S: ApplySession>(session: &mut S, statements: &[String], gate: &ApplyGate) -> Result<(), (usize, DbError)> {
    session.execute(S::BEGIN).await.map_err(|e| (0, e))?;
    for (i, sql) in statements.iter().enumerate() {
        if gate.cancelled() {
            return Err((i, DbError::Cancelled));
        }
        let changed = session.execute(sql).await.map_err(|e| (i, e))?;
        if changed != Some(1) {
            return Err((i, DbError::RowNotFound { statement: i }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_counts_until_the_commit() {
        let gate = ApplyGate::default();
        assert!(!gate.cancelled() && !gate.committing());
        assert!(gate.cancel() && gate.cancel(), "a second cancel is still before the commit");
        assert!(gate.cancelled() && !gate.commit() && !gate.committing());

        let gate = ApplyGate::default();
        assert!(gate.commit() && gate.committing());
        assert!(!gate.cancel() && !gate.cancelled(), "too late once the apply decided to commit");
    }

    #[test]
    fn apply_error_texts() {
        let text = |error| ApplyError { index: 1, error }.to_string();
        assert_eq!(text(DbError::Cancelled), "the Apply was cancelled at change 2, so nothing was written");
        assert!(text(DbError::ConnectionLost).contains("the outcome is unknown"));
        assert!(text(DbError::RowNotFound { statement: 1 }).starts_with("change 2 did not match exactly one row"));
        assert_eq!(text(DbError::Other("no".into())), "change 2 failed, so nothing was written: no");
    }

    use std::collections::VecDeque;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use futures_util::FutureExt;

    /// A session that answers each statement with the next scripted answer (`Ok(Some(1))` once
    /// they run out), records what it was sent, and never answers the statement `hang_on`.
    #[derive(Default)]
    struct Fake {
        sent: Arc<Mutex<Vec<String>>>,
        answers: VecDeque<Result<Option<u64>, DbError>>,
        lost: Arc<AtomicBool>,
        hang_on: Option<&'static str>,
    }

    impl ApplySession for Fake {
        const BEGIN: &'static str = "START";

        fn execute(&mut self, sql: &str) -> impl Future<Output = Result<Option<u64>, DbError>> + Send {
            self.sent.lock().unwrap().push(sql.to_owned());
            let hang = self.hang_on == Some(sql);
            let answer = self.answers.pop_front().unwrap_or(Ok(Some(1)));
            async move {
                if hang {
                    std::future::pending::<()>().await;
                }
                answer
            }
        }

        fn mark_lost(&self) {
            self.lost.store(true, Ordering::SeqCst);
        }
    }

    struct Run {
        result: Option<Result<u64, ApplyError>>,
        sent: Vec<String>,
        lost: bool,
    }

    fn statements(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("UPDATE t SET a = {i} WHERE id = {i}")).collect()
    }

    /// Apply `statements` on a [`Fake`] with `answers`, polled once: every fake answer is ready,
    /// so only a hanging statement leaves it unfinished, and the future is dropped then.
    fn run(statements: &[String], gate: &ApplyGate, answers: Vec<Result<Option<u64>, DbError>>, hang_on: Option<&'static str>) -> Run {
        let fake = Fake { answers: answers.into(), hang_on, ..Fake::default() };
        let (sent, lost) = (Arc::clone(&fake.sent), Arc::clone(&fake.lost));
        let result = apply(Dialect::Postgres, statements, gate, async { Ok(fake) }).now_or_never();
        let sent = sent.lock().unwrap().clone();
        Run { result, sent, lost: lost.load(Ordering::SeqCst) }
    }

    fn sent(r: &Run) -> Vec<&str> {
        r.sent.iter().map(|s| s.split(' ').next().unwrap_or_default()).collect()
    }

    #[test]
    fn every_statement_in_one_transaction() {
        let r = run(&statements(2), &ApplyGate::default(), vec![], None);
        assert_eq!(r.result, Some(Ok(2)));
        assert_eq!(sent(&r), ["START", "UPDATE", "UPDATE", "COMMIT"]);
        assert!(!r.lost);
    }

    #[test]
    fn nothing_is_sent_for_an_empty_list_or_a_text_of_two_statements() {
        let never = async { Err::<Fake, _>(DbError::Other("the lock was taken".into())) };
        assert_eq!(apply(Dialect::Postgres, &[], &ApplyGate::default(), never).now_or_never(), Some(Ok(0)));
        let two = vec!["UPDATE t SET a = 1 WHERE id = 1".to_owned(), "UPDATE t SET a = 1; SELECT 1".to_owned()];
        let r = run(&two, &ApplyGate::default(), vec![], None);
        let Some(Err(e)) = r.result else { panic!("refused") };
        assert_eq!((e.index, e.error.code()), (1, "other"));
        assert!(r.sent.is_empty() && !r.lost);
    }

    #[test]
    fn a_lock_that_fails_fails_the_first_change() {
        let lost = async { Err::<Fake, _>(DbError::ConnectionLost) };
        let e = apply(Dialect::Postgres, &statements(2), &ApplyGate::default(), lost).now_or_never();
        assert_eq!(e, Some(Err(ApplyError { index: 0, error: DbError::ConnectionLost })));
    }

    #[test]
    fn a_change_that_matched_no_row_or_two_rolls_back() {
        for (count, index) in [(Some(0), 1), (Some(2), 1), (None, 1)] {
            let r = run(&statements(3), &ApplyGate::default(), vec![Ok(None), Ok(Some(1)), Ok(count)], None);
            assert_eq!(r.result, Some(Err(ApplyError { index, error: DbError::RowNotFound { statement: index } })), "{count:?}");
            assert_eq!(sent(&r), ["START", "UPDATE", "UPDATE", "ROLLBACK"]);
        }
    }

    #[test]
    fn a_failing_start_or_commit_is_reported_at_its_place() {
        let failed = || Err(DbError::Other("no".into()));
        let r = run(&statements(2), &ApplyGate::default(), vec![failed()], None);
        assert_eq!(r.result.as_ref().map(|r| r.as_ref().map_err(|e| e.index)), Some(Err(0)));
        assert_eq!(sent(&r), ["START", "ROLLBACK"]);
        let r = run(&statements(2), &ApplyGate::default(), vec![Ok(None), Ok(Some(1)), Ok(Some(1)), failed()], None);
        assert_eq!(r.result, Some(Err(ApplyError { index: 1, error: DbError::Other("no".into()) })));
        assert_eq!(sent(&r), ["START", "UPDATE", "UPDATE", "COMMIT", "ROLLBACK"]);
    }

    #[test]
    fn a_cancel_that_meets_the_rollback_rolls_back_again() {
        let answers = vec![Ok(None), Err(DbError::Cancelled), Err(DbError::Cancelled), Ok(None)];
        let r = run(&statements(2), &ApplyGate::default(), answers, None);
        assert_eq!(r.result, Some(Err(ApplyError { index: 0, error: DbError::Cancelled })));
        assert_eq!(sent(&r), ["START", "UPDATE", "ROLLBACK", "ROLLBACK"]);
        assert!(!r.lost);
    }

    #[test]
    fn a_rollback_that_fails_loses_the_connection() {
        for second in [vec![Err(DbError::ConnectionLost)], vec![Err(DbError::Cancelled), Err(DbError::Cancelled)]] {
            let answers = [vec![Ok(None), Err(DbError::Other("no".into()))], second].concat();
            let r = run(&statements(1), &ApplyGate::default(), answers, None);
            assert_eq!(r.result, Some(Err(ApplyError { index: 0, error: DbError::ConnectionLost })));
            assert!(r.lost);
        }
    }

    #[test]
    fn a_cancelled_gate_sends_no_further_change_and_no_commit() {
        let gate = ApplyGate::default();
        assert!(gate.cancel());
        let r = run(&statements(2), &gate, vec![], None);
        assert_eq!(r.result, Some(Err(ApplyError { index: 0, error: DbError::Cancelled })));
        assert_eq!(sent(&r), ["START", "ROLLBACK"]);
        assert!(!gate.committing());
    }

    #[test]
    fn a_dropped_apply_loses_the_connection() {
        for hang in ["START", "UPDATE t SET a = 2 WHERE id = 2", "COMMIT", "ROLLBACK"] {
            let answers = if hang == "ROLLBACK" { vec![Ok(None), Err(DbError::Other("no".into()))] } else { vec![] };
            let r = run(&statements(2), &ApplyGate::default(), answers, Some(hang));
            assert!(r.result.is_none() && r.lost, "{hang}");
        }
        let r = run(&statements(2), &ApplyGate::default(), vec![], None);
        assert!(!r.lost, "a finished apply leaves the connection");
    }
}

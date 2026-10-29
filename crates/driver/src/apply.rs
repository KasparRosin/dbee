//! Writing staged changes, every engine alike: the gate a cancel goes through, and why an
//! apply wrote nothing.

use std::sync::atomic::{AtomicU8, Ordering};

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
}

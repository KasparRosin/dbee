//! The browse connection's queue in the engine. Every tab's pages and counts, the catalog and the
//! Apply share that one connection, which runs one operation at a time, in turn. Each operation
//! waits for its turn here first ([`InTurn`]), so a job knows when its own statement starts: a page,
//! a count or an Apply stopped before then leaves the queue and sends no cancel request, which
//! would stop the statement ahead of it, another tab's ([`stoppable_in_turn`], [`apply_in_turn`]);
//! and a time limit counts from the turn on ([`in_turn`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dbee_core::{Catalog, ResultSet};
use dbee_driver::{ApplyError, ApplyGate, BoxFuture, Cancel, Connection, DbError, DbFuture, Limits, ServerInfo};
use dbee_sql::Dialect;
use tokio::sync::{Mutex, MutexGuard};

use crate::jobs::{self, Stop};

/// A connection whose operations take their turn in order (a fair lock) before the driver's own.
pub(crate) struct InTurn {
    conn: Arc<dyn Connection>,
    turn: Mutex<()>,
    /// Operations waiting for their turn.
    waiting: AtomicUsize,
}

/// Counts one operation as waiting until dropped.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl InTurn {
    pub(crate) fn new(conn: Arc<dyn Connection>) -> Arc<InTurn> {
        Arc::new(InTurn { conn, turn: Mutex::new(()), waiting: AtomicUsize::new(0) })
    }

    /// Waits for the turn, after every operation that asked before.
    pub(crate) async fn turn(&self) -> MutexGuard<'_, ()> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let _waiting = Waiting(&self.waiting);
        self.turn.lock().await
    }

    /// How many operations wait for their turn now.
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Acquire)
    }
}

impl Connection for InTurn {
    fn info(&self) -> &ServerInfo {
        self.conn.info()
    }

    fn dialect(&self) -> Dialect {
        self.conn.dialect()
    }

    fn is_closed(&self) -> bool {
        self.conn.is_closed()
    }

    fn canceller(&self) -> Arc<dyn Cancel> {
        self.conn.canceller()
    }

    fn run<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.run(sql, limits).await
        })
    }

    fn run_untyped<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.run_untyped(sql, limits).await
        })
    }

    fn scalar_u64<'a>(&'a self, sql: &'a str) -> DbFuture<'a, u64> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.scalar_u64(sql).await
        })
    }

    fn load_catalog(&self) -> DbFuture<'_, Catalog> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.load_catalog().await
        })
    }

    fn list_databases(&self) -> DbFuture<'_, Vec<String>> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.list_databases().await
        })
    }

    fn apply<'a>(&'a self, statements: &'a [String], gate: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>> {
        Box::pin(async move {
            let _turn = self.turn().await;
            self.conn.apply(statements, gate).await
        })
    }
}

/// [`jobs::stoppable`] for `statement` on `conn` once its turn comes. Stopped before then, it
/// leaves the queue and ends `Cancelled`, and no cancel request goes out. With its turn, nothing
/// else runs on the connection, so a request reaches its statement or none.
pub(crate) async fn stoppable_in_turn<'a, T, F>(conn: &'a InTurn, statement: impl FnOnce(&'a dyn Connection) -> F, stop: &Stop) -> Result<T, DbError>
where
    F: Future<Output = Result<T, DbError>>,
{
    let turn = tokio::select! {
        biased;
        () = stop.stopped() => return Err(DbError::Cancelled),
        turn = conn.turn() => turn,
    };
    let ended = jobs::stoppable(conn.conn.canceller(), statement(conn.conn.as_ref()), stop).await;
    drop(turn);
    ended
}

/// `operation` on `conn` once its turn comes, so nothing else runs on the connection meanwhile and
/// a limit inside it counts from then.
pub(crate) async fn in_turn<'a, F: Future>(conn: &'a InTurn, operation: impl FnOnce(&'a dyn Connection) -> F) -> F::Output {
    let _turn = conn.turn().await;
    operation(conn.conn.as_ref()).await
}

/// The Apply of `statements` on `conn` once its turn comes. Stopped before then, it leaves the
/// queue and ends `Cancelled` at its first change, with no request sent. With its turn, a stop
/// sends one cancel request for the statement it runs; its gate, cancelled before the stop is
/// set, keeps it from going on.
pub(crate) async fn apply_in_turn(conn: &InTurn, statements: &[String], gate: &ApplyGate, stop: &Stop) -> Result<u64, ApplyError> {
    let turn = tokio::select! {
        biased;
        () = stop.stopped() => return Err(ApplyError { index: 0, error: DbError::Cancelled }),
        turn = conn.turn() => turn,
    };
    let mut applying = std::pin::pin!(conn.conn.apply(statements, gate));
    let written = tokio::select! {
        biased;
        written = &mut applying => written,
        () = stop.stopped() => {
            if conn.conn.canceller().cancel().await.is_err() {
                log::info!("a cancel request was not delivered");
            }
            applying.await
        }
    };
    drop(turn);
    written
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// A connection whose catalog loads at once, and which counts the cancel requests sent to it.
    pub(crate) struct Quick {
        info: ServerInfo,
        cancels: Arc<Cancels>,
    }

    #[derive(Default)]
    pub(crate) struct Cancels(AtomicUsize);

    impl Cancel for Cancels {
        fn cancel(&self) -> DbFuture<'_, ()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    impl Default for Quick {
        fn default() -> Self {
            let info = ServerInfo {
                product: "Quick".into(),
                version: "1".into(),
                version_num: 1,
                read_only: false,
                warnings: Vec::new(),
                tls: None,
                local: true,
            };
            Quick { info, cancels: Arc::default() }
        }
    }

    impl Quick {
        pub(crate) fn cancels(&self) -> usize {
            self.cancels.0.load(Ordering::SeqCst)
        }
    }

    impl Connection for Quick {
        fn info(&self) -> &ServerInfo {
            &self.info
        }
        fn dialect(&self) -> Dialect {
            Dialect::Postgres
        }
        fn is_closed(&self) -> bool {
            false
        }
        fn canceller(&self) -> Arc<dyn Cancel> {
            Arc::clone(&self.cancels) as Arc<dyn Cancel>
        }
        fn run<'a>(&'a self, _: &'a str, _: Limits) -> DbFuture<'a, ResultSet> {
            Box::pin(async { Err(DbError::Other("not in this test".into())) })
        }
        fn run_untyped<'a>(&'a self, _: &'a str, _: Limits) -> DbFuture<'a, ResultSet> {
            Box::pin(async { Err(DbError::Other("not in this test".into())) })
        }
        fn scalar_u64<'a>(&'a self, _: &'a str) -> DbFuture<'a, u64> {
            Box::pin(async { Err(DbError::Other("not in this test".into())) })
        }
        fn load_catalog(&self) -> DbFuture<'_, Catalog> {
            Box::pin(async { Ok(Catalog::default()) })
        }
        fn apply<'a>(&'a self, _: &'a [String], _: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>> {
            Box::pin(async { Err(ApplyError { index: 0, error: DbError::Other("not in this test".into()) }) })
        }
    }
}

//! The browse connection's queue in the engine. Every tab's pages and counts, the catalog and the
//! Apply share that one connection, which runs one operation at a time, in turn. Each operation
//! waits for its turn here first ([`InTurn`]), so a job knows when its own statement starts: a page
//! or count stopped before then leaves the queue and sends no cancel request, which would stop the
//! statement ahead of it, another tab's ([`stoppable_in_turn`]).

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
    async fn turn(&self) -> MutexGuard<'_, ()> {
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

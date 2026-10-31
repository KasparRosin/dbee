//! Cancelling: `KILL QUERY <id>` over a second connection, and the statement numbers that keep
//! a late KILL from reaching the next statement.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dbee_driver::{CONNECT_TIMEOUT, Cancel, DbError, DbFuture};
use tokio::sync::Mutex;

use crate::auth::Login;
use crate::connect::{Route, Session, TlsUse, while_connecting};

/// The statements run on one connection, numbered from 1, and the one a `KILL QUERY` targeted,
/// so a late KILL never cancels the next statement.
#[derive(Default)]
pub(crate) struct Statements {
    last: AtomicU64,
    /// The running statement's number, 0 while none runs.
    running: AtomicU64,
    killed: AtomicU64,
    /// Held while a KILL is on its way: no statement starts meanwhile, so the KILL meets the
    /// statement it was meant for or none.
    kill: Mutex<()>,
}

impl Statements {
    /// A statement starts; its number.
    pub(crate) async fn begin(&self) -> u64 {
        let _no_kill_on_its_way = self.kill.lock().await;
        let n = self.last.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        self.running.store(n, Ordering::SeqCst);
        n
    }

    /// Statement `n` ended; true when a KILL targeted it.
    pub(crate) fn end(&self, n: u64) -> bool {
        let _ = self.running.compare_exchange(n, 0, Ordering::SeqCst, Ordering::SeqCst);
        self.killed.load(Ordering::SeqCst) == n
    }
}

/// What a cancel needs to log in a second time: the route, the login, the TLS the connection
/// ended up with, and whose statement to kill.
pub(crate) struct KillRoute {
    pub(crate) route: Route,
    pub(crate) user: String,
    pub(crate) password: String,
    pub(crate) tls: TlsUse,
    pub(crate) connection_id: u64,
    pub(crate) statements: Arc<Statements>,
}

/// Stops the statement running on one connection with `KILL QUERY <id>`, sent over a second
/// connection (through the tunnel when the connection uses one). Cheap to clone; holds no lock
/// of the connection's.
#[derive(Clone)]
pub struct Canceller(pub(crate) Arc<KillRoute>);

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The route holds the password: not shown.
        f.debug_struct("Canceller").field("connection_id", &self.0.connection_id).field("tunnel", &self.0.route.tunnel.is_some()).finish()
    }
}

impl Canceller {
    /// Kill the statement running now, if one is. `Ok` means the KILL was delivered, or nothing
    /// ran; the statement then fails with [`DbError::Cancelled`], unless it finished first.
    pub async fn cancel(&self) -> Result<(), DbError> {
        // The timeout below needs a runtime; without one it would panic.
        tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("cancelling needs a tokio runtime".into()))?;
        let k = &self.0;
        let sending = async {
            let _gate = k.statements.kill.lock().await;
            if k.statements.running.load(Ordering::SeqCst) == 0 {
                return Ok(false);
            }
            let who = Login { user: &k.user, password: k.password.as_bytes(), database: None };
            let mut session = Session::open(&k.route, &who, &k.tls).await.map_err(while_connecting)?;
            let target = k.statements.running.load(Ordering::SeqCst);
            if target == 0 {
                session.quit().await;
                return Ok(false);
            }
            k.statements.killed.store(target, Ordering::SeqCst);
            let killed = session.query(&format!("KILL QUERY {}", k.connection_id)).await;
            if killed.is_err() {
                let _ = k.statements.killed.compare_exchange(target, 0, Ordering::SeqCst, Ordering::SeqCst);
            }
            session.quit().await;
            killed.map(|_| true)
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, sending).await {
            Ok(Ok(sent)) => {
                log::info!("mysql: {}", if sent { "KILL QUERY sent" } else { "nothing to cancel" });
                Ok(())
            }
            Ok(Err(e)) => {
                log::info!("mysql: cancel failed ({})", e.code());
                Err(e)
            }
            Err(_) => Err(DbError::Connect(format!("the cancel was not delivered within {} s", CONNECT_TIMEOUT.as_secs()))),
        }
    }
}

impl Cancel for Canceller {
    fn cancel(&self) -> DbFuture<'_, ()> {
        Box::pin(Canceller::cancel(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_kill_counts_only_for_the_statement_it_targeted() {
        let s = Statements::default();
        let first = s.begin().await;
        s.killed.store(first, Ordering::SeqCst);
        assert!(s.end(first));
        // The next statement is not the one the KILL targeted.
        let second = s.begin().await;
        assert!(second > first && !s.end(second));
        assert_eq!(s.running.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_statement_starts_while_a_kill_is_on_its_way() {
        let s = Arc::new(Statements::default());
        let gate = s.kill.lock().await;
        let starting = tokio::spawn({
            let s = Arc::clone(&s);
            async move { s.begin().await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(s.running.load(Ordering::SeqCst), 0, "started under the gate");
        drop(gate);
        let n = starting.await.unwrap();
        assert_eq!(s.running.load(Ordering::SeqCst), n);
    }
}

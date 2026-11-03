//! Opening a file: the file read-write or read-only, the session setup and the check that it is a
//! database; the connection, whose calls run on blocking threads, and the canceller that
//! interrupts the call running on it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dbee_driver::{DbError, ServerInfo, Target};
use rusqlite::ffi::ErrorCode;
use rusqlite::{InterruptHandle, OpenFlags};

use crate::errors;

/// How long a statement waits for a file another connection has locked.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Open `t.config.file`: read-write, or read-only when the settings say so; a missing file is made
/// only when `t.create_file` is set (and never read-only).
///
/// The session then turns foreign keys on and waits [`BUSY_TIMEOUT`] for a locked file; a
/// read-only one also runs `PRAGMA query_only = ON`, which keeps it from temporary tables too.
/// `PRAGMA quick_check(1)` turns a file that is not a database, or a damaged one, into
/// [`DbError::Connect`] naming the file. The file is opened by its absolute path, so a name that
/// starts with `file:` is a file name, never a URI.
pub async fn connect(t: Target<'_>) -> Result<SqliteConnection, DbError> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("SQLite calls need a tokio runtime".into()))?;
    let cfg = t.config;
    cfg.validate().map_err(|e| DbError::Connect(e.to_string()))?;
    let file = cfg.file.as_deref().ok_or_else(|| DbError::Connect("no file is set".into()))?;
    let path = std::path::absolute(file).map_err(|e| DbError::Connect(format!("{}: {e}", file.display())))?;
    let (read_only, create) = (cfg.read_only, t.create_file);
    let opened = runtime.spawn_blocking(move || open(&path, read_only, create)).await;
    let (conn, handle) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(e)) => {
            log::info!("sqlite: opening a file failed ({})", e.code());
            return Err(e);
        }
        Err(_) => return Err(DbError::Other("opening the file stopped unexpectedly".into())),
    };
    let version_num = u32::try_from(rusqlite::version_number()).unwrap_or_default();
    let info = ServerInfo { product: "SQLite".into(), version: rusqlite::version().into(), version_num, read_only, warnings: Vec::new() };
    log::info!("sqlite: opened a file (SQLite {version_num}{})", if read_only { ", read-only" } else { "" });
    let running = Arc::new(Running { handle, state: Mutex::new(State::default()) });
    Ok(SqliteConnection {
        shared: Arc::new(Shared { conn: Mutex::new(conn), closed: AtomicBool::new(false), running }),
        info,
        turn: tokio::sync::Mutex::new(()),
    })
}

/// Open the file at `path` and set its session up, on the calling (blocking) thread.
fn open(path: &Path, read_only: bool, create: bool) -> Result<(rusqlite::Connection, InterruptHandle), DbError> {
    let shown = path.display();
    let mode = match (read_only, create) {
        (true, _) => OpenFlags::SQLITE_OPEN_READ_ONLY,
        (false, true) => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        (false, false) => OpenFlags::SQLITE_OPEN_READ_WRITE,
    };
    let conn = rusqlite::Connection::open_with_flags(path, mode | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| {
        let why = e.sqlite_error().map_or_else(|| e.to_string(), |error| rusqlite::ffi::code_to_str(error.extended_code).to_owned());
        match e.sqlite_error_code() {
            Some(ErrorCode::CannotOpen) if matches!(path.try_exists(), Ok(false)) => DbError::Connect(format!("no file at {shown}")),
            _ => DbError::Connect(format!("could not open {shown}: {why}")),
        }
    })?;
    let setup = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => DbError::Connect(format!("{shown} is not a SQLite database")),
        Some(ErrorCode::DatabaseCorrupt) => DbError::Connect(format!("{shown} is damaged: {e}")),
        _ => errors::from_sqlite(e),
    };
    conn.execute_batch("PRAGMA foreign_keys = ON").map_err(setup)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(setup)?;
    if read_only {
        conn.execute_batch("PRAGMA query_only = ON").map_err(setup)?;
    }
    let check = conn
        .query_row("PRAGMA quick_check(1)", [], |row| row.get_ref(0).map(|v| String::from_utf8_lossy(v.as_bytes().unwrap_or_default()).into_owned()));
    match check.map_err(setup)?.as_str() {
        "ok" => {}
        problem => return Err(DbError::Connect(format!("{shown} is damaged: {problem}"))),
    }
    let handle = conn.get_interrupt_handle();
    Ok((conn, handle))
}

/// An open file. Every operation holds the connection's turn for its whole run, so operations
/// never interleave; its SQLite calls run on blocking threads. Dropping the last handle
/// interrupts what still runs on it and closes the file once that has stopped.
pub struct SqliteConnection {
    shared: Arc<Shared>,
    info: ServerInfo,
    turn: tokio::sync::Mutex<()>,
}

/// What a blocking call needs, which outlives the caller's future when that is dropped.
struct Shared {
    conn: Mutex<rusqlite::Connection>,
    /// Set when an operation on it was abandoned half-way (an apply inside its transaction).
    closed: AtomicBool,
    running: Arc<Running>,
}

impl std::fmt::Debug for SqliteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteConnection").field("info", &self.info).field("closed", &self.is_closed()).finish()
    }
}

impl Drop for SqliteConnection {
    fn drop(&mut self) {
        self.shared.running.interrupt();
    }
}

impl SqliteConnection {
    /// What the library said about itself, and whether the file is open read-only.
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// The connection has ended; every further operation fails with
    /// [`DbError::ConnectionLost`].
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Cancels whatever runs on this connection at the time of the call.
    pub fn canceller(&self) -> Canceller {
        Canceller(Arc::clone(&self.shared.running))
    }

    /// The connection's turn, for one operation. Refuses with [`DbError::ConnectionLost`] once the
    /// connection is closed, checked after the turn is taken, so an operation queued behind an
    /// abandoned apply cannot run inside its transaction.
    pub(crate) async fn lock(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, DbError> {
        let turn = self.turn.lock().await;
        if self.is_closed() {
            return Err(DbError::ConnectionLost);
        }
        Ok(turn)
    }

    /// Refuse every later operation with [`DbError::ConnectionLost`]; the file stays open until the
    /// connection is dropped.
    pub(crate) fn mark_closed(&self) {
        self.shared.closed.store(true, Ordering::Release);
    }

    /// Run `work` on the connection on a blocking thread, as one call that a cancel can interrupt.
    /// The caller holds the turn. When the caller's future is dropped, the call still runs to its
    /// end.
    pub(crate) async fn blocking<T, F>(&self, work: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection, &Running) -> Result<T, DbError> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("SQLite calls need a tokio runtime".into()))?;
        let shared = Arc::clone(&self.shared);
        let call = runtime.spawn_blocking(move || {
            // Poisoned only when a call panicked half-way: the session is in an unknown state.
            let conn = shared.conn.lock().map_err(|_| DbError::ConnectionLost)?;
            if shared.closed.load(Ordering::Acquire) {
                return Err(DbError::ConnectionLost);
            }
            let _call = shared.running.start();
            work(&conn, &shared.running)
        });
        call.await.map_err(|_| DbError::Other("the SQLite call stopped unexpectedly".into()))?
    }
}

/// Whether a call runs on a connection, so an interrupt reaches only the call that runs when it
/// is sent. SQLite takes an interrupt sent while no statement runs as nothing, and one sent while
/// the last statement of a call ends is cleared by the next call's first statement: so a cancel
/// never reaches a later call.
pub(crate) struct Running {
    handle: InterruptHandle,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    running: bool,
    interrupted: bool,
}

/// A call that runs; dropping it ends the call.
struct Call<'a>(&'a Running);

impl Drop for Call<'_> {
    fn drop(&mut self) {
        self.0.state().running = false;
    }
}

impl Running {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self) -> Call<'_> {
        *self.state() = State { running: true, interrupted: false };
        Call(self)
    }

    /// Interrupt the call running now; false when none runs.
    pub(crate) fn interrupt(&self) -> bool {
        let mut state = self.state();
        if !state.running {
            return false;
        }
        self.handle.interrupt();
        state.interrupted = true;
        true
    }

    /// The running call was interrupted, so it starts no further statement.
    pub(crate) fn interrupted(&self) -> bool {
        self.state().interrupted
    }
}

/// Stops the call running on one connection with SQLite's interrupt. Cheap to clone; it keeps
/// neither the connection nor the file open.
#[derive(Clone)]
pub struct Canceller(Arc<Running>);

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canceller").field("running", &self.0.state().running).finish()
    }
}

impl Canceller {
    /// Interrupt the statement running now, if one is. `Ok` means it was interrupted, or nothing
    /// ran; the statement then fails with [`DbError::Cancelled`], unless it finished first.
    pub async fn cancel(&self) -> Result<(), DbError> {
        let sent = self.0.interrupt();
        log::info!("sqlite: {}", if sent { "interrupt sent" } else { "nothing to cancel" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A file of this test binary's own under `target/`.
    fn scratch_file(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testkit/sqlite-driver");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}-{}.sqlite", std::process::id()))
    }

    fn running(conn: &rusqlite::Connection) -> Running {
        Running { handle: conn.get_interrupt_handle(), state: Mutex::new(State::default()) }
    }

    #[test]
    fn an_interrupt_reaches_only_a_running_call() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let r = running(&conn);
        assert!(!r.interrupt() && !conn.is_interrupted(), "nothing runs: nothing is sent");
        let call = r.start();
        assert!(r.interrupt() && r.interrupted() && conn.is_interrupted());
        drop(call);
        assert!(!r.interrupt(), "the call ended");
        // The next call starts uninterrupted; SQLite clears the late interrupt at its first statement.
        let _next = r.start();
        assert!(!r.interrupted());
        assert_eq!(conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        assert!(!conn.is_interrupted());
    }

    #[test]
    fn sqlite_takes_an_interrupt_while_idle_as_nothing() {
        // SQLite's own rule, which the design above leans on.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.get_interrupt_handle().interrupt();
        assert!(conn.is_interrupted());
        assert_eq!(conn.query_row("SELECT 2", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
        assert!(!conn.is_interrupted());
    }

    #[test]
    fn opening_sets_the_session_up() {
        let file = scratch_file("open-setup");
        let _ = std::fs::remove_file(&file);
        assert_eq!(open(&file, false, false).err(), Some(DbError::Connect(format!("no file at {}", file.display()))));
        let (conn, _) = open(&file, false, true).unwrap();
        let pragma = |name: &str| conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!((pragma("foreign_keys"), pragma("busy_timeout"), pragma("query_only")), (1, 5000, 0));
        drop(conn);
        let (ro, _) = open(&file, true, false).unwrap();
        assert_eq!(ro.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        drop(ro);
        let _ = std::fs::remove_file(&file);
    }
}

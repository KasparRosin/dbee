//! Opening a file: the file read-write or read-only, the session setup and the check that it is a
//! database; the connection, whose calls run on blocking threads, and the canceller that
//! interrupts the call running on it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dbee_driver::{CONNECT_TIMEOUT, DbError, ServerInfo, Target};
use rusqlite::config::DbConfig;
use rusqlite::ffi::ErrorCode;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{InterruptHandle, OpenFlags};
use tokio::runtime::Handle;

use crate::errors::Refused;
use crate::query::NamesChecked;

/// The pragmas that set a folder SQLite makes its own files in.
const FOLDER_PRAGMAS: [&str; 2] = ["temp_store_directory", "data_store_directory"];

/// How long a statement waits for a file another connection has locked.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Open `t.config.file`: read-write, or read-only when the settings say so; a missing file is made
/// only when `t.create_file` is set (and never read-only).
///
/// A path that exists but is no regular file (a folder, a FIFO, a device), or that ends in a
/// separator, is refused before it is opened. The file opens with SQLite's hardening for files
/// from sources dbee does not know ([`harden`]). The session then turns foreign keys on and waits
/// [`BUSY_TIMEOUT`] for a locked file; a read-only one also runs `PRAGMA query_only = ON`, which
/// keeps it from temporary tables too. A read of `sqlite_schema` turns a file that is not a
/// database, or one whose schema is damaged, into [`DbError::Connect`] naming the file; damage
/// elsewhere shows when it is read. A file the system lets dbee read only opens read-only, and says
/// so; SQL on it may attach no file (`VACUUM INTO` writes one) nor turn `query_only` off. With
/// `t.allowed_dirs` (an isolated run), the file must lie in one of those folders
/// ([`dbee_driver::allowed_file`]), and the path that check resolved is the one opened; SQL may
/// then open no other file: every `ATTACH` is refused, and with it `VACUUM`, which attaches the
/// file it writes, and the pragmas that move SQLite's temporary files. All of it within
/// [`CONNECT_TIMEOUT`].
/// The file is opened by its absolute path, so a name that starts with `file:` is a file name,
/// never a URI.
pub async fn connect(t: Target<'_>) -> Result<SqliteConnection, DbError> {
    let runtime = Handle::try_current().map_err(|_| DbError::Other("SQLite calls need a tokio runtime".into()))?;
    let cfg = t.config;
    cfg.validate().map_err(|e| DbError::Connect(e.to_string()))?;
    let file = cfg.file.as_deref().ok_or_else(|| DbError::Connect("no file is set".into()))?;
    let path = std::path::absolute(file).map_err(|e| DbError::Connect(format!("{}: {e}", file.display())))?;
    let shown = path.display().to_string();
    let (read_only, create, allowed) = (cfg.read_only, t.create_file, t.allowed_dirs.map(<[PathBuf]>::to_vec));
    // An open that outlives the timeout closes its file on its own blocking thread when it ends.
    let opening = runtime.spawn_blocking(move || open(&path, read_only, create, allowed.as_deref()));
    let opened = match tokio::time::timeout(CONNECT_TIMEOUT, opening).await {
        Ok(Ok(opened)) => opened,
        Ok(Err(_)) => Err(DbError::Other("opening the file stopped unexpectedly".into())),
        Err(_) => Err(DbError::Connect(format!("{shown} did not open within {} s", CONNECT_TIMEOUT.as_secs()))),
    };
    let (conn, handle, read_only, refused) = opened.inspect_err(|e| log::info!("sqlite: opening a file failed ({})", e.code()))?;
    let version_num = u32::try_from(rusqlite::version_number()).unwrap_or_default();
    let info = ServerInfo {
        product: "SQLite".into(),
        version: rusqlite::version().into(),
        version_num,
        read_only,
        warnings: Vec::new(),
        tls: None,
        local: true,
    };
    log::info!("sqlite: opened a file (SQLite {version_num}{})", if read_only { ", read-only" } else { "" });
    let running = Arc::new(Running::new(handle));
    let session = Session { refused, ..Session::new(conn) };
    let shared = Shared { conn: Mutex::new(session), closed: AtomicBool::new(false), running: Arc::clone(&running) };
    Ok(SqliteConnection { shared: Some(Arc::new(shared)), running, info, turn: tokio::sync::Mutex::new(()), runtime })
}

/// Open the file at `given` (checked against `allowed` first, when there are allowed folders) and
/// set its session up, on the calling (blocking) thread; the connection, its interrupt handle,
/// whether it is read-only, and where its authorizer says why it refused a statement.
fn open(given: &Path, read_only: bool, create: bool, allowed: Option<&[PathBuf]>) -> Result<Opened, DbError> {
    let shown = given.display();
    let path = match allowed {
        Some(dirs) => dbee_driver::allowed_file(dirs, given)?,
        None => given.to_path_buf(),
    };
    let path = path.as_path();
    if !dbee_driver::plain_file(given) || !dbee_driver::plain_file(path) {
        return Err(DbError::Connect(format!("{shown} is not a file")));
    }
    let conn = rusqlite::Connection::open_with_flags(path, open_flags(read_only, create, allowed.is_some())).map_err(|e| {
        let why = e.sqlite_error().map_or_else(|| e.to_string(), |error| rusqlite::ffi::code_to_str(error.extended_code).to_owned());
        match e.sqlite_error_code() {
            Some(ErrorCode::CannotOpen) if matches!(path.try_exists(), Ok(false)) => DbError::Connect(format!("no file at {shown}")),
            _ => DbError::Connect(format!("could not open {shown}: {why}")),
        }
    })?;
    let setup = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => DbError::Connect(format!("{shown} is not a SQLite database")),
        Some(ErrorCode::DatabaseCorrupt) => DbError::Connect(format!("{shown} is damaged: {e}")),
        _ => DbError::Connect(format!("{shown}: {e}")),
    };
    harden(&conn).map_err(setup)?;
    conn.execute_batch("PRAGMA foreign_keys = ON").map_err(setup)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(setup)?;
    conn.query_row("SELECT count(*) FROM main.sqlite_schema", [], |_| Ok(())).map_err(setup)?;
    let read_only = read_only || conn.is_readonly(rusqlite::MAIN_DB).map_err(setup)?;
    if read_only {
        conn.execute_batch("PRAGMA query_only = ON").map_err(setup)?;
    }
    let isolated = allowed.is_some();
    let refused = Arc::new(AtomicU8::new(0));
    if isolated || read_only {
        let why = Arc::clone(&refused);
        conn.authorizer(Some(move |ctx: AuthContext<'_>| match authorize(&ctx.action, isolated, read_only) {
            Some(reason) => {
                why.store(reason as u8, Ordering::Relaxed);
                Authorization::Deny
            }
            None => Authorization::Allow,
        }))
        .map_err(setup)?;
    }
    let handle = conn.get_interrupt_handle();
    Ok((conn, handle, read_only, refused))
}

/// What [`open`] gives: the connection, its interrupt handle, whether it is read-only, and the
/// last [`Refused`] reason its authorizer gave.
type Opened = (rusqlite::Connection, InterruptHandle, bool, Arc<AtomicU8>);

/// The flags a file opens with: read-only, read-write, or read-write making a missing one. In an
/// isolated run the path its check resolved, which holds no link, opens with
/// `SQLITE_OPEN_NOFOLLOW`, so a link swapped into it after the check is refused, not followed out
/// of the allowed folders.
fn open_flags(read_only: bool, create: bool, isolated: bool) -> OpenFlags {
    let mode = match (read_only, create) {
        (true, _) => OpenFlags::SQLITE_OPEN_READ_ONLY,
        (false, true) => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        (false, false) => OpenFlags::SQLITE_OPEN_READ_WRITE,
    };
    let links = if isolated { OpenFlags::SQLITE_OPEN_NOFOLLOW } else { OpenFlags::empty() };
    mode | links | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// SQLite's own settings for a file from a source dbee does not know, which any file a file
/// manager hands it can be: SQL may not write the file's structure by hand (`DEFENSIVE`), and each
/// page's cells are checked as it is read. `trusted_schema` stays on, SQLite's default: dbee
/// registers no SQL function and no virtual-table module, so turning it off would keep nothing
/// from a file's views and triggers but FTS's and the pragma functions, which they need. Turn it
/// off the day dbee registers a function or a module that has effects.
fn harden(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.execute_batch("PRAGMA cell_size_check = ON")
}

/// Why SQL may not do `action` on a connection, or `None` when it may: in an isolated run it opens
/// no other file (no `ATTACH`, and so no `VACUUM`, which attaches the file it writes) and moves none
/// of SQLite's own; on a read-only one it attaches no file (`VACUUM INTO` writes one wherever it
/// names) and keeps `query_only` on.
fn authorize(action: &AuthAction<'_>, isolated: bool, read_only: bool) -> Option<Refused> {
    match *action {
        AuthAction::Attach { .. } if isolated => Some(Refused::IsolatedAttach),
        AuthAction::Attach { .. } if read_only => Some(Refused::ReadOnlyAttach),
        AuthAction::Pragma { pragma_name, pragma_value: Some(_) }
            if isolated && FOLDER_PRAGMAS.iter().any(|p| pragma_name.eq_ignore_ascii_case(p)) =>
        {
            Some(Refused::IsolatedFolder)
        }
        AuthAction::Pragma { pragma_name, pragma_value: Some(_) } if read_only && pragma_name.eq_ignore_ascii_case("query_only") => {
            Some(Refused::ReadOnlyQueryOnly)
        }
        _ => None,
    }
}

/// An open file. Every operation holds the connection's turn for its whole run, so operations
/// never interleave; its SQLite calls run on blocking threads. Dropping the last handle
/// interrupts what still runs on it, and the file is closed on a blocking thread once that has
/// stopped.
pub struct SqliteConnection {
    /// `None` only while it is dropped.
    shared: Option<Arc<Shared>>,
    running: Arc<Running>,
    info: ServerInfo,
    turn: tokio::sync::Mutex<()>,
    /// The runtime the file is closed on, also when the connection is dropped off it.
    runtime: Handle,
}

/// What a blocking call needs, which outlives the caller's future when that is dropped.
struct Shared {
    conn: Mutex<Session>,
    /// Set when an operation on it was abandoned half-way (an apply inside its transaction).
    closed: AtomicBool,
    running: Arc<Running>,
}

/// The open file and what dbee keeps of it between calls.
pub(crate) struct Session {
    pub(crate) conn: rusqlite::Connection,
    pub(crate) names: NamesChecked,
    /// Why the authorizer last refused a statement ([`Refused`] as a number, 0 for none).
    pub(crate) refused: Arc<AtomicU8>,
}

impl Session {
    pub(crate) fn new(conn: rusqlite::Connection) -> Session {
        Session { conn, names: NamesChecked::default(), refused: Arc::default() }
    }
}

impl std::fmt::Debug for SqliteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteConnection").field("info", &self.info).field("closed", &self.is_closed()).finish()
    }
}

impl Drop for SqliteConnection {
    fn drop(&mut self) {
        self.running.interrupt();
        if let Some(shared) = self.shared.take() {
            // Closing can write (a WAL checkpoint, a rollback); the dropping thread may be the UI's.
            drop(self.runtime.spawn_blocking(move || drop(shared)));
        }
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
        self.shared.as_ref().is_none_or(|s| s.closed.load(Ordering::Acquire))
    }

    /// Cancels whatever runs on this connection at the time of the call.
    pub fn canceller(&self) -> Canceller {
        Canceller(Arc::clone(&self.running))
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
        if let Some(shared) = &self.shared {
            shared.closed.store(true, Ordering::Release);
        }
    }

    /// Run `work` on the connection on a blocking thread, as one call that a cancel can interrupt.
    /// The caller holds the turn. When the caller's future is dropped, the call still runs to its
    /// end.
    pub(crate) async fn blocking<T, F>(&self, work: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Session, &Running) -> Result<T, DbError> + Send + 'static,
    {
        let runtime = Handle::try_current().map_err(|_| DbError::Other("SQLite calls need a tokio runtime".into()))?;
        let shared = Arc::clone(self.shared.as_ref().ok_or(DbError::ConnectionLost)?);
        let call = runtime.spawn_blocking(move || {
            // Poisoned only when a call panicked half-way: the session is in an unknown state.
            let Ok(mut session) = shared.conn.lock() else {
                shared.closed.store(true, Ordering::Release);
                return Err(DbError::ConnectionLost);
            };
            if shared.closed.load(Ordering::Acquire) {
                return Err(DbError::ConnectionLost);
            }
            let _call = shared.running.start();
            work(&mut session, &shared.running)
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
pub(crate) struct Call<'a>(&'a Running);

impl Drop for Call<'_> {
    fn drop(&mut self) {
        self.0.state().running = false;
    }
}

impl Running {
    pub(crate) fn new(handle: InterruptHandle) -> Running {
        Running { handle, state: Mutex::new(State::default()) }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn start(&self) -> Call<'_> {
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

    use dbee_config::{ConnectionConfig, DbKind};
    use dbee_driver::Purpose;

    use super::*;

    /// A file of this test binary's own under `target/`.
    fn scratch_file(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testkit/sqlite-driver");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}-{}.sqlite", std::process::id()))
    }

    #[test]
    fn an_interrupt_reaches_only_a_running_call() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let r = Running::new(conn.get_interrupt_handle());
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
        assert_eq!(open(&file, false, false, None).err(), Some(DbError::Connect(format!("no file at {}", file.display()))));
        let (conn, _, read_only, _) = open(&file, false, true, None).unwrap();
        let pragma = |name: &str| conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!((pragma("foreign_keys"), pragma("busy_timeout"), pragma("query_only"), read_only), (1, 5000, 0, false));
        drop(conn);
        let (ro, _, read_only, _) = open(&file, true, false, None).unwrap();
        assert_eq!((ro.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0)).unwrap(), read_only), (1, true));
        drop(ro);
        // A trailing separator names a folder: refused, not opened, nor made.
        let slashed = PathBuf::from(format!("{}/", file.display()));
        for create in [false, true] {
            assert_eq!(open(&slashed, false, create, None).err(), Some(DbError::Connect(format!("{} is not a file", slashed.display()))));
        }
        let _ = std::fs::remove_file(&file);
        for create in [false, true] {
            assert!(open(&slashed, false, create, None).is_err());
            assert!(!file.exists(), "nothing is made");
        }
    }

    /// SQLite's hardening for a file from a source dbee does not know, on every open.
    #[test]
    fn files_open_with_sqlites_hardening() {
        let file = scratch_file("hardened");
        let _ = std::fs::remove_file(&file);
        for (read_only, create) in [(false, true), (true, false)] {
            let (conn, _, _, _) = open(&file, read_only, create, None).unwrap();
            let pragma = |name: &str| conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0)).unwrap();
            assert_eq!((pragma("trusted_schema"), pragma("cell_size_check")), (1, 1), "read-only {read_only}");
            assert_eq!(conn.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE).ok(), Some(true), "read-only {read_only}");
        }
        let _ = std::fs::remove_file(&file);
    }

    /// In an isolated run the path the check resolved opens without following a link anywhere in
    /// it; outside one a link is the user's own, and is followed.
    #[cfg(unix)]
    #[test]
    fn an_isolated_run_follows_no_link_at_the_path_it_checked() {
        let (target, link) = (scratch_file("nofollow-target"), scratch_file("nofollow-link"));
        let _ = std::fs::remove_file(&link);
        rusqlite::Connection::open(&target).unwrap().execute_batch("CREATE TABLE IF NOT EXISTS t (x)").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        for read_only in [false, true] {
            assert!(rusqlite::Connection::open_with_flags(&link, open_flags(read_only, false, false)).is_ok(), "read-only {read_only}");
            let refused = rusqlite::Connection::open_with_flags(&link, open_flags(read_only, false, true)).err();
            let code = refused.as_ref().and_then(rusqlite::Error::sqlite_error).map(|e| e.extended_code);
            assert_eq!(code, Some(rusqlite::ffi::SQLITE_CANTOPEN_SYMLINK), "read-only {read_only}: {refused:?}");
        }
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);
    }

    #[tokio::test]
    async fn a_poisoned_connection_is_closed() {
        let file = scratch_file("poisoned");
        let _ = std::fs::remove_file(&file);
        let config = ConnectionConfig { kind: DbKind::Sqlite, file: Some(file.clone()), ..ConnectionConfig::default() };
        let conn = connect(Target { config: &config, password: None, tunnel: None, purpose: Purpose::Browse, create_file: true, allowed_dirs: None })
            .await
            .unwrap();
        // A call that panics with the session locked, its message kept out of the test's output.
        let shared = Arc::clone(conn.shared.as_ref().unwrap());
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let poisoning = std::thread::spawn(move || {
            let _session = shared.conn.lock();
            panic!("a call that panicked");
        });
        assert!(poisoning.join().is_err());
        std::panic::set_hook(hook);
        assert_eq!(conn.scalar_u64("SELECT 1").await, Err(DbError::ConnectionLost));
        assert!(conn.is_closed(), "the engine sees it closed");
        drop(conn);
        let _ = std::fs::remove_file(&file);
    }
}

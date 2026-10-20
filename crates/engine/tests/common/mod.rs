//! Helpers shared by the engine's integration tests (`mod common;`): sessions on a throwaway
//! home, configs for the Docker test servers, saving a connection, a connected session, and
//! killing dbee's backends.
//!
//! Safety: every session keeps its connections file in the temp home, its secrets in a
//! `MemoryStore`, and reads SSH keys and `known_hosts` from `<temp home>/.ssh` with no agent, so
//! the real home, `~/.ssh`, SSH agent and keyring are never touched. Tests connect only to the
//! test servers (`dbee_testkit::testdb()`); call `require_testdb!()` before any helper that
//! names a server.
//!
//! Killing backends: dbee connects with `application_name = 'dbee'`, so terminating its
//! backends hits every dbee connection on the server, also those of tests running in parallel
//! threads of the same binary. So every [`TestSession`] holds a process-wide lock: shared for
//! normal sessions, exclusive for [`exclusive_session`], and [`kill_dbee_backends`] runs only
//! inside an exclusive one. Cargo runs test binaries one after another, so other binaries do not
//! overlap. Hold at most one `TestSession` per thread at a time (a second shared lock taken while
//! an exclusive one waits would deadlock).
#![allow(dead_code, reason = "each test binary uses some of these helpers")]

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use dbee_config::{ConnectionConfig, MemoryStore, SshAuth, SshConfig, SslMode};
use dbee_engine::{EngineError, Json, Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{PgServer, TempHome, TestDb, temp_home};
use serde_json::json;

/// How long a test waits for a job.
pub const T: Duration = Duration::from_secs(30);

/// Shared by every session; exclusive while a test kills backends.
static DB_LOCK: RwLock<()> = RwLock::new(());

/// The lock a session holds until it drops.
enum Hold {
    Shared(RwLockReadGuard<'static, ()>),
    Exclusive(RwLockWriteGuard<'static, ()>),
}

/// A session on a throwaway home. Derefs to the [`Session`].
pub struct TestSession {
    // Fields drop in this order: the session (closing its connections) before the lock goes.
    pub session: Session,
    /// The session's secret store.
    pub secrets: Arc<MemoryStore>,
    /// Connections file in `home/config`, SSH files in `home/.ssh`.
    pub home: TempHome,
    hold: Hold,
}

impl Deref for TestSession {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl DerefMut for TestSession {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

fn session_with(home: TempHome, exclusive: bool, adjust: impl FnOnce(&mut SessionOptions)) -> TestSession {
    let hold = if exclusive {
        Hold::Exclusive(DB_LOCK.write().unwrap_or_else(PoisonError::into_inner))
    } else {
        Hold::Shared(DB_LOCK.read().unwrap_or_else(PoisonError::into_inner))
    };
    let secrets = Arc::new(MemoryStore::default());
    let mut options = SessionOptions {
        config_dir: Some(home.home.join("config")),
        secrets: Some(secrets.clone()),
        ssh: SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None },
        notify: None,
    };
    adjust(&mut options);
    TestSession { session: Session::new(options).unwrap(), secrets, home, hold }
}

/// A session on a new empty temp home.
pub fn session() -> TestSession {
    session_with(temp_home(None, None), false, |_| {})
}

/// A session on `home` (e.g. `temp_home(Some(key), Some(known_hosts))`).
pub fn session_in(home: TempHome) -> TestSession {
    session_with(home, false, |_| {})
}

/// A session whose options `adjust` changes (another secret store, none, …). The SSH settings
/// must stay on the temp home.
pub fn session_adjusted(adjust: impl FnOnce(&mut SessionOptions)) -> TestSession {
    session_with(temp_home(None, None), false, adjust)
}

/// A session that no other test of this binary runs beside; needed by [`kill_dbee_backends`].
pub fn exclusive_session() -> TestSession {
    session_with(temp_home(None, None), true, |_| {})
}

/// The test servers; `require_testdb!()` must have passed.
pub fn db() -> &'static TestDb {
    dbee_testkit::testdb().expect("call require_testdb!() before the test server helpers")
}

/// The Postgres 14 server.
pub fn pg14() -> &'static PgServer {
    db().servers.iter().find(|s| s.major == 14).expect("the test servers include pg14")
}

/// The newest Postgres.
pub fn pgnew() -> &'static PgServer {
    db().servers.iter().max_by_key(|s| s.major).expect("the test servers include pgnew")
}

/// `postgres@127.0.0.1:<port>/dbee`, named after the server, TLS `prefer`, no id.
pub fn server_config(s: &PgServer) -> ConnectionConfig {
    ConnectionConfig {
        name: s.name.clone(),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

/// [`server_config`] for pg14, named "pg14".
pub fn pg14_config() -> ConnectionConfig {
    server_config(pg14())
}

/// The `postgres` password of the test servers.
pub fn pw() -> String {
    db().password.clone()
}

/// The test sshd as user `dbee` (keys) with `auth`.
pub fn ssh_config(auth: SshAuth) -> SshConfig {
    let d = db();
    SshConfig { host: "127.0.0.1".into(), port: d.sshd.port, user: d.sshd.user.clone(), auth }
}

/// pg14 through the test sshd: the database host as the sshd sees it (`pg14:5432`).
pub fn via_ssh(auth: SshAuth) -> ConnectionConfig {
    let d = db();
    ConnectionConfig { name: "pg14 via ssh".into(), host: d.sshd.pg_host.clone(), port: d.sshd.pg_port, ssh: Some(ssh_config(auth)), ..pg14_config() }
}

/// `[127.0.0.1]:<sshd port>`, as `known_hosts` and the host key errors name the sshd.
pub fn sshd_entry() -> String {
    format!("[127.0.0.1]:{}", db().sshd.port)
}

/// Saves `config` with `password` through `connection.save`; returns its id.
pub fn save(s: &mut Session, config: ConnectionConfig, password: Option<String>) -> String {
    let mut params = json!({ "config": config });
    if let Some(p) = password {
        params["password"] = json!(p);
    }
    let r = s.run_blocking("connection.save", params, T).unwrap();
    r["id"].as_str().expect("connection.save returns {id}").to_owned()
}

/// Opens `params` with `connection.open` and checks that the workspace shows.
pub fn open(s: &mut Session, params: Json) -> Json {
    let r = s.run_blocking("connection.open", params, T).unwrap_or_else(|e| panic!("connection.open: {e} [{}] {}", e.code(), e.data()));
    assert_eq!(s.inspect()["screen"], "workspace");
    r
}

/// A job command's error.
pub fn run_err(s: &mut Session, id: &str, params: Json) -> EngineError {
    match s.run_blocking(id, params, T) {
        Ok(v) => panic!("{id} succeeded: {v}"),
        Err(e) => e,
    }
}

fn connect(mut s: TestSession) -> TestSession {
    let id = save(&mut s, pg14_config(), Some(pw()));
    open(&mut s, json!({ "id": id }));
    s
}

/// A session connected to pg14, saved as "pg14" with its password in the store.
pub fn connected() -> TestSession {
    connect(session())
}

/// [`connected`] in an [`exclusive_session`], for tests that kill backends.
pub fn connected_exclusive() -> TestSession {
    connect(exclusive_session())
}

/// Terminates every dbee backend on `server`, then waits (up to 10 s) until none is left. Only
/// inside an [`exclusive_session`]: it kills every dbee connection on the server.
pub fn kill_dbee_backends(s: &TestSession, server: &PgServer) {
    assert!(matches!(s.hold, Hold::Exclusive(_)), "kill_dbee_backends needs an exclusive_session()");
    // The two-argument form (Postgres 14+) waits until the backend has exited. A backend of an
    // earlier test may already be exiting by itself and report `f`; the session's own is `t`.
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    let out = dbee_testkit::psql(server, sql).unwrap();
    assert!(out.lines().any(|l| l.trim() == "t"), "no dbee backend was terminated on {}: {out:?}", server.name);
    let count = "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee'";
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = dbee_testkit::psql(server, count).unwrap();
        if left.trim() == "0" {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{} dbee backend(s) still on {} after 10 s", left.trim(), server.name);
        std::thread::sleep(Duration::from_millis(50));
    }
}

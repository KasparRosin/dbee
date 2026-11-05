//! Helpers shared by the engine's integration tests (`mod common;`): sessions on a throwaway
//! home, configs for the Docker test servers, saving a connection, a connected session, running a
//! test on every engine, the table helpers, and killing dbee's connections.
//!
//! Every session gets every driver dbee ships (`dbee_drivers::all()`).
//!
//! **Every engine.** [`for_each_engine`] runs a test on a session connected to each target of
//! `dbee_testkit::targets()` whose kind has a driver, one after another; [`engine_targets`] lists
//! them for a test that opens its own sessions. A test skips a target by what it can do (`t.is_file()`
//! cannot lose its connection, a dialect without `USE`), never by its kind's name. What differs
//! per kind in test SQL is in `dbee_testkit::Kind::info`.
//!
//! Safety: every session keeps its connections file in the temp home, its secrets in a
//! `MemoryStore`, and reads SSH keys and `known_hosts` from `<temp home>/.ssh` with no agent, so
//! the real home, `~/.ssh`, SSH agent and keyring are never touched. Tests connect only to the
//! test servers (`dbee_testkit::testdb()`); call `require_testdb!()` before any helper that
//! names a server.
//!
//! Killing connections: [`kill_dbee_connections`] ends every dbee connection on the server, also
//! those of tests running in parallel threads of the same binary. So every [`TestSession`] holds a
//! process-wide lock: shared for normal sessions, exclusive for [`exclusive_session`], and the
//! kill runs only inside an exclusive one. Cargo runs test binaries one after another, so other
//! binaries do not overlap. Hold at most one `TestSession` per thread at a time (a second shared
//! lock taken while an exclusive one waits would deadlock).
#![allow(dead_code, reason = "each test binary uses some of these helpers")]

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind, MemoryStore, SshAuth, SshConfig, SslMode};
use dbee_engine::{EngineError, Json, Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{Kind, Server, TempHome, TestDb, TestTarget, temp_home};
use serde_json::json;

/// How long a test waits for a job.
pub const T: Duration = Duration::from_secs(30);

/// Shared by every session; exclusive while a test kills connections.
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
        allowed_targets: None,
        allowed_dirs: None,
        drivers: dbee_drivers::all(),
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

/// A session that no other test of this binary runs beside; needed by [`kill_dbee_connections`].
pub fn exclusive_session() -> TestSession {
    session_with(temp_home(None, None), true, |_| {})
}

/// [`exclusive_session`] on `home`.
pub fn exclusive_session_in(home: TempHome) -> TestSession {
    session_with(home, true, |_| {})
}

/// The test servers; `require_testdb!()` must have passed.
pub fn db() -> &'static TestDb {
    dbee_testkit::testdb().expect("call require_testdb!() before the test server helpers")
}

/// The Postgres 14 server.
pub fn pg14() -> &'static Server {
    db().server("pg14").expect("the test servers include pg14")
}

/// The newest Postgres.
pub fn pgnew() -> &'static Server {
    db().server("pgnew").expect("the test servers include pgnew")
}

/// `postgres@127.0.0.1:<port>/dbee`, named after the server, TLS `prefer`, no id.
pub fn server_config(s: &Server) -> ConnectionConfig {
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

/// The admin password of the test servers.
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
    ConnectionConfig { name: "pg14 via ssh".into(), ..via_ssh_to(pg14(), &pg14_config(), auth) }
}

/// `config` for `server` through the test sshd, which reaches it as `name:service_port`.
pub fn via_ssh_to(server: &Server, config: &ConnectionConfig, auth: SshAuth) -> ConnectionConfig {
    ConnectionConfig { host: server.name.clone(), port: server.service_port, ssh: Some(ssh_config(auth)), ..config.clone() }
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

/// [`connected`] in an [`exclusive_session`], for tests that kill connections.
pub fn connected_exclusive() -> TestSession {
    connect(exclusive_session())
}

// ---- every engine ----------------------------------------------------------------------------

/// The database type of `t`.
pub fn kind_of(t: &TestTarget) -> DbKind {
    DbKind::from_id(t.kind().id()).expect("the test kinds are dbee's")
}

/// Every target of `dbee_testkit::targets()` whose kind has a driver in `dbee_drivers::all()`.
pub fn engine_targets() -> Vec<TestTarget> {
    let kinds = dbee_drivers::all().kinds();
    let with_driver: Vec<Kind> = Kind::ALL.into_iter().filter(|k| DbKind::from_id(k.id()).is_some_and(|k| kinds.contains(&k))).collect();
    dbee_testkit::targets_of(&with_driver)
}

/// pg14 as a target, for the tests of Postgres's own types and features.
pub fn pg14_target() -> TestTarget {
    dbee_testkit::target(pg14())
}

/// The settings for `t`: its server on `127.0.0.1` as the kind's user, on its database, TLS
/// `prefer`; or its file. Named after the target, no id.
pub fn config_for(t: &TestTarget) -> ConnectionConfig {
    let info = t.kind().info();
    let base = ConnectionConfig { name: t.name(), kind: kind_of(t), ..ConnectionConfig::default() };
    match t.server() {
        Some(s) => ConnectionConfig {
            host: "127.0.0.1".into(),
            port: s.port,
            user: info.user.into(),
            database: info.database.into(),
            ssl_mode: SslMode::Prefer,
            ..base
        },
        None => ConnectionConfig { file: t.file().map(Path::to_path_buf), ..base },
    }
}

fn connect_to(mut s: TestSession, t: &TestTarget) -> TestSession {
    let id = save(&mut s, config_for(t), Some(pw()));
    open(&mut s, json!({ "id": id }));
    s
}

/// A session connected to `t`, saved with its password in the store.
pub fn connected_to(t: &TestTarget) -> TestSession {
    connect_to(session(), t)
}

/// [`connected_to`] in an [`exclusive_session`].
pub fn connected_exclusive_to(t: &TestTarget) -> TestSession {
    connect_to(exclusive_session(), t)
}

/// Runs `f` on a session connected to each of [`engine_targets`], one after another.
pub fn for_each_engine(mut f: impl FnMut(&TestTarget, &mut TestSession)) {
    for t in engine_targets() {
        println!("on {}", t.name());
        let mut s = connected_to(&t);
        f(&t, &mut s);
    }
}

/// [`for_each_engine`] in exclusive sessions, for tests that kill connections.
pub fn for_each_engine_exclusive(mut f: impl FnMut(&TestTarget, &mut TestSession)) {
    for t in engine_targets() {
        println!("on {}", t.name());
        let mut s = connected_exclusive_to(&t);
        f(&t, &mut s);
    }
}

/// [`for_each_engine`] for the targets that are files (the SQLite seed copy).
pub fn for_sqlite(mut f: impl FnMut(&TestTarget, &mut TestSession)) {
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let mut s = connected_to(&t);
        f(&t, &mut s);
    }
}

/// Ends every dbee connection on `t`'s server and waits until none is left. Only inside an
/// [`exclusive_session`]: it ends every dbee connection on the server.
pub fn kill_dbee_connections(t: &TestTarget, s: &TestSession) {
    assert!(matches!(s.hold, Hold::Exclusive(_)), "kill_dbee_connections needs an exclusive_session()");
    let server = t.server().unwrap_or_else(|| panic!("{} is a file, which has no connection to end", t.name()));
    dbee_testkit::kill_dbee_connections(server).unwrap();
}

/// `sql` run on `t`'s server ([`TestTarget::sql`]), trimmed.
pub fn q(t: &TestTarget, sql: &str) -> String {
    t.sql(sql).unwrap_or_else(|e| panic!("{}: {e}", t.name())).trim().to_owned()
}

/// Polls `sql` on `t`'s server until it prints `want` (up to 20 s).
pub fn until_sql(t: &TestTarget, sql: &str, want: &str, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while q(t, sql) != want {
        assert!(std::time::Instant::now() < deadline, "{}: gave up waiting for {what}", t.name());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until a statement whose text holds `marker` runs on `t`'s server. A file lists no
/// statements: its statement gets a moment to start.
pub fn until_running(t: &TestTarget, marker: &str) {
    if t.lists_running_statements() {
        until_sql(t, &t.running_with_sql(marker), "1", "the statement to run");
    } else {
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Drops the scratch objects it names (`TABLE IF EXISTS scratch.t`) on its target when the test
/// ends, also when it fails.
pub struct Scratch<'a>(pub &'a TestTarget, pub Vec<String>);

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        for what in &self.1 {
            let _ = self.0.sql(&format!("DROP {what}"));
        }
    }
}

/// `scratch.<name>` (on a kind with one schema, in that one) made by `ddl` (which names it `{t}`),
/// dropped with the guard.
pub fn scratch_table<'a>(t: &'a TestTarget, name: &str, ddl: &str) -> Scratch<'a> {
    let table = format!("{}.{name}", t.schema("scratch"));
    let guard = Scratch(t, vec![format!("TABLE IF EXISTS {table}")]);
    q(t, &ddl.replace("{t}", &table));
    guard
}

// ---- table tabs ------------------------------------------------------------------------------

/// A command that must succeed.
pub fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.run_blocking(id, params, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data()))
}

/// `table.open` of `schema.table`.
pub fn open_table(s: &mut Session, schema: &str, table: &str) -> Json {
    s.run_blocking("table.open", json!({ "schema": schema, "table": table }), T)
        .unwrap_or_else(|e| panic!("table.open {schema}.{table}: {e} [{}] {}", e.code(), e.data()))
}

/// `cell.edit` of loaded row `row`, column `column` (a name or an index).
pub fn set_cell(s: &mut Session, row: usize, column: impl Into<Json>, value: impl Into<Json>) -> Json {
    run(s, "cell.edit", json!({ "row": row, "column": column.into(), "value": value.into() }))
}

/// `edit.apply`, which must succeed.
pub fn apply(s: &mut Session) -> Json {
    run(s, "edit.apply", json!({}))
}

/// The active table tab as `inspect()` shows it.
pub fn summary(s: &Session) -> Json {
    s.inspect()["table"].clone()
}

/// `table.read` of the active tab, every loaded row.
pub fn read(s: &mut Session) -> Json {
    run(s, "table.read", json!({}))
}

/// The index of column `name` in a `table.read` result.
pub fn col(read: &Json, name: &str) -> usize {
    read["columns"].as_array().unwrap().iter().position(|c| c["name"] == name).unwrap_or_else(|| panic!("no column {name}: {}", read["columns"]))
}

/// Row `row`, column `name` of a `table.read` result, as text (NULL is `None`).
pub fn cell<'a>(read: &'a Json, row: usize, name: &str) -> Option<&'a str> {
    read["rows"][row][col(read, name)].as_str()
}

/// The rows of a `table.read` result.
pub fn rows(read: &Json) -> usize {
    read["rows"].as_array().unwrap().len()
}

/// The error's code (`connectionLost`, `server`, …).
pub fn code(e: &EngineError) -> &'static str {
    e.code()
}

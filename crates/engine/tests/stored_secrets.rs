//! A stored secret goes only where the settings kept with it allow, whatever the connections file
//! or a window's copy of it says (the security review's C-2 and C-4).
//!
//! A stand-in driver records what each connect receives and takes only the password it is told to
//! take, so no server is reached. Every session keeps its connections file in a throwaway home and
//! its secrets in a `MemoryStore`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind, MemoryStore, SecretError, SecretKind, SecretStore, SslMode};
use dbee_core::{Catalog, ResultSet};
use dbee_driver::{
    ApplyError, ApplyGate, BoxFuture, Cancel, Connection, DbError, DbFuture, Driver, Drivers, Limits, ServerInfo, Target, TransactionState,
};
use dbee_engine::{Session, SessionOptions};
use dbee_sql::Dialect;
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TempHome, temp_home};
use serde_json::{Value as Json, json};

const T: Duration = Duration::from_secs(10);
const ID: &str = "x";

/// What one connect received and how it ended.
#[derive(Debug, PartialEq, Eq)]
struct Tried {
    port: u16,
    password: Option<String>,
    /// The error's code, or `ok`.
    end: String,
}

fn tried(port: u16, password: Option<&str>, end: &str) -> Tried {
    Tried { port, password: password.map(str::to_owned), end: end.to_owned() }
}

/// The driver: records each connect and logs in only with `accepts`.
struct StandIn {
    received: Arc<Mutex<Vec<(u16, Option<String>)>>>,
    accepts: Option<&'static str>,
}

impl StandIn {
    fn log_in<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        self.received.lock().unwrap().push((target.config.port, target.password.map(str::to_owned)));
        let accepted = self.accepts.is_some() && self.accepts == target.password;
        Box::pin(async move {
            if accepted {
                Ok(Arc::new(LoggedIn::default()) as Arc<dyn Connection>)
            } else {
                Err(DbError::Auth { message: "stand-in driver: login refused".into() })
            }
        })
    }
}

impl Driver for StandIn {
    fn kinds(&self) -> &'static [DbKind] {
        &[DbKind::Postgres, DbKind::MySql, DbKind::MariaDb]
    }
    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        self.log_in(target)
    }
    fn list_connection<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Option<Arc<dyn Connection>>> {
        let connecting = self.log_in(target);
        Box::pin(async move { connecting.await.map(Some) })
    }
}

/// A connection nobody runs anything on.
struct LoggedIn(ServerInfo);

impl Default for LoggedIn {
    fn default() -> Self {
        LoggedIn(ServerInfo {
            product: "Stand-in".into(),
            version: "1".into(),
            version_num: 1,
            read_only: false,
            warnings: Vec::new(),
            tls: None,
            local: true,
        })
    }
}

struct NoCancel;

impl Cancel for NoCancel {
    fn cancel(&self) -> DbFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Connection for LoggedIn {
    fn info(&self) -> &ServerInfo {
        &self.0
    }
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }
    fn is_closed(&self) -> bool {
        false
    }
    fn canceller(&self) -> Arc<dyn Cancel> {
        Arc::new(NoCancel)
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
    fn transaction(&self) -> BoxFuture<'_, TransactionState> {
        Box::pin(async { TransactionState::Idle })
    }
}

/// One connections file and one secret store, shared by the windows on them.
struct Rig {
    home: TempHome,
    store: Arc<dyn SecretStore>,
    received: Arc<Mutex<Vec<(u16, Option<String>)>>>,
    accepts: Option<&'static str>,
}

impl Rig {
    fn new() -> Rig {
        Rig::on(Arc::new(MemoryStore::default()))
    }

    fn on(store: Arc<dyn SecretStore>) -> Rig {
        Rig { home: temp_home(None, None), store, received: Arc::default(), accepts: None }
    }

    fn file(&self) -> PathBuf {
        self.home.home.join("config").join(dbee_config::store::FILE)
    }

    /// A dbee window: it reads the saved list now, and again only after its own changes.
    fn window(&self) -> Session {
        Session::new(SessionOptions {
            config_dir: Some(self.home.home.join("config")),
            secrets: Some(Arc::clone(&self.store)),
            ssh: SshEnv { ssh_dir: None, agent: AgentSource::None },
            notify: None,
            allowed_targets: None,
            allowed_dirs: None,
            drivers: Drivers::new(vec![Arc::new(StandIn { received: Arc::clone(&self.received), accepts: self.accepts })]),
        })
        .unwrap()
    }

    /// What `command` with `params` sent to the driver from `window`, and how it ended.
    fn run(&self, window: &mut Session, command: &str, params: Json) -> Tried {
        let end = match window.run_blocking(command, params, T) {
            Ok(_) => "ok".to_owned(),
            Err(e) => e.code().to_owned(),
        };
        let (port, password) = self.received.lock().unwrap().last().cloned().expect("the driver was called");
        Tried { port, password, end }
    }

    fn open(&self, window: &mut Session, params: Json) -> Tried {
        self.run(window, "connection.open", params)
    }

    /// The store's text for the database password, as a JSON value; `Null` when that text is none.
    fn stored(&self) -> Json {
        let raw = self.store.get(ID, SecretKind::DbPassword).unwrap().expect("a stored password");
        serde_json::from_str(&raw).unwrap_or(Json::Null)
    }
}

/// Moves the saved connection in the file, as another program or a second dbee would.
fn move_in_file(file: &Path, from: u16, to: u16) {
    let text = std::fs::read_to_string(file).unwrap();
    let moved = text.replace(&format!("\"port\": {from}"), &format!("\"port\": {to}"));
    assert_ne!(text, moved, "the port is in the file");
    std::fs::write(file, moved).unwrap();
}

fn config(port: u16) -> ConnectionConfig {
    ConnectionConfig {
        id: ID.into(),
        name: "shop".into(),
        host: "127.0.0.1".into(),
        port,
        user: "ann".into(),
        database: "shop".into(),
        ssl_mode: SslMode::Disable,
        save_secrets: true,
        ..ConnectionConfig::default()
    }
}

fn save(window: &mut Session, config: &ConnectionConfig, password: Option<&str>) {
    let mut params = json!({ "config": config });
    if let Some(p) = password {
        params["password"] = json!(p);
    }
    window.run_blocking("connection.save", params, T).unwrap();
}

/// Window B moves the connection and stores the new server's password. Window A, which still holds
/// the old settings, gets a password prompt for them, not that password.
#[test]
fn a_stale_window_does_not_send_the_new_servers_password_to_the_old_server() {
    let rig = Rig::new();
    let mut a = rig.window();
    save(&mut a, &config(2001), Some("old-server-pw"));
    let mut b = rig.window();
    save(&mut b, &config(2002), Some("NEW-SERVER-PW"));

    let asked = tried(2001, None, "needsPassword");
    assert_eq!(rig.open(&mut a, json!({ "config": config(2001) })), asked);
    assert_eq!(rig.open(&mut a, json!({ "id": ID })), asked);
    assert_eq!(rig.run(&mut a, "connection.test", json!({ "config": config(2001) })), asked);
    assert_eq!(rig.run(&mut a, "connection.listDatabases", json!({ "config": config(2001) })), asked);
    // Window B holds what the password was saved for.
    assert_eq!(rig.open(&mut b, json!({ "id": ID })), tried(2002, Some("NEW-SERVER-PW"), "auth"));
}

/// Rewriting the connections file moves the connection but not its password.
#[test]
fn an_edited_connections_file_does_not_move_the_stored_password() {
    let rig = Rig::new();
    save(&mut rig.window(), &config(2001), Some("kept-in-keyring"));
    move_in_file(&rig.file(), 2001, 6666);

    let mut next = rig.window();
    assert_eq!(rig.open(&mut next, json!({ "id": ID })), tried(6666, None, "needsPassword"));
    // The file does not take it from where it was saved for either.
    assert_eq!(rig.open(&mut next, json!({ "config": config(2001) })), tried(2001, Some("kept-in-keyring"), "auth"));
}

/// The value in the store says what it was saved for, and the connections file holds no secret.
#[test]
fn the_store_keeps_the_settings_with_the_password() {
    let rig = Rig::new();
    let saved = ConnectionConfig {
        kind: DbKind::MySql,
        ssl_mode: SslMode::Require,
        ca_file: Some("/ca/ours.pem".into()),
        options: [(dbee_config::ALLOW_PUBLIC_KEY.id.to_owned(), "true".to_owned())].into(),
        ..config(3306)
    };
    save(&mut rig.window(), &saved, Some("pw-1234"));

    let stored = rig.stored();
    assert_eq!((&stored["v"], &stored["secret"]), (&json!(1), &json!("pw-1234")), "{stored}");
    let saved_for = &stored["for"];
    assert_eq!(
        (&saved_for["kind"], &saved_for["host"], &saved_for["port"], &saved_for["user"]),
        (&json!("mysql"), &json!("127.0.0.1"), &json!(3306), &json!("ann")),
        "{stored}"
    );
    assert!(saved_for.get("database").is_none(), "the database decides nothing: {stored}");
    assert_eq!(
        (&saved_for["sslMode"], &saved_for["caFile"], &saved_for["ssh"], &saved_for["weakening"]),
        (&json!("require"), &json!("/ca/ours.pem"), &Json::Null, &json!([dbee_config::ALLOW_PUBLIC_KEY.id])),
        "{stored}"
    );
    let file = std::fs::read_to_string(rig.file()).unwrap();
    assert!(!file.contains("pw-1234"), "{file}");
}

/// Each setting the password was saved for counts, at the open itself. The tunnel's settings are
/// in the unit tests, as a tunnel needs a server.
#[test]
fn a_stored_password_goes_only_to_the_settings_it_was_saved_for() {
    let rig = Rig::new();
    let saved = ConnectionConfig { ssl_mode: SslMode::Require, ca_file: Some("/ca/ours.pem".into()), ..config(2001) };
    let mut w = rig.window();
    save(&mut w, &saved, Some("the-password"));
    let others = [
        ("the database type", ConnectionConfig { kind: DbKind::MariaDb, ..saved.clone() }),
        ("no CA file", ConnectionConfig { ca_file: None, ..saved.clone() }),
        ("another CA file", ConnectionConfig { ca_file: Some("/ca/theirs.pem".into()), ..saved.clone() }),
        ("a weaker TLS mode", ConnectionConfig { ssl_mode: SslMode::Prefer, ..saved.clone() }),
        ("the host", ConnectionConfig { host: "127.0.0.2".into(), ..saved.clone() }),
        ("the port", ConnectionConfig { port: 2002, ..saved.clone() }),
        ("the user", ConnectionConfig { user: "bob".into(), ..saved.clone() }),
    ];
    for (what, other) in others {
        let got = rig.open(&mut w, json!({ "config": other }));
        assert_eq!((got.password.as_deref(), got.end.as_str()), (None, "needsPassword"), "another {what}");
    }
    assert_eq!(rig.open(&mut w, json!({ "config": saved })).password.as_deref(), Some("the-password"));
    let verified = ConnectionConfig { ssl_mode: SslMode::VerifyFull, ..saved };
    assert_eq!(rig.open(&mut w, json!({ "config": verified })).password.as_deref(), Some("the-password"), "a stronger mode, the same CA file");
}

/// Under `require` a CA file makes dbee check the certificate; without one any certificate passes.
#[test]
fn require_with_a_ca_file_does_not_give_its_password_to_require_without_one() {
    let rig = Rig::new();
    let saved = ConnectionConfig { ssl_mode: SslMode::Require, ca_file: Some("/etc/ssl/our-ca.pem".into()), ..config(2001) };
    let mut w = rig.window();
    save(&mut w, &saved, Some("checked-chain-only"));
    let no_ca = ConnectionConfig { ca_file: None, ..saved };
    assert_eq!(rig.open(&mut w, json!({ "config": no_ca })), tried(2001, None, "needsPassword"));
}

/// A MySQL connection refuses a plain-link login under `prefer`; the same settings as a Postgres
/// connection would answer a cleartext password request on one.
#[test]
fn another_database_type_does_not_get_the_stored_password() {
    let rig = Rig::new();
    let mysql = ConnectionConfig { kind: DbKind::MySql, ssl_mode: SslMode::Prefer, ..config(3306) };
    let mut w = rig.window();
    save(&mut w, &mysql, Some("mysql-pw"));
    let as_pg = ConnectionConfig { kind: DbKind::Postgres, ..mysql };
    assert_eq!(rig.open(&mut w, json!({ "config": as_pg })), tried(3306, None, "needsPassword"));
}

/// A store that runs `between` just before its next write lands: what another dbee can do after a
/// window lets go of the connections file's lock and before its keyring write.
struct WritesLate {
    inner: MemoryStore,
    between: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl WritesLate {
    fn new() -> Arc<WritesLate> {
        Arc::new(WritesLate { inner: MemoryStore::default(), between: Mutex::new(None) })
    }
    fn before_the_next_write(&self, between: impl FnOnce() + Send + 'static) {
        *self.between.lock().unwrap() = Some(Box::new(between));
    }
}

impl SecretStore for WritesLate {
    fn get(&self, id: &str, kind: SecretKind) -> Result<Option<String>, SecretError> {
        self.inner.get(id, kind)
    }
    fn set(&self, id: &str, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        let between = self.between.lock().unwrap().take();
        if let Some(between) = between {
            between();
        }
        self.inner.set(id, kind, value)
    }
    fn delete(&self, id: &str, kind: SecretKind) -> Result<(), SecretError> {
        self.inner.delete(id, kind)
    }
    fn name(&self) -> &'static str {
        "writes late"
    }
}

/// The race carried forward from M2's bug round: the connection moves between the save's file
/// change and its keyring write. The password stays with what it was saved for.
#[test]
fn a_move_before_the_saves_keyring_write_does_not_take_the_password_along() {
    let store = WritesLate::new();
    let rig = Rig::on(store.clone());
    let file = rig.file();
    store.before_the_next_write(move || move_in_file(&file, 2001, 2002));
    save(&mut rig.window(), &config(2001), Some("for-2001"));

    let mut next = rig.window();
    assert_eq!(rig.open(&mut next, json!({ "id": ID })), tried(2002, None, "needsPassword"));
    assert_eq!(rig.open(&mut next, json!({ "config": config(2001) })), tried(2001, Some("for-2001"), "auth"));
}

/// The same race for a password kept when it connected (`savePassword`).
#[test]
fn a_move_before_the_keyring_write_of_save_password_does_not_take_the_password_along() {
    let store = WritesLate::new();
    let mut rig = Rig::on(store.clone());
    rig.accepts = Some("typed");
    let mut w = rig.window();
    save(&mut w, &ConnectionConfig { save_secrets: false, ..config(2001) }, None);
    let file = rig.file();
    store.before_the_next_write(move || move_in_file(&file, 2001, 2002));
    let opened = w.run_blocking("connection.open", json!({ "id": ID, "password": "typed", "savePassword": true }), T).unwrap();
    assert_eq!(opened["secretsSaved"], true, "{opened}");

    let mut next = rig.window();
    assert_eq!(rig.open(&mut next, json!({ "id": ID })), tried(2002, None, "needsPassword"));
    assert_eq!(rig.open(&mut next, json!({ "config": config(2001) })), tried(2001, Some("typed"), "ok"));
}

/// A save that keeps a password moves it to the new settings: going back from `verify-full` in a
/// window that holds the old settings gets no password.
#[test]
fn a_save_that_keeps_a_password_saves_it_for_the_new_settings() {
    let rig = Rig::new();
    let mut a = rig.window();
    let was = ConnectionConfig { ssl_mode: SslMode::Require, ..config(2001) };
    save(&mut a, &was, Some("the-password"));

    let mut b = rig.window();
    save(&mut b, &ConnectionConfig { ssl_mode: SslMode::VerifyFull, ..was.clone() }, None);
    assert_eq!(rig.stored()["for"]["sslMode"], "verify-full");
    assert_eq!(rig.open(&mut a, json!({ "config": was })), tried(2001, None, "needsPassword"));
    assert_eq!(rig.open(&mut b, json!({ "id": ID })), tried(2001, Some("the-password"), "auth"));
}

/// The password goes to the same server and the same user, whichever database the settings name:
/// the form's Database field and its List… ask for nothing, saved or not.
#[test]
fn another_database_on_the_same_server_asks_for_nothing() {
    let mut rig = Rig::new();
    rig.accepts = Some("the-password");
    let mut w = rig.window();
    save(&mut w, &config(2001), Some("the-password"));
    let other = ConnectionConfig { database: "orders".into(), ..config(2001) };
    let logged_in = tried(2001, Some("the-password"), "ok");
    assert_eq!(rig.run(&mut w, "connection.listDatabases", json!({ "config": config(2001) })), logged_in);
    assert_eq!(rig.run(&mut w, "connection.listDatabases", json!({ "config": other })), logged_in);
    assert_eq!(rig.run(&mut w, "connection.test", json!({ "config": other })), logged_in);
    assert_eq!(rig.open(&mut w, json!({ "config": other })), logged_in);
    w.run_blocking("connection.close", json!({}), T).unwrap();
    // A password typed there is kept for the saved connection like any other.
    let kept = w.run_blocking("connection.open", json!({ "config": other, "password": "the-password", "savePassword": true }), T).unwrap();
    assert_eq!(kept["secretsSaved"], true, "{kept}");
}

/// A save that only names another database touches no secret, and the password still goes.
#[test]
fn a_save_that_names_another_database_leaves_the_store_alone() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), Some("the-password"));
    let before = rig.store.get(ID, SecretKind::DbPassword).unwrap();
    save(&mut w, &ConnectionConfig { database: "orders".into(), ..config(2001) }, None);
    assert_eq!(rig.store.get(ID, SecretKind::DbPassword).unwrap(), before);
    assert_eq!(rig.open(&mut w, json!({ "id": ID })), tried(2001, Some("the-password"), "auth"));
}

/// A stale window's copy of a connection takes its secret along with what that secret was saved
/// for, not with the stale settings.
#[test]
fn a_copy_keeps_what_the_secret_was_saved_for() {
    let rig = Rig::new();
    let mut a = rig.window();
    save(&mut a, &config(2001), Some("old-server-pw"));
    save(&mut rig.window(), &config(2002), Some("NEW-SERVER-PW"));

    let copy = a.run_blocking("connection.duplicate", json!({ "id": ID }), T).unwrap();
    assert_eq!(rig.open(&mut a, json!({ "id": copy["id"] })), tried(2001, None, "needsPassword"));
}

/// An older build's password has no settings with it. It goes where the saved connection says, this
/// once, and from then on it has them.
#[test]
fn a_password_from_an_older_build_is_used_once_and_stored_again_with_its_settings() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    rig.store.set(ID, SecretKind::DbPassword, "from-an-older-build").unwrap();

    assert_eq!(rig.open(&mut w, json!({ "id": ID })), tried(2001, Some("from-an-older-build"), "auth"));
    let stored = rig.stored();
    assert_eq!((&stored["secret"], &stored["for"]["port"]), (&json!("from-an-older-build"), &json!(2001)), "{stored}");

    // From here on the settings kept with it decide: the file no longer moves it.
    move_in_file(&rig.file(), 2001, 6666);
    assert_eq!(rig.open(&mut rig.window(), json!({ "id": ID })), tried(6666, None, "needsPassword"));
}

/// It is saved again for the saved settings, not for what this one open happens to use.
#[test]
fn an_older_builds_password_is_stored_again_with_the_saved_settings() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    rig.store.set(ID, SecretKind::DbPassword, "from-an-older-build").unwrap();

    let stronger = ConnectionConfig { ssl_mode: SslMode::Require, ..config(2001) };
    assert_eq!(rig.open(&mut w, json!({ "config": stronger })).password.as_deref(), Some("from-an-older-build"));
    assert_eq!(rig.stored()["for"]["sslMode"], "disable");
    assert_eq!(rig.open(&mut w, json!({ "id": ID })).password.as_deref(), Some("from-an-older-build"));
}

/// The first use follows today's rule: the saved settings, and a saved connection at all.
#[test]
fn an_older_builds_password_goes_nowhere_the_saved_settings_do_not_allow() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    rig.store.set(ID, SecretKind::DbPassword, "from-an-older-build").unwrap();
    assert_eq!(rig.open(&mut w, json!({ "config": config(2002) })), tried(2002, None, "needsPassword"));
    assert_eq!(rig.store.get(ID, SecretKind::DbPassword).unwrap().as_deref(), Some("from-an-older-build"), "not used, so as it was");

    let unsaved = ConnectionConfig { id: "unsaved".into(), ..config(2001) };
    rig.store.set("unsaved", SecretKind::DbPassword, "no-connection-owns-it").unwrap();
    assert_eq!(rig.open(&mut w, json!({ "config": unsaved })), tried(2001, None, "needsPassword"));
}

/// A store that keeps what it holds, like a keyring that lets reads through and refuses writes.
struct NoWrites(MemoryStore);

impl SecretStore for NoWrites {
    fn get(&self, id: &str, kind: SecretKind) -> Result<Option<String>, SecretError> {
        self.0.get(id, kind)
    }
    fn set(&self, _: &str, _: SecretKind, _: &str) -> Result<(), SecretError> {
        Err(SecretError("read only".into()))
    }
    fn delete(&self, id: &str, kind: SecretKind) -> Result<(), SecretError> {
        self.0.delete(id, kind)
    }
    fn name(&self) -> &'static str {
        "no writes"
    }
}

/// A refused write does not cost the user the password they had.
#[test]
fn an_older_builds_password_is_still_used_when_the_store_refuses_to_store_it_again() {
    let store = Arc::new(NoWrites(MemoryStore::default()));
    let rig = Rig::on(store.clone());
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    store.0.set(ID, SecretKind::DbPassword, "from-an-older-build").unwrap();
    for _ in 0..2 {
        assert_eq!(rig.open(&mut w, json!({ "id": ID })).password.as_deref(), Some("from-an-older-build"));
    }
}

/// A later build's form is neither a password to send nor a value to overwrite.
#[test]
fn a_value_this_build_cannot_read_is_never_sent_as_a_password() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    for text in [r#"{"v":2,"secret":"s","for":{"kind":"postgres"}}"#, r#"{"v":1,"secret":"s","for":{"kind":"a later type"}}"#] {
        rig.store.set(ID, SecretKind::DbPassword, text).unwrap();
        assert_eq!(rig.open(&mut w, json!({ "id": ID })), tried(2001, None, "needsPassword"), "{text}");
        assert_eq!(rig.store.get(ID, SecretKind::DbPassword).unwrap().as_deref(), Some(text));
    }
}

/// A password can look like anything: only the store's own form is read as one.
#[test]
fn an_older_builds_password_may_look_like_json() {
    for text in ["{\"v\":1}", "[1,2]", "12345", "{not json", "{\"secret\":\"s\",\"for\":{}}"] {
        let rig = Rig::new();
        let mut w = rig.window();
        save(&mut w, &config(2001), None);
        rig.store.set(ID, SecretKind::DbPassword, text).unwrap();
        assert_eq!(rig.open(&mut w, json!({ "id": ID })).password.as_deref(), Some(text));
    }
}

/// Saving other settings with a value this build can't read forgets it, as nothing says it applies.
#[test]
fn a_save_forgets_a_value_it_cannot_read_when_the_settings_change() {
    let rig = Rig::new();
    let mut w = rig.window();
    save(&mut w, &config(2001), None);
    let text = r#"{"v":2,"secret":"s","for":{}}"#;
    rig.store.set(ID, SecretKind::DbPassword, text).unwrap();
    save(&mut w, &ConnectionConfig { name: "renamed".into(), ..config(2001) }, None);
    assert_eq!(rig.store.get(ID, SecretKind::DbPassword).unwrap().as_deref(), Some(text), "a rename touches no secret");
    save(&mut w, &config(2002), None);
    assert_eq!(rig.store.get(ID, SecretKind::DbPassword).unwrap(), None);
}

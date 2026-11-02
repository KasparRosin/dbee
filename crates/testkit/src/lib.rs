//! dbee test kit (layer L0, a dev-dependency only): finds the Docker test servers, the skip rule,
//! `psql` and `mysql` helpers, the SQLite seed file, the list of targets over every engine with
//! each kind's test SQL ([`Kind::info`]), unique names and throwaway home directories.
//!
//! The servers come from `cargo xtask testdb up`, which writes `<repo>/target/testdb/env.json`
//! (`xtask/src/testdb.rs`). A test that needs them starts with [`require_testdb!`]: without the
//! servers it prints `SKIPPED: …` and returns, and with `DBEE_REQUIRE_ORACLES=1` it fails instead.
//! The SQLite seed needs no server: [`sqlite_seeded`] builds it from `xtask/testdb/seed-sqlite.sql`.
//! The repository is found from this crate's manifest directory, never from the current directory.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::hash::{BuildHasher, DefaultHasher, Hasher, RandomState};
use std::io::Write as _;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::Value;

/// The compose project of the test servers.
pub const PROJECT: &str = "dbee-testdb";

/// What a skipped test says.
pub const NOT_UP: &str = "the Docker test servers are not up (run `cargo xtask testdb up`)";

/// The rows of `shop.orders` in every seed (`orders` on SQLite).
pub const ORDERS_COUNT: u64 = 5_000;

/// The rows of `shop.big_rows` on a target with the full seed: the servers with
/// [`Server::full_seed`], and SQLite.
pub const BIG_ROWS: u64 = 1_000_000;

/// The rows of `shop.big_rows` on the other servers.
pub const BIG_ROWS_LIGHT: u64 = 30_000;

/// A database type, by the id dbee saves it under (`dbee_config::DbKind`'s ids; this crate sits
/// below `dbee-config`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Postgres,
    MySql,
    MariaDb,
    Sqlite,
}

/// What the tests need to know of a database type: the one table of each kind's test SQL.
#[derive(Debug)]
pub struct KindInfo {
    pub id: &'static str,
    /// Its client inside a server's container; `None` for a file.
    client: Option<Client>,
    /// A query that runs about `secs` seconds, for cancel tests.
    sleep_sql: fn(u32) -> String,
}

/// A database client inside a server's container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Client {
    Psql,
    /// The `mysql` client or one taking the same arguments (`mariadb`).
    MySql(&'static str),
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Postgres, Kind::MySql, Kind::MariaDb, Kind::Sqlite];

    /// The one `match` on a kind.
    pub fn info(self) -> &'static KindInfo {
        match self {
            Kind::Postgres => &KindInfo { id: "postgres", client: Some(Client::Psql), sleep_sql: |secs| format!("SELECT pg_sleep({secs})") },
            Kind::MySql => &KindInfo { id: "mysql", client: Some(Client::MySql("mysql")), sleep_sql: mysql_sleep },
            Kind::MariaDb => &KindInfo { id: "mariadb", client: Some(Client::MySql("mariadb")), sleep_sql: mysql_sleep },
            // SQLite has no sleep: a count to 10⁹, which runs far longer than any test waits.
            Kind::Sqlite => &KindInfo {
                id: "sqlite",
                client: None,
                sleep_sql: |_| "WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < 1000000000) SELECT count(*) FROM c".into(),
            },
        }
    }

    /// `postgres`, `mysql`, `mariadb` or `sqlite`.
    pub fn id(self) -> &'static str {
        self.info().id
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }
}

fn mysql_sleep(secs: u32) -> String {
    format!("SELECT SLEEP({secs})")
}

/// The test servers, as `env.json` describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDb {
    /// The servers' admin password (`postgres`, MySQL's `root`), also the SSH password of
    /// [`Sshd::password_user`].
    pub password: String,
    /// Every database server, in the order of `SERVICES` in `xtask/src/testdb.rs`: `pg14` and
    /// `pgnew` (the newest Postgres major), database `dbee`; `mysql57`, `mysql80`, `mysqlnew` (the
    /// newest MySQL), `mariadb1011` and `mariadbnew` (the newest MariaDB), user `root`, no default
    /// database.
    pub servers: Vec<Server>,
    pub sshd: Sshd,
    pub keys: Keys,
    /// The CA that signed the servers' certificate (SAN `localhost`, `127.0.0.1` and every database
    /// service's name: `pg14`, `pgnew`, `mysql57`, …).
    pub ca_file: PathBuf,
}

impl TestDb {
    /// The server whose compose service is `name`.
    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// The servers of these kinds, in order.
    pub fn servers_of<'a>(&'a self, kinds: &'a [Kind]) -> impl Iterator<Item = &'a Server> + 'a {
        self.servers.iter().filter(|s| kinds.contains(&s.kind))
    }
}

/// One database server on `127.0.0.1:<port>`; `name` is its compose service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub kind: Kind,
    /// What the server prints as its version: `14.19`, `8.0.46`, `10.11.19`.
    pub version: String,
    pub port: u16,
    /// Its port inside the compose network, where the sshd reaches it as `name:service_port`.
    pub service_port: u16,
    /// It holds the full seed: [`BIG_ROWS`] rows in `shop.big_rows` and the 20 MB row of
    /// `shop.big_cell`. The others hold [`BIG_ROWS_LIGHT`] rows and no big cell.
    pub full_seed: bool,
}

impl Server {
    /// The version's first number: 14, 8, 10.
    pub fn major(&self) -> u32 {
        self.version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or_default()
    }

    /// The rows of `shop.big_rows` in its seed.
    pub fn big_rows(&self) -> u64 {
        if self.full_seed { BIG_ROWS } else { BIG_ROWS_LIGHT }
    }
}

/// The SSH server on `127.0.0.1:<port>`. `user` logs in with [`Keys`], `password_user` with
/// [`TestDb::password`]. Forwarding reaches every database server at `name:service_port`
/// (`pg14:5432`, `mysql80:3306`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sshd {
    pub port: u16,
    pub user: String,
    pub password_user: String,
    /// `[127.0.0.1]:<port> ssh-ed25519 AAAA…`, ready for a `known_hosts` file.
    pub known_hosts_line: String,
}

/// SSH client keys accepted for [`Sshd::user`] (files with mode 0600).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    /// ed25519, no passphrase.
    pub plain: PathBuf,
    /// ed25519, encrypted with `passphrase`.
    pub encrypted: PathBuf,
    pub passphrase: String,
    /// RSA, 3072 bits, no passphrase.
    pub rsa: PathBuf,
}

/// The repository root: two levels above this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(Path::parent).unwrap_or(manifest).to_path_buf()
}

/// `<repo>/target/testdb/env.json`.
pub fn env_file() -> PathBuf {
    repo_root().join("target").join("testdb").join("env.json")
}

/// The test servers when the env file exists and every server answers on its port; read once.
pub fn testdb() -> Option<&'static TestDb> {
    static DB: OnceLock<Option<TestDb>> = OnceLock::new();
    DB.get_or_init(|| load_from(&env_file()).filter(reachable)).as_ref()
}

/// Parse an env file; `None` when it is missing or not the expected shape.
pub fn load_from(path: &Path) -> Option<TestDb> {
    let text = std::fs::read_to_string(path).ok()?;
    parse(&serde_json::from_str(&text).ok()?)
}

fn parse(v: &Value) -> Option<TestDb> {
    let text = |v: &Value, k: &str| v.get(k)?.as_str().map(str::to_owned);
    let port = |v: &Value, k: &str| u16::try_from(v.get(k)?.as_u64()?).ok();
    let mut servers = Vec::new();
    for s in v.get("servers")?.as_array()? {
        servers.push(Server {
            name: text(s, "name")?,
            kind: Kind::from_id(&text(s, "kind")?).filter(|k| k.info().client.is_some())?,
            version: text(s, "version")?,
            port: port(s, "port")?,
            service_port: port(s, "servicePort")?,
            full_seed: s.get("fullSeed")?.as_bool()?,
        });
    }
    let sshd = v.get("sshd")?;
    let keys = v.get("keys")?;
    Some(TestDb {
        password: text(v, "password")?,
        servers,
        sshd: Sshd {
            port: port(sshd, "port")?,
            user: text(sshd, "user")?,
            password_user: text(sshd, "passwordUser")?,
            known_hosts_line: text(sshd, "knownHostsLine")?,
        },
        keys: Keys {
            plain: text(keys, "plain")?.into(),
            encrypted: text(keys, "encrypted")?.into(),
            passphrase: text(keys, "passphrase")?,
            rsa: text(keys, "rsa")?.into(),
        },
        ca_file: text(v, "caFile")?.into(),
    })
}

/// Every published port accepts a TCP connection (an env file can outlive its containers).
fn reachable(db: &TestDb) -> bool {
    let ports = db.servers.iter().map(|s| s.port).chain([db.sshd.port]);
    ports.into_iter().all(|p| TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], p)), Duration::from_secs(2)).is_ok())
}

/// `DBEE_REQUIRE_ORACLES=1`: a test that would skip fails instead.
pub fn oracles_required() -> bool {
    std::env::var("DBEE_REQUIRE_ORACLES").is_ok_and(|v| v == "1")
}

/// Print `SKIPPED: <reason>` straight to stderr, so it shows even while the test harness captures
/// output.
pub fn skipped(reason: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED: {reason}");
}

/// `let db = require_testdb!();` gives `&'static TestDb`, or returns from the test with
/// `SKIPPED: …` on stderr. With `DBEE_REQUIRE_ORACLES=1` it panics instead; only the expanding
/// test code panics.
#[macro_export]
macro_rules! require_testdb {
    () => {
        match $crate::testdb() {
            ::core::option::Option::Some(db) => db,
            ::core::option::Option::None => {
                if $crate::oracles_required() {
                    ::core::panic!("DBEE_REQUIRE_ORACLES=1 and {}", $crate::NOT_UP);
                }
                $crate::skipped(&::std::format!("{}: {}", ::core::module_path!(), $crate::NOT_UP));
                return;
            }
        }
    };
}

/// A random 32-bit value from std's randomly keyed hasher.
fn random_u32() -> u32 {
    let mut h = RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default());
    (h.finish() >> 16) as u32
}

/// `<prefix>_<6 hex digits>`, e.g. `t_edit_5f3a9c`: distinct within a process (the first 2^24
/// calls), random across processes. For tables in schema `scratch`.
pub fn unique(prefix: &str) -> String {
    static BASE: OnceLock<u32> = OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let base = *BASE.get_or_init(random_u32);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    // Multiplying by an odd number is a bijection modulo 2^24, so the first 2^24 values differ.
    let v = base.wrapping_add(n.wrapping_mul(0x9E37_79B1)) & 0x00FF_FFFF;
    format!("{prefix}_{v:06x}")
}

/// `docker compose` for the test servers, with the variables `cargo xtask testdb up` wrote. The
/// process environment never overrides them.
fn compose() -> Command {
    let root = repo_root();
    let env_file = root.join("target").join("testdb").join("compose.env");
    let mut c = Command::new("docker");
    c.current_dir(&root)
        .stdin(Stdio::null())
        .arg("compose")
        .args(["-p", PROJECT, "-f"])
        .arg(root.join("xtask").join("testdb").join("compose.yml"))
        .arg("--env-file")
        .arg(&env_file);
    let text = std::fs::read_to_string(&env_file).unwrap_or_default();
    for var in text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once('=')).map(|(k, _)| k.trim()) {
        c.env_remove(var);
    }
    c
}

/// Run `sql` with `psql` inside a Postgres server's container, as `postgres` on database `dbee`
/// (`-At -v ON_ERROR_STOP=1 -c <sql>`): unaligned, tuples only. Returns stdout, or stderr on error.
pub fn psql(server: &Server, sql: &str) -> Result<String, String> {
    let mut c = compose();
    c.args(["exec", "-T", &server.name, "psql", "-U", "postgres", "-d", "dbee", "-At", "-v", "ON_ERROR_STOP=1", "-c", sql]);
    let out = c.output().map_err(|e| format!("docker compose exec {}: {e}", server.name))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("psql on {} ({}): {}", server.name, out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Run `sql` with the `mysql` client (`mariadb` on MariaDB) inside a MySQL or MariaDB server's
/// container, as root with no default database: `--batch --skip-column-names --raw`, so one line
/// per row, the values tab-separated, unescaped, NULL as `NULL`. Returns stdout, or stderr on
/// error. The password reaches the client as `MYSQL_PWD`, never as an argument.
pub fn mysql(server: &Server, sql: &str) -> Result<String, String> {
    let db = testdb().ok_or_else(|| NOT_UP.to_owned())?;
    let Some(Client::MySql(client)) = server.kind.info().client else {
        return Err(format!("{} is not a MySQL or MariaDB server", server.name));
    };
    let mut c = compose();
    c.env("MYSQL_PWD", &db.password);
    c.args(["exec", "-T", "-e", "MYSQL_PWD", &server.name, client, "--user=root", "--default-character-set=utf8mb4"]);
    c.args(["--batch", "--skip-column-names", "--raw"]).arg(format!("--execute={sql}"));
    let out = c.output().map_err(|e| format!("docker compose exec {}: {e}", server.name))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{client} on {} ({}): {}", server.name, out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// A psql session of its own on a test server that ran `sql` in an open transaction and now
/// sleeps, until dropped (at most 60 s): what a lock `sql` took (`SELECT … FOR UPDATE`) is held
/// that long. Dropping it ends the session, which rolls the transaction back.
#[derive(Debug)]
pub struct Held {
    server: Server,
    marker: String,
    child: std::process::Child,
}

impl Drop for Held {
    fn drop(&mut self) {
        let end = format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = '{}'", self.marker);
        let _ = psql(&self.server, &end);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs `sql` in a transaction of a psql session of its own on a Postgres server, in the
/// background, and returns once the session sleeps with the transaction open (see [`Held`]).
/// `sql` must not end the transaction.
pub fn hold(server: &Server, sql: &str) -> Result<Held, String> {
    let marker = unique("dbee_test_hold");
    let script = format!("SET application_name = '{marker}'; BEGIN; {sql}; SELECT pg_sleep(60); ROLLBACK;");
    let mut c = compose();
    c.args(["exec", "-T", &server.name, "psql", "-U", "postgres", "-d", "dbee", "-At", "-v", "ON_ERROR_STOP=1", "-c", &script]);
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let child = c.spawn().map_err(|e| format!("docker compose exec {}: {e}", server.name))?;
    let held = Held { server: server.clone(), marker, child };
    let sleeping = format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = '{}' AND wait_event = 'PgSleep'", held.marker);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while psql(server, &sleeping)?.trim() != "1" {
        if std::time::Instant::now() > deadline {
            return Err(format!("the psql session on {} did not get to its sleep within 20 s", server.name));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(held)
}

/// `<repo>/target/testkit`: the SQLite seed and the copies [`targets`] makes. Under `target/`, as the
/// system temp directory may be small (a tmpfs, or a small partition).
fn testkit_dir() -> PathBuf {
    repo_root().join("target").join("testkit")
}

/// The SQLite seed built from `xtask/testdb/seed-sqlite.sql`: `target/testkit/seed-<hash>.sqlite`,
/// named by the seed's text and the SQLite version. Built at most once per test binary, and only
/// when no earlier run built it; tests use copies of it, never the file itself.
fn sqlite_master() -> Result<&'static Path, String> {
    static MASTER: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    MASTER.get_or_init(build_sqlite_master).as_deref().map_err(Clone::clone)
}

fn build_sqlite_master() -> Result<PathBuf, String> {
    let src = repo_root().join("xtask").join("testdb").join("seed-sqlite.sql");
    let sql = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut h = DefaultHasher::new();
    h.write(sql.as_bytes());
    h.write(rusqlite::version().as_bytes());
    let dir = testkit_dir();
    let path = dir.join(format!("seed-{:016x}.sqlite", h.finish()));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Built under a name of its own and renamed, so a test binary running at the same time never
    // copies half a file.
    let tmp = dir.join(format!("{}.tmp", unique(&format!("seed-build-{}", std::process::id()))));
    let built = rusqlite::Connection::open(&tmp).and_then(|c| {
        c.execute_batch(&sql)?;
        c.close().map_err(|(_, e)| e)
    });
    if let Err(e) = built {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", src.display()));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Seeds built from an earlier text of the file.
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("seed-") && name.ends_with(".sqlite") && entry.path() != path {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(path)
}

/// A fresh copy of the SQLite seed (`xtask/testdb/seed-sqlite.sql`) in `dir`, created if needed,
/// as `dir/seed-<pid>_<6 hex digits>.sqlite`. The seed is built at most once per test binary
/// (see `sqlite_master`); each call copies it. The caller removes the copy; keep `dir` under
/// `target/`, as a copy is about 70 MB. Failures are printed (the test then fails on the missing
/// file), never panics.
pub fn sqlite_seeded(dir: &Path) -> PathBuf {
    let dest = dir.join(format!("{}.sqlite", unique(&format!("seed-{}", std::process::id()))));
    let made = sqlite_master().and_then(|master| {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::copy(master, &dest).map(drop).map_err(|e| format!("copy to {}: {e}", dest.display()))
    });
    if let Err(e) = made {
        let _ = writeln!(std::io::stderr(), "sqlite_seeded: {e}");
    }
    dest
}

/// One database a test can run on: a kind, and a test server or a fresh copy of the SQLite seed,
/// which is removed when the target is dropped.
#[derive(Debug)]
pub struct TestTarget {
    kind: Kind,
    place: Place,
}

#[derive(Debug)]
enum Place {
    Server(&'static Server),
    File(PathBuf),
}

impl TestTarget {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The compose service (`pg14`, `mysql80`, …), or the kind's id for a file (`sqlite`).
    pub fn name(&self) -> String {
        match &self.place {
            Place::Server(s) => s.name.clone(),
            Place::File(_) => self.kind.id().into(),
        }
    }

    /// The test server, unless the target is a file.
    pub fn server(&self) -> Option<&'static Server> {
        match &self.place {
            Place::Server(s) => Some(s),
            Place::File(_) => None,
        }
    }

    /// The database file, for a target that is one.
    pub fn file(&self) -> Option<&Path> {
        match &self.place {
            Place::Server(_) => None,
            Place::File(path) => Some(path),
        }
    }

    pub fn is_file(&self) -> bool {
        self.file().is_some()
    }

    /// A query that runs about `secs` seconds, for cancel tests (on SQLite much longer).
    pub fn sleep_sql(&self, secs: u32) -> String {
        (self.kind.info().sleep_sql)(secs)
    }

    /// The rows of `big_rows` in this target's seed.
    pub fn big_rows(&self) -> u64 {
        self.server().map_or(BIG_ROWS, Server::big_rows)
    }
}

impl Drop for TestTarget {
    fn drop(&mut self) {
        if let Place::File(path) = &self.place {
            let _ = std::fs::remove_file(path);
            for suffix in ["-journal", "-wal", "-shm"] {
                let mut side = path.clone().into_os_string();
                side.push(suffix);
                let _ = std::fs::remove_file(side);
            }
        }
    }
}

/// Every database to run an engine test on: every test server, and one fresh SQLite copy (under
/// `target/testkit/copies`). Without the test servers only the SQLite copy.
pub fn targets() -> Vec<TestTarget> {
    let mut all = Vec::new();
    if let Some(db) = testdb() {
        all.extend(db.servers.iter().map(|s| TestTarget { kind: s.kind, place: Place::Server(s) }));
    }
    all.push(TestTarget { kind: Kind::Sqlite, place: Place::File(sqlite_seeded(&testkit_dir().join("copies"))) });
    all
}

/// A throwaway home directory with `.ssh/` (mode 0700), removed on drop.
#[derive(Debug)]
pub struct TempHome {
    pub home: PathBuf,
}

impl TempHome {
    /// `home/.ssh`.
    pub fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// A new home under the system temp directory with `.ssh/`. `with_key` is copied to
/// `.ssh/id_ed25519` (mode 0600); `known_hosts` becomes `.ssh/known_hosts`. Failures are printed
/// (the test then fails on the missing file), never panics.
pub fn temp_home(with_key: Option<&Path>, known_hosts: Option<&str>) -> TempHome {
    let home = std::env::temp_dir().join(unique(&format!("dbee-home-{}", std::process::id())));
    let th = TempHome { home };
    let ssh = th.ssh_dir();
    let warn = |what: &str, e: std::io::Error| {
        let _ = writeln!(std::io::stderr(), "temp_home: {what}: {e}");
    };
    if let Err(e) = std::fs::create_dir_all(&ssh) {
        warn(&ssh.display().to_string(), e);
    }
    set_mode(&ssh, 0o700);
    if let Some(key) = with_key {
        let dest = ssh.join("id_ed25519");
        match std::fs::copy(key, &dest) {
            Ok(_) => set_mode(&dest, 0o600),
            Err(e) => warn(&format!("copy {}", key.display()), e),
        }
    }
    if let Some(lines) = known_hosts {
        let mut text = lines.to_owned();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if let Err(e) = std::fs::write(ssh.join("known_hosts"), text) {
            warn("known_hosts", e);
        }
    }
    th
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        let _ = writeln!(std::io::stderr(), "temp_home: chmod {}: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn missing_env_file_means_no_testdb() {
        assert!(load_from(Path::new("/nonexistent/env.json")).is_none());
    }

    #[test]
    fn unique_names_differ() {
        assert_ne!(unique("t"), unique("t"));
        assert!(unique("t_x").starts_with("t_x_"));
    }

    #[test]
    fn unique_names_have_six_hex_digits() {
        let name = unique("t_edit");
        let suffix = name.strip_prefix("t_edit_").unwrap();
        assert_eq!(suffix.len(), 6);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
        let many: std::collections::HashSet<String> = (0..10_000).map(|_| unique("t")).collect();
        assert_eq!(many.len(), 10_000);
    }

    #[test]
    fn env_file_shape_parses() {
        let dir = temp_home(None, None);
        let path = dir.home.join("env.json");
        std::fs::write(
            &path,
            r#"{"password":"pw","servers":[{"name":"pg14","kind":"postgres","version":"14.19","port":49001,"servicePort":5432,"fullSeed":true},
                                           {"name":"pgnew","kind":"postgres","version":"18.0","port":49002,"servicePort":5432,"fullSeed":true},
                                           {"name":"mysql57","kind":"mysql","version":"5.7.44","port":49004,"servicePort":3306,"fullSeed":false},
                                           {"name":"mariadbnew","kind":"mariadb","version":"13.0.2","port":49005,"servicePort":3306,"fullSeed":true}],
                "sshd":{"port":49003,"user":"dbee","passwordUser":"dbeepw","knownHostsLine":"[127.0.0.1]:49003 ssh-ed25519 AAAA"},
                "keys":{"plain":"/k/id_ed25519","encrypted":"/k/id_ed25519_enc","passphrase":"pp","rsa":"/k/id_rsa"},
                "caFile":"/k/ca.crt"}"#,
        )
        .unwrap();
        let db = load_from(&path).unwrap();
        assert_eq!(db.password, "pw");
        let server = |name: &str, kind, version: &str, port, service_port, full_seed| Server {
            name: name.into(),
            kind,
            version: version.into(),
            port,
            service_port,
            full_seed,
        };
        assert_eq!(
            db.servers,
            vec![
                server("pg14", Kind::Postgres, "14.19", 49_001, 5432, true),
                server("pgnew", Kind::Postgres, "18.0", 49_002, 5432, true),
                server("mysql57", Kind::MySql, "5.7.44", 49_004, 3306, false),
                server("mariadbnew", Kind::MariaDb, "13.0.2", 49_005, 3306, true),
            ]
        );
        assert_eq!(db.servers_of(&[Kind::Postgres]).map(Server::major).collect::<Vec<_>>(), [14, 18]);
        assert_eq!(db.servers_of(&[Kind::MySql, Kind::MariaDb]).map(|s| s.name.as_str()).collect::<Vec<_>>(), ["mysql57", "mariadbnew"]);
        assert_eq!(db.server("mysql57").map(|s| (s.major(), s.big_rows())), Some((5, BIG_ROWS_LIGHT)));
        assert_eq!(db.server("mariadbnew").map(Server::big_rows), Some(BIG_ROWS));
        assert_eq!(db.server("nope"), None);
        assert_eq!(db.sshd.password_user, "dbeepw");
        assert_eq!(db.sshd.known_hosts_line, "[127.0.0.1]:49003 ssh-ed25519 AAAA");
        let text = std::fs::read_to_string(&path).unwrap();
        for unknown in [r#""kind":"percona""#, r#""kind":"sqlite""#] {
            std::fs::write(&path, text.replace(r#""kind":"mariadb""#, unknown)).unwrap();
            assert!(load_from(&path).is_none(), "only the server kinds: {unknown}");
        }
        assert_eq!(db.keys.encrypted, PathBuf::from("/k/id_ed25519_enc"));
        assert_eq!(db.keys.passphrase, "pp");
        assert_eq!(db.ca_file, PathBuf::from("/k/ca.crt"));

        std::fs::write(&path, r#"{"password":"pw"}"#).unwrap();
        assert!(load_from(&path).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_none());
    }

    #[test]
    fn kinds_have_their_ids_and_sleeps() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_id(k.id()), Some(k));
        }
        assert_eq!(Kind::Postgres.id(), "postgres");
        assert_eq!(Kind::from_id("oracle"), None);
        assert_eq!((Kind::MariaDb.info().sleep_sql)(3), "SELECT SLEEP(3)");
    }

    #[test]
    fn repo_root_holds_the_compose_file() {
        assert!(repo_root().join("xtask").join("testdb").join("compose.yml").is_file());
        assert!(env_file().ends_with("target/testdb/env.json"));
    }

    #[test]
    fn temp_home_has_key_and_known_hosts_and_goes_away() {
        let src = temp_home(None, None);
        let key = src.home.join("some_key");
        std::fs::write(&key, "KEY").unwrap();
        let home = temp_home(Some(&key), Some("[127.0.0.1]:1 ssh-ed25519 AAAA"));
        let ssh = home.home.join(".ssh");
        assert_eq!(std::fs::read_to_string(ssh.join("id_ed25519")).unwrap(), "KEY");
        assert_eq!(std::fs::read_to_string(ssh.join("known_hosts")).unwrap(), "[127.0.0.1]:1 ssh-ed25519 AAAA\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(ssh.join("id_ed25519")).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&ssh).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let path = home.home.clone();
        drop(home);
        assert!(!path.exists());

        let bare = temp_home(None, None);
        assert!(bare.ssh_dir().is_dir());
        assert!(!bare.ssh_dir().join("id_ed25519").exists() && !bare.ssh_dir().join("known_hosts").exists());
        assert_ne!(bare.home, src.home);
    }
}

//! dbee test kit (layer L0, a dev-dependency only): finds the Docker test servers, the skip rule,
//! a `psql` helper, unique names and throwaway home directories.
//!
//! The servers come from `cargo xtask testdb up`, which writes `<repo>/target/testdb/env.json`
//! (`xtask/src/testdb.rs`). A test that needs them starts with [`require_testdb!`]: without the
//! servers it prints `SKIPPED: …` and returns, and with `DBEE_REQUIRE_ORACLES=1` it fails instead.
//! The repository is found from this crate's manifest directory, never from the current directory.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::hash::{BuildHasher, Hasher, RandomState};
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

/// The test servers, as `env.json` describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDb {
    /// The `postgres` superuser's password, also the SSH password of [`Sshd::password_user`].
    pub password: String,
    /// `pg14` (major 14) and `pgnew` (the newest major), each on `127.0.0.1:<port>`, database `dbee`.
    pub servers: Vec<PgServer>,
    pub sshd: Sshd,
    pub keys: Keys,
    /// The CA that signed the servers' certificate (SAN `localhost`, `127.0.0.1`, `pg14`, `pgnew`).
    pub ca_file: PathBuf,
}

/// One Postgres server; `name` is its compose service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgServer {
    pub name: String,
    pub major: u32,
    pub port: u16,
}

/// The SSH server on `127.0.0.1:<port>`. `user` logs in with [`Keys`], `password_user` with
/// [`TestDb::password`]. Forwarding reaches `pg_host:pg_port` (and `pgnew:5432`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sshd {
    pub port: u16,
    pub user: String,
    pub password_user: String,
    /// `[127.0.0.1]:<port> ssh-ed25519 AAAA…`, ready for a `known_hosts` file.
    pub known_hosts_line: String,
    pub pg_host: String,
    pub pg_port: u16,
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
        servers.push(PgServer { name: text(s, "name")?, major: u32::try_from(s.get("major")?.as_u64()?).ok()?, port: port(s, "port")? });
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
            pg_host: text(sshd, "pgHost")?,
            pg_port: port(sshd, "pgPort")?,
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

/// `docker compose` for the test servers, with the variables `cargo xtask testdb up` wrote.
fn compose() -> Command {
    let root = repo_root();
    let mut c = Command::new("docker");
    c.current_dir(&root)
        .stdin(Stdio::null())
        .arg("compose")
        .args(["-p", PROJECT, "-f"])
        .arg(root.join("xtask").join("testdb").join("compose.yml"))
        .arg("--env-file")
        .arg(root.join("target").join("testdb").join("compose.env"));
    for var in ["PG_NEWEST", "DBEE_TEST_PASSWORD", "DBEE_TEST_PASSPHRASE"] {
        c.env_remove(var);
    }
    c
}

/// Run `sql` with `psql` inside the server's container, as `postgres` on database `dbee`
/// (`-At -v ON_ERROR_STOP=1 -c <sql>`): unaligned, tuples only. Returns stdout, or stderr on error.
pub fn psql(server: &PgServer, sql: &str) -> Result<String, String> {
    let mut c = compose();
    c.args(["exec", "-T", &server.name, "psql", "-U", "postgres", "-d", "dbee", "-At", "-v", "ON_ERROR_STOP=1", "-c", sql]);
    let out = c.output().map_err(|e| format!("docker compose exec {}: {e}", server.name))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("psql on {} ({}): {}", server.name, out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// A psql session of its own on a test server that ran `sql` in an open transaction and now
/// sleeps, until dropped (at most 60 s): what a lock `sql` took (`SELECT … FOR UPDATE`) is held
/// that long. Dropping it ends the session, which rolls the transaction back.
#[derive(Debug)]
pub struct Held {
    server: PgServer,
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

/// Runs `sql` in a transaction of a psql session of its own on `server`, in the background, and
/// returns once the session sleeps with the transaction open (see [`Held`]). `sql` must not end
/// the transaction.
pub fn hold(server: &PgServer, sql: &str) -> Result<Held, String> {
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
            r#"{"password":"pw","servers":[{"name":"pg14","major":14,"port":49001},{"name":"pgnew","major":18,"port":49002}],
                "sshd":{"port":49003,"user":"dbee","passwordUser":"dbeepw","knownHostsLine":"[127.0.0.1]:49003 ssh-ed25519 AAAA",
                        "pgHost":"pg14","pgPort":5432},
                "keys":{"plain":"/k/id_ed25519","encrypted":"/k/id_ed25519_enc","passphrase":"pp","rsa":"/k/id_rsa"},
                "caFile":"/k/ca.crt"}"#,
        )
        .unwrap();
        let db = load_from(&path).unwrap();
        assert_eq!(db.password, "pw");
        assert_eq!(
            db.servers,
            vec![PgServer { name: "pg14".into(), major: 14, port: 49_001 }, PgServer { name: "pgnew".into(), major: 18, port: 49_002 },]
        );
        assert_eq!(db.sshd.password_user, "dbeepw");
        assert_eq!(db.sshd.known_hosts_line, "[127.0.0.1]:49003 ssh-ed25519 AAAA");
        assert_eq!((db.sshd.pg_host.as_str(), db.sshd.pg_port), ("pg14", 5432));
        assert_eq!(db.keys.encrypted, PathBuf::from("/k/id_ed25519_enc"));
        assert_eq!(db.keys.passphrase, "pp");
        assert_eq!(db.ca_file, PathBuf::from("/k/ca.crt"));

        std::fs::write(&path, r#"{"password":"pw"}"#).unwrap();
        assert!(load_from(&path).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_none());
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

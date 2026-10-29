//! `cargo xtask testdb up|down|status`: the Docker test servers for driver tests.
//!
//! Compose project `dbee-testdb` (`xtask/testdb/compose.yml`), all published on random `127.0.0.1`
//! ports:
//! - Postgres 14 (`pg14`) and the newest Postgres major (`pgnew`, [`PG_NEWEST_MAJOR`]);
//! - MySQL 5.7 (`mysql57`), 8.0 (`mysql80`) and the newest (`mysqlnew`, [`MYSQL_NEWEST`]);
//! - MariaDB 10.11 (`mariadb1011`) and the newest (`mariadbnew`, [`MARIADB_NEWEST`]);
//! - an SSH server (`sshd`).
//!
//! `up` writes `<repo>/target/testdb/env.json` (always under the repo's
//! `target/`, whatever `CARGO_TARGET_DIR` says), which `dbee-testkit` reads; the shape is in
//! [`Env::to_json`]. It also writes `compose.env` there, the variables every `docker compose` call
//! passes with `--env-file`, so `exec`, `port` and `cp` parse the compose file the same way `up` did.
//!
//! `up` is idempotent: running containers and existing credentials are reused, so the seed runs
//! once. New credentials (no earlier `compose.env` or `env.json`) start from a clean project.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The compose project; nothing else on the machine is touched.
pub const PROJECT: &str = "dbee-testdb";

/// The newest Postgres major released on Docker Hub (`postgres:<n>-alpine`) when this was written.
/// `postgres:19-alpine` did not exist yet on 2026-10-19.
pub const PG_NEWEST_MAJOR: u32 = 18;

/// The newest MySQL release on Docker Hub (`mysql:<this>`) when this was written: on 2026-10-28
/// `mysql:26.10` was `latest` and `innovation` (26.10.0); `9.7` was `lts`.
pub const MYSQL_NEWEST: &str = "26.10";

/// The newest MariaDB release on Docker Hub (`mariadb:<this>`) when this was written: on 2026-10-28
/// `mariadb:13.0` was `latest` (13.0.2); 13.1 existed only as `-rc`, and `12.3` was `lts`.
pub const MARIADB_NEWEST: &str = "13.0";

/// `up --wait` gives the seeds (a million rows and a 20 MB cell) this long.
const WAIT_TIMEOUT_SECS: u32 = 900;

/// The MySQL and MariaDB services: name, flavor, and the release their image tag names.
const MY_SERVICES: [(&str, &str, &str); 5] = [
    ("mysql57", "mysql", "5.7"),
    ("mysql80", "mysql", "8.0"),
    ("mysqlnew", "mysql", MYSQL_NEWEST),
    ("mariadb1011", "mariadb", "10.11"),
    ("mariadbnew", "mariadb", MARIADB_NEWEST),
];

/// The compose file's variables.
const VAR_PG_NEWEST: &str = "PG_NEWEST";
const VAR_MYSQL_NEWEST: &str = "MYSQL_NEWEST";
const VAR_MARIADB_NEWEST: &str = "MARIADB_NEWEST";
const VAR_PASSWORD: &str = "DBEE_TEST_PASSWORD";
const VAR_PASSPHRASE: &str = "DBEE_TEST_PASSPHRASE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub major: u32,
    pub port: u16,
}

/// A MySQL or MariaDB server: `flavor` is `mysql` or `mariadb`, `version` what `SELECT VERSION()`
/// prints up to its first `-` (`8.0.46`, `10.11.19`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyServer {
    pub name: String,
    pub flavor: String,
    pub version: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sshd {
    pub port: u16,
    pub user: String,
    pub password_user: String,
    pub known_hosts_line: String,
    pub pg_host: String,
    pub pg_port: u16,
    pub my_host: String,
    pub my_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub plain: PathBuf,
    pub encrypted: PathBuf,
    pub passphrase: String,
    pub rsa: PathBuf,
}

/// The contents of `env.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub password: String,
    pub servers: Vec<Server>,
    pub mysql: Vec<MyServer>,
    pub sshd: Sshd,
    pub keys: Keys,
    pub ca_file: PathBuf,
}

impl Env {
    /// The binding shape read by `dbee-testkit`.
    pub fn to_json(&self) -> Value {
        let path = |p: &Path| Value::String(p.to_string_lossy().into_owned());
        json!({
            "password": self.password,
            "servers": self.servers.iter().map(|s| json!({"name": s.name, "major": s.major, "port": s.port})).collect::<Vec<_>>(),
            "mysql": self.mysql.iter().map(|s| json!({"name": s.name, "flavor": s.flavor, "version": s.version, "port": s.port})).collect::<Vec<_>>(),
            "sshd": {
                "port": self.sshd.port,
                "user": self.sshd.user,
                "passwordUser": self.sshd.password_user,
                "knownHostsLine": self.sshd.known_hosts_line,
                "pgHost": self.sshd.pg_host,
                "pgPort": self.sshd.pg_port,
                "myHost": self.sshd.my_host,
                "myPort": self.sshd.my_port,
            },
            "keys": {
                "plain": path(&self.keys.plain),
                "encrypted": path(&self.keys.encrypted),
                "passphrase": self.keys.passphrase,
                "rsa": path(&self.keys.rsa),
            },
            "caFile": path(&self.ca_file),
        })
    }

    pub fn from_json(v: &Value) -> Option<Env> {
        let text = |v: &Value, k: &str| v.get(k)?.as_str().map(str::to_owned);
        let port = |v: &Value, k: &str| u16::try_from(v.get(k)?.as_u64()?).ok();
        let mut servers = Vec::new();
        for s in v.get("servers")?.as_array()? {
            servers.push(Server { name: text(s, "name")?, major: u32::try_from(s.get("major")?.as_u64()?).ok()?, port: port(s, "port")? });
        }
        let mut mysql = Vec::new();
        for s in v.get("mysql")?.as_array()? {
            let flavor = text(s, "flavor").filter(|f| f == "mysql" || f == "mariadb")?;
            mysql.push(MyServer { name: text(s, "name")?, flavor, version: text(s, "version")?, port: port(s, "port")? });
        }
        let sshd = v.get("sshd")?;
        let keys = v.get("keys")?;
        Some(Env {
            password: text(v, "password")?,
            servers,
            mysql,
            sshd: Sshd {
                port: port(sshd, "port")?,
                user: text(sshd, "user")?,
                password_user: text(sshd, "passwordUser")?,
                known_hosts_line: text(sshd, "knownHostsLine")?,
                pg_host: text(sshd, "pgHost")?,
                pg_port: port(sshd, "pgPort")?,
                my_host: text(sshd, "myHost")?,
                my_port: port(sshd, "myPort")?,
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
}

/// Write `env` to `path` (directories created, mode 0600 on unix: it holds the password).
pub fn write_env(path: &Path, env: &Env) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&env.to_json()).map_err(|e| format!("env.json: {e}"))?;
    write_private(path, &format!("{text}\n"))
}

pub fn read_env(path: &Path) -> Option<Env> {
    let text = std::fs::read_to_string(path).ok()?;
    Env::from_json(&serde_json::from_str(&text).ok()?)
}

/// The values the compose file interpolates.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposeVars {
    pg_newest: u32,
    mysql_newest: String,
    mariadb_newest: String,
    password: String,
    passphrase: String,
}

impl ComposeVars {
    fn render(&self) -> String {
        format!(
            "# Written by `cargo xtask testdb up`; passed to every docker compose call with --env-file.\n\
             {VAR_PG_NEWEST}={}\n{VAR_MYSQL_NEWEST}={}\n{VAR_MARIADB_NEWEST}={}\n{VAR_PASSWORD}={}\n{VAR_PASSPHRASE}={}\n",
            self.pg_newest, self.mysql_newest, self.mariadb_newest, self.password, self.passphrase
        )
    }

    fn parse(text: &str) -> Option<ComposeVars> {
        let get =
            |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        Some(ComposeVars {
            pg_newest: get(VAR_PG_NEWEST)?.parse().ok()?,
            mysql_newest: get(VAR_MYSQL_NEWEST)?,
            mariadb_newest: get(VAR_MARIADB_NEWEST)?,
            password: get(VAR_PASSWORD)?,
            passphrase: get(VAR_PASSPHRASE)?,
        })
    }

    /// The images these variables name are the ones this xtask expects.
    fn current(&self) -> bool {
        self.pg_newest == PG_NEWEST_MAJOR && self.mysql_newest == MYSQL_NEWEST && self.mariadb_newest == MARIADB_NEWEST
    }
}

/// `<repo>/target/testdb`.
pub fn state_dir(root: &Path) -> PathBuf {
    root.join("target").join("testdb")
}

fn env_path(root: &Path) -> PathBuf {
    state_dir(root).join("env.json")
}

fn compose_env_path(root: &Path) -> PathBuf {
    state_dir(root).join("compose.env")
}

fn keys_dir(root: &Path) -> PathBuf {
    state_dir(root).join("keys")
}

fn compose_file(root: &Path) -> PathBuf {
    root.join("xtask").join("testdb").join("compose.yml")
}

/// `cargo xtask testdb <args>`.
pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    match args {
        ["up"] => up(root).map(|env| print_env(root, &env)),
        ["down"] => down(root),
        ["status"] => status(root),
        _ => Err("usage: cargo xtask testdb up|down|status".into()),
    }
}

/// `docker info` answers.
pub fn docker_available() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `docker compose -p dbee-testdb -f <compose.yml> --env-file <compose.env>`. Before the first `up`
/// there is no env file, and one written before a variable existed lacks it; placeholders for what
/// the file does not set then let `down` and `status` parse the compose file.
///
/// Builds attach no provenance or SBOM attestations, which would give every rebuild of the tools
/// image a new id, so `up --build` would recreate `init` and `sshd`. The variable does this for
/// any Compose; the compose file's `provenance`/`sbom` keys are refused by older ones (GitHub's).
fn compose(root: &Path) -> Command {
    let mut c = Command::new("docker");
    c.current_dir(root).stdin(Stdio::null()).env("BUILDX_NO_DEFAULT_ATTESTATIONS", "1");
    c.arg("compose").args(["-p", PROJECT, "-f"]).arg(compose_file(root));
    let env_file = compose_env_path(root);
    let in_file = std::fs::read_to_string(&env_file).ok();
    if in_file.is_some() {
        c.arg("--env-file").arg(env_file);
    }
    let placeholders = [
        (VAR_PG_NEWEST, PG_NEWEST_MAJOR.to_string()),
        (VAR_MYSQL_NEWEST, MYSQL_NEWEST.to_owned()),
        (VAR_MARIADB_NEWEST, MARIADB_NEWEST.to_owned()),
        (VAR_PASSWORD, "unset".to_owned()),
        (VAR_PASSPHRASE, "unset".to_owned()),
    ];
    for (var, placeholder) in placeholders {
        if in_file.as_deref().is_some_and(|text| sets(text, var)) {
            c.env_remove(var);
        } else {
            c.env(var, placeholder);
        }
    }
    c
}

/// `text`, an env file, has a line `var=…`.
fn sets(text: &str, var: &str) -> bool {
    text.lines().any(|l| l.strip_prefix(var).is_some_and(|rest| rest.starts_with('=')))
}

/// Run with inherited output.
fn run_shown(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

/// Run and return stdout.
fn run_captured(mut cmd: Command, what: &str) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{what}: exited with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// 24 hex characters from std's randomly keyed hasher.
fn secret() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let mut out = String::with_capacity(32);
    while out.len() < 24 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_usize(out.len());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(24);
    out
}

/// The port in `docker compose port` output (`127.0.0.1:49153`).
fn parse_port(out: &str) -> Option<u16> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    line.rsplit(':').next()?.parse().ok().filter(|p| *p != 0)
}

/// A `known_hosts` line for the host key `pubkey` (`ssh-ed25519 AAAA… comment`) on a local port.
fn known_hosts_line(port: u16, pubkey: &str) -> Option<String> {
    let mut fields = pubkey.split_whitespace();
    let (kind, key) = (fields.next()?, fields.next()?);
    Some(format!("[127.0.0.1]:{port} {kind} {key}"))
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    set_private(&tmp)?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn set_private(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Credentials to reuse: `compose.env` (what the containers were created with), else `env.json`.
fn existing_vars(root: &Path) -> Option<ComposeVars> {
    let from_file = std::fs::read_to_string(compose_env_path(root)).ok().and_then(|t| ComposeVars::parse(&t));
    let vars = from_file.or_else(|| {
        let env = read_env(&env_path(root))?;
        let pg_newest = env.servers.iter().find(|s| s.name == "pgnew")?.major;
        // The release is the version's first two parts (`26.10` of `26.10.0`).
        let release = |name: &str| {
            let version = &env.mysql.iter().find(|s| s.name == name)?.version;
            Some(version.split('.').take(2).collect::<Vec<_>>().join("."))
        };
        Some(ComposeVars {
            pg_newest,
            mysql_newest: release("mysqlnew")?,
            mariadb_newest: release("mariadbnew")?,
            password: env.password,
            passphrase: env.keys.passphrase,
        })
    })?;
    // A different newest release needs a fresh data directory.
    vars.current().then_some(vars)
}

fn compose_port(root: &Path, service: &str, port: u16) -> Result<u16, String> {
    let mut c = compose(root);
    c.args(["port", service, &port.to_string()]);
    let out = run_captured(c, &format!("docker compose port {service} {port}"))?;
    parse_port(&out).ok_or_else(|| format!("docker compose port {service} {port}: no port in {out:?}"))
}

fn server_major(root: &Path, service: &str) -> Result<u32, String> {
    let mut c = compose(root);
    c.args(["exec", "-T", service, "psql", "-U", "postgres", "-d", "dbee", "-At", "-c", "SHOW server_version_num"]);
    let out = run_captured(c, &format!("server version of {service}"))?;
    out.trim().parse::<u32>().map(|n| n / 10_000).map_err(|e| format!("server version of {service}: {out:?}: {e}"))
}

/// `SELECT VERSION()` on a MySQL or MariaDB service as root, up to its first `-` (`10.11.19` of
/// `10.11.19-MariaDB-ubu2204`). The password reaches the client as `MYSQL_PWD`, not as an argument.
fn my_version(root: &Path, service: &str, flavor: &str, password: &str) -> Result<String, String> {
    let client = if flavor == "mariadb" { "mariadb" } else { "mysql" };
    let mut c = compose(root);
    c.env("MYSQL_PWD", password);
    c.args(["exec", "-T", "-e", "MYSQL_PWD", service, client, "--user=root", "--batch", "--skip-column-names", "--execute=SELECT VERSION()"]);
    let out = run_captured(c, &format!("server version of {service}"))?;
    let version = out.trim().split('-').next().unwrap_or_default();
    if version.is_empty() { Err(format!("server version of {service}: {out:?}")) } else { Ok(version.to_owned()) }
}

/// The cipher of a TLS connection to a MySQL or MariaDB service: its own client over TCP, the CA
/// checked (`/secrets/ca.crt`). A server that cannot agree on a cipher fails here, not later as a
/// driver test's mystery (MySQL 5.7's image has none for an ECDSA key).
fn my_tls_cipher(root: &Path, service: &str, flavor: &str, password: &str) -> Result<String, String> {
    let mut c = compose(root);
    c.env("MYSQL_PWD", password);
    c.args(["exec", "-T", "-e", "MYSQL_PWD", service]);
    if flavor == "mariadb" {
        c.args(["mariadb", "--ssl", "--ssl-verify-server-cert"]);
    } else {
        c.args(["mysql", "--ssl-mode=VERIFY_CA"]);
    }
    c.args(["--ssl-ca=/secrets/ca.crt", "--host=127.0.0.1", "--protocol=TCP", "--user=root", "--batch", "--skip-column-names"]);
    c.arg("--execute=SHOW SESSION STATUS LIKE 'Ssl_cipher'");
    let out = run_captured(c, &format!("a TLS connection to {service}"))?;
    match out.split_whitespace().nth(1) {
        Some(cipher) => Ok(cipher.to_owned()),
        None => Err(format!("a TLS connection to {service}: no cipher in {out:?}")),
    }
}

/// Start (or reuse) the servers and write `env.json`.
pub fn up(root: &Path) -> Result<Env, String> {
    if !docker_available() {
        return Err("Docker is not available".into());
    }
    let vars = match existing_vars(root) {
        Some(v) => v,
        None => {
            // New credentials: volumes from an earlier run hold other passwords and keys.
            let mut c = compose(root);
            c.args(["down", "-v", "--remove-orphans"]);
            run_shown(c, "docker compose down -v (new credentials)")?;
            ComposeVars {
                pg_newest: PG_NEWEST_MAJOR,
                mysql_newest: MYSQL_NEWEST.into(),
                mariadb_newest: MARIADB_NEWEST.into(),
                password: secret(),
                passphrase: secret(),
            }
        }
    };
    write_private(&compose_env_path(root), &vars.render())?;

    let mut c = compose(root);
    c.args(["up", "-d", "--wait", "--wait-timeout", &WAIT_TIMEOUT_SECS.to_string(), "--build"]);
    run_shown(c, "docker compose up -d --wait --build")?;

    for (service, major) in [("pg14", 14), ("pgnew", PG_NEWEST_MAJOR)] {
        let got = server_major(root, service)?;
        if got != major {
            return Err(format!("{service} runs Postgres {got}, expected {major}"));
        }
    }
    let pg14 = compose_port(root, "pg14", 5432)?;
    let pgnew = compose_port(root, "pgnew", 5432)?;
    let mut mysql = Vec::new();
    for (name, flavor, release) in MY_SERVICES {
        let version = my_version(root, name, flavor, &vars.password)?;
        if !version.starts_with(&format!("{release}.")) {
            return Err(format!("{name} runs {flavor} {version}, expected {release}"));
        }
        my_tls_cipher(root, name, flavor, &vars.password)?;
        mysql.push(MyServer { name: name.into(), flavor: flavor.into(), version, port: compose_port(root, name, 3306)? });
    }
    let ssh = compose_port(root, "sshd", 22)?;

    let keys = keys_dir(root);
    if keys.exists() {
        std::fs::remove_dir_all(&keys).map_err(|e| format!("{}: {e}", keys.display()))?;
    }
    std::fs::create_dir_all(&keys).map_err(|e| format!("{}: {e}", keys.display()))?;
    let mut c = compose(root);
    c.args(["cp", "sshd:/secrets/."]).arg(&keys);
    run_captured(c, "docker compose cp sshd:/secrets/.")?;
    // tools-init.sh's completion marker is not a key, and mysql/server.key is server.key again.
    let _ = std::fs::remove_file(keys.join(".done"));
    let _ = std::fs::remove_dir_all(keys.join("mysql"));
    for entry in std::fs::read_dir(&keys).map_err(|e| format!("{}: {e}", keys.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", keys.display()))?;
        if entry.path().is_file() {
            set_private(&entry.path())?;
        }
    }

    let mut c = compose(root);
    c.args(["exec", "-T", "sshd", "cat", "/etc/ssh/ssh_host_ed25519_key.pub"]);
    let host_key = run_captured(c, "read the sshd host key")?;
    let known_hosts_line = known_hosts_line(ssh, &host_key).ok_or_else(|| format!("sshd host key: unexpected {host_key:?}"))?;

    let env = Env {
        password: vars.password,
        servers: vec![Server { name: "pg14".into(), major: 14, port: pg14 }, Server { name: "pgnew".into(), major: PG_NEWEST_MAJOR, port: pgnew }],
        mysql,
        sshd: Sshd {
            port: ssh,
            user: "dbee".into(),
            password_user: "dbeepw".into(),
            known_hosts_line,
            pg_host: "pg14".into(),
            pg_port: 5432,
            my_host: "mysql80".into(),
            my_port: 3306,
        },
        keys: Keys { plain: keys.join("id_ed25519"), encrypted: keys.join("id_ed25519_enc"), passphrase: vars.passphrase, rsa: keys.join("id_rsa") },
        ca_file: keys.join("ca.crt"),
    };
    write_env(&env_path(root), &env)?;
    Ok(env)
}

fn print_env(root: &Path, env: &Env) {
    println!("test servers ({PROJECT}):");
    for s in &env.servers {
        println!("  {:<11} {:<17} 127.0.0.1:{}", s.name, format!("Postgres {}", s.major), s.port);
    }
    for s in &env.mysql {
        let product = if s.flavor == "mariadb" { "MariaDB" } else { "MySQL" };
        println!("  {:<11} {:<17} 127.0.0.1:{}", s.name, format!("{product} {}", s.version), s.port);
    }
    println!("  {:<11} {:<17} 127.0.0.1:{} (users {}, {})", "sshd", "SSH", env.sshd.port, env.sshd.user, env.sshd.password_user);
    println!("env file: {}", env_path(root).display());
}

/// Stop and remove the servers, their volumes and `target/testdb`.
pub fn down(root: &Path) -> Result<(), String> {
    if !docker_available() {
        return Err("Docker is not available".into());
    }
    let mut c = compose(root);
    c.args(["down", "-v", "--remove-orphans"]);
    run_shown(c, "docker compose down -v")?;
    let dir = state_dir(root);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Print the env file's servers and `docker compose ps`.
pub fn status(root: &Path) -> Result<(), String> {
    match read_env(&env_path(root)) {
        Some(env) => print_env(root, &env),
        None => println!("no env file at {} (run `cargo xtask testdb up`)", env_path(root).display()),
    }
    if !docker_available() {
        println!("Docker is not available");
        return Ok(());
    }
    let mut c = compose(root);
    c.args(["ps", "--all"]);
    run_shown(c, "docker compose ps --all")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn env_json_round_trip() {
        let keys = PathBuf::from("/repo/target/testdb/keys");
        let env = Env {
            password: "0123456789abcdef01234567".into(),
            servers: vec![
                Server { name: "pg14".into(), major: 14, port: 49_001 },
                Server { name: "pgnew".into(), major: PG_NEWEST_MAJOR, port: 49_002 },
            ],
            mysql: vec![
                MyServer { name: "mysql57".into(), flavor: "mysql".into(), version: "5.7.44".into(), port: 49_004 },
                MyServer { name: "mariadbnew".into(), flavor: "mariadb".into(), version: "13.0.2".into(), port: 49_005 },
            ],
            sshd: Sshd {
                port: 49_003,
                user: "dbee".into(),
                password_user: "dbeepw".into(),
                known_hosts_line: "[127.0.0.1]:49003 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample".into(),
                pg_host: "pg14".into(),
                pg_port: 5432,
                my_host: "mysql80".into(),
                my_port: 3306,
            },
            keys: Keys {
                plain: keys.join("id_ed25519"),
                encrypted: keys.join("id_ed25519_enc"),
                passphrase: "fedcba9876543210fedcba98".into(),
                rsa: keys.join("id_rsa"),
            },
            ca_file: keys.join("ca.crt"),
        };
        let dir = std::env::temp_dir().join(format!("dbee-xtask-env-{}", std::process::id()));
        let path = dir.join("env.json");
        write_env(&path, &env).unwrap();
        let back = read_env(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, env);
        assert_eq!(back.servers[1].port, 49_002);
        assert_eq!(back.sshd.port, 49_003);
        assert_eq!(back.mysql[1].flavor, "mariadb");
        assert_eq!((back.sshd.my_host.as_str(), back.sshd.my_port), ("mysql80", 3306));
        assert_eq!(back.keys.encrypted, keys.join("id_ed25519_enc"));
        assert_eq!(back.ca_file, keys.join("ca.crt"));
    }

    #[test]
    fn env_json_has_the_binding_keys() {
        let v: Value = serde_json::from_str(
            r#"{"password":"p","servers":[{"name":"pg14","major":14,"port":1},{"name":"pgnew","major":18,"port":2}],
                "mysql":[{"name":"mysql80","flavor":"mysql","version":"8.0.46","port":4},
                         {"name":"mariadb1011","flavor":"mariadb","version":"10.11.19","port":5}],
                "sshd":{"port":3,"user":"dbee","passwordUser":"dbeepw","knownHostsLine":"[127.0.0.1]:3 ssh-ed25519 AAAA",
                        "pgHost":"pg14","pgPort":5432,"myHost":"mysql80","myPort":3306},
                "keys":{"plain":"/k/id_ed25519","encrypted":"/k/id_ed25519_enc","passphrase":"q","rsa":"/k/id_rsa"},
                "caFile":"/k/ca.crt"}"#,
        )
        .unwrap();
        let env = Env::from_json(&v).unwrap();
        assert_eq!(env.to_json(), v);
        assert_eq!(env.sshd.known_hosts_line, "[127.0.0.1]:3 ssh-ed25519 AAAA");
        // A port that does not fit in u16 is not an env file.
        let mut bad = v.clone();
        bad["sshd"]["port"] = json!(70_000);
        assert!(Env::from_json(&bad).is_none());
        // Only the two flavors.
        let mut bad = v.clone();
        bad["mysql"][0]["flavor"] = json!("percona");
        assert!(Env::from_json(&bad).is_none());
    }

    #[test]
    fn compose_vars_round_trip() {
        let vars = ComposeVars {
            pg_newest: PG_NEWEST_MAJOR,
            mysql_newest: MYSQL_NEWEST.into(),
            mariadb_newest: MARIADB_NEWEST.into(),
            password: "abc".into(),
            passphrase: "def".into(),
        };
        assert!(vars.current());
        assert_eq!(ComposeVars::parse(&vars.render()), Some(vars.clone()));
        assert_eq!(ComposeVars::parse("PG_NEWEST=18\nDBEE_TEST_PASSWORD=\nDBEE_TEST_PASSPHRASE=x\n"), None);
        // A compose.env from before MySQL: new credentials, a clean project.
        assert_eq!(ComposeVars::parse("PG_NEWEST=18\nDBEE_TEST_PASSWORD=abc\nDBEE_TEST_PASSPHRASE=def\n"), None);
        assert!(!ComposeVars { mysql_newest: "9.7".into(), ..vars }.current());
    }

    #[test]
    fn an_env_file_from_before_mysql_still_parses() {
        let root = std::env::temp_dir().join(format!("dbee-xtask-compose-{}", std::process::id()));
        write_private(&compose_env_path(&root), "PG_NEWEST=18\nDBEE_TEST_PASSWORD=pw\nDBEE_TEST_PASSPHRASE=pp\n").unwrap();
        let c = compose(&root);
        let envs: Vec<_> = c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        std::fs::remove_dir_all(&root).unwrap();
        // The file's own values win; what it lacks gets a placeholder.
        assert!(envs.contains(&("PG_NEWEST".into(), None)), "{envs:?}");
        assert!(envs.contains(&("DBEE_TEST_PASSWORD".into(), None)), "{envs:?}");
        assert!(envs.contains(&("MYSQL_NEWEST".into(), Some(MYSQL_NEWEST.into()))), "{envs:?}");
        assert!(envs.contains(&("MARIADB_NEWEST".into(), Some(MARIADB_NEWEST.into()))), "{envs:?}");
        assert!(c.get_args().any(|a| a == "--env-file"));
        assert!(sets("A=1\nMYSQL_NEWEST=26.10\n", "MYSQL_NEWEST") && !sets("MYSQL_NEWEST_X=1\n", "MYSQL_NEWEST"));
    }

    #[test]
    fn secrets_are_24_hex_and_differ() {
        let (a, b) = (secret(), secret());
        assert_eq!(a.len(), 24);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn port_from_compose_output() {
        assert_eq!(parse_port("127.0.0.1:49153\n"), Some(49_153));
        assert_eq!(parse_port("\n[::1]:5432\n"), Some(5432));
        assert_eq!(parse_port(""), None);
        assert_eq!(parse_port("127.0.0.1:0"), None);
        assert_eq!(parse_port("127.0.0.1:99999"), None);
    }

    #[test]
    fn known_hosts_line_from_host_key() {
        assert_eq!(known_hosts_line(2222, "ssh-ed25519 AAAAC3Nza dbee-testdb-host\n").as_deref(), Some("[127.0.0.1]:2222 ssh-ed25519 AAAAC3Nza"));
        assert_eq!(known_hosts_line(2222, "garbage"), None);
    }
}

//! `cargo xtask testdb up|down|status`: the Docker test servers for driver tests.
//!
//! Compose project `dbee-testdb` (`xtask/testdb/compose.yml`), all published on random `127.0.0.1`
//! ports: the database servers of [`SERVICES`] and an SSH server (`sshd`). [`SERVICES`] is the one
//! list of the database servers: the compose file takes each one's image from it and the
//! certificate its names (both through `compose.env`), `up` checks each one against it, and
//! `env.json` lists them as `up` found them. What differs between database types is in
//! [`Kind::info`].
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

/// `up --wait` gives the seeds (a million rows and a 20 MB cell) this long.
const WAIT_TIMEOUT_SECS: u32 = 900;

/// A database server of the compose project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    /// The compose service, also its host name inside the project's network.
    pub name: &'static str,
    pub kind: Kind,
    /// The image it runs.
    pub image: &'static str,
    /// What its version starts with: `14` for `14.19`, `8.0` for `8.0.46`.
    pub release: &'static str,
    /// It holds the full seed: a million rows in `shop.big_rows` and the 20 MB row of `shop.big_cell`
    /// (`seed-mysql.sql` gives the others less, telling them by their host name).
    pub full_seed: bool,
}

/// Every database server, in the order `env.json` lists them. Adding one of a known kind is a row
/// here and its block in the compose file.
pub const SERVICES: [Service; 7] = [
    Service { name: "pg14", kind: Kind::Postgres, image: "postgres:14-alpine", release: "14", full_seed: true },
    // The newest Postgres major on Docker Hub when this was written: postgres:19-alpine did not exist
    // yet on 2026-10-19.
    Service { name: "pgnew", kind: Kind::Postgres, image: "postgres:18-alpine", release: "18", full_seed: true },
    // Built for amd64 only.
    Service { name: "mysql57", kind: Kind::MySql, image: "mysql:5.7", release: "5.7", full_seed: false },
    Service { name: "mysql80", kind: Kind::MySql, image: "mysql:8.0", release: "8.0", full_seed: true },
    // The newest MySQL release on Docker Hub when this was written: on 2026-10-28 mysql:26.10 was
    // `latest` and `innovation` (26.10.0); 9.7 was `lts`.
    Service { name: "mysqlnew", kind: Kind::MySql, image: "mysql:26.10", release: "26.10", full_seed: false },
    Service { name: "mariadb1011", kind: Kind::MariaDb, image: "mariadb:10.11", release: "10.11", full_seed: false },
    // The newest MariaDB release on Docker Hub when this was written: on 2026-10-28 mariadb:13.0 was
    // `latest` (13.0.2); 13.1 existed only as -rc, and 12.3 was `lts`.
    Service { name: "mariadbnew", kind: Kind::MariaDb, image: "mariadb:13.0", release: "13.0", full_seed: true },
];

/// A database server's type, by the id dbee saves it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Postgres,
    MySql,
    MariaDb,
}

/// What `up` needs to know of a database type.
pub struct KindInfo {
    pub id: &'static str,
    /// What `status` calls it.
    pub product: &'static str,
    /// The port its servers listen on inside the network (the compose file's block for the kind
    /// publishes it).
    pub port: u16,
    /// Its client inside a server's container.
    pub client: Client,
    /// Prints the server's version: `14.19`, `8.0.46`.
    pub version_sql: &'static str,
    /// The client and its arguments for a TLS connection with the CA checked, when `up` checks one
    /// (Postgres's TLS is checked by `dbee-testkit`'s smoke test).
    pub tls: Option<&'static [&'static str]>,
}

/// A database client inside a server's container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    /// `psql` as `postgres` on database `dbee`.
    Psql,
    /// The `mysql` client or one taking the same arguments (`mariadb`), as `root`.
    MySql(&'static str),
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Postgres, Kind::MySql, Kind::MariaDb];

    /// The one `match` on a kind.
    pub fn info(self) -> &'static KindInfo {
        match self {
            Kind::Postgres => &KindInfo {
                id: "postgres",
                product: "Postgres",
                port: 5432,
                client: Client::Psql,
                version_sql: "SELECT split_part(current_setting('server_version'), ' ', 1)",
                tls: None,
            },
            Kind::MySql => &KindInfo {
                id: "mysql",
                product: "MySQL",
                port: 3306,
                client: Client::MySql("mysql"),
                version_sql: "SELECT SUBSTRING_INDEX(VERSION(), '-', 1)",
                tls: Some(&["mysql", "--ssl-mode=VERIFY_CA"]),
            },
            Kind::MariaDb => &KindInfo {
                id: "mariadb",
                product: "MariaDB",
                port: 3306,
                client: Client::MySql("mariadb"),
                version_sql: "SELECT SUBSTRING_INDEX(VERSION(), '-', 1)",
                tls: Some(&["mariadb", "--ssl", "--ssl-verify-server-cert"]),
            },
        }
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// The compose file's variables besides the images ([`image_var`]).
const VAR_PASSWORD: &str = "DBEE_TEST_PASSWORD";
const VAR_PASSPHRASE: &str = "DBEE_TEST_PASSPHRASE";
/// The database services' names, comma-separated, for the servers' certificate.
const VAR_HOSTS: &str = "DBEE_TEST_HOSTS";

/// The variable the compose file takes `service`'s image from: `DBEE_IMAGE_PG14`.
fn image_var(service: &str) -> String {
    format!("DBEE_IMAGE_{}", service.to_ascii_uppercase())
}

/// [`VAR_HOSTS`]'s value.
fn hosts() -> String {
    SERVICES.iter().map(|s| s.name).collect::<Vec<_>>().join(",")
}

/// Each service's image variable and image.
fn images() -> Vec<(String, String)> {
    SERVICES.iter().map(|s| (image_var(s.name), s.image.to_owned())).collect()
}

/// A database server as `up` found it. `version` is what the server prints (`14.19`, `8.0.46`,
/// `10.11.19`); `port` is on `127.0.0.1`, `service_port` inside the network, where the sshd
/// reaches it as `name:service_port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub kind: Kind,
    pub version: String,
    pub port: u16,
    pub service_port: u16,
    pub full_seed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sshd {
    pub port: u16,
    pub user: String,
    pub password_user: String,
    pub known_hosts_line: String,
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
    pub sshd: Sshd,
    pub keys: Keys,
    pub ca_file: PathBuf,
}

impl Env {
    /// The binding shape read by `dbee-testkit`.
    pub fn to_json(&self) -> Value {
        let path = |p: &Path| Value::String(p.to_string_lossy().into_owned());
        let server = |s: &Server| {
            json!({
                "name": s.name,
                "kind": s.kind.id(),
                "version": s.version,
                "port": s.port,
                "servicePort": s.service_port,
                "fullSeed": s.full_seed,
            })
        };
        json!({
            "password": self.password,
            "servers": self.servers.iter().map(server).collect::<Vec<_>>(),
            "sshd": {
                "port": self.sshd.port,
                "user": self.sshd.user,
                "passwordUser": self.sshd.password_user,
                "knownHostsLine": self.sshd.known_hosts_line,
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
            servers.push(Server {
                name: text(s, "name")?,
                kind: Kind::from_id(&text(s, "kind")?)?,
                version: text(s, "version")?,
                port: port(s, "port")?,
                service_port: port(s, "servicePort")?,
                full_seed: s.get("fullSeed")?.as_bool()?,
            });
        }
        let sshd = v.get("sshd")?;
        let keys = v.get("keys")?;
        Some(Env {
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
    password: String,
    passphrase: String,
    /// [`VAR_HOSTS`].
    hosts: String,
    /// Each service's [`image_var`] and image, in [`SERVICES`] order.
    images: Vec<(String, String)>,
}

impl ComposeVars {
    /// The variables for [`SERVICES`] with these credentials.
    fn new(password: String, passphrase: String) -> ComposeVars {
        ComposeVars { password, passphrase, hosts: hosts(), images: images() }
    }

    fn render(&self) -> String {
        let mut text = "# Written by `cargo xtask testdb up`; passed to every docker compose call with --env-file.\n".to_owned();
        for (var, value) in [(VAR_PASSWORD, &self.password), (VAR_PASSPHRASE, &self.passphrase), (VAR_HOSTS, &self.hosts)] {
            text.push_str(&format!("{var}={value}\n"));
        }
        for (var, image) in &self.images {
            text.push_str(&format!("{var}={image}\n"));
        }
        text
    }

    /// The file's values; `None` when one of [`SERVICES`]' variables is missing or empty.
    fn parse(text: &str) -> Option<ComposeVars> {
        let get =
            |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        let mut images = Vec::new();
        for s in &SERVICES {
            let var = image_var(s.name);
            let image = get(&var)?;
            images.push((var, image));
        }
        Some(ComposeVars { password: get(VAR_PASSWORD)?, passphrase: get(VAR_PASSPHRASE)?, hosts: get(VAR_HOSTS)?, images })
    }

    /// The images and names these variables give are the ones [`SERVICES`] has.
    fn current(&self) -> bool {
        self.hosts == hosts() && self.images == images()
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
    let mut placeholders =
        vec![(VAR_PASSWORD.to_owned(), "unset".to_owned()), (VAR_PASSPHRASE.to_owned(), "unset".to_owned()), (VAR_HOSTS.to_owned(), hosts())];
    placeholders.extend(images());
    for (var, placeholder) in placeholders {
        if in_file.as_deref().is_some_and(|text| sets(text, &var)) {
            c.env_remove(&var);
        } else {
            c.env(&var, placeholder);
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

/// Credentials to reuse: `compose.env` (what the containers were created with), else `env.json`
/// when it lists every service at its release.
fn existing_vars(root: &Path) -> Option<ComposeVars> {
    let from_file = std::fs::read_to_string(compose_env_path(root)).ok().and_then(|t| ComposeVars::parse(&t));
    let vars = from_file.or_else(|| {
        let env = read_env(&env_path(root))?;
        let listed = |s: &Service| env.servers.iter().any(|e| e.name == s.name && e.version.starts_with(&format!("{}.", s.release)));
        SERVICES.iter().all(listed).then(|| ComposeVars::new(env.password, env.keys.passphrase))
    })?;
    // Another image or another set of services needs fresh data directories and certificates.
    vars.current().then_some(vars)
}

fn compose_port(root: &Path, service: &str, port: u16) -> Result<u16, String> {
    let mut c = compose(root);
    c.args(["port", service, &port.to_string()]);
    let out = run_captured(c, &format!("docker compose port {service} {port}"))?;
    parse_port(&out).ok_or_else(|| format!("docker compose port {service} {port}: no port in {out:?}"))
}

/// Runs `sql` with the kind's client inside the service's container and returns what it prints,
/// trimmed. The password reaches a MySQL client as `MYSQL_PWD`, not as an argument.
fn query(root: &Path, s: &Service, password: &str, sql: &str) -> Result<String, String> {
    let mut c = compose(root);
    match s.kind.info().client {
        Client::Psql => {
            c.args(["exec", "-T", s.name, "psql", "-U", "postgres", "-d", "dbee", "-At", "-c", sql]);
        }
        Client::MySql(program) => {
            c.env("MYSQL_PWD", password);
            c.args(["exec", "-T", "-e", "MYSQL_PWD", s.name, program, "--user=root", "--batch", "--skip-column-names"]);
            c.arg(format!("--execute={sql}"));
        }
    }
    Ok(run_captured(c, &format!("a query on {}", s.name))?.trim().to_owned())
}

/// The cipher of a TLS connection to a service with its own client over TCP, the CA checked
/// (`/secrets/ca.crt`). A server that cannot agree on a cipher fails here, not later as a driver
/// test's mystery (MySQL 5.7's image has none for an ECDSA key).
fn tls_cipher(root: &Path, s: &Service, client: &[&str], password: &str) -> Result<String, String> {
    let mut c = compose(root);
    c.env("MYSQL_PWD", password);
    c.args(["exec", "-T", "-e", "MYSQL_PWD", s.name]);
    c.args(client);
    c.args(["--ssl-ca=/secrets/ca.crt", "--host=127.0.0.1", "--protocol=TCP", "--user=root", "--batch", "--skip-column-names"]);
    c.arg("--execute=SHOW SESSION STATUS LIKE 'Ssl_cipher'");
    let out = run_captured(c, &format!("a TLS connection to {}", s.name))?;
    match out.split_whitespace().nth(1) {
        Some(cipher) => Ok(cipher.to_owned()),
        None => Err(format!("a TLS connection to {}: no cipher in {out:?}", s.name)),
    }
}

/// Checks the running service against its row: its version and its seed, and its TLS where the
/// kind has a check. The server as `env.json` lists it.
fn check(root: &Path, s: &Service, password: &str) -> Result<Server, String> {
    let info = s.kind.info();
    let version = query(root, s, password, info.version_sql)?;
    if !version.starts_with(&format!("{}.", s.release)) {
        return Err(format!("{} runs {} {version}, expected {}", s.name, info.product, s.release));
    }
    if let Some(client) = info.tls {
        tls_cipher(root, s, client, password)?;
    }
    let full = query(root, s, password, "SELECT COUNT(*) FROM shop.big_cell")? == "1";
    if full != s.full_seed {
        return Err(format!("{}: the seed's big tables are {}, but its row in SERVICES says otherwise", s.name, if full { "full" } else { "light" }));
    }
    Ok(Server {
        name: s.name.into(),
        kind: s.kind,
        version,
        port: compose_port(root, s.name, info.port)?,
        service_port: info.port,
        full_seed: s.full_seed,
    })
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
            ComposeVars::new(secret(), secret())
        }
    };
    write_private(&compose_env_path(root), &vars.render())?;

    let mut c = compose(root);
    c.args(["up", "-d", "--wait", "--wait-timeout", &WAIT_TIMEOUT_SECS.to_string(), "--build"]);
    run_shown(c, "docker compose up -d --wait --build")?;

    let servers = SERVICES.iter().map(|s| check(root, s, &vars.password)).collect::<Result<Vec<_>, _>>()?;
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
        servers,
        sshd: Sshd { port: ssh, user: "dbee".into(), password_user: "dbeepw".into(), known_hosts_line },
        keys: Keys { plain: keys.join("id_ed25519"), encrypted: keys.join("id_ed25519_enc"), passphrase: vars.passphrase, rsa: keys.join("id_rsa") },
        ca_file: keys.join("ca.crt"),
    };
    write_env(&env_path(root), &env)?;
    Ok(env)
}

fn print_env(root: &Path, env: &Env) {
    println!("test servers ({PROJECT}):");
    for s in &env.servers {
        println!("  {:<11} {:<17} 127.0.0.1:{}", s.name, format!("{} {}", s.kind.info().product, s.version), s.port);
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

    fn server(name: &str, kind: Kind, version: &str, port: u16, full_seed: bool) -> Server {
        Server { name: name.into(), kind, version: version.into(), port, service_port: kind.info().port, full_seed }
    }

    #[test]
    fn env_json_round_trip() {
        let keys = PathBuf::from("/repo/target/testdb/keys");
        let env = Env {
            password: "0123456789abcdef01234567".into(),
            servers: vec![
                server("pg14", Kind::Postgres, "14.19", 49_001, true),
                server("pgnew", Kind::Postgres, "18.0", 49_002, true),
                server("mysql57", Kind::MySql, "5.7.44", 49_004, false),
                server("mariadbnew", Kind::MariaDb, "13.0.2", 49_005, true),
            ],
            sshd: Sshd {
                port: 49_003,
                user: "dbee".into(),
                password_user: "dbeepw".into(),
                known_hosts_line: "[127.0.0.1]:49003 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample".into(),
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
        assert_eq!(back.servers[3].kind, Kind::MariaDb);
        assert_eq!((back.servers[0].service_port, back.servers[2].service_port), (5432, 3306));
        assert!(!back.servers[2].full_seed && back.servers[3].full_seed);
        assert_eq!(back.keys.encrypted, keys.join("id_ed25519_enc"));
        assert_eq!(back.ca_file, keys.join("ca.crt"));
    }

    #[test]
    fn env_json_has_the_binding_keys() {
        let v: Value = serde_json::from_str(
            r#"{"password":"p","servers":[{"name":"pg14","kind":"postgres","version":"14.19","port":1,"servicePort":5432,"fullSeed":true},
                                          {"name":"mysql80","kind":"mysql","version":"8.0.46","port":4,"servicePort":3306,"fullSeed":true},
                                          {"name":"mariadb1011","kind":"mariadb","version":"10.11.19","port":5,"servicePort":3306,"fullSeed":false}],
                "sshd":{"port":3,"user":"dbee","passwordUser":"dbeepw","knownHostsLine":"[127.0.0.1]:3 ssh-ed25519 AAAA"},
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
        // Only the known kinds.
        let mut bad = v.clone();
        bad["servers"][1]["kind"] = json!("percona");
        assert!(Env::from_json(&bad).is_none());
    }

    #[test]
    fn services_are_one_list() {
        let names: Vec<&str> = SERVICES.iter().map(|s| s.name).collect();
        assert_eq!(names, ["pg14", "pgnew", "mysql57", "mysql80", "mysqlnew", "mariadb1011", "mariadbnew"]);
        let full: Vec<&str> = SERVICES.iter().filter(|s| s.full_seed).map(|s| s.name).collect();
        assert_eq!(full, ["pg14", "pgnew", "mysql80", "mariadbnew"]);
        for s in &SERVICES {
            assert!(s.image.split_once(':').is_some_and(|(_, tag)| tag.starts_with(s.release)), "{s:?}");
            assert_eq!(names.iter().filter(|n| **n == s.name).count(), 1, "{}", s.name);
        }
        for k in Kind::ALL {
            assert_eq!(Kind::from_id(k.id()), Some(k));
        }
        assert_eq!(Kind::from_id("sqlite"), None, "no server");
        assert_eq!(image_var("mariadb1011"), "DBEE_IMAGE_MARIADB1011");
    }

    #[test]
    fn compose_vars_round_trip() {
        let vars = ComposeVars::new("abc".into(), "def".into());
        assert!(vars.current());
        assert_eq!(ComposeVars::parse(&vars.render()), Some(vars.clone()));
        assert!(vars.render().contains("DBEE_IMAGE_PGNEW=postgres:18-alpine\n"), "{}", vars.render());
        assert!(vars.render().contains("DBEE_TEST_HOSTS=pg14,pgnew,mysql57,mysql80,mysqlnew,mariadb1011,mariadbnew\n"));
        let without = |var: &str| vars.render().lines().filter(|l| !l.starts_with(var)).collect::<Vec<_>>().join("\n");
        assert_eq!(ComposeVars::parse(&without(VAR_PASSWORD)), None);
        assert_eq!(ComposeVars::parse(&vars.render().replace("DBEE_TEST_PASSWORD=abc", "DBEE_TEST_PASSWORD=")), None);
        // A compose.env from before a service existed: new credentials, a clean project.
        assert_eq!(ComposeVars::parse(&without("DBEE_IMAGE_MYSQLNEW")), None);
        assert_eq!(ComposeVars::parse("PG_NEWEST=18\nDBEE_TEST_PASSWORD=abc\nDBEE_TEST_PASSPHRASE=def\n"), None);
        // Another image is not current.
        let mut other = vars.clone();
        other.images[4].1 = "mysql:9.7".into();
        assert!(!other.current());
        assert!(!ComposeVars { hosts: "pg14".into(), ..vars }.current());
    }

    #[test]
    fn an_env_file_without_some_variables_still_parses() {
        let root = std::env::temp_dir().join(format!("dbee-xtask-compose-{}", std::process::id()));
        write_private(&compose_env_path(&root), "DBEE_TEST_PASSWORD=pw\nDBEE_TEST_PASSPHRASE=pp\nDBEE_IMAGE_PG14=postgres:14-alpine\n").unwrap();
        let c = compose(&root);
        let envs: Vec<_> = c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        std::fs::remove_dir_all(&root).unwrap();
        // The file's own values win; what it lacks gets a placeholder.
        assert!(envs.contains(&("DBEE_IMAGE_PG14".into(), None)), "{envs:?}");
        assert!(envs.contains(&("DBEE_TEST_PASSWORD".into(), None)), "{envs:?}");
        assert!(envs.contains(&("DBEE_IMAGE_MYSQLNEW".into(), Some("mysql:26.10".into()))), "{envs:?}");
        assert!(envs.contains(&("DBEE_TEST_HOSTS".into(), Some(hosts()))), "{envs:?}");
        assert!(c.get_args().any(|a| a == "--env-file"));
        assert!(sets("A=1\nDBEE_IMAGE_PG14=x\n", "DBEE_IMAGE_PG14") && !sets("DBEE_IMAGE_PG14_X=1\n", "DBEE_IMAGE_PG14"));
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

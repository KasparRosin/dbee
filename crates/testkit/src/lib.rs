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

/// What a skipped test says when the env file is not in this tree's shape.
pub const OLD_ENV: &str =
    "target/testdb/env.json was written by an older tree's `cargo xtask testdb up` (run `cargo xtask testdb down`, then `cargo xtask testdb up`)";

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
    /// What the server calls itself (`ServerInfo.product`).
    pub product: &'static str,
    /// The SQL dialect dbee reads it with, as `dbee_sql::Dialect` names it in JSON.
    pub dialect: &'static str,
    /// The user tests connect as, with [`TestDb::password`]; empty for a file.
    pub user: &'static str,
    /// The database tests connect to; empty for none (MySQL's servers have no default one).
    pub database: &'static str,
    /// Its client inside a server's container; `None` for a file.
    client: Option<Client>,
    /// A query that runs about `secs` seconds, for cancel tests.
    sleep_sql: fn(u32) -> String,
    /// A `WHERE` condition that holds and takes about `secs` seconds once per statement, whatever
    /// the number of rows: a slow page or count.
    slow_condition_sql: fn(u32) -> String,
    /// The one schema of a kind that has only one: every schema of the seed (`shop`, `odd`,
    /// `many`, `scratch`) is this one (SQLite's `main`). `None` keeps the seed's names.
    pub one_schema: Option<&'static str>,
    /// The seed's table of odd names in schema `odd`: quotes, a space, and a character outside
    /// ASCII (MySQL names hold no emoji).
    pub odd_table: &'static str,
    /// A table without a key still takes edits, by the row id the engine gives every row.
    pub keyless_tables_editable: bool,
    /// Two sessions can each hold a write transaction at once (a SQLite file has one writer at a
    /// time).
    pub parallel_writes: bool,
    /// The engine refuses a value longer than its column's length (`varchar(10)`, `char(2)`).
    pub enforces_lengths: bool,
    /// Makes the table `{t}` (first argument) a copy of `source`, keyed by its `id` where the
    /// engine can add a key to a copy (else by its row id).
    copy_table_sql: fn(&str, &str) -> String,
    /// A column definition `<column> date` that refuses text that is no date (the engine's own
    /// date type does; else a CHECK).
    date_column_sql: fn(&str) -> String,
    /// `count` times the one-character string literal given (`repeat('x', 3)`).
    repeat_sql: fn(&str, u64) -> String,
    /// A condition that holds while the condition given does not, and fails its statement with
    /// [`KindInfo::failing_error`] once it does.
    failing_when_sql: fn(&str) -> String,
    /// The type a scratch table gives a text that may be 20 MB long.
    pub long_text_type: &'static str,
    /// The type a scratch table gives a JSON document.
    pub json_type: &'static str,
    /// Lists the ids of dbee's connections, run through the kind's client; `None` for a file,
    /// which has no connection to lose.
    dbee_connections_sql: Option<&'static str>,
    /// Ends the connection with that id.
    kill_sql: fn(&str) -> String,
    /// Makes every UPDATE of `scratch.<table>` take `secs` seconds: the SQL that does it, and what
    /// a [`Scratch`]-style guard drops afterwards (`DROP <that>`).
    slow_updates_sql: fn(&str, u32) -> (String, String),
    /// Counts dbee's statements on `scratch.<table>` that wait for a row lock.
    row_lock_waits_sql: fn(&str) -> String,
    /// How a test makes a COMMIT wait ([`CommitGate`]); `None` where it cannot.
    commit_wait: Option<CommitWait>,
    /// A query a session runs to show whether it is read-only, and what it prints when it is.
    pub read_only_sql: (&'static str, &'static str),
    /// A query a session runs to print its own connection's id (the one `kill_sql` takes).
    pub connection_id_sql: &'static str,
    /// Counts dbee's statements running now whose text holds `marker`.
    running_with_sql: fn(&str) -> String,
    /// Takes the advisory lock named by the argument until the session's transaction ends (on
    /// MySQL, until the session does), as a [`hold`] runs it.
    take_lock_sql: fn(&str) -> String,
    /// A `WHERE` condition that holds once it can take that lock, waiting for it once per
    /// statement whatever the number of rows: a page or count that cannot end while it is held.
    waits_for_lock_sql: fn(&str) -> String,
    /// What the engine shows for this kind (expected values of its tests):
    /// the type label of a column made with [`KindInfo::json_type`], and of `shop.big_cell.j`;
    pub json_type_label: &'static str,
    /// the type label of the seed's plain JSON column, `shop.orders.meta`;
    pub plain_json_label: &'static str,
    /// whether that JSON type keeps an object's keys sorted (else as written);
    pub json_sorts_keys: bool,
    /// the class and the type of an integer literal in a query (`select 3`);
    pub int_literal_class: &'static str,
    pub int_literal_type: &'static str,
    /// the start of the error of [`KindInfo::failing_when_sql`] once its condition holds.
    pub failing_error: &'static str,
}

/// How a [`CommitGate`] holds a COMMIT, from before the update it commits starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitWait {
    /// A deferred trigger on the table takes an advisory lock in the COMMIT, which the gate holds
    /// (Postgres).
    AdvisoryLock,
    /// The gate's `BACKUP STAGE BLOCK_COMMIT` lets updates run and holds every COMMIT (MariaDB).
    BackupStage,
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
            Kind::Postgres => &KindInfo {
                id: "postgres",
                product: "PostgreSQL",
                dialect: "Postgres",
                user: "postgres",
                database: "dbee",
                client: Some(Client::Psql),
                sleep_sql: |secs| format!("SELECT pg_sleep({secs})"),
                slow_condition_sql: |secs| format!("(SELECT true FROM pg_sleep({secs}))"),
                one_schema: None,
                odd_table: "Odd \"Name\" 🐝",
                keyless_tables_editable: false,
                parallel_writes: true,
                enforces_lengths: true,
                copy_table_sql: copy_keyed_by_id,
                date_column_sql: |c| format!("{c} date"),
                repeat_sql: |text, n| format!("repeat({text}, {n})"),
                failing_when_sql: two_rows_when,
                long_text_type: "text",
                json_type: "jsonb",
                // dbee connects with application_name `dbee`; the two-argument terminate (Postgres
                // 14+) waits until the backend has exited.
                dbee_connections_sql: Some("SELECT pid FROM pg_stat_activity WHERE application_name = 'dbee'"),
                kill_sql: |pid| format!("SELECT pg_terminate_backend({pid}, 5000)"),
                slow_updates_sql: |t, secs| {
                    let f = format!("scratch.{t}_slow()");
                    let sql = format!(
                        "CREATE FUNCTION {f} RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep({secs}); RETURN NEW; END $$;
                         CREATE TRIGGER slow BEFORE UPDATE ON scratch.{t} FOR EACH ROW EXECUTE FUNCTION {f};"
                    );
                    (sql, format!("FUNCTION IF EXISTS {f} CASCADE"))
                },
                row_lock_waits_sql: |t| {
                    format!(
                        "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND wait_event_type = 'Lock' AND query LIKE 'UPDATE \"scratch\".\"{t}\"%'"
                    )
                },
                commit_wait: Some(CommitWait::AdvisoryLock),
                read_only_sql: ("SHOW default_transaction_read_only", "on"),
                connection_id_sql: "SELECT pg_backend_pid()",
                running_with_sql: |m| {
                    format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND state = 'active' AND query LIKE '%{m}%'")
                },
                take_lock_sql: |key| format!("SELECT pg_advisory_xact_lock(hashtext('{key}'))"),
                waits_for_lock_sql: |key| format!("(SELECT true FROM pg_advisory_xact_lock_shared(hashtext('{key}')))"),
                json_type_label: "jsonb",
                plain_json_label: "json",
                json_sorts_keys: true,
                int_literal_class: "number",
                int_literal_type: "int4",
                failing_error: "more than one row returned by a subquery used as an expression",
            },
            Kind::MySql => &KindInfo { product: "MySQL", id: "mysql", client: Some(Client::MySql("mysql")), ..MYSQL },
            // MariaDB 10.11 has no `transaction_read_only`.
            Kind::MariaDb => &KindInfo {
                product: "MariaDB",
                id: "mariadb",
                client: Some(Client::MySql("mariadb")),
                read_only_sql: ("SELECT @@SESSION.tx_read_only", "1"),
                commit_wait: Some(CommitWait::BackupStage),
                // Its JSON is a LONGTEXT with a json_valid check, kept as written.
                json_type_label: "longtext",
                plain_json_label: "longtext",
                json_sorts_keys: false,
                int_literal_type: "int",
                ..MYSQL
            },
            // SQLite has no sleep: a count that takes about that long, at least on this machine.
            Kind::Sqlite => &KindInfo {
                id: "sqlite",
                product: "SQLite",
                dialect: "Sqlite",
                user: "",
                database: "",
                client: None,
                sleep_sql: |secs| format!("SELECT (SELECT count(*) FROM ({}))", sqlite_count(secs)),
                slow_condition_sql: |secs| format!("(SELECT count(*) FROM ({})) > 0", sqlite_count(secs)),
                one_schema: Some("main"),
                odd_table: "Odd \"Name\" 🐝",
                keyless_tables_editable: true,
                parallel_writes: false,
                enforces_lengths: false,
                // SQLite adds no key to a table once it is made: the copy is keyed by its rowid.
                copy_table_sql: |t, source| format!("CREATE TABLE {t} AS SELECT * FROM {source}"),
                date_column_sql: |c| format!("{c} date CHECK ({c} IS date({c}))"),
                repeat_sql: |text, n| format!("printf('%.*c', {n}, {text})"),
                failing_when_sql: |cond| format!("json(CASE WHEN {cond} THEN 'not JSON' ELSE '1' END) IS NOT NULL"),
                long_text_type: "TEXT",
                json_type: "JSON",
                dbee_connections_sql: None,
                kill_sql: |_| String::new(),
                // A trigger cannot hold a WITH clause, so the count goes in a view the trigger reads.
                slow_updates_sql: |t, secs| {
                    let sql = format!(
                        "CREATE VIEW main.{t}_slow AS SELECT count(*) AS n FROM ({});
                         CREATE TRIGGER main.{t}_slow_update BEFORE UPDATE ON {t} BEGIN SELECT n FROM {t}_slow; END;",
                        sqlite_count(secs)
                    );
                    (sql, format!("TRIGGER IF EXISTS main.{t}_slow_update; DROP VIEW IF EXISTS main.{t}_slow"))
                },
                row_lock_waits_sql: |_| String::new(),
                commit_wait: None,
                read_only_sql: ("PRAGMA query_only", "1"),
                connection_id_sql: "",
                running_with_sql: |_| String::new(),
                take_lock_sql: |_| String::new(),
                waits_for_lock_sql: |_| String::new(),
                // Measured with its driver (M2.13): a column labels its declared type, and an
                // expression has none.
                json_type_label: "JSON",
                plain_json_label: "JSON",
                json_sorts_keys: false,
                int_literal_class: "other",
                int_literal_type: "",
                failing_error: "malformed JSON",
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

/// What MySQL and MariaDB share.
const MYSQL: KindInfo = KindInfo {
    id: "mysql",
    product: "MySQL",
    dialect: "MySql",
    user: "root",
    database: "",
    client: None,
    sleep_sql: |secs| format!("SELECT SLEEP({secs})"),
    // A sleep in the WHERE clause or a subquery runs once per row; a derived table runs once.
    slow_condition_sql: |secs| format!("EXISTS (SELECT 1 FROM (SELECT SLEEP({secs}) AS z) AS d)"),
    one_schema: None,
    odd_table: "Odd \"Name\" é",
    keyless_tables_editable: false,
    parallel_writes: true,
    enforces_lengths: true,
    copy_table_sql: copy_keyed_by_id,
    date_column_sql: |c| format!("{c} date"),
    repeat_sql: |text, n| format!("repeat({text}, {n})"),
    failing_when_sql: two_rows_when,
    long_text_type: "LONGTEXT",
    json_type: "JSON",
    // The tests' own client connects over the socket (host `localhost`); dbee comes over TCP, so
    // these are dbee's connections while no other tool connects as root.
    dbee_connections_sql: Some("SELECT ID FROM information_schema.PROCESSLIST WHERE USER = 'root' AND HOST <> 'localhost' AND ID <> CONNECTION_ID()"),
    kill_sql: |id| format!("KILL {id}"),
    slow_updates_sql: |t, secs| {
        // While it sleeps, the session's statement is the trigger's, which names the table.
        let sql = format!("CREATE TRIGGER scratch.{t}_slow BEFORE UPDATE ON scratch.{t} FOR EACH ROW SET @dbee_slow_{t} = SLEEP({secs})");
        (sql, format!("TRIGGER IF EXISTS scratch.{t}_slow"))
    },
    row_lock_waits_sql: |t| {
        format!("SELECT COUNT(*) FROM information_schema.INNODB_TRX WHERE trx_state = 'LOCK WAIT' AND trx_query LIKE 'UPDATE `scratch`.`{t}`%'")
    },
    // MySQL has no lock that holds a COMMIT but not the update before it: the global read lock
    // holds both, and taken between the two it loses the race with a COMMIT sent at once.
    commit_wait: None,
    read_only_sql: ("SELECT @@SESSION.transaction_read_only", "1"),
    connection_id_sql: "SELECT CONNECTION_ID()",
    running_with_sql: |m| format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE INFO LIKE '%{m}%' AND ID <> CONNECTION_ID()"),
    take_lock_sql: |key| format!("SELECT GET_LOCK('{key}', 0)"),
    // Taken and given back at once, in a derived table, which runs once.
    waits_for_lock_sql: |key| format!("EXISTS (SELECT 1 FROM (SELECT GET_LOCK('{key}', 60) + RELEASE_LOCK('{key}') AS z) AS d)"),
    json_type_label: "json",
    plain_json_label: "json",
    json_sorts_keys: true,
    int_literal_class: "number",
    int_literal_type: "bigint",
    failing_error: "Subquery returns more than 1 row",
};

/// [`KindInfo::copy_table_sql`] for an engine that adds a key to a copy.
fn copy_keyed_by_id(t: &str, source: &str) -> String {
    format!("CREATE TABLE {t} AS SELECT * FROM {source}; ALTER TABLE {t} ADD PRIMARY KEY (id)")
}

/// [`KindInfo::failing_when_sql`] by a scalar subquery that returns two rows once `cond` holds.
fn two_rows_when(cond: &str) -> String {
    format!("(SELECT 1 UNION ALL SELECT 1 WHERE {cond}) > 0")
}

/// A count SQLite steps through in about `secs` seconds: 2.5 million rows a second, as measured
/// on a busy development machine (2026-11-03).
fn sqlite_count(secs: u32) -> String {
    let rows = u64::from(secs).saturating_mul(2_500_000);
    format!("WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < {rows}) SELECT i FROM c")
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
    pub fn servers_of<'a, 'k>(&'a self, kinds: &'k [Kind]) -> impl Iterator<Item = &'a Server> + use<'a, 'k> {
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

/// Why [`testdb`] has no servers: [`OLD_ENV`] when the env file is JSON but not this tree's shape,
/// else [`NOT_UP`].
pub fn not_up() -> &'static str {
    not_up_at(&env_file())
}

fn not_up_at(path: &Path) -> &'static str {
    let json = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if json.is_some_and(|v| parse(&v).is_none()) { OLD_ENV } else { NOT_UP }
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
                    ::core::panic!("DBEE_REQUIRE_ORACLES=1 and {}", $crate::not_up());
                }
                $crate::skipped(&::std::format!("{}: {}", ::core::module_path!(), $crate::not_up()));
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
    let db = testdb().ok_or_else(|| not_up().to_owned())?;
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

/// Run `sql` on a server with its kind's client, as [`KindInfo::user`] on
/// [`KindInfo::database`]: one line per row, the values separated by `|`, NULL as an empty value
/// (`psql -At`'s shape, on every kind). Returns stdout, or stderr on error.
pub fn sql(server: &Server, sql: &str) -> Result<String, String> {
    match server.kind.info().client {
        Some(Client::Psql) => psql(server, sql),
        Some(Client::MySql(_)) => {
            let out = mysql(server, sql)?;
            let line = |l: &str| l.split('\t').map(|v| if v == "NULL" { "" } else { v }).collect::<Vec<_>>().join("|");
            Ok(out.lines().map(line).map(|l| l + "\n").collect())
        }
        None => Err(format!("{} has no client", server.name)),
    }
}

/// Ends every dbee connection on `server` and waits (up to 10 s) until none is left; `Err` when
/// there was none to end. It ends every dbee connection on the server, also those of tests running
/// beside the caller.
pub fn kill_dbee_connections(server: &Server) -> Result<(), String> {
    let info = server.kind.info();
    let list = info.dbee_connections_sql.ok_or_else(|| format!("{} has no connections to end", server.name))?;
    let ids = |what: &str| -> Result<Vec<String>, String> {
        Ok(sql(server, list).map_err(|e| format!("{what}: {e}"))?.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
    };
    let found = ids("list dbee's connections")?;
    if found.is_empty() {
        return Err(format!("no dbee connection on {}", server.name));
    }
    for id in &found {
        // A connection that ended meanwhile cannot be ended again.
        let _ = sql(server, &(info.kill_sql)(id));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = ids("count dbee's connections")?;
        if left.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("{} dbee connection(s) still on {} after 10 s", left.len(), server.name));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A client session of its own on a test server that ran `sql` in an open transaction and now
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
        let _ = match self.server.kind.info().client {
            Some(Client::Psql) => {
                psql(&self.server, &format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = '{}'", self.marker))
            }
            Some(Client::MySql(_)) => {
                let ids = mysql(&self.server, &holder_ids(&self.marker)).unwrap_or_default();
                let kills: String = ids.lines().map(str::trim).filter(|l| !l.is_empty()).map(|id| format!("KILL {id};")).collect();
                if kills.is_empty() { Ok(String::new()) } else { mysql(&self.server, &kills) }
            }
            None => Ok(String::new()),
        };
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The ids of the MySQL sessions running a statement that holds `marker`, but this one.
fn holder_ids(marker: &str) -> String {
    format!("SELECT ID FROM information_schema.PROCESSLIST WHERE INFO LIKE '%{marker}%' AND ID <> CONNECTION_ID()")
}

/// Runs `sql` in a transaction of a client session of its own on a server, in the background, and
/// returns once the session sleeps with the transaction open (see [`Held`]). `sql` must not end
/// the transaction.
pub fn hold(server: &Server, sql: &str) -> Result<Held, String> {
    let db = testdb().ok_or_else(|| not_up().to_owned())?;
    let marker = unique("dbee_test_hold");
    let mut c = compose();
    let sleeping = match server.kind.info().client {
        Some(Client::Psql) => {
            let script = format!("SET application_name = '{marker}'; BEGIN; {sql}; SELECT pg_sleep(60); ROLLBACK;");
            c.args(["exec", "-T", &server.name, "psql", "-U", "postgres", "-d", "dbee", "-At", "-v", "ON_ERROR_STOP=1", "-c", &script]);
            format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = '{marker}' AND wait_event = 'PgSleep'")
        }
        Some(Client::MySql(client)) => {
            c.env("MYSQL_PWD", &db.password);
            c.args(["exec", "-T", "-e", "MYSQL_PWD", &server.name, client, "--user=root", "--batch"]);
            c.arg(format!("--execute=START TRANSACTION; {sql}; SELECT SLEEP(60), '{marker}'; ROLLBACK;"));
            format!(
                "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE INFO LIKE '%{marker}%' AND STATE = 'User sleep' AND ID <> CONNECTION_ID()"
            )
        }
        None => return Err(format!("{} has no client", server.name)),
    };
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let child = c.spawn().map_err(|e| format!("docker compose exec {}: {e}", server.name))?;
    let held = Held { server: server.clone(), marker, child };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while crate::sql(server, &sleeping)?.trim() != "1" {
        if std::time::Instant::now() > deadline {
            return Err(format!("the client session on {} did not get to its sleep within 20 s", server.name));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(held)
}

/// Polls `sql` on `server` until it prints `want` (up to 20 s).
fn until(server: &Server, sql: &str, want: &str, what: &str) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while crate::sql(server, sql)?.trim() != want {
        if std::time::Instant::now() > deadline {
            return Err(format!("{}: gave up waiting for {what}", server.name));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Makes the COMMIT of a transaction that updated `scratch.<table>` wait, until the gate is
/// dropped ([`TestTarget::commit_gate`]).
#[derive(Debug)]
pub struct CommitGate {
    server: Server,
    table: String,
    wait: CommitWait,
    held: Option<Held>,
    /// What to drop when the gate goes (`DROP <that>`).
    cleanup: String,
}

impl CommitGate {
    /// Waits (up to 20 s) until a dbee COMMIT on the table waits at the gate.
    pub fn until_committing(&mut self) -> Result<(), String> {
        let t = &self.table;
        let committing = match self.wait {
            CommitWait::AdvisoryLock => format!(
                "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND query = 'COMMIT' AND wait_event = 'advisory'
                   AND pid IN (SELECT pid FROM pg_locks WHERE relation = 'scratch.{t}'::regclass)"
            ),
            CommitWait::BackupStage => {
                "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE INFO = 'COMMIT' AND STATE = 'Waiting for backup lock'".to_owned()
            }
        };
        until(&self.server, &committing, "1", "the COMMIT to wait at the gate")
    }
}

impl Drop for CommitGate {
    fn drop(&mut self) {
        self.held = None;
        if !self.cleanup.is_empty() {
            let _ = crate::sql(&self.server, &format!("DROP {}", self.cleanup));
        }
    }
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

    /// A query that runs about `secs` seconds, for cancel tests (on SQLite a count, whose time
    /// depends on the machine).
    pub fn sleep_sql(&self, secs: u32) -> String {
        (self.kind.info().sleep_sql)(secs)
    }

    /// A `WHERE` condition that holds and takes about `secs` seconds once per statement (on
    /// SQLite a count, as for [`TestTarget::sleep_sql`]).
    pub fn slow_condition_sql(&self, secs: u32) -> String {
        (self.kind.info().slow_condition_sql)(secs)
    }

    /// The rows of `big_rows` in this target's seed.
    pub fn big_rows(&self) -> u64 {
        self.server().map_or(BIG_ROWS, Server::big_rows)
    }

    /// The target's seed is the full one, with the 20 MB row of `big_cell` ([`Server::full_seed`];
    /// a file always is).
    pub fn full_seed(&self) -> bool {
        self.server().is_none_or(|s| s.full_seed)
    }

    /// SQL that makes every UPDATE of `scratch.<table>` take `secs` seconds, and what to drop
    /// afterwards (`DROP <that>`).
    pub fn slow_updates_sql(&self, table: &str, secs: u32) -> (String, String) {
        (self.kind.info().slow_updates_sql)(table, secs)
    }

    /// A query that counts dbee's statements running now whose text holds `marker`.
    pub fn running_with_sql(&self, marker: &str) -> String {
        (self.kind.info().running_with_sql)(marker)
    }

    /// A session of its own ([`hold`]) that holds the advisory lock `key` until dropped, so a
    /// statement with [`TestTarget::waits_for_lock_sql`] cannot end meanwhile unless it is
    /// cancelled. Not on a file, which has no other session.
    pub fn hold_lock(&self, key: &str) -> Result<Held, String> {
        let server = self.server().ok_or_else(|| format!("{}: a file has no other session to hold a lock", self.name()))?;
        hold(server, &(self.kind.info().take_lock_sql)(key))
    }

    /// A `WHERE` condition that holds, and waits once per statement while [`TestTarget::hold_lock`]
    /// holds the lock `key`.
    pub fn waits_for_lock_sql(&self, key: &str) -> String {
        (self.kind.info().waits_for_lock_sql)(key)
    }

    /// Ends the server's connection `id` (from [`KindInfo::connection_id_sql`]) and waits (up to
    /// 10 s) until it is gone.
    pub fn kill_connection(&self, id: &str) -> Result<(), String> {
        let server = self.server().ok_or_else(|| format!("{}: no server", self.name()))?;
        let info = self.kind.info();
        let list = info.dbee_connections_sql.ok_or_else(|| format!("{}: no connections", self.name()))?;
        crate::sql(server, &(info.kill_sql)(id))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while crate::sql(server, list)?.lines().any(|l| l.trim() == id) {
            if std::time::Instant::now() > deadline {
                return Err(format!("{}: connection {id} still there after 10 s", self.name()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    /// A query that counts dbee's statements on `scratch.<table>` waiting for a row lock.
    pub fn row_lock_waits_sql(&self, table: &str) -> String {
        (self.kind.info().row_lock_waits_sql)(table)
    }

    /// A [`CommitGate`] on `scratch.<table>`, set up before the session that commits loads the
    /// table (it may add a trigger) and starts its update. On MariaDB it holds every COMMIT and DDL
    /// on the server: take it only while no other test's session runs.
    pub fn commit_gate(&self, table: &str) -> Result<CommitGate, String> {
        let server = self.server().ok_or_else(|| format!("{}: no server", self.name()))?;
        let wait = self.kind.info().commit_wait.ok_or_else(|| format!("{}: no way to hold a COMMIT", self.name()))?;
        let mut gate = CommitGate { server: server.clone(), table: table.to_owned(), wait, held: None, cleanup: String::new() };
        match wait {
            CommitWait::AdvisoryLock => {
                let f = format!("scratch.{table}_commit_gate()");
                crate::sql(
                    server,
                    &format!(
                        "CREATE FUNCTION {f} RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('{table}')); RETURN NULL; END $$;
                         CREATE CONSTRAINT TRIGGER commit_gate AFTER UPDATE ON scratch.{table} DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {f};"
                    ),
                )?;
                gate.cleanup = format!("FUNCTION IF EXISTS {f} CASCADE");
                gate.held = Some(hold(server, &format!("SELECT pg_advisory_lock(hashtext('{table}'))"))?);
            }
            CommitWait::BackupStage => {
                gate.held = Some(hold(server, "BACKUP STAGE START; BACKUP STAGE FLUSH; BACKUP STAGE BLOCK_DDL; BACKUP STAGE BLOCK_COMMIT")?);
            }
        }
        Ok(gate)
    }

    /// The target can hold a COMMIT with a [`CommitGate`].
    pub fn can_hold_commit(&self) -> bool {
        self.server().is_some() && self.kind.info().commit_wait.is_some()
    }

    /// Runs `sql` on the target as [`sql`] does: on a server with its kind's client, on a file
    /// with a connection of the test's own.
    pub fn sql(&self, sql: &str) -> Result<String, String> {
        match &self.place {
            Place::Server(server) => crate::sql(server, sql),
            Place::File(path) => sqlite_sql(path, sql),
        }
    }

    /// Where the seed's schema `seed` (`shop`, `odd`, `many`, `scratch`) is on this target: that
    /// schema, or the kind's one schema ([`KindInfo::one_schema`]).
    pub fn schema(&self, seed: &'static str) -> &'static str {
        self.kind.info().one_schema.unwrap_or(seed)
    }

    /// The server lists the statements that run on it, through its client; a file has neither.
    pub fn lists_running_statements(&self) -> bool {
        self.server().is_some()
    }

    /// SQL that makes `t` a copy of `source`, keyed by its `id` ([`KindInfo::copy_table_sql`]).
    pub fn copy_table_sql(&self, t: &str, source: &str) -> String {
        (self.kind.info().copy_table_sql)(t, source)
    }

    /// A date column named `column` that refuses text that is no date.
    pub fn date_column_sql(&self, column: &str) -> String {
        (self.kind.info().date_column_sql)(column)
    }

    /// `count` times the one-character string literal `text`.
    pub fn repeat_sql(&self, text: &str, count: u64) -> String {
        (self.kind.info().repeat_sql)(text, count)
    }

    /// A condition that holds while `cond` does not, and fails its statement with
    /// [`KindInfo::failing_error`] once `cond` holds.
    pub fn failing_when_sql(&self, cond: &str) -> String {
        (self.kind.info().failing_when_sql)(cond)
    }
}

/// Runs `sql` on the SQLite file at `path` with a connection of its own, every statement in turn,
/// in [`sql`]'s shape: one line per row, the values separated by `|`, NULL as an empty value, reals
/// as SQLite prints them and blobs as `X'…'`. Waits up to 5 s for a file dbee has locked.
fn sqlite_sql(path: &Path, sql: &str) -> Result<String, String> {
    use rusqlite::fallible_iterator::FallibleIterator as _;
    use rusqlite::types::ValueRef;
    let fail = |e: rusqlite::Error| format!("{}: {e}", path.display());
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(fail)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(fail)?;
    let mut out = String::new();
    let mut batch = rusqlite::Batch::new(&conn, sql);
    while let Some(mut statement) = batch.next().map_err(fail)? {
        let width = statement.column_count();
        let mut rows = statement.raw_query();
        while let Some(row) = rows.next().map_err(fail)? {
            let mut values = Vec::with_capacity(width);
            for i in 0..width {
                values.push(match row.get_ref(i).map_err(fail)? {
                    ValueRef::Null => String::new(),
                    ValueRef::Integer(n) => n.to_string(),
                    ValueRef::Real(x) => conn.query_row("SELECT CAST(?1 AS TEXT)", [x], |r| r.get(0)).map_err(fail)?,
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    ValueRef::Blob(b) => format!("X'{}'", b.iter().map(|byte| format!("{byte:02X}")).collect::<String>()),
                });
            }
            out.push_str(&values.join("|"));
            out.push('\n');
        }
    }
    Ok(out)
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
    targets_of(&Kind::ALL)
}

/// [`targets`] of these kinds only; the SQLite copy is made only when they hold [`Kind::Sqlite`].
pub fn targets_of(kinds: &[Kind]) -> Vec<TestTarget> {
    let mut all = Vec::new();
    if let Some(db) = testdb() {
        all.extend(db.servers_of(kinds).map(target));
    }
    if kinds.contains(&Kind::Sqlite) {
        all.push(TestTarget { kind: Kind::Sqlite, place: Place::File(sqlite_seeded(&testkit_dir().join("copies"))) });
    }
    all
}

/// The target of one test server.
pub fn target(server: &'static Server) -> TestTarget {
    TestTarget { kind: server.kind, place: Place::Server(server) }
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

        std::fs::write(&path, &text).unwrap();
        assert_eq!(not_up_at(&path), NOT_UP, "this tree's shape");
        // The shape an older tree wrote (one `mysql` list beside the Postgres `servers`).
        std::fs::write(&path, r#"{"password":"pw","servers":[{"name":"pg14","major":14,"port":1}],"mysql":[]}"#).unwrap();
        assert!(load_from(&path).is_none());
        assert_eq!(not_up_at(&path), OLD_ENV);
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_none());
        assert_eq!(not_up_at(&path), NOT_UP);
        assert_eq!(not_up_at(&dir.home.join("missing.json")), NOT_UP);
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

//! Connection tests. Most run against every MySQL and MariaDB server in `db.servers` (`cargo xtask
//! testdb up`); the rest talk to a fake server on 127.0.0.1 that this file plays, byte by byte as
//! MySQL's protocol documentation describes the packets.
//!
//! Safety: tunnels are opened with an `SshEnv` over a throwaway home from `temp_home` and no
//! agent, so the real `~/.ssh` and SSH agent are never touched. Writes go to the `scratch`
//! database under `unique` names; the login tests create throwaway users with `unique` names
//! and drop them.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use dbee_config::{ALLOW_PUBLIC_KEY, ConnectionConfig, DbKind, SshAuth, SshConfig, SslMode};
use dbee_core::TableRef;
use dbee_driver::{ApplyGate, Cancel, Connection, DbError, Driver, Purpose, QUERY_LIMITS, Target, TlsState};
use dbee_mysql::wire::{MAX_PAYLOAD, Packets};
use dbee_mysql::{Canceller, Flavor, MySqlConnection, MySqlDriver, connect, version_num, version_warning};
use dbee_sql::Dialect;
use dbee_ssh::{AgentSource, SshEnv, SshSecrets, Tunnel};
use dbee_testkit::{Kind, Server, TempHome, TestDb, require_testdb, temp_home, unique};
use mysql_common::auth::plugins::mysql_native_password::scramble_native;
use mysql_common::constants::{CapabilityFlags, StatusFlags};
use proptest::prelude::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;

/// The MySQL and MariaDB test servers.
fn mysql_servers(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::MySql, Kind::MariaDb])
}

fn kind(s: &Server) -> DbKind {
    DbKind::from_id(s.kind.id()).unwrap()
}

/// `root@127.0.0.1:<port>`, no database, with the given TLS mode.
fn config(s: &Server, mode: SslMode) -> ConnectionConfig {
    ConnectionConfig {
        name: format!("test {}", s.name),
        kind: kind(s),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "root".into(),
        ssl_mode: mode,
        ..ConnectionConfig::default()
    }
}

fn target<'a>(c: &'a ConnectionConfig, password: Option<&'a str>) -> Target<'a> {
    Target { config: c, password, tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None }
}

async fn open_as(c: &ConnectionConfig, password: &str, purpose: Purpose) -> MySqlConnection {
    match connect(Target { purpose, ..target(c, Some(password)) }).await {
        Ok(conn) => conn,
        Err(e) => panic!("connect to {}:{} ({:?}, {}): {e} [{}]", c.host, c.port, c.ssl_mode, c.user, e.code()),
    }
}

async fn open(c: &ConnectionConfig, db: &TestDb) -> MySqlConnection {
    open_as(c, &db.password, Purpose::Browse).await
}

/// The cell `col` of the first row.
async fn cell(conn: &MySqlConnection, sql: &str, col: usize) -> Option<String> {
    let r = conn.run(sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    r.rows.first().and_then(|row| row.get(col)).cloned().flatten()
}

async fn scalar(conn: &MySqlConnection, sql: &str) -> Option<String> {
    cell(conn, sql, 0).await
}

/// The session's TLS cipher; empty without TLS.
async fn cipher(conn: &MySqlConnection) -> String {
    cell(conn, "SHOW SESSION STATUS LIKE 'Ssl_cipher'", 1).await.unwrap_or_default()
}

fn code(r: Result<MySqlConnection, DbError>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

/// MySQL 5.7's image offers TLS only with ciphers rustls does not speak (DHE-RSA on TLS 1.2).
fn rustls_can_reach(s: &Server) -> bool {
    s.name != "mysql57"
}

/// What a TLS failure against mysql57 must say.
fn assert_cipher_error(e: &DbError, s: &Server) {
    assert_eq!(e.code(), "tls", "{}: {e:?}", s.name);
    let text = e.to_string();
    assert!(text.contains("cipher") && text.contains("SSH"), "{}: {text}", s.name);
}

/// The server's version as `version_num` counts it: `8.0.46` is 80046, `10.11.19` is 101119.
fn expected_num(version: &str) -> u32 {
    let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap()).collect();
    parts[0] * 10_000 + parts[1] * 100 + parts[2]
}

/// The CA that did not sign the servers' certificate: the server certificate itself.
fn wrong_ca(db: &TestDb) -> std::path::PathBuf {
    db.ca_file.with_file_name("server.crt")
}

fn ssh_config(db: &TestDb) -> SshConfig {
    SshConfig {
        host: "127.0.0.1".into(),
        port: db.sshd.port,
        user: db.sshd.user.clone(),
        auth: SshAuth::Key { path: Some(db.keys.plain.clone()) },
        ..SshConfig::default()
    }
}

/// A tunnel to the test sshd, logging in with the plain key. `home` holds `known_hosts`.
async fn tunnel(db: &TestDb, home: &TempHome) -> Arc<Tunnel> {
    let env = SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None };
    Arc::new(Tunnel::open(&ssh_config(db), &env, &SshSecrets::default(), None).await.unwrap())
}

/// The server as the sshd sees it: `<service>:3306`.
fn tunnelled(s: &Server, db: &TestDb, mode: SslMode) -> ConnectionConfig {
    ConnectionConfig { host: s.name.clone(), port: s.service_port, ssh: Some(ssh_config(db)), ..config(s, mode) }
}

async fn open_tunnelled(c: &ConnectionConfig, db: &TestDb, t: &Arc<Tunnel>) -> MySqlConnection {
    let target = Target { tunnel: Some(Arc::clone(t)), ..target(c, Some(&db.password)) };
    connect(target).await.unwrap_or_else(|e| panic!("{} through SSH: {e}", c.host))
}

#[tokio::test]
async fn connects_and_reports_version() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(&config(s, SslMode::Prefer), db).await;
        let info = c.info();
        let product = if s.kind == Kind::MariaDb { "MariaDB" } else { "MySQL" };
        assert_eq!(info.product, product, "{}", s.name);
        assert!(info.version.starts_with(&s.version), "{}: {info:?}", s.name);
        assert_eq!(info.version_num, expected_num(&s.version), "{}: {info:?}", s.name);
        assert!(!info.read_only, "{}: {info:?}", s.name);
        // prefer ends up plain on mysql57, whose TLS rustls cannot speak: the connection says so.
        let plain = !rustls_can_reach(s);
        assert_eq!(info.warnings.len(), usize::from(plain), "{}: {info:?}", s.name);
        assert!(info.warnings.iter().all(|w| w.contains("not encrypted")), "{}: {info:?}", s.name);
        assert_eq!(info.tls, Some(if plain { TlsState::Off } else { TlsState::Unchecked }), "{}", s.name);
        assert!(!c.is_closed());
        let printed = scalar(&c, "SELECT @@version").await.unwrap_or_default();
        assert!(printed.starts_with(&info.version) && info.version.chars().all(|c| c.is_ascii_digit() || c == '.'), "{printed} {info:?}");
        assert_eq!(scalar(&c, "SELECT DATABASE()").await, None);
        assert_eq!(scalar(&c, "SELECT 1 + 1").await.as_deref(), Some("2"));

        // A database set in the settings is the session's database.
        let shop = ConnectionConfig { database: "shop".into(), ..config(s, SslMode::Prefer) };
        assert_eq!(scalar(&open(&shop, db).await, "SELECT DATABASE()").await.as_deref(), Some("shop"));
        // The product comes from the server, not from the connection's type.
        let other = if kind(s) == DbKind::MySql { DbKind::MariaDb } else { DbKind::MySql };
        let swapped = ConnectionConfig { kind: other, ..config(s, SslMode::Prefer) };
        assert_eq!(open(&swapped, db).await.info().product, product, "{}", s.name);
    }
}

#[tokio::test]
async fn mariadb_version_num_is_the_real_version() {
    let db = require_testdb!();
    let mut seen = 0;
    for s in mysql_servers(db).filter(|s| s.kind == Kind::MariaDb) {
        let info = open(&config(s, SslMode::Prefer), db).await.info().clone();
        // The numbers alone: no `5.5.5-` before them, no `-MariaDB-…` after.
        assert!(info.product == "MariaDB" && info.version.chars().all(|c| c.is_ascii_digit() || c == '.'), "{info:?}");
        assert!(info.version_num >= 101100, "{info:?}");
        assert_eq!(info.version_num, expected_num(&s.version));
        seen += 1;
    }
    assert_eq!(seen, 2);
    assert_eq!(version_num("10.11.19-MariaDB-ubu2204"), Some(101119));
    assert_eq!(version_num("13.0.2-MariaDB-ubu2604"), Some(130002));
    assert_eq!(version_num("8.0.46"), Some(80046));
    assert_eq!(version_num("26.10.0"), Some(261000));
    assert_eq!(version_num("5.7.44-log"), Some(50744));
    for bad in ["", "MariaDB", "8", "8.0", "x.y.z", "99999999999.0.0"] {
        assert_eq!(version_num(bad), None, "{bad:?}");
    }
    assert_eq!(Flavor::from_version("10.11.19-MariaDB-ubu2204"), Flavor::MariaDb);
    assert_eq!(Flavor::from_version("8.0.46"), Flavor::MySql);
}

#[test]
fn old_versions_warn() {
    let mysql = version_warning(Flavor::MySql, 50651).unwrap();
    assert!(mysql.contains("MySQL 5.6") && mysql.contains("5.7"), "{mysql}");
    assert_eq!(version_warning(Flavor::MySql, 50700), None);
    assert_eq!(version_warning(Flavor::MySql, 261000), None);
    let maria = version_warning(Flavor::MariaDb, 100611).unwrap();
    assert!(maria.contains("MariaDB 10.6") && maria.contains("10.11"), "{maria}");
    assert_eq!(version_warning(Flavor::MariaDb, 101100), None);
    assert_eq!(version_warning(Flavor::MariaDb, 130002), None);
}

#[tokio::test]
async fn wrong_password_is_auth_error() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = config(s, SslMode::Prefer);
        let e = connect(target(&c, Some("nope-pw"))).await.err().unwrap();
        assert_eq!(e.code(), "auth", "{}: {e:?}", s.name);
        assert!(matches!(&e, DbError::Auth { message } if !message.is_empty()), "{e:?}");
        assert!(!e.to_string().contains("nope-pw"), "{e}");
        // No password at all, when the account has one, is an auth error too.
        assert_eq!(code(connect(target(&c, None)).await), "auth", "{}", s.name);
    }
}

#[tokio::test]
async fn tls_require_encrypts() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let plain = open(&config(s, SslMode::Disable), db).await;
        assert_eq!(cipher(&plain).await, "", "{}", s.name);
        assert_eq!((plain.info().tls, plain.info().warnings.as_slice()), (Some(TlsState::Off), &[][..]), "{}", s.name);
        let prefer = open(&config(s, SslMode::Prefer), db).await;
        if rustls_can_reach(s) {
            assert!(!cipher(&prefer).await.is_empty(), "{}", s.name);
            assert_eq!(prefer.info().tls, Some(TlsState::Unchecked), "{}", s.name);
            let require = open(&config(s, SslMode::Require), db).await;
            assert!(!cipher(&require).await.is_empty(), "{}", s.name);
            assert_eq!(require.info().tls, Some(TlsState::Unchecked), "{}", s.name);
            // With a CA file, require checks the chain (libpq's rule), not the host name.
            let ca = ConnectionConfig { ca_file: Some(db.ca_file.clone()), ..config(s, SslMode::Require) };
            let checked = open(&ca, db).await;
            assert!(!cipher(&checked).await.is_empty(), "{}", s.name);
            assert_eq!(checked.info().tls, Some(TlsState::Checked), "{}", s.name);
            let wrong = ConnectionConfig { ca_file: Some(wrong_ca(db)), ..ca.clone() };
            assert_eq!(code(connect(target(&wrong, Some(&db.password))).await), "tls", "{}", s.name);
        } else {
            // prefer falls back to plain when the TLS handshake fails, and says so; require says why
            // it failed.
            assert_eq!(cipher(&prefer).await, "", "{}", s.name);
            assert_eq!(prefer.info().tls, Some(TlsState::Off), "{}", s.name);
            assert!(prefer.info().warnings.iter().any(|w| w.contains("not encrypted")), "{}: {:?}", s.name, prefer.info());
            let e = connect(target(&config(s, SslMode::Require), Some(&db.password))).await.err().unwrap();
            assert_cipher_error(&e, s);
        }
    }
}

#[tokio::test]
async fn tls_verify_full_needs_trusted_ca() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let local = ConnectionConfig { host: "localhost".into(), ..config(s, SslMode::VerifyFull) };
        let with_ca = ConnectionConfig { ca_file: Some(db.ca_file.clone()), ..local.clone() };
        if !rustls_can_reach(s) {
            assert_cipher_error(&connect(target(&with_ca, Some(&db.password))).await.err().unwrap(), s);
            continue;
        }
        let full = open(&with_ca, db).await;
        assert!(!cipher(&full).await.is_empty(), "{}", s.name);
        assert_eq!(full.info().tls, Some(TlsState::Checked), "{}", s.name);
        // The certificate names 127.0.0.1 too.
        open(&ConnectionConfig { host: "127.0.0.1".into(), ..with_ca.clone() }, db).await;
        // The OS does not trust the test CA.
        let e = connect(target(&local, Some(&db.password))).await.err().unwrap();
        assert!(matches!(&e, DbError::Tls(m) if !m.is_empty()), "{}: {e:?}", s.name);
        assert_eq!(code(connect(target(&ConnectionConfig { ca_file: Some(wrong_ca(db)), ..local.clone() }, Some(&db.password))).await), "tls");
        // verify-ca checks the chain only, and needs a CA file.
        open(&ConnectionConfig { ssl_mode: SslMode::VerifyCa, ..with_ca.clone() }, db).await;
        let e = connect(target(&ConnectionConfig { ssl_mode: SslMode::VerifyCa, ..local.clone() }, Some(&db.password))).await.err().unwrap();
        assert!(e.to_string().contains("verify-ca needs a CA file"), "{e}");
    }
}

#[tokio::test]
async fn through_ssh_tunnel() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    for s in mysql_servers(db) {
        let c = tunnelled(s, db, SslMode::Prefer);
        let conn = open_tunnelled(&c, db, &t).await;
        assert!(conn.info().version.starts_with(&s.version), "{}: {:?}", s.name, conn.info());
        // The tunnel encrypts the link as far as the SSH server: a plain link past it gets no warning.
        let tls = if rustls_can_reach(s) { TlsState::Unchecked } else { TlsState::Off };
        assert_eq!((conn.info().tls, conn.info().warnings.as_slice()), (Some(tls), &[][..]), "{}", s.name);
        assert_eq!(scalar(&conn, "SELECT 40 + 2").await.as_deref(), Some("42"));
        if rustls_can_reach(s) {
            // TLS through the tunnel checks the configured host name (the service name).
            let full = ConnectionConfig { ssl_mode: SslMode::VerifyFull, ca_file: Some(db.ca_file.clone()), ..c.clone() };
            assert!(!cipher(&open_tunnelled(&full, db, &t).await).await.is_empty(), "{}", s.name);
        }
        // An SSH connection without a tunnel is refused rather than tried directly.
        assert_eq!(code(connect(target(&c, Some(&db.password))).await), "connect");
    }
    t.close().await;
}

/// The variable a server's flavor reads for read-only mode.
fn read_only_variable(s: &Server) -> &'static str {
    match s.kind {
        Kind::MariaDb => "@@session.tx_read_only",
        _ => "@@session.transaction_read_only",
    }
}

#[tokio::test]
async fn read_only_refuses_writes() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let ro = ConnectionConfig { read_only: true, ..config(s, SslMode::Prefer) };
        let c = open(&ro, db).await;
        assert!(c.info().read_only, "{}", s.name);
        assert_eq!(scalar(&c, &format!("SELECT {}", read_only_variable(s))).await.as_deref(), Some("1"), "{}", s.name);
        let name = unique("t_ro");
        let e = c.run(&format!("CREATE TABLE scratch.{name} (a int)"), QUERY_LIMITS).await.unwrap_err();
        // Safety net: if the table was created after all, drop it.
        let rw = open(&config(s, SslMode::Prefer), db).await;
        rw.run(&format!("DROP TABLE IF EXISTS scratch.{name}"), QUERY_LIMITS).await.unwrap();
        assert_eq!(e.code(), "readOnly", "{}: {e:?}", s.name);
        assert!(matches!(&e, DbError::ReadOnly { message } if message.contains("READ ONLY")), "{}: {e:?}", s.name);
        // Reading still works, on the same connection.
        assert_eq!(scalar(&c, "SELECT 1 + 1").await.as_deref(), Some("2"));
        assert!(!rw.info().read_only);
        assert_eq!(scalar(&rw, &format!("SELECT {}", read_only_variable(s))).await.as_deref(), Some("0"));
    }
}

#[tokio::test]
async fn browse_session_is_strict() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let browse = open_as(&config(s, SslMode::Prefer), &db.password, Purpose::Browse).await;
        let mode = scalar(&browse, "SELECT @@SESSION.sql_mode").await.unwrap_or_default();
        assert!(mode.split(',').any(|m| m == "STRICT_ALL_TABLES"), "{}: {mode}", s.name);
        assert!(mode.split(',').any(|m| m == "NO_BACKSLASH_ESCAPES"), "{}: {mode}", s.name);
        assert_eq!(scalar(&browse, "SELECT 'a\\b'").await.as_deref(), Some("a\\b"));

        assert_eq!(scalar(&browse, "SELECT @@SESSION.autocommit").await.as_deref(), Some("1"), "{}", s.name);

        let query = open_as(&config(s, SslMode::Prefer), &db.password, Purpose::Query).await;
        let session = scalar(&query, "SELECT @@SESSION.sql_mode").await;
        assert_eq!(session, scalar(&query, "SELECT @@GLOBAL.sql_mode").await, "{}", s.name);
        let autocommit = scalar(&query, "SELECT @@SESSION.autocommit").await;
        assert_eq!(autocommit, scalar(&query, "SELECT @@GLOBAL.autocommit").await, "{}", s.name);
        for conn in [&browse, &query] {
            assert_eq!(scalar(conn, "SELECT @@character_set_client").await.as_deref(), Some("utf8mb4"));
            assert_eq!(scalar(conn, "SELECT @@character_set_results").await.as_deref(), Some("utf8mb4"));
        }
        // The browse setting works on a server whose mode is empty.
        query.run("SET SESSION sql_mode = ''", QUERY_LIMITS).await.unwrap();
        let set = "SET SESSION sql_mode = TRIM(BOTH ',' FROM CONCAT(@@SESSION.sql_mode, ',STRICT_ALL_TABLES,NO_BACKSLASH_ESCAPES'))";
        query.run(set, QUERY_LIMITS).await.unwrap();
        let mode = scalar(&query, "SELECT @@SESSION.sql_mode").await.unwrap_or_default();
        let mut modes: Vec<&str> = mode.split(',').collect();
        modes.sort_unstable();
        assert_eq!(modes, ["NO_BACKSLASH_ESCAPES", "STRICT_ALL_TABLES"], "{}", s.name);
    }
}

/// Waits until connection `id` sleeps in `SLEEP()`, as `watcher`, a connection to the same server,
/// sees in the processlist: the statement runs.
async fn until_sleeping(watcher: &MySqlConnection, id: u64) {
    let sql = format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id} AND STATE = 'User sleep'");
    let deadline = Instant::now() + Duration::from_secs(20);
    while watcher.scalar_u64(&sql).await != Ok(1) {
        assert!(Instant::now() < deadline, "connection {id} never got to its sleep");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Starts `SELECT SLEEP(30)`, cancels it through the `Cancel` trait once `watcher` sees it sleep,
/// and expects `cancelled`; then the connection still works.
async fn cancel_sleep(conn: Arc<MySqlConnection>, watcher: &MySqlConnection) {
    let canceller: Arc<dyn Cancel> = Arc::new(conn.canceller());
    let id = conn.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
    let runner = Arc::clone(&conn);
    let job = tokio::spawn(async move { runner.run("SELECT SLEEP(30)", QUERY_LIMITS).await });
    until_sleeping(watcher, id).await;
    canceller.cancel().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not cancelled").unwrap();
    assert_eq!(result.err(), Some(DbError::Cancelled));
    assert!(!conn.is_closed());
    assert_eq!(scalar(&conn, "SELECT 'still here'").await.as_deref(), Some("still here"));
}

#[tokio::test]
async fn cancel_stops_sleep() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let watcher = open(&config(s, SslMode::Prefer), db).await;
        for mode in [SslMode::Prefer, SslMode::Disable] {
            cancel_sleep(Arc::new(open(&config(s, mode), db).await), &watcher).await;
        }
    }
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    for s in mysql_servers(db) {
        let watcher = open(&config(s, SslMode::Prefer), db).await;
        cancel_sleep(Arc::new(open_tunnelled(&tunnelled(s, db, SslMode::Prefer), db, &t).await), &watcher).await;
    }
    t.close().await;
}

#[tokio::test]
async fn late_kill_never_cancels_the_next_statement() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn = Arc::new(open(&config(s, SslMode::Prefer), db).await);
        let canceller: Canceller = conn.canceller();
        // A cancel with nothing running reaches nothing.
        assert_eq!(scalar(&conn, "SELECT 1").await.as_deref(), Some("1"));
        canceller.cancel().await.unwrap();
        assert_eq!(scalar(&conn, "SELECT SLEEP(0.5)").await.as_deref(), Some("0"), "{}", s.name);
        // The statement after a cancelled one runs to its end.
        cancel_sleep(Arc::clone(&conn), &open(&config(s, SslMode::Prefer), db).await).await;
        assert_eq!(scalar(&conn, "SELECT SLEEP(0.5)").await.as_deref(), Some("0"), "{}", s.name);
        // A KILL QUERY the server gets while the connection is idle does not carry over.
        let id = scalar(&conn, "SELECT CONNECTION_ID()").await.unwrap();
        open(&config(s, SslMode::Prefer), db).await.run(&format!("KILL QUERY {id}"), QUERY_LIMITS).await.unwrap();
        assert_eq!(scalar(&conn, "SELECT SLEEP(0.5)").await.as_deref(), Some("0"), "{}", s.name);
    }
}

#[tokio::test]
async fn an_abandoned_query_closes_the_connection() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn = open(&config(s, SslMode::Prefer), db).await;
        // Its answer would still be on the wire for the next statement to read.
        assert!(tokio::time::timeout(Duration::from_millis(200), conn.run("SELECT SLEEP(5)", QUERY_LIMITS)).await.is_err());
        assert!(conn.is_closed(), "{}", s.name);
        assert_eq!(conn.run("SELECT 1", QUERY_LIMITS).await.unwrap_err(), DbError::ConnectionLost);
    }
}

#[tokio::test]
async fn the_servers_key_only_without_encryption_or_when_allowed() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    let mut seen = 0;
    for s in mysql_servers(db).filter(|s| !rustls_can_reach(s)) {
        // prefer ends up plain on this server, and sha256_password then wants the server's key.
        let root = open(&config(s, SslMode::Prefer), db).await;
        let user = User::create(s, &root, "sha256_password").await;
        let direct = ConnectionConfig { user: user.name.clone(), ..config(s, SslMode::Prefer) };
        // Its TLS is on but unusable to rustls, so the text does not say to turn it on.
        let e = connect(target(&direct, Some(&user.password))).await.err().unwrap();
        let unusable = |m: &str| m.contains("TLS could not be used") && m.contains("allow the server's public key") && !m.contains("turn on TLS");
        assert!(matches!(&e, DbError::Auth { message } if unusable(message)), "{}: {e:?}", s.name);
        let mut allowed = direct.clone();
        allowed.options.insert(ALLOW_PUBLIC_KEY.id.into(), "true".into());
        let warnings = open_as(&allowed, &user.password, Purpose::Query).await.info().warnings.clone();
        assert!(warnings.len() == 2 && warnings[0].contains("not encrypted") && warnings[1].contains("could not check"), "{}: {warnings:?}", s.name);
        // No encryption chosen, or the tunnel's: no warning.
        let plain = ConnectionConfig { ssl_mode: SslMode::Disable, ..direct.clone() };
        assert!(open_as(&plain, &user.password, Purpose::Query).await.info().warnings.is_empty(), "{}", s.name);
        let through = ConnectionConfig { user: user.name.clone(), ..tunnelled(s, db, SslMode::Prefer) };
        let conn = connect(Target { tunnel: Some(Arc::clone(&t)), ..target(&through, Some(&user.password)) }).await.unwrap();
        assert!(conn.info().warnings.is_empty(), "{}: {:?}", s.name, conn.info());
        user.drop_now(&root).await;
        seen += 1;
    }
    assert_eq!(seen, 1, "mysql57 is the server prefer reaches without TLS");
    t.close().await;
}

#[tokio::test]
async fn login_methods() {
    let db = require_testdb!();
    for s in mysql_servers(db).filter(|s| s.kind == Kind::MySql) {
        let root = open(&config(s, SslMode::Prefer), db).await;
        let methods = active_methods(&root).await;
        // Each server has the methods of its version: native is gone from 9, sha256 and caching_sha2 came in 5.6 and 8.0.
        let expected: &[&str] = match s.name.as_str() {
            "mysql57" => &["mysql_native_password", "sha256_password"],
            "mysql80" => &["mysql_native_password", "sha256_password", "caching_sha2_password"],
            _ => &["sha256_password", "caching_sha2_password"],
        };
        assert_eq!(methods, expected, "{}", s.name);
        for method in methods {
            let user = User::create(s, &root, method).await;
            // Without TLS: a scramble, or the password encrypted with the server's RSA key. The
            // second login finds caching_sha2_password's cache filled.
            for _ in 0..2 {
                let c = ConnectionConfig { user: user.name.clone(), ..config(s, SslMode::Disable) };
                let conn = open_as(&c, &user.password, Purpose::Query).await;
                assert_eq!(cipher(&conn).await, "", "{} {method}", s.name);
                assert!(conn.info().warnings.is_empty(), "{} {method}: {:?}", s.name, conn.info());
                assert_eq!(scalar(&conn, "SELECT CURRENT_USER()").await, Some(format!("{}@%", user.name)));
            }
            let c = ConnectionConfig { user: user.name.clone(), ..config(s, SslMode::Disable) };
            let e = connect(target(&c, Some("nope-pw"))).await.err().unwrap();
            assert_eq!(e.code(), "auth", "{} {method}: {e:?}", s.name);
            if rustls_can_reach(s) {
                // A new account's first login over TLS: the password goes over TLS.
                let tls_user = User::create(s, &root, method).await;
                let c = ConnectionConfig { user: tls_user.name.clone(), ..config(s, SslMode::Require) };
                assert!(!cipher(&open_as(&c, &tls_user.password, Purpose::Query).await).await.is_empty(), "{} {method}", s.name);
                tls_user.drop_now(&root).await;
            }
            user.drop_now(&root).await;
        }
    }
}

/// The login methods the server has as active plugins, of the three dbee speaks.
async fn active_methods(root: &MySqlConnection) -> Vec<&'static str> {
    let mut out = Vec::new();
    for m in ["mysql_native_password", "sha256_password", "caching_sha2_password"] {
        let sql = format!("SELECT PLUGIN_STATUS FROM information_schema.PLUGINS WHERE PLUGIN_NAME = '{m}'");
        if scalar(root, &sql).await.as_deref() == Some("ACTIVE") {
            out.push(m);
        }
    }
    out
}

/// A throwaway account `<unique>@%` with a random password, dropped by `drop_now` (or, should a
/// test fail first, by `Drop` through the server's own client).
struct User {
    server: Server,
    name: String,
    password: String,
    dropped: bool,
}

impl User {
    async fn create(s: &Server, root: &MySqlConnection, method: &str) -> User {
        let name = unique("dbee_u");
        let password = unique("pw");
        root.run(&format!("CREATE USER '{name}'@'%' IDENTIFIED WITH {method} BY '{password}'"), QUERY_LIMITS).await.unwrap();
        User { server: s.clone(), name, password, dropped: false }
    }

    async fn drop_now(mut self, root: &MySqlConnection) {
        root.run(&format!("DROP USER IF EXISTS '{}'@'%'", self.name), QUERY_LIMITS).await.unwrap();
        self.dropped = true;
    }
}

impl Drop for User {
    fn drop(&mut self) {
        if !self.dropped {
            let _ = dbee_testkit::mysql(&self.server, &format!("DROP USER IF EXISTS '{}'@'%'", self.name));
        }
    }
}

#[tokio::test]
async fn unreachable_server_is_connect_error() {
    unreachable().await;
}

#[tokio::test]
#[ignore = "timing; run with --ignored --nocapture"]
async fn unreachable_server_is_a_connect_error_within_ten_seconds() {
    let took = unreachable().await;
    println!("mysql: two unreachable servers refused in {took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

/// A port nobody listens on and a server that hangs up at once are `connect` errors; how long the
/// two took. Settings that cannot work are one too.
async fn unreachable() -> Duration {
    // A port nobody listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let none = ConnectionConfig { kind: DbKind::MySql, host: "127.0.0.1".into(), port, user: "root".into(), ..ConnectionConfig::default() };
    let started = Instant::now();
    assert_eq!(code(connect(target(&none, Some("pw"))).await), "connect");
    // A server that hangs up at once.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            drop(sock);
        }
    });
    assert_eq!(code(connect(target(&ConnectionConfig { port, ..none.clone() }, Some("pw"))).await), "connect");
    let took = started.elapsed();
    // Settings that cannot work.
    assert_eq!(code(connect(target(&ConnectionConfig { host: String::new(), ..none.clone() }, Some("pw"))).await), "connect");
    took
}

#[test]
fn connection_types_are_shareable() {
    fn send_sync<T: Send + Sync + 'static>() {}
    send_sync::<MySqlConnection>();
    send_sync::<Canceller>();
    send_sync::<MySqlDriver>();
}

#[tokio::test]
async fn mysql_driver_connects_through_the_trait() {
    let db = require_testdb!();
    assert_eq!(MySqlDriver.kinds(), [DbKind::MySql, DbKind::MariaDb]);
    for s in mysql_servers(db) {
        let c = config(s, SslMode::Prefer);
        let conn = MySqlDriver.connect(target(&c, Some(&db.password))).await.unwrap();
        let product = if s.kind == Kind::MariaDb { "MariaDB" } else { "MySQL" };
        assert_eq!((conn.info().product.as_str(), conn.info().version_num), (product, expected_num(&s.version)), "{}", s.name);
        assert_eq!(conn.dialect(), Dialect::MySql);
        assert!(!conn.is_closed());
        // Each method reaches the MySQL connection.
        assert_eq!(conn.scalar_u64("SELECT 40 + 2").await, Ok(42));
        let typed = conn.run("SELECT CAST(1.5 AS DECIMAL(3,1)) AS n", QUERY_LIMITS).await.unwrap();
        assert_eq!((typed.columns[0].base_type.as_str(), typed.rows[0][0].as_deref()), ("decimal", Some("1.5")), "{}", s.name);
        assert!(conn.run_untyped("SELECT 1 AS n", QUERY_LIMITS).await.unwrap().columns[0].base_type.is_empty());
        let catalog = conn.load_catalog().await.unwrap();
        assert!(catalog.table(&TableRef { schema: "shop".into(), name: "orders".into() }).is_some(), "{}", s.name);
        assert_eq!(conn.apply(&[], &ApplyGate::default()).await.map_err(|e| e.to_string()), Ok(0));
        // The trait's catalog is the connection's own, which dropped no row.
        let shop = ConnectionConfig { database: "shop".into(), ..c.clone() };
        let by_trait = MySqlDriver.connect(target(&shop, Some(&db.password))).await.unwrap().load_catalog().await.unwrap();
        let own = open(&shop, db).await;
        assert_eq!(own.catalog_rows().await.unwrap().dropped(), 0, "{}", s.name);
        assert_eq!(by_trait, own.load_catalog().await.unwrap(), "{}", s.name);
        let e = MySqlDriver.connect(target(&c, Some("nope"))).await.err().unwrap();
        assert_eq!(e.code(), "auth", "{}: {e:?}", s.name);
    }
}

#[tokio::test]
async fn the_trait_canceller_stops_sleep() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn: Arc<dyn Connection> = MySqlDriver.connect(target(&config(s, SslMode::Prefer), Some(&db.password))).await.unwrap();
        let canceller = conn.canceller();
        let id = conn.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
        let runner = Arc::clone(&conn);
        let job = tokio::spawn(async move { runner.run_untyped("SELECT SLEEP(30)", QUERY_LIMITS).await });
        until_sleeping(&open(&config(s, SslMode::Prefer), db).await, id).await;
        canceller.cancel().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not cancelled").unwrap();
        assert_eq!(result.err(), Some(DbError::Cancelled), "{}", s.name);
        assert_eq!(conn.scalar_u64("SELECT 1").await, Ok(1), "{}", s.name);
    }
}

/// The server's databases, read through the connection the driver opens for the list.
async fn listed(target: Target<'_>) -> Result<Vec<String>, DbError> {
    let conn = MySqlDriver.list_connection(target).await?.expect("MySQL has a database list");
    conn.list_databases().await
}

/// The server's databases, its own after the user's: listed without a database, whatever the
/// settings name, also one that is not there; through the tunnel too.
#[tokio::test]
async fn mysql_lists_databases_system_last() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    let system = ["mysql", "information_schema", "performance_schema", "sys"];
    for s in mysql_servers(db) {
        let direct = ["", "shop", "no_such_database"].map(|database| ConnectionConfig { database: database.into(), ..config(s, SslMode::Prefer) });
        let tunnelled = tunnelled(s, db, SslMode::Prefer);
        for c in direct.iter().chain([&tunnelled]) {
            let tunnel = c.ssh.as_ref().map(|_| Arc::clone(&t));
            let names = listed(Target { tunnel, ..target(c, Some(&db.password)) }).await.unwrap();
            let at = |name: &str| names.iter().position(|n| n == name).unwrap_or_else(|| panic!("{}: no {name} in {names:?}", s.name));
            for user in ["many", "odd", "scratch", "shop"] {
                assert!(system.iter().all(|own| at(user) < at(own)), "{} {:?}: {names:?}", s.name, c.database);
            }
        }
        assert_eq!(listed(target(&config(s, SslMode::Prefer), Some("nope"))).await.unwrap_err().code(), "auth", "{}", s.name);
    }
    t.close().await;
}

// ---------------------------------------------------------------------------------------------
// The fake server: packets as MySQL's protocol documentation describes them
// ---------------------------------------------------------------------------------------------

/// The password the fake server's client logs in with; no packet may carry it in clear.
const FAKE_PASSWORD: &str = "fake-secret-pw";

/// What a fake server announces.
fn server_caps(deprecate_eof: bool) -> u32 {
    let mut caps = CapabilityFlags::CLIENT_PROTOCOL_41
        | CapabilityFlags::CLIENT_SECURE_CONNECTION
        | CapabilityFlags::CLIENT_PLUGIN_AUTH
        | CapabilityFlags::CLIENT_FOUND_ROWS
        | CapabilityFlags::CLIENT_TRANSACTIONS
        | CapabilityFlags::CLIENT_CONNECT_WITH_DB
        | CapabilityFlags::CLIENT_LONG_PASSWORD;
    if deprecate_eof {
        caps |= CapabilityFlags::CLIENT_DEPRECATE_EOF;
    }
    caps.bits()
}

/// Protocol::HandshakeV10 with a 20-byte nonce.
fn greeting(version: &str, plugin: &str, caps: u32) -> Vec<u8> {
    let nonce: [u8; 20] = *b"abcdefghijklmnopqrst";
    let mut p = vec![10];
    p.extend_from_slice(version.as_bytes());
    p.push(0);
    p.extend_from_slice(&7u32.to_le_bytes());
    p.extend_from_slice(&nonce[..8]);
    p.push(0);
    p.extend_from_slice(&(caps as u16).to_le_bytes());
    p.push(45);
    p.extend_from_slice(&StatusFlags::SERVER_STATUS_AUTOCOMMIT.bits().to_le_bytes());
    p.extend_from_slice(&((caps >> 16) as u16).to_le_bytes());
    p.push(21);
    p.extend_from_slice(&[0; 10]);
    p.extend_from_slice(&nonce[8..]);
    p.push(0);
    p.extend_from_slice(plugin.as_bytes());
    p.push(0);
    p
}

fn lenenc(n: u64) -> Vec<u8> {
    match n {
        0..=250 => vec![n as u8],
        251..=0xFFFF => [&[0xFC][..], &(n as u16).to_le_bytes()].concat(),
        0x1_0000..=0xFF_FFFF => [&[0xFD][..], &(n as u32).to_le_bytes()[..3]].concat(),
        _ => [&[0xFE][..], &n.to_le_bytes()].concat(),
    }
}

fn lenenc_str(s: &[u8]) -> Vec<u8> {
    [lenenc(s.len() as u64), s.to_vec()].concat()
}

/// OK_Packet (header `0x00`, or `0xFE` to end a result set under CLIENT_DEPRECATE_EOF).
fn ok_packet(header: u8, affected: u64) -> Vec<u8> {
    let mut p = vec![header];
    p.extend(lenenc(affected));
    p.extend(lenenc(0));
    p.extend_from_slice(&StatusFlags::SERVER_STATUS_AUTOCOMMIT.bits().to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p
}

/// ERR_Packet with a SQL state.
fn err_packet(code: u16, message: &str) -> Vec<u8> {
    let mut p = vec![0xFF];
    p.extend_from_slice(&code.to_le_bytes());
    p.extend_from_slice(b"#HY000");
    p.extend_from_slice(message.as_bytes());
    p
}

/// EOF_Packet, before CLIENT_DEPRECATE_EOF.
fn eof_packet() -> Vec<u8> {
    let mut p = vec![0xFE, 0, 0];
    p.extend_from_slice(&StatusFlags::SERVER_STATUS_AUTOCOMMIT.bits().to_le_bytes());
    p
}

/// Protocol::ColumnDefinition41 of a `VAR_STRING` column.
fn column_def(name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    for part in ["def", "", "", "", name, name] {
        p.extend(lenenc_str(part.as_bytes()));
    }
    p.push(0x0C);
    p.extend_from_slice(&45u16.to_le_bytes());
    p.extend_from_slice(&1024u32.to_le_bytes());
    p.push(0xFD);
    p.extend_from_slice(&0u16.to_le_bytes());
    p.push(0);
    p.extend_from_slice(&[0, 0]);
    p
}

/// A text result set of one column and one row, ended the way `deprecate_eof` says.
fn one_value(name: &str, value: Option<&str>, deprecate_eof: bool) -> Vec<Vec<u8>> {
    let mut packets = vec![lenenc(1), column_def(name)];
    if !deprecate_eof {
        packets.push(eof_packet());
    }
    packets.push(match value {
        Some(v) => lenenc_str(v.as_bytes()),
        None => vec![0xFB],
    });
    packets.push(if deprecate_eof { ok_packet(0xFE, 0) } else { eof_packet() });
    packets
}

/// Frame `payload` as one packet with sequence id `seq` (payloads under 16 MiB only).
fn frame(payload: &[u8], seq: u8) -> Vec<u8> {
    let len = (payload.len() as u32).to_le_bytes();
    [&len[..3], &[seq], payload].concat()
}

/// A fake server for one connection on 127.0.0.1. It sends `first` (raw bytes), then answers
/// each packet the client sends with what `answer` returns for it, framed with the sequence ids
/// that follow the client's; an empty answer closes the connection. Returns the port, and every
/// payload the client sent once the connection has closed.
async fn fake_server<F>(first: Vec<u8>, mut answer: F) -> (u16, tokio::task::JoinHandle<Vec<Vec<u8>>>)
where
    F: FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { serve_fake(&listener, &first, &mut answer).await });
    (port, server)
}

/// One connection of [`fake_server`] on `listener`: `first`, then `answer` to each packet; what
/// the client sent.
async fn serve_fake<F>(listener: &TcpListener, first: &[u8], answer: &mut F) -> Vec<Vec<u8>>
where
    F: FnMut(&[u8]) -> Vec<Vec<u8>>,
{
    {
        let mut got = Vec::new();
        let Ok((mut sock, _)) = listener.accept().await else { return got };
        if sock.write_all(first).await.is_err() {
            return got;
        }
        loop {
            let mut header = [0u8; 4];
            if sock.read_exact(&mut header).await.is_err() {
                return got;
            }
            let len = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
            let mut payload = vec![0; len];
            if sock.read_exact(&mut payload).await.is_err() {
                return got;
            }
            let replies = answer(&payload);
            got.push(payload);
            if replies.is_empty() {
                return got;
            }
            let mut seq = header[3];
            for reply in replies {
                seq = seq.wrapping_add(1);
                if sock.write_all(&frame(&reply, seq)).await.is_err() {
                    return got;
                }
            }
        }
    }
}

fn fake_config(port: u16) -> ConnectionConfig {
    ConnectionConfig {
        kind: DbKind::MySql,
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        ssl_mode: SslMode::Disable,
        ..ConnectionConfig::default()
    }
}

/// Connect to the fake server on `port` as `root` with [`FAKE_PASSWORD`].
async fn connect_fake(port: u16) -> Result<MySqlConnection, DbError> {
    connect_fake_as(&fake_config(port)).await
}

async fn connect_fake_as(c: &ConnectionConfig) -> Result<MySqlConnection, DbError> {
    connect(Target { config: c, password: Some(FAKE_PASSWORD), tunnel: None, purpose: Purpose::Query, create_file: false, allowed_dirs: None }).await
}

fn holds_password(payloads: &[Vec<u8>]) -> bool {
    payloads.iter().any(|p| p.windows(FAKE_PASSWORD.len()).any(|w| w == FAKE_PASSWORD.as_bytes()))
}

#[tokio::test]
async fn auth_switch_to_an_unknown_method_is_auth_error() {
    for method in ["mysql_clear_password", "auth_gssapi_client", "client_ed25519"] {
        let caps = server_caps(true);
        let switch = [&[0xFE][..], method.as_bytes(), &[0], b"0123456789abcdefghij", &[0]].concat();
        let again = switch.clone();
        let (port, server) = fake_server(frame(&greeting("8.0.99", "mysql_native_password", caps), 0), move |_| vec![switch.clone()]).await;
        let e = connect_fake(port).await.err().unwrap();
        assert_eq!(e.code(), "auth", "{method}: {e:?}");
        assert!(e.to_string().contains(method), "{e}");
        let got = server.await.unwrap();
        // The handshake response with its scramble, then nothing: the client hung up.
        assert_eq!(got.len(), 1, "{method}: {got:?}");
        assert!(!holds_password(&got), "{method}");

        // A greeting that names it is answered with a method dbee speaks; the switch back to it
        // ends the login.
        let (port, server) = fake_server(frame(&greeting("8.0.99", method, caps), 0), move |_| vec![again.clone()]).await;
        let e = connect_fake(port).await.err().unwrap();
        assert!(e.code() == "auth" && e.to_string().contains(method), "{method}: {e:?}");
        let got = server.await.unwrap();
        assert_eq!(got.len(), 1, "{method}: {got:?}");
        assert!(got[0].ends_with(b"caching_sha2_password\0") && !holds_password(&got), "{method}");
    }
}

#[tokio::test]
async fn full_login_without_tls_never_sends_the_password_in_clear() {
    // caching_sha2_password: the server asks for the full login (AuthMoreData 0x04); without TLS
    // the client must ask for the RSA key (0x02). The key the fake server sends is no key.
    let caps = server_caps(true);
    let mut step = 0;
    let (port, server) = fake_server(frame(&greeting("8.0.99", "caching_sha2_password", caps), 0), move |payload| {
        step += 1;
        match step {
            1 => vec![vec![0x01, 0x04]],
            2 if payload == [0x02] => vec![[&[0x01][..], b"-----BEGIN PUBLIC KEY-----\nnot a key\n-----END PUBLIC KEY-----\n"].concat()],
            _ => vec![],
        }
    })
    .await;
    let e = connect_fake(port).await.err().unwrap();
    assert!(matches!(&e, DbError::Connect(m) if m.contains("RSA key")), "{e:?}");
    let got = server.await.unwrap();
    assert_eq!(got.get(1).map(Vec::as_slice), Some(&[0x02][..]), "{got:?}");
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(!holds_password(&got));

    // sha256_password without TLS asks for the key (0x01) instead of sending the password.
    let (port, server) = fake_server(frame(&greeting("5.7.99", "sha256_password", caps), 0), |_| vec![]).await;
    assert!(connect_fake(port).await.is_err());
    let got = server.await.unwrap();
    assert_eq!(got.len(), 1);
    assert!(!holds_password(&got));
}

/// A 2048-bit RSA public key, as MySQL sends its own.
const RSA_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAlZT/mLblmmuu4gQOB1ha
7ixbmi0XguCiWT9sXg6N/C9/wEfE/t96ZzdQIg9YduYVHEgWG3QgYUj4Oh3r7nFo
k276KvixSutLYjAz8Pp58iDPRxDQz5Po6uEABLna312psovJLOTH87OfJKlKVQG2
oLdSJuoVVp+YzNE9xlR8k03mTVCpRgBKR8bKb8oZe0OKhacqokceaisVzsk837rj
HVS3m+rvy/Ea3+mtZpqmawtVPvxzCElTfdGtOVwlp63JlJ3CCQ5FB6swEfTW5BWa
iUYSUy3d4JX3yQ/kS5EeezphehJ0WUg1WKgxwaSRAriJwl7p4NVCi+ScixSPyC9D
iwIDAQAB
-----END PUBLIC KEY-----
";

#[tokio::test]
async fn the_servers_key_is_refused_where_encryption_was_wanted() {
    // prefer, and the server offers no TLS: an attacker on the link could have taken it away.
    let caps = server_caps(true);
    let prefer = |port| ConnectionConfig { ssl_mode: SslMode::Prefer, ..fake_config(port) };
    let (port, server) = fake_server(frame(&greeting("8.0.99", "caching_sha2_password", caps), 0), |_| vec![vec![0x01, 0x04]]).await;
    let e = connect_fake_as(&prefer(port)).await.err().unwrap();
    let no_tls =
        |e: &DbError| matches!(e, DbError::Auth { message } if message.contains("offers no TLS") && message.contains("turn on TLS on the server"));
    assert!(no_tls(&e), "{e:?}");
    let got = server.await.unwrap();
    assert_eq!(got.len(), 1, "no key request after the scramble: {got:?}");
    assert!(!holds_password(&got));
    // A greeting naming sha256_password is answered with the flavor's default; the switch to
    // sha256_password, whose first answer would ask for the key, ends the login.
    let switch = [&b"\xFEsha256_password\0"[..], b"0123456789abcdefghij", &[0]].concat();
    let (port, server) = fake_server(frame(&greeting("5.7.99", "sha256_password", caps), 0), move |_| vec![switch.clone()]).await;
    let e = connect_fake_as(&prefer(port)).await.err().unwrap();
    assert!(no_tls(&e), "{e:?}");
    let got = server.await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].ends_with(b"caching_sha2_password\0") && !holds_password(&got));
}

#[tokio::test]
async fn a_sha256_greeting_lets_the_accounts_method_decide() {
    // prefer, no TLS, the key not allowed: a mysql_native_password account still logs in.
    let nonce = *b"ABCDEFGHIJKLMNOPQRST";
    let mut step = 0;
    let answer = move |payload: &[u8]| -> Vec<Vec<u8>> {
        step += 1;
        match step {
            1 => vec![[&[0xFE][..], b"mysql_native_password\0", &nonce, &[0]].concat()],
            2 => vec![ok_packet(0, 0)],
            _ => answer_as_mariadb(payload),
        }
    };
    let (port, server) = fake_server(frame(&greeting("8.0.99", "sha256_password", server_caps(true)), 0), answer).await;
    let conn = connect_fake_as(&ConnectionConfig { ssl_mode: SslMode::Prefer, ..fake_config(port) }).await.unwrap();
    // No word of the server's key; the plain link after prefer warns.
    let warnings = &conn.info().warnings;
    assert!(warnings.len() == 1 && warnings[0].contains("not encrypted"), "{:?}", conn.info());
    drop(conn);
    let got = server.await.unwrap();
    assert!(got[0].ends_with(b"caching_sha2_password\0"), "{got:?}");
    assert_eq!(got[1], scramble_native(&nonce, FAKE_PASSWORD.as_bytes()).unwrap());
    assert!(!holds_password(&got));
}

#[tokio::test]
async fn the_servers_key_when_allowed_comes_with_a_warning() {
    for (mode, allow, warned) in [(SslMode::Prefer, true, true), (SslMode::Disable, false, false)] {
        let mut step = 0;
        let answer = move |payload: &[u8]| -> Vec<Vec<u8>> {
            step += 1;
            match step {
                1 => vec![vec![0x01, 0x04]],
                2 if payload == [0x02] => vec![[&[0x01][..], RSA_KEY.as_bytes()].concat()],
                3 if payload.len() == 256 => vec![ok_packet(0, 0)],
                2 | 3 => vec![],
                _ => answer_as_mariadb(payload),
            }
        };
        let (port, server) = fake_server(frame(&greeting("8.0.99", "caching_sha2_password", server_caps(true)), 0), answer).await;
        let mut c = ConnectionConfig { ssl_mode: mode, ..fake_config(port) };
        if allow {
            c.options.insert(ALLOW_PUBLIC_KEY.id.into(), "true".into());
        }
        let conn = connect_fake_as(&c).await.unwrap_or_else(|e| panic!("{mode:?}: {e}"));
        let warnings = conn.info().warnings.clone();
        assert_eq!(warnings.iter().any(|w| w.contains("could not check")), warned, "{mode:?}: {warnings:?}");
        // prefer ended up plain on the direct link: that warns too.
        assert_eq!(warnings.iter().any(|w| w.contains("not encrypted")), mode == SslMode::Prefer, "{mode:?}: {warnings:?}");
        assert_eq!(warnings.len(), 2 * usize::from(warned), "{mode:?}: {warnings:?}");
        drop(conn);
        let got = server.await.unwrap();
        assert_eq!((got[1].as_slice(), got[2].len()), (&[0x02][..], 256), "{mode:?}");
        assert!(!holds_password(&got));
    }
}

/// Answers a fake MySQL 5.6: no CLIENT_DEPRECATE_EOF, and no `transaction_read_only`.
fn answer_as_mysql56(payload: &[u8]) -> Vec<Vec<u8>> {
    let sql = String::from_utf8_lossy(payload.get(1..).unwrap_or_default()).into_owned();
    match payload.first() {
        Some(0x03) if sql.starts_with("SELECT CONNECTION_ID()") => {
            let mut packets = vec![lenenc(2), column_def("CONNECTION_ID()"), column_def("@@version"), eof_packet()];
            packets.push([lenenc_str(b"7"), lenenc_str(b"5.6.51-log")].concat());
            packets.push(eof_packet());
            packets
        }
        Some(0x03) if sql.contains("@@session.transaction_read_only") => vec![err_packet(1193, "Unknown system variable 'transaction_read_only'")],
        Some(0x03) if sql.contains("@@session.tx_read_only") => one_value("@@session.tx_read_only", Some("0"), false),
        Some(0x03) if sql.starts_with("SELECT") => one_value("v", None, false),
        Some(0x03) if sql.starts_with("SET") => vec![ok_packet(0, 0)],
        Some(0x03) => vec![err_packet(1064, "You have an error in your SQL syntax")],
        Some(0x01) => vec![],
        _ => vec![ok_packet(0, 0)],
    }
}

#[tokio::test]
async fn an_older_server_warns_and_goes_on() {
    let (port, _server) = fake_server(frame(&greeting("5.6.51-log", "mysql_native_password", server_caps(false)), 0), answer_as_mysql56).await;
    let conn = connect_fake(port).await.unwrap();
    let info = conn.info();
    assert_eq!((info.product.as_str(), info.version.as_str(), info.version_num), ("MySQL", "5.6.51", 50651));
    assert_eq!(info.warnings.len(), 1, "{info:?}");
    assert!(info.warnings[0].contains("5.7"), "{info:?}");
    assert!(!info.read_only);
    // Result sets end with EOF packets on a server without CLIENT_DEPRECATE_EOF.
    let r = conn.run("SELECT 1", QUERY_LIMITS).await.unwrap();
    assert_eq!((r.columns.len(), r.columns[0].name.as_str(), r.rows.as_slice()), (1, "v", &[vec![None]][..]));
    let e = conn.run("BOGUS", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.code == "1064" && s.code_label == Some("MySQL error")), "{e:?}");
    assert!(e.to_string().ends_with("(MySQL error 1064)"), "{e}");
    assert!(!conn.is_closed(), "a server error leaves the connection usable");
}

/// Answers a fake MariaDB 11.4 that refuses some queries with errors (`q<errno>`) and hangs up on
/// any other.
fn answer_as_mariadb(payload: &[u8]) -> Vec<Vec<u8>> {
    let sql = String::from_utf8_lossy(payload.get(1..).unwrap_or_default()).into_owned();
    match payload.first() {
        Some(0x03) if sql.starts_with("SELECT CONNECTION_ID()") => {
            let mut p = vec![lenenc(2), column_def("a"), column_def("b")];
            p.push([lenenc_str(b"7"), lenenc_str(b"11.4.2-MariaDB")].concat());
            p.push(ok_packet(0xFE, 0));
            p
        }
        Some(0x03) if sql.contains("@@session.tx_read_only") => one_value("x", Some("0"), true),
        Some(0x03) if sql.starts_with("SET") => vec![ok_packet(0, 0)],
        Some(0x03) if sql == "q1146" => vec![err_packet(1146, "Table 'shop.x' doesn't exist")],
        Some(0x03) if sql == "q1317" => vec![err_packet(1317, "Query execution was interrupted")],
        Some(0x03) if sql == "q1792" => vec![err_packet(1792, "Cannot execute statement in a READ ONLY transaction")],
        Some(0x03) => vec![],
        _ => vec![ok_packet(0, 0)],
    }
}

#[tokio::test]
async fn an_unknown_greeting_method_is_answered_with_a_known_one() {
    // MariaDB's greeting names client_ed25519; dbee answers with mysql_native_password, and the
    // server's switch to it, with a new challenge, gets that method's scramble.
    let nonce = *b"ABCDEFGHIJKLMNOPQRST";
    let mut step = 0;
    let answer = move |payload: &[u8]| -> Vec<Vec<u8>> {
        step += 1;
        match step {
            1 => vec![[&[0xFE][..], b"mysql_native_password\0", &nonce, &[0]].concat()],
            2 => vec![ok_packet(0, 0)],
            _ => answer_as_mariadb(payload),
        }
    };
    let (port, server) = fake_server(frame(&greeting("5.5.5-11.4.2-MariaDB", "client_ed25519", server_caps(true)), 0), answer).await;
    let conn = connect_fake(port).await.unwrap();
    assert_eq!(conn.info().product, "MariaDB");
    drop(conn);
    let got = server.await.unwrap();
    assert!(got[0].ends_with(b"mysql_native_password\0"), "{got:?}");
    assert_eq!(got[1], scramble_native(&nonce, FAKE_PASSWORD.as_bytes()).unwrap());
    assert!(!holds_password(&got));
}

#[tokio::test]
async fn a_read_only_check_error_is_reported_as_itself() {
    // Only "no such variable" (1193) moves on to the other spelling.
    let answer = |payload: &[u8]| -> Vec<Vec<u8>> {
        let sql = String::from_utf8_lossy(payload.get(1..).unwrap_or_default()).into_owned();
        match payload.first() {
            Some(0x03) if sql.contains("@@session.transaction_read_only") => vec![err_packet(1227, "Access denied; you need the SUPER privilege")],
            _ => answer_as_mysql56(payload),
        }
    };
    let (port, server) = fake_server(frame(&greeting("5.6.51-log", "mysql_native_password", server_caps(false)), 0), answer).await;
    let e = connect_fake(port).await.err().unwrap();
    assert!(matches!(&e, DbError::Server(s) if s.code == "1227"), "{e:?}");
    let got = server.await.unwrap();
    assert!(!got.iter().any(|p| p.windows(12).any(|w| w == b"tx_read_only")), "asked for the other spelling");
}

#[tokio::test]
async fn a_server_without_found_rows_is_refused() {
    let caps = server_caps(true) & !CapabilityFlags::CLIENT_FOUND_ROWS.bits();
    let (port, server) = fake_server(frame(&greeting("8.0.99", "mysql_native_password", caps), 0), |_| vec![ok_packet(0, 0)]).await;
    let e = connect_fake(port).await.err().unwrap();
    assert!(matches!(&e, DbError::Connect(m) if m.contains("CLIENT_FOUND_ROWS")), "{e:?}");
    assert!(server.await.unwrap().is_empty(), "refused before the handshake response");
}

/// The statements a connection's setup sent to a fake MariaDB server, for `purpose`.
async fn setup_statements(purpose: Purpose) -> Vec<String> {
    let (port, server) =
        fake_server(frame(&greeting("5.5.5-11.4.2-MariaDB", "mysql_native_password", server_caps(true)), 0), answer_as_mariadb).await;
    let c = fake_config(port);
    let conn =
        connect(Target { config: &c, password: Some(FAKE_PASSWORD), tunnel: None, purpose, create_file: false, allowed_dirs: None }).await.unwrap();
    drop(conn);
    let got = server.await.unwrap();
    got.iter().filter(|p| p.first() == Some(&0x03)).map(|p| String::from_utf8_lossy(&p[1..]).into_owned()).collect()
}

/// The test servers' default is autocommit on, and only a global setting changes that, so a
/// fake server shows what the browse session sends.
#[tokio::test]
async fn the_browse_session_turns_autocommit_on() {
    let browse = setup_statements(Purpose::Browse).await;
    let at = |prefix: &str| browse.iter().position(|s| s.starts_with(prefix));
    let (names, mode, autocommit) = (at("SET NAMES"), at("SET SESSION sql_mode"), at("SET SESSION autocommit = 1"));
    assert!(names < mode && mode < autocommit && names.is_some(), "{browse:?}");
    let query = setup_statements(Purpose::Query).await;
    assert!(!query.iter().any(|s| s.contains("autocommit") || s.contains("sql_mode")), "the query session keeps the server's: {query:?}");
}

/// A fake MariaDB server's error packets: the error number with its flavor's label, or what
/// dbee makes of the number.
#[tokio::test]
async fn server_errors_carry_the_errno_and_flavor() {
    let (port, _server) =
        fake_server(frame(&greeting("5.5.5-11.4.2-MariaDB", "mysql_native_password", server_caps(true)), 0), answer_as_mariadb).await;
    let conn = connect_fake(port).await.unwrap();
    assert_eq!((conn.info().product.as_str(), conn.info().version_num), ("MariaDB", 110402));
    let e = conn.run("q1146", QUERY_LIMITS).await.unwrap_err();
    assert_eq!(e.to_string(), "Table 'shop.x' doesn't exist (MariaDB error 1146)");
    let DbError::Server(s) = &e else { panic!("{e:?}") };
    assert_eq!((s.code.as_str(), s.code_label, &s.detail, &s.hint, s.position, s.meaning), ("1146", Some("MariaDB error"), &None, &None, None, None));
    assert_eq!(conn.run("q1317", QUERY_LIMITS).await.unwrap_err(), DbError::Cancelled);
    assert_eq!(conn.run("q1792", QUERY_LIMITS).await.unwrap_err().code(), "readOnly");
    // The server hangs up: the connection is lost, and stays so.
    assert_eq!(conn.run("bye", QUERY_LIMITS).await.unwrap_err(), DbError::ConnectionLost);
    assert!(conn.is_closed());
    assert_eq!(conn.run("q1146", QUERY_LIMITS).await.unwrap_err(), DbError::ConnectionLost);
}

// ---------------------------------------------------------------------------------------------
// Hostile servers
// ---------------------------------------------------------------------------------------------

/// A stream that serves `packets` headers announcing `0xFFFFFF` bytes each, followed by that
/// many zero bytes, and counts what it served. Writes are swallowed.
struct Endless {
    packets: usize,
    served: Arc<AtomicUsize>,
}

impl AsyncRead for Endless {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let packet = 4 + 0xFF_FFFF;
        let at = self.served.load(Ordering::SeqCst);
        if at >= packet * self.packets {
            return Poll::Ready(Ok(()));
        }
        let (n, off) = (at / packet, at % packet);
        let mut chunk = Vec::new();
        if off < 4 {
            chunk.extend_from_slice(&[0xFF, 0xFF, 0xFF, n as u8][off..]);
        }
        chunk.resize(chunk.len() + (packet - off.max(4)).min(64 * 1024), 0);
        let take = chunk.len().min(buf.remaining());
        buf.put_slice(&chunk[..take]);
        self.served.fetch_add(take, Ordering::SeqCst);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Endless {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn huge_packet_header_is_refused() {
    assert_eq!(MAX_PAYLOAD, 1 << 30);
    let packet = 4 + 0xFF_FFFF;
    // Three continuation packets of 16 MiB against a cap of two: refused at the third header.
    let served = Arc::new(AtomicUsize::new(0));
    let mut p = Packets::new(Box::new(Endless { packets: 3, served: Arc::clone(&served) }), 2 * 0xFF_FFFF);
    let e = p.read().await.unwrap_err();
    assert!(matches!(&e, DbError::Other(m) if m.contains("larger than")), "{e:?}");
    assert!(served.load(Ordering::SeqCst) <= 2 * packet + 64 * 1024 + 4, "read {} bytes", served.load(Ordering::SeqCst));
    // A cap below one packet: refused at the first header, before any of its payload.
    let served = Arc::new(AtomicUsize::new(0));
    let mut p = Packets::new(Box::new(Endless { packets: 3, served: Arc::clone(&served) }), 1024 * 1024);
    assert_eq!(p.read().await.unwrap_err().code(), "other");
    assert!(served.load(Ordering::SeqCst) <= 64 * 1024 + 4);
    // Under the cap the payload goes on after each 0xFFFFFF packet, here into the end of the stream.
    let served = Arc::new(AtomicUsize::new(0));
    let mut p = Packets::new(Box::new(Endless { packets: 2, served }), MAX_PAYLOAD);
    assert_eq!(p.read().await.unwrap_err(), DbError::ConnectionLost, "the stream ends where the third packet would be");
}

/// A fake server that sends `first` (raw bytes), then each batch of `batches` after a packet
/// from the client, and closes its sending side after the last; it records the client's
/// packets until the client hangs up.
async fn scripted_server(first: Vec<u8>, batches: Vec<Vec<Vec<u8>>>) -> (u16, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let mut got = Vec::new();
        let Ok((mut sock, _)) = listener.accept().await else { return got };
        let mut batches = batches.into_iter();
        if sock.write_all(&first).await.is_err() || (batches.len() == 0 && sock.shutdown().await.is_err()) {
            return got;
        }
        loop {
            let mut header = [0u8; 4];
            if sock.read_exact(&mut header).await.is_err() {
                return got;
            }
            let mut payload = vec![0; u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize];
            if sock.read_exact(&mut payload).await.is_err() {
                return got;
            }
            got.push(payload);
            let Some(batch) = batches.next() else { continue };
            let mut seq = header[3];
            for packet in batch {
                seq = seq.wrapping_add(1);
                if sock.write_all(&frame(&packet, seq)).await.is_err() {
                    return got;
                }
            }
            if batches.len() == 0 && sock.shutdown().await.is_err() {
                return got;
            }
        }
    });
    (port, server)
}

/// Connect to a [`scripted_server`]; the result must be an error, never a panic, and no packet
/// may carry the password.
fn hostile(first: Vec<u8>, batches: Vec<Vec<Vec<u8>>>) -> Result<(), TestCaseError> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let (port, server) = scripted_server(first, batches).await;
        let result = tokio::time::timeout(Duration::from_secs(20), connect_fake(port)).await;
        let got = server.await.unwrap();
        prop_assert!(matches!(result, Ok(Err(_))), "{:?}", result.map(|r| r.map(|c| c.info().clone())));
        prop_assert!(!holds_password(&got));
        Ok(())
    })
}

/// A valid greeting with some bytes changed and maybe cut short.
fn mutated(base: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    let len = base.len();
    (prop::collection::vec((0..len, any::<u8>()), 0..6), 0..=len).prop_map(move |(changes, cut)| {
        let mut p = base.clone();
        for (at, byte) in changes {
            p[at] = byte;
        }
        p.truncate(cut.max(1));
        p
    })
}

/// A packet that starts like one of the server's packets (OK, more data, a lenenc prefix,
/// NULL, a switch or EOF, ERR) or with any byte, and goes on with random bytes.
fn packet() -> impl Strategy<Value = Vec<u8>> {
    let head = prop_oneof![Just(0x00u8), Just(0x01), Just(0x03), Just(0x04), Just(0xFB), Just(0xFC), Just(0xFD), Just(0xFE), Just(0xFF), any::<u8>()];
    (head, prop::collection::vec(any::<u8>(), 0..64)).prop_map(|(h, rest)| [vec![h], rest].concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn hostile_handshake_never_panics(
        random in prop::collection::vec(any::<u8>(), 0..200),
        changed in mutated(greeting("8.0.99", "caching_sha2_password", server_caps(true))),
        raw in prop::collection::vec(any::<u8>(), 0..40),
    ) {
        hostile(frame(&random, 0), vec![])?;
        hostile(frame(&changed, 0), vec![vec![ok_packet(0, 0)]])?;
        hostile(raw, vec![])?;
    }

    #[test]
    fn hostile_login_replies_never_panic(
        plugin in prop_oneof![Just("mysql_native_password"), Just("caching_sha2_password"), Just("sha256_password")],
        replies in prop::collection::vec(packet(), 1..6),
    ) {
        hostile(frame(&greeting("8.0.99", plugin, server_caps(true)), 0), vec![replies])?;
    }

    #[test]
    fn hostile_results_never_panic(
        deprecate_eof in any::<bool>(),
        count in 0u64..4,
        defs in prop::collection::vec(prop_oneof![packet(), mutated(column_def("c")), Just(column_def(&"n".repeat(70_000)))], 0..4),
        rows in prop::collection::vec(packet(), 0..4),
    ) {
        let answer = [vec![lenenc(count)], defs, rows].concat();
        hostile(frame(&greeting("8.0.99", "mysql_native_password", server_caps(deprecate_eof)), 0), vec![vec![ok_packet(0, 0)], answer])?;
    }
}

/// M1: a `prefer` connection to a server that offers no TLS says its link is not encrypted, and
/// warns; one that asked for no TLS says so too, without the warning.
#[tokio::test]
async fn a_plain_link_after_prefer_warns() {
    for (mode, warned) in [(SslMode::Prefer, true), (SslMode::Disable, false)] {
        let (port, server) =
            fake_server(frame(&greeting("5.5.5-11.4.2-MariaDB", "mysql_native_password", server_caps(true)), 0), answer_as_mariadb).await;
        let conn = connect_fake_as(&ConnectionConfig { ssl_mode: mode, ..fake_config(port) }).await.unwrap();
        let info = conn.info().clone();
        drop(conn);
        server.await.unwrap();
        assert_eq!(info.tls, Some(TlsState::Off), "{mode:?}");
        assert_eq!(info.warnings.iter().any(|w| w.contains("not encrypted") && w.contains("verify-full")), warned, "{mode:?}: {info:?}");
        assert_eq!(info.warnings.len(), usize::from(warned), "{mode:?}: {info:?}");
    }
}

/// M1: when the server offers TLS and the handshake fails, `prefer` connects again without TLS,
/// and that connection warns.
#[tokio::test]
async fn prefer_after_a_failed_handshake_warns() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let offers_tls = server_caps(true) | CapabilityFlags::CLIENT_SSL.bits();
        sock.write_all(&frame(&greeting("5.5.5-11.4.2-MariaDB", "mysql_native_password", offers_tls), 0)).await.unwrap();
        // The SSL request (32 bytes and the header), then a fatal handshake_failure alert.
        let mut ssl_request = [0u8; 36];
        sock.read_exact(&mut ssl_request).await.unwrap();
        sock.write_all(b"\x15\x03\x03\x00\x02\x02\x28").await.unwrap();
        drop(sock);
        let first = frame(&greeting("5.5.5-11.4.2-MariaDB", "mysql_native_password", server_caps(true)), 0);
        serve_fake(&listener, &first, &mut answer_as_mariadb).await
    });
    let conn = connect_fake_as(&ConnectionConfig { ssl_mode: SslMode::Prefer, ..fake_config(port) }).await.unwrap();
    let info = conn.info().clone();
    drop(conn);
    server.await.unwrap();
    assert_eq!(info.tls, Some(TlsState::Off));
    assert!(info.warnings.len() == 1 && info.warnings[0].contains("not encrypted"), "{info:?}");
}

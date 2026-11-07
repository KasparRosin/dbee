//! Connection tests against the Docker test servers (`cargo xtask testdb up`). Each test runs
//! against every Postgres server in `db.servers` (Postgres 14 and the newest major).
//!
//! Safety: tunnels are opened with an `SshEnv` over a throwaway home from `temp_home` and no
//! agent, so the real `~/.ssh` and SSH agent are never touched. Nothing here writes: the one
//! statement that would create a table runs on a read-only connection, and a writable
//! connection drops that name afterwards in case it was created anyway.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind, SshAuth, SshConfig, SslMode};
use dbee_core::TableRef;
use dbee_driver::{ApplyGate, Connection, DbError, Driver, Meaning, Purpose, QUERY_LIMITS, Target};
use dbee_pg::query;
use dbee_pg::{Canceller, MIN_VERSION_NUM, PgConnection, PgDriver, connect, version_warning};
use dbee_sql::Dialect;
use dbee_ssh::{AgentSource, SshEnv, SshSecrets, Tunnel};
use dbee_testkit::{Kind, Server, TempHome, TestDb, require_testdb, temp_home, unique};
use tokio::io::AsyncReadExt as _;

/// The Postgres test servers.
fn postgres(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::Postgres])
}

/// `postgres@127.0.0.1:<port>/dbee` with the given TLS mode.
fn config(s: &Server, mode: SslMode) -> ConnectionConfig {
    ConnectionConfig {
        name: format!("test {}", s.name),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: mode,
        ..ConnectionConfig::default()
    }
}

fn target<'a>(c: &'a ConnectionConfig, password: Option<&'a str>) -> Target<'a> {
    Target { config: c, password, tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None }
}

async fn open(c: &ConnectionConfig, db: &TestDb) -> PgConnection {
    match connect(target(c, Some(&db.password))).await {
        Ok(conn) => conn,
        Err(e) => panic!("connect to {}:{} ({:?}): {e} [{}]", c.host, c.port, c.ssl_mode, e.code()),
    }
}

/// The first cell of the first row.
async fn scalar(conn: &PgConnection, sql: &str) -> Option<String> {
    let r = query::run_untyped(conn, sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    r.rows.first().and_then(|row| row.first()).cloned().flatten()
}

fn code(r: Result<PgConnection, DbError>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

/// The CA that did not sign the servers' certificate: the server certificate itself, which is
/// not self-signed, so no chain ends in it.
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

/// The server as the sshd sees it: `<service>:5432`.
fn tunnelled(s: &Server, db: &TestDb, mode: SslMode) -> ConnectionConfig {
    ConnectionConfig { host: s.name.clone(), port: s.service_port, ssh: Some(ssh_config(db)), ..config(s, mode) }
}

#[tokio::test]
async fn connects_and_reports_version() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = open(&config(s, SslMode::Prefer), db).await;
        let info = c.info();
        assert_eq!(info.version_num / 10000, s.major(), "{info:?}");
        assert!(info.version.starts_with(&s.major().to_string()), "{info:?}");
        assert!(!info.read_only && info.warnings.is_empty(), "{info:?}");
        assert!(!c.is_closed());
        assert_eq!(scalar(&c, "SHOW application_name").await.as_deref(), Some("dbee"));
        assert_eq!(scalar(&c, "SELECT current_database()").await.as_deref(), Some("dbee"));
    }
}

#[tokio::test]
async fn wrong_password_is_auth_error() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = config(s, SslMode::Prefer);
        let e = connect(target(&c, Some("nope"))).await.err().unwrap();
        assert_eq!(e.code(), "auth", "{e:?}");
        assert!(matches!(&e, DbError::Auth { message } if !message.is_empty()), "{e:?}");
        assert!(!e.to_string().contains("nope"), "{e}");
        // No password at all, when the server asks for one, is an auth error too.
        assert_eq!(code(connect(target(&c, None)).await), "auth");
    }
}

#[tokio::test]
async fn read_only_refuses_writes() {
    let db = require_testdb!();
    for s in postgres(db) {
        let ro = ConnectionConfig { read_only: true, ..config(s, SslMode::Prefer) };
        let c = open(&ro, db).await;
        assert!(c.info().read_only);
        assert_eq!(scalar(&c, "SHOW default_transaction_read_only").await.as_deref(), Some("on"));
        let name = unique("t_ro");
        let e = query::run_untyped(&c, &format!("CREATE TABLE scratch.{name} (a int)"), QUERY_LIMITS).await.unwrap_err();
        // Safety net: if the table was created after all, drop it.
        let rw = open(&config(s, SslMode::Prefer), db).await;
        query::run_untyped(&rw, &format!("DROP TABLE IF EXISTS scratch.{name}"), QUERY_LIMITS).await.unwrap();
        assert_eq!(e.code(), "readOnly", "{e:?}");
        assert!(matches!(&e, DbError::ReadOnly { message } if message.contains("read-only")), "{e:?}");
        // Reading still works, on the same connection.
        assert_eq!(scalar(&c, "SELECT 1 + 1").await.as_deref(), Some("2"));
    }
}

#[tokio::test]
async fn standard_conforming_strings_on() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = open(&config(s, SslMode::Prefer), db).await;
        // Quoting (dbee-sql) is safe only with both of these.
        assert_eq!(scalar(&c, "SHOW standard_conforming_strings").await.as_deref(), Some("on"));
        assert_eq!(scalar(&c, "SHOW client_encoding").await.as_deref(), Some("UTF8"));
        // Set by dbee on this session, not just the server default.
        let source = "SELECT source FROM pg_settings WHERE name = 'standard_conforming_strings'";
        assert_eq!(scalar(&c, source).await.as_deref(), Some("session"));
        assert_eq!(scalar(&c, "SELECT 'a\\b'").await.as_deref(), Some("a\\b"));
    }
}

#[tokio::test]
async fn through_ssh_tunnel() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    for s in postgres(db) {
        let c = tunnelled(s, db, SslMode::Prefer);
        let conn = connect(Target {
            config: &c,
            password: Some(&db.password),
            tunnel: Some(Arc::clone(&t)),
            purpose: Purpose::Browse,
            create_file: false,
            allowed_dirs: None,
        })
        .await
        .unwrap();
        assert_eq!(conn.info().version_num / 10000, s.major());
        assert_eq!(scalar(&conn, "SELECT 40 + 2").await.as_deref(), Some("42"));
        // TLS through the tunnel checks the configured host name (the service name).
        let full = ConnectionConfig { ssl_mode: SslMode::VerifyFull, ca_file: Some(db.ca_file.clone()), ..c.clone() };
        let conn = connect(Target {
            config: &full,
            password: Some(&db.password),
            tunnel: Some(Arc::clone(&t)),
            purpose: Purpose::Browse,
            create_file: false,
            allowed_dirs: None,
        })
        .await
        .unwrap();
        assert_eq!(scalar(&conn, "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()").await.as_deref(), Some("t"));
        // An SSH connection without a tunnel is refused rather than tried directly.
        assert_eq!(
            code(
                connect(Target {
                    config: &c,
                    password: Some(&db.password),
                    tunnel: None,
                    purpose: Purpose::Browse,
                    create_file: false,
                    allowed_dirs: None
                })
                .await
            ),
            "connect"
        );
    }
    t.close().await;
}

#[tokio::test]
async fn tls_require_encrypts() {
    let db = require_testdb!();
    let ssl = "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()";
    for s in postgres(db) {
        for (mode, expected) in [(SslMode::Require, "t"), (SslMode::Prefer, "t"), (SslMode::Disable, "f")] {
            let c = open(&config(s, mode), db).await;
            assert_eq!(scalar(&c, ssl).await.as_deref(), Some(expected), "{mode:?}");
        }
        // With a CA file, require checks the chain (libpq's rule), not the host name.
        let ca = ConnectionConfig { host: "localhost".into(), ca_file: Some(db.ca_file.clone()), ..config(s, SslMode::Require) };
        assert_eq!(scalar(&open(&ca, db).await, ssl).await.as_deref(), Some("t"));
        let wrong = ConnectionConfig { ca_file: Some(wrong_ca(db)), ..ca.clone() };
        assert_eq!(code(connect(target(&wrong, Some(&db.password))).await), "tls");
    }
}

#[tokio::test]
async fn tls_verify_full_needs_trusted_ca() {
    let db = require_testdb!();
    for s in postgres(db) {
        let local = ConnectionConfig { host: "localhost".into(), ..config(s, SslMode::VerifyFull) };
        let with_ca = ConnectionConfig { ca_file: Some(db.ca_file.clone()), ..local.clone() };
        let c = open(&with_ca, db).await;
        assert_eq!(scalar(&c, "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()").await.as_deref(), Some("t"));
        // The certificate names 127.0.0.1 too.
        open(&ConnectionConfig { host: "127.0.0.1".into(), ..with_ca.clone() }, db).await;
        // The OS does not trust the test CA.
        let e = connect(target(&local, Some(&db.password))).await.err().unwrap();
        assert_eq!(e.code(), "tls", "{e:?}");
        assert!(matches!(&e, DbError::Tls(m) if !m.is_empty()), "{e:?}");
        // A CA that did not sign the certificate, a missing CA file.
        assert_eq!(code(connect(target(&ConnectionConfig { ca_file: Some(wrong_ca(db)), ..local.clone() }, Some(&db.password))).await), "tls");
        let missing = db.ca_file.with_file_name("no-such-ca.crt");
        assert_eq!(code(connect(target(&ConnectionConfig { ca_file: Some(missing), ..local.clone() }, Some(&db.password))).await), "tls");
        // verify-ca checks the chain only.
        open(&ConnectionConfig { ssl_mode: SslMode::VerifyCa, ..with_ca.clone() }, db).await;
        let ca_wrong = ConnectionConfig { ssl_mode: SslMode::VerifyCa, ca_file: Some(wrong_ca(db)), ..local.clone() };
        assert_eq!(code(connect(target(&ca_wrong, Some(&db.password))).await), "tls");
        // verify-ca without a CA file is refused before connecting (libpq refuses it with the system roots).
        let e = connect(target(&ConnectionConfig { ssl_mode: SslMode::VerifyCa, ..local.clone() }, Some(&db.password))).await.err().unwrap();
        assert_eq!(e.code(), "tls", "{e:?}");
        assert!(e.to_string().contains("verify-ca needs a CA file"), "{e}");
    }
}

/// Waits until backend `pid` sleeps in `pg_sleep()`, as `watcher`, a connection to the same
/// server, sees in `pg_stat_activity`: the statement runs.
async fn until_sleeping(watcher: &PgConnection, pid: &str) {
    let sql = format!("SELECT count(*) FROM pg_stat_activity WHERE pid = {pid} AND wait_event = 'PgSleep'");
    let deadline = Instant::now() + Duration::from_secs(20);
    while scalar(watcher, &sql).await.as_deref() != Some("1") {
        assert!(Instant::now() < deadline, "backend {pid} never got to its sleep");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Starts `SELECT pg_sleep(30)`, cancels it once `watcher` sees it sleep, and expects
/// `cancelled`; then the connection still works.
async fn cancel_sleep(conn: Arc<PgConnection>, watcher: &PgConnection) {
    let canceller: Canceller = conn.canceller();
    let pid = scalar(&conn, "SELECT pg_backend_pid()").await.unwrap();
    let runner = Arc::clone(&conn);
    let job = tokio::spawn(async move { query::run_untyped(&runner, "SELECT pg_sleep(30)", QUERY_LIMITS).await });
    until_sleeping(watcher, &pid).await;
    canceller.cancel().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not cancelled").unwrap();
    let e = result.unwrap_err();
    assert_eq!(e.code(), "cancelled", "{e:?}");
    assert_eq!(e, DbError::Cancelled);
    assert!(!conn.is_closed());
    assert_eq!(scalar(&conn, "SELECT 'still here'").await.as_deref(), Some("still here"));
}

/// A cancel request goes to the address the connection reached, not to whichever its host name
/// gives first when it is sent. Here `localhost` names `::1` and `127.0.0.1`, and the server
/// listens on `127.0.0.1` only: a stand-in on each other address, as another server behind the
/// name would be, must get no cancel request. (Without TLS, so the request is the protocol's plain
/// CancelRequest; other tests' logins to `localhost` that reach a stand-in first are not counted.)
#[tokio::test]
async fn a_cancel_goes_to_the_address_the_connection_reached() {
    const CANCEL_REQUEST_CODE: [u8; 4] = 80877102_u32.to_be_bytes();
    let db = require_testdb!();
    for s in postgres(db) {
        let conn = Arc::new(open(&ConnectionConfig { host: "localhost".into(), ..config(s, SslMode::Disable) }, db).await);
        let server = std::net::SocketAddr::from(([127, 0, 0, 1], s.port));
        let hits = Arc::new(AtomicUsize::new(0));
        for addr in tokio::net::lookup_host(("localhost", s.port)).await.unwrap().filter(|a| *a != server) {
            let Ok(listener) = tokio::net::TcpListener::bind(addr).await else { continue };
            let hits = Arc::clone(&hits);
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let mut head = [0; 8];
                    if socket.read_exact(&mut head).await.is_ok() && head[4..] == CANCEL_REQUEST_CODE {
                        hits.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
        let watcher = open(&config(s, SslMode::Prefer), db).await;
        cancel_sleep(conn, &watcher).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "pg{}: a cancel request went to another address", s.major());
    }
}

#[tokio::test]
async fn cancel_stops_pg_sleep() {
    let db = require_testdb!();
    for s in postgres(db) {
        let watcher = open(&config(s, SslMode::Prefer), db).await;
        for mode in [SslMode::Prefer, SslMode::Disable] {
            cancel_sleep(Arc::new(open(&config(s, mode), db).await), &watcher).await;
        }
    }
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let t = tunnel(db, &home).await;
    for s in postgres(db) {
        let c = tunnelled(s, db, SslMode::Prefer);
        let conn = connect(Target {
            config: &c,
            password: Some(&db.password),
            tunnel: Some(Arc::clone(&t)),
            purpose: Purpose::Browse,
            create_file: false,
            allowed_dirs: None,
        })
        .await
        .unwrap();
        cancel_sleep(Arc::new(conn), &open(&config(s, SslMode::Prefer), db).await).await;
    }
    t.close().await;
}

#[tokio::test]
async fn backend_killed_is_connection_lost() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = open(&config(s, SslMode::Prefer), db).await;
        let pid = scalar(&c, "SELECT pg_backend_pid()").await.unwrap();
        let other = open(&config(s, SslMode::Prefer), db).await;
        assert_eq!(scalar(&other, &format!("SELECT pg_terminate_backend({pid})")).await.as_deref(), Some("t"));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = None;
        while Instant::now() < deadline {
            match query::run_untyped(&c, "SELECT 1", QUERY_LIMITS).await {
                Err(e) if e.code() == "connectionLost" => {
                    last = Some(e);
                    break;
                }
                other => last = Some(other.err().unwrap_or(DbError::Other("still answering".into()))),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(last, Some(DbError::ConnectionLost));
        assert!(c.is_closed());
    }
}

#[tokio::test]
async fn run_untyped_keeps_the_last_result_and_server_errors() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = open(&config(s, SslMode::Prefer), db).await;
        let r = query::run_untyped(&c, "SELECT 1 AS a; SELECT 2 AS b, NULL AS c, '' AS d", QUERY_LIMITS).await.unwrap();
        assert_eq!(r.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["b", "c", "d"]);
        assert!(r.columns.iter().all(|c| c.base_type.is_empty() && c.type_label.is_empty()));
        assert_eq!(r.rows, vec![vec![Some("2".into()), None, Some(String::new())]]);
        assert_eq!((r.rows_seen, r.complete), (1, true));
        // Exactly as Postgres prints it.
        assert_eq!(scalar(&c, "SELECT 1.50::numeric(10,2)").await.as_deref(), Some("1.50"));

        let e = query::run_untyped(&c, "SELECT nope FROM pg_class", QUERY_LIMITS).await.unwrap_err();
        assert_eq!(e.code(), "server");
        match e {
            DbError::Server(s) => {
                assert_eq!((s.code.as_str(), s.position), ("42703", Some(8)));
                assert!(s.message.contains("nope"), "{}", s.message);
            }
            other => panic!("{other:?}"),
        }
        let e = query::run_untyped(&c, "SELECT 1/0", QUERY_LIMITS).await.unwrap_err();
        assert!(matches!(&e, DbError::Server(s) if s.code == "22012"), "{e:?}");
        // The connection survives errors.
        assert_eq!(scalar(&c, "SELECT 3").await.as_deref(), Some("3"));
    }
}

#[tokio::test]
async fn unreachable_server_is_connect_error() {
    let db = require_testdb!();
    unreachable(db).await;
}

#[tokio::test]
#[ignore = "timing; run with --ignored --nocapture"]
async fn unreachable_server_is_a_connect_error_within_ten_seconds() {
    let db = require_testdb!();
    let took = unreachable(db).await;
    println!("pg: two unreachable servers refused in {took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

/// A port nobody listens on and a server that hangs up at once are `connect` errors; how long the
/// two took. Settings that cannot work are one too.
async fn unreachable(db: &TestDb) -> Duration {
    // A port nobody listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let s = Server { name: "none".into(), kind: Kind::Postgres, version: "0".into(), port, service_port: 5432, full_seed: false };
    let started = Instant::now();
    assert_eq!(code(connect(target(&config(&s, SslMode::Prefer), Some(&db.password))).await), "connect");
    // A server that hangs up at once.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s = Server { port: listener.local_addr().unwrap().port(), ..s };
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            drop(sock);
        }
    });
    assert_eq!(code(connect(target(&config(&s, SslMode::Disable), Some(&db.password))).await), "connect");
    let took = started.elapsed();
    // Settings that cannot work.
    let bad = ConnectionConfig { host: String::new(), ..config(postgres(db).next().unwrap(), SslMode::Prefer) };
    assert_eq!(code(connect(target(&bad, Some(&db.password))).await), "connect");
    took
}

#[test]
fn old_versions_warn() {
    assert_eq!(MIN_VERSION_NUM, 140000);
    assert!(version_warning(130012).unwrap().contains("14"));
    assert!(version_warning(130012).unwrap().contains("13"));
    assert!(version_warning(90624).unwrap().contains("9.6"));
    assert_eq!(version_warning(140000), None);
    assert_eq!(version_warning(180001), None);
}

#[test]
fn connection_types_are_shareable() {
    fn send_sync<T: Send + Sync + 'static>() {}
    send_sync::<PgConnection>();
    send_sync::<Canceller>();
    send_sync::<DbError>();
    send_sync::<PgDriver>();
}

#[tokio::test]
async fn pg_driver_connects_through_the_trait() {
    let db = require_testdb!();
    assert_eq!(PgDriver.kinds(), [DbKind::Postgres]);
    for s in postgres(db) {
        let c = config(s, SslMode::Prefer);
        let conn = PgDriver.connect(target(&c, Some(&db.password))).await.unwrap();
        assert_eq!(conn.info().product, "PostgreSQL");
        assert_eq!(conn.dialect(), Dialect::Postgres);
        assert_eq!(conn.info().version_num / 10000, s.major());
        assert!(!conn.is_closed());
        // Each method reaches the Postgres connection.
        assert_eq!(conn.scalar_u64("SELECT 40 + 2").await, Ok(42));
        let typed = conn.run("SELECT 1::int4 AS n", QUERY_LIMITS).await.unwrap();
        assert_eq!((typed.columns[0].base_type.as_str(), typed.rows[0][0].as_deref()), ("int4", Some("1")));
        assert!(conn.run_untyped("SELECT 1 AS n", QUERY_LIMITS).await.unwrap().columns[0].base_type.is_empty());
        let catalog = conn.load_catalog().await.unwrap();
        assert!(catalog.table(&TableRef { schema: "shop".into(), name: "orders".into() }).is_some());
        assert_eq!(conn.apply(&[], &ApplyGate::default()).await.map_err(|e| e.to_string()), Ok(0));
        let e = PgDriver.connect(target(&c, Some("nope"))).await.err().unwrap();
        assert_eq!(e.code(), "auth", "{e:?}");
    }
}

#[tokio::test]
async fn the_trait_canceller_stops_pg_sleep() {
    let db = require_testdb!();
    for s in postgres(db) {
        let conn: Arc<dyn Connection> = PgDriver.connect(target(&config(s, SslMode::Prefer), Some(&db.password))).await.unwrap();
        let canceller = conn.canceller();
        let pid = conn.scalar_u64("SELECT pg_backend_pid()").await.unwrap().to_string();
        let runner = Arc::clone(&conn);
        let job = tokio::spawn(async move { runner.run_untyped("SELECT pg_sleep(30)", QUERY_LIMITS).await });
        until_sleeping(&open(&config(s, SslMode::Prefer), db).await, &pid).await;
        canceller.cancel().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not cancelled").unwrap();
        assert_eq!(result.err(), Some(DbError::Cancelled));
        assert_eq!(conn.scalar_u64("SELECT 1").await, Ok(1));
    }
}

#[tokio::test]
async fn failed_transaction_is_flagged() {
    let db = require_testdb!();
    for s in postgres(db) {
        let conn = PgDriver.connect(target(&config(s, SslMode::Prefer), Some(&db.password))).await.unwrap();
        conn.run("BEGIN", QUERY_LIMITS).await.unwrap();
        let first = conn.run("SELECT 1/0", QUERY_LIMITS).await.unwrap_err();
        assert!(matches!(&first, DbError::Server(s) if s.code == "22012" && s.meaning.is_none()), "{first:?}");
        let e = conn.run("SELECT 1", QUERY_LIMITS).await.unwrap_err();
        assert!(
            matches!(&e, DbError::Server(s) if s.code == "25P02" && s.code_label == Some("SQLSTATE") && s.meaning == Some(Meaning::TransactionAborted)),
            "{e:?}"
        );
        assert!(e.to_string().ends_with("(SQLSTATE 25P02)"), "{e}");
        conn.run("ROLLBACK", QUERY_LIMITS).await.unwrap();
        assert_eq!(conn.scalar_u64("SELECT 7").await, Ok(7));
    }
}

/// The server's databases, read through the connection the driver opens for the list.
async fn listed(c: &ConnectionConfig, password: &str) -> Result<Vec<String>, DbError> {
    let conn = PgDriver.list_connection(target(c, Some(password))).await?.expect("Postgres has a database list");
    conn.list_databases().await
}

/// The databases a connection may open, read through the server's default database (the one
/// named like the user, `postgres` here) whatever the settings name: nothing, a database, half a
/// name, one that is not there.
#[tokio::test]
async fn pg_lists_databases() {
    let db = require_testdb!();
    for s in postgres(db) {
        for database in ["", "dbee", "sh", "no_such_database"] {
            let c = ConnectionConfig { database: database.into(), ..config(s, SslMode::Prefer) };
            let names = listed(&c, &db.password).await.unwrap_or_else(|e| panic!("{} {database:?}: {e}", s.name));
            assert!(names.contains(&"dbee".to_string()) && names.contains(&"postgres".to_string()), "{} {database:?}: {names:?}", s.name);
            assert!(!names.iter().any(|n| n.starts_with("template")), "{}: {names:?}", s.name);
            assert!(names.is_sorted(), "{}: {names:?}", s.name);
        }
        assert_eq!(listed(&config(s, SslMode::Prefer), "nope").await.unwrap_err().code(), "auth", "{}", s.name);
    }
}

/// Drops a role made for a test when the test ends, also when it fails.
struct Role<'a>(&'a Server, String);

impl Drop for Role<'_> {
    fn drop(&mut self) {
        let _ = dbee_testkit::psql(self.0, &format!("DROP ROLE IF EXISTS {}", self.1));
    }
}

/// A user without a database of its own name lists through `postgres`. The listing connection is
/// gone once it is dropped.
#[tokio::test]
async fn pg_lists_through_postgres_when_the_default_database_is_missing() {
    let db = require_testdb!();
    const PASSWORD: &str = "dbee-list-pw";
    for s in postgres(db) {
        let role = Role(s, unique("dbee_list"));
        dbee_testkit::psql(s, &format!("CREATE ROLE {} LOGIN PASSWORD '{PASSWORD}'", role.1)).unwrap();
        let c = ConnectionConfig { user: role.1.clone(), database: String::new(), ..config(s, SslMode::Prefer) };
        // Its own database is not there, so a connect without a database fails.
        let e = connect(target(&c, Some(PASSWORD))).await.err().unwrap();
        assert!(matches!(&e, DbError::Server(e) if e.code == "3D000"), "{}: {e:?}", s.name);
        let conn = PgDriver.list_connection(target(&c, Some(PASSWORD))).await.unwrap().unwrap();
        let current = conn.run_untyped("SELECT current_database()", QUERY_LIMITS).await.unwrap();
        assert_eq!(current.rows[0][0].as_deref(), Some("postgres"), "{}", s.name);
        let names = conn.list_databases().await.unwrap();
        assert!(names.contains(&"dbee".to_string()) && names.contains(&"postgres".to_string()), "{}: {names:?}", s.name);
        drop(conn);
        let sessions = format!("SELECT count(*) FROM pg_stat_activity WHERE usename = '{}'", role.1);
        let deadline = Instant::now() + Duration::from_secs(10);
        while dbee_testkit::psql(s, &sessions).unwrap().trim() != "0" {
            assert!(Instant::now() < deadline, "{}: the listing connection is still open", s.name);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

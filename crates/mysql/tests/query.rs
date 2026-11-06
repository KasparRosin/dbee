//! Statement runner tests against every MySQL and MariaDB server in `db.servers` (`cargo xtask
//! testdb up`). Tests that write use a `scratch` table or procedure of their own and drop it.

use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_core::{ResultSet, TypeClass};
use dbee_driver::{DbError, Limits, PAGE_LIMITS, Purpose, QUERY_LIMITS, Target};
use dbee_mysql::{MySqlConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

/// The MySQL and MariaDB test servers.
fn mysql_servers(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::MySql, Kind::MariaDb])
}

fn config(s: &Server) -> ConnectionConfig {
    ConnectionConfig {
        name: format!("test {}", s.name),
        kind: DbKind::from_id(s.kind.id()).unwrap(),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "root".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

async fn open_for(s: &Server, db: &TestDb, purpose: Purpose) -> MySqlConnection {
    let c = config(s);
    match connect(Target { config: &c, password: Some(&db.password), tunnel: None, purpose, create_file: false, allowed_dirs: None }).await {
        Ok(conn) => conn,
        Err(e) => panic!("connect to {} failed: {e}", s.name),
    }
}

async fn open(s: &Server, db: &TestDb) -> MySqlConnection {
    open_for(s, db, Purpose::Browse).await
}

async fn ok(c: &MySqlConnection, sql: &str, limits: Limits) -> ResultSet {
    c.run(sql, limits).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn names(r: &ResultSet) -> Vec<&str> {
    r.columns.iter().map(|c| c.name.as_str()).collect()
}

#[tokio::test]
async fn text_exactly_as_mysql_prints() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let sql = "SELECT CAST(1.50 AS DECIMAL(10,2)) n, TIMESTAMP '2024-01-02 03:04:05' t, NULL z, '' e, CAST(X'00FF' AS BINARY) b";
        let r = ok(&c, sql, QUERY_LIMITS).await;
        assert_eq!(r.rows[0], [Some("1.50".into()), Some("2024-01-02 03:04:05".into()), None, Some("".into()), Some("0x00FF".into())], "{}", s.name);
        assert_eq!(names(&r), ["n", "t", "z", "e", "b"], "{}", s.name);
        let classes: Vec<TypeClass> = r.columns.iter().map(|c| c.class).collect();
        assert_eq!(classes, [TypeClass::Number, TypeClass::DateTime, TypeClass::Other, TypeClass::Text, TypeClass::Binary], "{}", s.name);
        assert!(r.complete && r.rows_seen == 1 && r.rows_affected.is_none(), "{}: {r:?}", s.name);
        // Untyped, the values are the same; an empty binary value is `0x`.
        let u = c.run_untyped("SELECT CAST(X'00FF' AS BINARY) b, CAST('' AS BINARY) e, 'x' x", QUERY_LIMITS).await.unwrap();
        assert_eq!(u.rows[0], [Some("0x00FF".into()), Some("0x".into()), Some("x".into())], "{}", s.name);
        assert!(u.columns.iter().all(|c| c.base_type.is_empty() && c.class == TypeClass::Other), "{}: {:?}", s.name, u.columns);
    }
}

#[tokio::test]
async fn text_that_is_not_utf8_is_replaced_and_counted() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open_for(s, db, Purpose::Query).await;
        // The query tab may change the results' character set: é comes as latin1's one byte.
        ok(&c, "SET character_set_results = latin1", QUERY_LIMITS).await;
        let r = ok(&c, "SELECT 'é' AS e", QUERY_LIMITS).await;
        assert_eq!((r.rows[0][0].as_deref(), r.columns[0].class), (Some("\u{FFFD}"), TypeClass::Text), "{}", s.name);
        // The replacement's 3 bytes count: with the name's 1 byte, 4 fit and 3 do not.
        let fits = ok(&c, "SELECT 'é' AS e", Limits { max_rows: 10, max_bytes: 4 }).await;
        let cut = ok(&c, "SELECT 'é' AS e", Limits { max_rows: 10, max_bytes: 3 }).await;
        assert_eq!((fits.rows.len(), fits.complete, cut.rows.len(), cut.complete, cut.rows_seen), (1, true, 0, false, 1), "{}", s.name);
    }
}

#[tokio::test]
async fn mariadbs_own_types_are_other() {
    let db = require_testdb!();
    for s in mysql_servers(db).filter(|s| s.kind == Kind::MariaDb) {
        let c = open(s, db).await;
        let t = unique("t_own_types");
        ok(&c, &format!("CREATE TABLE scratch.`{t}` (u UUID, i6 INET6, i4 INET4, pt POINT)"), QUERY_LIMITS).await;
        let insert = format!("INSERT INTO scratch.`{t}` VALUES ('123e4567-e89b-12d3-a456-426614174000', '::1', '10.0.0.1', POINT(1, 2))");
        let read = match c.run(&insert, QUERY_LIMITS).await {
            Ok(_) => c.run(&format!("SELECT * FROM scratch.`{t}`"), QUERY_LIMITS).await,
            Err(e) => Err(e),
        };
        ok(&c, &format!("DROP TABLE scratch.`{t}`"), QUERY_LIMITS).await;
        let r = read.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        // MariaDB sends them as text strings and names the type in its extended metadata.
        let got: Vec<(&str, &str, TypeClass)> = r.columns.iter().map(|c| (c.base_type.as_str(), c.type_label.as_str(), c.class)).collect();
        let expected = [
            ("uuid", "uuid", TypeClass::Other),
            ("inet6", "inet6", TypeClass::Other),
            ("inet4", "inet4", TypeClass::Other),
            ("geometry", "geometry", TypeClass::Binary),
        ];
        assert_eq!(got, expected, "{}", s.name);
        assert_eq!(r.rows[0][..3], [Some("123e4567-e89b-12d3-a456-426614174000".into()), Some("::1".into()), Some("10.0.0.1".into())]);
    }
}

#[tokio::test]
async fn classes_of_described_columns() {
    let db = require_testdb!();
    use TypeClass::*;
    let expected = [
        ("id", Number),
        ("note", LongText),
        ("st", Text),
        ("doc", Json),
        ("flag", Bool),
        ("tags", Text),
        ("iv", DateTime),
        ("y", Number),
        ("m", Number),
        ("n", Number),
        ("f", Number),
        ("big", Number),
        ("d", DateTime),
        ("ts", DateTime),
        ("b", Binary),
        ("bits", Binary),
        ("u", Text),
        ("ip", Text),
        ("pt", Binary),
        ("total", Number),
    ];
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let r = ok(&c, "SELECT * FROM shop.all_types ORDER BY id", QUERY_LIMITS).await;
        let got: Vec<(&str, TypeClass)> = r.columns.iter().map(|c| (c.name.as_str(), c.class)).collect();
        assert_eq!(got, expected, "{}", s.name);
        let types: Vec<(&str, &str)> = r.columns.iter().map(|c| (c.base_type.as_str(), c.type_label.as_str())).collect();
        assert_eq!(
            types[..6],
            [("int", "int"), ("text", "text"), ("enum", "enum"), ("json", "json"), ("tinyint", "tinyint"), ("set", "set")],
            "{}",
            s.name
        );
        assert_eq!(
            types[14..19],
            [("blob", "blob"), ("bit", "bit"), ("char", "char(36)"), ("varchar", "varchar(45)"), ("geometry", "geometry")],
            "{}",
            s.name
        );
        // Binary values as hex; the bool as the server prints it.
        let first = &r.rows[0];
        assert_eq!((first[4].as_deref(), first[14].as_deref(), first[15].as_deref()), (Some("1"), Some("0xDEADBEEF"), Some("0xA5")), "{}", s.name);
        assert_eq!(r.rows[1][14].as_deref(), Some("0x"), "{}", s.name);
        assert!(r.rows[2].iter().skip(1).take(18).all(Option::is_none), "{}: {:?}", s.name, r.rows[2]);

        // What tells TINYINT(1) from the other tinyints, and bytes from text, in a table's columns.
        let t = format!("scratch.{}", unique("t_types"));
        let create = format!(
            "CREATE TABLE {t} (a TINYINT, b TINYINT(1), c BOOL, d TINYINT UNSIGNED, e TINYINT(4), f BINARY(16), g VARBINARY(10), \
             h TINYTEXT, i MEDIUMTEXT, j LONGTEXT, k TINYBLOB, l MEDIUMBLOB, m LONGBLOB, n CHAR(3), o BIT(1), p VARCHAR(20) COLLATE utf8mb4_bin)"
        );
        ok(&c, &create, QUERY_LIMITS).await;
        let r = c.run(&format!("SELECT * FROM {t}"), QUERY_LIMITS).await;
        let _ = c.run(&format!("DROP TABLE IF EXISTS {t}"), QUERY_LIMITS).await;
        let r = r.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let got: Vec<(&str, &str, TypeClass)> = r.columns.iter().map(|c| (c.base_type.as_str(), c.type_label.as_str(), c.class)).collect();
        assert_eq!(
            got,
            [
                ("tinyint", "tinyint", Number),
                ("tinyint", "tinyint", Bool),
                ("tinyint", "tinyint", Bool),
                ("tinyint", "tinyint", Number),
                ("tinyint", "tinyint", Number),
                ("binary", "binary(16)", Binary),
                ("varbinary", "varbinary(10)", Binary),
                ("tinytext", "tinytext", LongText),
                ("mediumtext", "mediumtext", LongText),
                ("longtext", "longtext", LongText),
                ("tinyblob", "tinyblob", Binary),
                ("mediumblob", "mediumblob", Binary),
                ("longblob", "longblob", Binary),
                ("char", "char(3)", Text),
                ("bit", "bit", Binary),
                ("varchar", "varchar(20)", Text),
            ],
            "{}",
            s.name
        );
    }
}

#[tokio::test]
async fn a_mariadb_json_column_is_json() {
    let db = require_testdb!();
    let mut seen = 0;
    for s in mysql_servers(db).filter(|s| s.kind == Kind::MariaDb) {
        // MariaDB's JSON is a LONGTEXT with a check; its extended metadata says it holds JSON.
        let c = open(s, db).await;
        let r = ok(&c, "SELECT doc, JSON_OBJECT('a', 1) j, note FROM shop.all_types ORDER BY id", QUERY_LIMITS).await;
        // JSON_OBJECT(..) comes as a VARCHAR whose format is json: no length in its label.
        let got: Vec<(&str, &str, &str, TypeClass)> =
            r.columns.iter().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_label.as_str(), c.class)).collect();
        assert_eq!(
            got,
            [("doc", "json", "json", TypeClass::Json), ("j", "json", "json", TypeClass::Json), ("note", "text", "text", TypeClass::LongText)],
            "{}",
            s.name
        );
        assert_eq!(r.rows[0][0].as_deref(), Some(r#"{"a": {"b": [1, 2, 3]}, "s": "text"}"#), "{}", s.name);
        assert_eq!(r.rows[2][0], None, "{}", s.name);
        seen += 1;
    }
    assert_eq!(seen, 2, "mariadb1011 and mariadbnew");
}

#[tokio::test]
async fn row_cap_keeps_first_and_counts_all() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let r = ok(&c, "SELECT id FROM shop.big_rows ORDER BY id LIMIT 25000", QUERY_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (10_000, 25_000, false), "{}", s.name);
        assert_eq!(r.rows[0], [Some("1".to_owned())], "{}", s.name);
        assert_eq!(r.rows[9_999], [Some("10000".to_owned())], "{}", s.name);
        // The connection stayed in step.
        assert_eq!(c.scalar_u64("SELECT 7").await.unwrap(), 7, "{}", s.name);
    }
}

#[tokio::test]
async fn byte_cap_stops_collecting() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let r = ok(&c, "SELECT REPEAT('x', 100000) x FROM shop.big_rows LIMIT 100", Limits { max_rows: 1000, max_bytes: 1 << 20 }).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (10, 100, false), "{}", s.name);
        // A binary value counts as its text: 1000 bytes are 2002 characters of hex.
        let sql = "SELECT CAST(REPEAT('a', 1000) AS BINARY) b FROM shop.big_rows LIMIT 10";
        let r = ok(&c, sql, Limits { max_rows: 1000, max_bytes: 1 + 3 * 2002 }).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (3, 10, false), "{}", s.name);
        assert_eq!(r.rows[0][0].as_ref().map(String::len), Some(2002), "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT 1").await.unwrap(), 1, "{}", s.name);
    }
}

#[tokio::test]
async fn page_limit_of_101_rows_is_exact() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let r = ok(&c, "SELECT id FROM shop.big_rows ORDER BY id LIMIT 101", PAGE_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 101, true), "{}", s.name);
        let r = ok(&c, "SELECT id FROM shop.big_rows ORDER BY id LIMIT 102", PAGE_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 102, false), "{}", s.name);
    }
}

#[tokio::test]
async fn twenty_megabyte_cell_loads() {
    let db = require_testdb!();
    let mut seen = 0;
    for s in mysql_servers(db).filter(|s| s.full_seed) {
        let c = open(s, db).await;
        let r = ok(&c, "SELECT id, t, j FROM shop.big_cell", QUERY_LIMITS).await;
        assert!(r.complete && r.rows.len() == 1, "{}", s.name);
        let t = r.rows[0][1].as_deref().unwrap_or_default();
        assert_eq!(t.len(), 20_000_000, "{}", s.name);
        assert!(t.starts_with("0123456789abcdefghij") && t.ends_with("XYZ-_"), "{}", s.name);
        let j = r.rows[0][2].as_deref().unwrap_or_default();
        let length = c.scalar_u64("SELECT LENGTH(j) FROM shop.big_cell").await.unwrap();
        assert_eq!(j.len() as u64, length, "{}", s.name);
        seen += 1;
    }
    assert_eq!(seen, 2, "mysql80 and mariadbnew hold the big cell");
}

#[tokio::test]
#[ignore = "timing; run with --ignored --nocapture"]
async fn twenty_megabyte_cell_loads_within_twenty_seconds() {
    let db = require_testdb!();
    for s in mysql_servers(db).filter(|s| s.full_seed) {
        let c = open(s, db).await;
        let started = Instant::now();
        let r = ok(&c, "SELECT id, t, j FROM shop.big_cell", QUERY_LIMITS).await;
        assert_eq!(r.rows.len(), 1, "{}", s.name);
        println!("{}: a 20 MB cell read in {:?}", s.name, started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(20), "{}: {:?}", s.name, started.elapsed());
    }
}

#[tokio::test]
async fn several_result_sets_keep_the_last() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let p = format!("scratch.{}", unique("two_selects"));
        ok(&c, &format!("CREATE PROCEDURE {p}() BEGIN SELECT 1 AS a; SELECT 2 AS b, 'x' AS c UNION ALL SELECT 3, 'y'; END"), QUERY_LIMITS).await;
        let r = c.run(&format!("CALL {p}()"), QUERY_LIMITS).await;
        let _ = c.run(&format!("DROP PROCEDURE IF EXISTS {p}"), QUERY_LIMITS).await;
        let r = r.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        assert_eq!(names(&r), ["b", "c"], "{}", s.name);
        assert_eq!(r.rows, [[Some("2".to_owned()), Some("x".to_owned())], [Some("3".to_owned()), Some("y".to_owned())]], "{}", s.name);
        assert_eq!((r.rows_seen, r.complete, r.rows_affected), (2, true, Some(0)), "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT 5").await.unwrap(), 5, "{}: in step after the results", s.name);
    }
}

#[tokio::test]
async fn error_has_errno_and_flavor() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        match c.run("SELECT nope FROM shop.orders", QUERY_LIMITS).await {
            Err(DbError::Server(e)) => {
                let label = if s.kind == Kind::MariaDb { "MariaDB error" } else { "MySQL error" };
                assert_eq!((e.code.as_str(), e.code_label, e.detail.as_deref()), ("1054", Some(label), None), "{}", s.name);
                assert!(e.message.contains("nope"), "{}: {e:?}", s.name);
            }
            other => panic!("{}: {other:?}", s.name),
        }
        assert!(!c.is_closed(), "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT 2").await.unwrap(), 2, "{}: usable after an error", s.name);
    }
}

/// Waits until the server no longer lists connection `id`.
async fn wait_gone(other: &MySqlConnection, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let listed = format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id}");
    while other.scalar_u64(&listed).await.unwrap() != 0 {
        assert!(Instant::now() < deadline, "connection {id} still listed");
        tokio::time::sleep(Duration::from_millis(20)).await;
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

#[tokio::test]
async fn killed_connection_reports_connection_lost() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let id = c.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
        let other = open(s, db).await;
        ok(&other, &format!("KILL {id}"), QUERY_LIMITS).await;
        wait_gone(&other, id).await;
        assert_eq!(c.run("SELECT 1", QUERY_LIMITS).await.unwrap_err().code(), "connectionLost", "{}", s.name);
        assert!(c.is_closed(), "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT 1").await.unwrap_err(), DbError::ConnectionLost, "{}", s.name);

        // Killed while a statement runs: that statement reports it.
        let c = std::sync::Arc::new(open(s, db).await);
        let id = c.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
        let runner = std::sync::Arc::clone(&c);
        let job = tokio::spawn(async move { runner.run("SELECT SLEEP(30)", QUERY_LIMITS).await });
        until_sleeping(&other, id).await;
        ok(&other, &format!("KILL {id}"), QUERY_LIMITS).await;
        let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not ended").unwrap();
        assert_eq!(result.unwrap_err(), DbError::ConnectionLost, "{}", s.name);
        assert!(c.is_closed(), "{}", s.name);
    }
}

#[tokio::test]
async fn a_timed_out_connection_reports_connection_lost() {
    let db = require_testdb!();
    // MySQL 8 and newer say goodbye with an error packet (4031) before they close; the others
    // just close.
    let mut idle = Vec::new();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        ok(&c, "SET SESSION wait_timeout = 1", QUERY_LIMITS).await;
        idle.push((s, c));
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    for (s, c) in idle {
        assert_eq!(c.run("SELECT 1", QUERY_LIMITS).await.unwrap_err(), DbError::ConnectionLost, "{}", s.name);
        assert!(c.is_closed(), "{}", s.name);
    }
}

#[tokio::test]
async fn update_reports_rows_affected() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        let t = format!("scratch.{}", unique("t_query"));
        ok(&c, &format!("CREATE TABLE {t} (a INT)"), QUERY_LIMITS).await;
        ok(&c, &format!("INSERT INTO {t} VALUES (1), (2), (3)"), QUERY_LIMITS).await;
        let r = c.run(&format!("UPDATE {t} SET a = a + 1"), QUERY_LIMITS).await;
        let _ = c.run(&format!("DROP TABLE IF EXISTS {t}"), QUERY_LIMITS).await;
        let r = r.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        assert_eq!(r.rows_affected, Some(3), "{}", s.name);
        assert!(r.columns.is_empty() && r.rows.is_empty() && r.complete, "{}", s.name);
    }
}

#[tokio::test]
async fn scalar_u64_reads_a_count_and_rejects_the_rest() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = open(s, db).await;
        assert_eq!(c.scalar_u64("SELECT COUNT(*) FROM shop.orders").await.unwrap(), dbee_testkit::ORDERS_COUNT, "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT COUNT(*) FROM shop.orders WHERE 1 = 0").await.unwrap(), 0, "{}", s.name);
        for bad in ["SELECT 'abc'", "SELECT 1 FROM shop.orders WHERE 1 = 0", "SELECT -1", "SELECT NULL"] {
            assert_eq!(c.scalar_u64(bad).await.unwrap_err().code(), "other", "{}: {bad}", s.name);
        }
    }
}

#[tokio::test]
async fn a_cancelled_sleep_is_cancelled() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let c = std::sync::Arc::new(open(s, db).await);
        let canceller = c.canceller();
        let id = c.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
        let runner = std::sync::Arc::clone(&c);
        let job = tokio::spawn(async move { runner.run("SELECT SLEEP(30)", QUERY_LIMITS).await });
        until_sleeping(&open(s, db).await, id).await;
        canceller.cancel().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), job).await.expect("not cancelled").unwrap();
        assert_eq!(result.unwrap_err(), DbError::Cancelled, "{}", s.name);
        assert!(!c.is_closed(), "{}", s.name);
        assert_eq!(c.scalar_u64("SELECT 3").await.unwrap(), 3, "{}", s.name);
    }
}

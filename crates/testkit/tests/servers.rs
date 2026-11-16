//! The MySQL and MariaDB test servers, the SQLite seed and the list of targets over every engine.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dbee_testkit::{Kind, ORDERS_COUNT, TestDb, TestTarget, mysql, require_testdb, sqlite_seeded, targets, testdb};

/// Under `target/`: the system temp directory may be small (docs/testing.md §3).
fn tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit-servers")
}

/// Removes a seed copy when the test ends, also when it fails.
struct Copy(PathBuf);

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn open(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

fn count(c: &rusqlite::Connection, sql: &str) -> u64 {
    c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap().try_into().unwrap()
}

/// The MySQL and MariaDB servers.
fn mysql_servers(db: &TestDb) -> impl Iterator<Item = &dbee_testkit::Server> {
    db.servers_of(&[Kind::MySql, Kind::MariaDb])
}

#[test]
fn every_mysql_server_answers() {
    require_testdb!();
    let servers: Vec<_> = mysql_servers(testdb().unwrap()).collect();
    assert_eq!(servers.len(), 5);
    for s in servers {
        assert_eq!(mysql(s, "SELECT COUNT(*) FROM shop.orders").unwrap().trim(), ORDERS_COUNT.to_string(), "{}", s.name);
    }
}

#[test]
fn mysql_versions_are_the_supported_ones() {
    let db = require_testdb!();
    let names: Vec<_> = mysql_servers(db).map(|s| (s.name.as_str(), s.kind)).collect();
    assert_eq!(
        names,
        [
            ("mysql57", Kind::MySql),
            ("mysql80", Kind::MySql),
            ("mysqlnew", Kind::MySql),
            ("mariadb1011", Kind::MariaDb),
            ("mariadbnew", Kind::MariaDb)
        ]
    );
    let version = |name: &str| &db.server(name).unwrap().version;
    assert!(version("mysql57").starts_with("5.7."), "{}", version("mysql57"));
    assert!(version("mysql80").starts_with("8.0."), "{}", version("mysql80"));
    assert!(version("mariadb1011").starts_with("10.11."), "{}", version("mariadb1011"));
    let major = |v: &str| v.split('.').next().unwrap().parse::<u32>().unwrap();
    assert!(major(version("mysqlnew")) > 8, "{}", version("mysqlnew"));
    assert!(major(version("mariadbnew")) > 10, "{}", version("mariadbnew"));
    for s in mysql_servers(db) {
        let printed = mysql(s, "SELECT VERSION()").unwrap();
        assert!(printed.trim().starts_with(&s.version), "{}: {printed}", s.name);
        assert_eq!(printed.contains("MariaDB"), s.kind == Kind::MariaDb, "{}: {printed}", s.name);
    }
}

/// Facts later tasks rely on, on every MySQL and MariaDB server.
#[test]
fn mysql_seed_facts() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let q = |sql: &str| mysql(s, sql).unwrap_or_else(|e| panic!("{}: {e}", s.name)).trim().to_owned();
        let at = &s.name;
        assert_eq!(q("SELECT COUNT(*) FROM shop.customers"), "500", "{at}");
        assert_ne!(q("SELECT COUNT(*) FROM shop.orders WHERE dealer_id = 42 AND status = 'completed' AND total > 100"), "0", "{at}");
        assert_eq!(q("SELECT name FROM shop.customers WHERE id = 7"), "Customer 7", "{at}");
        assert_eq!(q("SELECT meta, raw_json_text FROM shop.orders WHERE id = 1"), "\"{\\\"a\\\":1}\"\t{\"k\":[1,2]}", "{at}");
        assert_eq!(q("SELECT JSON_EXTRACT(billing, '$.paid') FROM shop.orders WHERE id = 6"), "true", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.order_items"), "10001", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.shipments"), "2500", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.all_types"), "3", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.events"), "200", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.order_totals"), "500", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM shop.big_rows"), s.big_rows().to_string(), "{at}");
        assert_eq!(q("SELECT MAX(payload) FROM shop.big_rows"), if s.full_seed { "payload 999999" } else { "payload 9999" }, "{at}");
        let cell = if s.full_seed { "20000000" } else { "" };
        assert_eq!(q("SELECT CHAR_LENGTH(t) FROM shop.big_cell"), cell, "{at}");
        if s.full_seed {
            assert!(q("SELECT LENGTH(j) FROM shop.big_cell").parse::<u64>().unwrap() >= 20_000_000, "{at}");
        }
        assert_eq!(q("SELECT ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'shop' AND TABLE_NAME = 'audit_log'"), "MyISAM", "{at}");
        assert_eq!(q("SELECT CAST(d AS CHAR) FROM shop.legacy_dates WHERE id = 1"), "0000-00-00", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'many'"), "300", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM odd.`Odd \"Name\" é` WHERE `Total $` > 1"), "2", "{at}");
        assert_eq!(q("SELECT `lower UPPER` FROM odd.`Odd \"Name\" é` WHERE id = 3"), "gamma 🐙", "{at}");
        assert_eq!(q("SELECT `back``tick` FROM odd.`Odd \"Name\" é` WHERE id = 1"), "x`y", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'scratch'"), "0", "{at}");
        assert_eq!(q("SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = 'dbee_seed'"), "0", "{at}");
        assert_eq!(q("SELECT @@max_allowed_packet"), "67108864", "{at}");
        // TLS is on (`cargo xtask testdb up` also checks a handshake on each server).
        let not_after = q("SHOW GLOBAL STATUS LIKE 'Ssl_server_not_after'");
        assert!(not_after.split('\t').nth(1).is_some_and(|v| v.contains("GMT")), "{at}: no server certificate: {not_after:?}");
        assert!(mysql(s, "SELECT nope").unwrap_err().contains("nope"), "{at}");
    }
    assert_eq!(db.server("mysql80").map(|s| s.service_port), Some(3306), "the sshd reaches mysql80:3306");
    let full: Vec<_> = mysql_servers(db).filter(|s| s.full_seed).map(|s| s.name.as_str()).collect();
    assert_eq!(full, ["mysql80", "mariadbnew"]);
}

#[test]
fn sqlite_seed_has_the_shop() {
    let f = Copy(sqlite_seeded(&tmp()));
    let c = open(&f.0);
    assert_eq!(count(&c, "SELECT count(*) FROM orders"), ORDERS_COUNT);
    assert_eq!(count(&c, "SELECT count(*) FROM big_rows"), 1_000_000);
    assert_eq!(count(&c, "SELECT count(*) FROM customers"), 500);
    assert_eq!(count(&c, "SELECT count(*) FROM order_items"), 10_001);
    assert_eq!(count(&c, "SELECT count(*) FROM shipments"), 2_500);
    assert_ne!(count(&c, "SELECT count(*) FROM orders WHERE dealer_id = 42 AND status = 'completed' AND total > 100"), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM all_types"), 3);
    assert_eq!(count(&c, "SELECT length(t) FROM big_cell"), 20_000_000);
    assert!(count(&c, "SELECT length(j) FROM big_cell") >= 19_000_000, "about 20 MB");
    assert_eq!(count(&c, "SELECT count(*) FROM pragma_foreign_key_check"), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name GLOB 't_[0-9][0-9][0-9]'"), 300);
    assert_eq!(count(&c, r#"SELECT count(*) FROM "Odd ""Name"" 🐙" WHERE "Total $" > 1"#), 2);
    assert_eq!(count(&c, "SELECT count(*) FROM notes_nokey"), 3);
    assert_eq!(count(&c, "SELECT count(*) FROM pragma_table_info('rowid_taken') WHERE name = 'rowid'"), 1);
    assert_eq!(count(&c, "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'dbee_seed%'"), 0);
    let meta: (String, String) = c.query_row("SELECT meta, raw_json_text FROM orders WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(meta, (r#""{\"a\":1}""#.to_owned(), r#"{"k":[1,2]}"#.to_owned()));
    let integrity: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(integrity, "ok");
}

#[test]
fn sqlite_copies_are_independent() {
    let a = Copy(sqlite_seeded(&tmp()));
    let b = Copy(sqlite_seeded(&tmp()));
    assert_ne!(a.0, b.0);
    assert!(a.0.starts_with(tmp()) && b.0.starts_with(tmp()));
    open(&a.0).execute("DELETE FROM notes_nokey", []).unwrap();
    assert_eq!(count(&open(&a.0), "SELECT count(*) FROM notes_nokey"), 0);
    assert_eq!(count(&open(&b.0), "SELECT count(*) FROM notes_nokey"), 3);
}

#[test]
fn sql_on_a_file_has_the_shape_of_the_servers_clients() {
    let t = dbee_testkit::targets_of(&[Kind::Sqlite]).pop().unwrap();
    // Every statement in turn, each row a line, the values separated by `|`, NULL empty.
    let out =
        t.sql("CREATE TABLE t (a INTEGER, b TEXT); INSERT INTO t VALUES (1, 'x'), (2, NULL); SELECT a, b FROM t ORDER BY a; SELECT 0.5, X'0AFF'");
    assert_eq!(out.as_deref(), Ok("1|x\n2|\n0.5|X'0AFF'\n"));
    assert!(t.sql("SELECT nope").is_err_and(|e| e.contains("no such column")));
    // Its one schema holds every schema of the seed.
    assert_eq!((t.schema("shop"), t.schema("scratch")), ("main", "main"));
    assert_eq!(t.sql(&format!("SELECT count(*) FROM {}.customers", t.schema("shop"))).as_deref(), Ok("500\n"));
}

#[test]
fn targets_cover_every_engine() {
    require_testdb!();
    let all = targets();
    let k: BTreeSet<_> = all.iter().map(|t| t.kind().id()).collect();
    assert_eq!(k, ["mariadb", "mysql", "postgres", "sqlite"].into());
    let names: Vec<_> = all.iter().map(TestTarget::name).collect();
    assert_eq!(names, ["pg14", "pgnew", "mysql57", "mysql80", "mysqlnew", "mariadb1011", "mariadbnew", "sqlite"]);
    let lite = all.iter().find_map(|t| t.file().map(Path::to_path_buf)).unwrap();
    assert!(lite.is_file() && lite.starts_with(dbee_testkit::repo_root().join("target")), "{lite:?}");
    assert_eq!(all.iter().filter(|t| t.is_file()).count(), 1);
    for t in &all {
        let sleep = t.sleep_sql(30);
        match t.kind().id() {
            "postgres" => assert_eq!(sleep, "SELECT pg_sleep(30)"),
            "mysql" | "mariadb" => assert_eq!(sleep, "SELECT SLEEP(30)"),
            _ => {
                // A count sized by this machine's rate: any machine counts a million rows a second.
                let rows = sleep.split("WHERE i < ").nth(1).and_then(|r| r.split(')').next()).and_then(|r| r.parse::<u64>().ok());
                assert!(sleep.starts_with("SELECT (SELECT count(*) FROM (WITH RECURSIVE") && rows >= Some(30_000_000), "{sleep}");
            }
        }
        let full = !t.server().is_some_and(|s| !s.full_seed);
        assert_eq!(t.big_rows(), if full { 1_000_000 } else { 30_000 }, "{}", t.name());
        assert_eq!(t.server().map(|s| s.kind), (!t.is_file()).then_some(t.kind()), "{}", t.name());
    }
    drop(all);
    assert!(!lite.exists(), "a dropped SQLite target removes its copy");
}

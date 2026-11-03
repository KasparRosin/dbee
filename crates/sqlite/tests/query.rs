//! Statement runner tests on fresh copies of the SQLite seed (`dbee_testkit::targets_of`) and on
//! files the tests make under `target/`. No server needed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind};
use dbee_core::{ResultSet, TypeClass};
use dbee_driver::{DbError, Limits, PAGE_LIMITS, Purpose, QUERY_LIMITS, Target};
use dbee_sqlite::{SqliteConnection, connect};
use dbee_testkit::{Kind, TestTarget, repo_root, targets_of, unique};

fn config(file: &Path) -> ConnectionConfig {
    ConnectionConfig { name: "test sqlite".into(), kind: DbKind::Sqlite, file: Some(file.to_path_buf()), ..ConnectionConfig::default() }
}

/// A fresh copy of the SQLite seed, removed when dropped.
fn seeded() -> TestTarget {
    targets_of(&[Kind::Sqlite]).pop().expect("the SQLite seed")
}

async fn open_file(file: &Path, create_file: bool) -> SqliteConnection {
    let c = config(file);
    let t = Target { config: &c, password: None, tunnel: None, purpose: Purpose::Query, create_file, own_file_only: false };
    connect(t).await.unwrap_or_else(|e| panic!("open {}: {e}", file.display()))
}

async fn open(seed: &TestTarget) -> SqliteConnection {
    open_file(seed.file().unwrap(), false).await
}

/// A new, empty database file under `target/`, removed when dropped.
struct Fresh(PathBuf);

impl Fresh {
    async fn new() -> (Fresh, SqliteConnection) {
        let dir = repo_root().join("target").join("testkit").join("sqlite-driver");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.sqlite", unique("query")));
        let conn = open_file(&file, true).await;
        (Fresh(file), conn)
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn ok(c: &SqliteConnection, sql: &str, limits: Limits) -> ResultSet {
    c.run(sql, limits).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

#[tokio::test]
async fn reals_print_as_sqlite_prints_them() {
    let (_file, c) = Fresh::new().await;
    for x in [
        "0.1",
        "1.0",
        "1e300",
        "-0.0",
        "123456789012345678",
        "3.141592653589793",
        "123456789012345678.0",
        "0.30000000000000004",
        "1e-7",
        "1.5e15",
        "1e16",
        "1e17",
        "5e-324",
        "1.7976931348623157e308",
        "12345678901234567890.0123456789",
        "9e999",
        "-9e999",
    ] {
        let r = ok(&c, &format!("SELECT {x}, CAST({x} AS TEXT)"), QUERY_LIMITS).await;
        assert_eq!(r.rows[0][0], r.rows[0][1], "{x}");
    }
    // As SQLite prints them, which is not Rust's shortest form.
    let r = ok(&c, "SELECT 1e300, -0.0, 5e-324, 9e999, 1.0", QUERY_LIMITS).await;
    assert_eq!(r.rows[0], [some("1.0e+300"), some("0.0"), some("4.9406564584124654e-324"), some("Inf"), some("1.0")]);
}

#[tokio::test]
async fn values_as_text_by_their_storage_class() {
    let seed = seeded();
    let c = open(&seed).await;
    let r = ok(&c, "SELECT i, r, m, n, b, anything, note, flag, d FROM all_types ORDER BY id", QUERY_LIMITS).await;
    assert_eq!(
        r.rows,
        [
            [
                some("42"),
                // SQLite's 17 digits, where Rust's shortest form has 16.
                some("3.1415926535897931"),
                some("12.34"),
                some("1.2345678901234567e+19"),
                some("X'DEADBEEF'"),
                some("text in a column without a type"),
                some("first row"),
                some("1"),
                some("2025-06-01")
            ],
            [
                some("9223372036854775807"),
                some("1.0e+300"),
                some("-0.01"),
                some("-1.0e-10"),
                some("X''"),
                some("X'00FF'"),
                some("second row: edge values 🐝"),
                some("0"),
                some("1999-12-31")
            ],
            [None, None, None, None, None, None, None, None, None],
        ]
    );
    let r = ok(&c, "SELECT -9223372036854775808, NULL, '', 'plain', 2.0 * 3", QUERY_LIMITS).await;
    assert_eq!(r.rows[0], [some("-9223372036854775808"), None, some(""), some("plain"), some("6.0")]);
    assert!(r.complete && r.rows_seen == 1 && r.rows_affected.is_none(), "{r:?}");
}

#[tokio::test]
async fn blobs_print_as_hex_literals() {
    let (_file, c) = Fresh::new().await;
    let r = ok(&c, "SELECT X'00ff', X'', zeroblob(3), CAST('ab' AS BLOB)", QUERY_LIMITS).await;
    assert_eq!(r.rows[0], [some("X'00FF'"), some("X''"), some("X'000000'"), some("X'6162'")]);
    let u = c.run_untyped("SELECT X'00ff'", QUERY_LIMITS).await.unwrap();
    assert_eq!(u.rows[0], [some("X'00FF'")]);
}

#[tokio::test]
async fn text_that_is_not_utf8_is_replaced_and_counted() {
    let (_file, c) = Fresh::new().await;
    // SQLite keeps a TEXT's bytes unchecked.
    let r = ok(&c, "SELECT CAST(X'61FF62' AS TEXT) AS e, typeof(CAST(X'61FF62' AS TEXT)) AS t", QUERY_LIMITS).await;
    assert_eq!(r.rows[0], [some("a\u{FFFD}b"), some("text")]);
    // The replacement's 3 bytes count: with the name's 1 byte, 6 fit and 5 do not.
    let sql = "SELECT CAST(X'61FF62' AS TEXT) AS e";
    let fits = ok(&c, sql, Limits { max_rows: 10, max_bytes: 6 }).await;
    let cut = ok(&c, sql, Limits { max_rows: 10, max_bytes: 5 }).await;
    assert_eq!((fits.rows.len(), fits.complete, cut.rows.len(), cut.complete, cut.rows_seen), (1, true, 0, false, 1));
}

#[tokio::test]
async fn classes_of_described_columns() {
    let seed = seeded();
    let c = open(&seed).await;
    use TypeClass::*;
    let r = ok(&c, "SELECT * FROM all_types ORDER BY id", QUERY_LIMITS).await;
    let got: Vec<(&str, &str, &str, TypeClass)> =
        r.columns.iter().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_label.as_str(), c.class)).collect();
    let expected = [
        ("id", "INTEGER", "INTEGER", Number),
        ("note", "TEXT", "TEXT", LongText),
        ("st", "TEXT", "TEXT", LongText),
        ("doc", "JSON", "JSON", Json),
        ("flag", "BOOLEAN", "BOOLEAN", Bool),
        ("i", "INTEGER", "INTEGER", Number),
        ("r", "REAL", "REAL", Number),
        ("m", "NUMERIC", "NUMERIC", Number),
        ("n", "DECIMAL", "DECIMAL(30,10)", Number),
        ("d", "DATE", "DATE", DateTime),
        ("ts", "DATETIME", "DATETIME", DateTime),
        ("b", "BLOB", "BLOB", Binary),
        ("u", "VARCHAR", "VARCHAR(36)", LongText),
        ("anything", "", "", Other),
        ("total", "REAL", "REAL", Number),
    ];
    assert_eq!(got, expected);
    // An expression has no declared type; a subquery's column and a view's keep their table's.
    let r = ok(&c, "SELECT 3, customer_id + 1, (SELECT total FROM orders LIMIT 1) AS t, customer_id FROM order_totals LIMIT 1", QUERY_LIMITS).await;
    let got: Vec<(&str, TypeClass)> = r.columns.iter().map(|c| (c.type_label.as_str(), c.class)).collect();
    assert_eq!(got, [("", Other), ("", Other), ("NUMERIC(12,2)", Number), ("INTEGER", Number)]);
    // Untyped, the values are the same and the columns carry names only.
    let u = c.run_untyped("SELECT b, total, CAST(total AS TEXT) FROM all_types WHERE id = 1", QUERY_LIMITS).await.unwrap();
    assert_eq!((u.rows[0][0].as_deref(), &u.rows[0][1]), (Some("X'DEADBEEF'"), &u.rows[0][2]));
    assert!(u.columns.iter().all(|c| c.base_type.is_empty() && c.type_label.is_empty() && c.class == Other), "{:?}", u.columns);
    assert_eq!(u.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["b", "total", "CAST(total AS TEXT)"]);
}

#[tokio::test]
async fn row_cap_keeps_first_and_counts_all() {
    let seed = seeded();
    let c = open(&seed).await;
    let r = ok(&c, "SELECT id FROM big_rows ORDER BY id LIMIT 25000", QUERY_LIMITS).await;
    assert_eq!((r.rows.len(), r.rows_seen, r.complete), (10_000, 25_000, false));
    assert_eq!((r.rows[0][0].as_deref(), r.rows[9_999][0].as_deref()), (Some("1"), Some("10000")));
    assert_eq!(c.scalar_u64("SELECT 7").await.unwrap(), 7);
}

#[tokio::test]
async fn byte_cap_stops_collecting() {
    let seed = seeded();
    let c = open(&seed).await;
    // 100,000 characters each: hex() of 50,000 zero bytes, its digits replaced.
    let sql = "SELECT replace(hex(zeroblob(50000)), '0', 'x') AS x FROM big_rows LIMIT 100";
    let r = ok(&c, sql, Limits { max_rows: 1000, max_bytes: 1 << 20 }).await;
    assert_eq!((r.rows.len(), r.rows_seen, r.complete), (10, 100, false));
    // A blob counts as its literal: 1000 bytes are 2003 characters.
    let r = ok(&c, "SELECT zeroblob(1000) AS b FROM big_rows LIMIT 10", Limits { max_rows: 1000, max_bytes: 1 + 3 * 2003 }).await;
    assert_eq!((r.rows.len(), r.rows_seen, r.complete), (3, 10, false));
    assert_eq!(r.rows[0][0].as_ref().map(String::len), Some(2003));
}

#[tokio::test]
async fn page_limit_of_101_rows_is_exact() {
    let seed = seeded();
    let c = open(&seed).await;
    let r = ok(&c, "SELECT id FROM big_rows ORDER BY id LIMIT 101", PAGE_LIMITS).await;
    assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 101, true));
    let r = ok(&c, "SELECT id FROM big_rows ORDER BY id LIMIT 102", PAGE_LIMITS).await;
    assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 102, false));
}

#[tokio::test]
async fn twenty_megabyte_cell_loads() {
    let seed = seeded();
    let c = open(&seed).await;
    let started = Instant::now();
    let r = ok(&c, "SELECT id, t, j FROM big_cell", QUERY_LIMITS).await;
    assert!(r.complete && r.rows.len() == 1);
    let t = r.rows[0][1].as_deref().unwrap_or_default();
    assert_eq!(t.len(), 20_000_000);
    assert!(t.starts_with("0123456789abcdefghij") && t.ends_with("XYZ-_"));
    let j = r.rows[0][2].as_deref().unwrap_or_default();
    assert_eq!(j.len() as u64, c.scalar_u64("SELECT length(j) FROM big_cell").await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
}

#[tokio::test]
async fn error_position_is_a_character_offset() {
    let seed = seeded();
    let c = open(&seed).await;
    match c.run("SELECT nope FROM orders", QUERY_LIMITS).await {
        Err(DbError::Server(e)) => {
            assert_eq!((e.code.as_str(), e.code_label, e.position), ("1", Some("SQLite error"), Some(8)), "{e:?}");
            assert!(e.message.contains("nope"), "{e:?}");
            assert_eq!(DbError::Server(e).to_string(), "no such column: nope (SQLite error 1)");
        }
        other => panic!("{other:?}"),
    }
    // Characters, not bytes: é is two bytes.
    let e = c.run("SELECT 'é', nope FROM orders", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.position == Some(13)), "{e:?}");
    let e = c.run("SELEC 1", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.position == Some(1) && s.message.contains("syntax error")), "{e:?}");
    // An error SQLite gives no place for has none.
    let e = c.run("SELECT 1 FROM nope", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.position.is_none()), "{e:?}");
    // In a text of several statements, the position counts from the text's start.
    let e = c.run("SELECT 1; SELECT nope FROM orders", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.position == Some(18)), "{e:?}");
    assert!(!c.is_closed());
    assert_eq!(c.scalar_u64("SELECT 2").await.unwrap(), 2, "usable after an error");
}

#[tokio::test]
async fn constraint_errors_carry_the_extended_code() {
    let seed = seeded();
    let c = open(&seed).await;
    let e = c.run("INSERT INTO customers (id, name) VALUES (1, 'twice')", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.code == "1555"), "{e:?}");
    assert_eq!(e.to_string(), "UNIQUE constraint failed: customers.id (SQLite error 1555)");
    // Foreign keys are checked.
    let e = c.run("INSERT INTO orders (id, customer_id) VALUES (99999, 99999)", QUERY_LIMITS).await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.code == "787"), "{e:?}");
}

#[tokio::test]
async fn a_nul_never_reaches_sqlite() {
    let seed = seeded();
    let c = open(&seed).await;
    // SQLite would end the statement at the NUL and delete every row.
    for sql in ["DELETE FROM orders\0 WHERE id = 1", "DELETE FROM orders \0  ", "SELECT 'a\0b'", "\0"] {
        let e = c.run(sql, QUERY_LIMITS).await.unwrap_err();
        assert_eq!(e.code(), "other", "{sql:?}");
        assert!(e.to_string().contains("NUL"), "{e}");
        assert_eq!(c.scalar_u64("SELECT count(*) FROM orders\0").await.unwrap_err().code(), "other");
    }
    assert_eq!(c.scalar_u64("SELECT count(*) FROM orders").await.unwrap(), dbee_testkit::ORDERS_COUNT);
}

#[tokio::test]
async fn update_reports_rows_affected() {
    let (_file, c) = Fresh::new().await;
    let r = ok(&c, "CREATE TABLE t (a INTEGER)", QUERY_LIMITS).await;
    assert_eq!((r.rows_affected, r.columns.len(), r.complete), (Some(0), 0, true));
    assert_eq!(ok(&c, "INSERT INTO t VALUES (1), (2), (3)", QUERY_LIMITS).await.rows_affected, Some(3));
    let r = ok(&c, "UPDATE t SET a = a + 1", QUERY_LIMITS).await;
    assert!(r.rows_affected == Some(3) && r.columns.is_empty() && r.rows.is_empty() && r.complete, "{r:?}");
    // A statement after a write reports its own count, not the write's.
    assert_eq!(ok(&c, "CREATE INDEX t_a ON t (a)", QUERY_LIMITS).await.rows_affected, Some(0));
    assert_eq!(ok(&c, "SELECT a FROM t", QUERY_LIMITS).await.rows_affected, None);
    assert_eq!(ok(&c, "UPDATE t SET a = 0 WHERE a > 100", QUERY_LIMITS).await.rows_affected, Some(0));
    let r = ok(&c, "DELETE FROM t WHERE a = 2 RETURNING a", QUERY_LIMITS).await;
    assert_eq!((r.rows.clone(), r.rows_affected), (vec![vec![some("2")]], Some(1)));
    assert_eq!(ok(&c, "DELETE FROM t", QUERY_LIMITS).await.rows_affected, Some(2));
}

#[tokio::test]
async fn several_statements_run_in_turn_and_keep_the_last_rows() {
    let (_file, c) = Fresh::new().await;
    let r =
        ok(&c, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1), (2); SELECT a FROM t ORDER BY a; UPDATE t SET a = a + 1", QUERY_LIMITS).await;
    assert_eq!(r.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a"]);
    assert_eq!((r.rows.clone(), r.rows_affected), (vec![vec![some("1")], vec![some("2")]], Some(2)));
    assert_eq!(c.scalar_u64("SELECT sum(a) FROM t").await.unwrap(), 5, "every statement ran");
    // A text with no statement runs nothing.
    for empty in ["", "  ", "-- a comment", ";"] {
        let r = ok(&c, empty, QUERY_LIMITS).await;
        assert!(r.columns.is_empty() && r.rows.is_empty() && r.complete && r.rows_affected.is_none(), "{empty:?}: {r:?}");
    }
    // The first failure ends the run.
    let e = c.run("INSERT INTO t VALUES (10); INSERT INTO nope VALUES (1); INSERT INTO t VALUES (20)", QUERY_LIMITS).await.unwrap_err();
    assert_eq!(e.code(), "server");
    assert_eq!(c.scalar_u64("SELECT count(*) FROM t WHERE a >= 10").await.unwrap(), 1);
}

#[tokio::test]
async fn parameters_are_refused() {
    let (_file, c) = Fresh::new().await;
    for sql in ["SELECT ?", "SELECT :a", "SELECT ?1 + 1"] {
        let e = c.run(sql, QUERY_LIMITS).await.unwrap_err();
        assert!(e.code() == "other" && e.to_string().contains("parameter"), "{sql}: {e}");
    }
}

#[tokio::test]
async fn scalar_u64_reads_a_count_and_rejects_the_rest() {
    let seed = seeded();
    let c = open(&seed).await;
    assert_eq!(c.scalar_u64("SELECT count(*) FROM orders").await.unwrap(), dbee_testkit::ORDERS_COUNT);
    assert_eq!(c.scalar_u64("SELECT count(*) FROM orders WHERE 1 = 0").await.unwrap(), 0);
    for bad in ["SELECT 'abc'", "SELECT 1 FROM orders WHERE 1 = 0", "SELECT -1", "SELECT NULL", "SELECT 1.5"] {
        assert_eq!(c.scalar_u64(bad).await.unwrap_err().code(), "other", "{bad}");
    }
}

#[tokio::test]
async fn a_name_that_is_not_utf8_never_panics() {
    let (file, c) = Fresh::new().await;
    ok(&c, "CREATE TABLE odd_bytes (a INTEGER)", QUERY_LIMITS).await;
    ok(&c, "INSERT INTO odd_bytes VALUES (1)", QUERY_LIMITS).await;
    // A column name and a declared type that are not UTF-8, as a crafted file can hold them.
    ok(&c, "PRAGMA writable_schema = ON", QUERY_LIMITS).await;
    let crafted = "UPDATE sqlite_schema SET sql = 'CREATE TABLE odd_bytes (' || CAST(X'61FF' AS TEXT) || ' ' || CAST(X'54FF' AS TEXT) || ')' \
                   WHERE name = 'odd_bytes'";
    ok(&c, crafted, QUERY_LIMITS).await;
    drop(c);
    let c = open_file(&file.0, false).await;
    for typed in [true, false] {
        let r =
            if typed { c.run("SELECT * FROM odd_bytes", QUERY_LIMITS).await } else { c.run_untyped("SELECT * FROM odd_bytes", QUERY_LIMITS).await };
        let e = r.unwrap_err();
        assert!(e.code() == "other" && e.to_string().contains("UTF-8") && e.to_string().contains("odd_bytes"), "{e}");
    }
    // What needs no column name still works, and so does the catalog.
    assert_eq!(c.scalar_u64("SELECT count(*) FROM odd_bytes").await.unwrap(), 1);
    let catalog = c.load_catalog().await.unwrap();
    let t = catalog.table(&dbee_core::TableRef { schema: "main".into(), name: "odd_bytes".into() }).unwrap();
    assert_eq!(t.visible_columns().map(|c| (c.name.as_str(), c.type_sql.as_str())).collect::<Vec<_>>(), [("a\u{FFFD}", "T\u{FFFD}")]);
    assert!(!c.is_closed());
}

/// Makes `file` hold a table `name` whose column name and declared type are not UTF-8, through a
/// connection of its own; the schema's version moves on, as any schema change moves it.
async fn craft_odd_bytes(file: &Path, name: &str) {
    let c = open_file(file, true).await;
    for sql in [
        format!("CREATE TABLE {name} (a INTEGER)"),
        "PRAGMA writable_schema = ON".to_owned(),
        format!(
            "UPDATE sqlite_schema SET sql = 'CREATE TABLE {name} (' || CAST(X'61FF' AS TEXT) || ' ' || CAST(X'54FF' AS TEXT) || ')' WHERE name = '{name}'"
        ),
        "PRAGMA writable_schema = OFF".to_owned(),
    ] {
        ok(&c, &sql, QUERY_LIMITS).await;
    }
    let version = c.scalar_u64("PRAGMA schema_version").await.unwrap();
    ok(&c, &format!("PRAGMA schema_version = {}", version + 1), QUERY_LIMITS).await;
}

#[tokio::test]
async fn the_names_check_runs_again_after_a_schema_change_or_an_attach() {
    let (fresh, c) = Fresh::new().await;
    ok(&c, "CREATE TABLE plain (a INTEGER)", QUERY_LIMITS).await;
    assert_eq!(ok(&c, "SELECT a FROM plain", QUERY_LIMITS).await.columns[0].name, "a");
    // Another connection changes the schema: the next statement checks again.
    craft_odd_bytes(&fresh.0, "odd_main").await;
    let e = c.run("SELECT a FROM plain", QUERY_LIMITS).await.unwrap_err();
    assert!(e.code() == "other" && e.to_string().contains("odd_main"), "{e}");
    // A file attached later is checked too, and named with its database.
    let (_clean, d) = Fresh::new().await;
    ok(&d, "CREATE TABLE plain (a INTEGER)", QUERY_LIMITS).await;
    assert_eq!(ok(&d, "SELECT a FROM plain", QUERY_LIMITS).await.rows_seen, 0);
    let (other, _o) = Fresh::new().await;
    craft_odd_bytes(&other.0, "odd_other").await;
    let attach = format!("ATTACH {} AS other", dbee_sql::Dialect::Sqlite.literal(&other.0.display().to_string()).unwrap());
    ok(&d, &attach, QUERY_LIMITS).await;
    let e = d.run("SELECT a FROM plain", QUERY_LIMITS).await.unwrap_err();
    assert!(e.code() == "other" && e.to_string().contains("other.odd_other"), "{e}");
    ok(&d, "DETACH other", QUERY_LIMITS).await;
    assert_eq!(ok(&d, "SELECT a FROM plain", QUERY_LIMITS).await.columns[0].name, "a");
}

#[tokio::test]
async fn a_trigger_body_is_one_statement_as_sqlite_reads_it() {
    let (_file, c) = Fresh::new().await;
    let text = "CREATE TABLE tl (a INTEGER); \
                CREATE TRIGGER tl_t AFTER INSERT ON tl BEGIN \
                    UPDATE tl SET a = a + 1 WHERE rowid = new.rowid; \
                    SELECT CASE WHEN new.a > 100 THEN RAISE(ABORT, 'too big') END; \
                END; \
                INSERT INTO tl VALUES (1); \
                SELECT a FROM tl";
    let r = ok(&c, text, QUERY_LIMITS).await;
    assert_eq!(r.rows, [[some("2")]]);
    let e = c.run("INSERT INTO tl VALUES (101)", QUERY_LIMITS).await.unwrap_err();
    assert!(e.to_string().contains("too big"), "{e}");
}

#[tokio::test]
async fn a_comment_after_a_write_keeps_its_count() {
    let (_file, c) = Fresh::new().await;
    ok(&c, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1), (2)", QUERY_LIMITS).await;
    assert_eq!(ok(&c, "UPDATE t SET a = a + 1; -- c", QUERY_LIMITS).await.rows_affected, Some(2));
    assert_eq!(ok(&c, "DELETE FROM t WHERE a = 3 /* c */", QUERY_LIMITS).await.rows_affected, Some(1));
}

#[tokio::test]
async fn sql_opens_no_other_file_when_the_run_allows_only_its_own() {
    let dir = repo_root().join("target").join("testkit").join("sqlite-driver").join(unique("own-file"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("own.sqlite");
    let c = config(&file);
    let confined = Target { config: &c, password: None, tunnel: None, purpose: Purpose::Query, create_file: true, own_file_only: true };
    let conn = connect(confined).await.unwrap();
    ok(&conn, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1)", QUERY_LIMITS).await;
    let at = |name: &str| dbee_sql::Dialect::Sqlite.literal(&dir.join(name).display().to_string()).unwrap();
    let uri = |name: &str| dbee_sql::Dialect::Sqlite.literal(&format!("file:{}?mode=rwc", dir.join(name).display())).unwrap();
    // Every ATTACH and every VACUUM INTO, whatever the name; a plain VACUUM attaches a database of
    // its own, so it goes too.
    for sql in [
        format!("ATTACH {} AS other", at("other.sqlite")),
        format!("ATTACH {} AS other", uri("uri.sqlite")),
        "ATTACH '' AS other".to_owned(),
        "ATTACH ':memory:' AS other".to_owned(),
        format!("VACUUM INTO {}", at("copy.sqlite")),
        format!("VACUUM main INTO {}", uri("copy-uri.sqlite")),
        "VACUUM".to_owned(),
    ] {
        let e = conn.run(&sql, QUERY_LIMITS).await.unwrap_err();
        let DbError::Server(s) = &e else { panic!("{sql}: {e}") };
        assert_eq!(s.code, "23", "{sql}: {e}");
        assert!(s.hint.as_deref().is_some_and(|h| h.contains("ATTACH") && h.contains("VACUUM")), "{sql}: {:?}", s.hint);
    }
    let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["own.sqlite"], "no other file was made");
    assert_eq!(ok(&conn, "SELECT a FROM t", QUERY_LIMITS).await.rows, [[some("1")]], "the file's own SQL runs");
    drop(conn);
    // A run that allows any file: both reach other files (ATTACH one that is there, as the file
    // was opened without leave to make files).
    std::fs::write(dir.join("other.sqlite"), "").unwrap();
    let open = Target { config: &c, password: None, tunnel: None, purpose: Purpose::Query, create_file: false, own_file_only: false };
    let conn = connect(open).await.unwrap();
    ok(&conn, &format!("ATTACH {} AS other; CREATE TABLE other.o (b INTEGER); DETACH other", at("other.sqlite")), QUERY_LIMITS).await;
    ok(&conn, &format!("VACUUM INTO {}", at("copy.sqlite")), QUERY_LIMITS).await;
    ok(&conn, "VACUUM", QUERY_LIMITS).await;
    assert!(dir.join("other.sqlite").is_file() && dir.join("copy.sqlite").is_file());
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

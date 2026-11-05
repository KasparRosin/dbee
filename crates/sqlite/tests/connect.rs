//! Opening SQLite files, read-only mode and cancel, on fresh copies of the SQLite seed
//! (`dbee_testkit::targets_of`) and on files the tests make under `target/`. No server needed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind};
use dbee_core::TableRef;
use dbee_driver::{ApplyGate, Cancel, Connection, DbError, Driver, Purpose, QUERY_LIMITS, Target};
use dbee_sql::Dialect;
use dbee_sqlite::{Canceller, SqliteConnection, SqliteDriver, connect};
use dbee_testkit::{Kind, TestTarget, repo_root, targets_of, unique};

fn config(file: &Path, read_only: bool) -> ConnectionConfig {
    ConnectionConfig { name: "test sqlite".into(), kind: DbKind::Sqlite, file: Some(file.to_path_buf()), read_only, ..ConnectionConfig::default() }
}

fn target(c: &ConnectionConfig) -> Target<'_> {
    Target { config: c, password: None, tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None }
}

/// A fresh copy of the SQLite seed, removed when dropped.
fn seeded() -> TestTarget {
    targets_of(&[Kind::Sqlite]).pop().expect("the SQLite seed")
}

/// A directory of its own under `target/`, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let dir = repo_root().join("target").join("testkit").join("sqlite-driver").join(unique("connect"));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn open(c: &ConnectionConfig) -> SqliteConnection {
    connect(target(c)).await.unwrap_or_else(|e| panic!("open {:?}: {e}", c.file))
}

async fn scalar(conn: &SqliteConnection, sql: &str) -> u64 {
    conn.scalar_u64(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// Counts to `n` in SQLite itself, so it takes a while (about 0.1 s per million on this machine).
fn count_to(n: u64) -> String {
    format!("WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < {n}) SELECT count(*) FROM c")
}

#[tokio::test]
async fn opens_and_reports_version() {
    let seed = seeded();
    let c = config(seed.file().unwrap(), false);
    let conn = SqliteDriver.connect(target(&c)).await.unwrap();
    let info = conn.info();
    assert_eq!(info.product, "SQLite");
    assert!(info.version.starts_with("3."), "{info:?}");
    assert!(info.version_num >= 3_045_000 && info.version_num.to_string().starts_with('3'), "{info:?}");
    assert!(!info.read_only && info.warnings.is_empty(), "{info:?}");
    assert_eq!(conn.dialect(), Dialect::Sqlite);
    assert!(!conn.is_closed());
    // The session's settings: foreign keys checked, and a busy file waited for 5 s.
    assert_eq!(conn.scalar_u64("PRAGMA foreign_keys").await, Ok(1));
    assert_eq!(conn.scalar_u64("PRAGMA busy_timeout").await, Ok(5000));
    assert_eq!(conn.scalar_u64("PRAGMA query_only").await, Ok(0));
    assert_eq!(conn.scalar_u64("SELECT count(*) FROM orders").await, Ok(dbee_testkit::ORDERS_COUNT));
}

#[tokio::test]
async fn missing_file_is_not_created() {
    let dir = Scratch::new();
    let p = dir.path("nope.sqlite");
    for read_only in [false, true] {
        let c = config(&p, read_only);
        let e = SqliteDriver.connect(target(&c)).await.err().unwrap();
        assert_eq!(e.code(), "connect", "{e}");
        assert!(e.to_string().contains(&format!("no file at {}", p.display())), "{e}");
        assert!(!p.exists(), "read_only {read_only}");
        // A read-only connection never makes a file, even when asked to.
        let asked = Target { create_file: true, ..target(&c) };
        if read_only {
            assert_eq!(SqliteDriver.connect(asked).await.err().map(|e| e.code()), Some("connect"));
            assert!(!p.exists());
        }
    }
    // A missing directory too.
    let deeper = dir.path("gone").join("nope.sqlite");
    let e = SqliteDriver.connect(target(&config(&deeper, false))).await.err().unwrap();
    assert!(e.code() == "connect" && e.to_string().contains("no file at"), "{e}");
    assert!(!deeper.exists() && !dir.path("gone").exists());
}

/// A file is one database: there is no list. The driver opens nothing for one, and makes no
/// file; an open file's connection lists none.
#[tokio::test]
async fn a_file_lists_no_databases() {
    let dir = Scratch::new();
    let p = dir.path("nope.sqlite");
    let missing = config(&p, false);
    let asked = Target { create_file: true, ..target(&missing) };
    assert!(SqliteDriver.list_connection(asked).await.is_ok_and(|c| c.is_none()));
    assert!(!p.exists());
    let seed = seeded();
    let conn = SqliteDriver.connect(target(&config(seed.file().unwrap(), false))).await.unwrap();
    assert_eq!(conn.list_databases().await, Ok(Vec::new()));
}

#[tokio::test]
async fn new_file_is_created_when_asked() {
    let dir = Scratch::new();
    let p = dir.path("new.sqlite");
    let c = config(&p, false);
    let conn = SqliteDriver.connect(Target { create_file: true, ..target(&c) }).await.unwrap();
    assert!(p.is_file(), "the file exists");
    let catalog = conn.load_catalog().await.unwrap();
    assert_eq!(catalog.table_count(), 0);
    assert_eq!(catalog.schemas.iter().map(|s| (s.name.as_str(), s.system)).collect::<Vec<_>>(), [("main", false)]);
    assert_eq!(catalog.default_schema.as_deref(), Some("main"));
    // It is a database: it takes a table, and a second connection sees it.
    conn.run("CREATE TABLE t (a INTEGER)", QUERY_LIMITS).await.unwrap();
    let again = open(&c).await;
    assert_eq!(scalar(&again, "SELECT count(*) FROM t").await, 0);
    // An existing file is opened as it is, also when asked to make one.
    let reopened = SqliteDriver.connect(Target { create_file: true, ..target(&c) }).await.unwrap();
    assert_eq!(reopened.scalar_u64("SELECT count(*) FROM sqlite_schema").await, Ok(1));
}

#[tokio::test]
async fn not_a_database_file_is_a_connect_error() {
    let dir = Scratch::new();
    let p = dir.path("junk.sqlite");
    // 4 KiB of pseudo-random bytes (xorshift), never a SQLite header.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let bytes: Vec<u8> = (0..4096)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(&p, &bytes).unwrap();
    for read_only in [false, true] {
        let e = SqliteDriver.connect(target(&config(&p, read_only))).await.err().unwrap();
        assert_eq!(e.code(), "connect", "{e}");
        let text = e.to_string();
        assert!(text.contains(&p.display().to_string()) && text.contains("not a SQLite database"), "{text}");
    }
    assert_eq!(std::fs::read(&p).unwrap(), bytes, "the file is left as it was");
    // A directory is no database either.
    let e = SqliteDriver.connect(target(&config(&dir.0, false))).await.err().unwrap();
    assert!(e.code() == "connect" && e.to_string().contains(&dir.0.display().to_string()), "{e}");
}

#[tokio::test]
async fn a_uri_is_a_file_name() {
    // The library reads `file:` names as URIs; dbee opens the file of that name instead.
    let seed = seeded();
    let uri = PathBuf::from(format!("file:{}?mode=ro", seed.file().unwrap().display()));
    let e = SqliteDriver.connect(target(&config(&uri, false))).await.err().unwrap();
    assert!(e.code() == "connect" && e.to_string().contains("no file at"), "{e}");
    assert!(!uri.exists());
}

#[tokio::test]
async fn read_only_refuses_writes() {
    let seed = seeded();
    let c = config(seed.file().unwrap(), true);
    let conn = open(&c).await;
    assert!(conn.info().read_only);
    let new_orders = "SELECT count(*) FROM orders WHERE status = 'new'";
    let before = scalar(&conn, new_orders).await;
    for write in ["CREATE TABLE z (a INTEGER)", "UPDATE orders SET status = 'new'", "DELETE FROM orders", "CREATE TEMP TABLE z (a INTEGER)"] {
        let e = conn.run(write, QUERY_LIMITS).await.unwrap_err();
        assert_eq!(e.code(), "readOnly", "{write}: {e}");
        assert!(e.to_string().contains("readonly database"), "{e}");
    }
    assert_eq!(scalar(&conn, "PRAGMA query_only").await, 1);
    assert_eq!(scalar(&conn, "SELECT count(*) FROM orders").await, dbee_testkit::ORDERS_COUNT);
    assert!(!conn.is_closed());
    let writer = open(&config(seed.file().unwrap(), false)).await;
    assert_eq!(scalar(&writer, new_orders).await, before, "nothing was written");
    assert!(before < dbee_testkit::ORDERS_COUNT);
}

/// Starts `sql`, cancels it after 300 ms through the `Cancel` trait, and expects `cancelled`
/// within 5 s; then the connection still works.
async fn cancel_after_300_ms(conn: Arc<SqliteConnection>, sql: String) {
    let canceller: Arc<dyn Cancel> = Arc::new(conn.canceller());
    let runner = Arc::clone(&conn);
    let job = tokio::spawn(async move { runner.run(&sql, QUERY_LIMITS).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    canceller.cancel().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), job).await.expect("not cancelled within 5 s").unwrap();
    assert_eq!(result.err(), Some(DbError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!conn.is_closed());
    assert_eq!(conn.scalar_u64("SELECT 7").await, Ok(7));
}

#[tokio::test]
async fn cancel_stops_a_long_query() {
    let seed = seeded();
    let conn = Arc::new(open(&config(seed.file().unwrap(), false)).await);
    cancel_after_300_ms(Arc::clone(&conn), seed.sleep_sql(30)).await;
    // A long statement that writes: nothing of it stays.
    let insert = format!("CREATE TABLE counted AS {}", count_to(1_000_000_000));
    cancel_after_300_ms(Arc::clone(&conn), insert).await;
    assert_eq!(scalar(&conn, "SELECT count(*) FROM sqlite_schema WHERE name = 'counted'").await, 0);
}

#[tokio::test]
async fn late_interrupt_never_cancels_the_next_statement() {
    let seed = seeded();
    let conn = Arc::new(open(&config(seed.file().unwrap(), false)).await);
    let canceller: Canceller = conn.canceller();
    // A cancel with nothing running reaches nothing.
    canceller.cancel().await.unwrap();
    assert_eq!(scalar(&conn, &count_to(2_000_000)).await, 2_000_000);
    // The statement after a cancelled one runs to its end.
    cancel_after_300_ms(Arc::clone(&conn), seed.sleep_sql(30)).await;
    assert_eq!(scalar(&conn, &count_to(2_000_000)).await, 2_000_000);
    // Cancels that meet short statements at every point of their run: each stops the statement
    // it met or nothing, never the one after it.
    for i in 0..40u64 {
        let runner = Arc::clone(&conn);
        let job = tokio::spawn(async move { runner.scalar_u64(&count_to(50_000)).await });
        tokio::time::sleep(Duration::from_micros(i * 250)).await;
        canceller.cancel().await.unwrap();
        let first = job.await.unwrap();
        assert!(matches!(first, Ok(50_000) | Err(DbError::Cancelled)), "{i}: {first:?}");
        assert_eq!(conn.scalar_u64(&count_to(50_000)).await, Ok(50_000), "{i}: the next statement");
    }
}

#[test]
fn connection_types_are_shareable() {
    fn send_sync<T: Send + Sync + 'static>() {}
    send_sync::<SqliteConnection>();
    send_sync::<Canceller>();
    send_sync::<SqliteDriver>();
}

#[tokio::test]
async fn sqlite_driver_connects_through_the_trait() {
    assert_eq!(SqliteDriver.kinds(), [DbKind::Sqlite]);
    let seed = seeded();
    let c = config(seed.file().unwrap(), false);
    let conn: Arc<dyn Connection> = SqliteDriver.connect(target(&c)).await.unwrap();
    assert_eq!((conn.info().product.as_str(), conn.dialect()), ("SQLite", Dialect::Sqlite));
    // Each method reaches the SQLite connection.
    assert_eq!(conn.scalar_u64("SELECT 40 + 2").await, Ok(42));
    let typed = conn.run("SELECT total FROM orders WHERE id = 1", QUERY_LIMITS).await.unwrap();
    assert_eq!((typed.columns[0].type_label.as_str(), typed.rows[0][0].as_deref()), ("NUMERIC(12,2)", Some("80.19")));
    let untyped = conn.run_untyped("SELECT total FROM orders WHERE id = 1", QUERY_LIMITS).await.unwrap();
    assert!(untyped.columns[0].type_label.is_empty() && untyped.columns[0].name == "total", "{:?}", untyped.columns);
    let catalog = conn.load_catalog().await.unwrap();
    assert!(catalog.table(&TableRef { schema: "main".into(), name: "orders".into() }).is_some());
    assert_eq!(conn.apply(&[], &ApplyGate::default()).await.map_err(|e| e.to_string()), Ok(0));
    // The trait's catalog is the connection's own, which dropped no row.
    let own = open(&c).await;
    assert_eq!(own.catalog_rows().await.unwrap().dropped(), 0);
    assert_eq!(catalog, own.load_catalog().await.unwrap());
    // The trait's canceller stops a long statement.
    let canceller = conn.canceller();
    let runner = Arc::clone(&conn);
    let sleep = seed.sleep_sql(30);
    let job = tokio::spawn(async move { runner.run_untyped(&sleep, QUERY_LIMITS).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), job).await.expect("not cancelled within 5 s").unwrap();
    assert_eq!(result.err(), Some(DbError::Cancelled));
    assert_eq!(conn.scalar_u64("SELECT 1").await, Ok(1));
}

#[tokio::test]
async fn a_dropped_connection_stops_its_statement() {
    let seed = seeded();
    let conn = open(&config(seed.file().unwrap(), false)).await;
    let canceller = conn.canceller();
    let conn = Arc::new(conn);
    let runner = Arc::clone(&conn);
    // It reads a table, so it holds the file's read lock while it runs.
    let slow = format!("SELECT count(*) FROM orders WHERE {}", seed.slow_condition_sql(30));
    let job = tokio::spawn(async move { runner.run(&slow, QUERY_LIMITS).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The job holds the last handle; abandoning the job drops it, and the statement with it.
    job.abort();
    drop(conn);
    // Whatever ran on the file has stopped: a second connection writes at once.
    let started = Instant::now();
    let writer = open(&config(seed.file().unwrap(), false)).await;
    writer.run("CREATE TABLE after_drop (a INTEGER)", QUERY_LIMITS).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    // A canceller that outlives its connection sends nothing.
    canceller.cancel().await.unwrap();
}

#[tokio::test]
async fn opening_reads_the_schema_not_the_whole_file() {
    let dir = Scratch::new();
    let p = dir.path("damaged.sqlite");
    let c = config(&p, false);
    let conn = SqliteDriver.connect(Target { create_file: true, ..target(&c) }).await.unwrap();
    let rows = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200) SELECT zeroblob(2000) FROM n";
    conn.run(&format!("CREATE TABLE t (b BLOB); INSERT INTO t {rows}"), QUERY_LIMITS).await.unwrap();
    drop(conn);
    // Damage the last page, which holds rows, not the schema.
    let mut bytes = std::fs::read(&p).unwrap();
    let len = bytes.len();
    assert!(len > 64 * 4096, "{len}");
    bytes[len - 4096..].fill(0xA5);
    std::fs::write(&p, &bytes).unwrap();
    // Opening reads the schema only, so it opens; the damage shows where the rows are read.
    let conn = open(&c).await;
    let e = conn.scalar_u64("SELECT sum(length(b)) FROM t").await.unwrap_err();
    assert!(matches!(&e, DbError::Server(s) if s.code == "11"), "{e:?}");
    assert_eq!(scalar(&conn, "SELECT count(*) FROM sqlite_schema").await, 1);
}

#[tokio::test]
async fn a_locked_file_at_open_is_a_connect_error_naming_it() {
    let seed = seeded();
    let holder = open(&config(seed.file().unwrap(), false)).await;
    holder.run("BEGIN EXCLUSIVE", QUERY_LIMITS).await.unwrap();
    let started = Instant::now();
    let e = connect(target(&config(seed.file().unwrap(), false))).await.unwrap_err();
    assert!(started.elapsed() >= Duration::from_millis(4_500), "the busy timeout: {:?}", started.elapsed());
    assert_eq!(e.code(), "connect", "{e}");
    assert!(e.to_string().contains(&format!("{}: database is locked", seed.file().unwrap().display())), "{e}");
    holder.run("ROLLBACK", QUERY_LIMITS).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_path_that_is_no_file_is_refused_at_once() {
    let dir = Scratch::new();
    let fifo = dir.path("fifo.sqlite");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    for read_only in [true, false] {
        let started = Instant::now();
        // Opened, a FIFO would wait for a writer.
        let e = tokio::time::timeout(Duration::from_secs(5), connect(target(&config(&fifo, read_only)))).await.expect("refused at once").unwrap_err();
        assert!(e.code() == "connect" && e.to_string().contains(&format!("{} is not a file", fifo.display())), "{e}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }
    let e = connect(target(&config(&dir.0, false))).await.unwrap_err();
    assert!(e.to_string().contains("is not a file"), "{e}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_write_protected_file_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let seed = seeded();
    let file = seed.file().unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o444)).unwrap();
    // Asked for read-write, SQLite opens it read-only, and says so.
    let conn = open(&config(file, false)).await;
    assert!(conn.info().read_only);
    assert_eq!(conn.run("UPDATE orders SET status = 'new'", QUERY_LIMITS).await.unwrap_err().code(), "readOnly");
    assert_eq!(scalar(&conn, "PRAGMA query_only").await, 1);
    assert_eq!(scalar(&conn, "SELECT count(*) FROM orders").await, dbee_testkit::ORDERS_COUNT);
}

#[test]
fn the_file_is_closed_off_the_dropping_thread() {
    // One blocking thread, kept busy below, so a close sent to it waits where the test can see it.
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).max_blocking_threads(1).enable_all().build().unwrap();
    let dir = Scratch::new();
    let p = dir.path("wal.sqlite");
    let c = config(&p, false);
    let conn = rt.block_on(async {
        let conn = connect(Target { create_file: true, ..target(&c) }).await.unwrap();
        conn.run("PRAGMA journal_mode = WAL", QUERY_LIMITS).await.unwrap();
        conn.run("CREATE TABLE t (a INTEGER)", QUERY_LIMITS).await.unwrap();
        conn
    });
    // The last close of a WAL file checkpoints it and deletes the WAL: writes on the closing thread.
    let wal = dir.path("wal.sqlite-wal");
    assert!(wal.exists());
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let busy = rt.spawn_blocking(move || wait.recv());
    // Dropped on a thread with no runtime, as the UI's.
    std::thread::spawn(move || drop(conn)).join().unwrap();
    assert!(wal.exists(), "the file was closed on the dropping thread");
    go.send(()).unwrap();
    rt.block_on(busy).unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while wal.exists() {
        assert!(Instant::now() < deadline, "the file was never closed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn a_file_outside_the_allowed_folders_is_never_opened_or_made() {
    let (allowed, outside) = (Scratch::new(), Scratch::new());
    let folders = [allowed.0.clone()];
    async fn open(c: &ConnectionConfig, folders: &[PathBuf], create_file: bool) -> Result<SqliteConnection, DbError> {
        connect(Target { create_file, allowed_dirs: Some(folders), ..target(c) }).await
    }
    // Inside: opened, made when asked.
    let made = config(&allowed.path("made.sqlite"), false);
    assert!(open(&made, &folders, true).await.is_ok());
    assert!(allowed.path("made.sqlite").is_file());
    // Outside, or a link out of the folder: refused before anything is opened, and nothing is made.
    let mut refused = vec![outside.path("new.sqlite"), allowed.0.join("..").join(outside.0.file_name().unwrap()).join("other.sqlite")];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path("linked.sqlite"), allowed.path("link.sqlite")).unwrap();
        refused.push(allowed.path("link.sqlite"));
    }
    for path in &refused {
        let e = open(&config(path, false), &folders, true).await.err().unwrap_or_else(|| panic!("{} opened", path.display()));
        assert_eq!(e.code(), "config", "{}: {e}", path.display());
        assert!(e.to_string().starts_with("this isolated run may only open files in its own folders; "), "{e}");
    }
    assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0, "nothing was made outside");
    // Without allowed folders, no limit.
    let free = config(&outside.path("free.sqlite"), false);
    assert!(connect(Target { create_file: true, ..target(&free) }).await.is_ok());
}

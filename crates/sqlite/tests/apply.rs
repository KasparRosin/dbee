//! All-or-nothing apply tests, each on a fresh copy of the SQLite seed (`dbee_testkit::targets_of`)
//! with a table of its own, checked from a second connection to the file.

use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind};
use dbee_driver::{ApplyError, ApplyGate, DbError, Purpose, QUERY_LIMITS, Target};
use dbee_sqlite::{SqliteConnection, connect};
use dbee_testkit::{Kind, TestTarget, targets_of};

const TABLE: &str = "apply_t";

async fn open(seed: &TestTarget, read_only: bool, purpose: Purpose) -> SqliteConnection {
    let c = ConnectionConfig {
        name: "test sqlite".into(),
        kind: DbKind::Sqlite,
        file: Some(seed.file().unwrap().to_path_buf()),
        read_only,
        ..ConnectionConfig::default()
    };
    connect(Target { config: &c, password: None, tunnel: None, purpose, create_file: false, allowed_dirs: None })
        .await
        .unwrap_or_else(|e| panic!("open: {e}"))
}

async fn sql(c: &SqliteConnection, sql: &str) {
    if let Err(e) = c.run_untyped(sql, QUERY_LIMITS).await {
        panic!("{sql}: {e}");
    }
}

/// `id:name:amount` of every row, ordered by id, read through `c`.
async fn rows(c: &SqliteConnection) -> Vec<String> {
    let r = c
        .run_untyped(&format!("SELECT id || ':' || coalesce(name, '-') || ':' || coalesce(amount, '-') FROM {TABLE} ORDER BY id"), QUERY_LIMITS)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    r.rows.into_iter().filter_map(|row| row.into_iter().next().flatten()).collect()
}

async fn apply(c: &SqliteConnection, statements: &[String]) -> Result<u64, ApplyError> {
    c.apply(statements, &ApplyGate::default()).await
}

/// A fresh seed copy with a two-row table, a writer (the browse connection) and a reader.
async fn with_table() -> (TestTarget, SqliteConnection, SqliteConnection) {
    let seed = targets_of(&[Kind::Sqlite]).pop().expect("the SQLite seed");
    let writer = open(&seed, false, Purpose::Browse).await;
    let reader = open(&seed, false, Purpose::Query).await;
    sql(&writer, &format!("CREATE TABLE {TABLE} (id INTEGER PRIMARY KEY, name TEXT, amount NUMERIC)")).await;
    sql(&writer, &format!("INSERT INTO {TABLE} VALUES (1, 'one', 1.5), (2, 'two', 2.5)")).await;
    (seed, writer, reader)
}

const UNCHANGED: [&str; 2] = ["1:one:1.5", "2:two:2.5"];

/// Waits until a transaction holds `file`'s write lock (an Apply's `BEGIN IMMEDIATE` takes it): a
/// connection of the test's own then cannot take the file's lock.
async fn until_write_locked(file: &std::path::Path) {
    let file = file.to_path_buf();
    let probing = tokio::task::spawn_blocking(move || {
        let probe = rusqlite::Connection::open(&file).unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match probe.execute_batch("BEGIN EXCLUSIVE; ROLLBACK") {
                Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => return,
                Ok(()) => {}
                Err(e) => panic!("the probe: {e}"),
            }
            assert!(Instant::now() < deadline, "no transaction took the file's write lock");
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    probing.await.unwrap();
}

/// Waits until a write transaction has written its first change: SQLite has made the file's
/// rollback journal, `journal`, which it does at a transaction's first write and removes at its end.
async fn until_written(journal: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !journal.exists() {
        assert!(Instant::now() < deadline, "no change was written");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn apply_writes_all_or_nothing() {
    let (seed, w, r) = with_table().await;
    let t = TABLE;

    // Every statement in one transaction.
    let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET amount = 9.25 WHERE id = 2")];
    assert_eq!(apply(&w, &stmts).await.map_err(|e| e.to_string()), Ok(2));
    assert_eq!(rows(&r).await, ["1:uno:1.5", "2:two:9.25"]);
    sql(&w, &format!("UPDATE {t} SET name = 'one' WHERE id = 1; UPDATE {t} SET amount = 2.5 WHERE id = 2")).await;
    assert_eq!(rows(&r).await, UNCHANGED);

    // One failure writes nothing, and the connection is usable after it, outside a transaction.
    let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET id = 1 WHERE id = 2")];
    let err = apply(&w, &stmts).await.expect_err("must fail");
    assert_eq!(err.index, 1);
    assert!(matches!(&err.error, DbError::Server(e) if e.code == "1555"), "{:?}", err.error);
    assert_eq!(rows(&r).await, UNCHANGED);
    sql(&w, &format!("UPDATE {t} SET name = 'after' WHERE id = 1")).await;
    assert_eq!(rows(&r).await[0], "1:after:1.5");
    sql(&w, &format!("UPDATE {t} SET name = 'one' WHERE id = 1")).await;

    // A statement that matches no row, or two, rolls back the ones before it.
    let stmts = vec![format!("UPDATE {t} SET name = 'ghost' WHERE id = 99"), format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
    let err = apply(&w, &stmts).await.expect_err("must fail");
    assert_eq!((err.index, &err.error), (0, &DbError::RowNotFound { statement: 0 }));
    let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET name = 'many'")];
    let err = apply(&w, &stmts).await.expect_err("must fail");
    assert_eq!((err.index, err.error.code()), (1, "rowNotFound"));
    assert_eq!(rows(&r).await, UNCHANGED);

    // A read-only connection refuses at the start, rolls back and still answers.
    let ro = open(&seed, true, Purpose::Browse).await;
    let err = apply(&ro, &[format!("UPDATE {t} SET name = 'uno' WHERE id = 1")]).await.expect_err("must fail");
    assert_eq!((err.index, err.error.code()), (0, "readOnly"));
    assert!(!err.to_string().contains(t), "Display must not carry statement text");
    assert!(!ro.is_closed());
    assert_eq!(rows(&ro).await, UNCHANGED);
    assert!(!w.is_closed());
}

#[tokio::test]
async fn apply_while_file_locked_fails_cleanly() {
    apply_while_file_locked().await;
}

#[tokio::test]
#[ignore = "timing; run with --ignored --nocapture"]
async fn apply_while_file_locked_waits_the_busy_timeout() {
    let waited = apply_while_file_locked().await;
    println!("sqlite: an Apply on a locked file gave up after {waited:?}");
    assert!(waited >= Duration::from_millis(4_500) && waited < Duration::from_secs(10), "the busy timeout: {waited:?}");
}

/// An Apply while another connection holds the file's write lock fails at its first statement,
/// writes nothing and leaves the connection usable; how long it waited for the lock.
async fn apply_while_file_locked() -> Duration {
    let (seed, w, r) = with_table().await;
    // Another connection holds the file's write lock.
    let holder = open(&seed, false, Purpose::Query).await;
    sql(&holder, "BEGIN EXCLUSIVE").await;
    let started = Instant::now();
    let err = apply(&w, &[format!("UPDATE {TABLE} SET name = 'uno' WHERE id = 1")]).await.expect_err("locked");
    let waited = started.elapsed();
    assert_eq!((err.index, err.error.code()), (0, "server"), "{err}");
    assert!(matches!(&err.error, DbError::Server(e) if e.code == "5" && e.message == "database is locked"), "{:?}", err.error);
    sql(&holder, "ROLLBACK").await;
    assert_eq!(rows(&r).await, UNCHANGED, "nothing written once the lock is gone");
    assert!(!w.is_closed());
    let written = apply(&w, &[format!("UPDATE {TABLE} SET name = 'uno' WHERE id = 1")]).await.map_err(|e| e.to_string());
    assert_eq!(written, Ok(1));
    waited
}

#[tokio::test]
async fn empty_apply_sends_nothing() {
    let (_seed, w, _r) = with_table().await;
    assert_eq!(apply(&w, &[]).await.map_err(|e| e.to_string()), Ok(0));
    assert!(!w.is_closed());
}

#[tokio::test]
async fn multi_statement_text_is_refused_before_sending() {
    let (_seed, w, r) = with_table().await;
    let t = TABLE;
    for bad in [format!("UPDATE {t} SET name = 'x' WHERE id = 1; COMMIT"), format!("UPDATE {t} SET name = 'x' WHERE id = 1; SELECT 1")] {
        let stmts = vec![format!("UPDATE {t} SET name = 'ok' WHERE id = 2"), bad];
        let err = apply(&w, &stmts).await.expect_err("refused");
        assert_eq!((err.index, err.error.code()), (1, "other"));
    }
    assert_eq!(rows(&r).await, UNCHANGED);
    assert!(!w.is_closed());
}

#[tokio::test]
async fn dropped_apply_marks_the_connection_closed() {
    let (seed, w, r) = with_table().await;
    let stmts = vec![format!("UPDATE {TABLE} SET name = 'gone' WHERE id = 1 AND {}", seed.slow_condition_sql(30))];
    // The Apply is dropped once its transaction holds the file's write lock.
    tokio::select! {
        out = apply(&w, &stmts) => panic!("the apply ended: {out:?}"),
        () = until_write_locked(seed.file().unwrap()) => {}
    }
    assert!(w.is_closed());
    // Nothing may run on it: it is still inside the abandoned transaction.
    assert_eq!(w.run_untyped("SELECT 1", QUERY_LIMITS).await.unwrap_err().code(), "connectionLost");
    let later = vec![format!("UPDATE {TABLE} SET name = 'later' WHERE id = 2")];
    assert_eq!(apply(&w, &later).await.unwrap_err().error.code(), "connectionLost");
    // Stop the running statement and drop the connection: the original rows remain, and the
    // file takes writes again.
    let _ = w.canceller().cancel().await;
    drop(w);
    assert_eq!(rows(&r).await, UNCHANGED);
    let again = open(&seed, false, Purpose::Browse).await;
    assert_eq!(apply(&again, &later).await.map_err(|e| e.to_string()), Ok(1));
}

#[tokio::test]
async fn a_gate_cancels_an_apply_until_it_commits() {
    let (seed, w, r) = with_table().await;
    let t = TABLE;
    // Cancelled before it starts: nothing is sent past BEGIN IMMEDIATE, nothing is written.
    let gate = ApplyGate::default();
    assert!(gate.cancel());
    let err = w.apply(&[format!("UPDATE {t} SET name = 'x' WHERE id = 1")], &gate).await.expect_err("cancelled");
    assert_eq!((err.index, err.error.code()), (0, "cancelled"));
    assert!(!gate.committing() && !w.is_closed(), "it rolled back and stays usable");
    assert_eq!(rows(&r).await, UNCHANGED);

    // Cancelled while its second statement runs: the interrupt stops it, SQLite ends the
    // transaction, and the first statement is rolled back too.
    let gate = ApplyGate::default();
    let stmts = vec![
        format!("UPDATE {t} SET name = 'first' WHERE id = 2"),
        format!("UPDATE {t} SET name = 'slow' WHERE id = 1 AND {}", seed.slow_condition_sql(30)),
    ];
    let canceller = w.canceller();
    let journal = seed.file().unwrap().with_extension("sqlite-journal");
    let cancel = async {
        until_written(&journal).await;
        assert!(gate.cancel(), "before the commit");
        canceller.cancel().await.unwrap();
    };
    let (out, ()) = tokio::join!(w.apply(&stmts, &gate), cancel);
    let err = out.expect_err("cancelled");
    assert_eq!((err.index, err.error.code()), (1, "cancelled"));
    assert!(!w.is_closed(), "the rollback found the transaction ended, which is no loss");
    assert_eq!(rows(&r).await, UNCHANGED);

    // Once it decided to commit, a cancel is refused.
    let gate = ApplyGate::default();
    let written = w.apply(&[format!("UPDATE {t} SET name = 'z' WHERE id = 2")], &gate).await.map_err(|e| e.to_string());
    assert_eq!(written, Ok(1));
    assert!(gate.committing() && !gate.cancel());
    assert_eq!(rows(&r).await, ["1:one:1.5", "2:z:2.5"]);
}

#[tokio::test]
async fn one_row_is_the_rows_the_statement_itself_changed() {
    let (_seed, w, r) = with_table().await;
    sql(&w, "CREATE TABLE apply_log (id INTEGER PRIMARY KEY, note TEXT)").await;
    let trigger = format!(
        "CREATE TRIGGER apply_logged AFTER UPDATE ON {TABLE} BEGIN INSERT INTO apply_log (note) VALUES ('a'); INSERT INTO apply_log (note) VALUES ('b'); END"
    );
    sql(&w, &trigger).await;
    // The trigger's two rows are not the statement's.
    let written = apply(&w, &[format!("UPDATE {TABLE} SET name = 'uno' WHERE id = 1")]).await.map_err(|e| e.to_string());
    assert_eq!(written, Ok(1));
    assert_eq!(w.scalar_u64("SELECT count(*) FROM apply_log").await, Ok(2));
    // A cell set to the value it holds still changes its row.
    let same = apply(&w, &[format!("UPDATE {TABLE} SET name = 'two' WHERE id = 2")]).await.map_err(|e| e.to_string());
    assert_eq!(same, Ok(1));
    assert_eq!(rows(&r).await, ["1:uno:1.5", "2:two:2.5"]);
}

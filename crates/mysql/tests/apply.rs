//! All-or-nothing apply tests against every MySQL and MariaDB server in `db.mysql` (`cargo xtask
//! testdb up`). Each creates a `scratch` InnoDB table of its own, checks results from a second
//! connection, and drops the table.

use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_driver::{ApplyError, ApplyGate, DbError, Purpose, QUERY_LIMITS, Target};
use dbee_mysql::{MySqlConnection, connect};
use dbee_testkit::{MyFlavor, MyServer, TestDb, require_testdb, unique};

async fn open(s: &MyServer, db: &TestDb, read_only: bool) -> MySqlConnection {
    let config = ConnectionConfig {
        name: format!("test {}", s.name),
        kind: if s.flavor == MyFlavor::MariaDb { DbKind::MariaDb } else { DbKind::MySql },
        host: "127.0.0.1".into(),
        port: s.port,
        user: "root".into(),
        ssl_mode: SslMode::Prefer,
        read_only,
        ..ConnectionConfig::default()
    };
    match connect(Target { config: &config, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false }).await {
        Ok(c) => c,
        Err(e) => panic!("connect to {} failed: {e}", s.name),
    }
}

async fn sql(c: &MySqlConnection, sql: &str) {
    if let Err(e) = c.run_untyped(sql, QUERY_LIMITS).await {
        panic!("{sql}: {e}");
    }
}

/// `id:name:amount` of every row, ordered by id, read through `c`.
async fn rows(c: &MySqlConnection, table: &str) -> Vec<String> {
    let r = c
        .run_untyped(&format!("SELECT CONCAT(id, ':', COALESCE(name, '-'), ':', COALESCE(amount, '-')) FROM {table} ORDER BY id"), QUERY_LIMITS)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    r.rows.into_iter().filter_map(|row| row.into_iter().next().flatten()).collect()
}

async fn apply(c: &MySqlConnection, statements: &[String]) -> Result<u64, ApplyError> {
    c.apply(statements, &ApplyGate::default()).await
}

/// Run `body(server, writer, reader, table)` against every server with a fresh two-row table.
async fn with_table<F, Fut>(db: &'static TestDb, body: F)
where
    F: Fn(&'static MyServer, MySqlConnection, MySqlConnection, String) -> Fut,
    Fut: Future<Output = ()>,
{
    for s in &db.mysql {
        let writer = open(s, db, false).await;
        let reader = open(s, db, false).await;
        let table = format!("scratch.`{}`", unique("apply"));
        sql(&writer, &format!("CREATE TABLE {table} (id INT PRIMARY KEY, name VARCHAR(5), amount DECIMAL(10,2)) ENGINE = InnoDB")).await;
        sql(&writer, &format!("INSERT INTO {table} VALUES (1, 'one', 1.5), (2, 'two', 2.5)")).await;
        body(s, writer, reader, table.clone()).await;
        let cleaner = open(s, db, false).await;
        sql(&cleaner, &format!("DROP TABLE IF EXISTS {table}")).await;
    }
}

#[tokio::test]
async fn writes_all_in_one_transaction() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET amount = 9.25 WHERE id = 2")];
        assert_eq!(apply(&w, &stmts).await.map_err(|e| e.to_string()), Ok(2), "{}", s.name);
        assert_eq!(rows(&r, &t).await, ["1:uno:1.50", "2:two:9.25"], "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn one_failure_writes_nothing() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET id = 1 WHERE id = 2")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!(err.index, 1, "{}", s.name);
        assert!(matches!(&err.error, DbError::Server(e) if e.code == "1062"), "{}: {:?}", s.name, err.error);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
        // The connection is usable again and not inside a transaction.
        sql(&w, &format!("UPDATE {t} SET name = 'after' WHERE id = 1")).await;
        assert_eq!(rows(&r, &t).await[0], "1:after:1.50", "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn missing_row_rolls_back() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'ghost' WHERE id = 99"), format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, &err.error), (0, &DbError::RowNotFound { statement: 0 }), "{}", s.name);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);

        // A statement that matches two rows is also refused, and rolls back the earlier one.
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET name = 'many'")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (1, "rowNotFound"), "{}", s.name);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn read_only_connection_refuses() {
    let db = require_testdb!();
    with_table(db, |s, _w, _r, t| async move {
        let ro = open(s, db, true).await;
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
        let err = apply(&ro, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (0, "readOnly"), "{}", s.name);
        assert!(!err.to_string().contains(&t), "Display must not carry statement text");
        // The refused transaction was rolled back; the connection still answers.
        assert!(!ro.is_closed());
        assert_eq!(rows(&ro, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn update_to_the_same_value_still_counts() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = name WHERE id = 1"), format!("UPDATE {t} SET amount = 2.50 WHERE id = 2")];
        assert_eq!(apply(&w, &stmts).await.map_err(|e| e.to_string()), Ok(2), "{}: matched, not changed, rows count", s.name);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn strict_mode_refuses_a_too_long_value() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET amount = 7 WHERE id = 2"), format!("UPDATE {t} SET name = 'toolong' WHERE id = 1")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (1, "server"), "{}", s.name);
        assert!(matches!(&err.error, DbError::Server(e) if e.code == "1406"), "{}: {:?}", s.name, err.error);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}: nothing written, nothing cut", s.name);
    })
    .await;
}

#[tokio::test]
async fn empty_apply_sends_nothing() {
    let db = require_testdb!();
    with_table(db, |s, w, _r, _t| async move {
        assert_eq!(apply(&w, &[]).await.map_err(|e| e.to_string()), Ok(0), "{}", s.name);
        assert!(!w.is_closed());
    })
    .await;
}

#[tokio::test]
async fn multi_statement_text_is_refused_before_sending() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        for bad in [format!("UPDATE {t} SET name = 'x' WHERE id = 1; COMMIT"), format!("UPDATE {t} SET name = 'x' WHERE id = 1; SELECT 1")] {
            let stmts = vec![format!("UPDATE {t} SET name = 'ok' WHERE id = 2"), bad];
            let err = apply(&w, &stmts).await.expect_err("refused");
            assert_eq!((err.index, err.error.code()), (1, "other"), "{}", s.name);
        }
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
        assert!(!w.is_closed());
    })
    .await;
}

#[tokio::test]
async fn dropped_apply_marks_the_connection_closed() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'gone' WHERE id = 1 AND SLEEP(5) = 0")];
        let out = tokio::time::timeout(Duration::from_millis(300), apply(&w, &stmts)).await;
        assert!(out.is_err(), "apply should still be running");
        assert!(w.is_closed(), "{}", s.name);
        // Nothing may run on it: it is still inside the abandoned transaction.
        assert_eq!(w.run_untyped("SELECT 1", QUERY_LIMITS).await.unwrap_err().code(), "connectionLost", "{}", s.name);
        let later = vec![format!("UPDATE {t} SET name = 'later' WHERE id = 2")];
        assert_eq!(apply(&w, &later).await.unwrap_err().error.code(), "connectionLost", "{}", s.name);
        // Stop the sleeping statement, drop the connection, and the original rows remain.
        let _ = w.canceller().cancel().await;
        drop(w);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);
    })
    .await;
}

#[tokio::test]
async fn a_gate_cancels_an_apply_until_it_commits() {
    let db = require_testdb!();
    with_table(db, |s, w, r, t| async move {
        // Cancelled before it starts: nothing is sent past START TRANSACTION, nothing is written.
        let gate = ApplyGate::default();
        assert!(gate.cancel());
        let err = w.apply(&[format!("UPDATE {t} SET name = 'x' WHERE id = 1")], &gate).await.expect_err("cancelled");
        assert_eq!((err.index, err.error.code()), (0, "cancelled"), "{}", s.name);
        assert!(!gate.committing() && !w.is_closed(), "{}: it rolled back and stays usable", s.name);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);

        // Cancelled while its second statement waits for a row the reader holds: the KILL QUERY
        // stops it, and the first one is rolled back too.
        let writer_id = w.scalar_u64("SELECT CONNECTION_ID()").await.unwrap();
        sql(&r, "START TRANSACTION").await;
        sql(&r, &format!("SELECT id FROM {t} WHERE id = 1 FOR UPDATE")).await;
        let gate = ApplyGate::default();
        let stmts = vec![format!("UPDATE {t} SET name = 'first' WHERE id = 2"), format!("UPDATE {t} SET name = 'waits' WHERE id = 1")];
        let canceller = w.canceller();
        // The second statement runs, so it waits: the reader holds its row.
        let waiting = format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {writer_id} AND INFO LIKE '%''waits''%'");
        let cancel = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while r.scalar_u64(&waiting).await.unwrap() != 1 {
                assert!(tokio::time::Instant::now() < deadline, "{}: the apply never waited for the lock", s.name);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(gate.cancel(), "{}: before the commit", s.name);
            canceller.cancel().await.unwrap_or_else(|e| panic!("{}: cancel: {e}", s.name));
        };
        let (out, ()) = tokio::join!(w.apply(&stmts, &gate), cancel);
        let err = out.expect_err("cancelled");
        assert_eq!((err.index, err.error.code()), (1, "cancelled"), "{}", s.name);
        assert!(!w.is_closed(), "{}", s.name);
        sql(&r, "ROLLBACK").await;
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:two:2.50"], "{}", s.name);

        // Once it decided to commit, a cancel is refused.
        let gate = ApplyGate::default();
        let written = w.apply(&[format!("UPDATE {t} SET name = 'z' WHERE id = 2")], &gate).await.map_err(|e| e.to_string());
        assert_eq!(written, Ok(1), "{}", s.name);
        assert!(gate.committing() && !gate.cancel(), "{}", s.name);
        assert_eq!(rows(&r, &t).await, ["1:one:1.50", "2:z:2.50"], "{}", s.name);
    })
    .await;
}

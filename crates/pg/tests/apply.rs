//! All-or-nothing apply tests against the Docker test servers (`cargo xtask testdb up`); every
//! test runs against every Postgres server in `db.servers`. Each creates a `scratch` table of its own,
//! checks results from a second connection, and drops the table.

use std::time::Duration;

use dbee_config::{ConnectionConfig, SslMode};
use dbee_driver::{ApplyGate, DbError, Purpose, QUERY_LIMITS, Target};
use dbee_pg::apply::{apply, apply_gated};
use dbee_pg::query::run_untyped;
use dbee_pg::{PgConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

/// The Postgres test servers.
fn postgres(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::Postgres])
}

async fn open(s: &Server, password: &str, read_only: bool) -> PgConnection {
    let config = ConnectionConfig {
        name: format!("test {}", s.name),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        read_only,
        ..ConnectionConfig::default()
    };
    match connect(Target {
        config: &config,
        password: Some(password),
        tunnel: None,
        purpose: Purpose::Browse,
        create_file: false,
        allowed_dirs: None,
    })
    .await
    {
        Ok(c) => c,
        Err(e) => panic!("connect to {} failed: {e}", s.name),
    }
}

async fn sql(c: &PgConnection, sql: &str) {
    if let Err(e) = run_untyped(c, sql, QUERY_LIMITS).await {
        panic!("{sql}: {e}");
    }
}

/// `name|amount` of every row, ordered by id, read through `c`.
async fn rows(c: &PgConnection, table: &str) -> Vec<String> {
    let r = run_untyped(
        c,
        &format!("SELECT id || ':' || coalesce(name, '-') || ':' || coalesce(amount::text, '-') FROM {table} ORDER BY id"),
        QUERY_LIMITS,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    r.rows.into_iter().filter_map(|row| row.into_iter().next().flatten()).collect()
}

/// Run `body(server, writer, reader, table)` against every server with a fresh two-row table.
async fn with_table<F, Fut>(db: &TestDb, body: F)
where
    F: Fn(u32, PgConnection, PgConnection, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for s in postgres(db) {
        let writer = open(s, &db.password, false).await;
        let reader = open(s, &db.password, false).await;
        let table = format!("scratch.\"{}\"", unique("apply"));
        sql(&writer, &format!("CREATE TABLE {table} (id int PRIMARY KEY, name text, amount numeric)")).await;
        sql(&writer, &format!("INSERT INTO {table} VALUES (1, 'one', 1.5), (2, 'two', 2.5)")).await;
        body(s.major(), writer, reader, table.clone()).await;
        let cleaner = open(s, &db.password, false).await;
        sql(&cleaner, &format!("DROP TABLE IF EXISTS {table}")).await;
    }
}

#[tokio::test]
async fn writes_all_in_one_transaction() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        let stmts = vec![
            format!("UPDATE {t} SET name = CAST('uno' AS text) WHERE id = 1"),
            format!("UPDATE {t} SET amount = CAST('9.25' AS numeric) WHERE id = 2"),
        ];
        assert_eq!(apply(&w, &stmts).await.map_err(|e| e.to_string()), Ok(2), "pg{major}");
        assert_eq!(rows(&r, &t).await, ["1:uno:1.5", "2:two:9.25"], "pg{major}");
    })
    .await;
}

#[tokio::test]
async fn one_failure_writes_nothing() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        let stmts = vec![
            format!("UPDATE {t} SET name = CAST('uno' AS text) WHERE id = 1"),
            format!("UPDATE {t} SET amount = CAST('abc' AS numeric) WHERE id = 2"),
        ];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!(err.index, 1, "pg{major}");
        assert_eq!(err.error.code(), "server", "pg{major}");
        assert!(matches!(&err.error, DbError::Server(s) if s.code == "22P02"), "pg{major}: {:?}", err.error);
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");
        // The connection is usable again and not inside a transaction.
        sql(&w, &format!("UPDATE {t} SET name = 'after' WHERE id = 1")).await;
        assert_eq!(rows(&r, &t).await[0], "1:after:1.5", "pg{major}");
    })
    .await;
}

#[tokio::test]
async fn missing_row_rolls_back() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'ghost' WHERE id = 99"), format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!(err.index, 0, "pg{major}");
        assert_eq!(err.error, DbError::RowNotFound { statement: 0 }, "pg{major}");
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");

        // A statement that matches two rows is also refused, and rolls back the earlier one.
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1"), format!("UPDATE {t} SET name = 'many'")];
        let err = apply(&w, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (1, "rowNotFound"), "pg{major}");
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");
    })
    .await;
}

#[tokio::test]
async fn read_only_connection_refuses() {
    let db = require_testdb!();
    with_table(db, |major, _w, _r, t| async move {
        let s = postgres(db).find(|s| s.major() == major).expect("server");
        let ro = open(s, &db.password, true).await;
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
        let err = apply(&ro, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (0, "readOnly"), "pg{major}");
        assert!(!err.to_string().contains(&t), "Display must not carry statement text");
        // The refused transaction was rolled back; the connection still answers.
        assert!(!ro.is_closed());
        assert_eq!(rows(&ro, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");
    })
    .await;
}

#[tokio::test]
async fn empty_apply_sends_nothing() {
    let db = require_testdb!();
    with_table(db, |major, w, _r, _t| async move {
        assert_eq!(apply(&w, &[]).await.map_err(|e| e.to_string()), Ok(0), "pg{major}");
        assert!(!w.is_closed());
    })
    .await;
}

#[tokio::test]
async fn dropped_apply_marks_the_connection_closed() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        let stmts = vec![format!("UPDATE {t} SET name = 'abandoned' WHERE id = 1 AND pg_sleep(5) IS NOT NULL")];
        let out = tokio::time::timeout(Duration::from_millis(300), apply(&w, &stmts)).await;
        assert!(out.is_err(), "apply should still be running");
        assert!(w.is_closed(), "pg{major}");
        // Nothing may run on it: it is still inside the abandoned transaction.
        let e = run_untyped(&w, "SELECT now() <> statement_timestamp()", QUERY_LIMITS).await.expect_err("refused");
        assert_eq!(e.code(), "connectionLost", "pg{major}");
        let later = vec![format!("UPDATE {t} SET name = 'later' WHERE id = 2")];
        let e = apply(&w, &later).await.expect_err("refused");
        assert_eq!(e.error.code(), "connectionLost", "pg{major}");
        // Stop the sleeping statement, drop the connection, and the original rows remain.
        let _ = w.canceller().cancel().await;
        drop(w);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");
    })
    .await;
}

#[tokio::test]
async fn multi_statement_text_is_refused_before_sending() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        for bad in [format!("UPDATE {t} SET name = 'x' WHERE id = 1; COMMIT"), format!("UPDATE {t} SET name = 'x' WHERE id = 1; SELECT 1")] {
            let stmts = vec![format!("UPDATE {t} SET name = 'ok' WHERE id = 2"), bad];
            let err = apply(&w, &stmts).await.expect_err("refused");
            assert_eq!((err.index, err.error.code()), (1, "other"), "pg{major}");
        }
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");
        assert!(!w.is_closed());
    })
    .await;
}

/// The number in the first cell of `sql`'s first row, run through `c`.
async fn number(c: &PgConnection, sql: &str) -> u64 {
    let r = run_untyped(c, sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    r.rows
        .first()
        .and_then(|row| row.first())
        .and_then(|cell| cell.as_deref())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{sql}: no number"))
}

#[tokio::test]
async fn a_gate_cancels_an_apply_until_it_commits() {
    let db = require_testdb!();
    with_table(db, |major, w, r, t| async move {
        // Cancelled before it starts: nothing is sent past BEGIN, nothing is written.
        let gate = ApplyGate::default();
        assert!(gate.cancel() && gate.cancel(), "a second cancel is still before the commit");
        let err = apply_gated(&w, &[format!("UPDATE {t} SET name = 'x' WHERE id = 1")], &gate).await.expect_err("cancelled");
        assert_eq!((err.index, err.error.code()), (0, "cancelled"), "pg{major}");
        assert_eq!(err.to_string(), "the Apply was cancelled at change 1, so nothing was written");
        assert!(!gate.committing() && !w.is_closed(), "pg{major}: it rolled back and stays usable");
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");

        // Cancelled while its second statement waits for a row the reader holds: the cancel
        // request stops it, and the first one is rolled back too.
        sql(&r, "BEGIN").await;
        sql(&r, &format!("SELECT id FROM {t} WHERE id = 1 FOR UPDATE")).await;
        let gate = ApplyGate::default();
        let stmts = vec![format!("UPDATE {t} SET name = 'first' WHERE id = 2"), format!("UPDATE {t} SET name = 'waits' WHERE id = 1")];
        let canceller = w.canceller();
        let waiters = "SELECT count(*) FROM pg_locks WHERE locktype = 'transactionid' AND transactionid = pg_current_xact_id()::xid AND NOT granted";
        let cancel = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while number(&r, waiters).await != 1 {
                assert!(tokio::time::Instant::now() < deadline, "pg{major}: the apply never waited for the lock");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(gate.cancel(), "pg{major}: before the commit");
            canceller.cancel().await.unwrap_or_else(|e| panic!("pg{major}: cancel: {e}"));
        };
        let (out, ()) = tokio::join!(apply_gated(&w, &stmts, &gate), cancel);
        let err = out.expect_err("cancelled");
        assert_eq!((err.index, err.error.code()), (1, "cancelled"), "pg{major}");
        assert!(!w.is_closed(), "pg{major}");
        sql(&r, "ROLLBACK").await;
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:two:2.5"], "pg{major}");

        // Once it decided to commit, a cancel is refused.
        let gate = ApplyGate::default();
        let written = apply_gated(&w, &[format!("UPDATE {t} SET name = 'z' WHERE id = 2")], &gate).await.map_err(|e| e.to_string());
        assert_eq!(written, Ok(1), "pg{major}");
        assert!(gate.committing() && !gate.cancel(), "pg{major}");
        assert_eq!(rows(&r, &t).await, ["1:one:1.5", "2:z:2.5"], "pg{major}");
    })
    .await;
}

//! All-or-nothing apply tests against the Docker test servers (`cargo xtask testdb up`); every
//! test runs against every server in `db.servers`. Each creates a `scratch` table of its own,
//! checks results from a second connection, and drops the table.

use std::time::Duration;

use dbee_config::{ConnectionConfig, SslMode};
use dbee_pg::apply::apply;
use dbee_pg::query::{QUERY_LIMITS, run_untyped};
use dbee_pg::{PgConnection, PgError, Target, connect};
use dbee_testkit::{PgServer, TestDb, require_testdb, unique};

async fn open(s: &PgServer, password: &str, read_only: bool) -> PgConnection {
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
    match connect(Target { config: &config, password: Some(password), tunnel: None }).await {
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
    for s in &db.servers {
        let writer = open(s, &db.password, false).await;
        let reader = open(s, &db.password, false).await;
        let table = format!("scratch.\"{}\"", unique("apply"));
        sql(&writer, &format!("CREATE TABLE {table} (id int PRIMARY KEY, name text, amount numeric)")).await;
        sql(&writer, &format!("INSERT INTO {table} VALUES (1, 'one', 1.5), (2, 'two', 2.5)")).await;
        body(s.major, writer, reader, table.clone()).await;
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
        assert!(matches!(&err.error, PgError::Server { code, .. } if code == "22P02"), "pg{major}: {:?}", err.error);
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
        assert_eq!(err.error, PgError::RowNotFound { statement: 0 }, "pg{major}");
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
        let s = db.servers.iter().find(|s| s.major == major).expect("server");
        let ro = open(s, &db.password, true).await;
        let stmts = vec![format!("UPDATE {t} SET name = 'uno' WHERE id = 1")];
        let err = apply(&ro, &stmts).await.expect_err("must fail");
        assert_eq!((err.index, err.error.code()), (0, "readOnly"), "pg{major}");
        assert!(!err.to_string().contains("uno"), "Display must not carry values");
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
        let stmts = vec![format!("UPDATE {t} SET name = 'slow' WHERE id = 1 AND pg_sleep(5) IS NOT NULL")];
        let out = tokio::time::timeout(Duration::from_millis(300), apply(&w, &stmts)).await;
        assert!(out.is_err(), "apply should still be running");
        assert!(w.is_closed(), "pg{major}: a connection dropped mid-transaction must not be reused");
        // The abandoned write never committed.
        assert_eq!(rows(&r, &t).await[0], "1:one:1.5", "pg{major}");
    })
    .await;
}

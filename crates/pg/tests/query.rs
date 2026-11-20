//! Statement runner tests against the Docker test servers (`cargo xtask testdb up`); every test
//! runs against every Postgres server in `db.servers`. Writing tests use a `scratch` table of their own
//! and drop it.

use dbee_config::{ConnectionConfig, SslMode};
use dbee_core::{ResultSet, TypeClass};
use dbee_driver::{DbError, Limits, Purpose, QUERY_LIMITS, Target};
use dbee_pg::query::{describe, run, scalar_u64};
use dbee_pg::{PgConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

/// The Postgres test servers.
fn postgres(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::Postgres])
}

async fn open(s: &Server, password: &str) -> PgConnection {
    let config = ConnectionConfig {
        name: format!("test {}", s.name),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
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

/// One connection per test server.
async fn conns(db: &TestDb) -> Vec<(u32, PgConnection)> {
    let mut out = Vec::new();
    for s in postgres(db) {
        out.push((s.major(), open(s, &db.password).await));
    }
    out
}

async fn ok(c: &PgConnection, sql: &str, limits: Limits) -> ResultSet {
    run(c, sql, limits).await.unwrap_or_else(|e| panic!("{e}"))
}

#[tokio::test]
async fn text_exactly_as_postgres_prints() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let r = ok(&c, r#"SELECT 1.50::numeric(10,2) AS n, (timestamptz '2024-01-02 03:04:05+00') AT TIME ZONE 'UTC' AS t, NULL::text AS z, ''::text AS e, '{"b":1,"a":2}'::jsonb AS j"#, QUERY_LIMITS).await;
        assert_eq!(
            r.rows[0],
            vec![Some("1.50".into()), Some("2024-01-02 03:04:05".into()), None, Some("".into()), Some(r#"{"a": 2, "b": 1}"#.into())],
            "pg{major}"
        );
        assert_eq!(
            r.columns.iter().map(|c| c.base_type.as_str()).collect::<Vec<_>>(),
            ["numeric", "timestamp", "text", "text", "jsonb"],
            "pg{major}"
        );
        assert_eq!(r.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["n", "t", "z", "e", "j"], "pg{major}");
        assert!(r.complete);
    }
}

#[tokio::test]
async fn row_cap_keeps_first_and_counts_all() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let r = ok(&c, "SELECT g FROM generate_series(1, 25000) g", QUERY_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (10_000, 25_000, false), "pg{major}");
        assert_eq!(r.rows[0], vec![Some("1".to_string())]);
        assert_eq!(r.rows[9_999], vec![Some("10000".to_string())]);
        // The connection stayed in sync.
        assert_eq!(scalar_u64(&c, "SELECT 7").await.unwrap(), 7, "pg{major}");
    }
}

#[tokio::test]
async fn byte_cap_stops_collecting() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let r = ok(&c, "SELECT repeat('x', 100000) FROM generate_series(1, 100)", Limits { max_rows: 1000, max_bytes: 1 << 20 }).await;
        assert!(!r.complete && r.rows.len() == 10, "pg{major}: {} rows", r.rows.len());
        assert_eq!(r.rows_seen, 100, "pg{major}");
        assert_eq!(scalar_u64(&c, "SELECT 1").await.unwrap(), 1);
    }
}

#[tokio::test]
async fn page_limit_of_101_rows_is_exact() {
    let db = require_testdb!();
    for (_, c) in conns(db).await {
        let r = ok(&c, "SELECT g FROM generate_series(1, 101) g", dbee_driver::PAGE_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 101, true));
        let r = ok(&c, "SELECT g FROM generate_series(1, 102) g", dbee_driver::PAGE_LIMITS).await;
        assert_eq!((r.rows.len(), r.rows_seen, r.complete), (101, 102, false));
    }
}

#[tokio::test]
async fn error_has_position() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        match run(&c, "SELECT nope FROM shop.orders", QUERY_LIMITS).await {
            Err(DbError::Server(s)) => {
                assert_eq!(s.code, "42703", "pg{major}");
                assert_eq!(s.position, Some(8), "pg{major}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(scalar_u64(&c, "SELECT 2").await.unwrap(), 2, "usable after an error");
    }
}

#[tokio::test]
async fn update_reports_rows_affected() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let t = unique("t_query");
        ok(&c, &format!("CREATE TABLE scratch.\"{t}\" AS SELECT g AS a FROM generate_series(1, 3) g"), QUERY_LIMITS).await;
        let r = run(&c, &format!("UPDATE scratch.\"{t}\" SET a = a + 1"), QUERY_LIMITS).await;
        let _ = run(&c, &format!("DROP TABLE IF EXISTS scratch.\"{t}\""), QUERY_LIMITS).await;
        let r = r.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.rows_affected, Some(3), "pg{major}");
        assert!(r.rows.is_empty() && r.complete);
    }
}

/// A statement whose command Postgres never counts (`BEGIN`, `SET`, `CREATE TABLE`) has no rows
/// affected, where tokio-postgres says 0; the ones it counts keep their count, 0 included.
#[tokio::test]
async fn only_counted_commands_report_rows_affected() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let t = unique("t_count");
        let cases = [
            ("BEGIN".to_owned(), None),
            ("SET LOCAL statement_timeout = 0".to_owned(), None),
            (format!("CREATE TEMP TABLE \"{t}\" (a int)"), None),
            (format!("/* first */ INSERT INTO \"{t}\" VALUES (1)"), Some(1)),
            (format!("WITH d AS (DELETE FROM \"{t}\" WHERE a = 2 RETURNING a) SELECT count(*) FROM d"), Some(1)),
            (format!("update \"{t}\" SET a = 2 WHERE false"), Some(0)),
            ("ROLLBACK".to_owned(), None),
        ];
        for (sql, affected) in cases {
            let r = ok(&c, &sql, QUERY_LIMITS).await;
            assert_eq!(r.rows_affected, affected, "pg{major}: {sql}");
        }
    }
}

#[tokio::test]
async fn describe_failure_falls_back_to_names() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let r = ok(&c, "SELECT 1 AS a; SELECT 2 AS b", QUERY_LIMITS).await;
        assert_eq!(r.columns.len(), 1, "pg{major}");
        assert_eq!(r.columns[0].name, "b");
        assert_eq!(r.columns[0].base_type, "");
        assert_eq!(r.rows, vec![vec![Some("2".to_string())]]);
        assert!(describe(&c, "SELECT 1 AS a; SELECT 2 AS b").await.is_none());
        assert!(describe(&c, "SELECT nope").await.is_none());
        assert_eq!(scalar_u64(&c, "SELECT 3").await.unwrap(), 3, "usable after failed describes");
    }
}

#[tokio::test]
async fn describe_names_types_and_leaves_no_statement_behind() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let cols = describe(&c, "SELECT 1::int4 AS a, 'x'::text AS b").await.expect("described");
        assert_eq!(
            cols.iter().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_label.as_str())).collect::<Vec<_>>(),
            [("a", "int4", "int4"), ("b", "text", "text")],
            "pg{major}"
        );
        let open = ok(&c, "SELECT count(*) FROM pg_prepared_statements", QUERY_LIMITS).await;
        assert_eq!(open.rows[0][0].as_deref(), Some("0"), "pg{major}");
    }
}

#[tokio::test]
async fn pg_described_columns_have_classes() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let rs = ok(&c, "SELECT 1::int4, true, 'x'::text", QUERY_LIMITS).await;
        assert_eq!(rs.columns.iter().map(|c| c.class).collect::<Vec<_>>(), [TypeClass::Number, TypeClass::Bool, TypeClass::LongText], "pg{major}");
        // Not a table column: everything the catalog maps, by the prepared statement's type.
        let sql = "SELECT 'x'::varchar, now(), '{}'::jsonb, '\\x00'::bytea, ARRAY[1], 1.5::numeric, '1 day'::interval, '<a/>'::xml";
        let rs = ok(&c, sql, QUERY_LIMITS).await;
        assert_eq!(
            rs.columns.iter().map(|c| c.class).collect::<Vec<_>>(),
            [
                TypeClass::Text,
                TypeClass::DateTime,
                TypeClass::Json,
                TypeClass::Binary,
                TypeClass::Other,
                TypeClass::Number,
                TypeClass::Other,
                TypeClass::LongText
            ],
            "pg{major}"
        );
    }
}

#[tokio::test]
async fn scalar_u64_reads_a_count_and_rejects_the_rest() {
    let db = require_testdb!();
    for (_, c) in conns(db).await {
        assert_eq!(scalar_u64(&c, "SELECT count(*) FROM shop.orders WHERE false").await.unwrap(), 0);
        assert!(scalar_u64(&c, "SELECT 'abc'").await.is_err());
        assert!(scalar_u64(&c, "SELECT 1 WHERE false").await.is_err());
        assert!(scalar_u64(&c, "SELECT -1").await.is_err());
    }
}

#[tokio::test]
async fn run_inside_a_transaction_does_not_abort_it() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        ok(&c, "BEGIN", QUERY_LIMITS).await;
        match run(&c, "SELECT nope", QUERY_LIMITS).await {
            Err(DbError::Server(s)) => {
                assert_eq!(s.code, "42703", "pg{major}");
                assert_eq!(s.position, Some(8), "pg{major}");
            }
            other => panic!("{other:?}"),
        }
        ok(&c, "ROLLBACK", QUERY_LIMITS).await;
        ok(&c, "BEGIN", QUERY_LIMITS).await;
        let r = ok(&c, "SELECT 1 AS a; SELECT 2 AS b", QUERY_LIMITS).await;
        assert_eq!(r.columns.len(), 1, "pg{major}");
        assert_eq!((r.columns[0].name.as_str(), r.columns[0].base_type.as_str()), ("b", ""), "pg{major}");
        assert_eq!(r.rows, vec![vec![Some("2".to_string())]], "pg{major}");
        ok(&c, "SELECT 3", QUERY_LIMITS).await;
        ok(&c, "ROLLBACK", QUERY_LIMITS).await;
    }
}

/// dbee's literals read back unchanged whether `standard_conforming_strings` is on or, after a
/// function turned it off for the session, off.
#[tokio::test]
async fn literals_read_the_same_with_standard_conforming_strings_off() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let flip = unique("scs_set");
        let made = format!(
            "CREATE FUNCTION scratch.{flip}(text) RETURNS text LANGUAGE sql AS $$ SELECT pg_catalog.set_config('standard_conforming_strings', $1, false) $$"
        );
        ok(&c, &made, QUERY_LIMITS).await;
        let values = ["\\", "\\'", "a\\'b", "\\' AS text); SELECT 1; --", "''\\\\''", "it's", "plain", "é\\n", "\\x41", "$$\\", "E'\\''"];
        let mut wrong = Vec::new();
        for setting in ["on", "off"] {
            ok(&c, &format!("SELECT scratch.{flip}('{setting}')"), QUERY_LIMITS).await;
            for v in values {
                let literal = dbee_sql::Dialect::Postgres.literal(v).unwrap();
                let read = run(&c, &format!("SELECT {literal} AS a, 'MARK' AS b"), QUERY_LIMITS).await.map(|r| r.rows);
                if read.as_ref().ok() != Some(&vec![vec![Some(v.to_owned()), Some("MARK".to_owned())]]) {
                    wrong.push(format!("{setting}: {literal} read {read:?}"));
                }
            }
        }
        ok(&c, &format!("SELECT scratch.{flip}('on')"), QUERY_LIMITS).await;
        ok(&c, &format!("DROP FUNCTION scratch.{flip}(text)"), QUERY_LIMITS).await;
        assert!(wrong.is_empty(), "pg{major}: {wrong:#?}");
    }
}

/// dbee's own catalog queries hold no backslash in a string, so a session whose
/// `standard_conforming_strings` a function turned off still loads its catalog: the setting is
/// checked only before a statement whose `'…'` strings hold one.
#[tokio::test]
async fn the_catalog_loads_after_a_function_turned_standard_conforming_strings_off() {
    let db = require_testdb!();
    for (major, c) in conns(db).await {
        let flip = unique("scs_set");
        let made = format!(
            "CREATE FUNCTION scratch.{flip}(text) RETURNS text LANGUAGE sql AS $$ SELECT pg_catalog.set_config('standard_conforming_strings', $1, false) $$"
        );
        ok(&c, &made, QUERY_LIMITS).await;
        ok(&c, &format!("SELECT scratch.{flip}('off')"), QUERY_LIMITS).await;
        let loaded = dbee_pg::catalog::load_catalog(&c).await.map(drop);
        ok(&c, &format!("SELECT scratch.{flip}('on')"), QUERY_LIMITS).await;
        ok(&c, &format!("DROP FUNCTION scratch.{flip}(text)"), QUERY_LIMITS).await;
        assert!(loaded.is_ok(), "pg{major}: {loaded:?}");
    }
}

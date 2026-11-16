//! SQLite's test SQL.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{KindInfo, Unseen};

// SQLite has no sleep: a count that takes about that long on this machine.
pub(crate) const INFO: KindInfo = KindInfo {
    id: "sqlite",
    product: "SQLite",
    dialect: "Sqlite",
    user: "",
    database: "",
    client: None,
    sleep_sql: |secs| format!("SELECT (SELECT count(*) FROM ({}))", sqlite_count(secs)),
    slow_condition_sql: |secs| format!("(SELECT count(*) FROM ({})) > 0", sqlite_count(secs)),
    one_schema: Some("main"),
    odd_table: "Odd \"Name\" 🐙",
    keyless_tables_editable: true,
    parallel_writes: false,
    enforces_lengths: false,
    // SQLite adds no key to a table once it is made: the copy is keyed by its rowid.
    copy_table_sql: |t, source| format!("CREATE TABLE {t} AS SELECT * FROM {source}"),
    date_column_sql: |c| format!("{c} date CHECK ({c} IS date({c}))"),
    repeat_sql: |text, n| format!("printf('%.*c', {n}, {text})"),
    failing_when_sql: |cond| format!("json(CASE WHEN {cond} THEN 'not JSON' ELSE '1' END) IS NOT NULL"),
    long_text_type: "TEXT",
    json_type: "JSON",
    dbee_connections_sql: None,
    kill_sql: |_| String::new(),
    // A trigger cannot hold a WITH clause, so the count goes in a view the trigger reads.
    slow_updates_sql: Some(|t, secs| {
        let sql = format!(
            "CREATE VIEW main.{t}_slow AS SELECT count(*) AS n FROM ({});
             CREATE TRIGGER main.{t}_slow_update BEFORE UPDATE ON {t} BEGIN SELECT n FROM {t}_slow; END;",
            sqlite_count(secs)
        );
        (sql, format!("TRIGGER IF EXISTS main.{t}_slow_update; DROP VIEW IF EXISTS main.{t}_slow"))
    }),
    row_lock_waits_sql: |_| String::new(),
    commit_wait: None,
    read_only_sql: ("PRAGMA query_only", "1"),
    read_only_breakers: &["PRAGMA query_only = 0", "PRAGMA main.query_only = OFF", "ATTACH ':memory:' AS other"],
    read_only_unseen: Unseen::None(
        "a read-only SQLite file is its open flags and query_only: no SQL changes the first, and the authorizer refuses setting the second \
         whatever runs it",
    ),
    connection_id_sql: "",
    running_with_sql: |_| String::new(),
    running_ids_sql: |_| String::new(),
    cancel_sql: |_| String::new(),
    cancel_error: ("", ""),
    take_lock_sql: |_| String::new(),
    waits_for_lock_sql: |_| String::new(),
    // Measured with its driver: a column labels its declared type, and an expression has none.
    json_type_label: "JSON",
    plain_json_label: "JSON",
    json_sorts_keys: false,
    int_literal_class: "other",
    int_literal_type: "",
    failing_error: "malformed JSON",
};

/// A count SQLite steps through in about `secs` seconds, at the rate [`rows_per_second`] measured.
fn sqlite_count(secs: u32) -> String {
    let rows = u64::from(secs).saturating_mul(rows_per_second());
    format!("WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < {rows}) SELECT i FROM c")
}

/// The rows a second this machine counts through, measured once per test run: the fastest of three
/// counts, so a count sized by it lasts at least that long. Machines differ by four times and more.
/// Without a measurement, 2.5 million, a busy laptop's rate.
fn rows_per_second() -> u64 {
    static RATE: OnceLock<u64> = OnceLock::new();
    *RATE.get_or_init(|| {
        const ROWS: u64 = 500_000;
        let sql = format!("WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < {ROWS}) SELECT count(*) FROM c");
        let count = |db: &rusqlite::Connection| {
            let start = Instant::now();
            db.query_row(&sql, [], |r| r.get::<_, i64>(0)).map(|_| start.elapsed())
        };
        let fastest = rusqlite::Connection::open_in_memory()
            .and_then(|db| (0..3).map(|_| count(&db)).collect::<Result<Vec<Duration>, _>>())
            .ok()
            .and_then(|times| times.into_iter().min());
        fastest.map_or(2_500_000, |d| (ROWS as f64 / d.as_secs_f64().max(1e-6)) as u64)
    })
}

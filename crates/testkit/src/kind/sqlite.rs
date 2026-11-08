//! SQLite's test SQL.

use crate::{KindInfo, Unseen};

// SQLite has no sleep: a count that takes about that long, at least on this machine.
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

/// A count SQLite steps through in about `secs` seconds: 2.5 million rows a second, as measured
/// on a busy development machine (2026-11-03).
fn sqlite_count(secs: u32) -> String {
    let rows = u64::from(secs).saturating_mul(2_500_000);
    format!("WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < {rows}) SELECT i FROM c")
}

//! PostgreSQL's test SQL.

use super::{copy_keyed_by_id, two_rows_when};
use crate::{Client, CommitWait, KindInfo};

pub(crate) const INFO: KindInfo = KindInfo {
    id: "postgres",
    product: "PostgreSQL",
    dialect: "Postgres",
    user: "postgres",
    database: "dbee",
    client: Some(Client::Psql),
    sleep_sql: |secs| format!("SELECT pg_sleep({secs})"),
    slow_condition_sql: |secs| format!("(SELECT true FROM pg_sleep({secs}))"),
    one_schema: None,
    odd_table: "Odd \"Name\" 🐝",
    keyless_tables_editable: false,
    parallel_writes: true,
    enforces_lengths: true,
    copy_table_sql: copy_keyed_by_id,
    date_column_sql: |c| format!("{c} date"),
    repeat_sql: |text, n| format!("repeat({text}, {n})"),
    failing_when_sql: two_rows_when,
    long_text_type: "text",
    json_type: "jsonb",
    // dbee connects with application_name `dbee`; the two-argument terminate (Postgres
    // 14+) waits until the backend has exited.
    dbee_connections_sql: Some("SELECT pid FROM pg_stat_activity WHERE application_name = 'dbee'"),
    kill_sql: |pid| format!("SELECT pg_terminate_backend({pid}, 5000)"),
    slow_updates_sql: None,
    row_lock_waits_sql: |t| {
        format!(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND wait_event_type = 'Lock' AND query LIKE 'UPDATE \"scratch\".\"{t}\"%'"
        )
    },
    commit_wait: Some(CommitWait::AdvisoryLock),
    read_only_sql: ("SHOW default_transaction_read_only", "on"),
    connection_id_sql: "SELECT pg_backend_pid()",
    running_with_sql: |m| {
        format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND state = 'active' AND query LIKE '%{m}%'")
    },
    take_lock_sql: |key| format!("SELECT pg_advisory_xact_lock(hashtext('{key}'))"),
    waits_for_lock_sql: |key| format!("(SELECT true FROM pg_advisory_xact_lock_shared(hashtext('{key}')))"),
    json_type_label: "jsonb",
    plain_json_label: "json",
    json_sorts_keys: true,
    int_literal_class: "number",
    int_literal_type: "int4",
    failing_error: "more than one row returned by a subquery used as an expression",
};

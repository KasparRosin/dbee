//! MySQL's and MariaDB's test SQL.

use super::{copy_keyed_by_id, two_rows_when};
use crate::{Client, CommitWait, KindInfo};

/// What MySQL and MariaDB share.
const MYSQL: KindInfo = KindInfo {
    id: "mysql",
    product: "MySQL",
    dialect: "MySql",
    user: "root",
    database: "",
    client: None,
    sleep_sql: |secs| format!("SELECT SLEEP({secs})"),
    // A sleep in the WHERE clause or a subquery runs once per row; a derived table runs once.
    slow_condition_sql: |secs| format!("EXISTS (SELECT 1 FROM (SELECT SLEEP({secs}) AS z) AS d)"),
    one_schema: None,
    odd_table: "Odd \"Name\" é",
    keyless_tables_editable: false,
    parallel_writes: true,
    enforces_lengths: true,
    copy_table_sql: copy_keyed_by_id,
    date_column_sql: |c| format!("{c} date"),
    repeat_sql: |text, n| format!("repeat({text}, {n})"),
    failing_when_sql: two_rows_when,
    long_text_type: "LONGTEXT",
    json_type: "JSON",
    // The tests' own client connects over the socket (host `localhost`); dbee comes over TCP, so
    // these are dbee's connections while no other tool connects as root.
    dbee_connections_sql: Some("SELECT ID FROM information_schema.PROCESSLIST WHERE USER = 'root' AND HOST <> 'localhost' AND ID <> CONNECTION_ID()"),
    kill_sql: |id| format!("KILL {id}"),
    slow_updates_sql: |t, secs| {
        // While it sleeps, the session's statement is the trigger's, which names the table.
        let sql = format!("CREATE TRIGGER scratch.{t}_slow BEFORE UPDATE ON scratch.{t} FOR EACH ROW SET @dbee_slow_{t} = SLEEP({secs})");
        (sql, format!("TRIGGER IF EXISTS scratch.{t}_slow"))
    },
    row_lock_waits_sql: |t| {
        format!("SELECT COUNT(*) FROM information_schema.INNODB_TRX WHERE trx_state = 'LOCK WAIT' AND trx_query LIKE 'UPDATE `scratch`.`{t}`%'")
    },
    // MySQL has no lock that holds a COMMIT but not the update before it: the global read lock
    // holds both, and taken between the two it loses the race with a COMMIT sent at once.
    commit_wait: None,
    read_only_sql: ("SELECT @@SESSION.transaction_read_only", "1"),
    connection_id_sql: "SELECT CONNECTION_ID()",
    running_with_sql: |m| format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE INFO LIKE '%{m}%' AND ID <> CONNECTION_ID()"),
    take_lock_sql: |key| format!("SELECT GET_LOCK('{key}', 0)"),
    // Taken and given back at once, in a derived table, which runs once.
    waits_for_lock_sql: |key| format!("EXISTS (SELECT 1 FROM (SELECT GET_LOCK('{key}', 60) + RELEASE_LOCK('{key}') AS z) AS d)"),
    json_type_label: "json",
    plain_json_label: "json",
    json_sorts_keys: true,
    int_literal_class: "number",
    int_literal_type: "bigint",
    failing_error: "Subquery returns more than 1 row",
};

pub(crate) const INFO: KindInfo = KindInfo { product: "MySQL", id: "mysql", client: Some(Client::MySql("mysql")), ..MYSQL };

// MariaDB 10.11 has no `transaction_read_only`.
pub(crate) const MARIADB: KindInfo = KindInfo {
    product: "MariaDB",
    id: "mariadb",
    client: Some(Client::MySql("mariadb")),
    read_only_sql: ("SELECT @@SESSION.tx_read_only", "1"),
    commit_wait: Some(CommitWait::BackupStage),
    // Its JSON is a LONGTEXT with a json_valid check, kept as written.
    json_type_label: "longtext",
    plain_json_label: "longtext",
    json_sorts_keys: false,
    int_literal_type: "int",
    ..MYSQL
};

//! Each kind's test SQL ([`crate::Kind::info`]), in its engine's own file, and what the server
//! kinds share.

pub(crate) mod mysql;
pub(crate) mod postgres;
pub(crate) mod sqlite;

/// [`KindInfo::copy_table_sql`] for an engine that adds a key to a copy.
fn copy_keyed_by_id(t: &str, source: &str) -> String {
    format!("CREATE TABLE {t} AS SELECT * FROM {source}; ALTER TABLE {t} ADD PRIMARY KEY (id)")
}

/// [`KindInfo::failing_when_sql`] by a scalar subquery that returns two rows once `cond` holds.
fn two_rows_when(cond: &str) -> String {
    format!("(SELECT 1 UNION ALL SELECT 1 WHERE {cond}) > 0")
}

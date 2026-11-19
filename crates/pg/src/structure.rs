//! A table's indexes and triggers, read when asked: `pg_index` with its access method, each key
//! part (a column, or the expression `pg_get_indexdef` writes out for it) with its `DESC` bit from
//! `indoption`, and a partial index's condition from `pg_get_expr`; `pg_trigger` without the
//! triggers Postgres makes itself (`tgisinternal`: foreign keys' checks), its `tgtype` bits and
//! `pg_get_triggerdef`'s statement.
//!
//! The queries read columns that exist in Postgres 14 and every later version, run with the
//! catalog's `search_path` and budget ([`catalog::run`]), and name the table by literals. An
//! index's `INCLUDE` columns (past `indnkeyatts`) are not key parts and are left out.

use dbee_core::{IndexPart, TableRef, TableStructure, TriggerEvent, TriggerTiming};
use dbee_driver::catalog::{bad, number, owned, text};
use dbee_driver::structure::{self, IndexRow, TriggerRow};
use dbee_driver::{CatalogBudget, DbError};
use dbee_sql::Dialect;

use crate::catalog::{self, boolean};
use crate::connect::PgConnection;

/// One row per key part of each index of the table `{schema}`.`{table}` names.
const INDEXES_SQL: &str = "\
SELECT i.relname, x.indisunique::text, x.indisprimary::text, am.amname, k.ord::text, a.attname,
       CASE WHEN k.attnum = 0 THEN pg_catalog.pg_get_indexdef(x.indexrelid, k.ord::int4, true) END,
       (pg_catalog.int4(x.indoption[(k.ord - 1)::int4]) & 1 = 1)::text,
       pg_catalog.pg_get_expr(x.indpred, x.indrelid, true)
FROM pg_catalog.pg_index x
JOIN pg_catalog.pg_class c ON c.oid = x.indrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_class i ON i.oid = x.indexrelid
JOIN pg_catalog.pg_am am ON am.oid = i.relam
CROSS JOIN LATERAL pg_catalog.unnest(x.indkey::pg_catalog.int2[]) WITH ORDINALITY AS k(attnum, ord)
LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = x.indrelid AND a.attnum = k.attnum AND k.attnum > 0
WHERE n.nspname = {schema} AND c.relname = {table} AND k.ord <= x.indnkeyatts
ORDER BY i.relname, k.ord";

/// The table's own triggers.
const TRIGGERS_SQL: &str = "\
SELECT t.tgname, t.tgtype::text, pg_catalog.pg_get_triggerdef(t.oid, true)
FROM pg_catalog.pg_trigger t
JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = {schema} AND c.relname = {table} AND NOT t.tgisinternal
ORDER BY t.tgname";

/// `tgtype`'s bits (`pg_trigger.h`).
const ROW: u16 = 1 << 0;
const BEFORE: u16 = 1 << 1;
const INSERT: u16 = 1 << 2;
const DELETE: u16 = 1 << 3;
const UPDATE: u16 = 1 << 4;
const TRUNCATE: u16 = 1 << 5;
const INSTEAD: u16 = 1 << 6;

/// What `tgtype` says: the timing, the events, and whether it fires for each row.
fn trigger_type(t: u16) -> (TriggerTiming, Vec<TriggerEvent>, bool) {
    let timing = if t & INSTEAD != 0 {
        TriggerTiming::InsteadOf
    } else if t & BEFORE != 0 {
        TriggerTiming::Before
    } else {
        TriggerTiming::After
    };
    let events = [(INSERT, TriggerEvent::Insert), (UPDATE, TriggerEvent::Update), (DELETE, TriggerEvent::Delete), (TRUNCATE, TriggerEvent::Truncate)];
    let events = events.into_iter().filter(|(bit, _)| t & bit != 0).map(|(_, e)| e).collect();
    (timing, events, t & ROW != 0)
}

fn literal(name: &str) -> Result<String, DbError> {
    Dialect::Postgres.literal(name).map_err(|e| DbError::Other(format!("the table name cannot be used: {e}")))
}

/// The indexes and triggers of `table`.
pub async fn table_structure(conn: &PgConnection, table: &TableRef) -> Result<TableStructure, DbError> {
    let (schema, name) = (literal(&table.schema)?, literal(&table.name)?);
    let with_table = |sql: &str| sql.replace("{schema}", &schema).replace("{table}", &name);
    let budget = &mut CatalogBudget::default();
    let index_rows = catalog::run(conn, budget, &with_table(INDEXES_SQL)).await?;
    let trigger_rows = catalog::run(conn, budget, &with_table(TRIGGERS_SQL)).await?;

    let mut indexes = Vec::new();
    for row in &index_rows.rows {
        let nullable = |i: usize| row.get(i).cloned().ok_or_else(|| bad("index part"));
        indexes.push(IndexRow {
            name: owned(row, 0, "index name")?,
            unique: boolean(row, 1, "indisunique")?,
            primary: boolean(row, 2, "indisprimary")?,
            method: Some(owned(row, 3, "access method")?),
            predicate: nullable(8)?,
            position: number(row, 4, "index position")?,
            part: IndexPart { column: nullable(5)?, expression: nullable(6)?, descending: boolean(row, 7, "indoption")? },
        });
    }
    let mut triggers = Vec::new();
    for row in &trigger_rows.rows {
        let (timing, events, for_each_row) = trigger_type(number(row, 1, "tgtype")?);
        triggers.push(TriggerRow {
            name: owned(row, 0, "trigger name")?,
            timing: Some(timing),
            events,
            for_each_row: Some(for_each_row),
            definition: text(row, 2, "trigger definition")?.to_owned(),
        });
    }
    Ok(structure::build(indexes, triggers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tgtype_bits() {
        use TriggerEvent::*;
        assert_eq!(trigger_type(ROW | BEFORE | INSERT | UPDATE), (TriggerTiming::Before, vec![Insert, Update], true));
        assert_eq!(trigger_type(TRUNCATE), (TriggerTiming::After, vec![Truncate], false));
        assert_eq!(trigger_type(ROW | INSTEAD | DELETE), (TriggerTiming::InsteadOf, vec![Delete], true));
        assert_eq!(trigger_type(u16::MAX).1, vec![Insert, Update, Delete, Truncate]);
    }
}

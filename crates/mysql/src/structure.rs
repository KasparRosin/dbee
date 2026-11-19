//! A table's indexes and triggers, read when asked, from `information_schema`: `STATISTICS`, one
//! row per index part (`COLLATION` `D` is a descending part, `INDEX_TYPE` the kind), and
//! `TRIGGERS`, whose `ACTION_STATEMENT` is the trigger's body as the server keeps it.
//!
//! An expression part (MySQL 8.0.13's functional key parts) has no `COLUMN_NAME`; MySQL from
//! 8.0.13 writes it out in `STATISTICS.EXPRESSION`, which older servers and MariaDB do not have,
//! so the query asks for that column only where it exists. Neither engine has partial indexes.
//! Every trigger fires for each row. The queries run within the catalog's budget and name the
//! table by literals written with the MySQL dialect's literal writer.

use dbee_core::{IndexPart, TableRef, TableStructure};
use dbee_driver::catalog::{number, owned, text};
use dbee_driver::structure::{self, IndexRow, TriggerRow};
use dbee_driver::{CatalogBudget, DbError};
use dbee_sql::Dialect;

use crate::catalog::nullable;
use crate::connect::MySqlConnection;
use crate::flavor::Flavor;

/// The first MySQL with `STATISTICS.EXPRESSION`, as [`crate::flavor::version_num`].
const EXPRESSION_SINCE: u32 = 80013;

fn literal(name: &str) -> Result<String, DbError> {
    Dialect::MySql.literal(name).map_err(|e| DbError::Other(format!("the table name cannot be used: {e}")))
}

impl MySqlConnection {
    /// The indexes and triggers of `table`.
    pub async fn table_structure(&self, table: &TableRef) -> Result<TableStructure, DbError> {
        let (schema, name) = (literal(&table.schema)?, literal(&table.name)?);
        let expression = if self.flavor() == Flavor::MySql && self.info().version_num >= EXPRESSION_SINCE { "EXPRESSION" } else { "NULL" };
        let indexes_sql = format!(
            "SELECT INDEX_NAME, NON_UNIQUE, SEQ_IN_INDEX, COLUMN_NAME, COLLATION, INDEX_TYPE, {expression} \
             FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = {schema} AND TABLE_NAME = {name}"
        );
        let triggers_sql = format!(
            "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_ORIENTATION, ACTION_STATEMENT \
             FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = {schema} AND EVENT_OBJECT_TABLE = {name}"
        );
        let budget = &mut CatalogBudget::default();
        let index_rows = self.catalog_query(budget, &indexes_sql).await?;
        let trigger_rows = self.catalog_query(budget, &triggers_sql).await?;

        let mut indexes = Vec::new();
        for row in &index_rows.rows {
            let name = owned(row, 0, "index name")?;
            indexes.push(IndexRow {
                primary: name == "PRIMARY",
                name,
                unique: text(row, 1, "non unique")? == "0",
                method: nullable(row, 5, "index type")?.map(str::to_owned),
                predicate: None,
                position: number(row, 2, "index position")?,
                part: IndexPart {
                    column: nullable(row, 3, "index column")?.map(str::to_owned),
                    expression: nullable(row, 6, "index expression")?.map(str::to_owned),
                    descending: nullable(row, 4, "collation")? == Some("D"),
                },
            });
        }
        let mut triggers = Vec::new();
        for row in &trigger_rows.rows {
            let events = text(row, 2, "trigger event")?.split(',').filter_map(|e| structure::event(e.trim())).collect();
            triggers.push(TriggerRow {
                name: owned(row, 0, "trigger name")?,
                timing: structure::timing(text(row, 1, "trigger timing")?),
                events,
                for_each_row: nullable(row, 3, "trigger orientation")?.map(|o| o.eq_ignore_ascii_case("ROW")),
                definition: nullable(row, 4, "trigger action")?.unwrap_or_default().to_owned(),
            });
        }
        Ok(structure::build(indexes, triggers))
    }
}

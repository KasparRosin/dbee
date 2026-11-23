//! A table's indexes, triggers, column defaults and foreign key actions, read when asked, from
//! `information_schema`: `STATISTICS`, one row per index part (`COLLATION` `D` is a descending
//! part, `INDEX_TYPE` the kind); `TRIGGERS`, whose `ACTION_STATEMENT` is the trigger's body as the
//! server keeps it; `COLUMNS` for the defaults ([`default_text`]); `REFERENTIAL_CONSTRAINTS`
//! with `KEY_COLUMN_USAGE` for each foreign key's `DELETE_RULE` and `UPDATE_RULE`.
//!
//! The engines write a default differently: MySQL gives a string default's bare value (`pending`)
//! and MariaDB writes SQL (`'pending'`, `NULL` for none), so MySQL's strings are quoted here and
//! read the same on both.
//!
//! An expression part (MySQL 8.0.13's functional key parts) has no `COLUMN_NAME`; MySQL from
//! 8.0.13 writes it out in `STATISTICS.EXPRESSION`, which older servers and MariaDB do not have,
//! so the query asks for that column only where it exists. Neither engine has partial indexes.
//! Every trigger fires for each row. The queries run within the catalog's budget and name the
//! table by literals written with the MySQL dialect's literal writer.

use dbee_core::{ColumnDefault, ForeignKeyActions, IndexPart, TableRef, TableStructure};
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
    /// The indexes, triggers, defaults and foreign key actions of `table`.
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
        let defaults_sql = format!(
            "SELECT COLUMN_NAME, COLUMN_DEFAULT, EXTRA, GENERATION_EXPRESSION, DATA_TYPE \
             FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {schema} AND TABLE_NAME = {name} ORDER BY ORDINAL_POSITION"
        );
        let actions_sql = format!(
            "SELECT r.CONSTRAINT_NAME, k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, k.COLUMN_NAME, r.DELETE_RULE, r.UPDATE_RULE \
             FROM information_schema.REFERENTIAL_CONSTRAINTS r \
             JOIN information_schema.KEY_COLUMN_USAGE k ON k.CONSTRAINT_SCHEMA = r.CONSTRAINT_SCHEMA \
               AND k.CONSTRAINT_NAME = r.CONSTRAINT_NAME AND k.TABLE_NAME = r.TABLE_NAME \
             WHERE r.CONSTRAINT_SCHEMA = {schema} AND r.TABLE_NAME = {name} AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY r.CONSTRAINT_NAME, k.ORDINAL_POSITION"
        );
        let budget = &mut CatalogBudget::default();
        let index_rows = self.catalog_query(budget, &indexes_sql).await?;
        let trigger_rows = self.catalog_query(budget, &triggers_sql).await?;
        let default_rows = self.catalog_query(budget, &defaults_sql).await?;
        let action_rows = self.catalog_query(budget, &actions_sql).await?;

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
        let mut defaults = Vec::new();
        for row in &default_rows.rows {
            let column = Column {
                default: nullable(row, 1, "column default")?,
                extra: nullable(row, 2, "column extra")?.unwrap_or_default(),
                generation: nullable(row, 3, "generation expression")?.unwrap_or_default(),
                data_type: nullable(row, 4, "data type")?.unwrap_or_default(),
            };
            defaults.push(ColumnDefault { column: owned(row, 0, "column name")?, default: default_text(self.flavor(), &column) });
        }
        let mut actions: Vec<(String, ForeignKeyActions)> = Vec::new();
        for row in &action_rows.rows {
            let id = owned(row, 0, "foreign key")?;
            let column = owned(row, 3, "foreign key column")?;
            match actions.last_mut() {
                Some((last, key)) if *last == id => key.columns.push(column),
                _ => {
                    let key = ForeignKeyActions {
                        columns: vec![column],
                        target: TableRef { schema: owned(row, 1, "referenced schema")?, name: owned(row, 2, "referenced table")? },
                        on_delete: nullable(row, 4, "delete rule")?.and_then(structure::fk_action),
                        on_update: nullable(row, 5, "update rule")?.and_then(structure::fk_action),
                    };
                    actions.push((id, key));
                }
            }
        }
        let actions = actions.into_iter().map(|(_, key)| key).collect();
        Ok(structure::with_keys(structure::build(indexes, triggers), defaults, actions))
    }
}

/// What `information_schema.COLUMNS` says of a column's default.
struct Column<'a> {
    default: Option<&'a str>,
    extra: &'a str,
    generation: &'a str,
    data_type: &'a str,
}

/// The types whose default MySQL writes as it is read: numbers, bits and years.
const AS_GIVEN: [&str; 14] =
    ["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "decimal", "numeric", "dec", "float", "double", "real", "bit", "year"];

/// A column's default as the structure view writes it: `auto_increment`; a generated column's
/// `generated always as (<expression>)`; else the default, with a MySQL string quoted as a SQL
/// literal (`'` doubled) unless it is an expression (`DEFAULT_GENERATED`), a number, a bit or a
/// year, or starts with `CURRENT_TIMESTAMP`; MariaDB's `NULL` is none.
fn default_text(flavor: Flavor, c: &Column) -> Option<String> {
    let extra = c.extra.to_ascii_lowercase();
    if extra.contains("auto_increment") {
        return Some("auto_increment".to_owned());
    }
    if !c.generation.is_empty() {
        return Some(format!("generated always as ({})", c.generation));
    }
    let default = c.default?;
    match flavor {
        Flavor::MariaDb if default == "NULL" => None,
        Flavor::MariaDb => Some(default.to_owned()),
        Flavor::MySql => {
            let as_given = extra.contains("default_generated")
                || AS_GIVEN.iter().any(|t| c.data_type.eq_ignore_ascii_case(t))
                || default.get(..17).is_some_and(|head| head.eq_ignore_ascii_case("CURRENT_TIMESTAMP"));
            Some(if as_given { default.to_owned() } else { format!("'{}'", default.replace('\'', "''")) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column<'a>(default: Option<&'a str>, extra: &'a str, generation: &'a str, data_type: &'a str) -> Column<'a> {
        Column { default, extra, generation, data_type }
    }

    #[test]
    fn defaults_as_each_engine_writes_them() {
        use Flavor::*;
        let cases = [
            (MySql, column(None, "auto_increment", "", "bigint"), Some("auto_increment")),
            (MySql, column(None, "VIRTUAL GENERATED", "(`n` * 2)", "int"), Some("generated always as ((`n` * 2))")),
            (MySql, column(Some("it's"), "", "", "varchar"), Some("'it''s'")),
            (MySql, column(Some(""), "", "", "varchar"), Some("''")),
            (MySql, column(Some("5"), "", "", "INT"), Some("5")),
            (MySql, column(Some("current_timestamp(3)"), "on update", "", "datetime"), Some("current_timestamp(3)")),
            (MySql, column(Some("(`n` + 1)"), "DEFAULT_GENERATED", "", "int"), Some("(`n` + 1)")),
            (MySql, column(Some("é"), "", "", "varchar"), Some("'é'")),
            (MySql, column(None, "", "", "varchar"), None),
            (MariaDb, column(Some("NULL"), "", "", "varchar"), None),
            (MariaDb, column(Some("'NULL'"), "", "", "varchar"), Some("'NULL'")),
            (MariaDb, column(Some("'pending'"), "", "", "varchar"), Some("'pending'")),
            (MariaDb, column(None, "VIRTUAL GENERATED", "`n` * 2", "int"), Some("generated always as (`n` * 2)")),
        ];
        for (flavor, c, want) in cases {
            assert_eq!(default_text(flavor, &c).as_deref(), want, "{flavor:?} {:?}", c.default);
        }
    }
}

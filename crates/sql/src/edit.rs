//! UPDATE, INSERT and DELETE statements for staged edits.
//!
//! Column types come from the catalog's `ColumnInfo`, never from user text.

use crate::{Dialect, SqlError};
use dbee_core::{ColumnInfo, TableInfo};
use serde::{Deserialize, Serialize};

/// A value to write: SQL NULL or text cast to the column type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NewValue {
    Null,
    Text(String),
}

/// A value for `SET` or `VALUES`; on Postgres cast so that the assignment to the column refuses
/// a value longer than its length instead of the cast cutting it.
fn value_sql(d: Dialect, column: &ColumnInfo, value: &NewValue) -> Result<String, SqlError> {
    match value {
        NewValue::Null => Ok("NULL".to_string()),
        NewValue::Text(text) => d.value(column, text),
    }
}

/// The row's key, each value cast to the column's own type: they are values the server sent, so
/// they fit.
fn key_sql(d: Dialect, key: &[(&ColumnInfo, &str)]) -> Result<String, SqlError> {
    if key.is_empty() {
        return Err(SqlError::NoKey);
    }
    let parts = key.iter().map(|(c, v)| Ok(format!("{} = {}", d.ident(&c.name)?, d.key_value(c, v)?))).collect::<Result<Vec<_>, SqlError>>()?;
    Ok(parts.join(" AND "))
}

pub fn update_sql(d: Dialect, table: &TableInfo, key: &[(&ColumnInfo, &str)], changes: &[(&ColumnInfo, &NewValue)]) -> Result<String, SqlError> {
    if changes.is_empty() {
        return Err(SqlError::NoChanges);
    }
    let sets = changes.iter().map(|(c, v)| Ok(format!("{} = {}", d.ident(&c.name)?, value_sql(d, c, v)?))).collect::<Result<Vec<_>, SqlError>>()?;
    Ok(format!("UPDATE {} SET {} WHERE {}", d.table(&table.table)?, sets.join(", "), key_sql(d, key)?))
}

pub fn insert_sql(d: Dialect, table: &TableInfo, values: &[(&ColumnInfo, &NewValue)]) -> Result<String, SqlError> {
    let target = d.table(&table.table)?;
    if values.is_empty() {
        return Ok(d.insert_defaults(&target));
    }
    let names = values.iter().map(|(c, _)| d.ident(&c.name)).collect::<Result<Vec<_>, _>>()?;
    let vals = values.iter().map(|(c, v)| value_sql(d, c, v)).collect::<Result<Vec<_>, _>>()?;
    Ok(format!("INSERT INTO {target} ({}) VALUES ({})", names.join(", "), vals.join(", ")))
}

pub fn delete_sql(d: Dialect, table: &TableInfo, key: &[(&ColumnInfo, &str)]) -> Result<String, SqlError> {
    Ok(format!("DELETE FROM {} WHERE {}", d.table(&table.table)?, key_sql(d, key)?))
}

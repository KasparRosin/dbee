//! UPDATE, INSERT and DELETE statements for staged edits.
//!
//! Column types come from the catalog's `ColumnInfo`, never from user text.

use crate::{SqlError, quote};
use dbee_core::{ColumnInfo, TableInfo};
use serde::{Deserialize, Serialize};

/// A value to write: SQL NULL or text cast to the column type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NewValue {
    Null,
    Text(String),
}

/// A value for `SET` or `VALUES`, cast to [`ColumnInfo::write_type_sql`] so that the assignment
/// to the column refuses a value longer than its length instead of the cast cutting it.
fn value_sql(column: &ColumnInfo, value: &NewValue) -> Result<String, SqlError> {
    match value {
        NewValue::Null => Ok("NULL".to_string()),
        NewValue::Text(text) => quote::typed(text, column.write_type_sql()),
    }
}

/// The row's key, each value cast to the column's own type: they are values the server sent, so
/// they fit.
fn key_sql(key: &[(&ColumnInfo, &str)]) -> Result<String, SqlError> {
    if key.is_empty() {
        return Err(SqlError::NoKey);
    }
    let parts = key
        .iter()
        .map(|(c, v)| Ok(format!("{} = {}", quote::ident(&c.name)?, quote::typed(v, &c.type_sql)?)))
        .collect::<Result<Vec<_>, SqlError>>()?;
    Ok(parts.join(" AND "))
}

pub fn update_sql(table: &TableInfo, key: &[(&ColumnInfo, &str)], changes: &[(&ColumnInfo, &NewValue)]) -> Result<String, SqlError> {
    if changes.is_empty() {
        return Err(SqlError::NoChanges);
    }
    let sets = changes.iter().map(|(c, v)| Ok(format!("{} = {}", quote::ident(&c.name)?, value_sql(c, v)?))).collect::<Result<Vec<_>, SqlError>>()?;
    Ok(format!("UPDATE {} SET {} WHERE {}", quote::table(&table.table)?, sets.join(", "), key_sql(key)?))
}

pub fn insert_sql(table: &TableInfo, values: &[(&ColumnInfo, &NewValue)]) -> Result<String, SqlError> {
    let target = quote::table(&table.table)?;
    if values.is_empty() {
        return Ok(format!("INSERT INTO {target} DEFAULT VALUES"));
    }
    let names = values.iter().map(|(c, _)| quote::ident(&c.name)).collect::<Result<Vec<_>, _>>()?;
    let vals = values.iter().map(|(c, v)| value_sql(c, v)).collect::<Result<Vec<_>, _>>()?;
    Ok(format!("INSERT INTO {target} ({}) VALUES ({})", names.join(", "), vals.join(", ")))
}

pub fn delete_sql(table: &TableInfo, key: &[(&ColumnInfo, &str)]) -> Result<String, SqlError> {
    Ok(format!("DELETE FROM {} WHERE {}", quote::table(&table.table)?, key_sql(key)?))
}

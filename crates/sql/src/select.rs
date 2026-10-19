//! The page and count statements for browsing a table.

use crate::filter::{Condition, Sort, SortDir, where_sql};
use crate::{SqlError, quote};
use dbee_core::TableInfo;

/// Rows per page. The page statement asks for one more to learn whether a next page exists.
pub const PAGE_SIZE: u64 = 100;

fn where_clause(table: &TableInfo, cond: &Condition) -> Result<String, SqlError> {
    Ok(where_sql(cond, table)?.map(|w| format!(" WHERE {w}")).unwrap_or_default())
}

/// `SELECT <columns> FROM <table> [WHERE ..] [ORDER BY ..] LIMIT 101 OFFSET <page * 100>`.
pub fn page_sql(table: &TableInfo, cond: &Condition, sort: Option<&Sort>, page: u64) -> Result<String, SqlError> {
    let columns = table.columns.iter().map(|c| quote::ident(&c.name)).collect::<Result<Vec<_>, _>>()?.join(", ");
    let mut order = Vec::new();
    let mut sorted_by = None;
    if let Some(sort) = sort {
        let column = table.column(&sort.column).ok_or_else(|| SqlError::UnknownColumn(sort.column.clone()))?;
        let dir = match sort.dir {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        };
        order.push(format!("{} {dir}", quote::ident(&column.name)?));
        sorted_by = Some(column.name.as_str());
    }
    for key in table.row_key().unwrap_or_default() {
        if Some(key.name.as_str()) != sorted_by {
            order.push(quote::ident(&key.name)?);
        }
    }
    let order = if order.is_empty() { String::new() } else { format!(" ORDER BY {}", order.join(", ")) };
    let offset = page.saturating_mul(PAGE_SIZE);
    Ok(format!("SELECT {columns} FROM {}{}{order} LIMIT {} OFFSET {offset}", quote::table(&table.table)?, where_clause(table, cond)?, PAGE_SIZE + 1,))
}

/// `SELECT count(*) FROM <table> [WHERE ..]`.
pub fn count_sql(table: &TableInfo, cond: &Condition) -> Result<String, SqlError> {
    Ok(format!("SELECT count(*) FROM {}{}", quote::table(&table.table)?, where_clause(table, cond)?))
}

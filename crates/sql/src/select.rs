//! The page and count statements for browsing a table.

use crate::dialect::Paging;
use crate::filter::{Condition, Sort, SortDir, where_sql};
use crate::{Dialect, SqlError};
use dbee_core::TableInfo;

/// Rows per page. The page statement asks for one more to learn whether a next page exists.
pub const PAGE_SIZE: u64 = 100;

fn where_clause(d: Dialect, table: &TableInfo, cond: &Condition) -> Result<String, SqlError> {
    Ok(where_sql(d, cond, table)?.map(|w| format!(" WHERE {w}")).unwrap_or_default())
}

/// `SELECT <columns> FROM <table> [WHERE ..] [ORDER BY ..] LIMIT 101 OFFSET <page * 100>`, the
/// columns being the table's [`page_columns`](TableInfo::page_columns). The sort is by a visible
/// column.
pub fn page_sql(d: Dialect, table: &TableInfo, cond: &Condition, sort: Option<&Sort>, page: u64) -> Result<String, SqlError> {
    let columns = table.page_columns().iter().map(|c| d.ident(&c.name)).collect::<Result<Vec<_>, _>>()?.join(", ");
    let mut order = Vec::new();
    let mut sorted_by = None;
    if let Some(sort) = sort {
        let column = table.visible_column(&sort.column).ok_or_else(|| SqlError::UnknownColumn(sort.column.clone()))?;
        let dir = match sort.dir {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        };
        order.push(format!("{} {dir}", d.ident(&column.name)?));
        sorted_by = Some(column.name.as_str());
    }
    for key in table.row_key().unwrap_or_default() {
        if Some(key.name.as_str()) != sorted_by {
            order.push(d.ident(&key.name)?);
        }
    }
    let order = if order.is_empty() { String::new() } else { format!(" ORDER BY {}", order.join(", ")) };
    let offset = page.saturating_mul(PAGE_SIZE);
    let paging = match d.rules().paging {
        Paging::LimitOffset => format!(" LIMIT {} OFFSET {offset}", PAGE_SIZE + 1),
    };
    Ok(format!("SELECT {columns} FROM {}{}{order}{paging}", d.table(&table.table)?, where_clause(d, table, cond)?))
}

/// `SELECT count(*) FROM <table> [WHERE ..]`, `count(*)` as the engine's rules write it.
pub fn count_sql(d: Dialect, table: &TableInfo, cond: &Condition) -> Result<String, SqlError> {
    Ok(format!("SELECT {} FROM {}{}", d.rules().count_all, d.table(&table.table)?, where_clause(d, table, cond)?))
}

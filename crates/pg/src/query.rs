//! Running SQL text over the simple-query protocol: values come back as text, exactly as
//! Postgres prints them.

use dbee_core::{Cell, Column, ResultSet};
use futures_util::StreamExt;
use tokio_postgres::SimpleQueryMessage;

use crate::{PgConnection, PgError};

/// Caps on what one run keeps: rows, and bytes of cell text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_rows: usize,
    pub max_bytes: usize,
}

/// The query editor's caps: 10,000 rows or 256 MiB of cell text.
pub const QUERY_LIMITS: Limits = Limits { max_rows: 10_000, max_bytes: 256 * 1024 * 1024 };

/// Run `sql` (one or more statements) as one simple-query message, holding the connection lock.
///
/// The result holds the last row-returning statement's columns (untyped) and rows;
/// `rows_affected` is the count of the last `CommandComplete`. The first failing statement ends
/// the run with its error. `limits` is not applied yet.
pub async fn run_untyped(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, PgError> {
    let _ = limits;
    let _guard = conn.lock().await;
    let stream = conn.client().simple_query_raw(sql).await.map_err(|e| conn.error(&e))?;
    let mut stream = std::pin::pin!(stream);
    let mut result = ResultSet { complete: true, ..ResultSet::default() };
    while let Some(message) = stream.next().await {
        match message.map_err(|e| conn.error(&e))? {
            SimpleQueryMessage::RowDescription(columns) => {
                result.columns = columns.iter().map(|c| Column::untyped(c.name())).collect();
                result.rows.clear();
                result.rows_seen = 0;
            }
            SimpleQueryMessage::Row(row) => {
                let mut cells: Vec<Cell> = Vec::with_capacity(row.len());
                for i in 0..row.len() {
                    let value = row.try_get(i).map_err(|_| PgError::Other("the server sent a value that is not valid UTF-8".into()))?;
                    cells.push(value.map(str::to_owned));
                }
                result.rows.push(cells);
                result.rows_seen = result.rows_seen.saturating_add(1);
            }
            SimpleQueryMessage::CommandComplete(n) => result.rows_affected = Some(n),
            _ => {}
        }
    }
    Ok(result)
}

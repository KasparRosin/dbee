//! Running SQL text over the simple-query protocol: values come back as text, exactly as
//! Postgres prints them.

use dbee_core::{Cell, Column, ResultSet, TypeClass};
use futures_util::StreamExt;
use tokio_postgres::SimpleQueryMessage;
use tokio_postgres::types::{Kind, Type};

use crate::{PgConnection, PgError};

/// Caps on what one run keeps: rows, and bytes of cell text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_rows: usize,
    pub max_bytes: usize,
}

/// The query editor's caps: 10,000 rows or 256 MiB of cell text.
pub const QUERY_LIMITS: Limits = Limits { max_rows: 10_000, max_bytes: 256 * 1024 * 1024 };

/// A table page: 100 rows plus one, so the caller can tell whether another page exists.
pub const PAGE_LIMITS: Limits = Limits { max_rows: 101, max_bytes: 256 * 1024 * 1024 };

/// Run `sql` (one or more statements) as one simple-query message, holding the connection lock.
///
/// The result holds the last row-returning statement's columns (untyped) and rows;
/// `rows_affected` is the count of the last `CommandComplete`. The first failing statement ends
/// the run with its error. Rows past `limits` are still read (so the connection stays in sync)
/// and counted in `rows_seen`, but not kept, and `complete` becomes false. The byte cap counts
/// the cell text kept.
pub async fn run_untyped(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, PgError> {
    let _guard = conn.lock().await?;
    run_locked(conn, sql, limits).await
}

/// [`run_untyped`] for a caller that already holds the connection lock (an apply, which keeps
/// it from `BEGIN` to `COMMIT`).
pub(crate) async fn run_locked(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, PgError> {
    let stream = conn.client().simple_query_raw(sql).await.map_err(|e| conn.error(&e))?;
    let mut stream = std::pin::pin!(stream);
    let mut result = ResultSet { complete: true, ..ResultSet::default() };
    let mut bytes: usize = 0;
    while let Some(message) = stream.next().await {
        match message.map_err(|e| conn.error(&e))? {
            SimpleQueryMessage::RowDescription(columns) => {
                result.columns = columns.iter().map(|c| Column::untyped(c.name())).collect();
                result.rows.clear();
                result.rows_seen = 0;
                result.complete = true;
                bytes = 0;
            }
            SimpleQueryMessage::Row(row) => {
                result.rows_seen = result.rows_seen.saturating_add(1);
                if !result.complete {
                    continue;
                }
                let mut cells: Vec<Cell> = Vec::with_capacity(row.len());
                let mut row_bytes: usize = 0;
                for i in 0..row.len() {
                    let value = row.try_get(i).map_err(|_| PgError::Other("the server sent a value that is not valid UTF-8".into()))?;
                    row_bytes = row_bytes.saturating_add(value.map_or(0, str::len));
                    cells.push(value.map(str::to_owned));
                }
                if result.rows.len() >= limits.max_rows || bytes.saturating_add(row_bytes) > limits.max_bytes {
                    result.complete = false;
                    continue;
                }
                bytes += row_bytes;
                result.rows.push(cells);
            }
            SimpleQueryMessage::CommandComplete(n) => result.rows_affected = Some(n),
            _ => {}
        }
    }
    Ok(result)
}

/// The columns `sql` would return, typed by name (`base_type` and `type_label` are the type's
/// name), found by preparing it on the connection; the statement is closed again. `None` on any
/// error (several statements, a syntax error, a lost connection): the connection stays usable.
pub async fn describe(conn: &PgConnection, sql: &str) -> Option<Vec<Column>> {
    let Ok(_guard) = conn.lock().await else { return None };
    let statement = conn.client().prepare(sql).await.ok()?;
    let columns = statement
        .columns()
        .iter()
        .map(|c| {
            let base = base_type(c.type_());
            Column {
                name: c.name().to_owned(),
                base_type: base.name().to_owned(),
                type_label: c.type_().name().to_owned(),
                class: class_of_type(base),
            }
        })
        .collect();
    // Dropping the statement sends its Close ahead of the next message on this connection.
    drop(statement);
    Some(columns)
}

/// The type a domain is over (through domains over domains), else the type itself.
fn base_type(ty: &Type) -> &Type {
    match ty.kind() {
        Kind::Domain(inner) => base_type(inner),
        _ => ty,
    }
}

/// The class of a prepared statement's column, which carries a type but no `typcategory`: the
/// category of the built-in types by name (`typcategory` as `pg_type` lists it), and `citext` by
/// its name. Arrays, enums, composites, ranges and every other type are `Other`.
fn class_of_type(ty: &Type) -> TypeClass {
    if !matches!(ty.kind(), Kind::Simple) {
        return TypeClass::Other;
    }
    let category = match ty.name() {
        "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "money" | "oid" | "regproc" | "regprocedure" | "regoper" | "regoperator"
        | "regclass" | "regtype" | "regconfig" | "regdictionary" | "regrole" | "regnamespace" | "regcollation" => 'N',
        "bool" => 'B',
        "text" | "varchar" | "bpchar" | "name" | "xml" | "citext" => 'S',
        "date" | "time" | "timetz" | "timestamp" | "timestamptz" => 'D',
        _ => 'U',
    };
    crate::catalog::class_for(ty.name(), category)
}

/// [`run_untyped`], then (for a single statement that returned columns) [`describe`] to type
/// them. Running comes first so the user always sees the statement's own error and a failed
/// prepare cannot abort a transaction they opened; the described columns replace the untyped
/// ones when the counts match.
pub async fn run(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, PgError> {
    let mut result = run_untyped(conn, sql, limits).await?;
    if !result.columns.is_empty()
        && dbee_sql::lexer::statements(dbee_sql::Dialect::Postgres, sql).len() == 1
        && let Some(columns) = describe(conn, sql).await
        && columns.len() == result.columns.len()
    {
        result.columns = columns;
    }
    Ok(result)
}

/// The first cell of the first row as an unsigned integer (a count).
pub async fn scalar_u64(conn: &PgConnection, sql: &str) -> Result<u64, PgError> {
    let result = run_untyped(conn, sql, Limits { max_rows: 1, ..QUERY_LIMITS }).await?;
    let cell = result.rows.first().and_then(|row| row.first()).and_then(|c| c.as_deref());
    cell.and_then(|text| text.trim().parse::<u64>().ok()).ok_or_else(|| PgError::Other("the query did not return a count".into()))
}

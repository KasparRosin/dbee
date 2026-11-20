//! Running SQL text over the simple-query protocol: values come back as text, exactly as
//! Postgres prints them.

use dbee_core::{Cell, Column, ResultSet, TypeClass};
use dbee_driver::{DbError, Limits, QUERY_LIMITS, ROW_COST, cell_cost, column_cost, kept_text};
use futures_util::StreamExt;
use tokio_postgres::types::{Kind, Type};
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::{PgConnection, guard};

/// The most columns a Postgres query returns ("target lists can have at most 1664 entries").
pub const MAX_COLUMNS: usize = 1664;

/// The longest name Postgres has, of a column, a type or a schema: longer ones it cuts to this
/// (measured on both test servers), so only a hostile server sends one.
pub(crate) const MAX_NAME: usize = 63;

/// Run `sql` (one or more statements) as one simple-query message, holding the connection lock.
///
/// The result holds the last row-returning statement's columns (untyped) and rows;
/// `rows_affected` is the count of the last `CommandComplete`. The first failing statement ends
/// the run with its error. Rows past `limits` are still read (so the connection stays in sync)
/// and counted in `rows_seen`, but not kept, and `complete` becomes false. The byte cap counts
/// what the columns and the rows kept hold ([`dbee_driver::cell_cost`]). A result of more than
/// [`MAX_COLUMNS`] columns, or whose columns do not fit in `limits`, ends the run in an error once
/// its answer is read to the end. What dbee refuses is not sent, and a read-only connection's mode
/// is read back after it ([`crate::guard`]).
pub async fn run_untyped(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
    let _guard = conn.lock().await?;
    guard::before(conn, sql).await?;
    let ran = run_locked(conn, sql, limits).await;
    guard::after(conn, ran).await
}

/// [`run_untyped`] for a caller that already holds the connection lock (an apply, which keeps
/// it from `BEGIN` to `COMMIT`).
pub(crate) async fn run_locked(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
    let stream = conn.client().simple_query_raw(sql).await.map_err(|e| conn.error(&e))?;
    let mut stream = std::pin::pin!(stream);
    let mut result = ResultSet { complete: true, ..ResultSet::default() };
    let mut bytes: usize = 0;
    let mut refused = None;
    while let Some(message) = stream.next().await {
        let message = message.map_err(|e| conn.error(&e))?;
        // Read to the end, so the next statement finds the session in step.
        if refused.is_some() {
            continue;
        }
        match message {
            SimpleQueryMessage::RowDescription(columns) if columns.len() > MAX_COLUMNS => {
                refused = Some(DbError::Other(format!(
                    "the server sent a result of {} columns, more than the {MAX_COLUMNS} a Postgres query can return",
                    columns.len()
                )));
            }
            SimpleQueryMessage::RowDescription(columns) => {
                result.columns = columns.iter().map(|c| Column::untyped(&kept_text(c.name(), MAX_NAME))).collect();
                result.rows.clear();
                result.rows_seen = 0;
                result.complete = true;
                bytes = result.columns.iter().map(column_cost).fold(0, usize::saturating_add);
                if bytes > limits.max_bytes {
                    refused = Some(DbError::TooLarge { left: limits.max_bytes });
                }
            }
            SimpleQueryMessage::Row(row) => {
                result.rows_seen = result.rows_seen.saturating_add(1);
                if !result.complete {
                    continue;
                }
                let not_utf8 = |_| DbError::Other("the server sent a value that is not valid UTF-8".into());
                let mut row_bytes = ROW_COST;
                for i in 0..row.len() {
                    row_bytes = row_bytes.saturating_add(cell_cost(row.try_get(i).map_err(not_utf8)?.map(str::len)));
                }
                if result.rows.len() >= limits.max_rows || bytes.saturating_add(row_bytes) > limits.max_bytes {
                    result.complete = false;
                    continue;
                }
                bytes = bytes.saturating_add(row_bytes);
                let cells =
                    (0..row.len()).map(|i| row.try_get(i).map(|v| v.map(str::to_owned)).map_err(not_utf8)).collect::<Result<Vec<Cell>, _>>()?;
                result.rows.push(cells);
            }
            SimpleQueryMessage::CommandComplete(n) => result.rows_affected = Some(n),
            _ => {}
        }
    }
    match refused {
        Some(e) => Err(e),
        None => Ok(result),
    }
}

/// dbee's own reads, of a known shape: the first cell of every row of every statement of `sql`,
/// for the session setup and the read-only guard. A result of other than one column, or past
/// `limits`, which only a hostile server sends, ends the read in an error; `map` maps the errors
/// of the session.
pub(crate) async fn own_read(
    client: &Client,
    sql: &str,
    limits: Limits,
    map: impl Fn(&tokio_postgres::Error) -> DbError,
) -> Result<Vec<Option<String>>, DbError> {
    const CHECK: &str = "Check that this is the server you meant to reach.";
    let stream = client.simple_query_raw(sql).await.map_err(|e| map(&e))?;
    let mut stream = std::pin::pin!(stream);
    let mut cells = Vec::new();
    let mut bytes: usize = 0;
    while let Some(message) = stream.next().await {
        match message.map_err(|e| map(&e))? {
            SimpleQueryMessage::RowDescription(columns) if columns.len() != 1 => {
                return Err(DbError::Other(format!(
                    "the server answered a query of dbee's own with {} columns, where it asked for 1. {CHECK}",
                    columns.len()
                )));
            }
            SimpleQueryMessage::Row(row) => {
                let cell = row.try_get(0).map_err(|_| DbError::Other("the server sent a value that is not valid UTF-8".into()))?;
                bytes = bytes.saturating_add(ROW_COST).saturating_add(cell_cost(cell.map(str::len)));
                if cells.len() >= limits.max_rows || bytes > limits.max_bytes {
                    return Err(DbError::Other(format!(
                        "the server answered a query of dbee's own with more than such an answer holds, so dbee stopped reading. {CHECK}"
                    )));
                }
                cells.push(cell.map(str::to_owned));
            }
            _ => {}
        }
    }
    Ok(cells)
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
                name: kept_text(c.name(), MAX_NAME),
                base_type: kept_text(base.name(), MAX_NAME),
                type_label: kept_text(c.type_().name(), MAX_NAME),
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

/// Whether the last statement of `sql` is a command Postgres counts rows for (its tag ends in a
/// count: `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `SELECT`, `COPY`, `MOVE`, `FETCH`, and what reads
/// as `SELECT`: `WITH`, `TABLE`, `VALUES`). tokio-postgres hides the tag and says 0 for the others
/// (`BEGIN`, `SET`, `CREATE TABLE`), which have no count.
fn counts_rows(sql: &str) -> bool {
    use dbee_sql::lexer::{TokenKind, statements, tokenize};
    let d = dbee_sql::Dialect::Postgres;
    let Some(last) = statements(d, sql).pop() else { return true };
    let text = sql.get(last).unwrap_or_default();
    let first = tokenize(d, text).into_iter().find(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment));
    let Some(word) = first.and_then(|t| text.get(t.range)) else { return true };
    ["INSERT", "UPDATE", "DELETE", "MERGE", "SELECT", "COPY", "MOVE", "FETCH", "WITH", "TABLE", "VALUES"]
        .iter()
        .any(|w| w.eq_ignore_ascii_case(word))
        // A statement in parentheses, `(SELECT 1)`, reads rows too.
        || word == "("
}

/// [`run_untyped`], then (for a single statement that returned columns) [`describe`] to type
/// them. Running comes first so the user always sees the statement's own error and a failed
/// prepare cannot abort a transaction they opened; the described columns replace the untyped
/// ones when the counts match.
pub async fn run(conn: &PgConnection, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
    let mut result = run_untyped(conn, sql, limits).await?;
    if !counts_rows(sql) {
        result.rows_affected = None;
    }
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
pub async fn scalar_u64(conn: &PgConnection, sql: &str) -> Result<u64, DbError> {
    let result = run_untyped(conn, sql, Limits { max_rows: 1, ..QUERY_LIMITS }).await?;
    let cell = result.rows.first().and_then(|row| row.first()).and_then(|c| c.as_deref());
    cell.and_then(|text| text.trim().parse::<u64>().ok()).ok_or_else(|| DbError::Other("the query did not return a count".into()))
}

//! Running SQL text: each statement prepared and stepped in turn, within the row and byte caps,
//! with values as text by fixed rules ([`crate::values`]).

use dbee_core::{Column, ResultSet};
use dbee_driver::{DbError, Limits, QUERY_LIMITS};
use dbee_sql::Dialect;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Statement};

use crate::connect::{Running, SqliteConnection};
use crate::errors::{self, PARAMETERS};
use crate::values::{self, CellText};

/// SQLite ends a statement at a NUL outside a string literal, and runs the part before it.
const NUL: &str = "the statement holds a NUL character (U+0000), which SQLite would take as its end";

/// rusqlite cannot read a result column's name or declared type that is not UTF-8.
const NOT_UTF8: &str = "the database has a name or a type that is not valid UTF-8, so dbee cannot show this result";

/// What a result's columns carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Columns {
    /// Names, declared types and classes.
    Typed,
    /// Names.
    Untyped,
    /// Nothing but their number, for callers that read the values only.
    Unnamed,
}

impl SqliteConnection {
    /// Run `sql`, one statement after another, holding the connection's turn.
    ///
    /// The result holds the last row-returning statement's columns, typed from their declared
    /// types (an expression has none), and rows; `rows_affected` is the last statement's that gave
    /// one: the rows an `INSERT`, `UPDATE` or `DELETE` changed (SQLite's `changes()`), 0 for any
    /// other statement without rows, none for a query. The first error ends the run. Rows past
    /// `limits` are still stepped and counted in `rows_seen`, but not kept, and `complete`
    /// becomes false. The byte cap counts the column names and the cell text kept. Text with a NUL
    /// is refused before anything runs.
    pub async fn run(&self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        self.run_as(sql, limits, Columns::Typed).await
    }

    /// [`SqliteConnection::run`], with the columns untyped (table pages take them from the
    /// catalog).
    pub async fn run_untyped(&self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        self.run_as(sql, limits, Columns::Untyped).await
    }

    /// The first cell of the first row as an unsigned integer (a count).
    pub async fn scalar_u64(&self, sql: &str) -> Result<u64, DbError> {
        let result = self.run_as(sql, Limits { max_rows: 1, ..QUERY_LIMITS }, Columns::Unnamed).await?;
        let cell = result.rows.first().and_then(|row| row.first()).and_then(|c| c.as_deref());
        cell.and_then(|text| text.trim().parse::<u64>().ok()).ok_or_else(|| DbError::Other("the query did not return a count".into()))
    }

    async fn run_as(&self, sql: &str, limits: Limits, columns: Columns) -> Result<ResultSet, DbError> {
        let _turn = self.lock().await?;
        let sql = sql.to_owned();
        self.blocking(move |c, running| run_text(c, running, &sql, limits, columns)).await
    }
}

/// Run every statement of `sql` on `c` in turn, as [`SqliteConnection::run`] describes.
pub(crate) fn run_text(c: &Connection, running: &Running, sql: &str, limits: Limits, columns: Columns) -> Result<ResultSet, DbError> {
    if sql.contains('\0') {
        return Err(DbError::Other(NUL.into()));
    }
    let mut kept: Option<ResultSet> = None;
    let mut affected = None;
    for range in dbee_sql::lexer::statements(Dialect::Sqlite, sql) {
        if running.interrupted() {
            return Err(DbError::Cancelled);
        }
        let result = statement(c, sql, range.start, range.end, limits, columns)?;
        affected = result.rows_affected.or(affected);
        if !result.columns.is_empty() || kept.as_ref().is_none_or(|k| k.columns.is_empty()) {
            // A result with rows outranks one without.
            kept = Some(result);
        }
    }
    let mut result = kept.unwrap_or(ResultSet { complete: true, ..ResultSet::default() });
    result.rows_affected = affected;
    Ok(result)
}

/// Prepare and step the statement at bytes `start..end` of `sql`.
fn statement(c: &Connection, sql: &str, start: usize, end: usize, limits: Limits, columns: Columns) -> Result<ResultSet, DbError> {
    let failed = |e: rusqlite::Error| errors::in_text(e, sql, start);
    let text = sql.get(start..end).ok_or_else(|| DbError::Other("the statement is not in the text".into()))?;
    let mut st = c.prepare(text).map_err(failed)?;
    if st.parameter_count() > 0 {
        return Err(DbError::Other(PARAMETERS.into()));
    }
    if columns != Columns::Unnamed && st.column_count() > 0 && !names_are_utf8(c)? {
        return Err(DbError::Other(NOT_UTF8.into()));
    }
    let changes_before = c.total_changes();
    let mut reals = Reals { conn: c, cast: None };
    let mut set = ResultSet { complete: true, ..ResultSet::default() };
    let mut described: Option<Vec<Column>> = None;
    let mut bytes: usize = 0;
    let mut rows = st.raw_query();
    while let Some(row) = rows.next().map_err(failed)? {
        // Read after the first step, which prepares the statement again if the schema changed.
        if described.is_none() {
            let named = describe(row.as_ref(), columns)?;
            bytes = named.iter().fold(0usize, |sum, c| sum.saturating_add(c.name.len()));
            if bytes > limits.max_bytes {
                return Err(DbError::Other("the result's column names are larger than the cap".into()));
            }
            described = Some(named);
        }
        let width = described.as_ref().map_or(0, Vec::len);
        set.rows_seen = set.rows_seen.saturating_add(1);
        if !set.complete {
            continue;
        }
        if set.rows.len() >= limits.max_rows {
            set.complete = false;
            continue;
        }
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            cells.push(cell(row.get_ref(i).map_err(errors::from_sqlite)?, &mut reals).map_err(failed)?);
        }
        let row_bytes = cells.iter().fold(0usize, |sum, cell| sum.saturating_add(cell.as_ref().map_or(0, CellText::len)));
        if bytes.saturating_add(row_bytes) > limits.max_bytes {
            set.complete = false;
            continue;
        }
        bytes = bytes.saturating_add(row_bytes);
        set.rows.push(cells.into_iter().map(|cell| cell.map(CellText::into_string)).collect());
    }
    drop(rows);
    set.columns = match described {
        Some(named) => named,
        None => describe(&st, columns)?,
    };
    set.rows_affected = if c.total_changes() != changes_before {
        Some(c.changes())
    } else if set.columns.is_empty() {
        Some(0)
    } else {
        None
    };
    Ok(set)
}

/// The statement's result columns, as `columns` asks.
fn describe(st: &Statement<'_>, columns: Columns) -> Result<Vec<Column>, DbError> {
    match columns {
        Columns::Unnamed => Ok(vec![Column::default(); st.column_count()]),
        Columns::Untyped => (0..st.column_count()).map(|i| st.column_name(i).map(Column::untyped).map_err(errors::from_sqlite)).collect(),
        Columns::Typed => Ok(st.columns().iter().map(|col| values::column(col.name(), col.decl_type())).collect()),
    }
}

/// Whether every name and type in the schemas of the connection's databases is valid UTF-8, which
/// a result's column names and declared types come from (the rest from the statement's own text).
/// SQLite keeps them as bytes, unchecked; rusqlite panics on one that is not UTF-8.
fn names_are_utf8(c: &Connection) -> Result<bool, DbError> {
    let mut databases = Vec::new();
    for_each_row(c, "SELECT name FROM pragma_database_list", |row| {
        databases.push(String::from_utf8_lossy(row.get_ref(0)?.as_bytes().unwrap_or_default()).into_owned());
        Ok(true)
    })?;
    for database in databases {
        let schema = Dialect::Sqlite.ident(&database).map_err(|e| DbError::Other(format!("a database name cannot be used: {e}")))?;
        let utf8 = for_each_row(c, &format!("SELECT name, tbl_name, sql FROM {schema}.sqlite_schema"), |row| {
            for i in 0..3 {
                if let ValueRef::Text(bytes) = row.get_ref(i)?
                    && std::str::from_utf8(bytes).is_err()
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        if !utf8 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Run `sql` and call `f` on each row until it returns false; false when it did.
fn for_each_row(c: &Connection, sql: &str, mut f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<bool>) -> Result<bool, DbError> {
    let mut st = c.prepare(sql).map_err(errors::from_sqlite)?;
    let mut rows = st.raw_query();
    while let Some(row) = rows.next().map_err(errors::from_sqlite)? {
        if !f(row).map_err(errors::from_sqlite)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The value `v` before its text is built; NULL is `None`.
fn cell<'v>(v: ValueRef<'v>, reals: &mut Reals<'_>) -> rusqlite::Result<Option<CellText<'v>>> {
    Ok(match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(CellText::Number(i.to_string())),
        ValueRef::Real(x) => Some(CellText::Number(reals.text(x)?)),
        ValueRef::Text(bytes) => Some(CellText::Text(bytes)),
        ValueRef::Blob(bytes) => Some(CellText::Blob(bytes)),
    })
}

/// Reals as `CAST(x AS TEXT)` prints them, by SQLite itself: its rule is its own, neither the
/// shortest form nor a fixed number of digits.
struct Reals<'c> {
    conn: &'c Connection,
    cast: Option<Statement<'c>>,
}

impl Reals<'_> {
    fn text(&mut self, x: f64) -> rusqlite::Result<String> {
        let cast = match &mut self.cast {
            Some(cast) => cast,
            None => self.cast.insert(self.conn.prepare("SELECT CAST(?1 AS TEXT)")?),
        };
        cast.query_row([x], |row| Ok(String::from_utf8_lossy(row.get_ref(0)?.as_bytes().unwrap_or_default()).into_owned()))
    }
}

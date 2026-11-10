//! Running SQL text: each statement prepared and stepped in turn, within the row and byte caps,
//! with values as text by fixed rules ([`crate::values`]).

use dbee_core::{Column, ResultSet};
use dbee_driver::{DbError, Limits, QUERY_LIMITS, ROW_COST, TransactionState, cell_cost, column_cost};
use dbee_sql::Dialect;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Statement};

use crate::connect::{Running, Session, SqliteConnection};
use crate::errors::{self, PARAMETERS};
use crate::values::{self, CellText};

/// SQLite ends a statement at a NUL outside a string literal, and runs the part before it.
const NUL: &str = "the statement holds a NUL character (U+0000), which SQLite would take as its end";

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
    /// becomes false. The byte cap counts what the columns and the rows kept hold
    /// ([`dbee_driver::cell_cost`]). Text with a NUL is refused before anything runs.
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

    /// Whether the connection is in a transaction: SQLite's autocommit flag is off inside one.
    /// `Unknown` on a closed connection.
    pub async fn transaction(&self) -> TransactionState {
        let Ok(_turn) = self.lock().await else { return TransactionState::Unknown };
        match self.blocking(|s, _| Ok(s.conn.is_autocommit())).await {
            Ok(true) => TransactionState::Idle,
            Ok(false) => TransactionState::Open,
            Err(_) => TransactionState::Unknown,
        }
    }

    async fn run_as(&self, sql: &str, limits: Limits, columns: Columns) -> Result<ResultSet, DbError> {
        let _turn = self.lock().await?;
        let sql = sql.to_owned();
        self.blocking(move |s, running| run_text(s, running, &sql, limits, columns)).await
    }
}

/// Run every statement of `sql` on `s` in turn, as [`SqliteConnection::run`] describes.
pub(crate) fn run_text(s: &mut Session, running: &Running, sql: &str, limits: Limits, columns: Columns) -> Result<ResultSet, DbError> {
    if sql.contains('\0') {
        return Err(DbError::Other(NUL.into()));
    }
    let mut kept: Option<ResultSet> = None;
    let mut affected = None;
    for range in dbee_sql::lexer::statements(Dialect::Sqlite, sql) {
        if running.interrupted() {
            return Err(DbError::Cancelled);
        }
        s.refused.store(0, std::sync::atomic::Ordering::Relaxed);
        let result = statement(s, running, sql, range.start, range.end, limits, columns)
            .map_err(|e| errors::refused(e, s.refused.load(std::sync::atomic::Ordering::Relaxed)))?;
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
fn statement(
    s: &mut Session,
    running: &Running,
    sql: &str,
    start: usize,
    end: usize,
    limits: Limits,
    columns: Columns,
) -> Result<ResultSet, DbError> {
    let failed = |e: rusqlite::Error| errors::in_text(e, sql, start);
    let text = sql.get(start..end).ok_or_else(|| DbError::Other("the statement is not in the text".into()))?;
    let c = &s.conn;
    let mut st = c.prepare(text).map_err(failed)?;
    if st.parameter_count() > 0 {
        return Err(DbError::Other(PARAMETERS.into()));
    }
    if columns != Columns::Unnamed && st.column_count() > 0 {
        s.names.check(c)?;
    }
    // SQLite drops an interrupt that comes before the first step, as no statement runs yet.
    if running.interrupted() {
        return Err(DbError::Cancelled);
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
            bytes = named.iter().map(column_cost).fold(0, usize::saturating_add);
            if bytes > limits.max_bytes {
                return Err(DbError::TooLarge { left: limits.max_bytes });
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
        let row_bytes = cells.iter().fold(ROW_COST, |sum, cell| sum.saturating_add(cell_cost(cell.as_ref().map(CellText::len))));
        if bytes.saturating_add(row_bytes) > limits.max_bytes {
            set.complete = false;
            continue;
        }
        bytes = bytes.saturating_add(row_bytes);
        set.rows.push(cells.into_iter().map(|cell| cell.map(CellText::into_string)).collect());
    }
    drop(rows);
    // A write may change a schema without a new schema version (`writable_schema`).
    if !st.readonly() {
        s.names.forget();
    }
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

/// The databases whose schemas were last found to hold only UTF-8 names and types: each one's
/// name, file and schema version. A result's column names and declared types come from those
/// schemas (the rest from the statement's own text); SQLite keeps them as bytes, unchecked, and
/// rusqlite panics on one that is not UTF-8.
#[derive(Debug, Default)]
pub(crate) struct NamesChecked(Option<Vec<(String, String, i64)>>);

impl NamesChecked {
    /// Refuse a result while a schema holds a name or a type that is not UTF-8, naming its table.
    /// The schemas are read again only when a database or its schema version changed.
    fn check(&mut self, c: &Connection) -> Result<(), DbError> {
        let databases = databases(c)?;
        if self.0.as_ref() == Some(&databases) {
            return Ok(());
        }
        for (database, _, _) in &databases {
            if let Some(table) = not_utf8(c, database)? {
                let table = if database == "main" { table } else { format!("{database}.{table}") };
                return Err(DbError::Other(format!(
                    "the table {table} has a name or a type that is not valid UTF-8, so dbee cannot show this result"
                )));
            }
        }
        self.0 = Some(databases);
        Ok(())
    }

    fn forget(&mut self) {
        self.0 = None;
    }
}

/// Each database on the connection: its name, its file and its schema version.
fn databases(c: &Connection) -> Result<Vec<(String, String, i64)>, DbError> {
    let mut listed = Vec::new();
    for_each_row(c, "SELECT name, file FROM pragma_database_list", |row| {
        listed.push((lossy(row.get_ref(0)?), lossy(row.get_ref(1)?)));
        Ok(true)
    })?;
    let mut out = Vec::with_capacity(listed.len());
    for (name, file) in listed {
        let schema = Dialect::Sqlite.ident(&name).map_err(|e| DbError::Other(format!("a database name cannot be used: {e}")))?;
        let version = c.query_row(&format!("PRAGMA {schema}.schema_version"), [], |row| row.get::<_, i64>(0)).map_err(errors::from_sqlite)?;
        out.push((name, file, version));
    }
    Ok(out)
}

/// The table of the first entry of `database`'s schema with a name or text that is not UTF-8.
fn not_utf8(c: &Connection, database: &str) -> Result<Option<String>, DbError> {
    let schema = Dialect::Sqlite.ident(database).map_err(|e| DbError::Other(format!("a database name cannot be used: {e}")))?;
    let mut found = None;
    for_each_row(c, &format!("SELECT name, tbl_name, sql FROM {schema}.sqlite_schema"), |row| {
        for i in 0..3 {
            if let ValueRef::Text(bytes) = row.get_ref(i)?
                && std::str::from_utf8(bytes).is_err()
            {
                found = Some(lossy(row.get_ref(1)?));
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    Ok(found)
}

/// A text value with each invalid sequence replaced; any other value as nothing.
fn lossy(v: ValueRef<'_>) -> String {
    String::from_utf8_lossy(v.as_bytes().unwrap_or_default()).into_owned()
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
        cast.query_row([x], |row| Ok(lossy(row.get_ref(0)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupt_before_the_first_step_runs_nothing() {
        let mut s = Session::new(Connection::open_in_memory().unwrap());
        let running = Running::new(s.conn.get_interrupt_handle());
        let _call = running.start();
        assert!(running.interrupt());
        let sql = "CREATE TABLE never (a INTEGER)";
        assert_eq!(statement(&mut s, &running, sql, 0, sql.len(), QUERY_LIMITS, Columns::Typed).err(), Some(DbError::Cancelled));
        // SQLite cleared its own interrupt at the prepare: only dbee's mark kept the step from running.
        let made = s.conn.query_row("SELECT count(*) FROM sqlite_schema WHERE name = 'never'", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(made, 0);
    }
}

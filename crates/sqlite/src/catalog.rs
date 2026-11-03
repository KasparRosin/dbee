//! Catalog loading: the tables and views of `main` from `sqlite_schema` (SQLite's own `sqlite_%`
//! tables left out), then each one's columns, keys and foreign keys from the pragmas' table
//! functions, one relation at a time, into `dbee_driver`'s [`CatalogBuilder`].
//!
//! SQLite names no key, so dbee does: the primary key is `primary key`, a unique key is called by
//! its index (`sqlite_autoindex_…`, unique in the file), and a foreign key by its number in the
//! table. Only `UNIQUE` constraints are keys, not unique indexes. A relation SQLite cannot read (a
//! view over a dropped table, a virtual table whose module is missing) is listed without columns.
//! Results are text as SQLite prints them: flags `1` and `0`, numbers in decimal.
//!
//! **The rowid key.** An ordinary table without a primary key or a NOT NULL unique key is keyed by
//! its rowid: a hidden leading column named by the first of `rowid`, `_rowid_` and `oid` that no
//! column takes (ignoring ASCII case, as SQLite does). A table whose columns take all three, and a
//! virtual table, get no key and say why ([`NO_ROW_ID`], [`VIRTUAL`]). Views and `WITHOUT ROWID`
//! tables (which always have a primary key) get none.
//!
//! Limits: a catalog must never be truncated, so the queries run with `CATALOG_LIMITS`.

use std::collections::HashMap;

use dbee_core::TypeClass;
use dbee_core::cell::Cell;
use dbee_core::schema::{Catalog, ColumnInfo, Generated, RelKind, TableRef};
use dbee_driver::catalog::{ColumnRow, ForeignKeyRow, KeyKind, KeyRow, TableRow, bad, number, owned, require_complete, text};
use dbee_driver::{CATALOG_LIMITS, CatalogBuilder, DbError};
use dbee_sql::Dialect;
use rusqlite::ffi;

use crate::connect::{Running, Session, SqliteConnection};
use crate::errors;
use crate::query::{Columns, run_text};
use crate::values;

/// The one schema dbee shows: attached databases are not listed.
const SCHEMA: &str = "main";

/// The name dbee gives a table's primary key.
const PRIMARY_KEY: &str = "primary key";

/// The names SQLite gives a rowid table's row id, in the order dbee picks one for the key.
const ROW_ID_NAMES: [&str; 3] = ["rowid", "_rowid_", "oid"];

/// Why a table without a key whose columns take every name of the row id takes no edits.
const NO_ROW_ID: &str = "This table has no key, and its columns use every name SQLite gives the row id (rowid, _rowid_, oid), so the grid can't tell its rows apart; use a query tab to change it.";

/// Why a virtual table takes no edits.
const VIRTUAL: &str = "This is a virtual table, whose module decides what a write does, so the grid doesn't edit it; use a query tab to change it.";

/// A virtual table, like a view, has no root page of its own.
const RELATIONS: &str =
    "SELECT type, name, coalesce(rootpage, 0) = 0 FROM main.sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'";

/// `sqlite_stat1` exists once `ANALYZE` ran.
const HAS_STAT1: &str = "SELECT count(*) FROM main.sqlite_schema WHERE type = 'table' AND name = 'sqlite_stat1'";

const STAT1: &str = "SELECT tbl, stat FROM main.sqlite_stat1";

impl SqliteConnection {
    /// Load the tables and views of `main`, which is the default schema; tables by name, columns
    /// in the order they were declared.
    pub async fn load_catalog(&self) -> Result<Catalog, DbError> {
        let rows = self.catalog_rows().await?;
        let dropped = rows.dropped();
        if dropped > 0 {
            log::info!("sqlite: catalog: {dropped} rows named a table the catalog did not list");
        }
        Ok(rows.build(Some(SCHEMA)))
    }

    /// The catalog's rows, not yet built into a [`Catalog`].
    pub async fn catalog_rows(&self) -> Result<CatalogBuilder, DbError> {
        let _turn = self.lock().await?;
        self.blocking(rows).await
    }
}

/// What the pragmas say of one relation.
struct Relation {
    table: TableRef,
    kind: RelKind,
    /// SQLite could read its columns.
    read: bool,
    /// A virtual table: its module serves the rows.
    is_virtual: bool,
    /// Why the grid takes no edits of it, when the catalog knows.
    no_edit_reason: Option<&'static str>,
    /// The columns as declared, each with its place in the primary key (0: none).
    columns: Vec<(ColumnInfo, u32)>,
    /// The unique keys: an index's name, and its columns in key order.
    unique: Vec<(String, Vec<String>)>,
    foreign: Vec<ForeignPart>,
}

/// A column of a foreign key, as `pragma_foreign_key_list` writes it: the referenced table and
/// column as the statement spelled them, the column `None` for the referenced table's key.
struct ForeignPart {
    id: u32,
    seq: u32,
    target: String,
    from: String,
    to: Option<String>,
}

impl Relation {
    /// The column named `name` as SQLite finds it, ignoring ASCII case: its own spelling.
    fn column(&self, name: &str) -> Option<&str> {
        self.columns.iter().map(|(c, _)| c.name.as_str()).find(|c| c.eq_ignore_ascii_case(name))
    }

    /// A primary key, or a unique key whose columns are all NOT NULL (as `TableInfo::row_key`
    /// reads them).
    fn has_key(&self) -> bool {
        let not_null = |name: &String| self.columns.iter().any(|(c, _)| c.name == *name && c.not_null);
        self.columns.iter().any(|(_, at)| *at > 0) || self.unique.iter().any(|(_, columns)| columns.iter().all(not_null))
    }

    /// Column `seq` (0-based) of the primary key.
    fn key_column(&self, seq: u32) -> Option<&str> {
        let at = seq.checked_add(1)?;
        self.columns.iter().find(|(_, key)| *key == at).map(|(c, _)| c.name.as_str())
    }
}

/// A catalog query's rows; a result cut by the limits is an error.
fn query(s: &mut Session, running: &Running, sql: &str) -> Result<Vec<Vec<Cell>>, DbError> {
    Ok(require_complete(run_text(s, running, sql, CATALOG_LIMITS, Columns::Unnamed)?)?.rows)
}

/// The catalog's rows.
fn rows(s: &mut Session, running: &Running) -> Result<CatalogBuilder, DbError> {
    let estimates = estimates(s, running)?;
    let mut relations = Vec::new();
    for row in query(s, running, RELATIONS)? {
        let kind = match text(&row, 0, "relation type")? {
            "table" => RelKind::Table,
            "view" => RelKind::View,
            _ => return Err(bad("relation type")),
        };
        let is_virtual = kind == RelKind::Table && flag(&row, 2, "virtual table")?;
        relations.push(relation(s, running, owned(&row, 1, "relation name")?, kind, is_virtual)?);
    }
    let unread = relations.iter().filter(|r| !r.read).count();
    if unread > 0 {
        log::info!("sqlite: catalog: {unread} relations listed without columns, which SQLite could not read (SQLite error 1)");
    }
    let by_name: HashMap<String, usize> = relations.iter().enumerate().map(|(i, r)| (r.table.name.to_ascii_lowercase(), i)).collect();
    let mut catalog = CatalogBuilder::new(&[]);
    catalog.schema(SCHEMA);
    for r in &relations {
        let estimated_rows = if r.kind == RelKind::Table { estimates.get(&r.table.name.to_ascii_lowercase()).copied() } else { None };
        let no_edit_reason = r.no_edit_reason.map(str::to_owned);
        catalog.table(TableRow { table: r.table.clone(), kind: r.kind, estimated_rows, no_edit_reason });
        for (i, (column, key)) in r.columns.iter().enumerate() {
            let position = u32::try_from(i).map_err(|_| bad("column position"))?;
            catalog.column(ColumnRow { table: r.table.clone(), position, column: column.clone() });
            if *key > 0 {
                let column = column.name.clone();
                catalog.key(KeyRow { table: r.table.clone(), constraint: PRIMARY_KEY.into(), kind: KeyKind::Primary, position: *key, column });
            }
        }
        for (index, columns) in &r.unique {
            for (i, column) in columns.iter().enumerate() {
                let position = u32::try_from(i).map_err(|_| bad("key position"))?;
                let (constraint, column) = (index.clone(), column.clone());
                catalog.key(KeyRow { table: r.table.clone(), constraint, kind: KeyKind::Unique, position, column });
            }
        }
        for row in foreign_keys(r, &relations, &by_name) {
            catalog.foreign_key(row);
        }
    }
    Ok(catalog)
}

/// The relation `name` of `kind`, with what the pragmas say of it.
fn relation(s: &mut Session, running: &Running, name: String, kind: RelKind, is_virtual: bool) -> Result<Relation, DbError> {
    let quoted = literal(&name)?;
    let table = TableRef { schema: SCHEMA.into(), name };
    let mut r = Relation {
        table,
        kind,
        read: false,
        is_virtual,
        no_edit_reason: is_virtual.then_some(VIRTUAL),
        columns: Vec::new(),
        unique: Vec::new(),
        foreign: Vec::new(),
    };
    let xinfo = format!("SELECT name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo({quoted}, 'main')");
    let columns = match query(s, running, &xinfo) {
        Ok(rows) => rows,
        Err(e) if errors::is_code(&e, ffi::SQLITE_ERROR) => return Ok(r),
        Err(e) => return Err(e),
    };
    r.read = true;
    for row in &columns {
        let declared = owned(row, 1, "column type")?;
        // hidden: 1 a virtual table's hidden column, 2 a virtual generated column, 3 a stored one.
        let hidden: u32 = number(row, 5, "hidden")?;
        let generated = if matches!(hidden, 2 | 3) { Generated::Expression } else { Generated::No };
        let default = nullable(row, 3, "column default")?.is_some_and(|d| !d.eq_ignore_ascii_case("NULL"));
        let column = ColumnInfo {
            name: owned(row, 0, "column name")?,
            base_type: values::base_type(&declared).to_owned(),
            class: values::class_of(&declared),
            type_sql: declared,
            not_null: flag(row, 2, "not null")?,
            has_default: default || generated == Generated::Expression,
            generated,
            hidden: hidden == 1,
        };
        r.columns.push((column, number(row, 4, "key position")?));
    }
    if kind != RelKind::Table {
        return Ok(r);
    }
    let mut key_index = false;
    let mut unique = Vec::new();
    for row in query(s, running, &format!("SELECT name, origin FROM pragma_index_list({quoted}, 'main')"))? {
        match text(&row, 1, "index origin")? {
            "pk" => key_index = true,
            "u" => unique.push(owned(&row, 0, "index name")?),
            _ => {}
        }
    }
    // SQLite numbers them in the order written (`…_1`, `…_2`, `…_10`): shorter names first.
    unique.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    for index in unique {
        let parts = query(s, running, &format!("SELECT name FROM pragma_index_info({}, 'main') ORDER BY seqno", literal(&index)?))?;
        let columns = parts.iter().map(|row| nullable(row, 0, "key column").map(|n| n.map(str::to_owned))).collect::<Result<Option<Vec<_>>, _>>()?;
        // A part that is an expression has no name: the key is not one of columns.
        if let Some(columns) = columns.filter(|c| !c.is_empty()) {
            r.unique.push((index, columns));
        }
    }
    // A key of one column with no index of its own is the rowid, which SQLite fills.
    let mut key = r.columns.iter_mut().filter(|(_, at)| *at > 0);
    if let (Some((rowid, _)), None, false) = (key.next(), key.next(), key_index) {
        rowid.has_default = true;
    }
    for row in query(s, running, &format!("SELECT id, seq, \"table\", \"from\", \"to\" FROM pragma_foreign_key_list({quoted}, 'main')"))? {
        r.foreign.push(ForeignPart {
            id: number(&row, 0, "foreign key id")?,
            seq: number(&row, 1, "foreign key position")?,
            target: owned(&row, 2, "referenced table")?,
            from: owned(&row, 3, "foreign key column")?,
            to: nullable(&row, 4, "referenced column")?.map(str::to_owned),
        });
    }
    if !r.is_virtual && !r.has_key() {
        match ROW_ID_NAMES.into_iter().find(|name| r.column(name).is_none()) {
            Some(name) => r.columns.insert(0, (row_id(name), 1)),
            None => r.no_edit_reason = Some(NO_ROW_ID),
        }
    }
    Ok(r)
}

/// The row id of a table keyed by it, as the column `name`: hidden, and filled by SQLite.
fn row_id(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_sql: "INTEGER".into(),
        base_type: "INTEGER".into(),
        class: TypeClass::Number,
        not_null: true,
        has_default: true,
        generated: Generated::No,
        hidden: true,
    }
}

/// `r`'s foreign keys, their table and columns as SQLite resolves them: the target table and its
/// columns found ignoring ASCII case, and a key that names no columns references the target's
/// primary key. A key whose columns cannot be found that way is left out.
fn foreign_keys(r: &Relation, relations: &[Relation], by_name: &HashMap<String, usize>) -> Vec<ForeignKeyRow> {
    let mut ids: Vec<u32> = r.foreign.iter().map(|f| f.id).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        let mut parts: Vec<&ForeignPart> = r.foreign.iter().filter(|f| f.id == id).collect();
        parts.sort_by_key(|p| p.seq);
        let Some(first) = parts.first() else { continue };
        let target = by_name.get(&first.target.to_ascii_lowercase()).and_then(|&i| relations.get(i));
        let target_name = target.map_or_else(|| first.target.clone(), |t| t.table.name.clone());
        let target_columns: Option<Vec<String>> = parts
            .iter()
            .map(|p| match &p.to {
                Some(to) => Some(target.and_then(|t| t.column(to)).unwrap_or(to).to_owned()),
                None => target.and_then(|t| t.key_column(p.seq)).map(str::to_owned),
            })
            .collect();
        let Some(target_columns) = target_columns else { continue };
        for (p, target_column) in parts.iter().zip(target_columns) {
            out.push(ForeignKeyRow {
                table: r.table.clone(),
                constraint: format!("foreign key {id}"),
                position: p.seq,
                column: p.from.clone(),
                target: TableRef { schema: SCHEMA.into(), name: target_name.clone() },
                target_column,
            });
        }
    }
    out
}

/// Each table's row estimate from `sqlite_stat1`, by its name in lowercase, when `ANALYZE` made
/// that table. A `stat` starts with the rows of the table or of the index, which a partial index
/// has fewer of: the largest counts.
fn estimates(s: &mut Session, running: &Running) -> Result<HashMap<String, u64>, DbError> {
    let mut out = HashMap::new();
    let present = query(s, running, HAS_STAT1)?;
    if present.first().map(|row| number::<u64>(row, 0, "sqlite_stat1")).transpose()? != Some(1) {
        return Ok(out);
    }
    for row in query(s, running, STAT1)? {
        let (Some(table), Some(stat)) = (nullable(&row, 0, "stat1 table")?, nullable(&row, 1, "stat1 stat")?) else { continue };
        let Some(rows) = stat.split_ascii_whitespace().next().and_then(|n| n.parse::<u64>().ok()) else { continue };
        let most = out.entry(table.to_ascii_lowercase()).or_insert(0);
        *most = rows.max(*most);
    }
    Ok(out)
}

/// A pragma's flag: `1` or `0`.
fn flag(row: &[Cell], i: usize, what: &str) -> Result<bool, DbError> {
    match text(row, i, what)? {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(bad(what)),
    }
}

/// Cell `i` of `row`, which may be NULL; a missing cell is [`bad`].
fn nullable<'a>(row: &'a [Cell], i: usize, what: &str) -> Result<Option<&'a str>, DbError> {
    row.get(i).map(Option::as_deref).ok_or_else(|| bad(what))
}

/// `name` as a string literal, for a pragma's argument.
fn literal(name: &str) -> Result<String, DbError> {
    Dialect::Sqlite.literal(name).map_err(|e| DbError::Other(format!("a name cannot be used: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_as_the_pragmas_write_them() {
        let row: Vec<Cell> = vec![Some("1".into()), Some("0".into()), Some("2".into()), Some("true".into()), None];
        assert_eq!((flag(&row, 0, "f"), flag(&row, 1, "f")), (Ok(true), Ok(false)));
        for i in [2, 3, 4, 5] {
            assert!(flag(&row, i, "f").is_err_and(|e| e.to_string() == "unexpected catalog data (f)"), "{i}");
        }
        assert_eq!((nullable(&row, 4, "n"), nullable(&row, 0, "n")), (Ok(None), Ok(Some("1"))));
        assert!(nullable(&row, 5, "n").is_err(), "a missing cell is not NULL");
    }

    #[test]
    fn foreign_keys_resolve_as_sqlite_does() {
        let column = |name: &str| ColumnInfo {
            name: name.into(),
            type_sql: String::new(),
            base_type: String::new(),
            class: dbee_core::TypeClass::Other,
            not_null: false,
            has_default: false,
            generated: Generated::No,
            hidden: false,
        };
        let parent = Relation {
            table: TableRef { schema: SCHEMA.into(), name: "Parent".into() },
            kind: RelKind::Table,
            read: true,
            is_virtual: false,
            no_edit_reason: None,
            columns: vec![(column("A"), 2), (column("b"), 1)],
            unique: Vec::new(),
            foreign: Vec::new(),
        };
        let part = |id, seq, target: &str, from: &str, to: Option<&str>| ForeignPart {
            id,
            seq,
            target: target.into(),
            from: from.into(),
            to: to.map(Into::into),
        };
        let child = Relation {
            table: TableRef { schema: SCHEMA.into(), name: "child".into() },
            kind: RelKind::Table,
            read: true,
            is_virtual: false,
            no_edit_reason: None,
            columns: vec![(column("x"), 0), (column("y"), 0)],
            unique: Vec::new(),
            foreign: vec![
                part(1, 1, "PARENT", "x", Some("a")),
                part(1, 0, "PARENT", "y", Some("B")),
                part(0, 1, "parent", "y", None),
                part(0, 0, "parent", "x", None),
                part(2, 0, "gone", "x", Some("Id")),
                part(3, 0, "gone", "y", None),
            ],
        };
        let relations = [parent, child];
        let by_name: HashMap<String, usize> = relations.iter().enumerate().map(|(i, r)| (r.table.name.to_ascii_lowercase(), i)).collect();
        let rows = foreign_keys(&relations[1], &relations, &by_name);
        let got: Vec<(&str, u32, &str, &str, &str)> =
            rows.iter().map(|r| (r.constraint.as_str(), r.position, r.column.as_str(), r.target.name.as_str(), r.target_column.as_str())).collect();
        assert_eq!(
            got,
            [
                // No columns named: the key's, in its order.
                ("foreign key 0", 0, "x", "Parent", "b"),
                ("foreign key 0", 1, "y", "Parent", "A"),
                // Spelled in another case: the table's and the columns' own spelling.
                ("foreign key 1", 0, "y", "Parent", "b"),
                ("foreign key 1", 1, "x", "Parent", "A"),
                // A table that is not there keeps the names as written; with no columns named, it is left out.
                ("foreign key 2", 0, "x", "gone", "Id"),
            ]
        );
    }
}

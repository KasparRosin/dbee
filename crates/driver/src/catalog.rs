//! Building a [`Catalog`] from a driver's catalog rows, and reading those rows.
//!
//! A driver reads its engine's catalog (pg_catalog, information_schema, pragmas) into plain
//! rows: schemas, tables, columns, key columns and foreign-key column pairs, each naming its
//! table by schema and name, and each column with its position. [`CatalogBuilder`] puts them
//! together the same way for every engine. Which schemas are the engine's own is the driver's
//! input.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use dbee_core::{Catalog, Cell, ColumnInfo, ForeignKey, RelKind, ResultSet, SchemaInfo, TableInfo, TableRef};

use crate::{CATALOG_LIMITS, DbError, Limits, result_cost};

/// A catalog row that is not what the query promised.
pub fn bad(what: &str) -> DbError {
    DbError::Other(format!("unexpected catalog data ({what})"))
}

/// Cell `i` of `row` as text; NULL or missing is [`bad`].
pub fn text<'a>(row: &'a [Cell], i: usize, what: &str) -> Result<&'a str, DbError> {
    row.get(i).and_then(|c| c.as_deref()).ok_or_else(|| bad(what))
}

pub fn owned(row: &[Cell], i: usize, what: &str) -> Result<String, DbError> {
    text(row, i, what).map(str::to_owned)
}

pub fn number<T: FromStr>(row: &[Cell], i: usize, what: &str) -> Result<T, DbError> {
    text(row, i, what)?.parse().map_err(|_| bad(what))
}

/// A catalog result that was cut by the limits is an error: a partial catalog would be wrong.
pub fn require_complete(r: ResultSet) -> Result<ResultSet, DbError> {
    if r.complete { Ok(r) } else { Err(DbError::Other(format!("catalog too large: stopped after {} rows", r.rows_seen))) }
}

/// What one catalog read may still hold: its queries' results are held together until the
/// catalog is built, so [`CATALOG_LIMITS`]'s bytes are for all of them, and each query gets what
/// the ones before left.
#[derive(Debug, Clone)]
pub struct CatalogBudget {
    left: usize,
}

impl Default for CatalogBudget {
    fn default() -> CatalogBudget {
        CatalogBudget { left: CATALOG_LIMITS.max_bytes }
    }
}

impl CatalogBudget {
    /// The limits of the next query.
    pub fn limits(&self) -> Limits {
        Limits { max_rows: CATALOG_LIMITS.max_rows, max_bytes: self.left }
    }

    /// `r`, which must be complete ([`require_complete`]), taken out of the budget.
    pub fn take(&mut self, r: ResultSet) -> Result<ResultSet, DbError> {
        let r = require_complete(r)?;
        self.left = self.left.saturating_sub(result_cost(&r));
        Ok(r)
    }
}

/// A table, view or other relation.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub table: TableRef,
    pub kind: RelKind,
    pub estimated_rows: Option<u64>,
    pub no_edit_reason: Option<String>,
}

/// A column of `table`, at `position` (any numbers that grow left to right).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRow {
    pub table: TableRef,
    pub position: u32,
    pub column: ColumnInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Primary,
    Unique,
}

/// A column of `table`'s key `constraint`, at `position` in the key. `constraint` must be unique
/// within its table: an engine without key names (SQLite) makes one up per key, and rows that
/// share a name make one key.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyRow {
    pub table: TableRef,
    pub constraint: String,
    pub kind: KeyKind,
    pub position: u32,
    pub column: String,
}

/// A column of `table`'s foreign key `constraint` and the `target` column it references, at
/// `position` in the key. `constraint` is also the key's name, and must be unique within its
/// table as for [`KeyRow`].
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyRow {
    pub table: TableRef,
    pub constraint: String,
    pub position: u32,
    pub column: String,
    pub target: TableRef,
    pub target_column: String,
}

/// Collects a driver's catalog rows, in any order, and builds the [`Catalog`].
///
/// A table's schema is listed even when no [`schema`](Self::schema) call named it. A row for a
/// table that no [`table`](Self::table) call listed is dropped (the table went between two
/// catalog queries, say; [`dropped`](Self::dropped) counts them), and a schema or table listed
/// twice is kept once, as first listed.
#[derive(Debug, Default)]
pub struct CatalogBuilder {
    system: Vec<String>,
    schemas: Vec<String>,
    tables: Vec<TableRow>,
    columns: Vec<ColumnRow>,
    keys: Vec<KeyRow>,
    foreign_keys: Vec<ForeignKeyRow>,
}

impl CatalogBuilder {
    /// A builder for an engine whose own schemas are `system_schemas`.
    pub fn new(system_schemas: &[&str]) -> Self {
        CatalogBuilder { system: system_schemas.iter().map(|s| (*s).to_owned()).collect(), ..CatalogBuilder::default() }
    }

    /// A schema, listed even when it holds no table.
    pub fn schema(&mut self, name: impl Into<String>) {
        self.schemas.push(name.into());
    }

    pub fn table(&mut self, row: TableRow) {
        self.tables.push(row);
    }

    pub fn column(&mut self, row: ColumnRow) {
        self.columns.push(row);
    }

    pub fn key(&mut self, row: KeyRow) {
        self.keys.push(row);
    }

    pub fn foreign_key(&mut self, row: ForeignKeyRow) {
        self.foreign_keys.push(row);
    }

    /// How many column, key and foreign-key rows name a table no [`table`](Self::table) row
    /// listed: [`build`](Self::build) leaves them out.
    pub fn dropped(&self) -> usize {
        let listed: HashSet<&TableRef> = self.tables.iter().map(|t| &t.table).collect();
        let columns = self.columns.iter().map(|r| &r.table);
        let keys = self.keys.iter().map(|r| &r.table);
        let foreign_keys = self.foreign_keys.iter().map(|r| &r.table);
        columns.chain(keys).chain(foreign_keys).filter(|t| !listed.contains(t)).count()
    }

    /// The catalog: user schemas first, then the system ones, each group by name; tables by
    /// name; columns, and the columns of each key, by position; a table's keys in the order
    /// their first rows came. `default_schema` is kept when that schema exists.
    pub fn build(self, default_schema: Option<&str>) -> Catalog {
        let mut schemas: Vec<SchemaInfo> = Vec::new();
        let mut schema_at: HashMap<String, usize> = HashMap::new();
        for name in &self.schemas {
            add_schema(&mut schemas, &mut schema_at, &self.system, name);
        }

        let mut table_at: HashMap<TableRef, (usize, usize)> = HashMap::new();
        for row in self.tables {
            if table_at.contains_key(&row.table) {
                continue;
            }
            let si = add_schema(&mut schemas, &mut schema_at, &self.system, &row.table.schema);
            let Some(schema) = schemas.get_mut(si) else { continue };
            table_at.insert(row.table.clone(), (si, schema.tables.len()));
            schema.tables.push(TableInfo {
                table: row.table,
                kind: row.kind,
                columns: Vec::new(),
                primary_key: Vec::new(),
                unique_keys: Vec::new(),
                foreign_keys: Vec::new(),
                estimated_rows: row.estimated_rows,
                no_edit_reason: row.no_edit_reason,
            });
        }

        let mut columns = self.columns;
        columns.sort_by_key(|c| c.position);
        for row in columns {
            if let Some(t) = table_mut(&mut schemas, &table_at, &row.table) {
                t.columns.push(row.column);
            }
        }

        for key in by_constraint(self.keys, |k| (&k.table, &k.constraint), |k| k.position) {
            let Some(first) = key.first() else { continue };
            let (kind, at) = (first.kind, first.table.clone());
            let columns = key.into_iter().map(|k| k.column).collect();
            match (table_mut(&mut schemas, &table_at, &at), kind) {
                (Some(t), KeyKind::Primary) => t.primary_key = columns,
                (Some(t), KeyKind::Unique) => t.unique_keys.push(columns),
                (None, _) => {}
            }
        }

        for fk in by_constraint(self.foreign_keys, |k| (&k.table, &k.constraint), |k| k.position) {
            let Some(first) = fk.first() else { continue };
            let (at, name, target) = (first.table.clone(), first.constraint.clone(), first.target.clone());
            let (columns, target_columns) = fk.into_iter().map(|k| (k.column, k.target_column)).unzip();
            if let Some(t) = table_mut(&mut schemas, &table_at, &at) {
                t.foreign_keys.push(ForeignKey { name, columns, target, target_columns });
            }
        }

        schemas.sort_by(|a, b| a.system.cmp(&b.system).then_with(|| a.name.cmp(&b.name)));
        for s in &mut schemas {
            s.tables.sort_by(|a, b| a.table.name.cmp(&b.table.name));
        }
        let default_schema = default_schema.filter(|d| schemas.iter().any(|s| s.name == *d)).map(str::to_owned);
        Catalog { schemas, default_schema }
    }
}

/// The index of schema `name`, which is added when it is new.
fn add_schema(schemas: &mut Vec<SchemaInfo>, at: &mut HashMap<String, usize>, system: &[String], name: &str) -> usize {
    if let Some(&i) = at.get(name) {
        return i;
    }
    let i = schemas.len();
    schemas.push(SchemaInfo { name: name.to_owned(), system: system.iter().any(|s| s == name), tables: Vec::new() });
    at.insert(name.to_owned(), i);
    i
}

fn table_mut<'a>(schemas: &'a mut [SchemaInfo], at: &HashMap<TableRef, (usize, usize)>, table: &TableRef) -> Option<&'a mut TableInfo> {
    let &(si, ti) = at.get(table)?;
    schemas.get_mut(si)?.tables.get_mut(ti)
}

/// `rows` grouped by their table and constraint name: the groups in the order their first rows
/// came, each sorted by position.
fn by_constraint<R>(rows: Vec<R>, key: impl Fn(&R) -> (&TableRef, &String), position: impl Fn(&R) -> u32) -> Vec<Vec<R>> {
    let mut groups: Vec<Vec<R>> = Vec::new();
    let mut at: HashMap<(TableRef, String), usize> = HashMap::new();
    for row in rows {
        let (table, name) = key(&row);
        let id = (table.clone(), name.clone());
        match at.get(&id).and_then(|&i| groups.get_mut(i)) {
            Some(group) => group.push(row),
            None => {
                at.insert(id, groups.len());
                groups.push(vec![row]);
            }
        }
    }
    for group in &mut groups {
        group.sort_by_key(&position);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbee_core::{Generated, TypeClass};

    fn tr(schema: &str, name: &str) -> TableRef {
        TableRef { schema: schema.into(), name: name.into() }
    }

    fn table(schema: &str, name: &str) -> TableRow {
        TableRow { table: tr(schema, name), kind: RelKind::Table, estimated_rows: Some(3), no_edit_reason: None }
    }

    fn col(schema: &str, table: &str, position: u32, name: &str) -> ColumnRow {
        let column = ColumnInfo {
            name: name.into(),
            type_sql: "int".into(),
            base_type: "int4".into(),
            class: TypeClass::Number,
            not_null: false,
            has_default: false,
            generated: Generated::No,
            hidden: false,
            type_label: String::new(),
            operators: dbee_core::Operators::Unnamed,
        };
        ColumnRow { table: tr(schema, table), position, column }
    }

    fn key(table: &str, constraint: &str, kind: KeyKind, position: u32, column: &str) -> KeyRow {
        KeyRow { table: tr("s", table), constraint: constraint.into(), kind, position, column: column.into() }
    }

    fn fk(table: &str, constraint: &str, position: u32, column: &str, target: &str, target_column: &str) -> ForeignKeyRow {
        ForeignKeyRow {
            table: tr("s", table),
            constraint: constraint.into(),
            position,
            column: column.into(),
            target: tr("s", target),
            target_column: target_column.into(),
        }
    }

    fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
        items.iter().map(name).collect()
    }

    #[test]
    fn system_schemas_are_the_drivers_and_come_last() {
        let mut b = CatalogBuilder::new(&["sys_b", "sys_a"]);
        for s in ["sys_b", "zeta", "sys_a", "alpha", "empty"] {
            b.schema(s);
        }
        b.table(table("zeta", "b"));
        b.table(table("zeta", "a"));
        b.table(table("sys_a", "t"));
        b.table(table("unlisted", "t"));
        let cat = b.build(None);
        assert_eq!(names(&cat.schemas, |s| &s.name), ["alpha", "empty", "unlisted", "zeta", "sys_a", "sys_b"]);
        assert_eq!(cat.schemas.iter().map(|s| s.system).collect::<Vec<_>>(), [false, false, false, false, true, true]);
        let zeta = cat.schemas.iter().find(|s| s.name == "zeta").map(|s| names(&s.tables, |t| &t.table.name));
        assert_eq!(zeta, Some(vec!["a", "b"]), "tables by name");
        assert_eq!(cat.table(&tr("zeta", "a")).map(|t| (t.kind, t.estimated_rows)), Some((RelKind::Table, Some(3))));
        assert!(CatalogBuilder::new(&[]).build(None).schemas.is_empty());
    }

    #[test]
    fn a_schema_or_table_listed_twice_is_kept_once() {
        let mut b = CatalogBuilder::new(&[]);
        b.schema("s");
        b.schema("s");
        b.table(table("s", "t"));
        b.table(TableRow { estimated_rows: None, ..table("s", "t") });
        let cat = b.build(None);
        assert_eq!(cat.schemas.len(), 1);
        assert_eq!(cat.table_count(), 1);
        assert_eq!(cat.table(&tr("s", "t")).and_then(|t| t.estimated_rows), Some(3), "the first row counts");
    }

    #[test]
    fn columns_by_position() {
        let mut b = CatalogBuilder::new(&[]);
        b.table(table("s", "t"));
        b.table(table("s", "u"));
        b.column(col("s", "t", 3, "c"));
        b.column(col("s", "u", 1, "x"));
        b.column(col("s", "t", 1, "a"));
        b.column(col("s", "t", 2, "b"));
        b.column(col("s", "gone", 1, "lost"));
        assert_eq!(b.dropped(), 1);
        let cat = b.build(None);
        let cols = |name| cat.table(&tr("s", name)).map(|t| names(&t.columns, |c| &c.name));
        assert_eq!(cols("t"), Some(vec!["a", "b", "c"]));
        assert_eq!(cols("u"), Some(vec!["x"]));
        assert!(cat.table(&tr("s", "gone")).is_none(), "a column never makes a table");
    }

    #[test]
    fn keys_grouped_by_constraint_in_key_order() {
        let mut b = CatalogBuilder::new(&[]);
        b.table(table("s", "t"));
        b.key(key("t", "t_pkey", KeyKind::Primary, 2, "a"));
        b.key(key("t", "t_c_a", KeyKind::Unique, 1, "c"));
        b.key(key("t", "t_pkey", KeyKind::Primary, 1, "b"));
        b.key(key("t", "t_c_a", KeyKind::Unique, 2, "a"));
        b.key(key("t", "t_d", KeyKind::Unique, 1, "d"));
        b.key(key("gone", "gone_pkey", KeyKind::Primary, 1, "id"));
        assert_eq!(b.dropped(), 1);
        let cat = b.build(None);
        let t = cat.table(&tr("s", "t"));
        assert_eq!(t.map(|t| t.primary_key.clone()), Some(vec!["b".to_string(), "a".to_string()]));
        let unique = t.map(|t| t.unique_keys.clone()).unwrap_or_default();
        assert_eq!(unique, [vec!["c".to_string(), "a".to_string()], vec!["d".to_string()]], "in the order the keys came");
        assert_eq!(cat.table_count(), 1);
    }

    #[test]
    fn rows_for_unlisted_tables_are_counted() {
        let mut b = CatalogBuilder::new(&[]);
        b.table(table("s", "t"));
        b.column(col("s", "t", 1, "a"));
        b.key(key("t", "t_pkey", KeyKind::Primary, 1, "a"));
        b.foreign_key(fk("t", "t_fk", 1, "a", "gone", "id"));
        assert_eq!(b.dropped(), 0, "a foreign key's target needs no table row");
        b.column(col("s", "gone", 1, "x"));
        b.key(key("gone", "gone_pkey", KeyKind::Primary, 1, "x"));
        b.foreign_key(fk("gone", "gone_fk", 1, "x", "t", "a"));
        b.column(col("other", "t", 1, "x"));
        assert_eq!(b.dropped(), 4);
        let cat = b.build(None);
        assert_eq!((cat.table_count(), cat.schemas.len()), (1, 1));
        assert_eq!(cat.table(&tr("s", "t")).map(|t| t.foreign_keys.len()), Some(1));
    }

    #[test]
    fn foreign_key_columns_paired_by_position() {
        let mut b = CatalogBuilder::new(&[]);
        b.table(table("s", "t"));
        b.table(table("s", "r"));
        b.foreign_key(fk("r", "r_fk", 2, "x", "t", "a"));
        b.foreign_key(fk("r", "r_other", 1, "z", "t", "c"));
        b.foreign_key(fk("r", "r_fk", 1, "y", "t", "b"));
        let cat = b.build(None);
        let fks = cat.table(&tr("s", "r")).map(|t| t.foreign_keys.clone()).unwrap_or_default();
        assert_eq!(names(&fks, |f| &f.name), ["r_fk", "r_other"]);
        assert_eq!((&fks[0].columns, &fks[0].target_columns), (&vec!["y".to_string(), "x".into()], &vec!["b".to_string(), "a".into()]));
        assert_eq!(fks[0].target, tr("s", "t"));
        assert_eq!((&fks[1].columns, &fks[1].target_columns), (&vec!["z".to_string()], &vec!["c".to_string()]));
    }

    #[test]
    fn default_schema_only_when_it_exists() {
        let build = |default| {
            let mut b = CatalogBuilder::new(&[]);
            b.schema("public");
            b.build(default)
        };
        assert_eq!(build(Some("public")).default_schema.as_deref(), Some("public"));
        assert_eq!(build(Some("main")).default_schema, None);
        assert_eq!(build(None).default_schema, None);
    }

    #[test]
    fn row_readers() {
        let row: Vec<Cell> = vec![Some("t".into()), Some("false".into()), Some("42".into()), None, Some("x".into())];
        assert_eq!(number::<u32>(&row, 2, "n"), Ok(42));
        assert_eq!(text(&row, 4, "s"), Ok("x"));
        assert_eq!(owned(&row, 4, "s"), Ok("x".to_string()));
        for e in [text(&row, 3, "null").err(), text(&row, 9, "past the end").err(), number::<u32>(&row, 4, "num").err()] {
            assert!(e.is_some_and(|e| e.code() == "other" && e.to_string().starts_with("unexpected catalog data (")), "{row:?}");
        }
    }

    #[test]
    fn truncated_result_is_an_error() {
        let cut = ResultSet { complete: false, rows_seen: 7, ..ResultSet::default() };
        let err = require_complete(cut).err();
        assert!(err.is_some_and(|e| e.to_string().contains("catalog too large")));
        assert!(require_complete(ResultSet { complete: true, ..ResultSet::default() }).is_ok());
    }

    /// One read's queries share the catalog's bytes: each gets what the ones before left.
    #[test]
    fn a_catalog_read_holds_its_queries_within_one_budget() {
        let mut budget = CatalogBudget::default();
        assert_eq!(budget.limits(), CATALOG_LIMITS);
        let r = ResultSet { rows: vec![vec![Some("x".repeat(1000)), None]; 3], complete: true, ..ResultSet::default() };
        let held = result_cost(&r);
        assert_eq!(budget.take(r).map(|r| r.rows.len()), Ok(3));
        assert_eq!(budget.limits().max_bytes, CATALOG_LIMITS.max_bytes - held);
        assert_eq!(budget.limits().max_rows, CATALOG_LIMITS.max_rows);
        let cut = budget.take(ResultSet { complete: false, ..ResultSet::default() });
        assert!(cut.is_err_and(|e| e.to_string().contains("catalog too large")));
    }
}

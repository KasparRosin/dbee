//! Catalog loading: schemas, relations, columns and keys, from three fixed catalog queries
//! joined in Rust.
//!
//! The queries read only `pg_namespace`, `pg_class`, `pg_attribute`, `pg_type` and
//! `pg_constraint` columns that exist in Postgres 14 and every later version, and take no user
//! input. Results are simple-query text (`t`/`f` booleans, decimal numbers). Key columns come
//! from `unnest(conkey) WITH ORDINALITY`, so no array text is parsed.
//!
//! Limits: a catalog must never be truncated, so the queries run with limits no real catalog
//! reaches (`CATALOG_LIMITS`: 5 million rows and 512 MiB of text; a result that still hits them is an error, never a partial catalog) instead of the 10,000-row
//! query cap, because a database with a few hundred tables already has more columns than that.

use std::collections::HashMap;

use dbee_core::cell::{Cell, ResultSet};
use dbee_core::schema::{Catalog, ColumnInfo, ForeignKey, Generated, RelKind, SchemaInfo, TableInfo, TableRef};

use crate::connect::PgConnection;
use crate::error::PgError;
use crate::query::{self, Limits};

/// Limits for catalog queries: far above any real catalog, so nothing is ever cut.
pub const CATALOG_LIMITS: Limits = Limits { max_rows: 5_000_000, max_bytes: 512 * 1024 * 1024 };

/// Schemas and relations. A schema without relations yields one row with NULL relation columns.
/// Partitions (`relispartition`) are hidden; the partitioned parent (`p`) is shown.
const RELATIONS_SQL: &str = "\
SELECT n.nspname, c.oid::text, c.relname, c.relkind::text, c.reltuples::numeric::text
FROM pg_catalog.pg_namespace n
LEFT JOIN pg_catalog.pg_class c
  ON c.relnamespace = n.oid AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition
WHERE n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'
ORDER BY n.nspname, c.relname";

/// Columns of those relations. `d` resolves each domain (also a domain over a domain) to its
/// non-domain base type.
const COLUMNS_SQL: &str = "\
WITH RECURSIVE d(root, cur) AS (
    SELECT oid, typbasetype FROM pg_catalog.pg_type WHERE typtype = 'd'
  UNION ALL
    SELECT d.root, t.typbasetype FROM d JOIN pg_catalog.pg_type t ON t.oid = d.cur AND t.typtype = 'd'
), base(root, base) AS (
    SELECT d.root, d.cur FROM d JOIN pg_catalog.pg_type t ON t.oid = d.cur AND t.typtype <> 'd'
)
SELECT c.oid::text, a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod), a.atttypid::text,
       bt.typname, bt.typcategory::text, a.attnotnull::text,
       (a.atthasdef OR a.attidentity IN ('a', 'd'))::text, (a.attgenerated <> '')::text, (a.attidentity = 'a')::text
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
JOIN pg_catalog.pg_type t ON t.oid = a.atttypid
LEFT JOIN base ON base.root = t.oid
JOIN pg_catalog.pg_type bt ON bt.oid = COALESCE(base.base, t.oid)
WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition
  AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'
ORDER BY c.oid, a.attnum";

/// Primary, unique and foreign keys, one row per key column in key order. Constraints copied
/// down to partitions (`conparentid <> 0`) are skipped, so a foreign key to a partitioned table
/// names the parent.
const CONSTRAINTS_SQL: &str = "\
SELECT con.oid::text, con.conrelid::text, con.contype::text, con.conname, att.attname,
       fn.nspname, fc.relname, fatt.attname
FROM pg_catalog.pg_constraint con
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
JOIN pg_catalog.pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum
LEFT JOIN LATERAL unnest(con.confkey) WITH ORDINALITY AS fk(attnum, ord) ON con.contype = 'f' AND fk.ord = k.ord
LEFT JOIN pg_catalog.pg_class fc ON con.contype = 'f' AND fc.oid = con.confrelid
LEFT JOIN pg_catalog.pg_namespace fn ON fn.oid = fc.relnamespace
LEFT JOIN pg_catalog.pg_attribute fatt ON fatt.attrelid = con.confrelid AND fatt.attnum = fk.attnum
WHERE con.contype IN ('p', 'u', 'f') AND con.conparentid = 0
  AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition
  AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'
ORDER BY con.conrelid, con.conname, con.oid, k.ord";

fn bad(what: &str) -> PgError {
    PgError::Other(format!("unexpected catalog data ({what})"))
}

/// A text cell that must not be NULL.
fn text<'a>(row: &'a [Cell], i: usize, what: &str) -> Result<&'a str, PgError> {
    row.get(i).and_then(|c| c.as_deref()).ok_or_else(|| bad(what))
}

fn owned(row: &[Cell], i: usize, what: &str) -> Result<String, PgError> {
    text(row, i, what).map(str::to_owned)
}

fn boolean(row: &[Cell], i: usize, what: &str) -> Result<bool, PgError> {
    match text(row, i, what)? {
        "t" | "true" => Ok(true),
        "f" | "false" => Ok(false),
        _ => Err(bad(what)),
    }
}

fn number<T: std::str::FromStr>(row: &[Cell], i: usize, what: &str) -> Result<T, PgError> {
    text(row, i, what)?.parse().map_err(|_| bad(what))
}

fn rel_kind(s: &str) -> Result<RelKind, PgError> {
    match s {
        "r" => Ok(RelKind::Table),
        "p" => Ok(RelKind::PartitionedTable),
        "v" => Ok(RelKind::View),
        "m" => Ok(RelKind::MaterializedView),
        "f" => Ok(RelKind::ForeignTable),
        _ => Err(bad("relkind")),
    }
}

/// `reltuples` text as a row estimate; negative (never analyzed) is `None`.
fn estimate(s: &str) -> Option<u64> {
    let v: f64 = s.parse().ok()?;
    (v.is_finite() && v >= 0.0).then(|| v.round() as u64)
}

/// A catalog result that was cut by the limits is an error: a partial catalog would be wrong.
fn require_complete(r: ResultSet) -> Result<ResultSet, PgError> {
    if r.complete { Ok(r) } else { Err(PgError::Other(format!("catalog too large: stopped after {} rows", r.rows_seen))) }
}

async fn run(conn: &PgConnection, sql: &str) -> Result<ResultSet, PgError> {
    require_complete(query::run_untyped(conn, sql, CATALOG_LIMITS).await?)
}

/// Load every schema, relation, column and key. User schemas come first by name, then
/// `information_schema` and `pg_catalog`; tables by name; columns by `attnum`.
pub async fn load_catalog(conn: &PgConnection) -> Result<Catalog, PgError> {
    let relations = run(conn, RELATIONS_SQL).await?;
    let columns = run(conn, COLUMNS_SQL).await?;
    let constraints = run(conn, CONSTRAINTS_SQL).await?;

    let mut schemas: Vec<SchemaInfo> = Vec::new();
    // relation oid -> (schema index, table index)
    let mut index: HashMap<u32, (usize, usize)> = HashMap::new();
    for row in &relations.rows {
        let schema = owned(row, 0, "schema name")?;
        if schemas.last().map(|s| s.name.as_str()) != Some(schema.as_str()) {
            let system = schema == "pg_catalog" || schema == "information_schema";
            schemas.push(SchemaInfo { name: schema.clone(), system, tables: Vec::new() });
        }
        if row.get(1).is_some_and(Option::is_none) {
            continue; // a schema without relations
        }
        let oid: u32 = number(row, 1, "relation oid")?;
        let table = TableInfo {
            table: TableRef { schema, name: owned(row, 2, "relation name")? },
            kind: rel_kind(text(row, 3, "relkind")?)?,
            columns: Vec::new(),
            primary_key: Vec::new(),
            unique_keys: Vec::new(),
            foreign_keys: Vec::new(),
            estimated_rows: estimate(text(row, 4, "reltuples")?),
        };
        let si = schemas.len() - 1;
        let tables = &mut schemas[si].tables;
        index.insert(oid, (si, tables.len()));
        tables.push(table);
    }

    for row in &columns.rows {
        let oid: u32 = number(row, 0, "column relation")?;
        let Some(&(si, ti)) = index.get(&oid) else { continue };
        let category = text(row, 5, "typcategory")?.chars().next().ok_or_else(|| bad("typcategory"))?;
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: owned(row, 1, "column name")?,
            type_sql: owned(row, 2, "format_type")?,
            type_oid: number(row, 3, "type oid")?,
            base_type: owned(row, 4, "typname")?,
            category,
            not_null: boolean(row, 6, "attnotnull")?,
            has_default: boolean(row, 7, "has default")?,
            generated: match (boolean(row, 8, "attgenerated")?, boolean(row, 9, "attidentity")?) {
                (true, _) => Generated::Expression,
                (false, true) => Generated::Identity,
                (false, false) => Generated::No,
            },
        });
    }

    // Rows arrive grouped by constraint, in key-column order.
    let mut current: Option<(String, u32, char, String, Vec<String>, Vec<String>, Option<TableRef>)> = None;
    let mut keys = Vec::new();
    for row in &constraints.rows {
        let con = owned(row, 0, "constraint oid")?;
        if current.as_ref().is_none_or(|c| c.0 != con) {
            if let Some(done) = current.take() {
                keys.push(done);
            }
            let kind = text(row, 2, "contype")?.chars().next().ok_or_else(|| bad("contype"))?;
            let target = match (kind, row.get(5).and_then(|c| c.as_deref()), row.get(6).and_then(|c| c.as_deref())) {
                ('f', Some(s), Some(n)) => Some(TableRef { schema: s.to_owned(), name: n.to_owned() }),
                ('f', _, _) => return Err(bad("foreign key target")),
                _ => None,
            };
            current = Some((con, number(row, 1, "constraint relation")?, kind, owned(row, 3, "constraint name")?, Vec::new(), Vec::new(), target));
        }
        if let Some(c) = current.as_mut() {
            c.4.push(owned(row, 4, "key column")?);
            if c.2 == 'f' {
                c.5.push(owned(row, 7, "referenced column")?);
            }
        }
    }
    keys.extend(current);
    for (_, rel, kind, name, cols, target_cols, target) in keys {
        let Some(&(si, ti)) = index.get(&rel) else { continue };
        let t = &mut schemas[si].tables[ti];
        match (kind, target) {
            ('p', _) => t.primary_key = cols,
            ('u', _) => t.unique_keys.push(cols),
            ('f', Some(target)) => t.foreign_keys.push(ForeignKey { name, columns: cols, target, target_columns: target_cols }),
            _ => {}
        }
    }

    // User schemas first, then system ones; by name within each group. Tables are already by
    // name from the query's ORDER BY (database collation), so sort them the same way here.
    schemas.sort_by(|a, b| a.system.cmp(&b.system).then_with(|| a.name.cmp(&b.name)));
    for s in &mut schemas {
        s.tables.sort_by(|a, b| a.table.name.cmp(&b.table.name));
    }
    Ok(Catalog { schemas })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_result_is_an_error() {
        let cut = ResultSet { complete: false, rows_seen: 7, ..ResultSet::default() };
        let err = require_complete(cut).expect_err("must fail");
        assert!(err.to_string().contains("catalog too large"), "{err}");
        assert!(require_complete(ResultSet { complete: true, ..ResultSet::default() }).is_ok());
    }
}

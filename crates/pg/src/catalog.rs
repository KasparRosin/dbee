//! Catalog loading: schemas, relations, columns and keys, from three fixed catalog queries whose
//! rows go to `dbee_driver`'s [`CatalogBuilder`].
//!
//! The queries read only `pg_namespace`, `pg_class`, `pg_attribute`, `pg_type` and
//! `pg_constraint` columns that exist in Postgres 14 and every later version, and take no user
//! input. Results are simple-query text (`t`/`f` booleans, decimal numbers). Key columns come
//! from `unnest(conkey) WITH ORDINALITY`, so no array text is parsed.
//!
//! Limits: a catalog must never be truncated, so the queries run with `CATALOG_LIMITS`, which no
//! real catalog reaches, instead of the 10,000-row query cap: a database with a few hundred
//! tables already has more columns than that.

use dbee_core::cell::{ResultSet, TypeClass};
use dbee_core::schema::{Catalog, ColumnInfo, Generated, RelKind, TableRef};
use dbee_driver::catalog::{ColumnRow, ForeignKeyRow, KeyKind, KeyRow, TableRow, bad, boolean, number, owned, require_complete, text};
use dbee_driver::{CATALOG_LIMITS, CatalogBuilder, DbError};

use crate::connect::PgConnection;
use crate::query;

/// Postgres's own schemas, listed after the user's.
const SYSTEM_SCHEMAS: &[&str] = &["pg_catalog", "information_schema"];

/// The schema shown first.
const DEFAULT_SCHEMA: &str = "public";

/// Schemas and relations. A schema without relations yields one row with NULL relation columns.
/// Partitions (`relispartition`) are hidden; the partitioned parent (`p`) is shown.
const RELATIONS_SQL: &str = "\
SELECT n.nspname, c.relname, c.relkind::text, c.reltuples::numeric::text
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
SELECT n.nspname, c.relname, a.attnum::text, a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod),
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
SELECT n.nspname, c.relname, con.contype::text, con.conname, k.ord::text, att.attname,
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

/// The class of a column from its base type's `typname` and `typcategory` (a domain has its base
/// type's): numbers `N`, booleans `B`, `text`, `xml` and `citext` long text, other strings `S`
/// text, `json` and `jsonb`, dates and times `D`, `bytea`; arrays and everything else `Other`.
pub(crate) fn class_for(typname: &str, category: char) -> TypeClass {
    match (typname, category) {
        ("json" | "jsonb", _) => TypeClass::Json,
        ("bytea", _) => TypeClass::Binary,
        ("text" | "xml" | "citext", _) => TypeClass::LongText,
        (_, 'N') => TypeClass::Number,
        (_, 'B') => TypeClass::Bool,
        (_, 'S') => TypeClass::Text,
        (_, 'D') => TypeClass::DateTime,
        _ => TypeClass::Other,
    }
}

fn rel_kind(s: &str) -> Result<RelKind, DbError> {
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

async fn run(conn: &PgConnection, sql: &str) -> Result<ResultSet, DbError> {
    require_complete(query::run_untyped(conn, sql, CATALOG_LIMITS).await?)
}

/// Load every schema, relation, column and key. User schemas come first by name, then
/// `information_schema` and `pg_catalog`; tables by name; columns by `attnum`. `public` is the
/// default schema, when it exists.
pub async fn load_catalog(conn: &PgConnection) -> Result<Catalog, DbError> {
    let relations = run(conn, RELATIONS_SQL).await?;
    let columns = run(conn, COLUMNS_SQL).await?;
    let constraints = run(conn, CONSTRAINTS_SQL).await?;
    let mut catalog = CatalogBuilder::new(SYSTEM_SCHEMAS);

    for row in &relations.rows {
        let schema = owned(row, 0, "schema name")?;
        catalog.schema(schema.clone());
        if row.get(1).is_some_and(Option::is_none) {
            continue; // a schema without relations
        }
        catalog.table(TableRow {
            table: TableRef { schema, name: owned(row, 1, "relation name")? },
            kind: rel_kind(text(row, 2, "relkind")?)?,
            estimated_rows: estimate(text(row, 3, "reltuples")?),
            no_edit_reason: None,
        });
    }

    for row in &columns.rows {
        let category = text(row, 6, "typcategory")?.chars().next().ok_or_else(|| bad("typcategory"))?;
        let base_type = owned(row, 5, "typname")?;
        let column = ColumnInfo {
            name: owned(row, 3, "column name")?,
            type_sql: owned(row, 4, "format_type")?,
            class: class_for(&base_type, category),
            base_type,
            not_null: boolean(row, 7, "attnotnull")?,
            has_default: boolean(row, 8, "has default")?,
            generated: match (boolean(row, 9, "attgenerated")?, boolean(row, 10, "attidentity")?) {
                (true, _) => Generated::Expression,
                (false, true) => Generated::Identity,
                (false, false) => Generated::No,
            },
            hidden: false,
        };
        let table = TableRef { schema: owned(row, 0, "column schema")?, name: owned(row, 1, "column relation")? };
        catalog.column(ColumnRow { table, position: number(row, 2, "attnum")?, column });
    }

    for row in &constraints.rows {
        let table = TableRef { schema: owned(row, 0, "constraint schema")?, name: owned(row, 1, "constraint relation")? };
        let constraint = owned(row, 3, "constraint name")?;
        let position = number(row, 4, "key position")?;
        let column = owned(row, 5, "key column")?;
        match text(row, 2, "contype")? {
            "p" => catalog.key(KeyRow { table, constraint, kind: KeyKind::Primary, position, column }),
            "u" => catalog.key(KeyRow { table, constraint, kind: KeyKind::Unique, position, column }),
            "f" => {
                let target = match (row.get(6).and_then(|c| c.as_deref()), row.get(7).and_then(|c| c.as_deref())) {
                    (Some(s), Some(n)) => TableRef { schema: s.to_owned(), name: n.to_owned() },
                    _ => return Err(bad("foreign key target")),
                };
                let target_column = owned(row, 8, "referenced column")?;
                catalog.foreign_key(ForeignKeyRow { table, constraint, position, column, target, target_column });
            }
            _ => return Err(bad("contype")),
        }
    }

    Ok(catalog.build(Some(DEFAULT_SCHEMA)))
}

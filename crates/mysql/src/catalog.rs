//! Catalog loading: databases, tables, columns and keys from `information_schema`, in a fixed
//! number of queries whose rows are joined here and go to `dbee_driver`'s [`CatalogBuilder`].
//!
//! The queries read only columns that MySQL 5.7 and MariaDB 10.11 already have, and are the same
//! on every version. The flavors differ in two things, which [`Flavor`] names: MariaDB makes a
//! string column with its own `json_valid` check JSON (which `CHECK_CONSTRAINTS` lists), and its
//! NULL default is the text `NULL`. A database the settings name narrows every query to it,
//! written with the MySQL dialect's literal writer. Results are text as the server prints it:
//! flags `YES`/`NO`, numbers in decimal.
//!
//! Limits: a catalog must never be truncated, so the queries run with `CATALOG_LIMITS`.

use std::collections::{HashMap, HashSet};

use dbee_config::ConnectionConfig;
use dbee_core::cell::{Cell, ResultSet};
use dbee_core::schema::{Catalog, ColumnInfo, Generated, RelKind, TableRef};
use dbee_driver::catalog::{ColumnRow, ForeignKeyRow, KeyKind, KeyRow, TableRow, bad, number, owned, require_complete, text};
use dbee_driver::{CATALOG_LIMITS, CatalogBuilder, DbError, Target};
use dbee_sql::Dialect;

use crate::connect::{self, MySqlConnection};
use crate::flavor::Flavor;
use crate::values::{TypeFacts, class_of};

/// The server's own databases, listed after the user's.
const SYSTEM_SCHEMAS: &[&str] = &["mysql", "information_schema", "performance_schema", "sys"];

/// A query on one `information_schema` table.
struct Query {
    /// `SELECT … FROM information_schema.…`.
    select: &'static str,
    /// The condition every row meets, if any.
    filter: Option<&'static str>,
    /// The column that names a row's database.
    schema_column: &'static str,
}

impl Query {
    /// The query's SQL, narrowed to `database` when one is given.
    fn sql(&self, database: Option<&str>) -> Result<String, DbError> {
        let narrowed = match database {
            Some(name) => {
                // A name, which MySQL keeps free of NUL, though a literal could carry one.
                let literal = Dialect::MySql.ident(name).and_then(|_| Dialect::MySql.literal(name));
                let name = literal.map_err(|e| DbError::Other(format!("the database name cannot be used: {e}")))?;
                Some(format!("{} = {name}", self.schema_column))
            }
            None => None,
        };
        let conditions: Vec<&str> = self.filter.into_iter().chain(narrowed.as_deref()).collect();
        if conditions.is_empty() { Ok(self.select.to_owned()) } else { Ok(format!("{} WHERE {}", self.select, conditions.join(" AND "))) }
    }
}

const SCHEMATA: Query = Query { select: "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA", filter: None, schema_column: "SCHEMA_NAME" };

const TABLES: Query = Query {
    select: "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE, ENGINE, TABLE_ROWS FROM information_schema.TABLES",
    filter: None,
    schema_column: "TABLE_SCHEMA",
};

const COLUMNS: Query = Query {
    select: "SELECT TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION, COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, CHARACTER_SET_NAME, IS_NULLABLE, \
             COLUMN_DEFAULT, EXTRA FROM information_schema.COLUMNS",
    filter: None,
    schema_column: "TABLE_SCHEMA",
};

/// Which keys are primary and which unique; a foreign key's columns say so themselves.
const KEYS: Query = Query {
    select: "SELECT TABLE_SCHEMA, TABLE_NAME, CONSTRAINT_NAME, CONSTRAINT_TYPE FROM information_schema.TABLE_CONSTRAINTS",
    filter: Some("CONSTRAINT_TYPE IN ('PRIMARY KEY', 'UNIQUE')"),
    schema_column: "TABLE_SCHEMA",
};

/// The columns of every primary, unique and foreign key, a foreign key's with the column it
/// references, in any database.
const KEY_COLUMNS: Query = Query {
    select: "SELECT TABLE_SCHEMA, TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION, COLUMN_NAME, REFERENCED_TABLE_SCHEMA, \
             REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE",
    filter: None,
    schema_column: "TABLE_SCHEMA",
};

/// Unique keys with a part that is an expression, not a column (MySQL 8's functional key
/// parts). `KEY_COLUMN_USAGE` lists them without that part, so their listed columns are not
/// unique, and they are left out.
const FUNCTIONAL_KEYS: Query = Query {
    select: "SELECT TABLE_SCHEMA, TABLE_NAME, INDEX_NAME FROM information_schema.STATISTICS",
    filter: Some("NON_UNIQUE = 0 AND COLUMN_NAME IS NULL"),
    schema_column: "TABLE_SCHEMA",
};

/// MariaDB's checks on one column, ``json_valid(`col`)`` and others that start alike: a column's
/// own check makes it JSON, a table's does not.
const JSON_CHECKS: Query = Query {
    select: "SELECT CONSTRAINT_SCHEMA, TABLE_NAME, CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS",
    filter: Some("CHECK_CLAUSE LIKE 'json_valid(%' AND LEVEL = 'Column'"),
    schema_column: "CONSTRAINT_SCHEMA",
};

const ENGINES_SQL: &str = "SELECT ENGINE, TRANSACTIONS FROM information_schema.ENGINES";

/// Why the grid cannot edit a table of `engine`, which keeps no transactions.
fn no_edit_reason(engine: &str) -> String {
    format!("The {engine} table engine can't undo a write, so Apply couldn't be all or nothing; use a query tab to change it.")
}

/// Cell `i` of `row`, which may be NULL; a missing cell is [`bad`].
fn nullable<'a>(row: &'a [Cell], i: usize, what: &str) -> Result<Option<&'a str>, DbError> {
    row.get(i).map(Option::as_deref).ok_or_else(|| bad(what))
}

/// A flag as `information_schema` spells it: `YES` or `NO`.
fn yes(row: &[Cell], i: usize, what: &str) -> Result<bool, DbError> {
    match text(row, i, what)? {
        "YES" => Ok(true),
        "NO" => Ok(false),
        _ => Err(bad(what)),
    }
}

/// The kind of a `TABLE_TYPE`; `None` for one dbee does not show (MariaDB's `SEQUENCE`).
fn rel_kind(table_type: &str) -> Option<RelKind> {
    match table_type {
        "BASE TABLE" | "SYSTEM VERSIONED" => Some(RelKind::Table),
        "VIEW" | "SYSTEM VIEW" => Some(RelKind::View),
        _ => None,
    }
}

/// Whether the server fills the column when an insert leaves it out, other than with a NULL
/// default: a default that is not NULL (`default` is `COLUMN_DEFAULT`), an auto-increment
/// counter, MySQL 8's expression default, or a generated column (as Postgres's `atthasdef`).
/// `extra` is `EXTRA` in lowercase.
fn has_default(flavor: Flavor, default: Option<&str>, extra: &str) -> bool {
    let not_null_default = default.is_some_and(|d| Some(d) != flavor.null_default());
    not_null_default || extra.contains("auto_increment") || extra.contains("default_generated") || generated(extra) == Generated::Expression
}

/// A generated column, virtual or stored, by its lowercase `EXTRA`.
fn generated(extra: &str) -> Generated {
    if extra.contains("virtual generated") || extra.contains("stored generated") { Generated::Expression } else { Generated::No }
}

/// The default schema when the settings name `configured`: of the databases SCHEMATA listed for
/// that name, the one spelled the same, else the one the server took it for (a server that folds
/// names to lowercase).
fn default_schema(listed: &[String], configured: Option<&str>) -> Option<String> {
    let configured = configured?;
    listed.iter().find(|name| *name == configured).or_else(|| listed.first()).cloned()
}

/// `names` with the server's own databases last, each group in the order given.
fn system_last(mut names: Vec<String>) -> Vec<String> {
    names.sort_by_key(|name| SYSTEM_SCHEMAS.contains(&name.as_str()));
    names
}

/// A connection to read the server's databases through: one with no database, whatever `target`
/// names.
pub async fn list_connection(target: Target<'_>) -> Result<MySqlConnection, DbError> {
    let config = ConnectionConfig { database: String::new(), ..target.config.clone() };
    connect::connect(Target { config: &config, ..target }).await
}

impl MySqlConnection {
    async fn catalog_query(&self, sql: &str) -> Result<ResultSet, DbError> {
        require_complete(self.run_untyped(sql, CATALOG_LIMITS).await?)
    }

    /// The databases the server shows the user, its own last.
    pub async fn list_databases(&self) -> Result<Vec<String>, DbError> {
        let listed = self.catalog_query("SHOW DATABASES").await?;
        Ok(system_last(listed.rows.iter().map(|row| owned(row, 0, "database name")).collect::<Result<_, _>>()?))
    }

    /// Load every database, table, view, column and key, or only the database the settings
    /// name, which is then the default schema. User databases come first by name, then the
    /// server's own; tables by name; columns by `ORDINAL_POSITION`.
    pub async fn load_catalog(&self) -> Result<Catalog, DbError> {
        let (rows, default_schema) = self.rows().await?;
        let dropped = rows.dropped();
        if dropped > 0 {
            log::info!("mysql: catalog: {dropped} rows named a table the catalog did not list");
        }
        Ok(rows.build(default_schema.as_deref()))
    }

    /// The catalog queries' rows, joined, not yet built into a [`Catalog`].
    pub async fn catalog_rows(&self) -> Result<CatalogBuilder, DbError> {
        Ok(self.rows().await?.0)
    }

    /// The catalog's rows, and its default schema.
    async fn rows(&self) -> Result<(CatalogBuilder, Option<String>), DbError> {
        let database = self.database.as_deref();
        let flavor = self.flavor();
        let schemata = self.catalog_query(&SCHEMATA.sql(database)?).await?;
        let tables = self.catalog_query(&TABLES.sql(database)?).await?;
        let columns = self.catalog_query(&COLUMNS.sql(database)?).await?;
        let keys = self.catalog_query(&KEYS.sql(database)?).await?;
        let key_columns = self.catalog_query(&KEY_COLUMNS.sql(database)?).await?;
        let functional = self.catalog_query(&FUNCTIONAL_KEYS.sql(database)?).await?;
        let engines = self.catalog_query(ENGINES_SQL).await?;
        let json_checks = if flavor.json_is_checked_text() { self.catalog_query(&JSON_CHECKS.sql(database)?).await?.rows } else { Vec::new() };

        let mut catalog = CatalogBuilder::new(SYSTEM_SCHEMAS);
        let listed = schemata.rows.iter().map(|row| owned(row, 0, "database name")).collect::<Result<Vec<_>, _>>()?;
        let default_schema = default_schema(&listed, database);
        for name in listed {
            catalog.schema(name);
        }
        let skipped = add_tables(&mut catalog, &tables.rows, &engines.rows)?;
        add_columns(&mut catalog, &columns.rows, &json_checks, &skipped, flavor)?;
        add_keys(&mut catalog, &keys.rows, &key_columns.rows, &functional.rows, &skipped)?;
        Ok((catalog, default_schema))
    }
}

/// The tables and views of `tables`, read-only when `engines` says their engine keeps no
/// transactions; returns the relations dbee does not show, whose other rows are left out too.
fn add_tables(catalog: &mut CatalogBuilder, tables: &[Vec<Cell>], engines: &[Vec<Cell>]) -> Result<HashSet<TableRef>, DbError> {
    let mut transactional = HashSet::new();
    for row in engines {
        if nullable(row, 1, "transactions")? == Some("YES") {
            transactional.insert(text(row, 0, "engine name")?.to_ascii_lowercase());
        }
    }
    let mut skipped = HashSet::new();
    for row in tables {
        let table = TableRef { schema: owned(row, 0, "table schema")?, name: owned(row, 1, "table name")? };
        let Some(kind) = rel_kind(text(row, 2, "table type")?) else {
            skipped.insert(table);
            continue;
        };
        let no_edit_reason = match nullable(row, 3, "engine")? {
            Some(engine) if kind == RelKind::Table && !transactional.contains(&engine.to_ascii_lowercase()) => Some(no_edit_reason(engine)),
            _ => None,
        };
        let estimated_rows = nullable(row, 4, "table rows")?.and_then(|n| n.parse().ok());
        catalog.table(TableRow { table, kind, estimated_rows, no_edit_reason });
    }
    Ok(skipped)
}

/// The columns of `columns`, MariaDB's JSON by the `json_valid` checks of `json_checks`.
fn add_columns(
    catalog: &mut CatalogBuilder,
    columns: &[Vec<Cell>],
    json_checks: &[Vec<Cell>],
    skipped: &HashSet<TableRef>,
    flavor: Flavor,
) -> Result<(), DbError> {
    let mut checks: HashMap<TableRef, Vec<&str>> = HashMap::new();
    for row in json_checks {
        let table = TableRef { schema: owned(row, 0, "check schema")?, name: owned(row, 1, "check table")? };
        checks.entry(table).or_default().push(text(row, 2, "check clause")?);
    }
    for row in columns {
        let table = TableRef { schema: owned(row, 0, "column schema")?, name: owned(row, 1, "column table")? };
        if skipped.contains(&table) {
            continue;
        }
        let name = owned(row, 3, "column name")?;
        let type_sql = owned(row, 4, "column type")?;
        let base_type = owned(row, 5, "data type")?;
        let json_valid = match checks.get(&table) {
            Some(clauses) => {
                let clause = format!("json_valid({})", Dialect::MySql.ident(&name).map_err(|_| bad("column name"))?);
                clauses.contains(&clause.as_str())
            }
            None => false,
        };
        let facts = TypeFacts::from_catalog(&base_type, &type_sql, nullable(row, 6, "character set")?, json_valid);
        let extra = nullable(row, 9, "extra")?.unwrap_or_default().to_ascii_lowercase();
        let column = ColumnInfo {
            name,
            type_sql,
            base_type,
            class: class_of(&facts),
            not_null: !yes(row, 7, "is nullable")?,
            has_default: has_default(flavor, nullable(row, 8, "column default")?, &extra),
            generated: generated(&extra),
            hidden: false,
        };
        catalog.column(ColumnRow { table, position: number(row, 2, "ordinal position")?, column });
    }
    Ok(())
}

/// The primary and unique keys of `keys` but the `functional` ones, with their columns from
/// `key_columns`, and the foreign keys, whose columns name the columns they reference; each
/// table's keys by name, as Postgres lists them.
fn add_keys(
    catalog: &mut CatalogBuilder,
    keys: &[Vec<Cell>],
    key_columns: &[Vec<Cell>],
    functional: &[Vec<Cell>],
    skipped: &HashSet<TableRef>,
) -> Result<(), DbError> {
    let mut left_out = HashSet::new();
    for row in functional {
        let table = TableRef { schema: owned(row, 0, "index schema")?, name: owned(row, 1, "index table")? };
        left_out.insert((table, text(row, 2, "index name")?));
    }
    let mut key_kinds: HashMap<(TableRef, &str), KeyKind> = HashMap::new();
    for row in keys {
        let table = TableRef { schema: owned(row, 0, "key schema")?, name: owned(row, 1, "key table")? };
        let kind = match text(row, 3, "constraint type")? {
            "PRIMARY KEY" => KeyKind::Primary,
            "UNIQUE" => KeyKind::Unique,
            _ => return Err(bad("constraint type")),
        };
        let key = (table, text(row, 2, "constraint name")?);
        if !left_out.contains(&key) {
            key_kinds.insert(key, kind);
        }
    }
    let (mut key_rows, mut foreign_rows) = (Vec::new(), Vec::new());
    for row in key_columns {
        let table = TableRef { schema: owned(row, 0, "key column schema")?, name: owned(row, 1, "key column table")? };
        if skipped.contains(&table) {
            continue;
        }
        let constraint = owned(row, 2, "constraint name")?;
        let position = number(row, 3, "key position")?;
        let column = owned(row, 4, "key column")?;
        match (nullable(row, 5, "referenced schema")?, nullable(row, 6, "referenced table")?) {
            (Some(schema), Some(name)) => {
                let target = TableRef { schema: schema.to_owned(), name: name.to_owned() };
                let target_column = owned(row, 7, "referenced column")?;
                foreign_rows.push(ForeignKeyRow { table, constraint, position, column, target, target_column });
            }
            (None, None) => {
                // A key left out, or one that went after the keys were read.
                if let Some(&kind) = key_kinds.get(&(table.clone(), constraint.as_str())) {
                    key_rows.push(KeyRow { table, constraint, kind, position, column });
                }
            }
            _ => return Err(bad("foreign key target")),
        }
    }
    key_rows.sort_by(|a, b| (&a.table.schema, &a.table.name, &a.constraint).cmp(&(&b.table.schema, &b.table.name, &b.constraint)));
    foreign_rows.sort_by(|a, b| (&a.table.schema, &a.table.name, &a.constraint).cmp(&(&b.table.schema, &b.table.name, &b.constraint)));
    key_rows.into_iter().for_each(|row| catalog.key(row));
    foreign_rows.into_iter().for_each(|row| catalog.foreign_key(row));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_own_databases_go_last_in_the_order_given() {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        let given = names(&["information_schema", "many", "mysql", "odd", "performance_schema", "shop", "sys", "MySQL_backup"]);
        let want = names(&["many", "odd", "shop", "MySQL_backup", "information_schema", "mysql", "performance_schema", "sys"]);
        assert_eq!(system_last(given), want);
        assert_eq!(system_last(Vec::new()), Vec::<String>::new());
    }

    #[test]
    fn flags_as_information_schema_spells_them() {
        let row: Vec<Cell> = vec![Some("YES".into()), Some("NO".into()), Some("1".into()), Some("yes".into()), None];
        assert_eq!((yes(&row, 0, "f"), yes(&row, 1, "f")), (Ok(true), Ok(false)));
        for i in [2, 3, 4, 5] {
            assert!(yes(&row, i, "f").is_err_and(|e| e.to_string() == "unexpected catalog data (f)"), "{i}");
        }
        assert_eq!((nullable(&row, 4, "n"), nullable(&row, 0, "n")), (Ok(None), Ok(Some("YES"))));
        assert!(nullable(&row, 5, "n").is_err(), "a missing cell is not NULL");
    }

    #[test]
    fn defaults_as_each_flavor_writes_them() {
        use Flavor::*;
        for (flavor, default, extra, expected) in [
            // MySQL: SQL NULL for no default and for a NULL default; any text is a default.
            (MySql, None, "", false),
            (MySql, Some("NULL"), "", true),
            (MySql, Some(""), "", true),
            (MySql, Some("0000-00-00"), "", true),
            (MySql, Some("CURRENT_TIMESTAMP"), "default_generated on update current_timestamp", true),
            (MySql, None, "on update current_timestamp", false),
            (MySql, None, "auto_increment", true),
            (MySql, None, "virtual generated", true),
            (MySql, None, "stored generated", true),
            // MariaDB: the text NULL for a NULL default, and string defaults quoted.
            (MariaDb, Some("NULL"), "", false),
            (MariaDb, None, "", false),
            (MariaDb, Some("'NULL'"), "", true),
            (MariaDb, Some("''"), "", true),
            (MariaDb, Some("current_timestamp()"), "on update current_timestamp()", true),
            (MariaDb, Some("NULL"), "on update current_timestamp()", false),
            (MariaDb, None, "auto_increment", true),
            // A generated column: the server fills it, as Postgres's atthasdef says.
            (MariaDb, Some("NULL"), "stored generated", true),
            (MariaDb, Some("NULL"), "stored generated, invisible", true),
        ] {
            assert_eq!(has_default(flavor, default, extra), expected, "{flavor:?} {default:?} {extra:?}");
        }
    }

    #[test]
    fn generated_columns_by_extra() {
        for extra in ["virtual generated", "stored generated", "stored generated, invisible"] {
            assert_eq!(generated(extra), Generated::Expression, "{extra}");
        }
        for extra in ["", "auto_increment", "default_generated", "default_generated on update current_timestamp", "invisible"] {
            assert_eq!(generated(extra), Generated::No, "{extra}");
        }
    }

    #[test]
    fn table_types() {
        assert_eq!(rel_kind("BASE TABLE"), Some(RelKind::Table));
        assert_eq!(rel_kind("SYSTEM VERSIONED"), Some(RelKind::Table));
        assert_eq!((rel_kind("VIEW"), rel_kind("SYSTEM VIEW")), (Some(RelKind::View), Some(RelKind::View)));
        assert_eq!((rel_kind("SEQUENCE"), rel_kind("TEMPORARY")), (None, None));
    }

    #[test]
    fn the_settings_database_narrows_every_query() {
        assert_eq!(SCHEMATA.sql(None).unwrap(), "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA");
        assert_eq!(SCHEMATA.sql(Some("shop")).unwrap(), "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = 'shop'");
        let keys = KEYS.sql(Some("it's")).unwrap();
        assert!(keys.ends_with(" WHERE CONSTRAINT_TYPE IN ('PRIMARY KEY', 'UNIQUE') AND TABLE_SCHEMA = 'it''s'"), "{keys}");
        // A backslash is safe whatever the session's mode: the name goes as hex.
        let checks = JSON_CHECKS.sql(Some("a\\b")).unwrap();
        assert!(checks.ends_with(" AND CONSTRAINT_SCHEMA = _utf8mb4 X'615C62'"), "{checks}");
        assert_eq!(TABLES.sql(Some("a\0b")).unwrap_err().code(), "other");
        assert!(JSON_CHECKS.sql(None).unwrap().ends_with(" WHERE CHECK_CLAUSE LIKE 'json_valid(%' AND LEVEL = 'Column'"));
        let functional = FUNCTIONAL_KEYS.sql(Some("shop")).unwrap();
        assert!(functional.ends_with(" WHERE NON_UNIQUE = 0 AND COLUMN_NAME IS NULL AND TABLE_SCHEMA = 'shop'"), "{functional}");
    }

    #[test]
    fn the_default_schema_as_the_server_spells_it() {
        let listed = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(default_schema(&listed(&["shop"]), Some("shop")).as_deref(), Some("shop"));
        // A server that folds names to lowercase lists the database it took the settings' name for.
        assert_eq!(default_schema(&listed(&["shop"]), Some("Shop")).as_deref(), Some("shop"));
        assert_eq!(default_schema(&listed(&["shop", "Shop"]), Some("Shop")).as_deref(), Some("Shop"));
        assert_eq!(default_schema(&listed(&[]), Some("shop")), None);
        assert_eq!(default_schema(&listed(&["shop", "odd"]), None), None);
    }

    #[test]
    fn the_reason_names_the_engine() {
        assert_eq!(
            no_edit_reason("MyISAM"),
            "The MyISAM table engine can't undo a write, so Apply couldn't be all or nothing; use a query tab to change it."
        );
    }
}

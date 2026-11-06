//! Catalog tests against every MySQL and MariaDB server in `db.servers` (`cargo xtask testdb up`).
//! Every load also checks that no row named a table the catalog did not list. Tests that write
//! make their own `scratch` table and drop it; their DDL holds [`DDL`] for writing, so no load
//! runs while a table comes or goes.

use std::time::Instant;

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_core::{Catalog, Generated, RelKind, TableInfo, TableRef, TypeClass};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_mysql::{MySqlConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};
use tokio::sync::RwLock;

/// The MySQL and MariaDB test servers.
fn mysql_servers(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::MySql, Kind::MariaDb])
}

/// Loads hold it for reading; a test's DDL for writing.
static DDL: RwLock<()> = RwLock::const_new(());

fn tr(schema: &str, name: &str) -> TableRef {
    TableRef { schema: schema.into(), name: name.into() }
}

fn mariadb(s: &Server) -> bool {
    s.kind == Kind::MariaDb
}

/// `root@127.0.0.1`, on `database` when it is not empty.
fn config(s: &Server, database: &str) -> ConnectionConfig {
    ConnectionConfig {
        name: format!("test {}", s.name),
        kind: if mariadb(s) { DbKind::MariaDb } else { DbKind::MySql },
        host: "127.0.0.1".into(),
        port: s.port,
        user: "root".into(),
        database: database.into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

async fn open(s: &Server, db: &TestDb, database: &str) -> MySqlConnection {
    let c = config(s, database);
    match connect(Target { config: &c, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None })
        .await
    {
        Ok(conn) => conn,
        Err(e) => panic!("connect to {} failed: {e}", s.name),
    }
}

/// The catalog of `conn`, whose rows must all name a listed table.
async fn try_load(conn: &MySqlConnection, s: &Server) -> Result<Catalog, String> {
    let _no_ddl = DDL.read().await;
    let rows = conn.catalog_rows().await.map_err(|e| format!("catalog rows on {}: {e}", s.name))?;
    if rows.dropped() != 0 {
        return Err(format!("{}: {} rows named a table the catalog did not list", s.name, rows.dropped()));
    }
    conn.load_catalog().await.map_err(|e| format!("load_catalog on {}: {e}", s.name))
}

async fn load(conn: &MySqlConnection, s: &Server) -> Catalog {
    try_load(conn, s).await.unwrap_or_else(|e| panic!("{e}"))
}

/// Every server's whole catalog.
async fn catalogs(db: &TestDb) -> Vec<(&Server, Catalog)> {
    let mut out = Vec::new();
    for s in mysql_servers(db) {
        let conn = open(s, db, "").await;
        out.push((s, load(&conn, s).await));
    }
    out
}

fn table<'a>(cat: &'a Catalog, s: &Server, schema: &str, name: &str) -> &'a TableInfo {
    cat.table(&tr(schema, name)).unwrap_or_else(|| panic!("{}: no {schema}.{name}", s.name))
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(name).collect()
}

/// Run `sql` on `conn` while no load runs.
async fn ddl(conn: &MySqlConnection, sql: &str) -> Result<(), String> {
    let _alone = DDL.write().await;
    conn.run(sql, QUERY_LIMITS).await.map(|_| ()).map_err(|e| format!("{sql}: {e}"))
}

#[tokio::test]
async fn shop_tables_with_keys() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let orders = table(&cat, s, "shop", "orders");
        assert_eq!((orders.kind, orders.primary_key.clone()), (RelKind::Table, vec!["id".to_string()]), "{}", s.name);
        let fk = orders.fk_for("customer_id").expect("fk");
        assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("shop", "customers"), vec!["id".to_string()]), "{}", s.name);
        assert_eq!(&names(&orders.columns, |c| &c.name)[..3], ["id", "customer_id", "dealer_id"], "{}: ORDINAL_POSITION order", s.name);
        let total = orders.column("total").expect("total");
        assert_eq!((total.type_sql.as_str(), total.base_type.as_str(), total.class), ("decimal(12,2)", "decimal", TypeClass::Number));
        assert!(!total.has_default && !total.not_null, "{}: {total:?}", s.name);
        assert!(orders.no_edit_reason.is_none() && orders.row_key().is_some(), "{}: InnoDB can be edited", s.name);
        assert!(orders.estimated_rows.is_some_and(|n| n > 0), "{}: {:?}", s.name, orders.estimated_rows);
        assert_eq!(table(&cat, s, "shop", "order_items").primary_key, ["order_id", "line_no"], "{}", s.name);
        let customers = table(&cat, s, "shop", "customers");
        assert_eq!(customers.unique_keys, vec![vec!["email".to_string()]], "{}", s.name);
        assert!(customers.column("name").expect("name").not_null);
        let ship = table(&cat, s, "shop", "shipments");
        assert_eq!(ship.foreign_keys.len(), 1, "{}", s.name);
        assert_eq!(ship.foreign_keys[0].columns, ["order_id", "line_no"], "{}", s.name);
        assert_eq!(ship.foreign_keys[0].target_columns, ["order_id", "line_no"], "{}", s.name);
        assert_eq!(ship.foreign_keys[0].target, tr("shop", "order_items"), "{}", s.name);
        let no_key = table(&cat, s, "shop", "no_key");
        assert!(no_key.row_key().is_none() && no_key.primary_key.is_empty() && no_key.unique_keys.is_empty(), "{}", s.name);
        // A table with partitions is one table.
        assert_eq!(table(&cat, s, "shop", "events").kind, RelKind::Table, "{}", s.name);
        for view in ["order_totals", "customer_counts"] {
            let v = table(&cat, s, "shop", view);
            assert_eq!((v.kind, v.estimated_rows), (RelKind::View, None), "{}: {view}", s.name);
            assert!(v.row_key().is_none() && v.primary_key.is_empty() && v.foreign_keys.is_empty(), "{}: {view}", s.name);
            assert!(v.no_edit_reason.is_none(), "{}: {view}", s.name);
        }
        assert_eq!(table(&cat, s, "shop", "order_totals").columns.len(), 3, "{}", s.name);
    }
}

#[tokio::test]
async fn system_databases_last_and_flagged() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let listed = names(&cat.schemas, |x| &x.name);
        let expected = ["many", "odd", "scratch", "shop", "information_schema", "mysql", "performance_schema", "sys"];
        assert_eq!(listed, expected, "{}", s.name);
        let system: Vec<bool> = cat.schemas.iter().map(|x| x.system).collect();
        assert_eq!(system, [false, false, false, false, true, true, true, true], "{}", s.name);
        assert_eq!(cat.default_schema, None, "{}", s.name);
        assert_eq!(table(&cat, s, "information_schema", "TABLES").kind, RelKind::View, "{}: a system view", s.name);
        assert!(table(&cat, s, "mysql", "user").columns.iter().any(|c| c.name == "User"), "{}", s.name);
    }
}

#[tokio::test]
async fn set_database_narrows_the_list() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn = open(s, db, "shop").await;
        let cat = load(&conn, s).await;
        assert_eq!(names(&cat.schemas, |x| &x.name), ["shop"], "{}", s.name);
        assert_eq!(cat.default_schema.as_deref(), Some("shop"), "{}", s.name);
        assert_eq!(table(&cat, s, "shop", "orders").primary_key, ["id"], "{}", s.name);
        assert_eq!(table(&cat, s, "shop", "all_types").columns.len(), 20, "{}", s.name);
    }
}

#[tokio::test]
async fn myisam_table_cannot_be_edited() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let t = table(&cat, s, "shop", "audit_log");
        let reason = "The MyISAM table engine can't undo a write, so Apply couldn't be all or nothing; use a query tab to change it.";
        assert_eq!(t.no_edit_reason.as_deref(), Some(reason), "{}", s.name);
        assert!(t.row_key().is_none(), "{}", s.name);
        assert_eq!((t.primary_key.clone(), t.estimated_rows), (vec!["id".to_string()], Some(3)), "{}: the key is still known", s.name);
        // The server's own tables in an engine without transactions are read-only too.
        let threads = table(&cat, s, "performance_schema", "threads").no_edit_reason.as_deref();
        assert!(threads.is_some_and(|r| r.starts_with("The PERFORMANCE_SCHEMA table engine can't undo")), "{}: {threads:?}", s.name);
    }
}

#[tokio::test]
async fn generated_and_auto_increment() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let id = table(&cat, s, "shop", "orders").column("id").expect("id").clone();
        assert!(id.not_null && id.has_default && id.generated == Generated::No, "{}: {id:?}", s.name);
        let all = table(&cat, s, "shop", "all_types");
        let total = all.column("total").expect("total");
        assert_eq!((total.generated, total.has_default), (Generated::Expression, true), "{}: the server fills it: {total:?}", s.name);
        assert_eq!(total.not_editable(), Some("generated columns are read-only"));
        for name in ["note", "flag", "doc", "pt"] {
            let c = all.column(name).expect("column");
            assert!(!c.has_default && c.generated == Generated::No, "{}: a NULL default is none: {c:?}", s.name);
        }
        let created = table(&cat, s, "shop", "customers").column("created_at").expect("created_at").clone();
        assert!(created.has_default && !created.not_null, "{}: {created:?}", s.name);
        let legacy = table(&cat, s, "shop", "legacy_dates");
        for name in ["d", "dt"] {
            assert!(legacy.column(name).is_some_and(|c| c.has_default), "{}: the zero date default", s.name);
        }
        assert!(!legacy.column("name").expect("name").has_default, "{}", s.name);
    }
}

#[tokio::test]
async fn each_servers_defaults_keys_and_generated_columns() {
    let db = require_testdb!();
    let t = unique("t_columns");
    let persistent = |s: &Server| if mariadb(s) { ", p INT AS (x * 4) PERSISTENT" } else { "" };
    let mut loaded = Vec::new();
    for s in mysql_servers(db) {
        let conn = open(s, db, "").await;
        let create = format!(
            "CREATE TABLE scratch.`{t}` (a INT AUTO_INCREMENT, x INT NOT NULL, v INT GENERATED ALWAYS AS (x * 2) VIRTUAL, \
             s INT GENERATED ALWAYS AS (x * 3) STORED, sn VARCHAR(10) DEFAULT 'NULL', dn VARCHAR(10) DEFAULT NULL, \
             e VARCHAR(10) NOT NULL DEFAULT '', q VARCHAR(10) DEFAULT 'it''s', \
             ts TIMESTAMP NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP, u DATETIME NULL ON UPDATE CURRENT_TIMESTAMP, \
             c INT, `j``x é` JSON, lt LONGTEXT, ct TEXT CHECK (JSON_VALID(ct)){p}, \
             PRIMARY KEY (x, a), KEY (a), UNIQUE KEY z_last (e, q), UNIQUE KEY a_first (sn), FOREIGN KEY (c) REFERENCES shop.customers (id))",
            p = persistent(s)
        );
        // A MariaDB sequence is no table, and its columns are left out with it.
        let sequence = if mariadb(s) { ddl(&conn, &format!("CREATE SEQUENCE scratch.`{t}_seq`")).await } else { Ok(()) };
        let made = ddl(&conn, &create).await.and(sequence);
        let cat = match made {
            Ok(()) => try_load(&conn, s).await,
            Err(e) => Err(e),
        };
        let gone = ddl(&conn, &format!("DROP TABLE IF EXISTS scratch.`{t}`, scratch.`{t}_seq`")).await;
        loaded.push((s, cat, gone));
    }
    for (s, cat, gone) in loaded {
        let cat = cat.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        gone.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        assert!(cat.table(&tr("scratch", &format!("{t}_seq"))).is_none(), "{}", s.name);
        let table = table(&cat, s, "scratch", &t);
        let column = |name: &str| table.column(name).unwrap_or_else(|| panic!("{}: no column {name}", s.name));
        for (name, has_default) in
            [("a", true), ("x", false), ("sn", true), ("dn", false), ("e", true), ("q", true), ("ts", true), ("u", false), ("c", false)]
        {
            assert_eq!(column(name).has_default, has_default, "{}: {:?}", s.name, column(name));
        }
        for name in ["v", "s"].into_iter().chain(mariadb(s).then_some("p")) {
            assert_eq!((column(name).generated, column(name).has_default), (Generated::Expression, true), "{}: {name}", s.name);
        }
        assert_eq!(column("a").generated, Generated::No, "{}", s.name);
        assert!(column("x").not_null && column("e").not_null && !column("q").not_null, "{}", s.name);
        assert_eq!(column("j`x é").class, TypeClass::Json, "{}: JSON, also with a backtick in its name", s.name);
        // MariaDB makes a text with its own json_valid check JSON; MySQL does not.
        let checked = if mariadb(s) { TypeClass::Json } else { TypeClass::LongText };
        assert_eq!((column("lt").class, column("ct").class), (TypeClass::LongText, checked), "{}", s.name);
        // Keys in key order, not column order; unique keys by name; a foreign key into another database.
        assert_eq!(table.primary_key, ["x", "a"], "{}", s.name);
        assert_eq!(table.unique_keys, vec![vec!["sn".to_string()], vec!["e".to_string(), "q".to_string()]], "{}", s.name);
        let fk = table.fk_for("c").expect("fk");
        assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("shop", "customers"), vec!["id".to_string()]), "{}", s.name);
    }
}

#[tokio::test]
async fn mariadb_json_is_json() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let doc = table(&cat, s, "shop", "all_types").column("doc").expect("doc").clone();
        let json_type = if mariadb(s) { "longtext" } else { "json" };
        assert_eq!((doc.class, doc.base_type.as_str(), doc.type_sql.as_str()), (TypeClass::Json, json_type, json_type), "{}", s.name);
        let orders = table(&cat, s, "shop", "orders");
        assert_eq!(orders.column("billing").map(|c| c.class), Some(TypeClass::Json), "{}", s.name);
        assert_eq!(orders.column("raw_json_text").map(|c| c.class), Some(TypeClass::LongText), "{}", s.name);
        let big = table(&cat, s, "shop", "big_cell");
        assert_eq!(big.columns.iter().map(|c| c.class).collect::<Vec<_>>(), [TypeClass::Number, TypeClass::LongText, TypeClass::Json], "{}", s.name);
    }
}

/// MariaDB's JSON and its own types, as a query and the catalog see them: a column's own
/// `json_valid` check makes a string or blob JSON, a table's check does not.
const MARIADB_TYPES: &str = "(j JSON, t TEXT CHECK (JSON_VALID(t)), v VARCHAR(20) CHECK (json_valid(v)), \
    c CHAR(10) CHECK (json_valid(c)), b BLOB CHECK (json_valid(b)), e ENUM('a', '[]') CHECK (json_valid(e)), lt LONGTEXT, \
    u UUID, i6 INET6, i4 INET4, CHECK (json_valid(lt)))";

#[tokio::test]
async fn classes_as_a_query_gives_them() {
    let db = require_testdb!();
    let t = unique("t_classes");
    for s in mysql_servers(db) {
        let conn = open(s, db, "").await;
        let made = if mariadb(s) { ddl(&conn, &format!("CREATE TABLE scratch.`{t}` {MARIADB_TYPES}")).await } else { Ok(()) };
        let compared = match made {
            Ok(()) => compare_classes(&conn, s, &t).await,
            Err(e) => Err(e),
        };
        let gone = ddl(&conn, &format!("DROP TABLE IF EXISTS scratch.`{t}`")).await;
        let cat = compared.unwrap_or_else(|e| panic!("{e}"));
        gone.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let all = table(&cat, s, "shop", "all_types");
        for (name, class) in [
            ("flag", TypeClass::Bool),
            ("st", TypeClass::Text),
            ("tags", TypeClass::Text),
            ("b", TypeClass::Binary),
            ("bits", TypeClass::Binary),
            ("pt", TypeClass::Binary),
            ("ts", TypeClass::DateTime),
            ("y", TypeClass::Number),
            ("note", TypeClass::LongText),
        ] {
            assert_eq!(all.column(name).map(|c| c.class), Some(class), "{}: {name}", s.name);
        }
        if mariadb(s) {
            let own = table(&cat, s, "scratch", &t);
            let classes: Vec<(&str, TypeClass)> = own.columns.iter().map(|c| (c.name.as_str(), c.class)).collect();
            use TypeClass::*;
            let expected = [
                ("j", Json),
                ("t", Json),
                ("v", Json),
                ("c", Json),
                ("b", Json),
                ("e", Text),
                ("lt", LongText),
                ("u", Other),
                ("i6", Other),
                ("i4", Other),
            ];
            assert_eq!(classes, expected, "{}", s.name);
        }
    }
}

/// The catalog, after checking that its classes are a query's for some seed tables and, on
/// MariaDB, for the scratch table `t`.
async fn compare_classes(conn: &MySqlConnection, s: &Server, t: &str) -> Result<Catalog, String> {
    let cat = try_load(conn, s).await?;
    let mut tables = vec![("shop", "all_types"), ("odd", "Odd \"Name\" é"), ("shop", "orders"), ("shop", "legacy_dates")];
    if mariadb(s) {
        tables.push(("scratch", t));
    }
    for (schema, name) in tables {
        let listed = cat.table(&tr(schema, name)).ok_or_else(|| format!("{}: no {schema}.{name}", s.name))?;
        let sql = format!("SELECT * FROM `{schema}`.`{name}` LIMIT 0");
        let described = conn.run(&sql, QUERY_LIMITS).await.map_err(|e| format!("{}: {sql}: {e}", s.name))?;
        let by_query: Vec<(&str, TypeClass)> = described.columns.iter().map(|c| (c.name.as_str(), c.class)).collect();
        let by_catalog: Vec<(&str, TypeClass)> = listed.columns.iter().map(|c| (c.name.as_str(), c.class)).collect();
        if by_catalog != by_query {
            return Err(format!("{}: {schema}.{name}: the catalog says {by_catalog:?}, a query {by_query:?}", s.name));
        }
    }
    Ok(cat)
}

#[tokio::test]
async fn functional_key_parts_are_no_key() {
    let db = require_testdb!();
    let t = unique("t_functional");
    for s in mysql_servers(db) {
        let (conn, in_scratch) = (open(s, db, "").await, open(s, db, "scratch").await);
        let create = format!("CREATE TABLE scratch.`{t}` (a INT NOT NULL, b INT, UNIQUE KEY uk (a, (b + 1)))");
        if let Err(e) = ddl(&conn, &create).await {
            println!("{}: no functional key parts, skipped ({e})", s.name);
            continue;
        }
        // The settings' database narrows the query that finds them too.
        let whole = try_load(&conn, s).await;
        let narrowed = try_load(&in_scratch, s).await;
        let gone = ddl(&conn, &format!("DROP TABLE IF EXISTS scratch.`{t}`")).await;
        gone.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        for cat in [whole, narrowed] {
            let cat = cat.unwrap_or_else(|e| panic!("{e}"));
            let listed = table(&cat, s, "scratch", &t);
            // Its listed column, a, is not unique: the key is left out.
            assert!(listed.unique_keys.is_empty() && listed.row_key().is_none(), "{}: {:?}", s.name, listed.unique_keys);
        }
    }
}

#[tokio::test]
async fn odd_identifiers_survive() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let t = table(&cat, s, "odd", "Odd \"Name\" é");
        assert_eq!(names(&t.columns, |c| &c.name), ["id", "Total $", "lower UPPER", "Other Ödd id", "back`tick"], "{}", s.name);
        let fk = t.fk_for("Other Ödd id").expect("fk");
        assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("odd", "Other Ödd"), vec!["id".to_string()]), "{}", s.name);
        assert_eq!(t.primary_key, ["id"], "{}", s.name);
        assert_eq!(table(&cat, s, "odd", "Other Ödd").column("Label").map(|c| c.class), Some(TypeClass::LongText), "{}", s.name);
    }
}

#[tokio::test]
async fn row_estimates() {
    let db = require_testdb!();
    for (s, cat) in catalogs(db).await {
        let expected: u64 = if s.full_seed { 1_000_000 } else { 30_000 };
        let n = table(&cat, s, "shop", "big_rows").estimated_rows.expect("estimate");
        assert!((expected / 2..=expected * 2).contains(&n), "{}: {n}", s.name);
    }
}

#[tokio::test]
async fn three_hundred_tables_load() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn = open(s, db, "").await;
        load(&conn, s).await;
        let _no_ddl = DDL.read().await;
        let rows = conn.catalog_rows().await.unwrap_or_else(|e| panic!("catalog rows on {}: {e}", s.name));
        assert_eq!(rows.dropped(), 0, "{}", s.name);
        let cat = rows.build(None);
        let many = cat.schemas.iter().find(|x| x.name == "many").expect("many");
        assert_eq!((many.tables.len(), many.tables[0].table.name.as_str()), (300, "t_001"), "{}", s.name);
    }
}

#[tokio::test]
#[ignore = "timing; run with --ignored --nocapture"]
async fn three_hundred_tables_load_quickly() {
    let db = require_testdb!();
    for s in mysql_servers(db) {
        let conn = open(s, db, "").await;
        load(&conn, s).await;
        // The second load, alone, so the time is the load's own and not a cold cache's.
        let _alone = DDL.write().await;
        let start = Instant::now();
        let rows = conn.catalog_rows().await.unwrap_or_else(|e| panic!("catalog rows on {}: {e}", s.name));
        let cat = rows.build(None);
        let took = start.elapsed();
        println!("{}: the second catalog load took {took:?} ({} tables)", s.name, cat.table_count());
        assert!(took.as_secs_f64() < 2.0, "{}: took {took:?}", s.name);
    }
}

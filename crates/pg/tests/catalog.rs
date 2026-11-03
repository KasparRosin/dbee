//! Catalog tests against the Docker test servers (`cargo xtask testdb up`); every test runs
//! against every Postgres server in `db.servers`. Nothing here writes.

use std::time::Instant;

use dbee_config::{ConnectionConfig, SslMode};
use dbee_core::TypeClass;
use dbee_core::schema::{Catalog, Generated, RelKind, TableRef};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_pg::catalog::{catalog_rows, load_catalog};
use dbee_pg::query;
use dbee_pg::{PgConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

/// The Postgres test servers.
fn postgres(db: &TestDb) -> impl Iterator<Item = &Server> {
    db.servers_of(&[Kind::Postgres])
}

fn tr(schema: &str, name: &str) -> TableRef {
    TableRef { schema: schema.into(), name: name.into() }
}

async fn conn(s: &Server, db: &TestDb) -> PgConnection {
    let config = ConnectionConfig {
        name: format!("test {}", s.name),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    match connect(Target {
        config: &config,
        password: Some(&db.password),
        tunnel: None,
        purpose: Purpose::Browse,
        create_file: false,
        allowed_dirs: None,
    })
    .await
    {
        Ok(c) => c,
        Err(e) => panic!("connect to {} failed: {e}", s.name),
    }
}

async fn catalogs(db: &TestDb) -> Vec<(u32, Catalog)> {
    let mut out = Vec::new();
    for s in postgres(db) {
        let c = conn(s, db).await;
        match load_catalog(&c).await {
            Ok(cat) => out.push((s.major(), cat)),
            Err(e) => panic!("load_catalog on {}: {e}", s.name),
        }
    }
    out
}

#[tokio::test]
async fn shop_tables_with_keys() {
    let db = require_testdb!();
    for (major, cat) in catalogs(db).await {
        let orders = cat.table(&tr("shop", "orders")).expect("orders");
        assert_eq!(orders.primary_key, ["id"], "pg{major}");
        let fk = orders.fk_for("customer_id").expect("fk");
        assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("shop", "customers"), vec!["id".to_string()]));
        assert_eq!(cat.table(&tr("shop", "order_items")).expect("items").primary_key, ["order_id", "line_no"]);
        assert!(cat.table(&tr("shop", "customers")).expect("customers").unique_keys.contains(&vec!["email".to_string()]));
        let ship = cat.table(&tr("shop", "shipments")).expect("shipments");
        assert_eq!(ship.foreign_keys[0].columns, ["order_id", "line_no"]);
        assert_eq!(ship.foreign_keys[0].target_columns, ["order_id", "line_no"]);
        assert_eq!(ship.foreign_keys[0].target, tr("shop", "order_items"));
        let id = orders.column("id").expect("id");
        assert!(id.not_null && id.has_default, "bigserial id: {id:?}");
        assert!(!orders.column("total").expect("total").has_default);
        assert_eq!(orders.column("total").expect("total").type_sql, "numeric(12,2)");
        let cols: Vec<&str> = orders.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&cols[..3], ["id", "customer_id", "dealer_id"], "attnum order");
        assert_eq!(orders.kind, RelKind::Table);
        let no_key = cat.table(&tr("shop", "no_key")).expect("no_key");
        assert!(no_key.row_key().is_none() && no_key.primary_key.is_empty());
    }
}

#[tokio::test]
async fn system_schemas_last_and_flagged() {
    let db = require_testdb!();
    for (major, cat) in catalogs(db).await {
        let names: Vec<&str> = cat.schemas.iter().map(|s| s.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("pg_toast") || n.starts_with("pg_temp")), "pg{major}: {names:?}");
        let first_system = cat.schemas.iter().position(|s| s.system).expect("system schemas");
        assert!(cat.schemas[first_system..].iter().all(|s| s.system), "system schemas come last: {names:?}");
        assert!(cat.schemas[..first_system].iter().all(|s| !s.system));
        let sys: Vec<&str> = cat.schemas[first_system..].iter().map(|s| s.name.as_str()).collect();
        assert_eq!(sys, ["information_schema", "pg_catalog"]);
        let user: Vec<&str> = cat.schemas[..first_system].iter().map(|s| s.name.as_str()).collect();
        assert_eq!(user, ["many", "odd", "public", "scratch", "shop"], "empty user schemas stay listed");
        assert!(cat.table(&tr("pg_catalog", "pg_class")).is_some());
    }
}

#[tokio::test]
async fn partitions_hidden_parent_shown() {
    let db = require_testdb!();
    for (_, cat) in catalogs(db).await {
        assert_eq!(cat.table(&tr("shop", "events")).expect("events").kind, RelKind::PartitionedTable);
        assert!(cat.table(&tr("shop", "events_2025")).is_none());
        assert!(cat.table(&tr("shop", "events_2026")).is_none());
    }
}

#[tokio::test]
async fn domain_enum_and_types() {
    let db = require_testdb!();
    for (major, cat) in catalogs(db).await {
        let t = cat.table(&tr("shop", "all_types")).expect("all_types");
        let doc = t.column("doc").expect("doc");
        assert_eq!((doc.base_type.as_str(), doc.class), ("jsonb", TypeClass::Json), "pg{major}: {doc:?}");
        assert!(doc.type_sql.contains("json_doc"), "{doc:?}");
        let st = t.column("st").expect("st");
        assert!(st.type_sql.contains("order_status"), "{st:?}");
        assert_eq!((st.class, st.base_type.as_str()), (TypeClass::Other, "order_status"));
        assert_eq!(t.column("note").expect("note").class, TypeClass::LongText);
        assert_eq!(t.column("arr").expect("arr").class, TypeClass::Other);
        assert_eq!(t.column("comp").expect("comp").class, TypeClass::Other);
        let email = cat.table(&tr("shop", "customers")).expect("customers").column("email").expect("email");
        assert_eq!(email.type_sql, "character varying(200)");
        assert_eq!((email.base_type.as_str(), email.class), ("varchar", TypeClass::Text));
        assert!(!email.not_null);
        assert!(cat.table(&tr("shop", "customers")).expect("c").column("name").expect("name").not_null);
    }
}

#[tokio::test]
async fn views_listed_without_keys() {
    let db = require_testdb!();
    for (_, cat) in catalogs(db).await {
        let v = cat.table(&tr("shop", "order_totals")).expect("order_totals");
        assert_eq!(v.kind, RelKind::View);
        assert!(v.row_key().is_none() && v.primary_key.is_empty() && v.foreign_keys.is_empty());
        assert_eq!(v.columns.len(), 3);
        let m = cat.table(&tr("shop", "customer_counts")).expect("customer_counts");
        assert_eq!(m.kind, RelKind::MaterializedView);
        assert!(m.row_key().is_none());
    }
}

#[tokio::test]
async fn odd_identifiers_survive() {
    let db = require_testdb!();
    for (_, cat) in catalogs(db).await {
        let t = cat.table(&tr("odd", "Odd \"Name\" 🐝")).expect("odd table");
        let cols: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cols, ["id", "Total $", "lower UPPER", "Other Ödd id"]);
        let fk = t.fk_for("Other Ödd id").expect("fk");
        assert_eq!(fk.target, tr("odd", "Other Ödd"));
        assert_eq!(fk.target_columns, ["id"]);
        assert_eq!(t.primary_key, ["id"]);
    }
}

#[tokio::test]
async fn three_hundred_tables_load_quickly() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = conn(s, db).await;
        let start = Instant::now();
        let cat = load_catalog(&c).await.expect("catalog");
        let took = start.elapsed();
        println!("pg{}: load_catalog took {took:?} ({} tables)", s.major(), cat.table_count());
        let many = cat.schemas.iter().find(|x| x.name == "many").expect("many");
        assert_eq!(many.tables.len(), 300);
        assert_eq!(many.tables[0].table.name, "t_001");
        assert!(took.as_secs_f64() < 2.0, "took {took:?}");
    }
}

#[tokio::test]
async fn big_rows_estimate() {
    let db = require_testdb!();
    for (_, cat) in catalogs(db).await {
        let n = cat.table(&tr("shop", "big_rows")).expect("big_rows").estimated_rows.expect("estimate");
        assert!(n >= 900_000, "{n}");
    }
}

#[tokio::test]
async fn composite_key_order_is_key_order_not_attnum() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = conn(s, db).await;
        let (target, refer) = (unique("keyt"), unique("keyr"));
        let setup = format!(
            "CREATE TABLE scratch.\"{target}\" (a int, b int, c int, PRIMARY KEY (b, a), UNIQUE (c, a)); \
             CREATE TABLE scratch.\"{refer}\" (x int, y int, FOREIGN KEY (y, x) REFERENCES scratch.\"{target}\" (b, a))"
        );
        let created = query::run_untyped(&c, &setup, QUERY_LIMITS).await;
        let loaded = match created {
            Ok(_) => load_catalog(&c).await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let drop = format!("DROP TABLE IF EXISTS scratch.\"{refer}\", scratch.\"{target}\"");
        let dropped = query::run_untyped(&c, &drop, QUERY_LIMITS).await;
        let cat = loaded.expect("catalog with key tables");
        dropped.expect("drop");
        let t = cat.table(&tr("scratch", &target)).expect("target");
        assert_eq!(t.primary_key, ["b", "a"]);
        assert_eq!(t.unique_keys, vec![vec!["c".to_string(), "a".to_string()]]);
        let fk = &cat.table(&tr("scratch", &refer)).expect("referencing").foreign_keys[0];
        assert_eq!(fk.columns, ["y", "x"]);
        assert_eq!(fk.target_columns, ["b", "a"]);
        assert_eq!(fk.target, tr("scratch", &target));
    }
}

#[tokio::test]
async fn columns_postgres_computes_are_marked() {
    let db = require_testdb!();
    let t = dbee_testkit::unique("t_generated");
    for s in postgres(db) {
        let ddl = format!(
            "CREATE TABLE scratch.{t} (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, a int,
                 twice int GENERATED ALWAYS AS (a * 2) STORED, d int GENERATED BY DEFAULT AS IDENTITY)"
        );
        dbee_testkit::psql(s, &ddl).unwrap();
    }
    let loaded = catalogs(db).await;
    for s in postgres(db) {
        let _ = dbee_testkit::psql(s, &format!("DROP TABLE IF EXISTS scratch.{t}"));
    }
    for (major, cat) in loaded {
        let table = cat.table(&tr("scratch", &t)).unwrap_or_else(|| panic!("pg{major}: no scratch.{t}"));
        let generated = |name: &str| table.column(name).map(|c| c.generated);
        assert_eq!(generated("id"), Some(Generated::Identity), "pg{major}");
        assert_eq!(generated("twice"), Some(Generated::Expression), "pg{major}");
        assert_eq!((generated("a"), generated("d")), (Some(Generated::No), Some(Generated::No)), "pg{major}: BY DEFAULT can be written");
        assert!(table.column("d").is_some_and(|c| c.has_default), "pg{major}");
    }
}

#[tokio::test]
async fn pg_catalog_classes() {
    let db = require_testdb!();
    let t = dbee_testkit::unique("t_classes");
    let domain = dbee_testkit::unique("d_text");
    for s in postgres(db) {
        let ddl = format!(
            "CREATE DOMAIN scratch.{domain} AS text;
             CREATE TABLE scratch.{t} (i int4, f float8, n numeric(10,2), m money, b bool, t text, v varchar(20), c char(3), x xml,
                 j json, jb jsonb, d date, ts timestamptz, tm time, iv interval, ba bytea, u uuid, ia int4[], d_t scratch.{domain})"
        );
        dbee_testkit::psql(s, &ddl).unwrap();
    }
    let loaded = catalogs(db).await;
    for s in postgres(db) {
        let _ = dbee_testkit::psql(s, &format!("DROP TABLE IF EXISTS scratch.{t}; DROP DOMAIN IF EXISTS scratch.{domain}"));
    }
    for (major, cat) in loaded {
        let table = cat.table(&tr("scratch", &t)).unwrap_or_else(|| panic!("pg{major}: no scratch.{t}"));
        for (name, class) in [
            ("i", TypeClass::Number),
            ("f", TypeClass::Number),
            ("n", TypeClass::Number),
            ("m", TypeClass::Number),
            ("b", TypeClass::Bool),
            ("t", TypeClass::LongText),
            ("v", TypeClass::Text),
            ("c", TypeClass::Text),
            ("x", TypeClass::LongText),
            ("j", TypeClass::Json),
            ("jb", TypeClass::Json),
            ("d", TypeClass::DateTime),
            ("ts", TypeClass::DateTime),
            ("tm", TypeClass::DateTime),
            ("iv", TypeClass::Other),
            ("ba", TypeClass::Binary),
            ("u", TypeClass::Other),
            ("ia", TypeClass::Other),
            ("d_t", TypeClass::LongText),
        ] {
            assert_eq!(table.column(name).map(|c| c.class), Some(class), "pg{major}: {name}");
        }
    }
}

#[tokio::test]
async fn public_is_the_default_schema() {
    let db = require_testdb!();
    for (major, cat) in catalogs(db).await {
        assert_eq!(cat.default_schema.as_deref(), Some("public"), "pg{major}");
    }
}

#[tokio::test]
async fn no_catalog_row_is_dropped() {
    let db = require_testdb!();
    for s in postgres(db) {
        let c = conn(s, db).await;
        // One snapshot for the three queries: other tests here create tables meanwhile.
        query::run_untyped(&c, "BEGIN ISOLATION LEVEL REPEATABLE READ", QUERY_LIMITS).await.unwrap();
        let rows = catalog_rows(&c).await;
        query::run_untyped(&c, "COMMIT", QUERY_LIMITS).await.unwrap();
        let rows = rows.unwrap_or_else(|e| panic!("pg{}: {e}", s.major()));
        assert_eq!(rows.dropped(), 0, "pg{}", s.major());
    }
}

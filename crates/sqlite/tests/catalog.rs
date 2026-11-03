//! Catalog tests on fresh copies of the SQLite seed (`dbee_testkit::targets_of`) and on files the
//! tests make under `target/`. Every load also checks that no row named a table the catalog did
//! not list.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind};
use dbee_core::{Catalog, Generated, RelKind, TableInfo, TableRef, TypeClass};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_sqlite::{SqliteConnection, connect};
use dbee_testkit::{Kind, TestTarget, repo_root, targets_of, unique};

fn tr(name: &str) -> TableRef {
    TableRef { schema: "main".into(), name: name.into() }
}

async fn open_file(file: &Path, create_file: bool) -> SqliteConnection {
    let c = ConnectionConfig { name: "test sqlite".into(), kind: DbKind::Sqlite, file: Some(file.to_path_buf()), ..ConnectionConfig::default() };
    let t = Target { config: &c, password: None, tunnel: None, purpose: Purpose::Browse, create_file, own_file_only: false };
    connect(t).await.unwrap_or_else(|e| panic!("open {}: {e}", file.display()))
}

/// A fresh copy of the SQLite seed, removed when dropped, and a connection to it.
async fn seeded() -> (TestTarget, SqliteConnection) {
    let seed = targets_of(&[Kind::Sqlite]).pop().expect("the SQLite seed");
    let conn = open_file(seed.file().unwrap(), false).await;
    (seed, conn)
}

/// A new database file under `target/` made by `sql`, removed when dropped.
struct Fresh(PathBuf);

impl Fresh {
    async fn with(sql: &str) -> (Fresh, SqliteConnection) {
        let dir = repo_root().join("target").join("testkit").join("sqlite-driver");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.sqlite", unique("catalog")));
        let conn = open_file(&file, true).await;
        conn.run(sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
        (Fresh(file), conn)
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The catalog of `conn`, whose rows must all name a listed table.
async fn load(conn: &SqliteConnection) -> Catalog {
    let rows = conn.catalog_rows().await.unwrap_or_else(|e| panic!("catalog rows: {e}"));
    assert_eq!(rows.dropped(), 0, "rows named a table the catalog did not list");
    conn.load_catalog().await.unwrap_or_else(|e| panic!("load_catalog: {e}"))
}

fn table<'a>(cat: &'a Catalog, name: &str) -> &'a TableInfo {
    cat.table(&tr(name)).unwrap_or_else(|| panic!("no main.{name}"))
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(name).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn shop_tables_with_keys() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    let orders = table(&cat, "orders");
    assert_eq!((orders.kind, orders.primary_key.clone()), (RelKind::Table, strings(&["id"])));
    let fk = orders.fk_for("customer_id").expect("fk");
    assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("customers"), strings(&["id"])));
    assert_eq!(&names(&orders.columns, |c| &c.name)[..3], ["id", "customer_id", "dealer_id"], "declaration order");
    let total = orders.column("total").expect("total");
    assert_eq!((total.type_sql.as_str(), total.base_type.as_str(), total.class), ("NUMERIC(12,2)", "NUMERIC", TypeClass::Number));
    assert!(!total.has_default && !total.not_null && !total.hidden, "{total:?}");
    assert!(orders.no_edit_reason.is_none() && orders.row_key().is_some());
    assert_eq!(orders.estimated_rows, Some(dbee_testkit::ORDERS_COUNT));
    assert_eq!(table(&cat, "order_items").primary_key, ["order_id", "line_no"]);
    let customers = table(&cat, "customers");
    assert_eq!(customers.unique_keys, vec![strings(&["email"])]);
    assert!(customers.column("name").expect("name").not_null);
    let ship = table(&cat, "shipments");
    assert_eq!(ship.foreign_keys.len(), 1);
    assert_eq!(ship.foreign_keys[0].columns, ["order_id", "line_no"]);
    assert_eq!(ship.foreign_keys[0].target_columns, ["order_id", "line_no"]);
    assert_eq!(ship.foreign_keys[0].target, tr("order_items"));
    // No key of its own: keyed by its rowid.
    let no_key = table(&cat, "no_key");
    assert_eq!((no_key.primary_key.clone(), no_key.unique_keys.len()), (strings(&["rowid"]), 0));
    assert_eq!(table(&cat, "events").kind, RelKind::Table);
    for view in ["order_totals", "customer_counts"] {
        let v = table(&cat, view);
        assert_eq!((v.kind, v.estimated_rows), (RelKind::View, None), "{view}");
        assert!(v.row_key().is_none() && v.primary_key.is_empty() && v.foreign_keys.is_empty(), "{view}");
        assert!(v.no_edit_reason.is_none(), "{view}");
    }
    assert_eq!(table(&cat, "order_totals").columns.len(), 3);
}

#[tokio::test]
async fn one_schema_main_without_internal_tables() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    assert_eq!(cat.schemas.iter().map(|s| (s.name.as_str(), s.system)).collect::<Vec<_>>(), [("main", false)]);
    assert_eq!(cat.default_schema.as_deref(), Some("main"));
    // The seed's 313 tables and 2 views; its ANALYZE made sqlite_stat1, which is SQLite's own.
    assert_eq!(conn.scalar_u64("SELECT count(*) FROM sqlite_schema WHERE name = 'sqlite_stat1'").await, Ok(1));
    assert_eq!(cat.table_count(), 315);
    let all = &cat.schemas[0].tables;
    assert!(all.iter().all(|t| !t.table.name.starts_with("sqlite_")), "no internal table");
    assert_eq!(all.iter().filter(|t| t.kind == RelKind::View).count(), 2);
}

#[tokio::test]
async fn columns_types_defaults_and_generated() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    use TypeClass::*;
    let all_types = table(&cat, "all_types");
    let got: Vec<(&str, &str, &str, TypeClass)> =
        all_types.columns.iter().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_sql.as_str(), c.class)).collect();
    let expected = [
        ("id", "INTEGER", "INTEGER", Number),
        ("note", "TEXT", "TEXT", LongText),
        ("st", "TEXT", "TEXT", LongText),
        ("doc", "JSON", "JSON", Json),
        ("flag", "BOOLEAN", "BOOLEAN", Bool),
        ("i", "INTEGER", "INTEGER", Number),
        ("r", "REAL", "REAL", Number),
        ("m", "NUMERIC", "NUMERIC", Number),
        ("n", "DECIMAL", "DECIMAL(30,10)", Number),
        ("d", "DATE", "DATE", DateTime),
        ("ts", "DATETIME", "DATETIME", DateTime),
        ("b", "BLOB", "BLOB", Binary),
        ("u", "VARCHAR", "VARCHAR(36)", LongText),
        ("anything", "", "", Other),
        ("total", "REAL", "REAL", Number),
    ];
    assert_eq!(got, expected);
    let total = all_types.column("total").unwrap();
    assert_eq!((total.generated, total.has_default), (Generated::Expression, true), "SQLite fills a generated column");
    assert!(all_types.columns.iter().filter(|c| c.name != "total").all(|c| c.generated == Generated::No && !c.hidden));
    // The rowid alias and a default fill a column an insert leaves out; a plain column does not.
    let has_default = |t: &str, c: &str| table(&cat, t).column(c).map(|c| c.has_default);
    assert_eq!(has_default("orders", "id"), Some(true));
    assert_eq!(has_default("customers", "created_at"), Some(true));
    assert_eq!(has_default("customers", "name"), Some(false));
    assert_eq!(has_default("order_items", "order_id"), Some(false), "part of a key of two columns");
    // A stored generated column, and a table whose key is not the rowid.
    let (_file, fresh) = Fresh::with(
        "CREATE TABLE g (a INTEGER, b TEXT GENERATED ALWAYS AS (a || 'x') STORED, c INT PRIMARY KEY, d INTEGER AS (a * 2) VIRTUAL); \
         CREATE TABLE w (k TEXT PRIMARY KEY, v) WITHOUT ROWID",
    )
    .await;
    let cat = load(&fresh).await;
    let g = table(&cat, "g");
    assert_eq!(
        g.columns.iter().map(|c| (c.name.as_str(), c.generated)).collect::<Vec<_>>(),
        [("a", Generated::No), ("b", Generated::Expression), ("c", Generated::No), ("d", Generated::Expression)]
    );
    assert!(g.columns.iter().all(|c| !c.hidden), "a generated column is no hidden one");
    assert_eq!((g.primary_key.clone(), g.column("c").map(|c| c.has_default)), (strings(&["c"]), Some(false)), "INT is no rowid alias");
    let w = table(&cat, "w");
    assert_eq!((w.primary_key.clone(), w.column("k").map(|c| (c.has_default, c.not_null))), (strings(&["k"]), Some((false, true))));
}

#[tokio::test]
async fn classes_as_a_query_gives_them() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    let mut compared = 0;
    for t in &cat.schemas[0].tables {
        let sql = format!("SELECT * FROM {} LIMIT 0", dbee_sql::Dialect::Sqlite.table(&t.table).unwrap());
        let r = conn.run(&sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
        let query: Vec<(&str, &str, &str, TypeClass)> =
            r.columns.iter().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_label.as_str(), c.class)).collect();
        let catalog: Vec<(&str, &str, &str, TypeClass)> =
            t.visible_columns().map(|c| (c.name.as_str(), c.base_type.as_str(), c.type_sql.as_str(), c.class)).collect();
        // A view's column over an expression has no declared type in either.
        assert_eq!(query, catalog, "{}", t.table.name);
        compared += 1;
    }
    assert_eq!(compared, 315);
}

#[tokio::test]
async fn keys_from_constraints_and_foreign_keys_as_sqlite_resolves_them() {
    let (_file, conn) = Fresh::with(
        "CREATE TABLE Parent (k INTEGER PRIMARY KEY, a TEXT NOT NULL, b TEXT NOT NULL, UNIQUE (a, b), UNIQUE (b)); \
         CREATE UNIQUE INDEX parent_a ON Parent (a); \
         CREATE UNIQUE INDEX parent_b_part ON Parent (b) WHERE b > 'm'; \
         CREATE TABLE pk2 (x INTEGER, y TEXT, PRIMARY KEY (y, x)); \
         CREATE TABLE eleven (c1 INT UNIQUE, c2 INT UNIQUE, c3 INT UNIQUE, c4 INT UNIQUE, c5 INT UNIQUE, c6 INT UNIQUE, \
             c7 INT UNIQUE, c8 INT UNIQUE, c9 INT UNIQUE, c10 INT UNIQUE, c11 INT UNIQUE); \
         CREATE TABLE child (id INTEGER PRIMARY KEY, p INTEGER REFERENCES parent, pa TEXT, pb TEXT, x INTEGER, y TEXT, \
             FOREIGN KEY (pb, pa) REFERENCES PARENT (B, A), FOREIGN KEY (y, x) REFERENCES pk2)",
    )
    .await;
    let cat = load(&conn).await;
    let parent = table(&cat, "Parent");
    assert_eq!(parent.primary_key, ["k"]);
    // UNIQUE constraints, in the order written; unique indexes are no constraint.
    assert_eq!(parent.unique_keys, vec![strings(&["a", "b"]), strings(&["b"])]);
    assert_eq!(table(&cat, "pk2").primary_key, ["y", "x"], "the key's order, not the columns'");
    let eleven: Vec<String> = (1..=11).map(|i| format!("c{i}")).collect();
    assert_eq!(table(&cat, "eleven").unique_keys, eleven.into_iter().map(|c| vec![c]).collect::<Vec<_>>(), "in the order written, past ten");
    let child = table(&cat, "child");
    let fks: Vec<(Vec<String>, TableRef, Vec<String>)> =
        child.foreign_keys.iter().map(|f| (f.columns.clone(), f.target.clone(), f.target_columns.clone())).collect();
    // As written (any case, no column list) and as SQLite resolves them: the table's spelling, its key.
    assert_eq!(fks.len(), 3, "{fks:?}");
    assert!(fks.contains(&(strings(&["p"]), tr("Parent"), strings(&["k"]))), "{fks:?}");
    assert!(fks.contains(&(strings(&["pb", "pa"]), tr("Parent"), strings(&["b", "a"]))), "{fks:?}");
    assert!(fks.contains(&(strings(&["y", "x"]), tr("pk2"), strings(&["y", "x"]))), "{fks:?}");
    let mut fk_names: Vec<&str> = child.foreign_keys.iter().map(|f| f.name.as_str()).collect();
    fk_names.sort_unstable();
    fk_names.dedup();
    assert_eq!(fk_names.len(), 3, "one name per key");
}

#[tokio::test]
async fn a_relation_sqlite_cannot_read_is_listed_without_columns() {
    let (_file, conn) = Fresh::with(
        "CREATE TABLE gone (a INTEGER); CREATE VIEW broken AS SELECT a FROM gone; DROP TABLE gone; \
         CREATE TABLE kept (a INTEGER PRIMARY KEY); CREATE VIRTUAL TABLE docs USING fts5(title, body)",
    )
    .await;
    let cat = load(&conn).await;
    let broken = table(&cat, "broken");
    assert_eq!((broken.kind, broken.columns.len()), (RelKind::View, 0));
    assert_eq!(table(&cat, "kept").primary_key, ["a"]);
    // A virtual table's hidden columns are marked; its shadow tables are ordinary tables.
    let docs = table(&cat, "docs");
    assert_eq!(
        docs.columns.iter().map(|c| (c.name.as_str(), c.hidden)).collect::<Vec<_>>(),
        [("title", false), ("body", false), ("docs", true), ("rank", true)]
    );
    assert!(cat.table(&tr("docs_content")).is_some());
}

#[tokio::test]
async fn row_estimates_come_from_sqlite_stat1() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    assert_eq!(table(&cat, "big_rows").estimated_rows, Some(dbee_testkit::BIG_ROWS));
    assert_eq!(table(&cat, "order_items").estimated_rows, Some(10_001));
    assert_eq!(table(&cat, "no_key").estimated_rows, Some(4));
    // Without ANALYZE there is no estimate.
    let (_file, fresh) = Fresh::with("CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1), (2)").await;
    assert_eq!(table(&load(&fresh).await, "t").estimated_rows, None);
}

#[tokio::test]
async fn odd_identifiers_survive() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    let odd = table(&cat, "Odd \"Name\" 🐝");
    assert_eq!(names(&odd.columns, |c| &c.name), ["id", "Total $", "lower UPPER", "Other Ödd id"]);
    let fk = odd.fk_for("Other Ödd id").expect("fk");
    assert_eq!((fk.target.clone(), fk.target_columns.clone()), (tr("Other Ödd"), strings(&["id"])));
    assert_eq!(table(&cat, "Other Ödd").primary_key, ["id"]);
    // A real column named rowid: the key takes the next name.
    let taken = table(&cat, "rowid_taken");
    assert_eq!(taken.columns.iter().map(|c| (c.name.as_str(), c.hidden)).collect::<Vec<_>>(), [("_rowid_", true), ("rowid", false), ("note", false)]);
    assert_eq!(taken.primary_key, ["_rowid_"]);
}

#[tokio::test]
async fn three_hundred_tables_load_quickly() {
    let (_seed, conn) = seeded().await;
    load(&conn).await;
    let started = Instant::now();
    let cat = conn.load_catalog().await.unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!((1..=300).all(|i| cat.table(&tr(&format!("t_{i:03}"))).is_some_and(|t| t.primary_key == ["id"])));
    println!("catalog of {} relations in {took:?}", cat.table_count());
}

#[tokio::test]
async fn names_with_quotes_and_brackets_survive() {
    let (_file, conn) = Fresh::with(
        "CREATE TABLE \"it's\" (id INTEGER PRIMARY KEY, \"a]b\" TEXT NOT NULL UNIQUE, \"q'r\" INTEGER REFERENCES \"it's\"); \
         CREATE TABLE \"brack]et [x\" (k TEXT PRIMARY KEY, \"it's id\" INTEGER REFERENCES \"IT'S\" (ID)) WITHOUT ROWID",
    )
    .await;
    let cat = load(&conn).await;
    let quoted = table(&cat, "it's");
    assert_eq!(names(&quoted.columns, |c| &c.name), ["id", "a]b", "q'r"]);
    assert_eq!((quoted.primary_key.clone(), quoted.unique_keys.clone()), (strings(&["id"]), vec![strings(&["a]b"])]));
    assert_eq!(quoted.fk_for("q'r").map(|f| (f.target.clone(), f.target_columns.clone())), Some((tr("it's"), strings(&["id"]))));
    let bracket = table(&cat, "brack]et [x");
    assert_eq!(bracket.primary_key, ["k"]);
    assert_eq!(bracket.fk_for("it's id").map(|f| (f.target.clone(), f.target_columns.clone())), Some((tr("it's"), strings(&["id"]))));
}

#[tokio::test]
async fn a_table_without_a_key_is_keyed_by_a_hidden_rowid() {
    let (_seed, conn) = seeded().await;
    let cat = load(&conn).await;
    let notes = table(&cat, "notes_nokey");
    let rowid = &notes.columns[0];
    assert_eq!(
        (rowid.name.as_str(), rowid.class, rowid.hidden, rowid.not_null, rowid.has_default, rowid.generated),
        ("rowid", TypeClass::Number, true, true, true, Generated::No)
    );
    assert_eq!(names(&notes.columns, |c| &c.name), ["rowid", "title", "body"]);
    assert_eq!(notes.visible_columns().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["title", "body"]);
    assert_eq!((notes.primary_key.clone(), notes.no_edit_reason.clone()), (strings(&["rowid"]), None));
    assert_eq!(notes.row_key().map(|k| k.len()), Some(1));
}

#[tokio::test]
async fn which_tables_get_the_rowid_and_why_the_others_take_no_edits() {
    let (_file, conn) = Fresh::with(
        "CREATE TABLE plain (a TEXT); \
         CREATE TABLE cased (\"ROWID\" TEXT, \"_Rowid_\" TEXT); \
         CREATE TABLE every_name (RowId TEXT, \"_ROWID_\" TEXT, Oid TEXT); \
         CREATE TABLE nullable_unique (a TEXT UNIQUE, b TEXT); \
         CREATE TABLE strict_unique (a TEXT NOT NULL UNIQUE, b TEXT); \
         CREATE TABLE int_key (id INTEGER PRIMARY KEY, b TEXT); \
         CREATE TABLE text_key (k TEXT PRIMARY KEY, b TEXT); \
         CREATE TABLE no_rowid (k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID; \
         CREATE TABLE generated (a INTEGER, rowid INTEGER AS (a * 2)); \
         CREATE VIEW plain_view AS SELECT a FROM plain; \
         CREATE VIRTUAL TABLE docs USING fts5(title, body)",
    )
    .await;
    let cat = load(&conn).await;
    let shape = |name: &str| {
        let t = table(&cat, name);
        let columns: Vec<String> = t.columns.iter().map(|c| if c.hidden { format!("({})", c.name) } else { c.name.clone() }).collect();
        (columns, t.primary_key.clone(), t.row_key().map(|k| k.iter().map(|c| c.name.clone()).collect::<Vec<_>>()))
    };
    let s = |v: &[&str]| strings(v);
    assert_eq!(shape("plain"), (s(&["(rowid)", "a"]), s(&["rowid"]), Some(s(&["rowid"]))));
    // The names are compared ignoring case, as SQLite reads them.
    assert_eq!(shape("cased"), (s(&["(oid)", "ROWID", "_Rowid_"]), s(&["oid"]), Some(s(&["oid"]))));
    // A generated column takes its name too.
    assert_eq!(shape("generated"), (s(&["(_rowid_)", "a", "rowid"]), s(&["_rowid_"]), Some(s(&["_rowid_"]))));
    // A unique key whose columns may be NULL is no key.
    assert_eq!(shape("nullable_unique"), (s(&["(rowid)", "a", "b"]), s(&["rowid"]), Some(s(&["rowid"]))));
    // A table with a key of its own keeps it.
    assert_eq!(shape("strict_unique"), (s(&["a", "b"]), s(&[]), Some(s(&["a"]))));
    assert_eq!(shape("int_key"), (s(&["id", "b"]), s(&["id"]), Some(s(&["id"]))));
    assert_eq!(shape("text_key"), (s(&["k", "b"]), s(&["k"]), Some(s(&["k"]))));
    assert_eq!(shape("no_rowid"), (s(&["k", "v"]), s(&["k"]), Some(s(&["k"]))));
    for name in ["plain", "cased", "generated", "nullable_unique", "strict_unique", "int_key", "text_key", "no_rowid"] {
        assert_eq!(table(&cat, name).no_edit_reason, None, "{name}");
    }
    // Columns that take every name of the row id: no key, and the reason in plain words.
    assert_eq!(shape("every_name"), (s(&["RowId", "_ROWID_", "Oid"]), s(&[]), None));
    let why = table(&cat, "every_name").no_edit_reason.clone().unwrap_or_default();
    assert!(why.starts_with("This table has no key, and its columns use every name SQLite gives the row id (rowid, _rowid_, oid)"), "{why}");
    // A view is read-only as on every engine; a virtual table says why it is.
    assert_eq!(shape("plain_view"), (s(&["a"]), s(&[]), None));
    assert_eq!(table(&cat, "plain_view").no_edit_reason, None);
    assert_eq!(shape("docs"), (s(&["title", "body", "(docs)", "(rank)"]), s(&[]), None));
    let why = table(&cat, "docs").no_edit_reason.clone().unwrap_or_default();
    assert!(why.starts_with("This is a virtual table") && why.contains("query tab"), "{why}");
}

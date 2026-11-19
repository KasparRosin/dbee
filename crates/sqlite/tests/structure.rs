//! A table's indexes and triggers (`table_structure`), on database files the tests make under
//! `target/`.

use std::path::PathBuf;

use dbee_config::{ConnectionConfig, DbKind};
use dbee_core::{IndexInfo, TableRef, TableStructure, TriggerEvent, TriggerTiming};
use dbee_driver::structure::MAX_DEFINITION;
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_sqlite::{SqliteConnection, connect};
use dbee_testkit::{repo_root, unique};

fn tr(name: &str) -> TableRef {
    TableRef { schema: "main".into(), name: name.into() }
}

/// A new database file made by `sql`, removed when dropped.
struct Fresh(PathBuf, SqliteConnection);

impl Fresh {
    async fn with(sql: &str) -> Fresh {
        let dir = repo_root().join("target").join("testkit").join("sqlite-driver");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.sqlite", unique("structure")));
        let c = ConnectionConfig { name: "test sqlite".into(), kind: DbKind::Sqlite, file: Some(file.clone()), ..ConnectionConfig::default() };
        let t = Target { config: &c, password: None, tunnel: None, purpose: Purpose::Browse, create_file: true, allowed_dirs: None };
        let conn = connect(t).await.unwrap();
        conn.run(sql, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
        Fresh(file, conn)
    }

    async fn structure(&self, table: &str) -> TableStructure {
        self.1.table_structure(&tr(table)).await.unwrap_or_else(|e| panic!("structure of {table}: {e}"))
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn index<'a>(s: &'a TableStructure, name: &str) -> &'a IndexInfo {
    s.indexes.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no index {name} in {s:#?}"))
}

const SHOP: &str = r#"
CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT UNIQUE, name TEXT, age INT);
CREATE TABLE pairs (a TEXT, b TEXT, PRIMARY KEY (a, b)) WITHOUT ROWID;
CREATE INDEX users_lower ON users (lower(email) DESC, name);
CREATE INDEX "odd ""name""" ON users (age) WHERE age > 18;
CREATE TRIGGER users_ai AFTER INSERT ON users BEGIN SELECT 1; END;
CREATE TRIGGER "Users_bu" BEFORE UPDATE OF name ON USERS BEGIN SELECT 1; END;
CREATE VIEW grown AS SELECT * FROM users WHERE age > 18;
CREATE TRIGGER grown_ins INSTEAD OF INSERT ON grown BEGIN SELECT 1; END;
"#;

#[tokio::test]
async fn indexes_with_expressions_conditions_and_order() {
    let db = Fresh::with(SHOP).await;
    let s = db.structure("users").await;
    let names: Vec<&str> = s.indexes.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["odd \"name\"", "sqlite_autoindex_users_1", "users_lower"], "by name; the rowid key has no index");
    let email = index(&s, "sqlite_autoindex_users_1");
    assert!(email.unique && !email.primary && email.predicate.is_none() && email.method.is_none());
    assert_eq!(email.parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("email")]);
    let lower = index(&s, "users_lower");
    assert!(!lower.unique);
    assert_eq!(lower.parts.len(), 2);
    assert_eq!(
        (lower.parts[0].column.as_deref(), lower.parts[0].expression.as_deref(), lower.parts[0].descending),
        (None, Some("lower(email)"), true)
    );
    assert_eq!((lower.parts[1].column.as_deref(), lower.parts[1].expression.as_deref(), lower.parts[1].descending), (Some("name"), None, false));
    assert_eq!(index(&s, "odd \"name\"").predicate.as_deref(), Some("age > 18"));

    let pairs = db.structure("pairs").await;
    assert_eq!(pairs.indexes.len(), 1);
    let pk = &pairs.indexes[0];
    assert!(pk.primary && pk.unique);
    assert_eq!(pk.parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("a"), Some("b")]);
}

#[tokio::test]
async fn triggers_with_timing_and_event() {
    let db = Fresh::with(SHOP).await;
    let s = db.structure("users").await;
    let got: Vec<_> = s.triggers.iter().map(|t| (t.name.as_str(), t.timing, t.events.clone(), t.for_each_row)).collect();
    assert_eq!(
        got,
        [
            ("Users_bu", Some(TriggerTiming::Before), vec![TriggerEvent::Update], Some(true)),
            ("users_ai", Some(TriggerTiming::After), vec![TriggerEvent::Insert], Some(true)),
        ],
        "a trigger whose ON names the table in another case is found"
    );
    assert!(s.triggers[1].definition.starts_with("CREATE TRIGGER users_ai AFTER INSERT ON users"));
    let view = db.structure("grown").await;
    assert!(view.indexes.is_empty());
    assert_eq!(view.triggers.iter().map(|t| t.timing).collect::<Vec<_>>(), [Some(TriggerTiming::InsteadOf)]);
}

#[tokio::test]
async fn a_missing_table_or_other_schema_has_nothing() {
    let db = Fresh::with(SHOP).await;
    assert_eq!(db.structure("nobody").await, TableStructure::default());
    let other = db.1.table_structure(&TableRef { schema: "temp".into(), name: "users".into() }).await.unwrap();
    assert_eq!(other, TableStructure::default());
}

#[tokio::test]
async fn odd_names_survive() {
    let sql = "CREATE TABLE \"we'ird \"\"t\"\"\" (\"a'b\" TEXT, \"😀\" INT);\
               CREATE UNIQUE INDEX \"i'x 😀\" ON \"we'ird \"\"t\"\"\" (\"a'b\", \"😀\");\
               CREATE TRIGGER \"tr'g\" AFTER DELETE ON \"we'ird \"\"t\"\"\" BEGIN SELECT 1; END;";
    let db = Fresh::with(sql).await;
    let s = db.structure("we'ird \"t\"").await;
    assert_eq!(s.indexes.len(), 1);
    assert_eq!(s.indexes[0].name, "i'x 😀");
    assert_eq!(s.indexes[0].parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("a'b"), Some("😀")]);
    assert_eq!(s.triggers.iter().map(|t| (t.name.as_str(), t.events.clone())).collect::<Vec<_>>(), [("tr'g", vec![TriggerEvent::Delete])]);
}

/// A trigger whose statement is far longer than dbee keeps is cut, not refused.
#[tokio::test]
async fn a_huge_trigger_is_cut() {
    let body = "x".repeat(MAX_DEFINITION * 2);
    let db = Fresh::with(&format!("CREATE TABLE t (a); CREATE TRIGGER big AFTER INSERT ON t BEGIN SELECT '{body}'; END;")).await;
    let s = db.structure("t").await;
    let d = &s.triggers[0].definition;
    assert!(d.len() <= MAX_DEFINITION + 3 && d.ends_with('…'), "{}", d.len());
    assert_eq!(s.triggers[0].timing, Some(TriggerTiming::After));
}

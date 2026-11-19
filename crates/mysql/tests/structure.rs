//! A table's indexes and triggers (`table_structure`) against every MySQL and MariaDB test
//! server. Each test makes its own database and drops it.

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_core::{IndexInfo, TableRef, TableStructure, TriggerEvent, TriggerTiming};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_mysql::{MySqlConnection, connect};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

async fn open(s: &Server, db: &TestDb) -> MySqlConnection {
    let c = ConnectionConfig {
        name: format!("test {}", s.name),
        kind: if s.kind == Kind::MariaDb { DbKind::MariaDb } else { DbKind::MySql },
        host: "127.0.0.1".into(),
        port: s.port,
        user: "root".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    let t = Target { config: &c, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
    connect(t).await.unwrap_or_else(|e| panic!("connect to {} failed: {e}", s.name))
}

/// MySQL 8.0.13 or later: functional key parts and descending indexes.
fn modern_mysql(conn: &MySqlConnection) -> bool {
    let info = conn.info();
    info.product == "MySQL" && info.version_num >= 80013
}

/// Runs each statement of `setup` (with `{d}` for a new database's name) on every MySQL and
/// MariaDB server, then `check` on the structure of each table `tables` names there; the database
/// is dropped whatever happens.
async fn on_each(setup: impl Fn(&MySqlConnection) -> Vec<String>, tables: &[&str], check: impl Fn(&Server, &MySqlConnection, &[TableStructure])) {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::MySql, Kind::MariaDb]) {
        let conn = open(s, db).await;
        let d = unique("st");
        let mut created = conn.run(&format!("CREATE DATABASE `{d}`"), QUERY_LIMITS).await.map(|_| ()).map_err(|e| e.to_string());
        for sql in setup(&conn) {
            if created.is_ok() {
                let sql = sql.replace("{d}", &format!("`{d}`"));
                created = conn.run(&sql, QUERY_LIMITS).await.map(|_| ()).map_err(|e| format!("{sql}: {e}"));
            }
        }
        let mut read = Vec::new();
        if created.is_ok() {
            for t in tables {
                read.push(conn.table_structure(&TableRef { schema: d.clone(), name: (*t).into() }).await.map_err(|e| e.to_string()));
            }
        }
        conn.run(&format!("DROP DATABASE `{d}`"), QUERY_LIMITS).await.expect("drop");
        created.unwrap_or_else(|e| panic!("{}: setup: {e}", s.name));
        let read: Vec<TableStructure> = read.into_iter().map(|r| r.unwrap_or_else(|e| panic!("{}: {e}", s.name))).collect();
        check(s, &conn, &read);
    }
}

fn index<'a>(s: &'a TableStructure, name: &str) -> &'a IndexInfo {
    s.indexes.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no index {name} in {s:#?}"))
}

fn shop(conn: &MySqlConnection) -> Vec<String> {
    let mut sql = vec![
        "CREATE TABLE {d}.users (id INT PRIMARY KEY, email VARCHAR(100) UNIQUE, name VARCHAR(100), age INT, bio TEXT, \
         KEY users_name_age (name, age), FULLTEXT KEY users_bio (bio)) ENGINE=InnoDB"
            .to_string(),
        "CREATE TRIGGER {d}.users_bi BEFORE INSERT ON {d}.users FOR EACH ROW SET NEW.age = COALESCE(NEW.age, 0)".into(),
        "CREATE TRIGGER {d}.users_au AFTER UPDATE ON {d}.users FOR EACH ROW BEGIN END".into(),
    ];
    if modern_mysql(conn) {
        sql.push("CREATE INDEX users_lower ON {d}.users ((lower(email)) DESC, name)".into());
    }
    sql
}

#[tokio::test]
async fn indexes_with_kinds_and_parts() {
    on_each(shop, &["users"], |s, conn, read| {
        let st = &read[0];
        let pk = index(st, "PRIMARY");
        assert!(st.indexes[0].primary && pk.unique, "{}", s.name);
        assert_eq!(pk.parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("id")]);
        assert_eq!(pk.method.as_deref(), Some("BTREE"));
        assert!(index(st, "email").unique);
        let name_age = index(st, "users_name_age");
        assert!(!name_age.unique && name_age.predicate.is_none());
        assert_eq!(name_age.parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("name"), Some("age")]);
        assert_eq!(index(st, "users_bio").method.as_deref(), Some("FULLTEXT"));
        if modern_mysql(conn) {
            let lower = index(st, "users_lower");
            let first = &lower.parts[0];
            assert_eq!(first.column, None, "{}", s.name);
            assert!(first.expression.as_deref().is_some_and(|e| e.contains("lower") && e.contains("email")), "{}: {first:?}", s.name);
            assert!(first.descending, "{}", s.name);
            assert_eq!(lower.parts[1].column.as_deref(), Some("name"));
        }
    })
    .await;
}

#[tokio::test]
async fn triggers_with_timing_and_action() {
    on_each(shop, &["users", "nobody"], |s, _, read| {
        let got: Vec<_> = read[0].triggers.iter().map(|t| (t.name.as_str(), t.timing, t.events.clone(), t.for_each_row)).collect();
        assert_eq!(
            got,
            [
                ("users_au", Some(TriggerTiming::After), vec![TriggerEvent::Update], Some(true)),
                ("users_bi", Some(TriggerTiming::Before), vec![TriggerEvent::Insert], Some(true)),
            ],
            "{}",
            s.name
        );
        assert_eq!(read[0].triggers[1].definition, "SET NEW.age = COALESCE(NEW.age, 0)", "{}", s.name);
        assert_eq!(read[1], TableStructure::default());
    })
    .await;
}

/// MySQL keeps names in utf8mb3, so no character past three bytes.
#[tokio::test]
async fn odd_names_survive() {
    let setup = |_: &MySqlConnection| {
        vec![
            "CREATE TABLE {d}.`we'ird \\ \"t\"` (`a'b` INT, `€é` VARCHAR(10), UNIQUE KEY `i'x €é` (`a'b`, `€é`))".to_string(),
            "CREATE TRIGGER {d}.`tr'g` AFTER DELETE ON {d}.`we'ird \\ \"t\"` FOR EACH ROW BEGIN END".into(),
        ]
    };
    on_each(setup, &["we'ird \\ \"t\""], |s, _, read| {
        assert_eq!(read[0].indexes.len(), 1, "{}", s.name);
        assert_eq!(read[0].indexes[0].name, "i'x €é");
        assert_eq!(read[0].indexes[0].parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("a'b"), Some("€é")]);
        assert_eq!(read[0].triggers.iter().map(|t| (t.name.as_str(), t.events.clone())).collect::<Vec<_>>(), [("tr'g", vec![TriggerEvent::Delete])]);
    })
    .await;
}

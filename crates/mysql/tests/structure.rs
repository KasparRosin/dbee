//! A table's indexes, triggers, column defaults and foreign key actions (`table_structure`) against every MySQL and MariaDB test
//! server. Each test makes its own database and drops it.

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_core::{FkAction, IndexInfo, TableRef, TableStructure, TriggerEvent, TriggerTiming};
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

/// Defaults as the structure view writes them on MySQL and MariaDB (`auto_increment`,
/// `generated always as (…)`, a string quoted as a SQL literal, an expression or a number as the
/// server gives it); each foreign key's actions by its columns.
#[tokio::test]
async fn mysql_defaults_and_actions() {
    let setup = |conn: &MySqlConnection| {
        let mut sql = vec![
            "CREATE TABLE {d}.p (id BIGINT AUTO_INCREMENT PRIMARY KEY, status VARCHAR(10) DEFAULT 'pending', blank VARCHAR(3) DEFAULT '', \
             at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, n INT DEFAULT 5, g INT AS (n * 2), q VARCHAR(5) DEFAULT 'it''s') ENGINE=InnoDB"
                .to_string(),
            "CREATE TABLE {d}.c (id INT, p_id BIGINT, FOREIGN KEY (p_id) REFERENCES {d}.p (id) ON DELETE CASCADE ON UPDATE SET NULL) ENGINE=InnoDB"
                .into(),
            "CREATE TABLE {d}.plain (id INT, p_id BIGINT, FOREIGN KEY (p_id) REFERENCES {d}.p (id)) ENGINE=InnoDB".into(),
        ];
        if modern_mysql(conn) || conn.info().product != "MySQL" {
            sql.push("ALTER TABLE {d}.p ADD COLUMN x INT DEFAULT (n + 1)".into());
        }
        sql
    };
    on_each(setup, &["p", "c", "plain"], |s, conn, read| {
        let default = |column: &str| {
            let d =
                read[0].defaults.iter().find(|d| d.column == column).unwrap_or_else(|| panic!("{}: no {column} in {:?}", s.name, read[0].defaults));
            d.default.clone().unwrap_or_default()
        };
        assert_eq!(default("id"), "auto_increment", "{}", s.name);
        assert_eq!(default("status"), "'pending'", "{}", s.name);
        assert_eq!(default("blank"), "''", "{}", s.name);
        assert!(default("at").to_ascii_uppercase().starts_with("CURRENT_TIMESTAMP"), "{}: {}", s.name, default("at"));
        assert_eq!(default("n"), "5", "{}", s.name);
        assert!(default("g").starts_with("generated always as ("), "{}: {}", s.name, default("g"));
        assert_eq!(default("q"), "'it''s'", "{}", s.name);
        if modern_mysql(conn) || conn.info().product != "MySQL" {
            assert_eq!(default("x"), "(`n` + 1)", "{}: the expression as the server writes it", s.name);
        }
        let none: Vec<_> = read[1].defaults.iter().filter(|d| d.default.is_some()).collect();
        assert!(none.is_empty(), "{}: no defaults in c, MariaDB's `NULL` read as none: {none:?}", s.name);
        let actions = |i: usize| -> Vec<(Vec<&str>, &str, Option<FkAction>, Option<FkAction>)> {
            read[i]
                .actions
                .iter()
                .map(|a| (a.columns.iter().map(String::as_str).collect(), a.target.name.as_str(), a.on_delete, a.on_update))
                .collect()
        };
        assert_eq!(actions(1), [(vec!["p_id"], "p", Some(FkAction::Cascade), Some(FkAction::SetNull))], "{}", s.name);
        assert!(read[1].actions.iter().all(|a| a.target.schema.starts_with("st_")), "{}", s.name);
        // A key that names no action: MySQL 5.7 and MariaDB say RESTRICT (their default, which acts
        // as NO ACTION does in InnoDB); MySQL 8 says NO ACTION. dbee gives what the server says.
        let plain = if modern_mysql(conn) { FkAction::NoAction } else { FkAction::Restrict };
        assert_eq!(actions(2), [(vec!["p_id"], "p", Some(plain), Some(plain))], "{}", s.name);
    })
    .await;
}

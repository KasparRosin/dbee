//! A table's indexes, triggers, column defaults and foreign key actions (`table_structure`) against every Postgres test server. Each
//! test makes its own schema and drops it.

use dbee_config::{ConnectionConfig, SslMode};
use dbee_core::{FkAction, IndexInfo, TableRef, TableStructure, TriggerEvent, TriggerTiming};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_pg::structure::table_structure;
use dbee_pg::{PgConnection, connect, query};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

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
    let target =
        Target { config: &config, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
    connect(target).await.unwrap_or_else(|e| panic!("connect to {} failed: {e}", s.name))
}

/// Runs `setup` (with `{s}` for a new schema's quoted name) on every Postgres server, then
/// `check` on the structure of each table `tables` names in that schema; the schema is dropped
/// whatever happens.
async fn on_each(setup: &str, tables: &[&str], check: impl Fn(&Server, &[TableStructure])) {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let c = conn(s, db).await;
        let schema = unique("st");
        let sql = format!("CREATE SCHEMA \"{schema}\"; {}", setup.replace("{s}", &format!("\"{schema}\"")));
        let created = query::run_untyped(&c, &sql, QUERY_LIMITS).await.map_err(|e| e.to_string());
        let mut read = Vec::new();
        if created.is_ok() {
            for t in tables {
                read.push(table_structure(&c, &TableRef { schema: schema.clone(), name: (*t).into() }).await.map_err(|e| e.to_string()));
            }
        }
        query::run_untyped(&c, &format!("DROP SCHEMA \"{schema}\" CASCADE"), QUERY_LIMITS).await.expect("drop");
        created.unwrap_or_else(|e| panic!("{}: setup: {e}", s.name));
        let read: Vec<TableStructure> = read.into_iter().map(|r| r.unwrap_or_else(|e| panic!("{}: {e}", s.name))).collect();
        check(s, &read);
    }
}

fn index<'a>(s: &'a TableStructure, name: &str) -> &'a IndexInfo {
    s.indexes.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no index {name} in {s:#?}"))
}

const SHOP: &str = "
CREATE TABLE {s}.users (id int PRIMARY KEY, email text UNIQUE, name text, age int, tags text[], note text);
CREATE INDEX users_lower ON {s}.users (lower(email) DESC, name);
CREATE INDEX users_adult ON {s}.users (age) INCLUDE (name) WHERE age > 18;
CREATE INDEX users_tags ON {s}.users USING gin (tags);
CREATE FUNCTION {s}.noop() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$;
CREATE TRIGGER users_biu BEFORE INSERT OR UPDATE ON {s}.users FOR EACH ROW EXECUTE FUNCTION {s}.noop();
CREATE TRIGGER users_trunc AFTER TRUNCATE ON {s}.users FOR EACH STATEMENT EXECUTE FUNCTION {s}.noop();
CREATE TABLE {s}.orders (id int PRIMARY KEY, user_id int REFERENCES {s}.users (id));
CREATE VIEW {s}.adults AS SELECT * FROM {s}.users WHERE age > 18;
CREATE TRIGGER adults_ins INSTEAD OF INSERT ON {s}.adults FOR EACH ROW EXECUTE FUNCTION {s}.noop();
";

#[tokio::test]
async fn indexes_with_methods_expressions_and_conditions() {
    on_each(SHOP, &["users"], |s, read| {
        let st = &read[0];
        let names: Vec<&str> = st.indexes.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["users_pkey", "users_adult", "users_email_key", "users_lower", "users_tags"], "{}", s.name);
        let pk = index(st, "users_pkey");
        assert!(pk.primary && pk.unique && pk.method.as_deref() == Some("btree"));
        assert_eq!(pk.parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("id")]);
        let lower = index(st, "users_lower");
        assert_eq!(
            (lower.parts[0].column.as_deref(), lower.parts[0].expression.as_deref(), lower.parts[0].descending),
            (None, Some("lower(email)"), true)
        );
        assert_eq!((lower.parts[1].column.as_deref(), lower.parts[1].descending), (Some("name"), false));
        let adult = index(st, "users_adult");
        assert_eq!(adult.parts.len(), 1, "INCLUDE columns are not key parts");
        assert_eq!(adult.predicate.as_deref(), Some("age > 18"));
        assert_eq!(index(st, "users_tags").method.as_deref(), Some("gin"));
        assert!(index(st, "users_email_key").unique);
    })
    .await;
}

#[tokio::test]
async fn triggers_with_timing_events_and_level() {
    on_each(SHOP, &["users", "orders", "adults"], |s, read| {
        let got: Vec<_> = read[0].triggers.iter().map(|t| (t.name.as_str(), t.timing, t.events.clone(), t.for_each_row)).collect();
        assert_eq!(
            got,
            [
                ("users_biu", Some(TriggerTiming::Before), vec![TriggerEvent::Insert, TriggerEvent::Update], Some(true)),
                ("users_trunc", Some(TriggerTiming::After), vec![TriggerEvent::Truncate], Some(false)),
            ],
            "{}: the foreign key's own triggers are not listed",
            s.name
        );
        assert!(
            read[0].triggers[0].definition.starts_with("CREATE TRIGGER users_biu BEFORE INSERT OR UPDATE ON"),
            "{}",
            read[0].triggers[0].definition
        );
        assert!(read[1].triggers.is_empty(), "{}: the referencing table's foreign key makes no listed trigger", s.name);
        assert!(read[2].indexes.is_empty());
        assert_eq!(read[2].triggers.iter().map(|t| t.timing).collect::<Vec<_>>(), [Some(TriggerTiming::InsteadOf)]);
    })
    .await;
}

#[tokio::test]
async fn odd_names_and_missing_tables() {
    let setup = "CREATE TABLE {s}.\"we'ird \\ \"\"t\"\"\" (\"a'b\" int, \"😀\" text);
                 CREATE UNIQUE INDEX \"i'x 😀\" ON {s}.\"we'ird \\ \"\"t\"\"\" (\"a'b\", \"😀\");";
    on_each(setup, &["we'ird \\ \"t\"", "nobody"], |s, read| {
        assert_eq!(read[0].indexes.len(), 1, "{}", s.name);
        assert_eq!(read[0].indexes[0].name, "i'x 😀");
        assert_eq!(read[0].indexes[0].parts.iter().map(|p| p.column.as_deref()).collect::<Vec<_>>(), [Some("a'b"), Some("😀")]);
        assert_eq!(read[1], TableStructure::default());
    })
    .await;
}

/// Defaults as Postgres writes them (`pg_get_expr`), `identity` for an identity column and
/// `generated always as (…)` for a generated one; each foreign key's actions by its columns.
#[tokio::test]
async fn pg_defaults_and_actions() {
    let setup = "
CREATE TABLE {s}.p (id int8 GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, status text DEFAULT 'pending', at timestamptz DEFAULT now(),
                    n int, g int GENERATED ALWAYS AS (n * 2) STORED, \"Odd\" text DEFAULT 'A''b');
CREATE TABLE {s}.k (x int, y int, UNIQUE (x, y));
CREATE TABLE {s}.c (id serial, p_id int8 REFERENCES {s}.p ON DELETE CASCADE, a int, b int,
                    FOREIGN KEY (a, b) REFERENCES {s}.k (x, y) ON UPDATE RESTRICT);";
    on_each(setup, &["p", "c"], |s, read| {
        let defaults: Vec<(&str, Option<&str>)> = read[0].defaults.iter().map(|d| (d.column.as_str(), d.default.as_deref())).collect();
        assert_eq!(
            defaults,
            [
                ("id", Some("identity")),
                ("status", Some("'pending'::text")),
                ("at", Some("now()")),
                ("n", None),
                ("g", Some("generated always as ((n * 2))")),
                ("Odd", Some("'A''b'::text")),
            ],
            "{}",
            s.name
        );
        assert!(read[0].actions.is_empty());
        let c_id = read[1].defaults.first().and_then(|d| d.default.as_deref()).unwrap_or_default();
        assert!(c_id.starts_with("nextval(") && c_id.contains("c_id_seq"), "{}: {c_id}", s.name);
        let mut actions: Vec<(Vec<&str>, &str, Option<FkAction>, Option<FkAction>)> = read[1]
            .actions
            .iter()
            .map(|a| (a.columns.iter().map(String::as_str).collect(), a.target.name.as_str(), a.on_delete, a.on_update))
            .collect();
        actions.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            actions,
            [
                (vec!["a", "b"], "k", Some(FkAction::NoAction), Some(FkAction::Restrict)),
                (vec!["p_id"], "p", Some(FkAction::Cascade), Some(FkAction::NoAction)),
            ],
            "{}",
            s.name
        );
        assert!(read[1].actions.iter().all(|a| a.target.schema.starts_with("st_")), "{}: {:?}", s.name, read[1].actions);
    })
    .await;
}

//! dbee's own Postgres SQL never runs a function or operator someone made in another schema: the
//! catalog reads run with `search_path` set to `pg_catalog, pg_temp` and name `pg_catalog`'s
//! functions, and a search path that puts another schema before `pg_catalog` gets a warning.
//!
//! Each test makes its own database on every Postgres test server, plants such objects in its
//! `public` schema, and drops the database at the end; nothing else is written.

use dbee_config::{ConnectionConfig, SslMode};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_pg::catalog::{list_connection, list_databases, load_catalog};
use dbee_pg::{PgConnection, connect, query};
use dbee_testkit::{Kind, Server, TestDb, require_testdb, unique};

fn config(s: &Server, database: &str) -> ConnectionConfig {
    ConnectionConfig {
        name: format!("test {}", s.name),
        host: "127.0.0.1".into(),
        port: s.port,
        user: "postgres".into(),
        database: database.into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

async fn conn(s: &Server, db: &TestDb, database: &str) -> PgConnection {
    let config = config(s, database);
    let target =
        Target { config: &config, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
    connect(target).await.unwrap_or_else(|e| panic!("{}: connect to {database}: {e}", s.name))
}

async fn sql(c: &PgConnection, text: &str) -> Vec<Vec<Option<String>>> {
    query::run_untyped(c, text, QUERY_LIMITS).await.unwrap_or_else(|e| panic!("{text}: {e}")).rows
}

/// A database of its own on `s`, dropped (with every connection to it) when this is.
struct OwnDatabase {
    name: String,
    admin: PgConnection,
    runtime: tokio::runtime::Handle,
}

impl OwnDatabase {
    async fn new(s: &Server, db: &TestDb) -> OwnDatabase {
        let admin = conn(s, db, "dbee").await;
        let name = unique("secb_path");
        sql(&admin, &format!("CREATE DATABASE {name}")).await;
        OwnDatabase { name, admin, runtime: tokio::runtime::Handle::current() }
    }
}

impl Drop for OwnDatabase {
    fn drop(&mut self) {
        let drop_it = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
        let (admin, runtime) = (&self.admin, &self.runtime);
        let _ = tokio::task::block_in_place(|| runtime.block_on(query::run_untyped(admin, &drop_it, QUERY_LIMITS)));
    }
}

/// Objects that log each call into `public.calls`: an `unnest(smallint[])`, which beats
/// `pg_catalog.unnest(anyarray)` as the exact match for a key's columns, and a `!~~` (`NOT LIKE`)
/// on two names, which beats `pg_catalog`'s `name !~~ text` for a name compared with a literal.
const PLANTED: &str = r#"
CREATE TABLE public.calls (what text);
CREATE FUNCTION public.unnest(smallint[]) RETURNS SETOF smallint LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO public.calls VALUES ('unnest');
  RETURN QUERY SELECT x FROM pg_catalog.unnest($1) x;
END $$;
CREATE FUNCTION public.not_like(name, name) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO public.calls VALUES ('!~~');
  RETURN $1::text OPERATOR(pg_catalog.!~~) $2::text;
END $$;
CREATE OPERATOR public.!~~ (LEFTARG = name, RIGHTARG = name, FUNCTION = public.not_like);
CREATE TABLE public.t (id int PRIMARY KEY, other int UNIQUE);
"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_read_runs_no_function_or_operator_planted_in_public() {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let own = OwnDatabase::new(s, db).await;
        let planted = conn(s, db, &own.name).await;
        sql(&planted, PLANTED).await;
        // The planted operator is the one an unqualified `NOT LIKE` on a name finds.
        sql(&planted, "SELECT 'x'::name NOT LIKE 'y'").await;
        assert_eq!(sql(&planted, "SELECT count(*)::text FROM public.calls").await, [[Some("1".to_owned())]], "{}", s.name);
        sql(&planted, "DELETE FROM public.calls").await;

        let c = conn(s, db, &own.name).await;
        let catalog = load_catalog(&c).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let t = catalog.schemas.iter().flat_map(|sc| &sc.tables).find(|t| t.table.name == "t").expect("public.t");
        assert_eq!(t.primary_key, ["id"], "{}: the keys still load", s.name);
        let config = config(s, &own.name);
        let target =
            Target { config: &config, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
        let listing = list_connection(target).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        assert!(list_databases(&listing).await.unwrap_or_else(|e| panic!("{}: {e}", s.name)).contains(&own.name));
        let calls = sql(&planted, "SELECT what || ' ' || count(*) FROM public.calls GROUP BY what ORDER BY what").await;
        println!("{}: calls of the planted objects while dbee read the catalog: {calls:?}", s.name);
        assert!(calls.is_empty(), "{}: dbee's catalog read ran {calls:?}", s.name);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_path_with_another_schema_before_pg_catalog_gets_a_warning() {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let own = OwnDatabase::new(s, db).await;
        assert!(conn(s, db, &own.name).await.info().warnings.is_empty(), "{}: the default search path", s.name);
        let admin = conn(s, db, &own.name).await;
        for (path, warned) in
            [("public, pg_catalog", true), ("\"$user\", public", false), ("pg_catalog, public", false), ("pg_temp, pg_catalog, public", false)]
        {
            sql(&admin, &format!("ALTER DATABASE {} SET search_path = {path}", own.name)).await;
            let warnings = conn(s, db, &own.name).await.info().warnings.clone();
            assert_eq!(warnings.iter().any(|w| w.contains("pg_catalog")), warned, "{}: {path}: {warnings:?}", s.name);
            if warned {
                println!("{}: {}", s.name, warnings.join(" / "));
            }
        }
    }
}

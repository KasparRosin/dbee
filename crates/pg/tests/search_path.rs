//! dbee's own Postgres SQL never runs a function or operator someone made in another schema: the
//! catalog reads run with `search_path` set to `pg_catalog, pg_temp` and name `pg_catalog`'s
//! functions, filters and saved changes name `pg_catalog`'s operators where the column's are built
//! in, and a search path that puts another schema before `pg_catalog` gets a warning.
//!
//! Each test makes its own database on every Postgres test server, plants such objects in its
//! `public` schema, and drops the database at the end; nothing else is written.

use dbee_config::{ConnectionConfig, SslMode};
use dbee_driver::{Purpose, QUERY_LIMITS, Target};
use dbee_pg::catalog::{list_connection, list_databases, load_catalog};
use dbee_pg::{PgConnection, connect, query};
use dbee_sql::Dialect;
use dbee_sql::edit::{NewValue, update_sql};
use dbee_sql::filter::{Condition, Filter, Op};
use dbee_sql::select::{count_sql, page_sql};
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

/// An `oid = oid` that logs each call, in `public`: the same signature as `pg_catalog`'s, so it is
/// found only where `public` comes before `pg_catalog` on the search path.
const PLANTED_OID_EQ: &str = r#"
CREATE TABLE public.calls (what text);
CREATE FUNCTION public.oid_eq(oid, oid) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO public.calls VALUES ('oid = oid');
  RETURN $1 OPERATOR(pg_catalog.=) $2;
END $$;
CREATE OPERATOR public.= (LEFTARG = oid, RIGHTARG = oid, FUNCTION = public.oid_eq);
"#;

/// The catalog read runs with its own search path, `pg_catalog, pg_temp`: a database whose path
/// puts `public` first, with an operator of `pg_catalog`'s exact signature planted there, still
/// gets none of it called while dbee reads the catalog.
#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_read_keeps_its_own_search_path() {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let own = OwnDatabase::new(s, db).await;
        let planted = conn(s, db, &own.name).await;
        sql(&planted, PLANTED_OID_EQ).await;
        sql(&planted, "CREATE TABLE public.t (id int PRIMARY KEY)").await;
        sql(&planted, &format!("ALTER DATABASE {} SET search_path = public, pg_catalog", own.name)).await;
        let c = conn(s, db, &own.name).await;
        // On this path an unqualified oid comparison finds the planted operator.
        sql(&c, "SELECT 1 WHERE 'pg_catalog.pg_class'::pg_catalog.regclass::pg_catalog.oid = 1259::pg_catalog.oid").await;
        assert_eq!(sql(&c, "SELECT pg_catalog.count(*)::pg_catalog.text FROM public.calls").await, [[Some("1".to_owned())]], "{}", s.name);
        sql(&c, "DELETE FROM public.calls").await;
        load_catalog(&c).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let calls = sql(&c, "SELECT pg_catalog.count(*)::pg_catalog.text FROM public.calls").await;
        println!("{}: calls of the planted oid = oid while dbee read the catalog: {calls:?}", s.name);
        assert_eq!(calls, [[Some("0".to_owned())]], "{}: the catalog read used the session's search path", s.name);
    }
}

/// Logging operators in `public`, each an exact match where `pg_catalog`'s needs a coercion or is
/// polymorphic (`varchar` with `varchar` or `text`, `integer[]`, an enum), so Postgres would take
/// them for an unqualified operator with the default search path.
const PLANTED_OPERATORS: &str = r#"
CREATE TABLE public.calls (what text);
CREATE TYPE public.mood AS ENUM ('sad', 'ok', 'happy');
CREATE FUNCTION public.log(what text) RETURNS void LANGUAGE sql AS $$ INSERT INTO public.calls VALUES (what) $$;
CREATE FUNCTION public.eq_vv(varchar, varchar) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('varchar = varchar'); RETURN $1::text OPERATOR(pg_catalog.=) $2::text; END $$;
CREATE OPERATOR public.= (LEFTARG = varchar, RIGHTARG = varchar, FUNCTION = public.eq_vv);
CREATE FUNCTION public.eq_vt(varchar, text) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('varchar = text'); RETURN $1::text OPERATOR(pg_catalog.=) $2; END $$;
CREATE OPERATOR public.= (LEFTARG = varchar, RIGHTARG = text, FUNCTION = public.eq_vt);
CREATE FUNCTION public.ne_vt(varchar, text) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('varchar <> text'); RETURN $1::text OPERATOR(pg_catalog.<>) $2; END $$;
CREATE OPERATOR public.<> (LEFTARG = varchar, RIGHTARG = text, FUNCTION = public.ne_vt);
CREATE FUNCTION public.lt_vt(varchar, text) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('varchar < text'); RETURN $1::text OPERATOR(pg_catalog.<) $2; END $$;
CREATE OPERATOR public.< (LEFTARG = varchar, RIGHTARG = text, FUNCTION = public.lt_vt);
CREATE FUNCTION public.like_vv(varchar, varchar) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('varchar ~~ varchar'); RETURN $1::text OPERATOR(pg_catalog.~~) $2::text; END $$;
CREATE OPERATOR public.~~ (LEFTARG = varchar, RIGHTARG = varchar, FUNCTION = public.like_vv);
CREATE FUNCTION public.eq_ii(int[], int[]) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('integer[] = integer[]'); RETURN $1 OPERATOR(pg_catalog.=) $2; END $$;
CREATE OPERATOR public.= (LEFTARG = int[], RIGHTARG = int[], FUNCTION = public.eq_ii);
CREATE FUNCTION public.eq_mm(public.mood, public.mood) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN PERFORM public.log('mood = mood'); RETURN $1 OPERATOR(pg_catalog.=) $2; END $$;
CREATE OPERATOR public.= (LEFTARG = public.mood, RIGHTARG = public.mood, FUNCTION = public.eq_mm);
CREATE TABLE public.t (code varchar(10) PRIMARY KEY, note text, tags int[]);
INSERT INTO public.t VALUES ('a', 'n', '{1,2}'), ('b', 'm', '{3}');
CREATE TABLE public.e (m public.mood PRIMARY KEY, note text);
INSERT INTO public.e VALUES ('ok', 'n');
"#;

/// dbee's own filters, counts and saved changes name `pg_catalog`'s operators where the column's
/// are built in (a `varchar`, an array, an enum), so the planted exact matches are never called,
/// with the default search path; and the column's index still serves the comparison.
#[tokio::test(flavor = "multi_thread")]
async fn filters_and_saved_changes_run_no_operator_planted_in_public() {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let own = OwnDatabase::new(s, db).await;
        let planted = conn(s, db, &own.name).await;
        sql(&planted, PLANTED_OPERATORS).await;
        // The planted operators are the ones unqualified SQL finds.
        sql(&planted, "SELECT count(*) FROM public.t WHERE code = 'a'::text").await;
        assert_eq!(sql(&planted, "SELECT count(*)::text FROM public.calls").await, [[Some("2".to_owned())]], "{}", s.name);
        sql(&planted, "DELETE FROM public.calls").await;

        let c = conn(s, db, &own.name).await;
        let catalog = load_catalog(&c).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let table = |name: &str| catalog.schemas.iter().flat_map(|sc| &sc.tables).find(|t| t.table.name == name).expect("a planted table").clone();
        let (t, e) = (table("t"), table("e"));
        let d = Dialect::Postgres;
        let filter = |column: &str, op: Op, value: &str| Condition::Filters(vec![Filter { column: column.into(), op, value: value.into() }]);
        let mut sent = Vec::new();
        for (cond, rows) in [
            (filter("code", Op::Eq, "a"), 1),
            (filter("code", Op::Ne, "a"), 1),
            (filter("code", Op::Lt, "b"), 1),
            (filter("code", Op::In, "a, b"), 2),
            (filter("code", Op::Like, "a%"), 1),
            (filter("tags", Op::Eq, "{1,2}"), 1),
        ] {
            let page = page_sql(d, &t, &cond, None, 0).unwrap();
            let count = count_sql(d, &t, &cond).unwrap();
            assert_eq!(query::run_untyped(&c, &page, QUERY_LIMITS).await.map(|r| r.rows.len()), Ok(rows), "{}: {page}", s.name);
            assert_eq!(query::scalar_u64(&c, &count).await, Ok(rows as u64), "{}: {count}", s.name);
            sent.push(page);
        }
        let note = |t: &dbee_core::TableInfo| t.column("note").unwrap().clone();
        let update_t = update_sql(d, &t, &[(t.column("code").unwrap(), "a")], &[(&note(&t), &NewValue::Text("x".into()))]).unwrap();
        let update_e = update_sql(d, &e, &[(e.column("m").unwrap(), "ok")], &[(&note(&e), &NewValue::Text("x".into()))]).unwrap();
        assert_eq!(dbee_pg::apply(&c, &[update_t.clone(), update_e.clone()]).await.map_err(|e| e.error), Ok(2), "{}", s.name);
        sent.extend([update_t, update_e]);
        let calls = sql(&planted, "SELECT what || ' ' || count(*) FROM public.calls GROUP BY what ORDER BY what").await;
        println!("{}: sent {sent:#?}\ncalls of the planted operators: {calls:?}", s.name);
        assert!(calls.is_empty(), "{}: dbee's SQL ran {calls:?}", s.name);
        // The primary keys' indexes still serve the comparisons.
        sql(&c, "SET enable_seqscan = off").await;
        for statement in [&sent[0], &sent[3], &sent[6], &sent[7]] {
            let plan: Vec<String> = sql(&c, &format!("EXPLAIN {statement}")).await.into_iter().flatten().flatten().collect();
            assert!(plan.iter().any(|line| line.contains("Index")), "{}: {statement}: {plan:#?}", s.name);
        }
    }
}

/// A type whose schema is on the session's search path shows without it, as before dbee read the
/// catalog with only `pg_catalog` on the path, while `type_sql` names the schema for SQL; a type
/// outside the path, and a built-in one, show as `type_sql`. An extension type keeps its own
/// operators, so `citext` still compares without case.
#[tokio::test(flavor = "multi_thread")]
async fn type_labels_leave_out_a_schema_on_the_search_path() {
    let db = require_testdb!();
    for s in db.servers_of(&[Kind::Postgres]) {
        let own = OwnDatabase::new(s, db).await;
        let c = conn(s, db, &own.name).await;
        sql(&c, "CREATE EXTENSION citext; CREATE SCHEMA other; CREATE TYPE public.mood AS ENUM ('ok'); CREATE TYPE other.feel AS ENUM ('ok')").await;
        sql(&c, "CREATE TABLE public.l (id int PRIMARY KEY, c public.citext, m public.mood, f other.feel, v varchar(5)); INSERT INTO public.l VALUES (1, 'Ann', 'ok', 'ok', 'x')").await;
        let c = conn(s, db, &own.name).await;
        let catalog = load_catalog(&c).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
        let l = catalog.schemas.iter().flat_map(|sc| &sc.tables).find(|t| t.table.name == "l").expect("l").clone();
        let shown = |name: &str| {
            let column = l.column(name).unwrap();
            (column.type_sql.clone(), column.column().type_label, column.builtin_operators)
        };
        assert_eq!(shown("c"), ("public.citext".to_owned(), "citext".to_owned(), false), "{}", s.name);
        assert_eq!(shown("m"), ("public.mood".to_owned(), "mood".to_owned(), true), "{}", s.name);
        assert_eq!(shown("f"), ("other.feel".to_owned(), "other.feel".to_owned(), true), "{}", s.name);
        assert_eq!(shown("v"), ("character varying(5)".to_owned(), "character varying(5)".to_owned(), true), "{}", s.name);
        let cond = Condition::Filters(vec![Filter { column: "c".into(), op: Op::Eq, value: "ANN".into() }]);
        assert_eq!(query::scalar_u64(&c, &count_sql(Dialect::Postgres, &l, &cond).unwrap()).await, Ok(1), "{}: citext's own =", s.name);
    }
}

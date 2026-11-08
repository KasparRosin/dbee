//! `schema.inspect`, `quickSearch.find` and the fuzzy scoring behind it. The command tests run on
//! every engine and need the Docker test servers (`cargo xtask testdb up`); the scoring, the
//! saved connections without a connection and the timings do not.

mod common;

use std::time::Instant;

use common::*;
use dbee_config::{ConnectionColor, ConnectionConfig, DbKind};
use dbee_core::{Catalog, ColumnInfo, RelKind, SchemaInfo, TableInfo, TableRef, TypeClass};
use dbee_engine::Json;
use dbee_engine::fuzzy::{Found, find, find_connections, score, search};
use dbee_testkit::{require_testdb, unique};
use serde_json::json;

fn table(schema: &str, name: &str, kind: RelKind) -> TableInfo {
    TableInfo {
        table: TableRef { schema: schema.into(), name: name.into() },
        kind,
        columns: vec![ColumnInfo {
            name: "id".into(),
            type_sql: "integer".into(),
            base_type: "int4".into(),
            class: TypeClass::Number,
            not_null: true,
            has_default: false,
            generated: Default::default(),
            hidden: false,
            type_label: String::new(),
            builtin_operators: false,
        }],
        primary_key: vec![],
        unique_keys: vec![],
        foreign_keys: vec![],
        estimated_rows: None,
        no_edit_reason: None,
    }
}

fn schema(name: &str, system: bool, tables: &[&str]) -> SchemaInfo {
    SchemaInfo { name: name.into(), system, tables: tables.iter().map(|t| table(name, t, RelKind::Table)).collect() }
}

/// A saved server connection named `name` with id `id`, never opened (nothing listens on port 1).
fn saved(id: &str, name: &str) -> ConnectionConfig {
    ConnectionConfig { id: id.into(), name: name.into(), host: "127.0.0.1".into(), port: 1, user: "nobody".into(), ..ConnectionConfig::default() }
}

/// The hits of `r` (a `quickSearch.find` result) that are of kind `hit`.
fn hits_of(r: &Json, hit: &str) -> Vec<Json> {
    r.as_array().unwrap().iter().filter(|h| h["hit"] == hit).cloned().collect()
}

#[test]
fn prefix_beats_scattered() {
    assert!(score("ord", "orders") > score("ord", "product_records"));
    assert_eq!(score("xyz", "orders"), None);
}

#[test]
fn scoring_rules() {
    assert!(score("ORD", "orders").is_some(), "case-insensitive");
    assert!(score("", "orders").is_some(), "an empty query matches");
    assert_eq!(score("orders2", "orders"), None, "the query must fit in the candidate");
    assert!(score("it", "order_items") > score("it", "orbit"), "after an underscore beats inside a word");
    assert!(score("shop.o", "shop.orders") > score("shop.o", "shop_xo"), "after a dot");
    assert!(score("orders", "orders") > score("orders", "orders_archive"), "exact beats longer");
    assert!(score("ab", "ab_x") > score("ab", "a_b"), "consecutive beats split");
    assert!(score("é", "École").is_some(), "non-ASCII");
    assert!(score("a", &"x".repeat(100_000)).is_none());
}

#[test]
fn find_orders_and_sorting() {
    let c = Catalog {
        schemas: vec![
            schema("pg_catalog", true, &["orders"]),
            schema("zeta", false, &["orders", "orders_archive", "my_orders"]),
            schema("shop", false, &["orders"]),
        ],
        default_schema: None,
    };
    let hits = find(&c, "orders", 20);
    let names: Vec<String> = hits.iter().map(|h| h.table.display()).collect();
    // Equal score and name length: user schemas first (catalog order), then system schemas.
    assert_eq!(&names[..3], ["zeta.orders", "shop.orders", "pg_catalog.orders"]);
    assert_eq!(names[3], "zeta.orders_archive");
    assert_eq!(names[4], "zeta.my_orders");
    assert_eq!(find(&c, "orders", 2).len(), 2);
    assert!(find(&c, "orders", 0).is_empty());
    assert!(find(&c, "qqq", 20).is_empty());
}

#[test]
fn quick_search_finds_orders() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let r = s.run_blocking("quickSearch.find", json!({ "query": "ord" }), T).unwrap();
        assert_eq!(r[0]["hit"], "table", "{}: {r}", t.name());
        assert_eq!(r[0]["schema"], t.schema("shop"), "{}: {r}", t.name());
        assert_eq!(r[0]["table"], "orders", "{r}");
        assert_eq!(r[0]["kind"], "table");
        let limited = s.run_blocking("quickSearch.find", json!({ "query": "o", "limit": 3 }), T).unwrap();
        assert_eq!(limited.as_array().map(Vec::len), Some(3));
        let e = s.run_blocking("quickSearch.find", json!({}), T).unwrap_err();
        assert_eq!(e.code(), "badParams");
    });
}

#[test]
fn table_hits_keep_their_fields() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let r = s.run_blocking("quickSearch.find", json!({ "query": "order_totals" }), T).unwrap();
        // `kind` is the relation's kind, as before connections were found too.
        assert_eq!(r[0], json!({ "hit": "table", "schema": t.schema("shop"), "table": "order_totals", "kind": "view" }), "{}: {r}", t.name());
    });
}

#[test]
fn quick_search_lists_saved_connections() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let shop = save(s, ConnectionConfig { color: ConnectionColor::Green, ..saved("", "Shop live") }, None);
        let open = s.inspect()["connection"]["id"].as_str().unwrap().to_string();
        let r = s.run_blocking("quickSearch.find", json!({ "query": "shop" }), T).unwrap();
        let want = json!({ "hit": "connection", "id": shop, "name": "Shop live", "dbKind": "postgres", "kindLabel": "PostgreSQL", "color": "green" });
        assert_eq!(hits_of(&r, "connection"), [want], "{}: {r}", t.name());
        // The open connection, saved under the target's name, is left out; its tables are found.
        let r = s.run_blocking("quickSearch.find", json!({ "query": t.name() }), T).unwrap();
        assert!(hits_of(&r, "connection").iter().all(|h| h["id"] != open.as_str()), "{}: {r}", t.name());
        let r = s.run_blocking("quickSearch.find", json!({ "query": "orders" }), T).unwrap();
        assert_eq!(r[0]["table"], "orders", "{r}");
    });
}

#[test]
fn quick_search_on_the_connection_screen_lists_connections_only() {
    let mut s = session();
    assert_eq!(s.run_blocking("quickSearch.find", json!({ "query": "pg" }), T).unwrap(), json!([]), "nothing saved, nothing found");
    let pg = save(&mut s, saved("", "pg staging"), None);
    save(&mut s, saved("", "mysql box"), None);
    let r = s.run_blocking("quickSearch.find", json!({ "query": "pg" }), T).unwrap();
    assert!(r.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|h| h["hit"] == "connection")), "{r}");
    assert_eq!(r[0]["id"], pg.as_str(), "{r}");
    assert_eq!(s.run_blocking("quickSearch.find", json!({ "query": "zzz" }), T).unwrap(), json!([]));
}

#[test]
fn a_file_connection_is_found_by_its_file_name() {
    let mut s = session();
    let file = ConnectionConfig {
        kind: DbKind::Sqlite,
        file: Some("/nowhere/inventory.sqlite".into()),
        host: String::new(),
        port: 0,
        ..ConnectionConfig::default()
    };
    let id = save(&mut s, file, None);
    let r = s.run_blocking("quickSearch.find", json!({ "query": "invent" }), T).unwrap();
    let want = json!({ "hit": "connection", "id": id, "name": "inventory.sqlite", "dbKind": "sqlite", "kindLabel": "SQLite", "color": "none" });
    assert_eq!(r, json!([want]));
}

#[test]
fn quick_search_while_lost_lists_connections_only() {
    let _db = require_testdb!();
    for_each_engine_exclusive(|t, s| {
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        let other = save(s, saved("", "orders replica"), None);
        let before = s.run_blocking("quickSearch.find", json!({ "query": "orders" }), T).unwrap();
        assert!(!hits_of(&before, "table").is_empty(), "{}: {before}", t.name());
        kill_dbee_connections(t, s);
        let e = run_err(s, "table.open", json!({ "schema": t.schema("shop"), "table": "orders" }));
        assert_eq!(e.code(), "connectionLost", "{}: {e}", t.name());
        let r = s.run_blocking("quickSearch.find", json!({ "query": "orders" }), T).unwrap();
        assert_eq!(hits_of(&r, "table"), Vec::<Json>::new(), "{}: no tables while lost: {r}", t.name());
        assert_eq!(hits_of(&r, "connection").iter().map(|h| h["id"].clone()).collect::<Vec<_>>(), [json!(other)], "{r}");
    });
}

#[test]
fn connections_are_scored_by_their_display_names() {
    let file = ConnectionConfig { id: "f".into(), kind: DbKind::Sqlite, file: Some("/data/inventory.db".into()), ..ConnectionConfig::default() };
    let unnamed =
        ConnectionConfig { id: "u".into(), user: "ann".into(), host: "db.example".into(), database: "shop".into(), ..ConnectionConfig::default() };
    let list = [saved("a", "Shop live"), saved("b", "Shop staging"), file, unnamed];
    let names = |hits: Vec<dbee_engine::fuzzy::ConnectionHit<'_>>| hits.into_iter().map(|h| h.name).collect::<Vec<_>>();
    assert_eq!(names(find_connections(&list, None, "shop", 20)), ["Shop live", "Shop staging", "ann@db.example/shop"]);
    assert_eq!(names(find_connections(&list, None, "invent", 20)), ["inventory.db"], "a file without a name by its file name");
    // The open connection is left out by its id; no id, or an empty one, leaves none out.
    assert_eq!(names(find_connections(&list, Some("a"), "shop", 20)), ["Shop staging", "ann@db.example/shop"]);
    assert_eq!(find_connections(&list, Some(""), "shop", 20).len(), 3);
    assert_eq!(find_connections(&list, Some("nope"), "shop", 20).len(), 3);
    assert_eq!(names(find_connections(&list, None, "shop", 1)), ["Shop live"]);
    assert!(find_connections(&list, None, "shop", 0).is_empty());
}

#[test]
fn a_space_or_a_dash_starts_a_word() {
    // A connection's name is free text: its words follow a space or a dash, as a table's follow `_`.
    assert!(score("live", "Shop live") > score("live", "delivery"));
    assert!(score("stag", "pg staging") > score("stag", "pg_stat_gssapi"));
    assert!(score("dev", "shop-dev") > score("dev", "shop_devices"));
    let c = Catalog { schemas: vec![schema("shop", false, &["delivery"]), schema("pg_catalog", true, &["pg_stat_gssapi"])], default_schema: None };
    let list = [saved("a", "Shop live"), saved("b", "pg staging")];
    let first = |q: &str| match search(Some(&c), &list, None, q, 20).into_iter().next() {
        Some(Found::Connection(h)) => h.name,
        other => panic!("{q}: a connection comes first, not {other:?}"),
    };
    assert_eq!((first("live"), first("stag")), ("Shop live".to_string(), "pg staging".to_string()));
}

fn labels(found: Vec<Found<'_>>) -> Vec<String> {
    found
        .into_iter()
        .map(|f| match f {
            Found::Table(h) => format!("table {}", h.table.name),
            Found::Connection(h) => format!("connection {}", h.name),
        })
        .collect()
}

#[test]
fn an_empty_query_keeps_the_saved_order() {
    let list = [saved("a", "Orders replica"), saved("b", "pg14"), saved("c", "Shop live")];
    let names = |hits: Vec<dbee_engine::fuzzy::ConnectionHit<'_>>| hits.into_iter().map(|h| h.name).collect::<Vec<_>>();
    // As the connection screen lists them, not by the length of their names.
    assert_eq!(names(find_connections(&list, None, "", 20)), ["Orders replica", "pg14", "Shop live"]);
    assert_eq!(names(find_connections(&list, None, "  ", 20)), ["Orders replica", "pg14", "Shop live"]);
    assert_eq!(names(find_connections(&list, Some("b"), "", 20)), ["Orders replica", "Shop live"]);
    assert_eq!(labels(search(None, &list, None, "", 20)), ["connection Orders replica", "connection pg14", "connection Shop live"]);
    // Connected: the tables in their own empty-query order (M1's), then the connections.
    let c = Catalog { schemas: vec![schema("shop", false, &["orders", "items"])], default_schema: None };
    assert_eq!(
        labels(search(Some(&c), &list, None, "", 20)),
        ["table items", "table orders", "connection Orders replica", "connection pg14", "connection Shop live"]
    );
    assert_eq!(labels(search(Some(&c), &list, None, "", 3)), ["table items", "table orders", "connection Orders replica"]);
}

#[test]
fn search_mixes_tables_and_connections_by_score() {
    let c = Catalog { schemas: vec![schema("shop", false, &["orders", "shop_orders"])], default_schema: None };
    let list = [saved("a", "orders"), saved("b", "Orders archive")];
    // A tie (the same name) puts the table first; a weaker match comes after a better one of
    // either kind.
    assert_eq!(
        labels(search(Some(&c), &list, None, "orders", 20)),
        ["table orders", "connection orders", "connection Orders archive", "table shop_orders"]
    );
    assert_eq!(labels(search(Some(&c), &list, None, "orders", 3)).len(), 3);
    assert_eq!(labels(search(None, &list, Some("a"), "orders", 20)), ["connection Orders archive"], "no catalog: connections only");
    assert!(search(Some(&c), &list, None, "orders", 0).is_empty());
}

#[test]
fn inspect_catalog_and_table() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let shop = t.schema("shop");
        let all = s.run_blocking("schema.inspect", json!({}), T).unwrap();
        assert!(all["schemas"].as_array().is_some_and(|a| !a.is_empty()), "{all}");
        let orders = s.run_blocking("schema.inspect", json!({ "schema": shop, "table": "orders" }), T).unwrap();
        assert_eq!(orders["table"]["name"], "orders");
        assert!(orders["columns"].as_array().is_some_and(|a| !a.is_empty()));
        let one = s.run_blocking("schema.inspect", json!({ "schema": shop }), T).unwrap();
        assert_eq!(one["name"], shop);
        assert_eq!(s.run_blocking("schema.inspect", json!({ "schema": shop, "table": "nope" }), T).unwrap_err().code(), "badParams");
        assert_eq!(s.run_blocking("schema.inspect", json!({ "table": "orders" }), T).unwrap_err().code(), "badParams");
    });
}

#[test]
fn inspect_needs_a_connection() {
    let mut s = session();
    assert_eq!(s.execute("schema.inspect", json!({})).unwrap_err().code(), "disabled");
}

#[test]
fn refresh_sees_new_table() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let u = unique("t_inspect");
        let guard = scratch_table(t, &u, "CREATE TABLE {t} (id int)");
        let refreshed = s.run_blocking("schema.refresh", json!({}), T);
        let inspected = s.run_blocking("schema.inspect", json!({ "schema": t.schema("scratch"), "table": u }), T);
        let found = s.run_blocking("quickSearch.find", json!({ "query": u }), T);
        drop(guard);
        refreshed.unwrap();
        assert_eq!(inspected.unwrap()["table"]["name"], u.as_str(), "{}", t.name());
        assert_eq!(found.unwrap()[0]["table"], u.as_str());
    });
}

/// 1,000 tables in two schemas, the system one last.
fn thousand_tables() -> Catalog {
    let names: Vec<String> =
        (0..1000).map(|i| format!("{}_{}_{i}", ["orders", "users", "items", "product_records", "audit"][i % 5], ["a", "bb", "ccc"][i % 3])).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    Catalog { schemas: vec![schema("shop", false, &refs[..800]), schema("pg_catalog", true, &refs[800..])], default_schema: None }
}

/// No server: the command runs this same `find` on the session's catalog.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn quick_search_1000_tables_within_a_frame() {
    let c = thousand_tables();
    let mut times = Vec::new();
    for q in ["ord", "usr", "pr_rec", "zzz", "o", "shop.it"] {
        for _ in 0..20 {
            let t = Instant::now();
            let hits = find(&c, q, 20);
            times.push(t.elapsed());
            std::hint::black_box(hits);
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    println!("quick search over 1000 tables: median {median:?}, max {:?}", times[times.len() - 1]);
    assert!(median.as_secs_f64() < 0.002, "median {median:?} is over 2 ms");
}

/// No server: the command runs this same `search` on the session's catalog and saved list.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn quick_search_with_connections_within_budget() {
    let c = thousand_tables();
    let list: Vec<ConnectionConfig> = (0..100)
        .map(|i| match i % 5 {
            // A file connection without a name is scored on its file name.
            4 => ConnectionConfig {
                id: format!("c{i}"),
                kind: DbKind::Sqlite,
                file: Some(format!("/data/orders_{i}.sqlite").into()),
                ..ConnectionConfig::default()
            },
            k => saved(&format!("c{i}"), &format!("{} {i}", ["Shop live", "pg staging", "orders replica", "analytics"][k])),
        })
        .collect();
    let mut times = Vec::new();
    for q in ["ord", "usr", "pr_rec", "zzz", "o", "shop.it", "shop live", "stag"] {
        for _ in 0..20 {
            let t = Instant::now();
            let hits = search(Some(&c), &list, Some("c0"), q, 20);
            times.push(t.elapsed());
            std::hint::black_box(hits);
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    println!("quick search over 1000 tables and 100 connections: median {median:?}, max {:?}", times[times.len() - 1]);
    assert!(median.as_secs_f64() < 0.002, "median {median:?} is over 2 ms");
}

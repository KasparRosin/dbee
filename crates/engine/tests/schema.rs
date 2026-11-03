//! `schema.inspect`, `quickSearch.find` and the fuzzy scoring behind it. The command tests run on
//! every engine and need the Docker test servers (`cargo xtask testdb up`); the scoring and the
//! 1,000-table timing do not.

mod common;

use std::time::Instant;

use common::*;
use dbee_core::{Catalog, ColumnInfo, RelKind, SchemaInfo, TableInfo, TableRef, TypeClass};
use dbee_engine::fuzzy::{find, score};
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
fn needs_a_connection() {
    let mut s = session();
    assert_eq!(s.execute("schema.inspect", json!({})).unwrap_err().code(), "disabled");
    assert_eq!(s.execute("quickSearch.find", json!({ "query": "a" })).unwrap_err().code(), "disabled");
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

/// No server: the command runs this same `find` on the session's catalog.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn quick_search_1000_tables_within_a_frame() {
    let names: Vec<String> =
        (0..1000).map(|i| format!("{}_{}_{i}", ["orders", "users", "items", "product_records", "audit"][i % 5], ["a", "bb", "ccc"][i % 3])).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let c = Catalog { schemas: vec![schema("shop", false, &refs[..800]), schema("pg_catalog", true, &refs[800..])], default_schema: None };
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

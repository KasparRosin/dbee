//! Table tabs: open, filters, paging, sort, count, cancel, refresh, read and following foreign
//! keys. Needs the Docker test servers (`cargo xtask testdb up`); the seed tables are read-only,
//! and the one test that writes makes its own tables in `scratch`.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_engine::{EngineError, Json, Outcome, Session};
use dbee_testkit::{require_testdb, unique};
use serde_json::json;

fn open_table(s: &mut Session, schema: &str, table: &str) -> Json {
    s.run_blocking("table.open", json!({ "schema": schema, "table": table }), T)
        .unwrap_or_else(|e| panic!("table.open {schema}.{table}: {e} [{}] {}", e.code(), e.data()))
}

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.run_blocking(id, params, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data()))
}

/// `table.read` of the active tab, every loaded row.
fn read(s: &mut Session) -> Json {
    run(s, "table.read", json!({}))
}

/// The index of column `name` in a `table.read` result.
fn col(read: &Json, name: &str) -> usize {
    read["columns"].as_array().unwrap().iter().position(|c| c["name"] == name).unwrap_or_else(|| panic!("no column {name}: {}", read["columns"]))
}

/// Row `row`, column `name` of a `table.read` result, as text (NULL is `None`).
fn cell<'a>(read: &'a Json, row: usize, name: &str) -> Option<&'a str> {
    read["rows"][row][col(read, name)].as_str()
}

fn rows(read: &Json) -> usize {
    read["rows"].as_array().unwrap().len()
}

fn first_id(s: &mut Session) -> i64 {
    let r = read(s);
    cell(&r, 0, "id").unwrap().parse().unwrap()
}

fn page(s: &mut Session, to: Json) -> Json {
    run(s, "table.page", json!({ "to": to }))
}

/// Drops the scratch tables it names when the test ends, also when it fails.
struct Scratch(Vec<String>);

impl Drop for Scratch {
    fn drop(&mut self) {
        for t in &self.0 {
            let _ = dbee_testkit::psql(pg14(), &format!("DROP TABLE IF EXISTS scratch.{t} CASCADE"));
        }
    }
}

#[test]
fn opens_first_page_of_100() {
    let _db = require_testdb!();
    let mut s = connected();
    let r = s.run_blocking("table.open", json!({"schema":"shop","table":"orders"}), T).unwrap();
    assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(100), Some(true)));
    assert_eq!(s.inspect()["active"], r["tab"]);
    assert_eq!(s.inspect()["table"]["name"], "orders");
    let d = read(&mut s);
    assert_eq!((rows(&d), d["page"].as_u64(), d["hasNext"].as_bool()), (100, Some(0), Some(true)));
    // The columns come from the catalog, with their types; rows in primary key order.
    assert_eq!(d["columns"][0], json!({ "name": "id", "baseType": "int8", "typeLabel": "bigint" }));
    assert_eq!(d["columns"][col(&d, "status")]["typeLabel"], "shop.order_status");
    assert_eq!((cell(&d, 0, "id"), cell(&d, 99, "id")), (Some("1"), Some("100")));
    assert_eq!(d["total"], "unknown");
    assert_eq!((d["editable"].as_bool(), &d["notEditableReason"]), (Some(true), &Json::Null));
    // A window of the loaded rows.
    let w = run(&mut s, "table.read", json!({ "offset": 98, "limit": 5 }));
    assert_eq!((rows(&w), cell(&w, 0, "id")), (2, Some("99")));
    assert_eq!(rows(&run(&mut s, "table.read", json!({ "offset": 1000 }))), 0);
    // A table the catalog does not have.
    assert_eq!(s.execute("table.open", json!({ "schema": "shop", "table": "nope" })).unwrap_err().code(), "badParams");
    assert_eq!(s.state().tabs.len(), 1);
}

#[test]
fn day_scenario_filters() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let filters = json!([
        { "column": "dealer_id", "op": "eq", "value": "42" },
        { "column": "status", "op": "eq", "value": "completed" },
        { "column": "total", "op": "gt", "value": "100" },
    ]);
    let r = run(&mut s, "table.setFilters", json!({ "filters": filters }));
    assert!(r["rows"].as_u64().unwrap() >= 1, "{r}");
    let d = read(&mut s);
    assert!(rows(&d) >= 1);
    for i in 0..rows(&d) {
        assert_eq!(cell(&d, i, "dealer_id"), Some("42"));
        assert_eq!(cell(&d, i, "status"), Some("completed"));
        assert!(cell(&d, i, "total").unwrap().parse::<f64>().unwrap() > 100.0, "row {i}");
    }
    let t = &s.inspect()["table"];
    assert_eq!(t["filters"], filters);
    assert_eq!(t["applied"]["filters"], filters);
    // Without `apply` only the bar changes; the rows stay.
    let bar = run(&mut s, "table.setFilters", json!({ "raw": "id < 3", "rawMode": true, "apply": false }));
    assert_eq!((bar["raw"].as_str(), bar["rawMode"].as_bool()), (Some("id < 3"), Some(true)));
    assert_eq!(s.inspect()["table"]["applied"]["filters"], filters);
    assert_eq!(rows(&read(&mut s)), rows(&d));
    // Applying the raw condition.
    let r = run(&mut s, "table.setFilters", json!({}));
    assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(2), Some(false)));
    assert_eq!(s.inspect()["table"]["applied"], json!({ "raw": "id < 3" }));
    // Back to no filter at all.
    let r = run(&mut s, "table.setFilters", json!({ "filters": [], "rawMode": false }));
    assert_eq!(r["rows"], 100);
}

#[test]
fn typing_in_the_filter_bar_journals_one_entry() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "customers");
    for value in ["C", "Cu", "Customer 7"] {
        run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": value }], "apply": false }));
    }
    run(&mut s, "table.setFilters", json!({ "raw": "id < 3", "apply": false }));
    run(&mut s, "table.setFilters", json!({ "apply": true }));
    let tail: Vec<(String, Json)> = s.journal().iter().rev().take(4).rev().cloned().collect();
    let expected = vec![
        ("table.open".to_string(), json!({ "schema": "shop", "table": "customers" })),
        ("table.setFilters".to_string(), json!({ "filters": [{ "column": "name", "op": "eq", "value": "Customer 7" }], "apply": false })),
        ("table.setFilters".to_string(), json!({ "raw": "id < 3", "apply": false })),
        ("table.setFilters".to_string(), json!({ "apply": true })),
    ];
    assert_eq!(tail, expected);
}

#[test]
fn hostile_filter_value_is_just_a_value() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "customers");
    let evil = "x'); DROP TABLE shop.customers; --";
    let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": evil }] }));
    assert_eq!(r["rows"], 0);
    let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "like", "value": evil }] }));
    assert_eq!(r["rows"], 0);
    assert_eq!(dbee_testkit::psql(pg14(), "SELECT count(*) FROM shop.customers").unwrap().trim(), "500");
}

#[test]
fn bad_value_sets_tab_error_and_keeps_tab() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let e = run_err(&mut s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "abc" }] }));
    assert_eq!(e.code(), "server", "{e}");
    let t = s.inspect()["table"].clone();
    assert!(t["error"].as_str().unwrap().contains("bigint"), "{t}");
    assert_eq!(s.state().tabs.len(), 1);
    // The bar keeps what was typed; the rows and what they were loaded with stay.
    assert_eq!(t["filters"][0]["value"], "abc");
    assert_eq!(t["applied"], "none");
    let d = read(&mut s);
    assert_eq!((rows(&d), cell(&d, 0, "id")), (100, Some("1")));
    assert!(d["error"].as_str().unwrap().contains("bigint"));
    // A good filter clears the error.
    run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "7" }] }));
    assert_eq!(s.inspect()["table"]["error"], Json::Null);
    // SQL dbee refuses to build is `badParams`, and changes nothing.
    for bad in [
        json!({ "filters": [{ "column": "nope", "op": "eq", "value": "1" }] }),
        json!({ "filters": [{ "column": "id", "op": "between", "value": "1" }] }),
        json!({ "filters": [{ "column": "id", "op": "in", "value": " , " }] }),
        json!({ "raw": "true; drop table shop.orders", "rawMode": true }),
        json!({ "filters": "id = 1" }),
    ] {
        let e = s.execute("table.setFilters", bad.clone()).unwrap_err();
        assert_eq!(e.code(), "badParams", "{bad}: {e}");
    }
    assert_eq!(s.inspect()["table"]["filters"][0]["value"], "7");
    assert_eq!(s.inspect()["table"]["rawMode"], false);
}

#[test]
fn paging_on_a_million_rows() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    let r = page(&mut s, json!("next"));
    assert_eq!((r["page"].as_u64(), r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(1), Some(100), Some(true)));
    assert_eq!(first_id(&mut s), 101);
    let r = page(&mut s, json!("last"));
    assert_eq!(r["total"], json!({ "exact": 1_000_000 }));
    let d = read(&mut s);
    assert_eq!((d["page"].as_u64(), rows(&d), d["hasNext"].as_bool()), (Some(9999), 100, Some(false)));
    assert_eq!(d["total"], json!({ "exact": 1_000_000 }));
    assert_eq!(cell(&d, 99, "id"), Some("1000000"));
    // Next on the last page stays there.
    assert_eq!(page(&mut s, json!("next"))["page"], 9999);
    assert_eq!(first_id(&mut s), 999_901);
    page(&mut s, json!("prev"));
    assert_eq!(read(&mut s)["page"], 9998);
    assert_eq!(first_id(&mut s), 999_801);
    // Page numbers count from 1.
    page(&mut s, json!(3));
    assert_eq!((read(&mut s)["page"].as_u64(), first_id(&mut s)), (Some(2), 201));
    page(&mut s, json!("first"));
    assert_eq!((read(&mut s)["page"].as_u64(), first_id(&mut s)), (Some(0), 1));
    // Prev on the first page stays there.
    page(&mut s, json!("prev"));
    assert_eq!(read(&mut s)["page"], 0);
    for bad in [json!(0), json!(-1), json!("middle"), json!(1.5), json!(u64::MAX), Json::Null] {
        assert_eq!(s.execute("table.page", json!({ "to": bad })).unwrap_err().code(), "badParams", "{bad}");
    }
    // A refresh reloads the page.
    let r = run(&mut s, "table.refresh", json!({}));
    assert_eq!((r["page"].as_u64(), r["rows"].as_u64()), (Some(0), Some(100)));
}

#[test]
fn sort_desc_then_none() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    page(&mut s, json!("next"));
    run(&mut s, "table.sort", json!({ "column": "id", "dir": "desc" }));
    assert_eq!(first_id(&mut s), 1_000_000);
    // Sorting goes back to the first page.
    assert_eq!(read(&mut s)["page"], 0);
    assert_eq!(s.inspect()["table"]["sort"], json!({ "column": "id", "dir": "desc" }));
    run(&mut s, "table.sort", json!({ "column": "id", "dir": "none" }));
    assert_eq!(first_id(&mut s), 1);
    assert_eq!(s.inspect()["table"]["sort"], Json::Null);
    // Without `dir` it cycles like a header click: asc, desc, none.
    run(&mut s, "table.sort", json!({ "column": "payload" }));
    assert_eq!(s.inspect()["table"]["sort"]["dir"], "asc");
    assert_eq!(cell(&read(&mut s), 0, "payload"), Some("payload 1"));
    run(&mut s, "table.sort", json!({ "column": "payload" }));
    assert_eq!(s.inspect()["table"]["sort"]["dir"], "desc");
    assert_eq!(cell(&read(&mut s), 0, "payload"), Some("payload 999999"));
    run(&mut s, "table.sort", json!({ "column": "payload" }));
    assert_eq!(s.inspect()["table"]["sort"], Json::Null);
    assert_eq!(s.execute("table.sort", json!({ "column": "nope" })).unwrap_err().code(), "badParams");
    assert_eq!(s.execute("table.sort", json!({ "column": "id", "dir": "up" })).unwrap_err().code(), "badParams");
}

#[test]
fn count_estimate_then_exact() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
    let total = s.inspect()["table"]["total"].clone();
    let estimate = total.get("estimate").or_else(|| total.get("counting").and_then(|c| c.get("estimate"))).and_then(Json::as_u64);
    assert!(estimate.is_some_and(|e| e >= 900_000), "{total}");
    let r = s.wait(job, T).unwrap();
    assert_eq!(r["total"], json!({ "exact": 1_000_000 }));
    assert_eq!(read(&mut s)["total"], json!({ "exact": 1_000_000 }));
    // Filtered: no estimate, and a new filter forgets the count.
    run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "le", "value": "10" }] }));
    assert_eq!(s.inspect()["table"]["total"], "unknown");
    let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
    assert_eq!(s.inspect()["table"]["total"], json!({ "counting": { "estimate": null } }));
    assert_eq!(s.wait(job, T).unwrap()["total"], json!({ "exact": 10 }));
    // A view has no estimate.
    open_table(&mut s, "shop", "order_totals");
    let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
    assert_eq!(s.inspect()["table"]["total"], json!({ "counting": { "estimate": null } }));
    assert_eq!(s.wait(job, T).unwrap()["total"], json!({ "exact": 500 }));
}

#[test]
fn follow_foreign_key() {
    let _db = require_testdb!();
    let mut s = connected();
    let orders = open_table(&mut s, "shop", "orders")["tab"].clone();
    let c = cell(&read(&mut s), 0, "customer_id").unwrap().to_owned();
    let r = run(&mut s, "table.followForeignKey", json!({ "row": 0, "column": "customer_id" }));
    assert_eq!(r["rows"], 1);
    let i = s.inspect();
    assert_ne!(i["active"], orders);
    assert_eq!(i["active"], r["tab"]);
    assert_eq!((i["table"]["schema"].as_str(), i["table"]["name"].as_str()), (Some("shop"), Some("customers")));
    assert_eq!(i["table"]["filters"], json!([{ "column": "id", "op": "eq", "value": c }]));
    assert_eq!(cell(&read(&mut s), 0, "id"), Some(c.as_str()));
    assert_eq!(s.state().tabs.len(), 2);
    // A column by its index works too; a column outside every foreign key does not.
    run(&mut s, "tab.activate", json!({ "tab": orders }));
    run(&mut s, "table.followForeignKey", json!({ "row": 1, "column": 1 }));
    assert_eq!(s.state().tabs.len(), 3);
    run(&mut s, "tab.activate", json!({ "tab": orders }));
    for bad in [json!({ "row": 0, "column": "total" }), json!({ "row": 100, "column": "customer_id" }), json!({ "row": 0, "column": 99 })] {
        assert_eq!(s.execute("table.followForeignKey", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
    }
}

#[test]
fn follow_composite_foreign_key() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "shipments");
    let d = read(&mut s);
    let (order, line) = (cell(&d, 0, "order_id").unwrap().to_owned(), cell(&d, 0, "line_no").unwrap().to_owned());
    let r = run(&mut s, "table.followForeignKey", json!({ "row": 0, "column": "line_no" }));
    assert_eq!(r["rows"], 1);
    let t = &s.inspect()["table"];
    assert_eq!(t["name"], "order_items");
    assert_eq!(t["filters"], json!([{ "column": "order_id", "op": "eq", "value": order }, { "column": "line_no", "op": "eq", "value": line }]));
    let d = read(&mut s);
    assert_eq!((cell(&d, 0, "order_id"), cell(&d, 0, "line_no")), (Some(order.as_str()), Some(line.as_str())));
}

#[test]
fn follow_null_reference_is_an_error() {
    let _db = require_testdb!();
    let (parent, child) = (unique("t_fk_parent"), unique("t_fk_child"));
    let _drop = Scratch(vec![child.clone(), parent.clone()]);
    dbee_testkit::psql(
        pg14(),
        &format!(
            "CREATE TABLE scratch.{parent} (id int PRIMARY KEY); INSERT INTO scratch.{parent} VALUES (1);
             CREATE TABLE scratch.{child} (id int PRIMARY KEY, parent_id int REFERENCES scratch.{parent});
             INSERT INTO scratch.{child} VALUES (1, NULL), (2, 1);"
        ),
    )
    .unwrap();
    let mut s = connected();
    open_table(&mut s, "scratch", &child);
    let e = s.execute("table.followForeignKey", json!({ "row": 0, "column": "parent_id" })).unwrap_err();
    assert_eq!(e.code(), "badParams");
    assert!(e.to_string().contains("no reference: the value is NULL"), "{e}");
    assert_eq!(s.state().tabs.len(), 1);
    let r = run(&mut s, "table.followForeignKey", json!({ "row": 1, "column": "parent_id" }));
    assert_eq!(r["rows"], 1);
}

#[test]
fn next_goes_on_from_the_page_asked_for() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    // The page shown is the only page of its filter …
    let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "le", "value": "50" }] }));
    assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(50), Some(false)));
    // … but a wider filter is on its way, so "next" is not at the end.
    let wider = json!({ "filters": [{ "column": "id", "op": "le", "value": "500" }] });
    let Outcome::Started(job) = s.execute("table.setFilters", wider).unwrap() else { panic!("a job") };
    let r = page(&mut s, json!("next"));
    assert_eq!((r["page"].as_u64(), r["hasNext"].as_bool()), (Some(1), Some(true)));
    assert_eq!(first_id(&mut s), 101);
    assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }));
}

#[test]
fn next_stops_at_the_last_counted_page() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    assert_eq!(run(&mut s, "table.count", json!({}))["total"], json!({ "exact": 1_000_000 }));
    assert_eq!(page(&mut s, json!(9999))["page"], 9998);
    // The first "next" asks for the last page; the second, before it shows, stays there.
    let Outcome::Started(a) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
    let Outcome::Started(b) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
    assert_eq!(s.inspect()["table"]["requestedPage"], 9999);
    let r = s.wait(b, T).unwrap();
    assert_eq!((r["page"].as_u64(), r["rows"].as_u64()), (Some(9999), Some(100)), "{r}");
    assert_eq!(s.wait(a, T).unwrap(), json!({ "ignored": true }));
    assert_eq!(first_id(&mut s), 999_901);
}

#[test]
fn stale_page_results_are_ignored() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    let Outcome::Started(a) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
    // Until it arrives, the rows and their page are the ones from before.
    let d = read(&mut s);
    assert_eq!((d["page"].as_u64(), d["requestedPage"].as_u64(), cell(&d, 0, "id")), (Some(0), Some(1), Some("1")));
    assert_eq!((s.inspect()["table"]["page"].as_u64(), s.inspect()["table"]["requestedPage"].as_u64()), (Some(0), Some(1)));
    let Outcome::Started(b) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
    assert!(s.wait_idle(T));
    let results: Vec<_> = s.pump();
    let of = |job| results.iter().find(|(j, _)| *j == job).map(|(_, r)| r.clone()).unwrap();
    assert_eq!(of(a).unwrap(), json!({ "ignored": true }));
    assert_eq!(of(b).unwrap()["page"], 2);
    assert_eq!(read(&mut s)["page"], 2);
    assert_eq!(first_id(&mut s), 201);
    // A page whose tab closed meanwhile is ignored too.
    let Outcome::Started(c) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
    run(&mut s, "tab.close", json!({}));
    assert_eq!(s.wait(c, T).unwrap(), json!({ "ignored": true }));
    assert!(s.state().tabs.is_empty());
}

#[test]
fn views_and_keyless_tables_are_read_only() {
    let _db = require_testdb!();
    {
        let mut s = connected();
        open_table(&mut s, "shop", "order_totals");
        let d = read(&mut s);
        assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(false), Some("views are read-only")));
        open_table(&mut s, "shop", "customer_counts");
        assert_eq!(read(&mut s)["notEditableReason"], "views are read-only");
        open_table(&mut s, "shop", "no_key");
        let d = read(&mut s);
        assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(false), Some("no primary key or unique not-null key")));
        open_table(&mut s, "shop", "events");
        assert_eq!(read(&mut s)["notEditableReason"], "no primary key or unique not-null key");
        open_table(&mut s, "shop", "customers");
        assert_eq!(read(&mut s)["editable"], true);
        assert_eq!(s.inspect()["table"]["editable"], true);
    }
    let mut s = session();
    let config = dbee_config::ConnectionConfig { read_only: true, ..pg14_config() };
    open(&mut s, json!({ "config": config, "password": pw() }));
    open_table(&mut s, "shop", "customers");
    let d = read(&mut s);
    assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(false), Some("read-only connection")));
}

#[test]
fn reconnect_forgets_the_loads_and_counts_it_cut_off() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let slow = json!({ "raw": "(SELECT true FROM pg_sleep(2))", "rawMode": true });
    let Outcome::Started(load) = s.execute("table.setFilters", slow).unwrap() else { panic!("a job") };
    let Outcome::Started(count) = s.execute("table.count", json!({})).unwrap() else { panic!("a job") };
    let t = s.inspect()["table"].clone();
    assert_eq!((t["loading"].as_u64(), &t["total"]), (Some(load.0), &json!({ "counting": { "estimate": null } })));
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "connection.reconnect", json!({}));
    // Both jobs ran on the connection the reconnect let go: nothing is loading or counting any
    // more, the tab is back to the rows it shows, and there is nothing to cancel.
    let t = s.inspect()["table"].clone();
    assert_eq!((&t["loading"], &t["counting"], &t["total"], &t["applied"]), (&Json::Null, &Json::Null, &json!("unknown"), &json!("none")));
    assert_eq!(run(&mut s, "table.cancel", json!({})), json!({ "sent": false }));
    assert_eq!(s.wait(load, T).unwrap(), json!({ "ignored": true }));
    assert_eq!(s.wait(count, T).unwrap(), json!({ "ignored": true }));
    assert_eq!((&s.inspect()["table"]["loading"], &s.inspect()["table"]["total"]), (&Json::Null, &json!("unknown")));
    assert_eq!(page(&mut s, json!("next"))["page"], 1);
}

#[test]
fn a_failed_count_leaves_a_newer_count_alone() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    run(&mut s, "table.setFilters", json!({ "raw": "(SELECT true FROM pg_sleep(2))", "rawMode": true }));
    // `last` counts first; its count is cancelled while a newer count waits behind it.
    let Outcome::Started(last) = s.execute("table.page", json!({ "to": "last" })).unwrap() else { panic!("a job") };
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(run(&mut s, "table.cancel", json!({})), json!({ "sent": true }));
    let Outcome::Started(count) = s.execute("table.count", json!({})).unwrap() else { panic!("a job") };
    assert_eq!(s.wait(last, T).unwrap_err().code(), "cancelled");
    let t = s.inspect()["table"].clone();
    assert_eq!((&t["total"], t["counting"].as_u64()), (&json!({ "counting": { "estimate": null } }), Some(count.0)));
    assert_eq!(s.wait(count, T).unwrap()["total"], json!({ "exact": 5000 }));
    assert_eq!(s.inspect()["table"]["counting"], Json::Null);
}

#[test]
fn typed_filters_survive_a_lost_connection() {
    let _db = require_testdb!();
    let mut s = connected_exclusive();
    open_table(&mut s, "shop", "orders");
    kill_dbee_backends(&s, pg14());
    let typed = json!([{ "column": "id", "op": "eq", "value": "5" }]);
    let e = run_err(&mut s, "table.setFilters", json!({ "filters": typed }));
    assert_eq!(e.code(), "connectionLost", "{e}");
    assert_eq!(s.inspect()["connection"]["lost"], true);
    assert_eq!(s.inspect()["table"]["filters"], typed);
    run(&mut s, "connection.reconnect", json!({}));
    assert_eq!(run(&mut s, "table.setFilters", json!({}))["rows"], 1);
}

#[test]
fn page_after_backend_killed_reports_connection_lost() {
    let _db = require_testdb!();
    let mut s = connected_exclusive();
    open_table(&mut s, "shop", "orders");
    kill_dbee_backends(&s, pg14());
    let e = run_err(&mut s, "table.page", json!({ "to": "next" }));
    assert_eq!(e.code(), "connectionLost", "{e}");
    assert_eq!(e.data()["action"], "connection.reconnect");
    assert_eq!(s.inspect()["connection"]["lost"], true);
    assert_eq!(s.execute("table.page", json!({ "to": "next" })).unwrap_err().code(), "disabled");
    run(&mut s, "connection.reconnect", json!({}));
    // The rows from before stay, and paging goes on from the page they are.
    let r = page(&mut s, json!("next"));
    assert_eq!(r["page"], 1);
    assert_eq!(first_id(&mut s), 101);
}

#[test]
fn odd_identifiers_open_filter_sort_follow() {
    let _db = require_testdb!();
    let mut s = connected();
    let name = "Odd \"Name\" 🐝";
    assert_eq!(open_table(&mut s, "odd", name)["rows"], 4);
    let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "Total $", "op": "gt", "value": "1" }] }));
    assert_eq!(r["rows"], 2);
    run(&mut s, "table.sort", json!({ "column": "lower UPPER", "dir": "asc" }));
    let asc: Vec<String> = {
        let d = read(&mut s);
        (0..rows(&d)).map(|i| cell(&d, i, "lower UPPER").unwrap().to_owned()).collect()
    };
    run(&mut s, "table.sort", json!({ "column": "lower UPPER", "dir": "desc" }));
    let d = read(&mut s);
    let desc: Vec<String> = (0..rows(&d)).map(|i| cell(&d, i, "lower UPPER").unwrap().to_owned()).collect();
    assert_eq!(desc.len(), 2);
    assert_eq!(desc, asc.iter().rev().cloned().collect::<Vec<_>>());
    let target = cell(&d, 0, "Other Ödd id").unwrap().to_owned();
    let r = run(&mut s, "table.followForeignKey", json!({ "row": 0, "column": "Other Ödd id" }));
    assert_eq!(r["rows"], 1);
    let t = &s.inspect()["table"];
    assert_eq!((t["schema"].as_str(), t["name"].as_str()), (Some("odd"), Some("Other Ödd")));
    assert_eq!(cell(&read(&mut s), 0, "id"), Some(target.as_str()));
    run(&mut s, "table.sort", json!({ "column": "Label", "dir": "desc" }));
    run(&mut s, "table.count", json!({}));
}

#[test]
fn json_string_keeps_its_quotes() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let d = read(&mut s);
    let meta = cell(&d, 0, "meta").unwrap();
    assert!(meta.starts_with('"'), "{meta}");
    assert_eq!(d["columns"][col(&d, "meta")]["baseType"], "json");
}

#[test]
fn twenty_megabyte_cell_loads() {
    let _db = require_testdb!();
    let mut s = connected();
    let started = Instant::now();
    assert_eq!(open_table(&mut s, "shop", "big_cell")["rows"], 1);
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let d = read(&mut s);
    assert_eq!(cell(&d, 0, "t").unwrap().chars().count(), 20_000_000);
}

#[test]
fn a_page_cut_by_the_byte_cap_says_so() {
    let _db = require_testdb!();
    let t = unique("t_page_cap");
    let _drop = Scratch(vec![t.clone()]);
    // 14 values of 20,000,000 characters: 280 MB, past a page's 256 MiB of cell text.
    dbee_testkit::psql(
        pg14(),
        &format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, v text); INSERT INTO scratch.{t} SELECT g, repeat('x', 20000000) FROM generate_series(1, 14) AS g;"),
    )
    .unwrap();
    let mut s = connected();
    // 13 values fit (260 MB); the 14th is read but not kept.
    assert_eq!(open_table(&mut s, "scratch", &t)["rows"], 13);
    assert_eq!(s.inspect()["table"]["pageComplete"], false, "the page was cut");
    let d = run(&mut s, "table.read", json!({ "limit": 0 }));
    assert_eq!((d["pageComplete"].as_bool(), d["hasNext"].as_bool()), (Some(false), Some(false)));
    // A page under the cap is whole, and so is a full page followed by more rows.
    open_table(&mut s, "shop", "orders");
    assert_eq!(s.inspect()["table"]["pageComplete"], true);
    assert_eq!(run(&mut s, "table.read", json!({ "limit": 0 }))["pageComplete"], true);
}

#[test]
fn cancel_stops_a_slow_page() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    // Nothing runs: nothing to cancel.
    assert_eq!(run(&mut s, "table.cancel", json!({})), json!({ "sent": false }));
    let slow = json!({ "raw": "(SELECT true FROM pg_sleep(30))", "rawMode": true });
    let Outcome::Started(job) = s.execute("table.setFilters", slow).unwrap() else { panic!("a job") };
    std::thread::sleep(Duration::from_millis(300));
    // A cancel that arrives before the statement starts cancels nothing, so it is sent again
    // until the job ends.
    let started = Instant::now();
    let e = loop {
        assert_eq!(run(&mut s, "table.cancel", json!({})), json!({ "sent": true }));
        match s.wait(job, Duration::from_millis(500)) {
            Err(EngineError::Timeout) => assert!(started.elapsed() < Duration::from_secs(10), "the page was not cancelled"),
            other => break other.unwrap_err(),
        }
    };
    assert_eq!(e.code(), "cancelled", "{e}");
    // The rows from before stay, and the connection works.
    assert_eq!(rows(&read(&mut s)), 100);
    assert_eq!(s.inspect()["table"]["applied"], "none");
    assert_eq!(page(&mut s, json!("next"))["page"], 1);
}

#[test]
fn table_commands_need_a_table_tab() {
    let _db = require_testdb!();
    let mut s = session();
    assert_eq!(s.execute("table.open", json!({ "schema": "shop", "table": "orders" })).unwrap_err().code(), "disabled");
    drop(s);
    let mut s = connected();
    s.execute("tab.newQuery", json!({})).unwrap();
    for id in ["table.setFilters", "table.page", "table.sort", "table.count", "table.cancel", "table.refresh", "table.read", "table.followForeignKey"]
    {
        let e = s.execute(id, json!({ "to": "next", "column": "id" })).unwrap_err();
        assert_eq!((e.code(), e.data()["why"].as_str()), ("disabled", Some("no table tab is active")), "{id}");
    }
}

#[test]
fn browses_on_every_server() {
    let db = require_testdb!();
    for server in &db.servers {
        let mut s = session();
        open(&mut s, json!({ "config": server_config(server), "password": pw() }));
        open_table(&mut s, "shop", "orders");
        let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "status", "op": "in", "value": "paid, completed" }] }));
        assert_eq!(r["rows"], 100, "{}", server.name);
        let n: u64 =
            dbee_testkit::psql(server, "SELECT count(*) FROM shop.orders WHERE status IN ('paid', 'completed')").unwrap().trim().parse().unwrap();
        assert_eq!(page(&mut s, json!("last"))["total"], json!({ "exact": n }), "{}", server.name);
        assert_eq!(read(&mut s)["rows"].as_array().map(Vec::len), Some((n - 1) as usize % 100 + 1), "{}", server.name);
        run(&mut s, "table.followForeignKey", json!({ "row": 0, "column": "customer_id" }));
        assert_eq!(read(&mut s)["rows"].as_array().map(Vec::len), Some(1), "{}", server.name);
    }
}

#[test]
#[ignore]
fn next_page_on_a_million_rows_is_fast() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "big_rows");
    let mut times: Vec<Duration> = (0..20)
        .map(|_| {
            let started = Instant::now();
            page(&mut s, json!("next"));
            started.elapsed()
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    println!("next page on shop.big_rows: median {median:?} over 20 loads (min {:?}, max {:?})", times[0], times[times.len() - 1]);
    assert!(median < Duration::from_millis(200), "median {median:?}");
}

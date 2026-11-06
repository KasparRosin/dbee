//! Table tabs: open, filters, paging, sort, count, cancel, refresh, read and following foreign
//! keys, on every engine ([`for_each_engine`]). Needs the Docker test servers (`cargo xtask testdb
//! up`); the seed tables are read-only, and the tests that write make their own tables in
//! `scratch`.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_core::TableRef;
use dbee_engine::{Json, Outcome, Session};
use dbee_testkit::{Held, TestTarget, require_testdb, unique};
use serde_json::json;

fn first_id(s: &mut Session) -> i64 {
    let r = read(s);
    cell(&r, 0, "id").unwrap().parse().unwrap()
}

fn page(s: &mut Session, to: Json) -> Json {
    run(s, "table.page", json!({ "to": to }))
}

/// The 0-based last page of `rows` rows, 100 a page.
fn last_page(rows: u64) -> u64 {
    (rows - 1) / 100
}

/// What `payload` sorts last in `big_rows`: `payload <n>` for the n that sorts last as text.
fn last_payload(rows: u64) -> String {
    let n = rows.to_string();
    format!("payload {}", ("9".repeat(n.len() - 1)).max(n))
}

/// A raw condition that holds for every row and, on a server, waits while [`hold_gate`] holds the
/// lock `key`: a load or count with it cannot end by itself meanwhile. A file has no other session
/// to hold one: its condition takes about 2 s instead.
fn gated(t: &TestTarget, key: &str) -> String {
    if t.is_file() { t.slow_condition_sql(2) } else { t.waits_for_lock_sql(key) }
}

/// Holds the lock that [`gated`] waits for, until dropped; nothing on a file.
fn hold_gate(t: &TestTarget, key: &str) -> Option<Held> {
    t.server().map(|_| t.hold_lock(key).unwrap())
}

#[test]
fn opens_first_page_of_100() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let r = s.run_blocking("table.open", json!({ "schema": t.schema("shop"), "table": "orders" }), T).unwrap();
        assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(100), Some(true)));
        assert_eq!(s.inspect()["active"], r["tab"]);
        assert_eq!(s.inspect()["table"]["name"], "orders");
        let d = read(s);
        assert_eq!((rows(&d), d["page"].as_u64(), d["hasNext"].as_bool()), (100, Some(0), Some(true)));
        // The columns come from the catalog, with their types; rows in primary key order.
        let info = s
            .state()
            .connected
            .as_ref()
            .unwrap()
            .catalog
            .table(&TableRef { schema: t.schema("shop").into(), name: "orders".into() })
            .cloned()
            .unwrap();
        let typed: Vec<Json> = info.visible_columns().map(|c| json!(c.column())).collect();
        assert_eq!(d["columns"].as_array(), Some(&typed), "{}", t.name());
        assert_eq!((&d["columns"][0]["name"], &d["columns"][0]["class"]), (&json!("id"), &json!("number")));
        assert_eq!((cell(&d, 0, "id"), cell(&d, 99, "id")), (Some("1"), Some("100")));
        assert_eq!(d["total"], "unknown");
        assert_eq!((d["editable"].as_bool(), &d["notEditableReason"]), (Some(true), &Json::Null));
        // A window of the loaded rows.
        let w = run(s, "table.read", json!({ "offset": 98, "limit": 5 }));
        assert_eq!((rows(&w), cell(&w, 0, "id")), (2, Some("99")));
        assert_eq!(rows(&run(s, "table.read", json!({ "offset": 1000 }))), 0);
        // A table the catalog does not have.
        assert_eq!(s.execute("table.open", json!({ "schema": t.schema("shop"), "table": "nope" })).unwrap_err().code(), "badParams");
        assert_eq!(s.state().tabs.len(), 1);
    });
}

/// Postgres types: the catalog's own names reach the page's columns.
#[test]
fn page_columns_carry_postgres_type_names() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let d = read(&mut s);
    assert_eq!(d["columns"][0], json!({ "name": "id", "baseType": "int8", "typeLabel": "bigint", "class": "number" }));
    assert_eq!(d["columns"][col(&d, "status")]["typeLabel"], "shop.order_status");
}

#[test]
fn day_scenario_filters() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        let filters = json!([
            { "column": "dealer_id", "op": "eq", "value": "42" },
            { "column": "status", "op": "eq", "value": "completed" },
            { "column": "total", "op": "gt", "value": "100" },
        ]);
        let r = run(s, "table.setFilters", json!({ "filters": filters }));
        assert!(r["rows"].as_u64().unwrap() >= 1, "{r}");
        let d = read(s);
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
        let bar = run(s, "table.setFilters", json!({ "raw": "id < 3", "rawMode": true, "apply": false }));
        assert_eq!((bar["raw"].as_str(), bar["rawMode"].as_bool()), (Some("id < 3"), Some(true)));
        assert_eq!(s.inspect()["table"]["applied"]["filters"], filters);
        assert_eq!(rows(&read(s)), rows(&d));
        // Applying the raw condition.
        let r = run(s, "table.setFilters", json!({}));
        assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(2), Some(false)));
        assert_eq!(s.inspect()["table"]["applied"], json!({ "raw": "id < 3" }));
        // Back to no filter at all.
        let r = run(s, "table.setFilters", json!({ "filters": [], "rawMode": false }));
        assert_eq!(r["rows"], 100);
    });
}

#[test]
fn filters_on_mysql() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "customers");
        let found = |s: &mut TestSession, op: &str, value: &str| -> Vec<String> {
            run(s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": op, "value": value }] }));
            let d = read(s);
            (0..rows(&d)).map(|i| cell(&d, i, "name").unwrap().to_owned()).collect()
        };
        // ilike ignores case on every engine.
        assert_eq!(found(s, "ilike", "CUSTOMER 7"), ["Customer 7"], "{}", t.name());
        assert_eq!(found(s, "notIlike", "customer %").len(), 0, "{}", t.name());
        // eq and like keep the engine's own meaning: they ignore case where the dialect's note on
        // the operator says so (MySQL's case-insensitive collations).
        let dialect = s.state().dialect();
        let folding = |op| if dialect.op_note(op).is_some() { vec!["Customer 7"] } else { vec![] };
        assert_eq!(found(s, "eq", "customer 7"), folding(dbee_sql::filter::Op::Eq), "{}", t.name());
        assert_eq!(found(s, "like", "CUSTOMER 7"), folding(dbee_sql::filter::Op::Like), "{}", t.name());
        assert_eq!(found(s, "eq", "Customer 7"), ["Customer 7"], "{}", t.name());
    });
}

/// The filter bar's `like`, `not like`, `ilike` and `not ilike` take `\` as their escape on every
/// engine, as Postgres does: `\%`, `\_` and `\\` match `%`, `_` and `\` themselves.
#[test]
fn like_escapes_with_a_backslash_on_every_engine() {
    let _db = require_testdb!();
    let values = ["50%", "500", "a_b", "axb", "a\\b", "ab"];
    let mut wrong = Vec::new();
    for t in engine_targets() {
        let name = unique("t_like");
        let ids: Vec<String> = (1..=values.len()).map(|i| format!("({i}, 'x')")).collect();
        let _drop =
            scratch_table(&t, &name, &format!("CREATE TABLE {{t}} (id int PRIMARY KEY, v varchar(20)); INSERT INTO {{t}} VALUES {}", ids.join(", ")));
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("scratch"), &name);
        // Written by dbee, which writes a backslash the same on every engine.
        for (row, v) in values.iter().enumerate() {
            set_cell(&mut s, row, "v", *v);
        }
        apply(&mut s);
        let mut found = |op: &str, value: &str| -> Vec<String> {
            run(&mut s, "table.setFilters", json!({ "filters": [{ "column": "v", "op": op, "value": value }] }));
            let d = read(&mut s);
            (0..rows(&d)).map(|i| cell(&d, i, "v").unwrap_or_default().to_owned()).collect()
        };
        let checks: [(&str, &str, &[&str]); 6] = [
            ("like", "50\\%", &["50%"]),
            ("like", "a\\_b", &["a_b"]),
            ("like", "a\\\\b", &["a\\b"]),
            ("notLike", "50\\%", &["500", "a_b", "axb", "a\\b", "ab"]),
            ("ilike", "A\\_B", &["a_b"]),
            ("notIlike", "%\\_%", &["50%", "500", "axb", "a\\b", "ab"]),
        ];
        for (op, value, want) in checks {
            let got = found(op, value);
            println!("{}: {op} {value} finds {got:?}", t.name());
            if got != want {
                wrong.push(format!("{}: {op} {value} found {got:?}, not {want:?}", t.name()));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn typing_in_the_filter_bar_journals_one_entry() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "customers");
        for value in ["C", "Cu", "Customer 7"] {
            run(s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": value }], "apply": false }));
        }
        run(s, "table.setFilters", json!({ "raw": "id < 3", "apply": false }));
        run(s, "table.setFilters", json!({ "apply": true }));
        let tail: Vec<(String, Json)> = s.journal().iter().rev().take(4).rev().cloned().collect();
        let expected = vec![
            ("table.open".to_string(), json!({ "schema": t.schema("shop"), "table": "customers" })),
            ("table.setFilters".to_string(), json!({ "filters": [{ "column": "name", "op": "eq", "value": "Customer 7" }], "apply": false })),
            ("table.setFilters".to_string(), json!({ "raw": "id < 3", "apply": false })),
            ("table.setFilters".to_string(), json!({ "apply": true })),
        ];
        assert_eq!(tail, expected);
    });
}

#[test]
fn hostile_filter_value_is_just_a_value() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "customers");
        let shop = t.schema("shop");
        for evil in [format!("x'); DROP TABLE {shop}.customers; --"), format!("x\\'); DROP TABLE {shop}.customers; -- \\")] {
            let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": evil }] }));
            assert_eq!(r["rows"], 0, "{}: {evil}", t.name());
            let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "like", "value": evil }] }));
            assert_eq!(r["rows"], 0, "{}: {evil}", t.name());
        }
        assert_eq!(q(t, &format!("SELECT count(*) FROM {shop}.customers")), "500");
    });
}

#[test]
fn bad_condition_sets_tab_error_and_keeps_tab() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        let e = run_err(s, "table.setFilters", json!({ "raw": "nope = 1", "rawMode": true }));
        assert_eq!(e.code(), "server", "{}: {e}", t.name());
        let tab = s.inspect()["table"].clone();
        assert!(tab["error"].as_str().unwrap().contains("nope"), "{}: {tab}", t.name());
        assert_eq!(s.state().tabs.len(), 1);
        // The bar keeps what was typed; the rows and what they were loaded with stay.
        assert_eq!((tab["raw"].as_str(), tab["rawMode"].as_bool()), (Some("nope = 1"), Some(true)));
        assert_eq!(tab["applied"], "none");
        let d = read(s);
        assert_eq!((rows(&d), cell(&d, 0, "id")), (100, Some("1")));
        assert!(d["error"].as_str().unwrap().contains("nope"));
        // A good filter clears the error.
        run(s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "7" }], "rawMode": false }));
        assert_eq!(s.inspect()["table"]["error"], Json::Null);
        // SQL dbee refuses to build is `badParams`, and changes nothing.
        for bad in [
            json!({ "filters": [{ "column": "nope", "op": "eq", "value": "1" }] }),
            json!({ "filters": [{ "column": "id", "op": "between", "value": "1" }] }),
            json!({ "filters": [{ "column": "id", "op": "in", "value": " , " }] }),
            json!({ "raw": format!("true; drop table {}.orders", t.schema("shop")), "rawMode": true }),
            json!({ "filters": "id = 1" }),
        ] {
            let e = s.execute("table.setFilters", bad.clone()).unwrap_err();
            assert_eq!(e.code(), "badParams", "{bad}: {e}");
        }
        assert_eq!(s.inspect()["table"]["filters"][0]["value"], "7");
        assert_eq!(s.inspect()["table"]["rawMode"], false);
    });
}

/// Postgres types: a value its column's type cannot take is a server error (MySQL compares such a
/// value loosely, so the test above uses a condition no engine takes).
#[test]
fn a_value_the_type_refuses_sets_the_tab_error() {
    let _db = require_testdb!();
    let mut s = connected();
    open_table(&mut s, "shop", "orders");
    let e = run_err(&mut s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "abc" }] }));
    assert_eq!(e.code(), "server", "{e}");
    let t = s.inspect()["table"].clone();
    assert!(t["error"].as_str().unwrap().contains("bigint"), "{t}");
    assert_eq!(t["filters"][0]["value"], "abc");
    assert_eq!(t["applied"], "none");
    assert_eq!(rows(&read(&mut s)), 100);
}

#[test]
fn paging_on_a_million_rows() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let n = t.big_rows();
        let last = last_page(n);
        open_table(s, t.schema("shop"), "big_rows");
        let r = page(s, json!("next"));
        assert_eq!((r["page"].as_u64(), r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(1), Some(100), Some(true)));
        assert_eq!(first_id(s), 101);
        let r = page(s, json!("last"));
        assert_eq!(r["total"], json!({ "exact": n }));
        let d = read(s);
        assert_eq!((d["page"].as_u64(), rows(&d), d["hasNext"].as_bool()), (Some(last), 100, Some(false)));
        assert_eq!(d["total"], json!({ "exact": n }));
        assert_eq!(cell(&d, 99, "id"), Some(n.to_string().as_str()));
        // Next on the last page stays there.
        assert_eq!(page(s, json!("next"))["page"], last);
        assert_eq!(first_id(s), n as i64 - 99);
        page(s, json!("prev"));
        assert_eq!(read(s)["page"], last - 1);
        assert_eq!(first_id(s), n as i64 - 199);
        // Page numbers count from 1.
        page(s, json!(3));
        assert_eq!((read(s)["page"].as_u64(), first_id(s)), (Some(2), 201));
        page(s, json!("first"));
        assert_eq!((read(s)["page"].as_u64(), first_id(s)), (Some(0), 1));
        // Prev on the first page stays there.
        page(s, json!("prev"));
        assert_eq!(read(s)["page"], 0);
        for bad in [json!(0), json!(-1), json!("middle"), json!(1.5), json!(u64::MAX), Json::Null] {
            assert_eq!(s.execute("table.page", json!({ "to": bad })).unwrap_err().code(), "badParams", "{bad}");
        }
        // A refresh reloads the page.
        let r = run(s, "table.refresh", json!({}));
        assert_eq!((r["page"].as_u64(), r["rows"].as_u64()), (Some(0), Some(100)));
    });
}

#[test]
fn sort_desc_then_none() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let n = t.big_rows();
        open_table(s, t.schema("shop"), "big_rows");
        page(s, json!("next"));
        run(s, "table.sort", json!({ "column": "id", "dir": "desc" }));
        assert_eq!(first_id(s), n as i64);
        // Sorting goes back to the first page.
        assert_eq!(read(s)["page"], 0);
        assert_eq!(s.inspect()["table"]["sort"], json!({ "column": "id", "dir": "desc" }));
        run(s, "table.sort", json!({ "column": "id", "dir": "none" }));
        assert_eq!(first_id(s), 1);
        assert_eq!(s.inspect()["table"]["sort"], Json::Null);
        // Without `dir` it cycles like a header click: asc, desc, none.
        run(s, "table.sort", json!({ "column": "payload" }));
        assert_eq!(s.inspect()["table"]["sort"]["dir"], "asc");
        assert_eq!(cell(&read(s), 0, "payload"), Some("payload 1"));
        run(s, "table.sort", json!({ "column": "payload" }));
        assert_eq!(s.inspect()["table"]["sort"]["dir"], "desc");
        assert_eq!(cell(&read(s), 0, "payload"), Some(last_payload(n).as_str()), "{}", t.name());
        run(s, "table.sort", json!({ "column": "payload" }));
        assert_eq!(s.inspect()["table"]["sort"], Json::Null);
        assert_eq!(s.execute("table.sort", json!({ "column": "nope" })).unwrap_err().code(), "badParams");
        assert_eq!(s.execute("table.sort", json!({ "column": "id", "dir": "up" })).unwrap_err().code(), "badParams");
    });
}

#[test]
fn count_estimate_then_exact() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let n = t.big_rows();
        open_table(s, t.schema("shop"), "big_rows");
        let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
        let total = s.inspect()["table"]["total"].clone();
        let estimate = total.get("estimate").or_else(|| total.get("counting").and_then(|c| c.get("estimate"))).and_then(Json::as_u64);
        assert!(estimate.is_some_and(|e| e >= n * 9 / 10), "{}: {total}", t.name());
        let r = s.wait(job, T).unwrap();
        assert_eq!(r["total"], json!({ "exact": n }));
        assert_eq!(read(s)["total"], json!({ "exact": n }));
        // Filtered: no estimate, and a new filter forgets the count.
        run(s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "le", "value": "10" }] }));
        assert_eq!(s.inspect()["table"]["total"], "unknown");
        let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
        assert_eq!(s.inspect()["table"]["total"], json!({ "counting": { "estimate": null } }));
        assert_eq!(s.wait(job, T).unwrap()["total"], json!({ "exact": 10 }));
        // A view has no estimate.
        open_table(s, t.schema("shop"), "order_totals");
        let Outcome::Started(job) = s.execute("table.count", json!({})).unwrap() else { panic!("table.count starts a job") };
        assert_eq!(s.inspect()["table"]["total"], json!({ "counting": { "estimate": null } }));
        assert_eq!(s.wait(job, T).unwrap()["total"], json!({ "exact": 500 }));
    });
}

#[test]
fn follow_foreign_key() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let orders = open_table(s, t.schema("shop"), "orders")["tab"].clone();
        let c = cell(&read(s), 0, "customer_id").unwrap().to_owned();
        let r = run(s, "table.followForeignKey", json!({ "row": 0, "column": "customer_id" }));
        assert_eq!(r["rows"], 1);
        let i = s.inspect();
        assert_ne!(i["active"], orders);
        assert_eq!(i["active"], r["tab"]);
        assert_eq!((i["table"]["schema"].as_str(), i["table"]["name"].as_str()), (Some(t.schema("shop")), Some("customers")));
        assert_eq!(i["table"]["filters"], json!([{ "column": "id", "op": "eq", "value": c }]));
        assert_eq!(cell(&read(s), 0, "id"), Some(c.as_str()));
        assert_eq!(s.state().tabs.len(), 2);
        // A column by its index works too; a column outside every foreign key does not.
        run(s, "tab.activate", json!({ "tab": orders }));
        run(s, "table.followForeignKey", json!({ "row": 1, "column": 1 }));
        assert_eq!(s.state().tabs.len(), 3);
        run(s, "tab.activate", json!({ "tab": orders }));
        for bad in [json!({ "row": 0, "column": "total" }), json!({ "row": 100, "column": "customer_id" }), json!({ "row": 0, "column": 99 })] {
            assert_eq!(s.execute("table.followForeignKey", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
        }
    });
}

#[test]
fn follow_composite_foreign_key() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "shipments");
        let d = read(s);
        let (order, line) = (cell(&d, 0, "order_id").unwrap().to_owned(), cell(&d, 0, "line_no").unwrap().to_owned());
        let r = run(s, "table.followForeignKey", json!({ "row": 0, "column": "line_no" }));
        assert_eq!(r["rows"], 1);
        let t = &s.inspect()["table"];
        assert_eq!(t["name"], "order_items");
        assert_eq!(t["filters"], json!([{ "column": "order_id", "op": "eq", "value": order }, { "column": "line_no", "op": "eq", "value": line }]));
        let d = read(s);
        assert_eq!((cell(&d, 0, "order_id"), cell(&d, 0, "line_no")), (Some(order.as_str()), Some(line.as_str())));
    });
}

#[test]
fn follow_null_reference_is_an_error() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (sc, parent, child) = (t.schema("scratch"), unique("t_fk_parent"), unique("t_fk_child"));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {sc}.{child}"), format!("TABLE IF EXISTS {sc}.{parent}")]);
        // A foreign key names its table without a schema where the engine takes none in it.
        let referenced = if t.kind().info().one_schema.is_some() { parent.clone() } else { format!("{sc}.{parent}") };
        q(
            &t,
            &format!(
                "CREATE TABLE {sc}.{parent} (id int PRIMARY KEY); INSERT INTO {sc}.{parent} VALUES (1);
                 CREATE TABLE {sc}.{child} (id int PRIMARY KEY, parent_id int, FOREIGN KEY (parent_id) REFERENCES {referenced} (id));
                 INSERT INTO {sc}.{child} VALUES (1, NULL), (2, 1);"
            ),
        );
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("scratch"), &child);
        let e = s.execute("table.followForeignKey", json!({ "row": 0, "column": "parent_id" })).unwrap_err();
        assert_eq!(e.code(), "badParams");
        assert!(e.to_string().contains("no reference: the value is NULL"), "{e}");
        assert_eq!(s.state().tabs.len(), 1);
        let r = run(&mut s, "table.followForeignKey", json!({ "row": 1, "column": "parent_id" }));
        assert_eq!(r["rows"], 1, "{}", t.name());
    }
}

#[test]
fn next_goes_on_from_the_page_asked_for() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "big_rows");
        // The page shown is the only page of its filter …
        let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "id", "op": "le", "value": "50" }] }));
        assert_eq!((r["rows"].as_u64(), r["hasNext"].as_bool()), (Some(50), Some(false)));
        // … but a wider filter is on its way, so "next" is not at the end.
        let wider = json!({ "filters": [{ "column": "id", "op": "le", "value": "500" }] });
        let Outcome::Started(job) = s.execute("table.setFilters", wider).unwrap() else { panic!("a job") };
        let r = page(s, json!("next"));
        assert_eq!((r["page"].as_u64(), r["hasNext"].as_bool()), (Some(1), Some(true)));
        assert_eq!(first_id(s), 101);
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }));
    });
}

#[test]
fn next_stops_at_the_last_counted_page() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let n = t.big_rows();
        let last = last_page(n);
        open_table(s, t.schema("shop"), "big_rows");
        assert_eq!(run(s, "table.count", json!({}))["total"], json!({ "exact": n }));
        assert_eq!(page(s, json!(last))["page"], last - 1);
        // The first "next" asks for the last page; the second, before it shows, stays there.
        let Outcome::Started(a) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
        let Outcome::Started(b) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
        assert_eq!(s.inspect()["table"]["requestedPage"], last);
        let r = s.wait(b, T).unwrap();
        assert_eq!((r["page"].as_u64(), r["rows"].as_u64()), (Some(last), Some(100)), "{r}");
        assert_eq!(s.wait(a, T).unwrap(), json!({ "ignored": true }));
        assert_eq!(first_id(s), n as i64 - 99);
    });
}

#[test]
fn stale_page_results_are_ignored() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "big_rows");
        let Outcome::Started(a) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
        // Until it arrives, the rows and their page are the ones from before.
        let d = read(s);
        assert_eq!((d["page"].as_u64(), d["requestedPage"].as_u64(), cell(&d, 0, "id")), (Some(0), Some(1), Some("1")));
        assert_eq!((s.inspect()["table"]["page"].as_u64(), s.inspect()["table"]["requestedPage"].as_u64()), (Some(0), Some(1)));
        let Outcome::Started(b) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
        assert!(s.wait_idle(T));
        let results: Vec<_> = s.pump();
        let of = |job| results.iter().find(|(j, _)| *j == job).map(|(_, r)| r.clone()).unwrap();
        assert_eq!(of(a).unwrap(), json!({ "ignored": true }));
        assert_eq!(of(b).unwrap()["page"], 2);
        assert_eq!(read(s)["page"], 2);
        assert_eq!(first_id(s), 201);
        // A page whose tab closed meanwhile is ignored too.
        let Outcome::Started(c) = s.execute("table.page", json!({ "to": "next" })).unwrap() else { panic!("a job") };
        run(s, "tab.close", json!({}));
        assert_eq!(s.wait(c, T).unwrap(), json!({ "ignored": true }));
        assert!(s.state().tabs.is_empty());
    });
}

#[test]
fn views_and_keyless_tables_are_read_only() {
    let _db = require_testdb!();
    for t in engine_targets() {
        {
            let mut s = connected_to(&t);
            open_table(&mut s, t.schema("shop"), "order_totals");
            let d = read(&mut s);
            assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(false), Some("views are read-only")));
            // A materialized view where the engine has them, else a plain view.
            open_table(&mut s, t.schema("shop"), "customer_counts");
            let why = read(&mut s)["notEditableReason"].as_str().unwrap_or_default().to_owned();
            assert!(["views are read-only", "materialized views are read-only"].contains(&why.as_str()), "{}: {why}", t.name());
            // A table without a key takes no edits, unless the engine keys its rows by a row id.
            let keyless = if t.kind().info().keyless_tables_editable { None } else { Some("no primary key or unique not-null key") };
            open_table(&mut s, t.schema("shop"), "no_key");
            let d = read(&mut s);
            assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(keyless.is_none()), keyless), "{}", t.name());
            open_table(&mut s, t.schema("shop"), "events");
            assert_eq!(read(&mut s)["notEditableReason"].as_str(), keyless);
            open_table(&mut s, t.schema("shop"), "customers");
            assert_eq!(read(&mut s)["editable"], true);
            assert_eq!(s.inspect()["table"]["editable"], true);
        }
        let mut s = session();
        let config = dbee_config::ConnectionConfig { read_only: true, ..config_for(&t) };
        open(&mut s, json!({ "config": config, "password": pw() }));
        open_table(&mut s, t.schema("shop"), "customers");
        let d = read(&mut s);
        assert_eq!((d["editable"].as_bool(), d["notEditableReason"].as_str()), (Some(false), Some("read-only connection")), "{}", t.name());
    }
}

#[test]
fn myisam_table_reports_why_it_is_read_only() {
    let _db = require_testdb!();
    let mut seen = 0;
    for_each_engine(|t, s| {
        // Only the MySQL seed has a table whose engine keeps no transactions.
        let audit = TableRef { schema: "shop".into(), name: "audit_log".into() };
        if s.state().connected.as_ref().unwrap().catalog.table(&audit).is_none() {
            return;
        }
        seen += 1;
        open_table(s, t.schema("shop"), "audit_log");
        let why = summary(s)["noEditReason"].as_str().unwrap_or_default().to_owned();
        assert!(why.starts_with("The MyISAM table engine can't undo a write"), "{}: {}", t.name(), summary(s));
        assert_eq!((&summary(s)["editable"], summary(s)["notEditableReason"].as_str()), (&json!(false), Some(why.as_str())));
        let d = read(s);
        assert_eq!((rows(&d), d["notEditableReason"].as_str()), (3, Some(why.as_str())));
        for (id, params) in [
            ("cell.edit", json!({ "row": 0, "column": "action", "value": "x" })),
            ("cell.setNull", json!({ "row": 0, "column": "detail" })),
            ("row.add", json!({})),
            ("row.delete", json!({ "rows": [0] })),
        ] {
            let e = s.execute(id, params).unwrap_err();
            assert_eq!((e.code(), e.data()["why"].as_str()), ("disabled", Some(why.as_str())), "{}: {id}", t.name());
        }
        // A table that can undo has no such reason.
        open_table(s, t.schema("shop"), "customers");
        assert_eq!(summary(s)["noEditReason"], Json::Null);
    });
    assert!(seen > 0, "the MySQL seed's servers have shop.audit_log");
}

#[test]
fn reconnect_forgets_the_loads_and_counts_it_cut_off() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        let key = unique("dbee_gate");
        let gate = hold_gate(t, &key);
        let Outcome::Started(load) = s.execute("table.setFilters", json!({ "raw": gated(t, &key), "rawMode": true })).unwrap() else {
            panic!("a job")
        };
        let Outcome::Started(count) = s.execute("table.count", json!({})).unwrap() else { panic!("a job") };
        let tab = s.inspect()["table"].clone();
        assert_eq!((tab["loading"].as_u64(), &tab["total"]), (Some(load.0), &json!({ "counting": { "estimate": null } })));
        run(s, "connection.reconnect", json!({}));
        // Both jobs ran on the connection the reconnect let go: nothing is loading or counting any
        // more, the tab is back to the rows it shows, and there is nothing to cancel.
        let tab = s.inspect()["table"].clone();
        assert_eq!(
            (&tab["loading"], &tab["counting"], &tab["total"], &tab["applied"]),
            (&Json::Null, &Json::Null, &json!("unknown"), &json!("none"))
        );
        assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": false }));
        drop(gate);
        assert_eq!(s.wait(load, T).unwrap(), json!({ "ignored": true }));
        assert_eq!(s.wait(count, T).unwrap(), json!({ "ignored": true }));
        assert_eq!((&s.inspect()["table"]["loading"], &s.inspect()["table"]["total"]), (&Json::Null, &json!("unknown")));
        assert_eq!(page(s, json!("next"))["page"], 1);
    });
}

#[test]
fn a_failed_count_leaves_a_newer_count_alone() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        let key = unique("dbee_gate");
        run(s, "table.setFilters", json!({ "raw": gated(t, &key), "rawMode": true }));
        let gate = hold_gate(t, &key);
        // `last` counts first; its count is cancelled while a newer count waits behind it.
        let Outcome::Started(last) = s.execute("table.page", json!({ "to": "last" })).unwrap() else { panic!("a job") };
        until_running(t, &key);
        assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": true }));
        let Outcome::Started(count) = s.execute("table.count", json!({})).unwrap() else { panic!("a job") };
        assert_eq!(s.wait(last, T).unwrap_err().code(), "cancelled", "{}", t.name());
        let tab = s.inspect()["table"].clone();
        assert_eq!((&tab["total"], tab["counting"].as_u64()), (&json!({ "counting": { "estimate": null } }), Some(count.0)));
        drop(gate);
        assert_eq!(s.wait(count, T).unwrap()["total"], json!({ "exact": 5000 }));
        assert_eq!(s.inspect()["table"]["counting"], Json::Null);
    });
}

#[test]
fn typed_filters_survive_a_lost_connection() {
    let _db = require_testdb!();
    for_each_engine_exclusive(|t, s| {
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        open_table(s, t.schema("shop"), "orders");
        kill_dbee_connections(t, s);
        let typed = json!([{ "column": "id", "op": "eq", "value": "5" }]);
        let e = run_err(s, "table.setFilters", json!({ "filters": typed }));
        assert_eq!(e.code(), "connectionLost", "{}: {e}", t.name());
        assert_eq!(s.inspect()["connection"]["lost"], true);
        assert_eq!(s.inspect()["table"]["filters"], typed);
        run(s, "connection.reconnect", json!({}));
        assert_eq!(run(s, "table.setFilters", json!({}))["rows"], 1);
    });
}

#[test]
fn page_after_connection_killed_reports_connection_lost() {
    let _db = require_testdb!();
    for_each_engine_exclusive(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        kill_dbee_connections(t, s);
        let e = run_err(s, "table.page", json!({ "to": "next" }));
        assert_eq!(code(&e), "connectionLost", "{}: {e}", t.name());
        assert_eq!(e.data()["action"], "connection.reconnect");
        assert_eq!(s.inspect()["connection"]["lost"], true);
        assert_eq!(s.execute("table.page", json!({ "to": "next" })).unwrap_err().code(), "disabled");
        run(s, "connection.reconnect", json!({}));
        // The rows from before stay, and paging goes on from the page they are.
        let r = page(s, json!("next"));
        assert_eq!(r["page"], 1);
        assert_eq!(first_id(s), 101);
    });
}

#[test]
fn odd_identifiers_open_filter_sort_follow() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let name = t.kind().info().odd_table;
        assert_eq!(open_table(s, t.schema("odd"), name)["rows"], 4);
        let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "Total $", "op": "gt", "value": "1" }] }));
        assert_eq!(r["rows"], 2);
        run(s, "table.sort", json!({ "column": "lower UPPER", "dir": "asc" }));
        let asc: Vec<String> = {
            let d = read(s);
            (0..rows(&d)).map(|i| cell(&d, i, "lower UPPER").unwrap().to_owned()).collect()
        };
        run(s, "table.sort", json!({ "column": "lower UPPER", "dir": "desc" }));
        let d = read(s);
        let desc: Vec<String> = (0..rows(&d)).map(|i| cell(&d, i, "lower UPPER").unwrap().to_owned()).collect();
        assert_eq!(desc.len(), 2);
        assert_eq!(desc, asc.iter().rev().cloned().collect::<Vec<_>>());
        let target = cell(&d, 0, "Other Ödd id").unwrap().to_owned();
        let r = run(s, "table.followForeignKey", json!({ "row": 0, "column": "Other Ödd id" }));
        assert_eq!(r["rows"], 1);
        let tab = &s.inspect()["table"];
        assert_eq!((tab["schema"].as_str(), tab["name"].as_str()), (Some(t.schema("odd")), Some("Other Ödd")));
        assert_eq!(cell(&read(s), 0, "id"), Some(target.as_str()));
        run(s, "table.sort", json!({ "column": "Label", "dir": "desc" }));
        run(s, "table.count", json!({}));
    });
}

#[test]
fn json_string_keeps_its_quotes() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        let d = read(s);
        let meta = cell(&d, 0, "meta").unwrap();
        assert!(meta.starts_with('"'), "{}: {meta}", t.name());
        let meta = &d["columns"][col(&d, "meta")];
        assert_eq!((&meta["class"], &meta["baseType"]), (&json!("json"), &json!(t.kind().info().plain_json_label)), "{}", t.name());
    });
}

#[test]
fn twenty_megabyte_cell_loads() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // The light seed has no big cell.
        if !t.full_seed() {
            return;
        }
        assert_eq!(open_table(s, t.schema("shop"), "big_cell")["rows"], 1);
        let d = read(s);
        assert_eq!(cell(&d, 0, "t").unwrap().chars().count(), 20_000_000);
    });
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn twenty_megabyte_cell_loads_within_ten_seconds() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        if !t.full_seed() {
            return;
        }
        let started = Instant::now();
        assert_eq!(open_table(s, t.schema("shop"), "big_cell")["rows"], 1);
        println!("{}: a 20 MB cell loaded in {:?}", t.name(), started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(10), "{}: {:?}", t.name(), started.elapsed());
    });
}

#[test]
fn a_page_cut_by_the_byte_cap_says_so() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // 280 MB written: on the servers that hold the full seed, which are sized for it.
        if !t.full_seed() {
            continue;
        }
        let name = unique("t_page_cap");
        // 14 values of 20,000,000 characters: 280 MB, past a page's 256 MiB of cell text.
        let (sc, x) = (t.schema("scratch"), t.repeat_sql("'x'", 20_000_000));
        let inserts: String = (1..=14).map(|i| format!("INSERT INTO {sc}.{name} VALUES ({i}, {x});")).collect();
        let _drop = scratch_table(&t, &name, &format!("CREATE TABLE {{t}} (id int PRIMARY KEY, v {}); {inserts}", t.kind().info().long_text_type));
        let mut s = connected_to(&t);
        // 13 values fit (260 MB); the 14th is read but not kept.
        assert_eq!(open_table(&mut s, t.schema("scratch"), &name)["rows"], 13, "{}", t.name());
        assert_eq!(s.inspect()["table"]["pageComplete"], false, "the page was cut");
        let d = run(&mut s, "table.read", json!({ "limit": 0 }));
        assert_eq!((d["pageComplete"].as_bool(), d["hasNext"].as_bool()), (Some(false), Some(false)));
        // A page under the cap is whole, and so is a full page followed by more rows.
        open_table(&mut s, t.schema("shop"), "orders");
        assert_eq!(s.inspect()["table"]["pageComplete"], true);
        assert_eq!(run(&mut s, "table.read", json!({ "limit": 0 }))["pageComplete"], true);
    }
}

#[test]
fn cancel_stops_a_slow_page() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "orders");
        // Nothing runs: nothing to cancel.
        assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": false }));
        let key = unique("dbee_gate");
        let gate = hold_gate(t, &key);
        // On a file the condition counts for about 30 s, as a file has no other session to hold a lock.
        let slow = if t.is_file() { t.slow_condition_sql(30) } else { t.waits_for_lock_sql(&key) };
        // Stopped right after it starts, and once it runs: each time it ends cancelled while its
        // statement cannot end by itself (a cancel that comes before the statement is sent again).
        for running in [false, true] {
            let Outcome::Started(job) = s.execute("table.setFilters", json!({ "raw": slow, "rawMode": true })).unwrap() else { panic!("a job") };
            if running && t.lists_running_statements() {
                until_running(t, &key);
            }
            assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": true }));
            assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": false, "stopping": true }), "a second cancel sends nothing itself");
            let e = s.wait(job, Duration::from_secs(60)).unwrap_err();
            assert_eq!(e.code(), "cancelled", "{}: {e}", t.name());
            // The rows from before stay.
            assert_eq!(rows(&read(s)), 100);
            assert_eq!(s.inspect()["table"]["applied"], "none");
        }
        // A count is stopped the same way: its condition applied while the lock is free.
        drop(gate);
        run(s, "table.setFilters", json!({ "raw": gated(t, &key), "rawMode": true }));
        let gate = hold_gate(t, &key);
        let Outcome::Started(count) = s.execute("table.count", json!({})).unwrap() else { panic!("a job") };
        assert_eq!(run(s, "table.cancel", json!({})), json!({ "sent": true }));
        assert_eq!(s.wait(count, Duration::from_secs(60)).unwrap_err().code(), "cancelled", "{}", t.name());
        drop(gate);
        // The connection works.
        assert_eq!(page(s, json!("next"))["page"], 1);
    });
}

#[test]
fn table_commands_need_a_table_tab() {
    let _db = require_testdb!();
    {
        let mut s = session();
        assert_eq!(s.execute("table.open", json!({ "schema": "shop", "table": "orders" })).unwrap_err().code(), "disabled");
    }
    for_each_engine(|_, s| {
        s.execute("tab.newQuery", json!({})).unwrap();
        for id in
            ["table.setFilters", "table.page", "table.sort", "table.count", "table.cancel", "table.refresh", "table.read", "table.followForeignKey"]
        {
            let e = s.execute(id, json!({ "to": "next", "column": "id" })).unwrap_err();
            assert_eq!((e.code(), e.data()["why"].as_str()), ("disabled", Some("no table tab is active")), "{id}");
        }
    });
}

#[test]
fn browses_on_every_server() {
    let _db = require_testdb!();
    for_each_engine(|t: &TestTarget, s| {
        open_table(s, t.schema("shop"), "orders");
        let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "status", "op": "in", "value": "paid, completed" }] }));
        assert_eq!(r["rows"], 100, "{}", t.name());
        let n: u64 = q(t, &format!("SELECT count(*) FROM {}.orders WHERE status IN ('paid', 'completed')", t.schema("shop"))).parse().unwrap();
        assert_eq!(page(s, json!("last"))["total"], json!({ "exact": n }), "{}", t.name());
        assert_eq!(read(s)["rows"].as_array().map(Vec::len), Some((n - 1) as usize % 100 + 1), "{}", t.name());
        run(s, "table.followForeignKey", json!({ "row": 0, "column": "customer_id" }));
        assert_eq!(read(s)["rows"].as_array().map(Vec::len), Some(1), "{}", t.name());
    });
}

#[test]
#[ignore]
fn next_page_on_a_million_rows_is_fast() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "big_rows");
        let mut times: Vec<Duration> = (0..20)
            .map(|_| {
                let started = Instant::now();
                page(s, json!("next"));
                started.elapsed()
            })
            .collect();
        times.sort();
        let median = times[times.len() / 2];
        println!(
            "{}: next page on shop.big_rows ({} rows): median {median:?} over 20 loads (min {:?}, max {:?})",
            t.name(),
            t.big_rows(),
            times[0],
            times[times.len() - 1]
        );
        assert!(median < Duration::from_millis(200), "median {median:?}");
    });
}

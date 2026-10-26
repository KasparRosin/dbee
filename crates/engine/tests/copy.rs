//! Selection, the copy formats and the value view. Needs the Docker test servers (`cargo xtask
//! testdb up`); every test writes only to its own table in `scratch`, which it drops at the end.

mod common;

use common::*;
use dbee_engine::{COPY_MAX_BYTES, Json, Session, VALUE_VIEW_MAX_BYTES};
use dbee_testkit::{require_testdb, unique};
use serde_json::json;

/// Drops the scratch tables it names when the test ends, also when it fails.
struct Scratch(String);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = dbee_testkit::psql(pg14(), &format!("DROP TABLE IF EXISTS scratch.{} CASCADE", self.0));
    }
}

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.run_blocking(id, params, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data()))
}

/// `scratch.<u> (id int PK, name text, billing jsonb, note text)` with 2 rows.
fn make_table() -> (String, Scratch) {
    let name = unique("t_copy");
    let guard = Scratch(name.clone());
    dbee_testkit::psql(
        pg14(),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, name text, billing jsonb, note text);
             INSERT INTO scratch.{name} VALUES (1, 'Ann', '{{\"b\":2,\"a\":1}}', NULL), (2, '', NULL, 'x');"
        ),
    )
    .unwrap();
    (name, guard)
}

fn opened(name: &str) -> TestSession {
    let mut s = connected();
    assert_eq!(run(&mut s, "table.open", json!({ "schema": "scratch", "table": name }))["rows"], 2);
    s
}

fn copy(s: &mut Session, format: &str) -> Json {
    run(s, "copy.selection", json!({ "format": format }))
}

fn text(r: &Json) -> &str {
    r["text"].as_str().unwrap_or_else(|| panic!("no text in {r}"))
}

#[test]
fn select_all_copy_tsv_has_no_header() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    let r = run(&mut s, "selection.all", json!({}));
    assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(2), json!(4)), "{r}");
    let r = copy(&mut s, "tsv");
    assert_eq!(text(&r), "1\tAnn\t{\"a\": 1, \"b\": 2}\t\n2\t\t\tx");
    assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(2), json!(4)));
    assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 0], "focus": [1, 3] }));
}

#[test]
fn copy_with_header_from_query_results() {
    let _db = require_testdb!();
    let mut s = connected();
    run(&mut s, "tab.newQuery", json!({ "text": "select 1 as a, null as b" }));
    run(&mut s, "query.run", json!({}));
    run(&mut s, "selection.all", json!({}));
    assert_eq!(text(&copy(&mut s, "tsvHeader")), "a\tb\n1\t");
    // A rectangle, anchor after focus: still in grid order.
    run(&mut s, "selection.set", json!({ "anchor": [0, 1], "focus": [0, 0] }));
    assert_eq!(text(&copy(&mut s, "tsvHeader")), "a\tb\n1\t");
    run(&mut s, "selection.set", json!({ "anchor": [0, 1] }));
    assert_eq!(text(&copy(&mut s, "json")), "[\n  {\"b\": null}\n]");
}

#[test]
fn csv_json_markdown_have_headers() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    run(&mut s, "selection.all", json!({}));
    assert!(text(&copy(&mut s, "csv")).starts_with("id,name,billing,note"));
    assert!(text(&copy(&mut s, "json")).starts_with("[\n  {\"id\": 1"));
    assert!(text(&copy(&mut s, "markdown")).starts_with("| id | name | billing | note |"));
    let e = s.execute("copy.selection", json!({ "format": "xml" })).unwrap_err();
    assert_eq!(e.code(), "badParams");
    assert_eq!(s.execute("copy.selection", json!({})).unwrap_err().code(), "badParams");
}

#[test]
fn copy_shows_staged_values() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "Zed" }));
    run(&mut s, "cell.setNull", json!({ "row": 1, "column": "note" }));
    run(&mut s, "row.add", json!({}));
    run(&mut s, "cell.edit", json!({ "row": 2, "column": "id", "value": "3" }));
    run(&mut s, "cell.edit", json!({ "row": 2, "column": "name", "value": "New" }));
    // The grid has the new row after the loaded ones.
    run(&mut s, "selection.all", json!({}));
    let r = copy(&mut s, "tsv");
    assert_eq!(text(&r), "1\tZed\t{\"a\": 1, \"b\": 2}\t\n2\t\t\t\n3\tNew\t\t");
    assert_eq!(r["rows"], 3);
    // Nothing was written.
    assert_eq!(dbee_testkit::psql(pg14(), &format!("SELECT name FROM scratch.{name} WHERE id = 1")).unwrap().trim(), "Ann");
}

#[test]
fn value_view_labels() {
    let _db = require_testdb!();
    let name = unique("t_view");
    let _g = Scratch(name.clone());
    dbee_testkit::psql(
        pg14(),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, j jsonb, t text, js json, plain text);
             INSERT INTO scratch.{name} VALUES
               (1, '{{\"b\":2,\"a\":1}}', '{{\"k\":1}}', $$\"{{\\\"a\\\":1}}\"$$, 'hello'),
               (2, NULL, NULL, NULL, NULL);"
        ),
    )
    .unwrap();
    let mut s = connected();
    run(&mut s, "table.open", json!({ "schema": "scratch", "table": name }));
    let view = |s: &mut Session, row: usize, col: usize| {
        run(s, "selection.set", json!({ "anchor": [row, col] }));
        let v = run(s, "value.show", json!({}));
        assert!(!v.is_null(), "no value at {row},{col}");
        v
    };
    let v = view(&mut s, 0, 1);
    assert_eq!((v["kindLabel"].as_str(), v["pretty"].clone(), v["isNull"].clone()), (Some("jsonb"), json!(true), json!(false)), "{v}");
    assert!(text(&v).starts_with("{\n  \"a\": 1"), "{v}");
    assert_eq!((v["column"].as_str(), v["typeLabel"].as_str()), (Some("j"), Some("jsonb")));
    let direct = s.value_view().unwrap();
    assert_eq!((direct.kind_label, direct.pretty, direct.truncated), ("jsonb", true, false));
    let v = view(&mut s, 0, 2);
    assert_eq!(v["kindLabel"], "text containing JSON", "{v}");
    assert!(text(&v).starts_with("{\n  \"k\": 1"));
    let v = view(&mut s, 0, 3);
    assert_eq!(v["kindLabel"], "JSON string containing JSON", "{v}");
    assert_eq!(text(&v), "{\n  \"a\": 1\n}");
    let v = view(&mut s, 0, 4);
    assert_eq!((v["kindLabel"].as_str(), v["pretty"].clone(), text(&v)), (Some(""), json!(false), "hello"));
    let v = view(&mut s, 1, 1);
    assert_eq!((v["isNull"].clone(), text(&v)), (json!(true), ""), "{v}");
    // No selection, no value.
    let mut other = connected();
    assert_eq!(run(&mut other, "value.show", json!({})), Json::Null);
    assert!(other.value_view().is_none());
}

#[test]
fn value_view_shows_staged_values() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    run(&mut s, "selection.set", json!({ "anchor": [0, 1] }));
    assert_eq!(s.value_view().unwrap().text, "Ann");
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "Zed" }));
    assert_eq!(s.value_view().unwrap().text, "Zed");
    run(&mut s, "cell.setNull", json!({ "row": 0, "column": "name" }));
    assert!(s.value_view().unwrap().is_null);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.value_view().unwrap().text, "Zed");
    run(&mut s, "edit.reset", json!({}));
    assert_eq!(s.value_view().unwrap().text, "Ann");
}

#[test]
fn value_view_is_computed_once_per_key() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    run(&mut s, "selection.set", json!({ "anchor": [0, 2] }));
    assert_eq!(s.value_view_computations(), 0);
    let first = s.value_view().unwrap();
    for _ in 0..5 {
        assert_eq!(s.value_view().unwrap().text, first.text);
        let _ = s.inspect();
    }
    assert_eq!(s.value_view_computations(), 1, "the same cell, data and edits");
    // A staged edit to the focused cell recomputes; another cell's edit does not need to, but the
    // focus moving does.
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "billing", "value": "[1]" }));
    assert_eq!(s.value_view().unwrap().text, "[\n  1\n]");
    assert_eq!(s.value_view_computations(), 2);
    s.value_view();
    assert_eq!(s.value_view_computations(), 2);
    run(&mut s, "selection.set", json!({ "anchor": [1, 3] }));
    assert_eq!(s.value_view().unwrap().text, "x");
    assert_eq!(s.value_view_computations(), 3);
    // A new page (new data) recomputes.
    run(&mut s, "edit.reset", json!({}));
    run(&mut s, "table.refresh", json!({}));
    s.value_view();
    assert_eq!(s.value_view_computations(), 4);
    s.value_view();
    assert_eq!(s.value_view_computations(), 4);
}

#[test]
fn value_view_truncates_huge_values() {
    let _db = require_testdb!();
    let mut s = connected();
    assert_eq!(run(&mut s, "table.open", json!({ "schema": "shop", "table": "big_cell" }))["rows"], 1);
    run(&mut s, "selection.set", json!({ "anchor": [0, 1] }));
    let started = std::time::Instant::now();
    let v = s.value_view().unwrap();
    assert!(v.truncated && !v.pretty, "{:?}", (v.truncated, v.pretty));
    assert!(v.text.len() <= VALUE_VIEW_MAX_BYTES && v.text.len() > VALUE_VIEW_MAX_BYTES - 4, "{}", v.text.len());
    assert_eq!(v.byte_len, 20_000_000);
    // Later frames cost nothing: the view is kept.
    for _ in 0..20 {
        s.value_view();
        let _ = s.inspect();
    }
    assert_eq!(s.value_view_computations(), 1);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    // The inspect summary holds the metadata and a short start of the text, not the whole value.
    let i = s.inspect()["value"].clone();
    assert_eq!((i["truncated"].clone(), i["column"].clone(), i["bytes"].clone()), (json!(true), json!("t"), json!(20_000_000)), "{}", i["kindLabel"]);
    assert!(i["text"].as_str().unwrap().len() <= 4096);
    // The jsonb cell is as large: cut as well, still labelled jsonb.
    run(&mut s, "selection.set", json!({ "anchor": [0, 2] }));
    let v = run(&mut s, "value.show", json!({}));
    assert_eq!((v["truncated"].clone(), v["kindLabel"].clone()), (json!(true), json!("jsonb")));
    assert!(text(&v).len() <= VALUE_VIEW_MAX_BYTES);
}

#[test]
fn selection_out_of_range_is_bad_params() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    for p in [
        json!({ "anchor": [999, 0] }),
        json!({ "anchor": [0, 4] }),
        json!({ "anchor": [2, 0] }),
        json!({ "anchor": [0] }),
        json!({ "anchor": "a" }),
        json!({ "anchor": [-1, 0] }),
        json!({ "anchor": [0, 0], "focus": [1] }),
        json!({}),
    ] {
        assert_eq!(s.execute("selection.set", p.clone()).unwrap_err().code(), "badParams", "{p}");
    }
    assert_eq!(s.inspect()["selection"], Json::Null);
    // A focus past the edge is clamped.
    run(&mut s, "selection.set", json!({ "anchor": [0, 1], "focus": [999, 999] }));
    assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 1], "focus": [1, 3] }));
    assert_eq!(copy(&mut s, "tsv")["rows"], 2);
    // A bad call changes nothing.
    assert_eq!(s.execute("selection.set", json!({ "anchor": [5, 5] })).unwrap_err().code(), "badParams");
    assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 1], "focus": [1, 3] }));
}

#[test]
fn selection_follows_the_grid() {
    let _db = require_testdb!();
    let (name, _g) = make_table();
    let mut s = opened(&name);
    // No grid tab, no selection command.
    let mut empty = connected();
    assert_eq!(empty.execute("selection.all", json!({})).unwrap_err().code(), "disabled");
    assert_eq!(empty.execute("copy.selection", json!({ "format": "tsv" })).unwrap_err().code(), "disabled");
    // Nothing selected: copy is disabled.
    assert_eq!(s.execute("copy.selection", json!({ "format": "tsv" })).unwrap_err().code(), "disabled");
    // A selection on a new row goes when that row does.
    run(&mut s, "row.add", json!({}));
    run(&mut s, "selection.set", json!({ "anchor": [2, 0] }));
    assert!(s.value_view().is_some());
    run(&mut s, "edit.undo", json!({}));
    assert!(s.value_view().is_none());
    assert_eq!(s.inspect()["selection"], Json::Null);
    assert_eq!(s.execute("copy.selection", json!({ "format": "tsv" })).unwrap_err().code(), "disabled");
}

#[test]
fn copy_takes_at_most_32_mib_of_cell_text() {
    let _db = require_testdb!();
    assert_eq!(COPY_MAX_BYTES, 32 * 1024 * 1024);
    let mut s = connected();
    // Two cells holding exactly the cap: copied, in every format.
    let at_cap = format!("select repeat('x', {}) as a, 'y' as b, null as c", COPY_MAX_BYTES - 1);
    run(&mut s, "tab.newQuery", json!({ "text": at_cap }));
    run(&mut s, "query.run", json!({}));
    run(&mut s, "selection.all", json!({}));
    let r = copy(&mut s, "tsv");
    assert_eq!((text(&r).len(), r["rows"].clone(), r["columns"].clone()), (COPY_MAX_BYTES + 2, json!(1), json!(3)));
    assert!(text(&copy(&mut s, "json")).ends_with("\"b\": \"y\", \"c\": null}\n]"));
    // One byte more: nothing is rendered, and the reason gives the size, the cap and what to do.
    let over = format!("select repeat('x', {}) as a, 'y' as b, null as c", COPY_MAX_BYTES);
    run(&mut s, "query.setText", json!({ "text": over }));
    run(&mut s, "query.run", json!({}));
    run(&mut s, "selection.all", json!({}));
    for format in ["tsv", "tsvHeader", "csv", "json", "markdown"] {
        let e = s.execute("copy.selection", json!({ "format": format })).unwrap_err();
        assert_eq!(e.code(), "disabled", "{format}: {e}");
        let why = e.data()["why"].as_str().unwrap_or_default().to_string();
        assert_eq!(why, "the selection holds 32.1 MiB of cell text, more than the 32 MiB a copy can take; select fewer rows", "{format}");
    }
    // A smaller part of the same result copies.
    run(&mut s, "selection.set", json!({ "anchor": [0, 1], "focus": [0, 2] }));
    assert_eq!(text(&copy(&mut s, "tsv")), "y\t");
}

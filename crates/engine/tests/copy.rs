//! Selection, the copy formats and the value view, on every engine ([`for_each_engine`]). Needs
//! the Docker test servers (`cargo xtask testdb up`); every test writes only to its own table in
//! `scratch`, which it drops at the end.

mod common;

use common::*;
use dbee_engine::{COPY_MAX_BYTES, Json, Outcome, Session, VALUE_VIEW_MAX_BYTES};
use dbee_testkit::{TestTarget, require_testdb, unique};
use serde_json::json;

/// `scratch.<u> (id int PK, name text, billing <json>, note text)` with 2 rows. The JSON is
/// written as every engine prints it.
fn make_table(t: &TestTarget) -> (String, Scratch<'_>) {
    let name = unique("t_copy");
    let json = t.kind().info().json_type;
    let guard = scratch_table(
        t,
        &name,
        &format!(
            "CREATE TABLE {{t}} (id int PRIMARY KEY, name text, billing {json}, note text);
             INSERT INTO {{t}} VALUES (1, 'Ann', '{{\"a\": 1, \"b\": 2}}', NULL), (2, '', NULL, 'x');"
        ),
    );
    (name, guard)
}

/// Runs `test` on every engine with the copy table made and open.
fn on_each_table(mut test: impl FnMut(&TestTarget, &str, &mut TestSession)) {
    for t in engine_targets() {
        println!("on {}", t.name());
        let (name, _drop) = make_table(&t);
        let mut s = connected_to(&t);
        assert_eq!(open_table(&mut s, t.schema("scratch"), &name)["rows"], 2);
        test(&t, &name, &mut s);
    }
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
    on_each_table(|_, _, s| {
        let r = run(s, "selection.all", json!({}));
        assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(2), json!(4)), "{r}");
        let r = copy(s, "tsv");
        assert_eq!(text(&r), "1\tAnn\t{\"a\": 1, \"b\": 2}\t\n2\t\t\tx");
        assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(2), json!(4)));
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 0], "focus": [1, 3], "whole": false }));
    });
}

#[test]
fn copy_with_header_from_query_results() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        run(s, "tab.newQuery", json!({ "text": "select 1 as a, null as b" }));
        run(s, "query.run", json!({}));
        run(s, "selection.all", json!({}));
        assert_eq!(text(&copy(s, "tsvHeader")), "a\tb\n1\t");
        // A rectangle, anchor after focus: still in grid order.
        run(s, "selection.set", json!({ "anchor": [0, 1], "focus": [0, 0] }));
        assert_eq!(text(&copy(s, "tsvHeader")), "a\tb\n1\t");
        run(s, "selection.set", json!({ "anchor": [0, 1] }));
        assert_eq!(text(&copy(s, "json")), "[\n  {\"b\": null}\n]");
    });
}

#[test]
fn csv_json_markdown_have_headers() {
    let _db = require_testdb!();
    on_each_table(|_, _, s| {
        run(s, "selection.all", json!({}));
        assert!(text(&copy(s, "csv")).starts_with("id,name,billing,note"));
        assert!(text(&copy(s, "json")).starts_with("[\n  {\"id\": 1"));
        assert!(text(&copy(s, "markdown")).starts_with("| id | name | billing | note |"));
        let e = s.execute("copy.selection", json!({ "format": "xml" })).unwrap_err();
        assert_eq!(e.code(), "badParams");
        assert_eq!(s.execute("copy.selection", json!({})).unwrap_err().code(), "badParams");
    });
}

#[test]
fn copy_shows_staged_values() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        set_cell(s, 0, "name", "Zed");
        run(s, "cell.setNull", json!({ "row": 1, "column": "note" }));
        run(s, "row.add", json!({}));
        set_cell(s, 2, "id", "3");
        set_cell(s, 2, "name", "New");
        // The grid has the new row after the loaded ones.
        run(s, "selection.all", json!({}));
        let r = copy(s, "tsv");
        assert_eq!(text(&r), "1\tZed\t{\"a\": 1, \"b\": 2}\t\n2\t\t\t\n3\tNew\t\t");
        assert_eq!(r["rows"], 3);
        // Nothing was written.
        assert_eq!(q(t, &format!("SELECT name FROM {}.{name} WHERE id = 1", t.schema("scratch"))), "Ann");
    });
}

#[test]
fn value_view_labels() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let name = unique("t_view");
        let json = t.kind().info().json_type;
        let _g = scratch_table(
            &t,
            &name,
            &format!(
                "CREATE TABLE {{t}} (id int PRIMARY KEY, j {json}, t text, js {json}, plain text);
                 INSERT INTO {{t}} VALUES (1, '{{\"b\": 2, \"a\": 1}}', '{{\"k\":1}}', NULL, 'hello'), (2, NULL, NULL, NULL, NULL);"
            ),
        );
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("scratch"), &name);
        // A JSON string holding JSON, written by dbee: its backslashes are text on every engine.
        set_cell(&mut s, 0, "js", r#""{\"a\":1}""#);
        apply(&mut s);
        let view = |s: &mut Session, row: usize, col: usize| {
            run(s, "selection.set", json!({ "anchor": [row, col] }));
            let v = run(s, "value.show", json!({}));
            assert!(!v.is_null(), "no value at {row},{col}");
            v
        };
        // A JSON column is labelled with its type, its text pretty, keys in the engine's order.
        let info = t.kind().info();
        let v = view(&mut s, 0, 1);
        let label = v["typeLabel"].as_str().unwrap_or_default().to_owned();
        assert_eq!(label, info.json_type_label, "{}: {v}", t.name());
        assert_eq!((v["kindLabel"].as_str(), v["pretty"].clone(), v["isNull"].clone()), (Some(label.as_str()), json!(true), json!(false)), "{v}");
        let pretty = if info.json_sorts_keys { "{\n  \"a\": 1,\n  \"b\": 2\n}" } else { "{\n  \"b\": 2,\n  \"a\": 1\n}" };
        assert_eq!(text(&v), pretty, "{}", t.name());
        assert_eq!((v["column"].as_str(), v["kind"].as_str()), (Some("j"), Some("json")));
        let direct = s.value_view().unwrap();
        assert_eq!((direct.kind_label.as_str(), direct.pretty, direct.truncated), (label.as_str(), true, false));
        let v = view(&mut s, 0, 2);
        assert_eq!(v["kindLabel"], "text containing JSON", "{v}");
        assert!(text(&v).starts_with("{\n  \"k\": 1"));
        let v = view(&mut s, 0, 3);
        assert_eq!(v["kindLabel"], "JSON string containing JSON", "{}: {v}", t.name());
        assert_eq!(text(&v), "{\n  \"a\": 1\n}");
        let v = view(&mut s, 0, 4);
        assert_eq!((v["kindLabel"].as_str(), v["pretty"].clone(), text(&v)), (Some(""), json!(false), "hello"));
        let v = view(&mut s, 1, 1);
        assert_eq!((v["isNull"].clone(), text(&v)), (json!(true), ""), "{v}");
        drop(s);
        // No selection, no value.
        let mut other = connected_to(&t);
        assert_eq!(run(&mut other, "value.show", json!({})), Json::Null);
        assert!(other.value_view().is_none());
    }
}

/// The object Copy as JSON writes for grid row `row`, selected across every column: the list's one
/// object, without the list.
fn copied_object(s: &mut Session, row: usize) -> String {
    run(s, "selection.set", json!({ "anchor": [row, 0], "focus": [row, 999] }));
    let list = text(&copy(s, "json")).to_owned();
    let object = list.strip_prefix("[\n  ").and_then(|l| l.strip_suffix("\n]"));
    object.unwrap_or_else(|| panic!("one object: {list}")).to_owned()
}

/// A whole-row selection's value view is the focus row's object as Copy as JSON writes it,
/// pretty-printed, titled with the row's number as the grid shows it (the page's offset counted
/// in), staged values included.
#[test]
fn the_value_view_of_a_row_is_its_json() {
    let _db = require_testdb!();
    for t in engine_targets() {
        println!("on {}", t.name());
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("shop"), "customers");
        let object = copied_object(&mut s, 2);
        run(&mut s, "selection.rows", json!({ "first": 2 }));
        let view = s.value_view().unwrap_or_else(|| panic!("{}: a view of the row", t.name()));
        assert_eq!(Some(view.text.as_str()), dbee_core::json::pretty(&object).as_deref(), "{}", t.name());
        assert!(view.text.starts_with("{\n  \"id\": 3,\n  \"name\": \"Customer 3\","), "{}: {}", t.name(), view.text);
        assert_eq!((view.title(), view.row, view.pretty, view.truncated, view.is_null), ("Row 3".into(), Some(3), true, false, false));
        assert_eq!((view.byte_len, view.column.as_str(), view.type_label.as_str(), view.kind_label.as_str()), (object.len(), "", "", ""));
        assert_eq!(s.inspect()["value"]["row"], 3);
        // Built once, until the row or its values change; a staged value shows.
        let built = s.value_view_computations();
        s.value_view();
        assert_eq!(s.value_view_computations(), built);
        set_cell(&mut s, 2, "name", "Zed");
        assert!(s.value_view().unwrap().text.contains("\"name\": \"Zed\""));
        run(&mut s, "edit.reset", json!({}));
        // A cell again: its own view.
        run(&mut s, "selection.set", json!({ "anchor": [2, 1] }));
        let cell = s.value_view().unwrap();
        assert_eq!((cell.row, cell.title()), (None, "name".into()));
        // The second page's third row is the 103rd.
        run(&mut s, "table.page", json!({ "to": "next" }));
        let object = copied_object(&mut s, 2);
        run(&mut s, "selection.rows", json!({ "first": 2 }));
        let view = s.value_view().unwrap();
        assert_eq!((view.title(), view.row), ("Row 103".into(), Some(103)), "{}", t.name());
        assert_eq!(Some(view.text), dbee_core::json::pretty(&object));
    }
}

#[test]
fn value_view_shows_staged_values() {
    let _db = require_testdb!();
    on_each_table(|_, _, s| {
        run(s, "selection.set", json!({ "anchor": [0, 1] }));
        assert_eq!(s.value_view().unwrap().text, "Ann");
        set_cell(s, 0, "name", "Zed");
        assert_eq!(s.value_view().unwrap().text, "Zed");
        run(s, "cell.setNull", json!({ "row": 0, "column": "name" }));
        assert!(s.value_view().unwrap().is_null);
        run(s, "edit.undo", json!({}));
        assert_eq!(s.value_view().unwrap().text, "Zed");
        run(s, "edit.reset", json!({}));
        assert_eq!(s.value_view().unwrap().text, "Ann");
    });
}

#[test]
fn value_view_is_computed_once_per_key() {
    let _db = require_testdb!();
    on_each_table(|_, _, s| {
        run(s, "selection.set", json!({ "anchor": [0, 2] }));
        assert_eq!(s.value_view_computations(), 0);
        let first = s.value_view().unwrap();
        for _ in 0..5 {
            assert_eq!(s.value_view().unwrap().text, first.text);
            let _ = s.inspect();
        }
        assert_eq!(s.value_view_computations(), 1, "the same cell, data and edits");
        // A staged edit to the focused cell recomputes; another cell's edit does not need to, but
        // the focus moving does.
        set_cell(s, 0, "billing", "[1]");
        assert_eq!(s.value_view().unwrap().text, "[\n  1\n]");
        assert_eq!(s.value_view_computations(), 2);
        s.value_view();
        assert_eq!(s.value_view_computations(), 2);
        run(s, "selection.set", json!({ "anchor": [1, 3] }));
        assert_eq!(s.value_view().unwrap().text, "x");
        assert_eq!(s.value_view_computations(), 3);
        // A new page (new data) recomputes.
        run(s, "edit.reset", json!({}));
        run(s, "table.refresh", json!({}));
        s.value_view();
        assert_eq!(s.value_view_computations(), 4);
        s.value_view();
        assert_eq!(s.value_view_computations(), 4);
    });
}

#[test]
fn value_view_truncates_huge_values() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // The light seed has no big cell.
        if !t.full_seed() {
            return;
        }
        assert_eq!(run(s, "table.open", json!({ "schema": t.schema("shop"), "table": "big_cell" }))["rows"], 1);
        run(s, "selection.set", json!({ "anchor": [0, 1] }));
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
        // The inspect summary holds the metadata and a short start of the text, not the whole value.
        let i = s.inspect()["value"].clone();
        assert_eq!(
            (i["truncated"].clone(), i["column"].clone(), i["bytes"].clone()),
            (json!(true), json!("t"), json!(20_000_000)),
            "{}",
            i["kindLabel"]
        );
        assert!(i["text"].as_str().unwrap().len() <= 4096);
        // The JSON cell is as large: cut as well, still labelled with its type.
        run(s, "selection.set", json!({ "anchor": [0, 2] }));
        let v = run(s, "value.show", json!({}));
        assert_eq!((v["truncated"].clone(), v["kindLabel"].clone()), (json!(true), json!(t.kind().info().json_type_label)), "{}: {v}", t.name());
        assert!(text(&v).len() <= VALUE_VIEW_MAX_BYTES);
    });
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn value_view_of_a_huge_value_within_ten_seconds() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        if !t.full_seed() {
            return;
        }
        run(s, "table.open", json!({ "schema": t.schema("shop"), "table": "big_cell" }));
        run(s, "selection.set", json!({ "anchor": [0, 1] }));
        let started = std::time::Instant::now();
        assert!(s.value_view().unwrap().truncated);
        for _ in 0..20 {
            s.value_view();
            let _ = s.inspect();
        }
        let took = started.elapsed();
        println!("{}: the 20 MB value's view and 20 frames: {took:?}", t.name());
        assert!(took < std::time::Duration::from_secs(10), "{}: {took:?}", t.name());
    });
}

#[test]
fn selection_out_of_range_is_bad_params() {
    let _db = require_testdb!();
    on_each_table(|_, _, s| {
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
        run(s, "selection.set", json!({ "anchor": [0, 1], "focus": [999, 999] }));
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 1], "focus": [1, 3], "whole": false }));
        assert_eq!(copy(s, "tsv")["rows"], 2);
        // A bad call changes nothing.
        assert_eq!(s.execute("selection.set", json!({ "anchor": [5, 5] })).unwrap_err().code(), "badParams");
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 1], "focus": [1, 3], "whole": false }));
    });
}

#[test]
fn selection_follows_the_grid() {
    let _db = require_testdb!();
    for t in engine_targets() {
        {
            // No grid tab, no selection command.
            let mut empty = connected_to(&t);
            assert_eq!(empty.execute("selection.all", json!({})).unwrap_err().code(), "disabled");
            assert_eq!(empty.execute("copy.selection", json!({ "format": "tsv" })).unwrap_err().code(), "disabled");
        }
        let (name, _drop) = make_table(&t);
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("scratch"), &name);
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
}

/// Runs `sql` in the session's query tab (a new one the first time) and selects every cell.
fn query_all(s: &mut Session, sql: &str) {
    if s.inspect()["query"].is_null() {
        run(s, "tab.newQuery", json!({ "text": sql }));
    } else {
        run(s, "query.setText", json!({ "text": sql }));
    }
    run(s, "query.run", json!({}));
    run(s, "selection.all", json!({}));
}

/// The reason a copy is refused, for `format`.
fn refused(s: &mut Session, format: &str) -> String {
    match s.execute("copy.selection", json!({ "format": format })) {
        // Not the text in the message: it can be tens of megabytes.
        Ok(Outcome::Done(v)) => panic!("{format}: copied {} bytes", v["text"].as_str().map_or(0, str::len)),
        Ok(other) => panic!("{format}: {other:?}"),
        Err(e) => {
            assert_eq!(e.code(), "disabled", "{format}: {e}");
            e.data()["why"].as_str().unwrap_or_default().to_string()
        }
    }
}

#[test]
fn the_cap_bounds_the_copied_text() {
    let _db = require_testdb!();
    assert_eq!(COPY_MAX_BYTES, 8 * 1024 * 1024);
    for_each_engine(|t, s| {
        let x = |n: usize| t.repeat_sql("'x'", n as u64);
        // TSV of `a`, `b`, `c` is `<a>\ty\t`: three bytes more than `a`.
        query_all(s, &format!("select {} as a, 'y' as b, null as c", x(COPY_MAX_BYTES - 3)));
        let r = copy(s, "tsv");
        assert_eq!((text(&r).len(), r["rows"].clone(), r["columns"].clone()), (COPY_MAX_BYTES, json!(1), json!(3)));
        // One byte more of text: refused, though the cells alone fit.
        query_all(s, &format!("select {} as a, 'y' as b, null as c", x(COPY_MAX_BYTES - 2)));
        assert_eq!(refused(s, "tsv"), "the copy as TSV would pass the 8 MiB a copy can take (0 of the selected rows fit); select fewer rows");
        // Cells over the cap are refused before anything is rendered, in every format.
        query_all(s, &format!("select {} as a, 'y' as b, null as c", x(COPY_MAX_BYTES)));
        for format in ["tsv", "tsvHeader", "csv", "json", "markdown"] {
            assert_eq!(
                refused(s, format),
                "the selection holds 8.1 MiB of cell text, more than the 8 MiB a copy can take; select fewer rows",
                "{format}"
            );
        }
        // A smaller part of the same result copies.
        run(s, "selection.set", json!({ "anchor": [0, 1], "focus": [0, 2] }));
        assert_eq!(text(&copy(s, "tsv")), "y\t");
    });
}

#[test]
fn escaping_nulls_and_names_count_toward_the_cap() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // JSON writes a control character as six bytes: `[\n  {"a": "` + 6k + `"}\n]` is 6k + 15.
        // The control characters are in the statement's text, which every engine takes.
        let k = (COPY_MAX_BYTES - 15) / 6;
        let times = |text: &str, n: usize| t.repeat_sql(text, n as u64);
        query_all(s, &format!("select {} as a", times("'\u{1}'", k)));
        assert_eq!(text(&copy(s, "json")).len(), 6 * k + 15, "{}", t.name());
        query_all(s, &format!("select {} as a", times("'\u{1}'", k + 1)));
        assert_eq!(refused(s, "json"), "the copy as JSON would pass the 8 MiB a copy can take (0 of the selected rows fit); select fewer rows");
        // Markdown writes a line break as `<br>`: `| a |\n|---|\n| ` + 4k + ` |` is 4k + 16.
        let k = (COPY_MAX_BYTES - 16) / 4;
        query_all(s, &format!("select {} as a", times("'\n'", k)));
        assert_eq!(text(&copy(s, "markdown")).len(), COPY_MAX_BYTES);
        query_all(s, &format!("select {} as a", times("'\n'", k + 1)));
        assert!(refused(s, "markdown").starts_with("the copy as Markdown would pass"));
        // 10,000 rows of 300 NULLs hold no cell text, but as JSON every NULL names its column.
        let nulls: Vec<String> = (1..=300).map(|i| format!("null as c{i:03}")).collect();
        query_all(s, &format!("select {} from {}.big_rows where id <= 10000", nulls.join(", "), t.schema("shop")));
        let why = refused(s, "json");
        assert!(why.starts_with("the copy as JSON would pass the 8 MiB a copy can take (") && why.contains(" of the selected rows fit)"), "{why}");
        // As TSV they are tabs: 299 a row, and the line breaks between rows.
        assert_eq!(text(&copy(s, "tsv")).len(), 10_000 * 299 + 9_999);
    });
}

#[test]
fn copy_and_value_view_skip_the_hidden_key() {
    let _db = require_testdb!();
    for_sqlite(|t, s| {
        open_table(s, t.schema("shop"), "notes_nokey");
        let r = run(s, "selection.all", json!({}));
        assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(3), json!(2)));
        assert_eq!(text(&copy(s, "tsvHeader")), "title\tbody\nfirst\ta note\nsecond\tanother note\nsecond\tanother note");
        for format in ["csv", "json", "markdown"] {
            let copied = copy(s, format);
            assert!(!text(&copied).contains("rowid"), "{format}: {}", text(&copied));
        }
        run(s, "selection.set", json!({ "anchor": [0, 0] }));
        assert_eq!(run(s, "value.show", json!({}))["column"], "title");
        assert_eq!(s.inspect()["value"]["column"], "title");
        run(s, "selection.set", json!({ "anchor": [0, 1] }));
        assert_eq!(s.value_view().map(|v| v.column), Some("body".to_owned()));
        assert_eq!(s.execute("selection.set", json!({ "anchor": [0, 2] })).unwrap_err().code(), "badParams", "no third column");
    });
}

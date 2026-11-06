//! Staged edits and Apply: cell edits, NULL and clearing, new and deleted rows, undo and redo,
//! all-or-nothing Apply and what it refuses, on every engine ([`engine_targets`]). Needs the
//! Docker test servers (`cargo xtask testdb up`); every test writes only to its own table in
//! `scratch`, which it drops at the end.

mod common;

use std::time::Duration;

use common::*;
use dbee_core::TableRef;
use dbee_engine::{EngineError, Json, Outcome, Session, Tab};
use dbee_testkit::{Held, TestTarget, require_testdb, unique};
use serde_json::json;

/// `scratch.<u> (id int PK, name text, total numeric(12,2), shipped_at date, flag bool, doc
/// <json>)` with 3 rows, ids 1 to 3. Made before the session connects, so the catalog has it.
fn make_table(t: &TestTarget) -> (String, Scratch<'_>) {
    let name = unique("t_edit");
    let (json, shipped_at) = (t.kind().info().json_type, t.date_column_sql("shipped_at"));
    let guard = scratch_table(
        t,
        &name,
        &format!(
            "CREATE TABLE {{t}} (id int PRIMARY KEY, name text, total numeric(12,2), {shipped_at}, flag bool, doc {json});
             INSERT INTO {{t}} VALUES
                 (1, 'Alice', 10.50, '2025-01-02', true, '{{\"a\": 1}}'),
                 (2, 'Bob', 20.00, '2025-02-03', false, '[1, 2]'),
                 (3, 'Cy', 5.25, NULL, NULL, NULL);"
        ),
    );
    (name, guard)
}

/// A session connected to `t` with `scratch.<name>` open.
fn opened(t: &TestTarget, name: &str) -> TestSession {
    opened_rows(t, name, 3)
}

fn opened_rows(t: &TestTarget, name: &str, rows: u64) -> TestSession {
    let mut s = connected_to(t);
    let r = open_table(&mut s, t.schema("scratch"), name);
    assert_eq!(r["rows"], rows, "{}: {r}", t.name());
    s
}

/// Runs `test` on every engine with the edit table made and open.
fn on_each_table(mut test: impl FnMut(&TestTarget, &str, &mut TestSession)) {
    for t in engine_targets() {
        println!("on {}", t.name());
        let (name, _drop) = make_table(&t);
        let mut s = opened(&t, &name);
        test(&t, &name, &mut s);
    }
}

/// Keeps the Apply's update of row `id` of `scratch.<name>` from ending by itself: on a server,
/// another session holds the row until the returned guard drops, so the update waits for it. A
/// file has no other session: a trigger makes its updates count for about 2 s instead (longer on a
/// busy machine), and `scratch` drops it.
fn hold_row(t: &TestTarget, scratch: &mut Scratch<'_>, name: &str, id: u32) -> Option<Held> {
    let Some(server) = t.server() else {
        let (slow, cleanup) = t.slow_updates_sql(name, 2).expect("a file has slow updates");
        q(t, &slow);
        scratch.1.insert(0, cleanup);
        return None;
    };
    let sql = format!("SELECT id FROM {}.{name} WHERE id = {id} FOR UPDATE", t.schema("scratch"));
    Some(dbee_testkit::hold(server, &sql).unwrap())
}

/// Waits until the Apply's update waits for the row [`hold_row`] holds. A file lists no
/// statements: its slow update gets a moment to start.
fn until_update_waits(t: &TestTarget, name: &str) {
    if t.lists_running_statements() {
        until_sql(t, &t.row_lock_waits_sql(name), "1", "the Apply's update to wait for the row");
    } else {
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `sql`'s rows, joined by `,`.
fn rows_of(t: &TestTarget, sql: &str) -> String {
    q(t, sql).lines().collect::<Vec<_>>().join(",")
}

/// `t` when `cond` holds, else `f`.
fn yn(cond: &str) -> String {
    format!("CASE WHEN {cond} THEN 't' ELSE 'f' END")
}

fn edits(s: &Session) -> Json {
    s.inspect()["table"]["edits"].clone()
}

fn count(s: &Session) -> u64 {
    edits(s)["count"].as_u64().unwrap_or_else(|| panic!("no edits count in {}", s.inspect()["table"]))
}

/// The staged value of a loaded cell as `inspect()` lists it: `Some(value)` when staged.
fn staged(s: &Session, row: usize, column: usize) -> Option<Json> {
    edits(s)["cells"].as_array().unwrap().iter().find(|c| c["row"] == row && c["column"] == column).map(|c| c["value"].clone())
}

/// The error of a command that is refused before it starts.
fn refused(s: &mut Session, id: &str, params: Json) -> EngineError {
    match s.execute(id, params) {
        Ok(o) => panic!("{id} ran: {o:?}"),
        Err(e) => e,
    }
}

fn why(e: &EngineError) -> String {
    e.data()["why"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn day_scenario_two_changes_one_apply() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        let r = set_cell(s, 0, "name", "Ann");
        assert_eq!((r["staged"].as_bool(), r["count"].as_u64()), (Some(true), Some(1)), "{r}");
        run(s, "cell.clear", json!({ "row": 0, "column": "shipped_at" }));
        assert_eq!(count(s), 2);
        assert_eq!(staged(s, 0, 1), Some(json!("Ann")));
        assert_eq!(staged(s, 0, 3), Some(Json::Null), "clearing a date stages NULL");
        // Nothing is written before Apply.
        assert_eq!(q(t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "Alice");
        let r = apply(s);
        assert_eq!(r["written"], 1, "both changes are one row: {r}");
        assert_eq!(q(t, &format!("SELECT CONCAT(name, '|', {}) FROM {sc}.{name} WHERE id = 1", yn("shipped_at IS NULL"))), "Ann|t");
        // The edits are gone and the page shows what was written.
        assert_eq!(count(s), 0);
        let d = read(s);
        assert_eq!((cell(&d, 0, "name"), cell(&d, 0, "shipped_at")), (Some("Ann"), None));
        assert_eq!(cell(&d, 1, "name"), Some("Bob"));
        assert_eq!(refused(s, "edit.undo", json!({})).code(), "disabled", "writes are not undone");
    });
}

#[test]
fn clearing_rules() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        run(s, "cell.clear", json!({ "row": 0, "column": "total" }));
        assert_eq!(staged(s, 0, 2), Some(Json::Null), "a number clears to NULL");
        run(s, "cell.clear", json!({ "row": 1, "column": "name" }));
        assert_eq!(staged(s, 1, 1), Some(json!("")), "text clears to an empty string");
        run(s, "cell.setNull", json!({ "row": 2, "column": "name" }));
        assert_eq!(staged(s, 2, 1), Some(Json::Null), "Set as NULL stores NULL in text too");
        // Booleans and JSON have no empty value either; by index this time.
        run(s, "cell.clear", json!({ "row": 0, "column": 4 }));
        run(s, "cell.clear", json!({ "row": 0, "column": 5 }));
        assert_eq!((staged(s, 0, 4), staged(s, 0, 5)), (Some(Json::Null), Some(Json::Null)), "{}", t.name());
        assert_eq!(count(s), 5);
        apply(s);
        let cols = ["total IS NULL", "name = ''", "name IS NULL", "flag IS NULL", "doc IS NULL"].map(yn).join(", '|', ");
        let rows = q(t, &format!("SELECT CONCAT(id, '|', {cols}) FROM {sc}.{name} ORDER BY id"));
        assert_eq!(rows, "1|t|f|f|t|t\n2|f|t|f|f|f\n3|f|f|t|t|t", "{}", t.name());
    });
}

#[test]
fn a_json_boolean_stages_the_engines_own_spelling() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        let (yes, no) = s.state().dialect().bool_literals();
        set_cell(s, 1, "flag", true);
        set_cell(s, 2, "flag", false);
        assert_eq!((staged(s, 1, 4), staged(s, 2, 4)), (Some(json!(yes)), Some(json!(no))), "as the engine prints a bool");
        // The loaded value again unstages it; numbers and text stay as given.
        set_cell(s, 0, "flag", true);
        assert_eq!(staged(s, 0, 4), None, "{}", t.name());
        set_cell(s, 0, "total", 12.5);
        set_cell(s, 0, "name", "true");
        assert_eq!((staged(s, 0, 2), staged(s, 0, 1)), (Some(json!("12.5")), Some(json!("true"))));
        apply(s);
        let flag = "CASE WHEN flag IS NULL THEN 'null' WHEN flag THEN 'true' ELSE 'false' END";
        assert_eq!(rows_of(t, &format!("SELECT CONCAT({flag}, ':', name) FROM {sc}.{name} ORDER BY id")), "true:true,true:Bob,false:Cy");
    });
}

#[test]
fn a_json_boolean_keeps_its_text_outside_bool_columns() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        set_cell(s, 0, "name", true);
        set_cell(s, 1, "doc", true);
        assert_eq!(
            (staged(s, 0, 1), staged(s, 1, 5)),
            (Some(json!("true")), Some(json!("true"))),
            "text and JSON take `true`, not the bool spelling"
        );
        apply(s);
        // As text a JSON true is `true`; a JSON string would be `"true"`.
        let doc = s.state().dialect().as_text("doc");
        assert_eq!(q(t, &format!("SELECT CONCAT(name, '|', {doc}) FROM {sc}.{name} WHERE id IN (1, 2) ORDER BY id")), "true|{\"a\": 1}\nBob|true");
    });
}

#[test]
fn failed_apply_writes_nothing_and_marks_the_row() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        set_cell(s, 0, "name", "ok");
        set_cell(s, 1, "shipped_at", "not a date");
        let e = run_err(s, "edit.apply", json!({}));
        assert_eq!(e.code(), "server", "{}: {e}", t.name());
        assert!(e.to_string().contains("nothing was written"), "{e}");
        assert_eq!((e.data()["row"].as_u64(), e.data()["column"].as_u64()), (Some(1), Some(3)), "{}", e.data());
        // Neither change was written: the good one rolled back with the bad one.
        let both = format!("SELECT CONCAT(name, '|', shipped_at) FROM {sc}.{name} WHERE id IN (1, 2) ORDER BY id");
        assert_eq!(q(t, &both), "Alice|2025-01-02\nBob|2025-02-03");
        let table = summary(s);
        assert_eq!((table["failed"]["row"].as_u64(), table["failed"]["column"].as_u64()), (Some(1), Some(3)), "{table}");
        assert!(table["failed"]["message"].as_str().unwrap().to_lowercase().contains("date"), "{table}");
        assert_eq!(table["failed"]["code"], "server", "{table}");
        assert_eq!(count(s), 2, "every edit is kept");
        assert_eq!(table["applying"], Json::Null);
        // Fixing the value and applying again writes both and clears the mark.
        set_cell(s, 1, "shipped_at", "2026-10-20");
        assert_eq!(apply(s)["written"], 2);
        assert_eq!(summary(s)["failed"], Json::Null);
        assert_eq!(q(t, &both), "ok|2025-01-02\nBob|2026-10-20");
    });
}

#[test]
fn add_and_delete_rows() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        assert_eq!(run(s, "row.add", json!({}))["row"], 3);
        assert_eq!(run(s, "row.add", json!({}))["row"], 4);
        set_cell(s, 4, "id", "11");
        // Deleting a new row takes it out; the rows after it move up.
        run(s, "row.delete", json!({ "rows": [3] }));
        assert_eq!(edits(s)["newRows"].as_array().map(Vec::len), Some(1));
        set_cell(s, 3, "id", "10");
        set_cell(s, 3, "name", "new");
        // Deleting a loaded row marks it, and drops the changes staged on it.
        set_cell(s, 1, "name", "gone anyway");
        run(s, "row.delete", json!({ "rows": [1] }));
        let e = edits(s);
        assert_eq!((e["deleted"].clone(), e["cells"].clone(), e["count"].as_u64()), (json!([1]), json!([]), Some(2)), "{e}");
        assert_eq!(e["newRows"], json!([{ "row": 3, "values": [{ "column": 0, "value": "10" }, { "column": 1, "value": "new" }] }]));
        assert_eq!(refused(s, "cell.edit", json!({ "row": 1, "column": "name", "value": "x" })).code(), "badParams", "a deleted row");
        for bad in [json!({ "rows": [4] }), json!({ "rows": "1" }), json!({ "rows": [-1] }), json!({})] {
            assert_eq!(refused(s, "row.delete", bad.clone()).code(), "badParams", "{bad}");
        }
        let r = apply(s);
        assert_eq!(r["written"], 2, "{r}");
        assert_eq!(rows_of(t, &format!("SELECT CONCAT(id, ':', COALESCE(name, '')) FROM {sc}.{name} ORDER BY id")), "1:Alice,3:Cy,10:new");
        // The page loads again with the new row in it.
        let d = read(s);
        assert_eq!(d["rows"].as_array().map(Vec::len), Some(3));
        assert_eq!(cell(&d, 2, "name"), Some("new"));
    });
}

#[test]
fn undo_redo_and_reset() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        assert_eq!(why(&refused(s, "edit.undo", json!({}))), "nothing to undo");
        assert_eq!(why(&refused(s, "edit.redo", json!({}))), "nothing to redo");
        assert_eq!(why(&refused(s, "edit.reset", json!({}))), "no staged changes");
        assert_eq!(why(&refused(s, "edit.apply", json!({}))), "no staged changes");
        set_cell(s, 0, "name", "a");
        set_cell(s, 1, "name", "b");
        set_cell(s, 2, "total", "1");
        assert_eq!(count(s), 3);
        run(s, "edit.undo", json!({}));
        assert_eq!(run(s, "edit.undo", json!({}))["count"], 1);
        assert_eq!(staged(s, 0, 1), Some(json!("a")));
        assert_eq!(edits(s)["canRedo"], true);
        assert_eq!(run(s, "edit.redo", json!({}))["count"], 2);
        assert_eq!(staged(s, 1, 1), Some(json!("b")));
        assert_eq!(run(s, "edit.reset", json!({}))["count"], 0);
        assert_eq!(count(s), 0);
        assert_eq!(edits(s)["canRedo"], false, "a change clears what could be redone");
        // A reset can be undone too.
        assert_eq!(run(s, "edit.undo", json!({}))["count"], 2);
        assert_eq!(run(s, "edit.redo", json!({}))["count"], 0);
        // Undoing everything leaves no staged change, and nothing reached the database.
        while s.execute("edit.undo", json!({})).is_ok() {}
        assert_eq!(count(s), 0);
        assert_eq!(rows_of(t, &format!("SELECT name FROM {sc}.{name} ORDER BY id")), "Alice,Bob,Cy");
    });
}

#[test]
fn editing_value_back_unstages() {
    let _db = require_testdb!();
    on_each_table(|t, _, s| {
        set_cell(s, 0, "name", "x");
        assert_eq!(count(s), 1);
        let r = set_cell(s, 0, "name", "Alice");
        assert_eq!((r["staged"].as_bool(), r["count"].as_u64()), (Some(false), Some(0)));
        assert_eq!(count(s), 0);
        // NULL onto a NULL cell, or the same text again, stages nothing.
        run(s, "cell.setNull", json!({ "row": 2, "column": "shipped_at" }));
        set_cell(s, 2, "doc", Json::Null);
        let total = cell(&read(s), 1, "total").unwrap().to_owned();
        set_cell(s, 1, "total", total);
        assert_eq!(count(s), 0, "{}", t.name());
        // Values are text, a number is taken as its text; other shapes are refused.
        set_cell(s, 1, "total", 21);
        assert_eq!(staged(s, 1, 2), Some(json!("21")));
        for bad in [
            json!({ "row": 0, "column": "name", "value": [1] }),
            json!({ "row": 0, "column": "name" }),
            json!({ "row": 0, "column": "nope", "value": "x" }),
            json!({ "row": 0, "column": 6, "value": "x" }),
            json!({ "row": 3, "column": "name", "value": "x" }),
            json!({ "column": "name", "value": "x" }),
        ] {
            assert_eq!(refused(s, "cell.edit", bad.clone()).code(), "badParams", "{bad}");
        }
        assert_eq!(count(s), 1);
    });
}

#[test]
fn a_nul_in_a_value_is_written_where_the_engine_takes_it() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        let value = "nul\u{0}x";
        if !s.state().dialect().statement_may_hold_nul() {
            let e = refused(s, "cell.edit", json!({ "row": 0, "column": "name", "value": value }));
            assert_eq!(e.code(), "badParams", "{}: {e}", t.name());
            assert!(e.to_string().contains("which this database can't take"), "{}: {e}", t.name());
            assert!(!e.to_string().contains("Postgres"), "{e}");
            let filter = s.execute("table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": value }] }));
            assert_eq!(filter.unwrap_err().code(), "badParams", "{}", t.name());
            return;
        }
        set_cell(s, 0, "name", value);
        let row = run(s, "row.add", json!({}))["row"].as_u64().unwrap() as usize;
        set_cell(s, row, "id", "4");
        set_cell(s, row, "name", "a\\b\u{0}");
        assert_eq!(apply(s)["written"], 2, "{}", t.name());
        assert_eq!(q(t, &format!("SELECT LENGTH(name) FROM {sc}.{name} WHERE id IN (1, 4) ORDER BY id")), "5\n4");
        let d = read(s);
        assert_eq!((cell(&d, 0, "name"), cell(&d, 3, "name")), (Some(value), Some("a\\b\u{0}")), "{}", t.name());
        // A filter finds it by the same value.
        assert_eq!(run(s, "table.setFilters", json!({ "filters": [{ "column": "name", "op": "eq", "value": value }] }))["rows"], 1);
    });
}

#[test]
fn read_only_and_keyless_refuse() {
    let _db = require_testdb!();
    let edit = json!({ "row": 0, "column": 0, "value": "1" });
    for t in engine_targets() {
        {
            let mut s = session();
            let config = dbee_config::ConnectionConfig { read_only: true, ..config_for(&t) };
            open(&mut s, json!({ "config": config, "password": pw() }));
            open_table(&mut s, t.schema("shop"), "customers");
            for (id, params) in [("cell.edit", edit.clone()), ("cell.setNull", edit.clone()), ("cell.clear", edit.clone()), ("row.add", json!({}))] {
                let e = refused(&mut s, id, params);
                assert_eq!((e.code(), why(&e).as_str()), ("disabled", "read-only connection"), "{}: {id}", t.name());
            }
        }
        let mut s = connected_to(&t);
        // A table without a key takes no edits, unless the engine keys its rows by a row id.
        if !t.kind().info().keyless_tables_editable {
            open_table(&mut s, t.schema("shop"), "no_key");
            let e = refused(&mut s, "cell.edit", edit.clone());
            assert_eq!((e.code(), why(&e).as_str()), ("disabled", "no primary key or unique not-null key"));
        }
        open_table(&mut s, t.schema("shop"), "order_totals");
        assert_eq!(why(&refused(&mut s, "row.add", json!({}))), "views are read-only");
        let infos = s.commands();
        for id in ["cell.edit", "cell.setNull", "cell.clear", "row.add", "row.delete", "edit.apply", "edit.reset", "edit.undo", "edit.redo"] {
            let info = infos.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no command {id}"));
            assert!(!info.enabled, "{id}");
        }
        // A query tab is not a table tab.
        run(&mut s, "tab.newQuery", json!({}));
        assert_eq!(why(&refused(&mut s, "cell.edit", edit.clone())), "no table tab is active");
    }
}

#[test]
fn paging_with_staged_edits_is_disabled() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        set_cell(s, 0, "name", "x");
        for (id, params) in [
            ("table.page", json!({ "to": "next" })),
            ("table.sort", json!({ "column": "name" })),
            ("table.refresh", json!({})),
            ("table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "1" }] })),
        ] {
            let e = refused(s, id, params);
            assert_eq!((e.code(), why(&e).as_str()), ("disabled", "apply or reset your changes first"), "{id}");
        }
        // Typing in the filter bar is fine; applying it waits.
        let typed = json!([{ "column": "id", "op": "eq", "value": "2" }]);
        run(s, "table.setFilters", json!({ "filters": typed, "apply": false }));
        assert_eq!(summary(s)["filters"], typed);
        assert_eq!(summary(s)["applied"], "none");
        // Closing the tab asks first.
        let e = refused(s, "tab.close", json!({}));
        assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("stagedEdits")));
        assert!(e.to_string().contains("1 staged change"), "{e}");
        assert_eq!(s.state().tabs.len(), 1);
        // So does disconnecting.
        let e = refused(s, "connection.close", json!({}));
        assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("stagedEdits")));
        // After a reset the bar's filter applies and paging works.
        run(s, "edit.reset", json!({}));
        assert_eq!(run(s, "table.setFilters", json!({}))["rows"], 1);
        assert_eq!(run(s, "table.page", json!({ "to": "next" }))["page"], 0);
        set_cell(s, 0, "name", "y");
        run(s, "tab.close", json!({ "force": true }));
        assert!(s.state().tabs.is_empty());
        assert_eq!(q(t, &format!("SELECT name FROM {sc}.{name} WHERE id = 2")), "Bob", "closing drops the edits unwritten");
    });
}

#[test]
fn second_apply_while_running_is_busy() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        let row = run(s, "row.add", json!({}))["row"].clone();
        run(s, "cell.edit", json!({ "row": row, "column": "id", "value": "10" }));
        let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        assert_eq!(summary(s)["applying"], json!(job.0));
        let e = s.execute("edit.apply", json!({})).unwrap_err();
        assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: apply in progress"));
        // The edits wait for the Apply too.
        assert_eq!(why(&refused(s, "cell.edit", json!({ "row": 0, "column": "name", "value": "x" }))), "apply in progress");
        assert_eq!(why(&refused(s, "edit.reset", json!({}))), "apply in progress");
        assert_eq!(s.wait(job, T).unwrap()["written"], 1);
        assert_eq!(q(t, &format!("SELECT count(*) FROM {sc}.{name} WHERE id = 10")), "1", "written once");
        assert_eq!((count(s), &summary(s)["applying"]), (0, &Json::Null));
    });
}

#[test]
fn editing_one_column_leaves_exotic_types_unchanged() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let name = unique("t_exotic");
        let _drop = scratch_table(&t, &name, &t.copy_table_sql("{t}", &format!("{}.all_types", t.schema("shop"))));
        let mut s = opened(&t, &name);
        // Every column but `note`, as the server's own client prints it.
        let d = s.state().dialect();
        let before = read(&mut s);
        let note = col(&before, "note");
        let names: Vec<&str> = before["columns"].as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).filter(|n| *n != "note").collect();
        let others: Vec<String> = names.iter().map(|n| d.ident(n).unwrap()).collect();
        let others = format!("SELECT {} FROM {sc}.{name} ORDER BY id", others.join(", "));
        let before_server = q(&t, &others);
        for row in 0..3 {
            set_cell(&mut s, row, "note", format!("edited 🐝 it's \"{row}\""));
        }
        assert_eq!(apply(&mut s)["written"], 3);
        let after = read(&mut s);
        for row in 0..3 {
            for (i, c) in before["columns"].as_array().unwrap().iter().enumerate() {
                if i == note {
                    assert_eq!(after["rows"][row][i], json!(format!("edited 🐝 it's \"{row}\"")));
                } else {
                    assert_eq!(after["rows"][row][i], before["rows"][row][i], "{}: row {row}, column {}", t.name(), c["name"]);
                }
            }
        }
        assert_eq!(q(&t, &others), before_server, "{}", t.name());
    }
}

/// MySQL's zero dates, which only its seed holds: editing another column of such a row under the
/// browse session's strict mode writes only that column, and the dates stay as they were.
#[test]
fn editing_one_column_leaves_zero_date_unchanged() {
    let _db = require_testdb!();
    let mut seen = 0;
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let mut s = connected_to(&t);
        let legacy = TableRef { schema: "shop".into(), name: "legacy_dates".into() };
        if s.state().connected.as_ref().unwrap().catalog.table(&legacy).is_none() {
            continue;
        }
        seen += 1;
        let name = unique("t_zero");
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {sc}.{name}")]);
        // The server's loose mode lets the copy take the zero dates, as the seed's did.
        q(
            &t,
            &format!(
                "SET SESSION sql_mode = ''; CREATE TABLE {sc}.{name} LIKE shop.legacy_dates; INSERT INTO {sc}.{name} SELECT * FROM shop.legacy_dates"
            ),
        );
        run(&mut s, "schema.refresh", json!({}));
        assert_eq!(open_table(&mut s, sc, &name)["rows"], 3);
        let shown = |s: &mut TestSession| {
            let d = read(s);
            (0..2).map(|i| (cell(&d, i, "d").map(str::to_owned), cell(&d, i, "dt").map(str::to_owned))).collect::<Vec<_>>()
        };
        let before = shown(&mut s);
        assert_eq!(before[0], (Some("0000-00-00".to_owned()), Some("0000-00-00 00:00:00".to_owned())), "{}", t.name());
        set_cell(&mut s, 0, "name", "edited");
        set_cell(&mut s, 1, "name", "also");
        assert_eq!(apply(&mut s)["written"], 2, "{}", t.name());
        let rows = q(&t, &format!("SELECT CONCAT(id, '|', name, '|', CAST(d AS CHAR), '|', CAST(dt AS CHAR)) FROM {sc}.{name} ORDER BY id"));
        assert_eq!(
            rows,
            "1|edited|0000-00-00|0000-00-00 00:00:00\n2|also|2025-01-00|2025-01-00 00:00:00\n3|ordinary|2025-01-02|2025-01-02 03:04:05",
            "{}",
            t.name()
        );
        assert_eq!(shown(&mut s), before, "{}: the dates print as before", t.name());
    }
    assert!(seen > 0, "the MySQL seed's servers have shop.legacy_dates");
}

#[test]
fn odd_table_edit_round_trip() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let odd = t.kind().info().odd_table;
        let name = format!("{} {odd}", unique("t_odd"));
        let mut s = connected_to(&t);
        let d = s.state().dialect();
        let quoted = d.table(&TableRef { schema: sc.into(), name: name.clone() }).unwrap();
        let source = d.table(&TableRef { schema: t.schema("odd").into(), name: odd.into() }).unwrap();
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {quoted}")]);
        q(&t, &t.copy_table_sql(&quoted, &source));
        run(&mut s, "schema.refresh", json!({}));
        assert_eq!(open_table(&mut s, sc, &name)["rows"], 4);
        set_cell(&mut s, 0, "lower UPPER", "it's \"edited\" 🐝");
        set_cell(&mut s, 1, "Total $", "99.5");
        let row = run(&mut s, "row.add", json!({}))["row"].as_u64().unwrap() as usize;
        set_cell(&mut s, row, "id", "7");
        set_cell(&mut s, row, "lower UPPER", "new 🐝");
        run(&mut s, "row.delete", json!({ "rows": [3] }));
        assert_eq!(apply(&mut s)["written"], 4);
        let (lower, total) = (d.ident("lower UPPER").unwrap(), d.as_text(&d.ident("Total $").unwrap()));
        let got = q(&t, &format!("SELECT CONCAT(id, '|', COALESCE({lower}, '-'), '|', COALESCE({total}, '-')) FROM {quoted} ORDER BY id"));
        let got: Vec<(String, String, Option<f64>)> = got
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('|').collect();
                (f[0].to_owned(), f[1].to_owned(), f[2].parse().ok())
            })
            .collect();
        let want = [("1", "it's \"edited\" 🐝", Some(0.5)), ("2", "Beta", Some(99.5)), ("3", "gamma 🐝", Some(100.0)), ("7", "new 🐝", None)];
        assert_eq!(got, want.map(|(a, b, c)| (a.to_owned(), b.to_owned(), c)), "{}", t.name());
    }
}

/// The message `failed` holds after an Apply whose outcome is unknown.
const UNKNOWN: &str = "connection lost during Apply — the outcome is unknown. Reset and reload to check what was written, or Apply again to retry";

#[test]
fn connection_lost_during_apply_keeps_the_edits() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file cannot lose its connection.
        if t.is_file() {
            continue;
        }
        let sc = t.schema("scratch");
        let (name, mut scratch) = make_table(&t);
        let lock = hold_row(&t, &mut scratch, &name, 1);
        let mut s = connected_exclusive_to(&t);
        open_table(&mut s, sc, &name);
        set_cell(&mut s, 0, "name", "lost");
        let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        until_update_waits(&t, &name);
        kill_dbee_connections(&t, &s);
        let e = s.wait(job, T).unwrap_err();
        assert_eq!(e.code(), "connectionLost", "{}: {e}", t.name());
        assert_eq!(e.data()["action"], "connection.reconnect");
        assert_eq!(e.to_string(), UNKNOWN);
        let table = summary(&s);
        assert_eq!((table["failed"]["row"].as_u64(), table["failed"]["message"].as_str()), (Some(0), Some(UNKNOWN)), "{table}");
        assert_eq!(count(&s), 1, "the edit is kept");
        assert_eq!(table["outcomeUnknown"], true, "{table}");
        assert_eq!(s.inspect()["connection"]["lost"], true);
        assert_eq!(why(&refused(&mut s, "edit.apply", json!({}))), "connection lost — reconnect");
        // Reset, undo and redo need no database: the edits can always be dropped.
        let infos = s.commands();
        let enabled = |id: &str| infos.iter().find(|c| c.id == id).is_some_and(|c| c.enabled);
        assert!(enabled("edit.reset") && enabled("edit.undo"), "lost: reset and undo stay enabled");
        assert!(!enabled("cell.edit") && !enabled("edit.apply"));
        drop(lock);
        // Nothing was retried, and the server rolled the transaction back with the connection.
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "Alice");
        run(&mut s, "connection.reconnect", json!({}));
        assert_eq!(count(&s), 1);
        // Applying again could write twice: it asks first.
        let e = refused(&mut s, "edit.apply", json!({}));
        assert_eq!((e.code(), e.data()["what"].as_str(), e.to_string().as_str()), ("needsConfirm", Some("applyAfterUnknownOutcome"), UNKNOWN));
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "Alice");
        assert_eq!(run(&mut s, "edit.apply", json!({ "force": true }))["written"], 1);
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "lost");
    }
}

#[test]
fn reconnect_during_apply_marks_the_outcome_unknown() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let (name, mut scratch) = make_table(&t);
        let lock = hold_row(&t, &mut scratch, &name, 2);
        let mut s = opened(&t, &name);
        set_cell(&mut s, 1, "name", "cut off");
        let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        until_update_waits(&t, &name);
        run(&mut s, "connection.reconnect", json!({}));
        // The Apply ran on the connection the reconnect let go: its result will be ignored, so the
        // tab cannot know what it wrote.
        let table = summary(&s);
        assert_eq!(table["applying"], Json::Null);
        assert_eq!(table["failed"]["row"], 1, "{table}");
        assert!(table["failed"]["message"].as_str().unwrap().contains("the outcome is unknown"), "{table}");
        assert_eq!(count(&s), 1);
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }));
        assert_eq!(count(&s), 1);
        drop(lock);
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 2")), "Bob", "{}: the update never wrote", t.name());
        assert_eq!(table["failed"]["message"], UNKNOWN);
        assert_eq!((&table["outcomeUnknown"], &read(&mut s)["outcomeUnknown"]), (&json!(true), &json!(true)), "the UI can see why Apply asks");
        let e = refused(&mut s, "edit.apply", json!({}));
        assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("applyAfterUnknownOutcome")));
        // A reset clears the mark, but the doubt stays until the page loads again: the same edits
        // brought back by an undo still ask.
        run(&mut s, "edit.reset", json!({}));
        assert_eq!((&summary(&s)["failed"], &summary(&s)["outcomeUnknown"]), (&Json::Null, &json!(true)));
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(count(&s), 1);
        assert_eq!(refused(&mut s, "edit.apply", json!({})).code(), "needsConfirm");
        // Reset and reload: what was written shows, and new edits apply without asking.
        run(&mut s, "edit.reset", json!({}));
        run(&mut s, "table.refresh", json!({}));
        let d = read(&mut s);
        assert_eq!((cell(&d, 1, "name"), &d["outcomeUnknown"]), (Some("Bob"), &json!(false)));
        set_cell(&mut s, 1, "name", "again");
        assert_eq!(apply(&mut s)["written"], 1);
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 2")), "again");
    }
}

#[test]
fn table_cancel_never_cancels_an_apply() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let (name, mut scratch) = make_table(&t);
        let lock = hold_row(&t, &mut scratch, &name, 1);
        let mut s = opened(&t, &name);
        set_cell(&mut s, 0, "name", "kept");
        let Outcome::Started(apply) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        until_update_waits(&t, &name);
        // Another tab's page load waits behind the Apply on the browse connection; cancelling it
        // would cancel the Apply's statement instead.
        let Outcome::Started(load) = s.execute("table.open", json!({ "schema": sc, "table": name })).unwrap() else {
            panic!("table.open starts a job")
        };
        let e = refused(&mut s, "table.cancel", json!({}));
        assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: apply in progress"));
        drop(lock);
        assert_eq!(s.wait(apply, T).unwrap()["written"], 1, "{}", t.name());
        assert_eq!(s.wait(load, T).unwrap()["rows"], 3);
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "kept");
    }
}

#[test]
fn reset_undo_redo_work_when_the_tab_is_no_longer_editable() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        set_cell(s, 0, "name", "a");
        set_cell(s, 1, "name", "b");
        q(t, &format!("DROP TABLE {sc}.{name}"));
        run(s, "schema.refresh", json!({}));
        let gone = "the table is no longer in the schema";
        assert_eq!(summary(s)["notEditableReason"], gone);
        assert_eq!(why(&refused(s, "edit.apply", json!({}))), gone, "Apply stays refused");
        assert_eq!(why(&refused(s, "cell.edit", json!({ "row": 2, "column": "name", "value": "c" }))), gone);
        assert_eq!(run(s, "edit.undo", json!({}))["count"], 1);
        assert_eq!(run(s, "edit.redo", json!({}))["count"], 2);
        assert_eq!(run(s, "edit.reset", json!({}))["count"], 0);
        // Nothing staged: the tab closes without asking.
        run(s, "tab.close", json!({}));
        assert!(s.state().tabs.is_empty());
    });
}

#[test]
fn edits_follow_the_tab_they_were_made_in() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let first = s.inspect()["active"].clone();
        set_cell(s, 0, "name", "first tab");
        open_table(s, t.schema("scratch"), name);
        assert_eq!(count(s), 0, "a new tab has its own edits");
        set_cell(s, 2, "name", "second tab");
        assert_eq!(run(s, "edit.undo", json!({}))["count"], 0);
        run(s, "tab.activate", json!({ "tab": first }));
        assert_eq!(staged(s, 0, 1), Some(json!("first tab")));
        let Some(Tab::Table(tab)) = s.state().active_tab() else { panic!("a table tab is active") };
        assert_eq!(tab.edits.count(), 1);
        let data = tab.data.clone().unwrap();
        assert_eq!(tab.edits.shown(&data, 0, 1), Some(Some("first tab")));
        assert_eq!(tab.edits.shown(&data, 1, 1), None);
        assert!(tab.has_staged_edits());
    });
}

/// Values longer than a column's length are refused by the server and written nowhere, never cut
/// to fit; a filter with such a value matches nothing instead of the rows its cut prefix would.
#[test]
fn over_long_values_are_refused_not_cut() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // An engine that keeps values of any length has nothing to refuse.
        if !t.kind().info().enforces_lengths {
            continue;
        }
        let sc = t.schema("scratch");
        let name = unique("t_long");
        let _drop = scratch_table(
            &t,
            &name,
            "CREATE TABLE {t} (id int PRIMARY KEY, v varchar(10), c char(2)); INSERT INTO {t} VALUES (1, 'abcdefghij', 'ab');",
        );
        let row = || q(&t, &format!("SELECT CONCAT(id, '|', v, '|', c) FROM {sc}.{name} ORDER BY id"));
        let before = row();
        let mut s = opened_rows(&t, &name, 1);
        let on = t.name();
        for (column, value, matches) in
            [("v", "abcdefghijkl", 0), ("v", "abcdefghij", 1), ("c", "abc", 0), ("c", "ab", 1), ("v", "abcdefghijkl, zzz", 0)]
        {
            let op = if value.contains(',') { "in" } else { "eq" };
            let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": column, "op": op, "value": value }] }));
            assert_eq!(r["rows"], matches, "{on}: {column} {op} {value:?}: {r}");
        }
        run(&mut s, "table.setFilters", json!({ "filters": [] }));
        for (column, value) in [("v", "abcdefghijkl"), ("c", "abc")] {
            set_cell(&mut s, 0, column, value);
            let e = run_err(&mut s, "edit.apply", json!({}));
            assert_eq!(e.code(), "server", "{on}: {column} = {value:?}: {e}");
            assert!(e.to_string().contains("nothing was written"), "{on}: {e}");
            assert_eq!(row(), before, "{on}: {column} = {value:?} wrote nothing");
            let failed = summary(&s)["failed"].clone();
            let index = col(&read(&mut s), column);
            assert_eq!((failed["row"].as_u64(), failed["column"].as_u64()), (Some(0), Some(index as u64)), "{on}: {column}: {failed}");
            run(&mut s, "edit.reset", json!({}));
        }
        // A new row is an INSERT: the same.
        let new = run(&mut s, "row.add", json!({}))["row"].as_u64().unwrap() as usize;
        set_cell(&mut s, new, "id", "2");
        set_cell(&mut s, new, "c", "abc");
        assert_eq!(run_err(&mut s, "edit.apply", json!({})).code(), "server", "{on}");
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "1", "{on}: no row was added");
        run(&mut s, "edit.reset", json!({}));
        // Values that fit are written.
        set_cell(&mut s, 0, "v", "xyz");
        set_cell(&mut s, 0, "c", "x");
        assert_eq!(apply(&mut s)["written"], 1, "{on}");
        assert!(row().starts_with("1|xyz|x"), "{on}: {}", row());
    }
}

#[test]
fn table_cancel_stops_an_apply_that_waits_for_a_lock() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        // A file has no other session to hold a lock.
        let Some(server) = t.server() else { continue };
        let (name, _drop) = make_table(&t);
        let mut s = opened(&t, &name);
        // Another session holds row 1, so the Apply's first UPDATE waits.
        let lock = dbee_testkit::hold(server, &format!("SELECT id FROM {sc}.{name} WHERE id = 1 FOR UPDATE")).unwrap();
        set_cell(&mut s, 0, "name", "waited");
        set_cell(&mut s, 1, "name", "also");
        let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        let waiting = t.row_lock_waits_sql(&name);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while q(&t, &waiting) != "1" {
            assert!(std::time::Instant::now() < deadline, "{}: gave up waiting for the Apply to wait for the lock", t.name());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(run(&mut s, "table.cancel", json!({})), json!({ "sent": true, "apply": true }));
        let e = s.wait(job, T).unwrap_err();
        assert_eq!(e.code(), "cancelled", "{}: {e}", t.name());
        assert_eq!(e.to_string(), "the Apply was cancelled at change 1, so nothing was written");
        let table = summary(&s);
        assert_eq!((table["failed"]["row"].as_u64(), table["failed"]["message"].as_str()), (Some(0), Some(e.to_string().as_str())), "{table}");
        assert_eq!(table["failed"]["code"], "cancelled", "{table}");
        assert_eq!((count(&s), &table["applying"], &table["outcomeUnknown"]), (2, &Json::Null, &json!(false)), "every edit is kept: {table}");
        assert_eq!(s.inspect()["connection"]["lost"], false);
        drop(lock);
        assert_eq!(rows_of(&t, &format!("SELECT name FROM {sc}.{name} ORDER BY id")), "Alice,Bob,Cy", "nothing was written");
        // With the lock gone, the same edits apply on the same connection.
        assert_eq!(apply(&mut s)["written"], 2);
        assert_eq!(rows_of(&t, &format!("SELECT name FROM {sc}.{name} ORDER BY id")), "waited,also,Cy");
    }
}

/// A cancel that would meet the Apply's COMMIT is refused, so no cancel request reaches it: a COMMIT
/// it stopped could have written or not.
#[test]
fn table_cancel_waits_for_an_apply_that_is_committing() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        // A COMMIT held from outside: not on a file, nor on MySQL, which has no lock that holds a
        // COMMIT but not the update before it (MariaDB's backup stage stands in for the family).
        if !t.can_hold_commit() {
            continue;
        }
        let (name, _drop) = make_table(&t);
        // Exclusive first: on MariaDB the gate holds every COMMIT and DDL on the server, so it is
        // taken only once no other test's session can be waiting on one.
        let mut s = connected_exclusive_to(&t);
        let mut gate = t.commit_gate(&name).unwrap();
        open_table(&mut s, sc, &name);
        set_cell(&mut s, 0, "name", "committed");
        let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
        gate.until_committing().unwrap();
        let e = refused(&mut s, "table.cancel", json!({}));
        assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: the changes are being committed"), "{}", t.name());
        // A cancel request sent now would reach the waiting COMMIT well within this.
        std::thread::sleep(Duration::from_millis(500));
        drop(gate);
        assert_eq!(s.wait(job, T).unwrap()["written"], 1, "{}: no cancel request reached the COMMIT", t.name());
        assert_eq!(q(&t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "committed");
        assert_eq!(count(&s), 0);
    }
}

#[test]
fn a_page_that_does_not_load_after_the_write_says_the_changes_were_written() {
    let _db = require_testdb!();
    on_each_table(|t, name, s| {
        let sc = t.schema("scratch");
        // A condition that holds while one row is named Ann or Bob, and fails on every engine once
        // two are.
        let raw = t.failing_when_sql(&format!("(SELECT count(*) FROM {sc}.{name} WHERE name IN ('Ann', 'Bob')) > 1"));
        assert_eq!(run(s, "table.setFilters", json!({ "raw": raw, "rawMode": true }))["rows"], 3);
        set_cell(s, 0, "name", "Ann");
        let r = apply(s);
        assert_eq!(r["written"], 1, "{r}");
        assert!(r["reloadError"].as_str().is_some_and(|e| e.starts_with(t.kind().info().failing_error)), "{}: {r}", t.name());
        assert_eq!(q(t, &format!("SELECT name FROM {sc}.{name} WHERE id = 1")), "Ann");
        let table = summary(s);
        let error = table["error"].as_str().unwrap_or_default();
        assert!(error.starts_with("The changes were written (1 row), but the page did not load again: "), "{table}");
        assert!(error.ends_with(r["reloadError"].as_str().unwrap_or_default()), "{table}");
        assert_eq!((count(s), &table["failed"]), (0, &Json::Null), "the edits went with the write: {table}");
    });
}

#[test]
fn generated_columns_cannot_be_edited() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let sc = t.schema("scratch");
        let name = unique("t_generated");
        let _drop = scratch_table(
            &t,
            &name,
            "CREATE TABLE {t} (id int PRIMARY KEY, a int, twice int GENERATED ALWAYS AS (a * 2) STORED); INSERT INTO {t} (id, a) VALUES (1, 1), (2, 2);",
        );
        let mut s = opened_rows(&t, &name, 2);
        let new = run(&mut s, "row.add", json!({}))["row"].as_u64().unwrap();
        for row in [0, new] {
            for id in ["cell.edit", "cell.setNull", "cell.clear"] {
                let e = refused(&mut s, id, json!({ "row": row, "column": "twice", "value": "9" }));
                assert_eq!(
                    (e.code(), e.to_string()),
                    ("badParams", format!("bad params for `{id}`: generated columns are read-only")),
                    "{}: row {row}",
                    t.name()
                );
            }
        }
        // The other columns edit and apply; the engine fills in the computed one.
        set_cell(&mut s, 0, "a", "7");
        set_cell(&mut s, new as usize, "id", "3");
        set_cell(&mut s, new as usize, "a", "5");
        assert_eq!(apply(&mut s)["written"], 2);
        assert_eq!(rows_of(&t, &format!("SELECT CONCAT(id, ':', a, ':', twice) FROM {sc}.{name} ORDER BY id")), "1:7:14,2:2:4,3:5:10");
    }
}

/// Postgres's own relations: materialized views and foreign tables.
#[test]
fn materialized_views_and_foreign_tables_say_what_they_are() {
    let _db = require_testdb!();
    let t = pg14_target();
    // A foreign table on a wrapper without a handler: it is in the catalog, and reading it fails.
    let fdw = unique("dbee_fdw");
    let _drop = Scratch(&t, vec![format!("FOREIGN DATA WRAPPER IF EXISTS {fdw} CASCADE")]);
    q(
        &t,
        &format!(
            "CREATE FOREIGN DATA WRAPPER {fdw}; CREATE SERVER {fdw}_s FOREIGN DATA WRAPPER {fdw}; CREATE FOREIGN TABLE scratch.{fdw}_t (id int) SERVER {fdw}_s;"
        ),
    );
    let mut s = connected();
    open_table(&mut s, "shop", "customer_counts");
    assert_eq!(why(&refused(&mut s, "row.add", json!({}))), "materialized views are read-only");
    assert_eq!(summary(&s)["notEditableReason"], "materialized views are read-only");
    let e = run_err(&mut s, "table.open", json!({ "schema": "scratch", "table": format!("{fdw}_t") }));
    assert_eq!(e.code(), "server", "the wrapper has no handler: {e}");
    assert_eq!(summary(&s)["notEditableReason"], "foreign tables are read-only");
    assert_eq!(why(&refused(&mut s, "row.add", json!({}))), "foreign tables are read-only");
}

/// Postgres's identity columns: GENERATED ALWAYS ones are read-only, BY DEFAULT ones are not.
#[test]
fn columns_postgres_computes_cannot_be_edited() {
    let _db = require_testdb!();
    let t = pg14_target();
    let name = unique("t_generated");
    let _drop = scratch_table(
        &t,
        &name,
        "CREATE TABLE {t} (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, a int, twice int GENERATED ALWAYS AS (a * 2) STORED,
             d int GENERATED BY DEFAULT AS IDENTITY);
         INSERT INTO {t} (a) VALUES (1), (2);",
    );
    let mut s = opened_rows(&t, &name, 2);
    let new = run(&mut s, "row.add", json!({}))["row"].as_u64().unwrap();
    for row in [0, new] {
        for (column, why) in [("twice", "generated columns are read-only"), ("id", "identity columns GENERATED ALWAYS are read-only")] {
            for id in ["cell.edit", "cell.setNull", "cell.clear"] {
                let e = refused(&mut s, id, json!({ "row": row, "column": column, "value": "9" }));
                assert_eq!((e.code(), e.to_string()), ("badParams", format!("bad params for `{id}`: {why}")), "row {row}");
            }
        }
    }
    // The other columns edit and apply; Postgres fills in the computed ones.
    set_cell(&mut s, 0, "a", "7");
    set_cell(&mut s, 1, "d", "40");
    set_cell(&mut s, new as usize, "a", "5");
    assert_eq!(apply(&mut s)["written"], 3);
    assert_eq!(rows_of(&t, &format!("SELECT CONCAT(id, ':', a, ':', twice, ':', d) FROM scratch.{name} ORDER BY id")), "1:7:14:1,2:2:4:40,3:5:10:3");
}

/// Drops the scratch objects it names on `server` when the test ends, also when it fails.
struct ScratchOn(&'static dbee_testkit::Server, Vec<String>);

impl Drop for ScratchOn {
    fn drop(&mut self) {
        for what in &self.1 {
            let _ = dbee_testkit::psql(self.0, &format!("DROP {what} CASCADE"));
        }
    }
}

/// Postgres types: values longer (or, for `bit(n)`, shorter) than a column's length are refused by
/// the server and written nowhere, never cut to fit: on scalars, arrays and a domain, on every
/// Postgres server. A filter with such a value matches nothing instead of the rows its cut prefix
/// would match.
#[test]
fn values_that_do_not_fit_the_length_are_refused_not_cut() {
    let db = require_testdb!();
    for server in db.servers_of(&[dbee_testkit::Kind::Postgres]) {
        let server: &'static dbee_testkit::Server = server;
        let t = unique("t_len");
        let domain = format!("{t}_d5");
        let _drop = ScratchOn(server, vec![format!("TABLE IF EXISTS scratch.{t}"), format!("DOMAIN IF EXISTS scratch.{domain}")]);
        let q = |sql: &str| dbee_testkit::psql(server, sql).unwrap().trim().to_owned();
        q(&format!(
            "CREATE DOMAIN scratch.{domain} AS varchar(5);
             CREATE TABLE scratch.{t} (id int PRIMARY KEY, v varchar(10), c char(2), b bit(2), vb varbit(2), d scratch.{domain},
                 va varchar(3)[], ca char(2)[], ba bit(2)[], vba varbit(2)[]);
             INSERT INTO scratch.{t} VALUES (1, 'abcdefghij', 'ab', '10', '10', 'abcde', '{{abc}}', '{{ab}}', '{{10}}', '{{10}}');"
        ));
        let row = || q(&format!("SELECT (v, c, b, vb, d, va, ca, ba, vba)::text FROM scratch.{t} ORDER BY id"));
        let before = row();
        let mut s = session();
        open(&mut s, json!({ "config": server_config(server), "password": pw() }));
        assert_eq!(open_table(&mut s, "scratch", &t)["rows"], 1);
        let at = |s: &mut Session, name: &str| col(&read(s), name);
        let on = &server.name;

        // Filters: a value that does not fit matches nothing; the value itself matches.
        for (column, op, value, matches) in [
            ("v", "eq", "abcdefghijkl", 0),
            ("v", "eq", "abcdefghij", 1),
            ("v", "in", "abcdefghijkl, zzz", 0),
            ("c", "eq", "abc", 0),
            ("c", "ge", "abc", 0),
            ("c", "eq", "ab", 1),
            ("c", "in", "abc", 0),
            ("b", "eq", "101", 0),
            ("b", "eq", "1", 0),
            ("b", "eq", "10", 1),
            ("b", "in", "101, 1", 0),
            ("vb", "eq", "101", 0),
            ("vb", "eq", "10", 1),
            ("d", "eq", "abcdefg", 0),
            ("d", "eq", "abcde", 1),
            ("vba", "eq", "{101}", 0),
            ("vba", "eq", "{10}", 1),
            ("va", "eq", "{abc}", 1),
            ("ca", "eq", "{ab}", 1),
            ("ba", "eq", "{10}", 1),
        ] {
            let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": column, "op": op, "value": value }] }));
            assert_eq!(r["rows"], matches, "{on}: {column} {op} {value:?}: {r}");
        }
        run(&mut s, "table.setFilters", json!({ "filters": [] }));

        // Edits: each value that does not fit fails the Apply, which writes nothing and marks the cell.
        for (column, value, sqlstate) in [
            ("v", "abcdefghijkl", "22001"),
            ("c", "abc", "22001"),
            ("b", "101", "22026"),
            ("b", "1", "22026"),
            ("vb", "101", "22001"),
            ("d", "abcdefg", "22001"),
            ("va", "{abcd}", "22001"),
            ("ca", "{abc}", "22001"),
            ("ba", "{101}", "22026"),
            ("vba", "{101}", "22001"),
        ] {
            run(&mut s, "cell.edit", json!({ "row": 0, "column": column, "value": value }));
            let e = run_err(&mut s, "edit.apply", json!({}));
            assert_eq!((e.code(), e.data()["serverCode"].as_str()), ("server", Some(sqlstate)), "{on}: {column} = {value:?}: {e}");
            assert!(e.to_string().contains("nothing was written"), "{on}: {e}");
            assert_eq!(row(), before, "{on}: {column} = {value:?} wrote nothing");
            let failed = s.inspect()["table"]["failed"].clone();
            let index = at(&mut s, column);
            assert_eq!((failed["row"].as_u64(), failed["column"].as_u64()), (Some(0), Some(index as u64)), "{on}: {column}: {failed}");
            run(&mut s, "edit.reset", json!({}));
        }
        // A new row is an INSERT: the same.
        let new = run(&mut s, "row.add", json!({}))["row"].clone();
        run(&mut s, "cell.edit", json!({ "row": new, "column": "id", "value": "2" }));
        run(&mut s, "cell.edit", json!({ "row": new, "column": "c", "value": "abc" }));
        let e = run_err(&mut s, "edit.apply", json!({}));
        assert_eq!((e.code(), e.data()["serverCode"].as_str()), ("server", Some("22001")), "{on}: {e}");
        assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t}")), "1", "{on}: no row was added");
        run(&mut s, "edit.reset", json!({}));

        // Values that fit are written, and the page shows them.
        for (column, value) in
            [("v", "xyz"), ("c", "x"), ("b", "01"), ("vb", "0"), ("d", "xy"), ("va", "{x,yz}"), ("ca", "{x}"), ("ba", "{01,11}"), ("vba", "{0}")]
        {
            run(&mut s, "cell.edit", json!({ "row": 0, "column": column, "value": value }));
        }
        assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 1, "{on}");
        assert_eq!(row(), "(xyz,\"x \",01,0,xy,\"{x,yz}\",\"{\"\"x \"\"}\",\"{01,11}\",{0})", "{on}");
        let d = read(&mut s);
        let shown: Vec<Option<&str>> = ["v", "c", "b", "vb", "d", "va", "ca", "ba", "vba"].iter().map(|c| cell(&d, 0, c)).collect();
        assert_eq!(
            shown,
            [Some("xyz"), Some("x "), Some("01"), Some("0"), Some("xy"), Some("{x,yz}"), Some("{\"x \"}"), Some("{01,11}"), Some("{0}")],
            "{on}"
        );
    }
}

/// Postgres types: filters compare a `char(n)`, `bit(n)` or array value as the column's own type without its
/// length: an over-long value or element matches nothing, trailing spaces count as Postgres's
/// `bpchar` comparison counts them (not at all), and an index on the column serves the filter.
#[test]
fn filters_on_length_types_compare_as_postgres_does() {
    let db = require_testdb!();
    for server in db.servers_of(&[dbee_testkit::Kind::Postgres]) {
        let server: &'static dbee_testkit::Server = server;
        let t = unique("t_lenf");
        let domain = format!("{t}_d5");
        let _drop = ScratchOn(server, vec![format!("TABLE IF EXISTS scratch.{t}"), format!("DOMAIN IF EXISTS scratch.{domain}")]);
        let q = |sql: &str| dbee_testkit::psql(server, sql).unwrap().trim().to_owned();
        q(&format!(
            "CREATE DOMAIN scratch.{domain} AS varchar(5);
             CREATE TABLE scratch.{t} (id int PRIMARY KEY, c char(3), b bit(2), d scratch.{domain}, va varchar(3)[], ca char(2)[], ba bit(2)[]);
             CREATE INDEX ON scratch.{t} (c);
             CREATE INDEX ON scratch.{t} (b);
             INSERT INTO scratch.{t} VALUES (1, 'ab', '10', 'abcde', '{{abc}}', '{{ab}}', '{{10}}'), (2, 'xyz', '01', 'x', '{{x}}', '{{y}}', '{{01}}');"
        ));
        let mut s = session();
        open(&mut s, json!({ "config": server_config(server), "password": pw() }));
        assert_eq!(open_table(&mut s, "scratch", &t)["rows"], 2);
        let on = &server.name;
        for (column, op, value, matches) in [
            // char(3): 'ab' is stored as 'ab '; trailing spaces do not count either way.
            ("c", "eq", "ab", 1),
            ("c", "eq", "ab ", 1),
            ("c", "eq", "abcd", 0),
            ("c", "lt", "b", 1),
            ("c", "in", "abcd, xyz", 1),
            ("b", "eq", "100", 0),
            ("b", "eq", "1", 0),
            ("b", "eq", "10", 1),
            ("d", "eq", "abcdefg", 0),
            ("d", "eq", "abcde", 1),
            // An array element longer than the length matches nothing; the value itself matches.
            ("va", "eq", "{abcd}", 0),
            ("va", "eq", "{abc}", 1),
            ("va", "lt", "{b}", 1),
            ("va", "in", "{abcd}", 0),
            ("ca", "eq", "{abc}", 0),
            ("ca", "eq", "{ab}", 1),
            ("ba", "eq", "{101}", 0),
            ("ba", "eq", "{10}", 1),
        ] {
            let r = run(&mut s, "table.setFilters", json!({ "filters": [{ "column": column, "op": op, "value": value }] }));
            assert_eq!(r["rows"], matches, "{on}: {column} {op} {value:?}: {r}");
        }
        // The condition dbee builds lets Postgres use the column's index.
        let info =
            s.state().connected.as_ref().unwrap().catalog.table(&dbee_core::TableRef { schema: "scratch".into(), name: t.clone() }).cloned().unwrap();
        for (column, value) in [("c", "ab"), ("b", "10")] {
            let filter = dbee_sql::filter::Filter { column: column.into(), op: dbee_sql::filter::Op::Eq, value: value.into() };
            let cond = dbee_sql::filter::where_sql(dbee_sql::Dialect::Postgres, &dbee_sql::filter::Condition::Filters(vec![filter]), &info)
                .unwrap()
                .unwrap();
            let plan = q(&format!("SET enable_seqscan = off; EXPLAIN (COSTS OFF) SELECT id FROM scratch.{t} WHERE {cond}"));
            assert!(plan.contains("Index") && !plan.contains("Seq Scan"), "{on}: {cond}: {plan}");
        }
    }
}

/// The names of a `table.read` result's columns.
fn column_names(read: &Json) -> Vec<String> {
    read["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap_or_default().to_owned()).collect()
}

#[test]
fn sqlite_table_without_key_is_edited_by_rowid() {
    let _db = require_testdb!();
    for_sqlite(|t, s| {
        let main = t.schema("shop");
        open_table(s, main, "notes_nokey");
        let d = read(s);
        assert_eq!(column_names(&d), ["title", "body"], "the rowid is not a column of the tab");
        assert_eq!((d["editable"].as_bool(), &summary(s)["noEditReason"]), (Some(true), &Json::Null));
        // Rows 1 and 2 are alike but for their rowid: each is edited on its own.
        set_cell(s, 1, "body", "changed");
        assert_eq!(apply(s)["written"], 1);
        let all = format!("SELECT rowid, title, body FROM {main}.notes_nokey ORDER BY rowid");
        assert_eq!(q(t, &all), "1|first|a note\n2|second|changed\n3|second|another note");
        assert_eq!(cell(&read(s), 1, "body"), Some("changed"));
        run(s, "row.delete", json!({ "rows": [2] }));
        assert_eq!(apply(s)["written"], 1);
        assert_eq!(q(t, &all), "1|first|a note\n2|second|changed");
        // A new row is written without a rowid; the page reads its rowid back, so it edits too.
        let row = run(s, "row.add", json!({}))["row"].as_u64().unwrap() as usize;
        set_cell(s, row, "title", "third");
        assert_eq!(apply(s)["written"], 1);
        let d = read(s);
        assert_eq!((rows(&d), cell(&d, 2, "title"), cell(&d, 2, "body")), (3, Some("third"), None));
        set_cell(s, 2, "body", "edited again");
        assert_eq!(apply(s)["written"], 1);
        assert_eq!(q(t, &format!("SELECT title, body FROM {main}.notes_nokey ORDER BY rowid")), "first|a note\nsecond|changed\nthird|edited again");
        // The key is no column to edit, sort or filter by.
        for (id, params) in [
            ("cell.edit", json!({ "row": 0, "column": "rowid", "value": "9" })),
            ("table.sort", json!({ "column": "rowid" })),
            ("table.setFilters", json!({ "filters": [{ "column": "rowid", "op": "eq", "value": "1" }] })),
        ] {
            assert_eq!(refused(s, id, params).code(), "badParams", "{id}");
        }
    });
}

#[test]
fn rowid_alias_skips_a_real_rowid_column() {
    let _db = require_testdb!();
    for_sqlite(|t, s| {
        let main = t.schema("odd");
        open_table(s, main, "rowid_taken");
        let d = read(s);
        assert_eq!(column_names(&d), ["rowid", "note"], "the real rowid column is shown, the key is not");
        // The real column edits, sorts and filters like any other; the rows are found by `_rowid_`.
        set_cell(s, 0, "rowid", "R1");
        set_cell(s, 1, "note", "edited");
        assert_eq!(apply(s)["written"], 2);
        assert_eq!(q(t, &format!("SELECT _rowid_, rowid, note FROM {main}.rowid_taken ORDER BY _rowid_")), "1|R1|first\n2|r2|edited");
        run(s, "table.sort", json!({ "column": "rowid", "dir": "desc" }));
        assert_eq!(cell(&read(s), 0, "rowid"), Some("r2"));
        let r = run(s, "table.setFilters", json!({ "filters": [{ "column": "rowid", "op": "eq", "value": "R1" }] }));
        assert_eq!(r["rows"], 1);
        assert_eq!(cell(&read(s), 0, "note"), Some("first"));
    });
}

#[test]
fn a_virtual_table_shows_its_columns_and_takes_no_edits() {
    let _db = require_testdb!();
    for_sqlite(|t, s| {
        let (sc, name) = (t.schema("scratch"), unique("t_docs"));
        let _drop = Scratch(t, vec![format!("TABLE IF EXISTS {sc}.{name}")]);
        q(t, &format!("CREATE VIRTUAL TABLE {sc}.{name} USING fts5(title, body); INSERT INTO {sc}.{name} VALUES ('a title', 'a body')"));
        run(s, "schema.refresh", json!({}));
        assert_eq!(open_table(s, sc, &name)["rows"], 1);
        let d = read(s);
        assert_eq!(column_names(&d), ["title", "body"], "its hidden columns are not shown");
        assert_eq!((cell(&d, 0, "title"), cell(&d, 0, "body")), (Some("a title"), Some("a body")));
        let Some(Tab::Table(tab)) = s.state().active_tab() else { panic!("a table tab is active") };
        assert!(tab.hidden.columns.is_empty(), "the page read no hidden column: {:?}", tab.hidden.columns);
        let reason = d["notEditableReason"].as_str().unwrap_or_default().to_owned();
        assert!(d["editable"] == false && reason.starts_with("This is a virtual table"), "{d}");
        assert_eq!(summary(s)["noEditReason"], reason.as_str());
        let e = refused(s, "cell.edit", json!({ "row": 0, "column": "title", "value": "x" }));
        assert_eq!((e.code(), why(&e)), ("disabled", reason));
    });
}

#[test]
fn a_sorted_page_of_a_rowid_table_edits_the_rows_it_shows() {
    let _db = require_testdb!();
    for_sqlite(|t, s| {
        let main = t.schema("shop");
        open_table(s, main, "notes_nokey");
        run(s, "table.sort", json!({ "column": "body", "dir": "desc" }));
        // "another note" twice (rowids 2 and 3, the hidden key breaking the tie), then "a note" (1).
        let d = read(s);
        assert_eq!((cell(&d, 0, "body"), cell(&d, 2, "body")), (Some("another note"), Some("a note")));
        set_cell(s, 0, "title", "shown first");
        set_cell(s, 2, "title", "shown last");
        assert_eq!(apply(s)["written"], 2);
        assert_eq!(q(t, &format!("SELECT rowid, title FROM {main}.notes_nokey ORDER BY rowid")), "1|shown last\n2|shown first\n3|second");
    });
}

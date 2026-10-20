//! Staged edits and Apply: cell edits, NULL and clearing, new and deleted rows, undo and redo,
//! all-or-nothing Apply and what it refuses. Needs the Docker test servers (`cargo xtask testdb
//! up`); every test writes only to its own table in `scratch`, which it drops at the end.

mod common;

use std::time::Duration;

use common::*;
use dbee_engine::{EngineError, Json, Outcome, Session, Tab};
use dbee_testkit::{require_testdb, unique};
use serde_json::json;

/// Drops the scratch objects it names (quoted, schema-qualified) when the test ends, also when it
/// fails.
struct Scratch(Vec<String>);

impl Drop for Scratch {
    fn drop(&mut self) {
        for what in &self.0 {
            let _ = dbee_testkit::psql(pg14(), &format!("DROP {what} CASCADE"));
        }
    }
}

/// `scratch.<u> (id int PK, name text, total numeric(12,2), shipped_at date, flag bool, doc
/// jsonb)` with 3 rows, ids 1 to 3. Made before the session connects, so the catalog has it.
fn make_table() -> (String, Scratch) {
    let name = unique("t_edit");
    let guard = Scratch(vec![format!("TABLE IF EXISTS scratch.{name}")]);
    dbee_testkit::psql(
        pg14(),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, name text, total numeric(12,2), shipped_at date, flag bool, doc jsonb);
             INSERT INTO scratch.{name} VALUES
                 (1, 'Alice', 10.50, '2025-01-02', true, '{{\"a\": 1}}'),
                 (2, 'Bob', 20.00, '2025-02-03', false, '[1, 2]'),
                 (3, 'Cy', 5.25, NULL, NULL, NULL);"
        ),
    )
    .unwrap();
    (name, guard)
}

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.run_blocking(id, params, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data()))
}

fn open_table(s: &mut Session, schema: &str, table: &str) -> Json {
    run(s, "table.open", json!({ "schema": schema, "table": table }))
}

/// A session connected to pg14 with `scratch.<name>` open.
fn opened(name: &str) -> TestSession {
    let mut s = connected();
    let r = open_table(&mut s, "scratch", name);
    assert_eq!(r["rows"], 3, "{r}");
    s
}

fn q(sql: &str) -> String {
    dbee_testkit::psql(pg14(), sql).unwrap().trim().to_owned()
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

fn read(s: &mut Session) -> Json {
    run(s, "table.read", json!({}))
}

fn col(read: &Json, name: &str) -> usize {
    read["columns"].as_array().unwrap().iter().position(|c| c["name"] == name).unwrap_or_else(|| panic!("no column {name}: {}", read["columns"]))
}

fn cell<'a>(read: &'a Json, row: usize, name: &str) -> Option<&'a str> {
    read["rows"][row][col(read, name)].as_str()
}

#[test]
fn day_scenario_two_changes_one_apply() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    let r = run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "Ann" }));
    assert_eq!((r["staged"].as_bool(), r["count"].as_u64()), (Some(true), Some(1)), "{r}");
    run(&mut s, "cell.clear", json!({ "row": 0, "column": "shipped_at" }));
    assert_eq!(count(&s), 2);
    assert_eq!(staged(&s, 0, 1), Some(json!("Ann")));
    assert_eq!(staged(&s, 0, 3), Some(Json::Null), "clearing a date stages NULL");
    // Nothing is written before Apply.
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 1")), "Alice");
    let r = run(&mut s, "edit.apply", json!({}));
    assert_eq!(r["written"], 1, "both changes are one row: {r}");
    assert_eq!(q(&format!("SELECT name || '|' || (shipped_at IS NULL) FROM scratch.{t} WHERE id = 1")), "Ann|true");
    // The edits are gone and the page shows what was written.
    assert_eq!(count(&s), 0);
    let d = read(&mut s);
    assert_eq!((cell(&d, 0, "name"), cell(&d, 0, "shipped_at")), (Some("Ann"), None));
    assert_eq!(cell(&d, 1, "name"), Some("Bob"));
    assert_eq!(refused(&mut s, "edit.undo", json!({})).code(), "disabled", "writes are not undone");
}

#[test]
fn clearing_rules() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    run(&mut s, "cell.clear", json!({ "row": 0, "column": "total" }));
    assert_eq!(staged(&s, 0, 2), Some(Json::Null), "a number clears to NULL");
    run(&mut s, "cell.clear", json!({ "row": 1, "column": "name" }));
    assert_eq!(staged(&s, 1, 1), Some(json!("")), "text clears to an empty string");
    run(&mut s, "cell.setNull", json!({ "row": 2, "column": "name" }));
    assert_eq!(staged(&s, 2, 1), Some(Json::Null), "Set as NULL stores NULL in text too");
    // Booleans and jsonb have no empty value either; by index this time.
    run(&mut s, "cell.clear", json!({ "row": 0, "column": 4 }));
    run(&mut s, "cell.clear", json!({ "row": 0, "column": 5 }));
    assert_eq!((staged(&s, 0, 4), staged(&s, 0, 5)), (Some(Json::Null), Some(Json::Null)));
    assert_eq!(count(&s), 5);
    run(&mut s, "edit.apply", json!({}));
    let rows =
        q(&format!("SELECT id, total IS NULL, coalesce(name = '', false), name IS NULL, flag IS NULL, doc IS NULL FROM scratch.{t} ORDER BY id"));
    assert_eq!(rows, "1|t|f|f|t|t\n2|f|t|f|f|f\n3|f|f|t|t|t");
}

#[test]
fn failed_apply_writes_nothing_and_marks_the_row() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "ok" }));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "shipped_at", "value": "not a date" }));
    let e = run_err(&mut s, "edit.apply", json!({}));
    assert_eq!(e.code(), "server", "{e}");
    assert!(e.to_string().contains("nothing was written"), "{e}");
    assert_eq!((e.data()["row"].as_u64(), e.data()["column"].as_u64()), (Some(1), Some(3)), "{}", e.data());
    // Neither change was written: the good one rolled back with the bad one.
    assert_eq!(q(&format!("SELECT name || '|' || shipped_at FROM scratch.{t} WHERE id IN (1, 2) ORDER BY id")), "Alice|2025-01-02\nBob|2025-02-03");
    let table = s.inspect()["table"].clone();
    assert_eq!((table["failed"]["row"].as_u64(), table["failed"]["column"].as_u64()), (Some(1), Some(3)), "{table}");
    assert!(table["failed"]["message"].as_str().unwrap().contains("date"), "{table}");
    assert_eq!(count(&s), 2, "every edit is kept");
    assert_eq!(table["applying"], Json::Null);
    // Fixing the value and applying again writes both and clears the mark.
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "shipped_at", "value": "2026-10-20" }));
    assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 2);
    assert_eq!(s.inspect()["table"]["failed"], Json::Null);
    assert_eq!(q(&format!("SELECT name || '|' || shipped_at FROM scratch.{t} WHERE id IN (1, 2) ORDER BY id")), "ok|2025-01-02\nBob|2026-10-20");
}

#[test]
fn add_and_delete_rows() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    assert_eq!(run(&mut s, "row.add", json!({}))["row"], 3);
    assert_eq!(run(&mut s, "row.add", json!({}))["row"], 4);
    run(&mut s, "cell.edit", json!({ "row": 4, "column": "id", "value": "11" }));
    // Deleting a new row takes it out; the rows after it move up.
    run(&mut s, "row.delete", json!({ "rows": [3] }));
    assert_eq!(edits(&s)["newRows"].as_array().map(Vec::len), Some(1));
    run(&mut s, "cell.edit", json!({ "row": 3, "column": "id", "value": "10" }));
    run(&mut s, "cell.edit", json!({ "row": 3, "column": "name", "value": "new" }));
    // Deleting a loaded row marks it, and drops the changes staged on it.
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "gone anyway" }));
    run(&mut s, "row.delete", json!({ "rows": [1] }));
    let e = edits(&s);
    assert_eq!((e["deleted"].clone(), e["cells"].clone(), e["count"].as_u64()), (json!([1]), json!([]), Some(2)), "{e}");
    assert_eq!(e["newRows"], json!([{ "row": 3, "values": [{ "column": 0, "value": "10" }, { "column": 1, "value": "new" }] }]));
    assert_eq!(refused(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "x" })).code(), "badParams", "a deleted row");
    for bad in [json!({ "rows": [4] }), json!({ "rows": "1" }), json!({ "rows": [-1] }), json!({})] {
        assert_eq!(refused(&mut s, "row.delete", bad.clone()).code(), "badParams", "{bad}");
    }
    let r = run(&mut s, "edit.apply", json!({}));
    assert_eq!(r["written"], 2, "{r}");
    assert_eq!(q(&format!("SELECT string_agg(id || ':' || coalesce(name, ''), ',' ORDER BY id) FROM scratch.{t}")), "1:Alice,3:Cy,10:new");
    // The page loads again with the new row in it.
    let d = read(&mut s);
    assert_eq!(d["rows"].as_array().map(Vec::len), Some(3));
    assert_eq!(cell(&d, 2, "name"), Some("new"));
}

#[test]
fn undo_redo_and_reset() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    assert_eq!(why(&refused(&mut s, "edit.undo", json!({}))), "nothing to undo");
    assert_eq!(why(&refused(&mut s, "edit.redo", json!({}))), "nothing to redo");
    assert_eq!(why(&refused(&mut s, "edit.reset", json!({}))), "no staged changes");
    assert_eq!(why(&refused(&mut s, "edit.apply", json!({}))), "no staged changes");
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "a" }));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "b" }));
    run(&mut s, "cell.edit", json!({ "row": 2, "column": "total", "value": "1" }));
    assert_eq!(count(&s), 3);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(run(&mut s, "edit.undo", json!({}))["count"], 1);
    assert_eq!(staged(&s, 0, 1), Some(json!("a")));
    assert_eq!(edits(&s)["canRedo"], true);
    assert_eq!(run(&mut s, "edit.redo", json!({}))["count"], 2);
    assert_eq!(staged(&s, 1, 1), Some(json!("b")));
    assert_eq!(run(&mut s, "edit.reset", json!({}))["count"], 0);
    assert_eq!(count(&s), 0);
    assert_eq!(edits(&s)["canRedo"], false, "a change clears what could be redone");
    // A reset can be undone too.
    assert_eq!(run(&mut s, "edit.undo", json!({}))["count"], 2);
    assert_eq!(run(&mut s, "edit.redo", json!({}))["count"], 0);
    // Undoing everything leaves no staged change, and nothing reached the database.
    while s.execute("edit.undo", json!({})).is_ok() {}
    assert_eq!(count(&s), 0);
    assert_eq!(q(&format!("SELECT string_agg(name, ',' ORDER BY id) FROM scratch.{t}")), "Alice,Bob,Cy");
}

#[test]
fn editing_value_back_unstages() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "x" }));
    assert_eq!(count(&s), 1);
    let r = run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "Alice" }));
    assert_eq!((r["staged"].as_bool(), r["count"].as_u64()), (Some(false), Some(0)));
    assert_eq!(count(&s), 0);
    // NULL onto a NULL cell, or the same text again, stages nothing.
    run(&mut s, "cell.setNull", json!({ "row": 2, "column": "shipped_at" }));
    run(&mut s, "cell.edit", json!({ "row": 2, "column": "doc", "value": null }));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "total", "value": "20.00" }));
    assert_eq!(count(&s), 0);
    // Values are text, a number is taken as its text; NUL and other shapes are refused.
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "total", "value": 21 }));
    assert_eq!(staged(&s, 1, 2), Some(json!("21")));
    for bad in [
        json!({ "row": 0, "column": "name", "value": "nul\u{0}" }),
        json!({ "row": 0, "column": "name", "value": [1] }),
        json!({ "row": 0, "column": "name" }),
        json!({ "row": 0, "column": "nope", "value": "x" }),
        json!({ "row": 0, "column": 6, "value": "x" }),
        json!({ "row": 3, "column": "name", "value": "x" }),
        json!({ "column": "name", "value": "x" }),
    ] {
        assert_eq!(refused(&mut s, "cell.edit", bad.clone()).code(), "badParams", "{bad}");
    }
    assert_eq!(count(&s), 1);
}

#[test]
fn read_only_and_keyless_refuse() {
    let _db = require_testdb!();
    let edit = json!({ "row": 0, "column": 0, "value": "1" });
    {
        let mut s = session();
        let config = dbee_config::ConnectionConfig { read_only: true, ..pg14_config() };
        open(&mut s, json!({ "config": config, "password": pw() }));
        open_table(&mut s, "shop", "customers");
        for (id, params) in [("cell.edit", edit.clone()), ("cell.setNull", edit.clone()), ("cell.clear", edit.clone()), ("row.add", json!({}))] {
            let e = refused(&mut s, id, params);
            assert_eq!((e.code(), why(&e).as_str()), ("disabled", "read-only connection"), "{id}");
        }
    }
    let mut s = connected();
    open_table(&mut s, "shop", "no_key");
    let e = refused(&mut s, "cell.edit", edit.clone());
    assert_eq!((e.code(), why(&e).as_str()), ("disabled", "no primary key or unique not-null key"));
    open_table(&mut s, "shop", "order_totals");
    assert_eq!(why(&refused(&mut s, "row.add", json!({}))), "views are read-only");
    let infos = s.commands();
    for id in ["cell.edit", "cell.setNull", "cell.clear", "row.add", "row.delete", "edit.apply", "edit.reset", "edit.undo", "edit.redo"] {
        let info = infos.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no command {id}"));
        assert!(!info.enabled, "{id}");
    }
    // A query tab is not a table tab.
    run(&mut s, "tab.newQuery", json!({}));
    assert_eq!(why(&refused(&mut s, "cell.edit", edit)), "no table tab is active");
}

#[test]
fn paging_with_staged_edits_is_disabled() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "x" }));
    for (id, params) in [
        ("table.page", json!({ "to": "next" })),
        ("table.sort", json!({ "column": "name" })),
        ("table.refresh", json!({})),
        ("table.setFilters", json!({ "filters": [{ "column": "id", "op": "eq", "value": "1" }] })),
    ] {
        let e = refused(&mut s, id, params);
        assert_eq!((e.code(), why(&e).as_str()), ("disabled", "apply or reset your changes first"), "{id}");
    }
    // Typing in the filter bar is fine; applying it waits.
    let typed = json!([{ "column": "id", "op": "eq", "value": "2" }]);
    run(&mut s, "table.setFilters", json!({ "filters": typed, "apply": false }));
    assert_eq!(s.inspect()["table"]["filters"], typed);
    assert_eq!(s.inspect()["table"]["applied"], "none");
    // Closing the tab asks first.
    let e = refused(&mut s, "tab.close", json!({}));
    assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("stagedEdits")));
    assert!(e.to_string().contains("1 staged change"), "{e}");
    assert_eq!(s.state().tabs.len(), 1);
    // So does disconnecting.
    let e = refused(&mut s, "connection.close", json!({}));
    assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("stagedEdits")));
    // After a reset the bar's filter applies and paging works.
    run(&mut s, "edit.reset", json!({}));
    assert_eq!(run(&mut s, "table.setFilters", json!({}))["rows"], 1);
    assert_eq!(run(&mut s, "table.page", json!({ "to": "next" }))["page"], 0);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "y" }));
    run(&mut s, "tab.close", json!({ "force": true }));
    assert!(s.state().tabs.is_empty());
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 2")), "Bob", "closing drops the edits unwritten");
}

#[test]
fn second_apply_while_running_is_busy() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    let row = run(&mut s, "row.add", json!({}))["row"].clone();
    run(&mut s, "cell.edit", json!({ "row": row, "column": "id", "value": "10" }));
    let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
    assert_eq!(s.inspect()["table"]["applying"], json!(job.0));
    let e = s.execute("edit.apply", json!({})).unwrap_err();
    assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: apply in progress"));
    // The edits wait for the Apply too.
    assert_eq!(why(&refused(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "x" }))), "apply in progress");
    assert_eq!(why(&refused(&mut s, "edit.reset", json!({}))), "apply in progress");
    assert_eq!(s.wait(job, T).unwrap()["written"], 1);
    assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t} WHERE id = 10")), "1", "written once");
    assert_eq!((count(&s), &s.inspect()["table"]["applying"]), (0, &Json::Null));
}

#[test]
fn editing_one_column_leaves_exotic_types_unchanged() {
    let _db = require_testdb!();
    let t = unique("t_exotic");
    let _drop = Scratch(vec![format!("TABLE IF EXISTS scratch.{t}")]);
    q(&format!("CREATE TABLE scratch.{t} AS SELECT * FROM shop.all_types; ALTER TABLE scratch.{t} ADD PRIMARY KEY (id)"));
    let others = "(st, doc, r, iv, m, n, ts, b, u, ip, arr, pt, comp)::text";
    let before_psql = q(&format!("SELECT {others} FROM scratch.{t} ORDER BY id"));
    let mut s = opened(&t);
    let before = read(&mut s);
    let note = col(&before, "note");
    for row in 0..3 {
        run(&mut s, "cell.edit", json!({ "row": row, "column": "note", "value": format!("edited 🐝 it's \"{row}\"") }));
    }
    assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 3);
    let after = read(&mut s);
    for row in 0..3 {
        for (i, c) in before["columns"].as_array().unwrap().iter().enumerate() {
            if i == note {
                assert_eq!(after["rows"][row][i], json!(format!("edited 🐝 it's \"{row}\"")));
            } else {
                assert_eq!(after["rows"][row][i], before["rows"][row][i], "row {row}, column {}", c["name"]);
            }
        }
    }
    assert_eq!(q(&format!("SELECT {others} FROM scratch.{t} ORDER BY id")), before_psql);
}

#[test]
fn odd_table_edit_round_trip() {
    let _db = require_testdb!();
    let name = format!("{} Odd \"Name\" 🐝", unique("t_odd"));
    let quoted = format!("scratch.\"{}\"", name.replace('"', "\"\""));
    let _drop = Scratch(vec![format!("TABLE IF EXISTS {quoted}")]);
    q(&format!("CREATE TABLE {quoted} AS SELECT * FROM odd.\"Odd \"\"Name\"\" 🐝\"; ALTER TABLE {quoted} ADD PRIMARY KEY (id)"));
    let mut s = opened_rows(&name, 4);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "lower UPPER", "value": "it's \"edited\" 🐝" }));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "Total $", "value": "99.5" }));
    let row = run(&mut s, "row.add", json!({}))["row"].clone();
    run(&mut s, "cell.edit", json!({ "row": row, "column": "id", "value": "7" }));
    run(&mut s, "cell.edit", json!({ "row": row, "column": "lower UPPER", "value": "new 🐝" }));
    run(&mut s, "row.delete", json!({ "rows": [3] }));
    assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 4);
    let got = q(&format!(
        "SELECT string_agg(id || ':' || coalesce(\"lower UPPER\", '-') || ':' || coalesce(\"Total $\"::text, '-'), ',' ORDER BY id) FROM {quoted}"
    ));
    assert_eq!(got, "1:it's \"edited\" 🐝:0.5,2:Beta:99.5,3:gamma 🐝:100,7:new 🐝:-");
}

fn opened_rows(name: &str, rows: u64) -> TestSession {
    let mut s = connected();
    let r = open_table(&mut s, "scratch", name);
    assert_eq!(r["rows"], rows, "{r}");
    s
}

/// A `BEFORE UPDATE` trigger on `scratch.<t>` that sleeps 2 s, so an Apply is still running when
/// the test acts on it. Returns the function's name, for the cleanup.
fn slow_updates(t: &str) -> String {
    let f = format!("scratch.{t}_slow()");
    q(&format!(
        "CREATE FUNCTION {f} RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(2); RETURN NEW; END $$;
         CREATE TRIGGER slow BEFORE UPDATE ON scratch.{t} FOR EACH ROW EXECUTE FUNCTION {f};"
    ));
    format!("FUNCTION IF EXISTS {f}")
}

#[test]
fn connection_lost_during_apply_keeps_the_edits() {
    let _db = require_testdb!();
    let (t, mut drop) = make_table();
    drop.0.push(slow_updates(&t));
    let mut s = connected_exclusive();
    open_table(&mut s, "scratch", &t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "lost" }));
    let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
    std::thread::sleep(Duration::from_millis(300));
    kill_dbee_backends(&s, pg14());
    let e = s.wait(job, T).unwrap_err();
    assert_eq!(e.code(), "connectionLost", "{e}");
    assert_eq!(e.data()["action"], "connection.reconnect");
    assert_eq!(e.to_string(), UNKNOWN);
    let table = s.inspect()["table"].clone();
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
    // Nothing was retried, and the server rolled the transaction back with the connection.
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 1")), "Alice");
    run(&mut s, "connection.reconnect", json!({}));
    assert_eq!(count(&s), 1);
    // Applying again could write twice: it asks first.
    let e = refused(&mut s, "edit.apply", json!({}));
    assert_eq!((e.code(), e.data()["what"].as_str(), e.to_string().as_str()), ("needsConfirm", Some("applyAfterUnknownOutcome"), UNKNOWN));
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 1")), "Alice");
    assert_eq!(run(&mut s, "edit.apply", json!({ "force": true }))["written"], 1);
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 1")), "lost");
}

/// `failed`'s message after an Apply whose outcome is unknown.
const UNKNOWN: &str = "connection lost during Apply — the outcome is unknown. Reset and reload to check what was written, or Apply again to retry";

#[test]
fn reconnect_during_apply_marks_the_outcome_unknown() {
    let _db = require_testdb!();
    let (t, mut drop) = make_table();
    drop.0.push(slow_updates(&t));
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "cut off" }));
    let Outcome::Started(job) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "connection.reconnect", json!({}));
    // The Apply ran on the connection the reconnect let go: its result will be ignored, so the
    // tab cannot know what it wrote.
    let table = s.inspect()["table"].clone();
    assert_eq!(table["applying"], Json::Null);
    assert_eq!(table["failed"]["row"], 1, "{table}");
    assert!(table["failed"]["message"].as_str().unwrap().contains("the outcome is unknown"), "{table}");
    assert_eq!(count(&s), 1);
    assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }));
    assert_eq!(count(&s), 1);
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 2")), "Bob", "the reconnect cancelled it");
    assert_eq!(table["failed"]["message"], UNKNOWN);
    assert_eq!((&table["outcomeUnknown"], &read(&mut s)["outcomeUnknown"]), (&json!(true), &json!(true)), "the UI can see why Apply asks");
    let e = refused(&mut s, "edit.apply", json!({}));
    assert_eq!((e.code(), e.data()["what"].as_str()), ("needsConfirm", Some("applyAfterUnknownOutcome")));
    // A reset clears the mark, but the doubt stays until the page loads again: the same edits
    // brought back by an undo still ask.
    run(&mut s, "edit.reset", json!({}));
    assert_eq!((&s.inspect()["table"]["failed"], &s.inspect()["table"]["outcomeUnknown"]), (&Json::Null, &json!(true)));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(count(&s), 1);
    assert_eq!(refused(&mut s, "edit.apply", json!({})).code(), "needsConfirm");
    // Reset and reload: what was written shows, and new edits apply without asking.
    run(&mut s, "edit.reset", json!({}));
    run(&mut s, "table.refresh", json!({}));
    let d = read(&mut s);
    assert_eq!((cell(&d, 1, "name"), &d["outcomeUnknown"]), (Some("Bob"), &json!(false)));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "again" }));
    assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 1);
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 2")), "again");
}

#[test]
fn table_cancel_never_cancels_an_apply() {
    let _db = require_testdb!();
    let (t, mut drop) = make_table();
    drop.0.push(slow_updates(&t));
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "kept" }));
    let Outcome::Started(apply) = s.execute("edit.apply", json!({})).unwrap() else { panic!("edit.apply starts a job") };
    std::thread::sleep(Duration::from_millis(300));
    // Another tab's page load waits behind the Apply on the browse connection; cancelling it
    // would cancel the Apply's statement instead.
    let Outcome::Started(load) = s.execute("table.open", json!({ "schema": "scratch", "table": t })).unwrap() else {
        panic!("table.open starts a job")
    };
    let e = refused(&mut s, "table.cancel", json!({}));
    assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: apply in progress"));
    assert_eq!(s.wait(apply, T).unwrap()["written"], 1);
    assert_eq!(s.wait(load, T).unwrap()["rows"], 3);
    assert_eq!(q(&format!("SELECT name FROM scratch.{t} WHERE id = 1")), "kept");
}

#[test]
fn reset_undo_redo_work_when_the_tab_is_no_longer_editable() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "a" }));
    run(&mut s, "cell.edit", json!({ "row": 1, "column": "name", "value": "b" }));
    q(&format!("DROP TABLE scratch.{t}"));
    run(&mut s, "schema.refresh", json!({}));
    let gone = "the table is no longer in the schema";
    assert_eq!(s.inspect()["table"]["notEditableReason"], gone);
    assert_eq!(why(&refused(&mut s, "edit.apply", json!({}))), gone, "Apply stays refused");
    assert_eq!(why(&refused(&mut s, "cell.edit", json!({ "row": 2, "column": "name", "value": "c" }))), gone);
    assert_eq!(run(&mut s, "edit.undo", json!({}))["count"], 1);
    assert_eq!(run(&mut s, "edit.redo", json!({}))["count"], 2);
    assert_eq!(run(&mut s, "edit.reset", json!({}))["count"], 0);
    // Nothing staged: the tab closes without asking.
    run(&mut s, "tab.close", json!({}));
    assert!(s.state().tabs.is_empty());
}

#[test]
fn edits_follow_the_tab_they_were_made_in() {
    let _db = require_testdb!();
    let (t, _drop) = make_table();
    let mut s = opened(&t);
    let first = s.inspect()["active"].clone();
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "first tab" }));
    open_table(&mut s, "scratch", &t);
    assert_eq!(count(&s), 0, "a new tab has its own edits");
    run(&mut s, "cell.edit", json!({ "row": 2, "column": "name", "value": "second tab" }));
    assert_eq!(run(&mut s, "edit.undo", json!({}))["count"], 0);
    run(&mut s, "tab.activate", json!({ "tab": first }));
    assert_eq!(staged(&s, 0, 1), Some(json!("first tab")));
    let Some(Tab::Table(tab)) = s.state().active_tab() else { panic!("a table tab is active") };
    assert_eq!(tab.edits.count(), 1);
    let data = tab.data.clone().unwrap();
    assert_eq!(tab.edits.shown(&data, 0, 1), Some(Some("first tab")));
    assert_eq!(tab.edits.shown(&data, 1, 1), None);
    assert!(tab.has_staged_edits());
}

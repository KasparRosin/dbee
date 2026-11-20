//! The query tab driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the SQL editor (keys, Tab, the cursor), Run, Run current and Cancel, the result
//! selector, the read-only result grid with its cap notice and rows-affected line, the error card
//! and the split between editor and results. Everything goes through control requests and
//! automation ids, the way an agent drives the real window; assertions read `ui.inspect` and
//! `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is read-only: a test that writes
//! makes its own table in `scratch` and drops it.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use dbee_config::{MemoryStore, SecretStore};
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TestDb, temp_home, unique};
use dbee_ui_egui::DbeeApp;
use egui::{Event, Modifiers, PointerButton, Pos2};
use serde_json::{Value as Json, json};

/// [`Ui::connected`], with a new query tab whose editor has the keyboard.
fn query_tab(db: &TestDb) -> Ui {
    let mut ui = Ui::connected(db);
    ui.ctrl("T");
    ui.frame();
    assert_eq!(ui.editor()["focused"], true, "a new query tab's editor has the keyboard: {}", ui.editor());
    ui
}

impl Ui {
    /// The `query` part of `document.inspect`: the active query tab.
    fn query(&mut self) -> Json {
        self.document()["query"].clone()
    }

    /// The editor's tag value: `{cursor, focused, lines, length}`.
    fn editor(&mut self) -> Json {
        self.value("query-editor")
    }

    /// The tags list `enabled: false` for a disabled widget only.
    fn enabled(&mut self, id: &str) -> bool {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["enabled"] != false
    }

    /// Runs frames until the active query tab's run has ended, then one more to draw it.
    fn ran(&mut self) {
        self.until("the run to end", |ui| {
            let q = ui.query();
            q["running"].is_null() && (q["results"].as_array().is_some_and(|r| !r.is_empty()) || !q["error"].is_null())
        });
        self.frame();
    }

    /// The rect of automation entry `id`: `[x, y, width, height]`.
    fn rect(&mut self, id: &str) -> [f32; 4] {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        [f(0), f(1), f(2), f(3)]
    }
}

const JOIN: &str = "select o.id, c.name, o.total\nfrom shop.orders o\njoin shop.customers c on c.id = o.customer_id\norder by o.id\nlimit 50";

#[test]
fn ctrl_enter_runs_and_shows_results() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    // Pasted straight into the new tab: its editor has the keyboard.
    ui.text(JOIN);
    ui.frame();
    assert_eq!(ui.query()["text"], JOIN);
    ui.ctrl("Enter");
    ui.ran();
    let q = ui.query();
    assert_eq!(q["text"], JOIN, "Ctrl+Enter ran the text and put no newline in it");
    assert!(q["error"].is_null(), "{q}");
    assert_eq!(q["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(q["results"][0]["rows"], 50, "{q}");
    assert!(visible(&ui.tag("grid-cell-0-1")), "the result grid shows its rows");
    assert_eq!(ui.value("grid-header-1")["name"], "name");
    assert_eq!(ui.value("query-rows"), "50 rows");
    assert!(visible(&ui.tag("result-1")), "one result: one tab");
    assert!(ui.tag("query-error").is_none());
    // The editor keeps the keyboard, and the text is highlighted.
    let ed = ui.editor();
    assert_eq!((ed["focused"].clone(), ed["lines"].clone(), ed["highlighted"].clone()), (json!(true), json!(5), json!(true)), "{ed}");
}

#[test]
fn ctrl_shift_enter_runs_current_statement() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql = "select 1 as first;\nselect 2 as second, 3 as third;";
    ui.text(sql);
    // The cursor into the first statement.
    ui.ctrl("Home");
    ui.key("ArrowRight");
    ui.key("ArrowRight");
    ui.frame();
    assert_eq!(ui.editor()["cursor"], 2, "a byte offset into the text");
    ui.ctrl_shift("Enter");
    ui.ran();
    let q = ui.query();
    assert_eq!(q["text"], sql, "Ctrl+Shift+Enter put no newline in");
    let results = q["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "only the statement under the cursor ran: {q}");
    assert_eq!(results[0]["columns"][0]["name"], "first");
    // Run current with the cursor at the end runs the last statement.
    ui.ctrl("End");
    ui.frame();
    assert_eq!(ui.editor()["cursor"], sql.len());
    ui.click("query-run-current");
    ui.until("the second statement's result", |ui| ui.query()["results"][0]["columns"][0]["name"] == "second");
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(1));
    // Run runs both.
    ui.click("query-run");
    ui.until("both results", |ui| ui.query()["results"].as_array().map(Vec::len) == Some(2));
}

#[test]
fn error_card_and_cursor_jump() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select nope");
    ui.ctrl("Enter");
    ui.ran();
    let card = ui.value("query-error");
    assert_eq!(card["code"], "server", "{card}");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("nope")), "{card}");
    assert_eq!((card["position"].clone(), card["line"].clone(), card["column"].clone()), (json!(7), json!(1), json!(8)), "{card}");
    assert!(ui.tag("notice").is_none(), "the card says it, not a notice too");
    // The band, as the page draws it: its heading, where the error is at the right of it, and
    // that line's number in `danger` in the gutter, though the cursor is on it too.
    assert_eq!(card["heading"], "Query failed", "{card}");
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let [x, y, w, h] = ui.rect("query-error");
    let band = egui::Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, h));
    let (link, _) = ui.text_in(band, "Line 1, column 8 →").expect("the position at the band's right");
    assert!((link.right() - (band.right() - 12.0)).abs() < 0.5, "{link:?} in {band:?}");
    assert!(ui.text_in(band, "Click to put the cursor there.").is_none(), "the page drops the old line");
    let text_left = ui.texts().into_iter().find(|(_, g)| g.text() == "select nope").map(|(r, _)| r.left()).expect("the editor's text");
    let number = ui.texts().into_iter().find(|(r, g)| g.text() == "1" && r.right() < text_left).map(|(_, g)| g.job.sections[0].format.color);
    assert_eq!(number, Some(t.danger), "the error's line number");
    // Elsewhere first: the cursor at the start, and the keyboard taken away.
    ui.ctrl("Home");
    ui.key("Escape");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(0), json!(false)), "{ed}");
    // A click on the card puts the cursor where the server points, with the keyboard.
    ui.click("query-error");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(7), json!(true)), "{ed}");
    ui.text("x");
    ui.frame();
    assert_eq!(ui.query()["text"], "select xnope", "typing goes in at the error");
}

#[test]
fn an_error_without_a_position_offers_no_jump() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1/0");
    ui.ctrl("Enter");
    ui.ran();
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("division by zero")), "{card}");
    assert_eq!((card["position"].clone(), card["jump"].clone()), (Json::Null, json!(false)), "{card}");
    ui.ctrl("Home");
    ui.key("Escape");
    ui.click("query-error");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(0), json!(false)), "a click does nothing: {ed}");
}

#[test]
fn cancel_button_stops_pg_sleep() {
    let db = dbee_testkit::require_testdb!();
    cancel_with_the_button(db);
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn cancel_button_stops_pg_sleep_within_five_seconds() {
    let db = dbee_testkit::require_testdb!();
    let took = cancel_with_the_button(db);
    assert!(took < Duration::from_secs(5), "cancelled in {took:?}");
}

/// Runs `select pg_sleep(30)` and stops it with the Cancel button: the card says cancelled. How
/// long from the click until the run ended.
fn cancel_with_the_button(db: &TestDb) -> Duration {
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.frame();
    assert!(ui.tag("query-cancel").is_none(), "nothing runs: no Cancel");
    assert!(ui.enabled("query-run"));
    ui.click("query-run");
    ui.until("Cancel to show", |ui| ui.tag("query-cancel").is_some());
    assert!(ui.enabled("query-cancel"), "Cancel is enabled while the run goes");
    assert!(!ui.enabled("query-run"), "Run is off while the run goes");
    let start = Instant::now();
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
    let took = start.elapsed();
    println!("cancelled in {took:?}");
    ui.frame();
    assert!(ui.tag("query-error").is_none(), "dbee's own cancel: no band");
    assert!(ui.value("query-facts").as_str().is_some_and(|f| f.starts_with("Cancelled")), "the bar says it");
    assert!(ui.tag("query-cancel").is_none());
    assert!(ui.tag("notice").is_none(), "the bar says it: {:?}", ui.tag("notice"));
    took
}

#[test]
fn ctrl_enter_while_a_run_goes_says_the_engine_is_busy() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.ctrl("Enter");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    let running = ui.query()["running"].clone();
    ui.ctrl("Enter");
    ui.frame();
    let notice = ui.value("notice");
    assert_eq!(notice, "A query is running in Query 1.");
    assert_eq!(ui.query()["running"], running, "the run goes on, and no second one started");
    ui.until("Cancel to show", |ui| ui.tag("query-cancel").is_some());
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
}

#[test]
fn several_results_have_a_selector() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as a;\nselect 'x' as b, 2 as c;\nselect 3 as d");
    ui.ctrl("Enter");
    ui.ran();
    assert_eq!(ui.query()["shown"], 2, "the last result shows");
    for n in 1..=3 {
        let v = ui.value(&format!("result-{n}"));
        assert_eq!(v["shown"], n == 3, "result-{n}: {v}");
    }
    assert!(ui.tag("result-0").is_none() && ui.tag("result-4").is_none(), "the ids count from 1");
    assert_eq!(ui.value("grid-header-0")["name"], "d");
    ui.click("result-1");
    assert_eq!(ui.query()["shown"], 0, "result-1 is the engine's index 0");
    ui.frame();
    assert_eq!(ui.value("grid-header-0")["name"], "a");
    assert_eq!(ui.value("result-1")["shown"], true);
    ui.click("result-2");
    ui.frame();
    assert_eq!((ui.value("grid-header-0")["name"].clone(), ui.value("grid-header-1")["name"].clone()), (json!("b"), json!("c")));
    assert_eq!(ui.value("grid-cell-0-0")["text"], "x");
}

#[test]
fn many_results_fit_the_toolbar() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql: String = (1..=1200).map(|n| format!("select {n} as n;\n")).collect();
    ui.text(&sql);
    ui.ctrl("Enter");
    ui.ran();
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(1200));
    // Not all fit: those around the shown one (the last) do, clear of the row count.
    assert!(!visible(&ui.tag("result-1")), "the first tab scrolled out of view");
    let last = ui.rect("result-1200");
    let rows = ui.rect("query-rows");
    assert!(last[0] + last[2] <= rows[0], "the selector stays clear of the row count: {last:?} {rows:?}");
    assert_eq!(ui.value("query-rows"), "1 row");
    ui.click("result-1199");
    assert_eq!(ui.query()["shown"], 1198);
    ui.frame();
    assert!(ui.tag("result-1200").is_some(), "the neighbours stay in reach");
    // The `…` lists every result, the first one too.
    let more = ui.rect("result-more");
    assert!(more[0] + more[2] <= rows[0], "the ⌄ list stays clear of the row count: {more:?} {rows:?}");
    ui.click("result-more");
    ui.frame();
    ui.click("result-list-1");
    ui.frame();
    assert_eq!(ui.query()["shown"], 0);
    assert!(ui.tag("result-list-1").is_none(), "the list closed");
}

/// Review 2, Minor 3: the `…` list of every result, chosen from with the keyboard, closes as
/// after a click.
#[test]
fn the_result_list_closes_when_chosen_by_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql: String = (1..=40).map(|n| format!("select {n} as n;\n")).collect();
    ui.text(&sql);
    ui.ctrl("Enter");
    ui.ran();
    ui.focus("result-more");
    ui.key("Enter");
    ui.frame();
    assert!(ui.tag("result-list-1").is_some(), "Enter opens the list");
    // egui may stop on the list's frame on the way.
    for _ in 0..3 {
        if ui.focused("result-list-1") {
            break;
        }
        ui.key("ArrowDown");
    }
    assert!(ui.focused("result-list-1"), "the arrows reach the first result");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.query()["shown"], 0, "the first result chosen");
    assert!(ui.tag("result-list-1").is_none(), "the list closed");
}

/// Review 2, Minor 3: the database chip's list, chosen from with the keyboard, closes as after a
/// click. The arrows take the keyboard down into the list (egui may stop on the list's frame on
/// the way).
#[test]
fn the_database_list_closes_when_chosen_by_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    ui.focus("query-database");
    ui.key("Enter");
    ui.frame();
    assert!(ui.tag("query-database-many").is_some(), "Enter opens the list");
    for _ in 0..3 {
        if ui.focused("query-database-many") {
            break;
        }
        ui.key("ArrowDown");
    }
    assert!(ui.focused("query-database-many"), "the arrows reach the first database");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.query()["database"], "many", "the first database chosen");
    assert!(ui.tag("query-database-many").is_none(), "the list closed");
}

/// Beside the database chip, at the app's first size with the value panel open, every result can
/// be chosen, also while a statement runs; the selector's label shows whole or not at all.
#[test]
fn every_result_can_be_chosen_beside_the_database_chip() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let mut ui = connected_to_target(db, &mysql, Some("shop"), APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    assert!(ui.tag("query-database").is_some(), "the chip is drawn");
    assert!(ui.tag("value-toggle").is_none(), "no toggle before the first result");
    ui.execute("query.setText", json!({ "text": "select 1 as a;\nselect 2 as b;\nselect 3 as c" }));
    ui.ctrl("Enter");
    ui.ran();
    assert_eq!(ui.value("value-toggle"), true, "the value panel shows, beside the results");
    choose_every_result(&mut ui, "after the run");
    let key = unique("dbee_lock");
    let held = mysql.hold_lock(&key).unwrap();
    ui.execute("query.setText", json!({ "text": format!("SELECT 1 AS d FROM DUAL WHERE {}", mysql.waits_for_lock_sql(&key)) }));
    ui.ctrl("Enter");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    ui.frame();
    choose_every_result(&mut ui, "while a statement runs");
    drop(held);
    ui.ran();
}

/// Chooses each of the three results `a`, `b`, `c`, from the selector or from its `…` list.
fn choose_every_result(ui: &mut Ui, when: &str) {
    for (n, column) in [(1, "a"), (2, "b"), (3, "c")] {
        selector_whole(ui, when);
        if ui.tag(&format!("result-{n}")).is_some() {
            ui.click(&format!("result-{n}"));
        } else {
            ui.click("result-more");
            ui.frame();
            ui.click(&format!("result-list-{n}"));
        }
        ui.frame();
        assert_eq!(ui.query()["shown"], n - 1, "{when}: result {n}");
        assert_eq!(ui.value("grid-header-0")["name"], column, "{when}: result {n}");
    }
}

/// The result tabs in view lie whole in the result bar under the run row, before the facts.
fn selector_whole(ui: &mut Ui, when: &str) {
    let i = ui.inspect();
    let rect = |id: &str| -> [f32; 4] {
        let r = tag_in(&i, id).unwrap_or_else(|| panic!("{when}: no `{id}`"))["rect"].clone();
        [0, 1, 2, 3].map(|k| r[k].as_f64().unwrap() as f32)
    };
    let (run, facts) = (rect("query-run"), rect("query-facts"));
    let parts: Vec<String> =
        ids(&i).into_iter().filter(|id| id.starts_with("result-") && !id.starts_with("result-list-") && visible(&tag_in(&i, id))).collect();
    assert!(!parts.is_empty(), "{when}: result tabs");
    for id in &parts {
        let [x, y, w, _] = rect(id);
        assert!(x + w <= facts[0] && y >= run[1] + run[3], "{when}: `{id}` lies under the run row, before the facts");
    }
}

#[test]
fn a_result_cut_at_the_cap_says_so() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select generate_series(1, 25000) as n");
    ui.ctrl("Enter");
    ui.ran();
    let r = ui.query()["results"][0].clone();
    assert_eq!((r["rows"].clone(), r["rowsSeen"].clone(), r["complete"].clone()), (json!(10_000), json!(25_000), json!(false)), "{r}");
    assert_eq!(ui.value("query-rows"), "First 10,000 of 25,000 rows");
    // A whole result says how many rows it has.
    ui.ctrl("A");
    ui.text("select generate_series(1, 3) as n");
    ui.ctrl("Enter");
    ui.until("the small result", |ui| ui.query()["results"][0]["rows"] == 3);
    ui.frame();
    assert_eq!(ui.value("query-rows"), "3 rows");
}

#[test]
fn statements_without_rows_show_rows_affected() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch(unique("t_query"));
    let t = &table.0;
    let mut ui = query_tab(db);
    ui.text(&format!("create table scratch.{t} (id int);\ninsert into scratch.{t} values (1), (2);\nupdate scratch.{t} set id = 3 where id = 1"));
    ui.ctrl("Enter");
    ui.ran();
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(3), "{}", ui.query());
    assert_eq!(ui.value("query-affected"), "1 row affected");
    assert!(ui.tag("grid-header-0").is_none(), "no rows, no grid");
    assert!(ui.tag("query-rows").is_none(), "no row count for a statement without rows");
    assert!(ui.value("query-facts").as_str().is_some_and(|f| f.starts_with("1 row affected ") && f.ends_with(" ms")), "the facts");
    ui.click("result-2");
    ui.frame();
    assert_eq!(ui.value("query-affected"), "2 rows affected");
    // The bar says it: nothing under it.
    let bar = as_rect(ui.rect("result-2"));
    let i = ui.inspect();
    let body = body_rect(&mut ui);
    // The value panel beside it, empty, is not under the bar.
    let under: Vec<String> = ids(&i)
        .into_iter()
        .filter(|id| !id.starts_with("status-") && !id.starts_with("value-"))
        .filter(|id| {
            let r = &tag_in(&i, id).unwrap()["rect"];
            let (x, y) = (r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32);
            y > bar.bottom() + 2.0 && y < body.bottom() && x >= body.left() && x < body.right()
        })
        .collect();
    assert_eq!(under, Vec::<String>::new(), "nothing tagged under the result bar");
}

#[test]
fn a_failed_statement_keeps_the_results_before_it() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as a;\nselect 2 as b;\nselect nope");
    ui.ctrl("Enter");
    ui.ran();
    let q = ui.query();
    assert_eq!((q["error"]["statement"].clone(), q["results"].as_array().map(Vec::len)), (json!(2), Some(2)), "{q}");
    let card = ui.value("query-error");
    assert!(card["heading"].as_str().is_some_and(|h| h.contains('3')), "the card names the third statement: {card}");
    assert_eq!(card["line"], 3);
    // The card sits above the results, which stay selectable.
    let (card_rect, grid_rect) = (ui.rect("query-error"), ui.rect("grid-header-0"));
    assert!(card_rect[1] + card_rect[3] <= grid_rect[1], "{card_rect:?} {grid_rect:?}");
    assert_eq!(ui.value("grid-header-0")["name"], "b");
    ui.click("result-1");
    ui.frame();
    assert_eq!(ui.value("grid-header-0")["name"], "a");
    assert!(ui.tag("query-error").is_some(), "the card stays");
}

#[test]
fn tab_inserts_four_spaces_and_keeps_the_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select");
    ui.key("Tab");
    ui.text("1");
    ui.frame();
    assert_eq!(ui.query()["text"], "select    1");
    assert_eq!(ui.editor()["focused"], true, "Tab kept the keyboard in the editor");
    // Shift+Tab (no outdent in M1) and Ctrl+Tab do nothing to the text, and keep the keyboard.
    ui.key("Home");
    ui.text("    ");
    ui.key_with("Tab", json!({ "shift": true }));
    ui.ctrl("Tab");
    ui.frame();
    assert_eq!(ui.query()["text"], "    select    1");
    assert_eq!(ui.editor()["focused"], true);
}

#[test]
fn the_value_panel_follows_the_result_grid() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as id, '{\"a\": [1, 2]}'::jsonb as doc union all select 2, null");
    ui.ctrl("Enter");
    ui.ran();
    ui.click("grid-cell-0-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 1], "focus": [0, 1], "whole": false }));
    let panel = ui.value("value-panel");
    assert_eq!((panel["column"].clone(), panel["label"].clone(), panel["null"].clone()), (json!("doc"), json!("jsonb"), json!(false)), "{panel}");
    ui.click("grid-cell-1-1");
    assert_eq!(ui.value("value-panel")["null"], true);
    ui.click("grid-cell-1-0");
    assert_eq!(ui.value("value-panel")["column"], "id");
    // The grid's cells copy, once the editor no longer has the keyboard.
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some("2"));
}

#[test]
fn the_result_grid_is_read_only() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select id, name from shop.customers order by id limit 5");
    ui.ctrl("Enter");
    ui.ran();
    ui.click("grid-header-0");
    ui.frame();
    assert_eq!(ui.document()["jobs"], json!([]), "a header click sorts nothing");
    ui.click_with("grid-cell-1-1", json!({ "count": 2 }));
    ui.frame();
    assert!(ui.tag("cell-editor").is_none(), "a double-click edits nothing");
    assert!(ui.tag("notice").is_none(), "{:?}", ui.tag("notice"));
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    assert_eq!(ui.tag("grid-menu-copy").map(|t| t["enabled"] != false), Some(true));
    for id in ["grid-menu-null", "grid-menu-delete"] {
        assert!(ui.tag(id).is_none(), "a query's results list no {id}");
    }
    ui.click("grid-menu-copy");
    assert_eq!(ui.copied().as_deref(), Some("Customer 2"));
}

#[test]
fn text_set_elsewhere_reaches_the_editor() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1");
    ui.frame();
    // An agent replaces the text: the editor takes it, and typing goes on from there.
    ui.execute("query.setText", json!({ "text": "select 2" }));
    ui.frame();
    assert_eq!(ui.editor()["length"], 8);
    ui.ctrl("End");
    ui.text(" + 1");
    ui.frame();
    assert_eq!(ui.query()["text"], "select 2 + 1");
    // Each tab keeps its own text.
    let first = ui.document()["active"].as_u64().unwrap();
    ui.ctrl("T");
    ui.text("select 'other'");
    ui.frame();
    ui.click(&format!("tab-{first}"));
    ui.frame();
    assert_eq!(ui.query()["text"], "select 2 + 1");
    assert_eq!(ui.editor()["focused"], true, "the editor of the tab switched to has the keyboard");
}

#[test]
fn run_buttons_say_why_they_cannot_run() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    // The buttons follow the engine's own enabled check for `query.run`: connected, a query tab
    // active. Whatever the run itself refuses (no statement, a run going) it says in a notice.
    assert!(ui.enabled("query-run") && ui.enabled("query-run-current"));
    ui.text("-- nothing here");
    ui.click("query-run");
    ui.frame();
    let notice = ui.value("notice");
    assert_eq!(notice, "There is no statement to run.");
}

/// Review Focus 4: a failure reads as an error toast (`danger`, its words and marks in
/// `on_danger`, no shadow), 12 from the window's right edge and 12 above the status bar; the
/// `notice` tag says what it always said.
#[test]
fn a_failed_command_shows_an_error_toast() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("-- nothing here");
    ui.click("query-run");
    ui.until("the toast", |ui| ui.toast().is_some());
    assert_eq!(ui.value("notice"), "There is no statement to run.");
    let t = dbee_ui_egui::theme::LIGHT;
    let toast = ui.toast().unwrap_or_else(|| panic!("the toast"));
    toast.assert_in(t.danger, t.on_danger, "a failure");
    let window = ui.harness.ctx.content_rect();
    assert_eq!(toast.rect.right(), window.right() - 12.0, "12 from the right: {:?}", toast.rect);
    assert_eq!(toast.rect.bottom(), window.bottom() - 30.0 - 12.0, "12 above the 30 tall status bar: {:?}", toast.rect);
}

#[test]
fn the_split_between_editor_and_results_drags() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let before = ui.rect("query-editor")[3];
    let split = ui.rect("query-split");
    let from = Pos2::new(split[0] + split[2] / 2.0, split[1] + split[3] / 2.0);
    let to = from + egui::vec2(0.0, 120.0);
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![button(from, true)]);
    for k in 1..=6 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))]);
    }
    ui.frame_with(vec![button(to, false)]);
    ui.frame();
    let after = ui.rect("query-editor")[3];
    assert!((after - before - 120.0).abs() < 2.0, "{before} → {after}");
    // It stays for the window's run: another query tab has the same split.
    ui.ctrl("T");
    ui.frame();
    let other = ui.rect("query-editor")[3];
    assert!((other - after).abs() < 1.0, "{after} vs {other}");
}

/// The editor lexes with the open connection's dialect: on MySQL `#` starts a comment (drawn in
/// the colour of a `--` one) and a backtick quotes a name, which on Postgres are an operator and
/// a stray character, both plain. A new editor starts with the dialect of no connection, so the
/// MySQL window shows the switch.
#[test]
fn query_highlighting_follows_the_dialect() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let highlighted = |ui: &mut Ui, text: &str| {
        ui.execute("query.setText", json!({ "text": text }));
        ui.frame();
        ui.editor()["highlighted"].clone()
    };
    let cases = [("x # y", "a comment"), ("x `y`", "a quoted name")];
    // The colour of `# c`'s `c` and of `-- d`'s `d`.
    let comment = "x # c\n-- d";
    let colours = |ui: &mut Ui| {
        ui.execute("query.setText", json!({ "text": comment }));
        ui.frame();
        let (_, g) = ui.texts().into_iter().find(|(_, g)| g.text() == comment).expect("the editor's text is drawn");
        let at = |byte: usize| g.job.sections.iter().find(|s| s.byte_range.contains(&egui::text::ByteIndex(byte))).map(|s| s.format.color);
        (at(4), at(9))
    };
    {
        let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
        ui.ctrl("T");
        ui.frame();
        for (text, what) in cases {
            assert_eq!(highlighted(&mut ui, text), true, "{text:?} on MySQL holds {what}");
        }
        let (hash, dash) = colours(&mut ui);
        assert!(hash.is_some() && hash == dash, "on MySQL `#` starts a comment: {hash:?} vs {dash:?}");
    }
    let mut ui = query_tab(db);
    for (text, _) in cases {
        assert_eq!(highlighted(&mut ui, text), false, "{text:?} on Postgres");
    }
    let (hash, dash) = colours(&mut ui);
    assert!(dash.is_some() && hash != dash, "on Postgres `#` is no comment: {hash:?} vs {dash:?}");
}

/// A MySQL query tab shows its database at the left of its toolbar, as tall as its buttons. A
/// table opened from the tree is the tree's selection, so a new query tab opens on its database;
/// a click on the chip lists the catalog's databases (not the server's own), and choosing one
/// sets the tab's. While the tab runs the chip waits.
#[test]
fn mysql_query_tab_shows_and_changes_its_database() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
    ui.click("tree-schema-shop");
    ui.double_click("tree-table-shop.orders");
    ui.until("the orders tab", |ui| ui.document()["table"]["name"] == "orders");
    ui.ctrl("T");
    ui.frame();
    assert_eq!(ui.query()["database"], "shop");
    assert_eq!(ui.value("query-database"), "shop");
    let (chip, run) = (ui.rect("query-database"), ui.rect("query-run"));
    assert!(chip[0] + chip[2] <= run[0], "the chip is left of Run: {chip:?} {run:?}");
    assert!((chip[1] - run[1]).abs() < 0.5 && (chip[3] - run[3]).abs() < 0.5, "as tall as Run, level with it: {chip:?} {run:?}");
    ui.click("query-database");
    ui.frame();
    let i = ui.inspect();
    for name in ["many", "odd", "scratch", "shop"] {
        assert!(tag_in(&i, &format!("query-database-{name}")).is_some(), "{name} is listed: {:?}", ids(&i));
    }
    for system in ["mysql", "information_schema", "performance_schema", "sys"] {
        assert!(tag_in(&i, &format!("query-database-{system}")).is_none(), "{system} is the server's own");
    }
    // The list's heading, the menu's 6 over its first row.
    let first = ui.rect("query-database-many");
    let heading = ui.texts().into_iter().find(|(_, g)| g.text() == "DATABASES").map(|(r, _)| r).expect("the list's heading is drawn");
    assert!((first[1] - heading.bottom() - 6.0).abs() < 0.5, "6 between the heading {heading:?} and the first row {first:?}");
    ui.click("query-database-many");
    ui.frame();
    assert_eq!(ui.query()["database"], "many");
    assert_eq!(ui.value("query-database"), "many");
    assert!(ui.tag("query-database-shop").is_none(), "the list closed");
    // While the tab runs, the chip waits, and its list goes: a run started with the list open
    // (Ctrl+Enter reaches it) holds on a lock until the test lets it go.
    let key = unique("dbee_lock");
    let held = mysql.hold_lock(&key).unwrap();
    ui.execute("query.setText", json!({ "text": format!("SELECT 1 FROM DUAL WHERE {}", mysql.waits_for_lock_sql(&key)) }));
    ui.click("query-database");
    ui.frame();
    assert!(ui.tag("query-database-shop").is_some(), "the list is open");
    ui.ctrl("Enter");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    ui.frame();
    assert!(!ui.enabled("query-database"), "disabled while the tab runs");
    let listed: Vec<String> = ids(&ui.inspect()).into_iter().filter(|id| id.starts_with("query-database-")).collect();
    assert!(listed.is_empty(), "no list while the tab runs: {listed:?}");
    drop(held);
    ui.ran();
    assert!(ui.enabled("query-database"));
    assert!(ui.tag("query-database-shop").is_none(), "the list stays closed");
    assert_eq!(ui.query()["error"], Json::Null, "the run in `many` went through");
}

/// Every way of opening a query tab opens it on the tree's selection: a selected database, or a
/// selected table's. With nothing selected it opens on the connection's database, or on none.
#[test]
fn a_new_query_tab_opens_on_the_trees_selection() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    {
        let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
        ui.ctrl("T");
        ui.frame();
        assert_eq!(ui.query()["database"], Json::Null, "nothing selected, and no database in the settings");
        assert_eq!(ui.value("query-database"), Json::Null);
        // A `USE` it runs becomes its database, as the engine reads it back.
        ui.execute("query.setText", json!({ "text": "USE odd" }));
        ui.ctrl("Enter");
        ui.ran();
        assert_eq!(ui.value("query-database"), "odd");
        // Ctrl+T on a selected database.
        ui.click("tree-schema-odd");
        ui.ctrl("T");
        ui.frame();
        assert_eq!(ui.query()["database"], "odd");
        // A table clicked after a database wins: its own database.
        ui.click("tree-schema-odd");
        ui.click("tree-table-many.t_001");
        ui.click("tab-new");
        ui.frame();
        assert_eq!(ui.query()["database"], "many");
        // + on a selected table.
        ui.click("tree-schema-shop");
        ui.click("tree-table-shop.customers");
        ui.click("tab-new");
        ui.frame();
        assert_eq!(ui.query()["database"], "shop");
        // The File menu.
        ui.click("tree-schema-scratch");
        ui.click("menu-File");
        ui.click("menu-tab.newQuery");
        ui.frame();
        assert_eq!(ui.query()["database"], "scratch");
    }
    let mut ui = connected_to_target(db, &mysql, Some("shop"), APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    assert_eq!(ui.query()["database"], "shop", "nothing selected: the settings' database");
    assert_eq!(ui.value("query-database"), "shop");
}

/// Drops `scratch.<name>` on `server` when the test ends, also when it fails.
struct DropAfter<'a>(&'a dbee_testkit::Server, String);

impl Drop for DropAfter<'_> {
    fn drop(&mut self) {
        let _ = dbee_testkit::sql(self.0, &format!("DROP TABLE IF EXISTS scratch.{}", self.1));
    }
}

/// A selected table that a refresh no longer finds is forgotten: a new query tab gets nothing
/// from it.
#[test]
fn a_selection_gone_after_a_refresh_gives_a_new_tab_nothing() {
    let db = dbee_testkit::require_testdb!();
    let server = db.server("mysql80").unwrap();
    let name = unique("t_sel");
    let _drop = DropAfter(server, name.clone());
    dbee_testkit::mysql(server, &format!("CREATE TABLE scratch.{name} (id INT PRIMARY KEY)")).unwrap();
    let mysql = dbee_testkit::target(server);
    let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
    ui.click("tree-schema-scratch");
    ui.click(&format!("tree-table-scratch.{name}"));
    dbee_testkit::mysql(server, &format!("DROP TABLE scratch.{name}")).unwrap();
    ui.click("tree-refresh");
    ui.ok("engine.wait", json!({}));
    ui.until("the refreshed tree", |ui| ui.tag(&format!("tree-table-scratch.{name}")).is_none());
    ui.click("tab-new");
    ui.frame();
    assert_eq!(ui.query()["database"], Json::Null, "nothing selected any more, and no database in the settings");
}

/// No chip where the engine has no `USE`: Postgres and SQLite.
#[test]
fn no_database_chip_without_use() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let pg = dbee_testkit::target(pg14_server(db));
    for (t, schema) in [(&pg, "shop"), (&sqlite, "main")] {
        let mut ui = connected_to_target(db, t, None, APP_SIZE);
        ui.click(&format!("tree-schema-{schema}"));
        ui.ctrl("T");
        ui.frame();
        assert!(ui.tag("query-run").is_some(), "{}: the toolbar is drawn", t.name());
        assert!(ui.tag("query-database").is_none(), "{}: no database chip", t.name());
        assert_eq!(ui.query()["database"], Json::Null, "{}: a query tab has no database of its own", t.name());
    }
}

#[test]
fn the_layouter_tokenizes_only_when_the_text_changes() {
    use dbee_ui_egui::query_tab::{EditorState, editor};
    let ctx = egui::Context::default();
    let mut ed = EditorState::new("select 1;\n-- two\nselect 'x' from t".into());
    let frame = |ed: &mut EditorState| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            editor(ui, egui::Id::new("timing-editor"), ed, None);
        })
        .drop_without_applying_deltas();
    };
    for _ in 0..5 {
        frame(&mut ed);
    }
    assert_eq!(ed.tokenized(), 1, "the same text is tokenized once");
    ed.set_text("select 2".into());
    frame(&mut ed);
    frame(&mut ed);
    assert_eq!(ed.tokenized(), 2, "a new text once more");
}

/// The machine and its load average, which a timing test prints with its numbers
/// (docs/testing.md §6).
fn machine() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|m| m.trim().to_string()))
        .unwrap_or_else(|| "an unknown CPU".into());
    let threads = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "unknown".into());
    format!("{} {}, {cpu}, {threads} threads, load average {load}", std::env::consts::OS, std::env::consts::ARCH)
}

/// SQL of about `bytes` bytes: a script of short lines, or (`one_line`) the same on one line.
fn script(bytes: usize, one_line: bool) -> String {
    let line = "select o.id, c.name, 'shipped' as status, 42.5 from shop.orders o join shop.customers c on c.id = o.customer_id; -- note\n";
    let mut s = String::with_capacity(bytes + line.len());
    while s.len() < bytes {
        s.push_str(line);
    }
    if one_line { s.replace('\n', " ") } else { s }
}

/// The median and the slowest of `frames`.
fn median_max(mut frames: Vec<Duration>) -> (Duration, Duration) {
    frames.sort();
    (frames[frames.len() / 2], frames[frames.len() - 1])
}

/// Keystroke frames (with their tessellation) of an editor alone in a plain egui context, the
/// cursor at the end of `text`: the query editor when `highlighted`, else egui's plain code editor.
fn editor_keystrokes(text: &str, highlighted: bool, keys: usize) -> (Duration, Duration) {
    use dbee_ui_egui::query_tab::{EditorState, editor};
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::light());
    let id = egui::Id::new("timing-editor");
    let mut ed = EditorState::new(text.to_string());
    let mut plain = text.to_string();
    let end = text.chars().count();
    let mut frames = Vec::new();
    for i in 0..=keys {
        let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 700.0))), ..Default::default() };
        if i > 0 {
            raw.events.push(Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
        }
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| {
            if i == 0 {
                // The keyboard, with the cursor at the end.
                ui.memory_mut(|m| m.request_focus(id));
                let mut state = egui::text_edit::TextEditState::default();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(end))));
                egui::TextEdit::store_state(ui.ctx(), id, state);
            }
            if highlighted {
                editor(ui, id, &mut ed, None);
            } else {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    egui::TextEdit::multiline(&mut plain).id(id).code_editor().desired_width(f32::INFINITY).show(ui);
                });
            }
        });
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        if i > 0 {
            frames.push(start.elapsed());
        }
    }
    let typed = if highlighted { &ed.text } else { &plain };
    assert_eq!(typed.len(), text.len() + keys, "every key went in");
    median_max(frames)
}

/// Keystroke frames (with their tessellation) of the whole window in a plain egui context, which
/// has no AccessKit tree (kittest always builds one; the app builds one only for a screen reader),
/// typing at the end of a query tab holding `text`.
fn window_keystrokes(db: &TestDb, text: &str, keys: usize) -> (Duration, Duration) {
    use dbee_engine::{Outcome, Screen, TabId};
    let home = temp_home(None, None);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let session = Session::new(SessionOptions {
        config_dir: Some(home.home.join("config")),
        secrets: Some(secrets),
        ssh: SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None },
        notify: None,
        allowed_targets: None,
        allowed_dirs: None,
        drivers: dbee_drivers::all(),
    })
    .unwrap();
    let mut app = DbeeApp::new(session, None);
    let ctx = egui::Context::default();
    let frame = |app: &mut DbeeApp, events: Vec<Event>| {
        let mut raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 900.0))), events, ..Default::default() };
        app.feed_input(&ctx, &mut raw);
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        start.elapsed()
    };
    let Ok(Outcome::Done(saved)) = app.run("connection.save", json!({ "config": pg14(db), "password": db.password })) else { panic!("saved") };
    app.run("connection.open", json!({ "id": saved["id"] })).unwrap();
    let start = Instant::now();
    while app.session().state().screen != Screen::Workspace {
        assert!(start.elapsed() < WAIT, "connected");
        frame(&mut app, Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
    let Ok(Outcome::Done(tab)) = app.run("tab.newQuery", json!({ "text": text })) else { panic!("a query tab") };
    let id = dbee_ui_egui::query_tab::editor_id(TabId(tab["tab"].as_u64().unwrap()));
    frame(&mut app, Vec::new());
    ctx.memory_mut(|m| m.request_focus(id));
    let mut state = egui::text_edit::TextEditState::default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(text.chars().count()))));
    egui::TextEdit::store_state(&ctx, id, state);
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    let frames = (0..keys).map(|i| frame(&mut app, vec![Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() })])).collect();
    let Some(dbee_engine::Tab::Query(q)) = app.session().state().active_tab() else { panic!("the query tab") };
    assert_eq!(q.text.len(), text.len() + keys, "every key reached the tab");
    median_max(frames)
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn keystroke_frames_in_big_scripts() {
    let keys = 20;
    let mut slow_32k = Duration::ZERO;
    println!("keystroke frames, editor alone (median, max), on {}", machine());
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        for one_line in [false, true] {
            let text = script(size, one_line);
            let shape = if one_line { "one line" } else { "short lines" };
            let (hm, hx) = editor_keystrokes(&text, true, keys);
            let (pm, px) = editor_keystrokes(&text, false, keys);
            println!("  {:>5} KiB, {shape:<11}: highlighted {hm:?} ({hx:?}); plain TextEdit {pm:?} ({px:?})", text.len() / 1024);
            if size == 32 * 1024 && !one_line {
                slow_32k = hm;
            }
        }
    }
    // The whole window, typing into a query tab: without an AccessKit tree, then with one (kittest).
    let db = dbee_testkit::require_testdb!();
    println!("keystroke frames, whole window without AccessKit (median, max):");
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        let text = script(size, false);
        let (m, x) = window_keystrokes(db, &text, keys);
        println!("  {:>5} KiB, short lines: {m:?} ({x:?})", text.len() / 1024);
    }
    let mut ui = Ui::connected(db);
    println!("keystroke frames, whole window with AccessKit, kittest (median, max):");
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        let text = script(size, false);
        ui.execute("tab.newQuery", json!({ "text": text }));
        ui.frame();
        ui.click("query-editor");
        ui.ctrl("End");
        let mut frames = Vec::new();
        for i in 0..keys {
            ui.harness.input_mut().events.push(Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
            let start = Instant::now();
            ui.frame();
            let shapes = ui.harness.output().shapes.clone();
            let ppp = ui.harness.ctx.pixels_per_point();
            let _ = ui.harness.ctx.tessellate(shapes, ppp);
            frames.push(start.elapsed());
        }
        let (m, x) = median_max(frames);
        println!("  {:>5} KiB, short lines: {m:?} ({x:?})", text.len() / 1024);
        assert_eq!(ui.query()["text"].as_str().map(str::len), Some(text.len() + keys), "every key reached the tab");
        ui.execute("tab.close", json!({ "force": true }));
    }
    // A script of 32 KiB keeps a keystroke within two frames at 60 Hz.
    assert!(slow_32k < Duration::from_millis(33), "{slow_32k:?}");
}

/// `select` of `columns` columns over 10,000 rows: NULLs, or (`text`) short numbers as text.
fn wide(columns: usize, text: bool) -> String {
    let cells: Vec<String> = (1..=columns).map(|i| if text { format!("(g * {i})::text as c{i:04}") } else { format!("null as c{i:04}") }).collect();
    format!("select {} from generate_series(1, 10000) g", cells.join(", "))
}

/// Frames (with their tessellation) of the whole window in a plain egui context, which has no
/// AccessKit tree (as the app without a screen reader), showing the result of `sql` in a query
/// tab: the median and slowest of `n` frames with nothing changing, then of `n` frames that each
/// scroll the result grid down. The run must keep all of `columns` × 10,000 rows.
fn result_frames(db: &TestDb, sql: &str, columns: usize, n: usize) -> ((Duration, Duration), (Duration, Duration)) {
    use dbee_engine::{Outcome, Screen};
    let home = temp_home(None, None);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let session = Session::new(SessionOptions {
        config_dir: Some(home.home.join("config")),
        secrets: Some(secrets),
        ssh: SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None },
        notify: None,
        allowed_targets: None,
        allowed_dirs: None,
        drivers: dbee_drivers::all(),
    })
    .unwrap();
    let mut app = DbeeApp::new(session, None);
    let ctx = egui::Context::default();
    let size = egui::vec2(1400.0, 900.0);
    let frame = |app: &mut DbeeApp, events: Vec<Event>| {
        let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        app.feed_input(&ctx, &mut raw);
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        start.elapsed()
    };
    let Ok(Outcome::Done(saved)) = app.run("connection.save", json!({ "config": pg14(db), "password": db.password })) else { panic!("saved") };
    app.run("connection.open", json!({ "id": saved["id"] })).unwrap();
    let start = Instant::now();
    while app.session().state().screen != Screen::Workspace {
        assert!(start.elapsed() < WAIT, "connected");
        frame(&mut app, Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
    app.run("tab.newQuery", json!({ "text": sql })).unwrap();
    app.run("query.run", json!({})).unwrap();
    let start = Instant::now();
    loop {
        frame(&mut app, Vec::new());
        if let Some(dbee_engine::Tab::Query(q)) = app.session().state().active_tab()
            && q.running.is_none()
        {
            assert!(q.error.is_none(), "the run failed: {:?}", q.error);
            let [outcome] = &q.results[..] else { panic!("one result, not {}", q.results.len()) };
            let r = &outcome.result;
            assert_eq!((r.columns.len(), r.rows.len(), r.complete), (columns, 10_000, true), "the whole result is kept");
            break;
        }
        assert!(start.elapsed() < WAIT, "the run ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    let idle = (0..n).map(|_| frame(&mut app, Vec::new())).collect();
    // The wheel over the result grid (the window's lower half).
    let over = Pos2::new(size.x * 0.6, size.y * 0.8);
    let wheel = || Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    frame(&mut app, vec![Event::PointerMoved(over)]);
    let scrolling = (0..n).map(|_| frame(&mut app, vec![wheel()])).collect();
    (median_max(idle), median_max(scrolling))
}

/// How long a frame takes with a wide result in view, by the number of columns (50, 100, 300 and
/// 1,664, Postgres's most in a result), 10,000 rows of NULLs or of short texts: the whole window
/// without an AccessKit tree (the app's case), unchanged and scrolling, and in kittest, which
/// builds one. Reported, not bounded: a wide result's frame grows with its columns
/// (`docs/gaps.md`).
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn wide_result_frames() {
    let db = dbee_testkit::require_testdb!();
    let frames = 15;
    println!("frames with a wide result in view (median, max), on {}", machine());
    for columns in [50, 100, 300, 1_664] {
        for text in [false, true] {
            let sql = wide(columns, text);
            let ((idle, idle_max), (scroll, scroll_max)) = result_frames(db, &sql, columns, frames);
            let mut ui = Ui::connected(db);
            ui.execute("tab.newQuery", json!({ "text": sql }));
            ui.execute("query.run", json!({}));
            ui.ran();
            let q = ui.query();
            assert!(q["error"].is_null(), "{}", q["error"]);
            let r = &q["results"][0];
            assert_eq!((r["columns"].as_array().map(Vec::len), r["rows"].as_u64()), (Some(columns), Some(10_000)), "the whole result in kittest");
            let kittest: Vec<Duration> = (0..frames)
                .map(|_| {
                    let start = Instant::now();
                    ui.frame();
                    start.elapsed()
                })
                .collect();
            let (k, kx) = median_max(kittest);
            let cells = if text { "texts" } else { "NULLs" };
            println!(
                "  {columns:>5} columns of {cells}: window unchanged {idle:?} ({idle_max:?}), scrolling {scroll:?} ({scroll_max:?}); kittest unchanged {k:?} ({kx:?})"
            );
        }
    }
}

#[test]
fn a_failed_transaction_says_a_rollback_ends_it() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let run = |ui: &mut Ui, sql: &str| {
        ui.ctrl("A");
        ui.text(sql);
        ui.ctrl("Enter");
        ui.ran();
    };
    run(&mut ui, "BEGIN; SELECT 1 / 0;");
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("SQLSTATE 22012")), "{card}");
    assert_eq!((card["rollback"].clone(), card.get("note")), (json!(true), None), "the transaction failed: ROLLBACK, no note: {card}");
    // The transaction stays open and failed: the next run offers ROLLBACK too.
    run(&mut ui, "SELECT 1");
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("SQLSTATE 25P02")), "{card}");
    assert_eq!(card["rollback"], true, "{card}");
    assert!(ui.tag("query-rollback").is_some());
    run(&mut ui, "ROLLBACK");
    assert!(ui.tag("query-rollback").is_none(), "a ROLLBACK statement ended it too");
    run(&mut ui, "SELECT 1");
    assert!(ui.tag("query-error").is_none(), "after the ROLLBACK statements run again");
}

/// The toolbar's buttons, moved to `components::button`, say in `ui.inspect` what they said
/// before: ids, values, enabled flags. Cancel, off, says why on hover.
#[test]
fn moved_buttons_keep_their_ids_values_and_flags() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.frame();
    let idle: [Moved<'_>; 2] =
        [("query-run", Some(json!("Run")), true, "RUN"), ("query-run-current", Some(json!("Run current")), true, "RUN CURRENT")];
    assert_moved(&mut ui, &idle, "idle");
    assert!(ui.tag("query-cancel").is_none(), "idle: no Cancel");
    ui.click("query-run");
    ui.until("Cancel to show", |ui| ui.tag("query-cancel").is_some());
    let running: [Moved<'_>; 3] = [
        ("query-run", Some(json!("Run")), false, "RUN"),
        ("query-run-current", Some(json!("Run current")), false, "RUN CURRENT"),
        ("query-cancel", Some(json!("Cancel")), true, "CANCEL"),
    ];
    assert_moved(&mut ui, &running, "running");
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
}

/// Within half a point.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

/// Light then dark: the command that switches to each, and its tokens.
const MODES: [(&str, dbee_ui_egui::theme::Tokens); 2] = [("ui.themeLight", dbee_ui_egui::theme::LIGHT), ("ui.themeDark", dbee_ui_egui::theme::DARK)];

/// A query tab showing the rows of `sql`, the pointer off the grid.
fn results_of(db: &TestDb, sql: &str) -> Ui {
    let mut ui = Ui::connected(db);
    ui.execute("tab.newQuery", json!({ "text": sql }));
    ui.execute("query.run", json!({}));
    ui.frame();
    ui.hover_away();
    ui
}

/// At `x`: the header's `ink` line, row `lit`'s `selection` ground, and row `plain`'s `card`
/// ground with its 1 px `line` along the bottom; the line and grounds run to the area's right edge.
/// Column `col` is in view.
fn assert_full_width_at(ui: &mut Ui, x: f32, (lit, plain, col): (usize, usize, usize), t: &dbee_ui_egui::theme::Tokens, at: &str) {
    let right = ui.body_x().max;
    let head = ui.drawn(&format!("grid-header-{col}"));
    let (lit, plain) = (ui.drawn(&format!("grid-cell-{lit}-{col}")), ui.drawn(&format!("grid-cell-{plain}-{col}")));
    let reaches = |d: Option<(egui::Color32, egui::Rect)>| d.is_some_and(|(_, r)| close(r.right(), right));
    let ink = painted_at(ui, egui::pos2(x, head.bottom() - 0.5));
    assert!(ink.is_some_and(|(c, _)| c == t.ink) && reaches(ink), "{at}: the header's ink line at {x}, to {right}: {ink:?}");
    let ground = painted_at(ui, egui::pos2(x, lit.center().y));
    let selected = ground.is_some_and(|(c, r)| c == t.selection && r.height() > 20.0);
    assert!(selected && reaches(ground), "{at}: the selected row's ground at {x}, to {right}: {ground:?}");
    assert_eq!(colour_at(ui, egui::pos2(x, plain.center().y)), Some(t.card), "{at}: a plain row's ground at {x}");
    let line = painted_at(ui, egui::pos2(x, plain.bottom() - 0.5));
    let thin = line.is_some_and(|(c, r)| c == t.line && r.height() <= 1.0);
    assert!(thin && reaches(line), "{at}: a row's line at {x}, to {right}: {line:?}");
}

/// The header's `ink` line, the rows' lines and their selection and hover grounds run the grid's
/// area to its right edge, past the last column (Screens 43147): a 2-column result in a wide tab.
/// A row lights under the pointer wherever it is on the row.
#[test]
fn lines_and_grounds_run_the_full_width() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = results_of(db, "select g as id, 'row ' || g as note from generate_series(1, 5) g");
    for (mode, t) in MODES {
        ui.execute(mode, json!({}));
        ui.click("grid-cell-1-0");
        ui.hover_away();
        let (left, right) = (ui.body_x().min, ui.body_x().max);
        let last = ui.drawn("grid-header-1");
        assert!(last.right() < right - 100.0, "{mode}: room past the last column: {last:?}, the area to {right}");
        let past = (last.right() + right) / 2.0;
        for x in [past, right - 2.0] {
            assert_full_width_at(&mut ui, x, (1, 0, 0), &t, mode);
        }
        // Row 3 under the pointer, past the last column, then on a cell: the whole row lights.
        let (a, b) = (ui.drawn("grid-cell-3-0"), ui.drawn("grid-cell-3-1"));
        for (what, at) in [("past the last column", egui::pos2(past, a.center().y)), ("on a cell", b.center())] {
            ui.hover(at);
            ui.frame();
            for x in [left + 3.0, a.left() + 4.0, b.left() + 4.0, past, right - 2.0] {
                let p = egui::pos2(x, a.center().y);
                assert_eq!(colour_at(&ui, p), Some(t.hover), "{mode}: the pointer {what}, row 3 at {x}: {:?}", painted_at(&ui, p));
            }
        }
    }
}

/// A result wider than its area, scrolled to its end: the line and grounds reach the area's right
/// edge there, and every cell of the selected row in view lights, so nothing stops at the width
/// the area had in view before it scrolled.
#[test]
fn lines_and_grounds_run_the_full_width_when_scrolled() {
    let db = dbee_testkit::require_testdb!();
    let columns: Vec<String> = (1..=30).map(|i| format!("g * {i} as c{i:02}")).collect();
    let mut ui = results_of(db, &format!("select {} from generate_series(1, 5) g", columns.join(", ")));
    // Out of view: the click scrolls the grid to it, the last column.
    ui.click("grid-cell-1-29");
    for (mode, t) in MODES {
        ui.execute(mode, json!({}));
        ui.hover_away();
        let (left, right) = (ui.body_x().min, ui.body_x().max);
        let last = ui.drawn("grid-header-29");
        assert!(close(last.right(), right), "{mode}: scrolled to its end, the last column ends at the area's edge {right}: {last:?}");
        assert_full_width_at(&mut ui, right - 4.0, (1, 0, 29), &t, &format!("{mode}, scrolled"));
        let y = ui.drawn("grid-cell-1-29").center().y;
        let mut seen = 0;
        for c in 0..30 {
            // Cells out of view have no rect.
            let Some(cell) = ui.harness.state().tags().rect(&format!("grid-cell-1-{c}")) else { continue };
            let x = cell.left() + 4.0;
            if x > left && x < right - 4.0 {
                seen += 1;
                assert_eq!(colour_at(&ui, egui::pos2(x, y)), Some(t.selection), "{mode}: row 1's cell {c}, in view at {x}");
            }
        }
        assert!(seen > 5, "{mode}: {seen} cells in view");
    }
}

/// The column lines stop at the last row: under it, at each column boundary, the tab's `bg`.
#[test]
fn column_lines_stop_at_the_last_row() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = results_of(db, "select g as id, 'row ' || g as note from generate_series(1, 3) g");
    for (mode, t) in MODES {
        ui.execute(mode, json!({}));
        ui.hover_away();
        let (h0, h1, last) = (ui.drawn("grid-header-0"), ui.drawn("grid-header-1"), ui.drawn("grid-cell-2-0"));
        for x in [h0.right(), h1.right()] {
            assert_eq!(colour_at(&ui, egui::pos2(x - 0.5, last.center().y)), Some(t.line), "{mode}: the line at {x} in the last row");
            for dy in [1.0, 10.0, 100.0] {
                let p = egui::pos2(x, last.bottom() + dy);
                assert_eq!(colour_at(&ui, p), Some(t.bg), "{mode}: nothing under the last row at {p:?}: {:?}", painted_at(&ui, p));
            }
        }
    }
}

/// Review Focus 3: a query result's row number selects its row, which copies and shows as JSON in
/// the panel; its menu has the copy items only, never Set as NULL or Delete row.
#[test]
fn a_query_result_row_selects_and_copies_but_never_deletes() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select id, name from shop.customers order by id limit 5");
    ui.ctrl("Enter");
    ui.ran();
    ui.click("grid-row-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [1, 1], "whole": true }));
    assert_eq!(ui.value("grid-row-1"), json!({ "number": 2, "selected": true }));
    assert_eq!(ui.value("grid-row-0"), json!({ "number": 1, "selected": false }));
    assert_eq!(ui.editor()["focused"], false, "the grid took the keyboard");
    let panel = ui.value("value-panel");
    assert_eq!((panel["row"].clone(), panel["column"].clone()), (json!(2), json!("")), "{panel}");
    assert_eq!(ui.value("value-text")["text"], "{\n  \"id\": 2,\n  \"name\": \"Customer 2\"\n}");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("2\tCustomer 2"));
    ui.right_click("grid-row-3");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [3, 0], "focus": [3, 1], "whole": true }), "a right-click selects the row");
    for id in ["grid-menu-copy", "grid-menu-copy-header", "grid-menu-copy-csv", "grid-menu-copy-json", "grid-menu-copy-markdown"] {
        assert!(ui.tag(id).is_some(), "{id}");
    }
    assert!(ui.tag("grid-menu-null").is_none() && ui.tag("grid-menu-delete").is_none(), "nothing that edits");
    ui.click("grid-menu-copy-json");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("[\n  {\"id\": 4, \"name\": \"Customer 4\"}\n]"));
}

/// The tab body's rect: from the sidebar to the window's edge, from the strip's line down to
/// about the status bar.
fn body_rect(ui: &mut Ui) -> egui::Rect {
    let x = ui.tab_x();
    let tab = ui.document()["active"].as_u64().expect("a tab is active");
    let top = ui.rect(&format!("tab-{tab}"));
    let status = ui.rect("status-connection");
    egui::Rect::from_x_y_ranges(x, egui::Rangef::new(top[1] + top[3], status[1]))
}

/// `[x, y, w, h]` as a rect.
fn as_rect(r: [f32; 4]) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]))
}

/// The editor is the area: from the strip's line, edge to edge, with no box, edge or gutter line;
/// the line numbers 12 in, the text 12 after them and 12 under the top.
#[test]
fn the_editor_runs_edge_to_edge() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    ui.text("select 1");
    ui.frame();
    let body = body_rect(&mut ui);
    let ed = as_rect(ui.rect("query-editor"));
    assert!(close(ed.left(), body.left()) && close(ed.right(), body.right()), "edge to edge: {ed:?} in {body:?}");
    assert!((ed.top() - body.top()).abs() <= 1.0, "from the strip's line: {ed:?} in {body:?}");
    let digit = ui.harness.ctx.fonts_mut(|f| f.glyph_width(&dbee_ui_egui::theme::Text::Code.font(), '0'));
    let number = ui.text_in(ed, "1").unwrap_or_else(|| panic!("the line number")).0;
    let text = ui.text_in(ed, "select 1").unwrap_or_else(|| panic!("the text")).0;
    assert!((number.right() - (body.left() + 12.0 + 2.0 * digit)).abs() <= 1.0, "the number's box 12 in: {number:?}");
    assert!((text.left() - (number.right() + 12.0)).abs() <= 1.0, "the text 12 after the gutter: {text:?}, {number:?}");
    assert!((text.top() - (ed.top() + 12.0)).abs() <= 1.0, "the text 12 under the top: {text:?} in {ed:?}");
    let t = dbee_ui_egui::theme::LIGHT;
    for (_, shape) in ui.shapes() {
        match shape {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.rect.intersects(ed.shrink(2.0)) => {
                assert!(r.stroke.color != t.line_strong && r.stroke.color != t.ink, "no box round the editor: {r:?}");
            }
            egui::Shape::LineSegment { points, stroke } if points[0].x == points[1].x && ed.shrink(2.0).contains(points[0]) => {
                assert!(stroke.color != t.line, "no line between gutter and text: {points:?}");
            }
            _ => {}
        }
    }
}

/// The split is the run row's top line: dragged, it moves the run row with it.
#[test]
fn the_split_is_the_run_rows_top_line() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    let split = as_rect(ui.rect("query-split"));
    let run = as_rect(ui.rect("query-run"));
    // The row is 35 tall above its line, its buttons 28 tall in its middle.
    let row_top = run.top() - 3.5;
    assert!((split.center().y - row_top).abs() <= 1.0, "the split on the row's top line: {split:?}, the row from {row_top}");
    assert!((split.height() - 8.0).abs() <= 0.5, "an 8 tall zone: {split:?}");
    let from = split.center();
    let to = from + egui::vec2(0.0, 40.0);
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![button(from, true)]);
    for k in 1..=4 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 4.0))]);
    }
    ui.frame_with(vec![button(to, false)]);
    ui.frame();
    let moved = as_rect(ui.rect("query-run"));
    assert!((moved.top() - run.top() - 40.0).abs() <= 1.0, "RUN moved with it: {run:?} → {moved:?}");
}

/// Idle: RUN 12 in, RUN CURRENT 4 after it, no CANCEL, no sentence on how to run, and nothing
/// tagged under the row.
#[test]
fn the_run_row_matches_the_page() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    let body = body_rect(&mut ui);
    let (run, current) = (as_rect(ui.rect("query-run")), as_rect(ui.rect("query-run-current")));
    assert!((run.left() - (body.left() + 12.0)).abs() <= 1.0, "RUN 12 in: {run:?} in {body:?}");
    assert!((current.left() - (run.right() + 4.0)).abs() <= 1.0, "RUN CURRENT 4 after: {current:?}");
    assert!(ui.tag("query-cancel").is_none(), "no CANCEL while idle");
    assert!(ui.tag("query-hint").is_none(), "no sentence on how to run");
    let i = ui.inspect();
    let under: Vec<String> = ids(&i)
        .into_iter()
        .filter(|id| !id.starts_with("status-"))
        .filter(|id| {
            let r = &tag_in(&i, id).unwrap()["rect"];
            let y = r[1].as_f64().unwrap() as f32;
            let x = r[0].as_f64().unwrap() as f32;
            y > run.bottom() + 4.0 && y < body.bottom() && x >= body.left() && x < body.right()
        })
        .collect();
    assert_eq!(under, Vec::<String>::new(), "nothing tagged under the run row");
}

/// Runs `select pg_sleep(30)` with Run; frames until the run goes, then on until the window's
/// clock has gone `secs` past the frame that first showed it, `check` on each frame with the time
/// gone so far.
fn held_run(ui: &mut Ui, secs: f64, check: impl FnMut(&mut Ui, f64)) {
    ui.click("query-run");
    ui.frames_while(|ui| !ui.query()["running"].is_null(), secs, check);
}

/// Review Focus 2: CANCEL shows, and RUN goes off, only once the run has gone 300 ms, counted
/// afresh for each run.
#[test]
fn cancel_shows_from_300_ms() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), ..Setup::default() });
    ui.ctrl("T");
    ui.frame();
    ui.text("select pg_sleep(30)");
    ui.frame();
    for run in ["first", "second"] {
        let mut shown = false;
        held_run(&mut ui, 0.45, |ui, gone| {
            if gone <= 0.25 {
                assert!(ui.tag("query-cancel").is_none(), "{run} run at {gone:.2} s: no CANCEL yet");
                assert!(ui.enabled("query-run"), "{run} run at {gone:.2} s: RUN as idle");
            }
            if gone >= 0.4 {
                assert!(ui.tag("query-cancel").is_some() && ui.enabled("query-cancel"), "{run} run at {gone:.2} s: CANCEL");
                assert!(!ui.enabled("query-run") && !ui.enabled("query-run-current"), "{run} run at {gone:.2} s: RUN off");
                shown = true;
            }
        });
        assert!(shown, "{run} run: CANCEL showed");
        ui.click("query-cancel");
        ui.until("the cancel", |ui| ui.query()["running"].is_null() && ui.query()["error"]["code"] == "cancelled");
        ui.frame();
        assert!(ui.tag("query-cancel").is_none(), "{run} run: no CANCEL once it ended");
    }
}

/// A2+: before the first run there is no result bar.
#[test]
fn no_result_bar_before_the_first_run() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    for id in ["query-facts", "result-1", "query-export", "query-rows", "query-affected"] {
        assert!(ui.tag(id).is_none(), "no `{id}` before the first run");
    }
    // The value toggle is the footer's until it moves into the result bar.
}

/// One result: one tab named for what it works on, the facts with the time, EXPORT off; the bar
/// right under the run row and the grid under it.
#[test]
fn one_result_has_one_tab_and_the_facts() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    ui.text("SELECT * FROM shop.orders LIMIT 50");
    ui.ctrl("Enter");
    ui.ran();
    let tab = as_rect(ui.rect("result-1"));
    assert!(ui.text_in(tab.expand(1.0), "SELECT … orders").is_some(), "the tab's title as written");
    assert_eq!(ui.value("result-1")["shown"], true);
    assert_eq!(ui.value("query-rows"), "50 rows");
    let facts = ui.value("query-facts");
    let f = facts.as_str().unwrap_or_default();
    let ms = f.strip_prefix("50 rows ").and_then(|f| f.strip_suffix(" ms"));
    assert!(ms.is_some_and(|ms| ms.parse::<u64>().is_ok()), "the facts: {facts}");
    assert!(!ui.enabled("query-export"), "EXPORT off");
    let export = ui.drawn("query-export").center();
    ui.rest(export);
    assert!(ui.texts().iter().any(|(_, g)| g.text() == dbee_ui_egui::components::results::EXPORT_LATER), "EXPORT says why");
    let run = as_rect(ui.rect("query-run"));
    // The run row is 35 tall, its buttons 28 in its middle, then its 1 px line.
    let row_bottom = run.bottom() + 3.5 + 1.0;
    assert!((tab.top() - row_bottom).abs() <= 1.0, "the bar right under the run row: {tab:?}, the row to {row_bottom}");
    let header = as_rect(ui.rect("grid-header-0"));
    assert!(header.top() >= tab.bottom(), "the grid under the bar: {header:?} {tab:?}");
}

/// Review Focus 2: the bar says RUNNING with its timer only once a run has gone 300 ms, keeping
/// the last results' tabs; the timer starts afresh with each run.
#[test]
fn the_running_state_waits_300_ms_and_restarts_per_run() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), ..Setup::default() });
    ui.ctrl("T");
    ui.frame();
    ui.text("select 1 as a");
    ui.ctrl("Enter");
    ui.ran();
    let old = ui.value("query-facts");
    assert!(old.as_str().is_some_and(|f| f.starts_with("1 row ")), "{old}");
    ui.execute("query.setText", json!({ "text": "select pg_sleep(30)" }));
    ui.frame();
    for run in ["first", "second"] {
        let mut first_shown: Option<String> = None;
        held_run(&mut ui, 0.5, |ui, gone| {
            let facts = ui.value("query-facts");
            let f = facts.as_str().unwrap_or_default().to_owned();
            // The window saw the run a frame or two before `gone` started counting.
            if gone <= 0.15 {
                assert!(!f.starts_with("Running"), "{run} run at {gone:.2} s: not yet running: {f}");
            }
            if f.starts_with("Running") && first_shown.is_none() {
                first_shown = Some(f.clone());
            }
            if gone >= 0.35 {
                assert!(f.starts_with("Running 00:00."), "{run} run at {gone:.2} s: {f}");
                // The first run keeps the earlier result; the second follows a cancel, which kept none.
                if run == "first" {
                    assert!(visible(&ui.tag("result-1")), "{run} run: the last results' tab stays");
                }
            }
        });
        let first = first_shown.unwrap_or_default();
        assert!(first.starts_with("Running 00:00.3"), "{run} run: the timer from 00:00.3: {first}");
        ui.click("query-cancel");
        ui.until("the cancel", |ui| ui.query()["running"].is_null() && ui.query()["error"]["code"] == "cancelled");
        ui.frame();
        let f = ui.value("query-facts");
        assert!(f.as_str().is_some_and(|f| f.starts_with("Cancelled ")), "{run} run: the new facts: {f}");
    }
}

/// Review Focus 1: dbee's own cancel shows no band, the bar says it; a stop dbee didn't ask for
/// (a `statement_timeout`) is the red band, `Failed` not in the bar, the results before it kept.
#[test]
fn a_cancel_shows_no_band_and_a_timeout_shows_red() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.ctrl("Enter");
    ui.until("Cancel to show", |ui| ui.tag("query-cancel").is_some());
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["running"].is_null() && ui.query()["error"]["code"] == "cancelled");
    ui.frame();
    assert!(ui.tag("query-error").is_none(), "no band for dbee's own cancel");
    let facts = ui.value("query-facts");
    assert!(facts.as_str().is_some_and(|f| f.starts_with("Cancelled")), "{facts}");
    ui.execute("query.setText", json!({ "text": "SET statement_timeout = 200; SELECT pg_sleep(5)" }));
    ui.frame();
    ui.click("query-run");
    ui.until("the timeout", |ui| ui.query()["running"].is_null() && ui.query()["error"]["code"] == "server");
    ui.frame();
    let card = ui.value("query-error");
    assert_eq!((card["code"].clone(), card["heading"].clone()), (json!("server"), json!("Statement 2 failed")), "{card}");
    assert_eq!(ui.query()["error"]["meaning"], "interrupted");
    let band = as_rect(ui.rect("query-error"));
    let t = dbee_ui_egui::theme::LIGHT;
    assert_eq!(colour_at(&ui, egui::pos2(band.right() - 4.0, band.bottom() - 4.0)), Some(t.deleted), "the band's red ground");
    assert!(visible(&ui.tag("result-1")), "the SET's result stays");
    let facts = ui.value("query-facts");
    assert!(facts.as_str().is_some_and(|f| !f.starts_with("Cancelled") && !f.starts_with("Failed")), "the SET's facts: {facts}");
}

/// Review Focus 3: ROLLBACK shows while the connection's transaction has failed, from any of its
/// query tabs; it ends it, toasts, and leaves the band without the button.
#[test]
fn rollback_ends_the_failed_transaction_from_the_band() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("BEGIN; SELECT nope");
    ui.ctrl("Enter");
    ui.ran();
    assert!(ui.enabled("query-rollback"), "ROLLBACK under the failed transaction's band");
    assert_eq!(ui.value("query-rollback"), "Rollback");
    assert_eq!(ui.document()["connection"]["transaction"]["state"], "failed");
    // A second query tab on the connection: its run fails with 25P02, and it offers ROLLBACK too.
    ui.ctrl("T");
    ui.frame();
    ui.text("SELECT 1");
    ui.ctrl("Enter");
    ui.ran();
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("25P02")), "{card}");
    let band = as_rect(ui.rect("query-error"));
    let button = as_rect(ui.rect("query-rollback"));
    assert!(band.contains_rect(button), "in the band: {button:?} {band:?}");
    ui.click("query-rollback");
    ui.until("the rollback", |ui| ui.document()["connection"]["transaction"].is_null());
    ui.until("a toast", |ui| ui.tag("notice").is_some());
    assert_eq!(ui.value("notice"), "Rolled back on pg14");
    ui.frame();
    assert!(ui.tag("query-rollback").is_none(), "ROLLBACK leaves the band");
    assert!(ui.tag("query-error").is_some(), "the band stays: the last run's error");
    ui.click("query-run");
    ui.until("the run", |ui| !ui.query()["results"].as_array().is_none_or(Vec::is_empty));
    assert!(ui.query()["error"].is_null(), "statements run again");
}

/// Review Focus 7: the error's token is underlined in `danger` while the text is the run's; the
/// first edit takes it away for good, the red line number stays until the next run.
#[test]
fn the_underline_goes_at_the_first_edit() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql = "SELECT o.totl FROM shop.orders o";
    ui.text(sql);
    ui.ctrl("Enter");
    ui.ran();
    let t = dbee_ui_egui::theme::LIGHT;
    let marks = |ui: &Ui| -> Vec<egui::Rect> {
        ui.shapes()
            .into_iter()
            .filter_map(|(_, s)| match s {
                egui::Shape::Rect(r) if r.fill == t.danger && (r.rect.height() - 1.5).abs() < 0.01 => Some(r.rect),
                _ => None,
            })
            .collect()
    };
    let ed = ui.editor();
    assert_eq!((ed["underline"].clone(), ed["errorLine"].clone()), (json!([7, 13]), json!(1)), "{ed}");
    let editor = as_rect(ui.rect("query-editor"));
    let digit = ui.harness.ctx.fonts_mut(|f| f.glyph_width(&dbee_ui_egui::theme::Text::Code.font(), '0'));
    let mark = marks(&ui);
    assert!(
        mark.len() == 1 && editor.contains_rect(mark[0]) && (mark[0].width() - 6.0 * digit).abs() <= 1.0,
        "one mark as wide as `o.totl`: {mark:?}"
    );
    ui.ctrl("End");
    ui.text(" ");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["underline"].clone(), ed["errorLine"].clone()), (Json::Null, json!(1)), "the first edit: {ed}");
    assert!(marks(&ui).is_empty());
    ui.ctrl("Z");
    ui.frame();
    assert_eq!(ui.query()["text"], sql, "undone");
    assert_eq!(ui.editor()["underline"], Json::Null, "it doesn't come back on undo");
    ui.ctrl("Enter");
    ui.ran();
    assert_eq!(ui.editor()["underline"], json!([7, 13]), "the next run's error");
    assert_eq!(marks(&ui).len(), 1);
}

/// The value panel shown or not, through `ui.toggleValuePanel`.
fn panel(ui: &mut Ui, on: bool) {
    if ui.execute("ui.toggleValuePanel", json!({}))["visible"] != json!(on) {
        ui.execute("ui.toggleValuePanel", json!({}));
    }
    ui.frame();
}

/// The window widths the narrow sweeps go through, from wide to narrow.
const SWEEP: [f32; 10] = [1400.0, 1100.0, 900.0, 760.0, 640.0, 520.0, 420.0, 300.0, 200.0, 120.0];

/// Two tagged controls that are one widget: the facts are also `query-rows` or `query-affected`.
fn one_widget(a: &str, b: &str) -> bool {
    let facts = |x: &str| ["query-facts", "query-rows", "query-affected"].contains(&x);
    facts(a) && facts(b)
}

/// Every tagged control of the query tab's run row, result bar and band lies inside the tab's
/// body, and no two overlap; RUN CURRENT, while drawn, is drawn whole, never cut at the body's
/// edge.
fn assert_nothing_over_anything(ui: &mut Ui, at: &str) {
    let body = ui.tab_x();
    let height = ui.harness.ctx.content_rect().height();
    let areas = ["query-editor", "query-split", "query-error"];
    let tags = ui.harness.state().tags();
    let drawn: Vec<(String, egui::Rect)> = tags
        .ids()
        .into_iter()
        .filter(|id| ["query-", "result-", "value-toggle"].iter().any(|p| id.starts_with(p)) && !areas.contains(id))
        .filter_map(|id| tags.rect(id).map(|r| (id.to_string(), r)))
        .collect();
    for (id, r) in &drawn {
        assert!(
            r.left() >= body.min - 0.5 && r.right() <= body.max + 0.5 && r.top() >= 0.0 && r.bottom() <= height + 0.5,
            "{at}: `{id}` {r:?} inside the body {body:?}"
        );
    }
    if let Some((_, r)) = drawn.iter().find(|(id, _)| id == "query-run-current") {
        assert!(r.right() <= body.max - 11.5, "{at}: RUN CURRENT {r:?} whole, 12 before the body's end {body:?}");
    }
    for (i, (a, ra)) in drawn.iter().enumerate() {
        for (b, rb) in &drawn[i + 1..] {
            let o = ra.intersect(*rb);
            assert!(one_widget(a, b) || !o.is_positive() || o.area() <= 0.5, "{at}: `{a}` {ra:?} over `{b}` {rb:?}");
        }
    }
}

/// [`assert_nothing_over_anything`] at each width of [`SWEEP`], with the value panel shut and then
/// open; the window back at 1100 × 720 after.
fn sweep_widths(ui: &mut Ui, state: &str) {
    for on in [false, true] {
        ui.harness.set_size(egui::vec2(1400.0, 720.0));
        ui.frame();
        panel(ui, on);
        for width in SWEEP {
            ui.harness.set_size(egui::vec2(width, 720.0));
            ui.frame();
            ui.frame();
            assert_nothing_over_anything(ui, &format!("{state}, {width} wide, the panel {}", if on { "open" } else { "shut" }));
        }
    }
    ui.harness.set_size(APP_SIZE);
    ui.frame();
}

/// Review Focus 6: at every width, the panel open and shut, with several results, while a run
/// goes (after 300 ms), and after a failure (with a position on Postgres), on Postgres and on
/// MySQL with its database chip, no control of the query tab's bars is drawn over another or past
/// the body.
#[test]
fn no_query_bar_draws_one_control_over_another() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let engines = [("pg14", pg14(db), "select pg_sleep(30)"), ("mysql80", config_of(&mysql), "select sleep(30)")];
    for (name, config, sleep) in engines {
        let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), config: Some(config), ..Setup::default() });
        ui.ctrl("T");
        ui.frame();
        if name == "mysql80" {
            assert!(ui.tag("query-database").is_some(), "{name}: the database chip");
        }
        ui.execute("query.setText", json!({ "text": "select 1 as a;\nselect * from shop.orders limit 5;\nselect 2 as b" }));
        ui.ctrl("Enter");
        ui.ran();
        assert!(ui.tag("result-3").is_some(), "{name}: three results");
        sweep_widths(&mut ui, &format!("{name}, several results"));
        ui.execute("query.setText", json!({ "text": "select o.totl from shop.orders o" }));
        ui.ctrl("Enter");
        ui.ran();
        assert!(ui.tag("query-error").is_some(), "{name}: the band");
        sweep_widths(&mut ui, &format!("{name}, failed"));
        ui.execute("query.setText", json!({ "text": sleep }));
        ui.frame();
        held_run(&mut ui, 0.4, |_, _| {});
        assert!(ui.tag("query-cancel").is_some(), "{name}: running");
        sweep_widths(&mut ui, &format!("{name}, running"));
        ui.click("query-cancel");
        ui.until("the cancel", |ui| ui.query()["running"].is_null());
    }
}

/// At 640 × 420 the panel gives the results the body: it isn't drawn, its toggle is off and says
/// why, `ui.toggleValuePanel` still flips it, and at 1100 it is back at its width.
#[test]
fn at_640_the_panel_gives_way() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, egui::vec2(640.0, 420.0));
    panel(&mut ui, true);
    ui.ctrl("T");
    ui.frame();
    ui.text("select * from shop.orders limit 5");
    ui.ctrl("Enter");
    ui.ran();
    assert!(ui.tag("value-panel").is_none(), "no panel at 640");
    assert_eq!(ui.value("value-toggle"), true, "the panel still asked for");
    assert!(!ui.enabled("value-toggle"), "the toggle off");
    let body = ui.tab_x();
    assert_eq!(ui.body_x(), body, "the results take the body");
    let toggle = ui.drawn("value-toggle");
    assert!((toggle.right() - body.max).abs() <= 0.5, "the toggle at the body's end: {toggle:?} {body:?}");
    ui.rest(toggle.center());
    let why = "Value panel: not now, the window is too narrow";
    assert!(ui.texts().iter().any(|(_, g)| g.text() == why), "the toggle says why");
    ui.hover_away();
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": false }));
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": true }));
    ui.harness.set_size(APP_SIZE);
    ui.frame();
    ui.frame();
    let panel = ui.drawn("value-panel");
    assert!((panel.width() - 340.0).abs() <= 1.0, "back at 1100, 340 wide: {panel:?}");
    assert!(ui.enabled("value-toggle"), "the toggle on again");
}

/// At 900 the panel, dragged wide, leaves the results at least 320.
#[test]
fn at_900_the_results_keep_320() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, egui::vec2(900.0, 720.0));
    panel(&mut ui, true);
    ui.ctrl("T");
    ui.frame();
    ui.text("select * from shop.orders limit 5");
    ui.ctrl("Enter");
    ui.ran();
    let before = ui.drawn("value-panel");
    let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let (from, to) = (egui::pos2(before.left() + 2.0, before.center().y), egui::pos2(before.left() - 300.0, before.center().y));
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![press(from, true)]);
    for i in 1..=8 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))]);
    }
    ui.frame_with(vec![press(to, false)]);
    ui.frame();
    let results = ui.body_x();
    assert!(results.span() >= 320.0 - 0.5, "the results keep 320: {results:?}");
    let toggle = ui.drawn("value-toggle");
    assert!((toggle.right() - results.max).abs() <= 0.5, "the toggle at the results' end: {toggle:?} {results:?}");
    assert!(ui.drawn("value-panel").width() >= 240.0 - 0.5, "the panel still drawn");
}

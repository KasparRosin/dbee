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
    assert!(ui.tag("result-1").is_none(), "one result: no selector");
    assert!(ui.tag("query-error").is_none());
    assert!(ui.tag("query-hint").is_none(), "the hint goes once there are results");
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
    let [text_left, ..] = ui.rect("query-editor");
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
    assert!(!ui.enabled("query-cancel"), "nothing runs: Cancel is disabled");
    assert!(ui.enabled("query-run"));
    ui.click("query-run");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    ui.frame();
    assert!(ui.enabled("query-cancel"), "Cancel is enabled while the run goes");
    assert!(ui.enabled("query-run"), "Run follows the engine's enabled check; a run going is its Busy answer");
    let start = Instant::now();
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
    let took = start.elapsed();
    println!("cancelled in {took:?}");
    ui.frame();
    let card = ui.value("query-error");
    assert_eq!(card["code"], "cancelled", "{card}");
    assert!(!ui.enabled("query-cancel"));
    assert!(ui.tag("notice").is_none(), "the card says it: {:?}", ui.tag("notice"));
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
    assert!(ui.tag("result-1").is_none());
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
    assert!(more[0] + more[2] <= rows[0], "the `…` stays clear of the row count: {more:?} {rows:?}");
    ui.click("result-more");
    ui.frame();
    ui.click("result-list-1");
    ui.frame();
    assert_eq!(ui.query()["shown"], 0);
    assert!(ui.tag("result-list-1").is_none(), "the list closed");
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
    assert_eq!(ui.value("value-toggle"), true, "the value panel shows");
    ui.execute("query.setText", json!({ "text": "select 1 as a;\nselect 2 as b;\nselect 3 as c" }));
    ui.ctrl("Enter");
    ui.ran();
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

/// The selector's buttons lie whole between Cancel and the row count, and its label, when drawn,
/// whole before them.
fn selector_whole(ui: &mut Ui, when: &str) {
    let i = ui.inspect();
    let rect = |id: &str| -> [f32; 4] {
        let r = tag_in(&i, id).unwrap_or_else(|| panic!("{when}: no `{id}`"))["rect"].clone();
        [0, 1, 2, 3].map(|k| r[k].as_f64().unwrap() as f32)
    };
    let (cancel, rows) = (rect("query-cancel"), rect("query-rows"));
    let parts: Vec<String> = ids(&i).into_iter().filter(|id| id.starts_with("result-") && !id.starts_with("result-list-")).collect();
    assert!(!parts.is_empty(), "{when}: a selector");
    let mut first = f32::MAX;
    for id in &parts {
        let [x, _, w, _] = rect(id);
        assert!(visible(&tag_in(&i, id)) && w >= 22.0, "{when}: `{id}` is whole: {:?}", tag_in(&i, id));
        assert!(x >= cancel[0] + cancel[2] && x + w <= rows[0], "{when}: `{id}` lies between Cancel and the row count");
        first = first.min(x);
    }
    let bar = egui::Rect::from_min_size(egui::pos2(cancel[0], cancel[1]), egui::vec2(rows[0] + rows[2] - cancel[0], cancel[3]));
    for (r, g) in ui.texts() {
        if g.text() == "Results" && bar.contains(r.center()) {
            assert!(r.max.x <= first, "{when}: the label is whole, before the buttons: {r:?} {first}");
        }
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
    ui.click("result-2");
    ui.frame();
    assert_eq!(ui.value("query-affected"), "2 rows affected");
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 1], "focus": [0, 1] }));
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
    assert!(ui.tag("query-hint").is_some(), "an empty results area says how to run");
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
    assert!(card["message"].as_str().is_some_and(|m| m.contains("SQLSTATE 22012")) && card["note"].is_null(), "{card}");
    // The transaction stays open and failed: the next run says how to end it.
    run(&mut ui, "SELECT 1");
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("SQLSTATE 25P02")), "{card}");
    assert_eq!(card["note"], dbee_ui_egui::query_tab::ROLLBACK_NOTE);
    run(&mut ui, "ROLLBACK");
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
    let idle: [Moved<'_>; 3] = [
        ("query-run", Some(json!("Run")), true, "RUN"),
        ("query-run-current", Some(json!("Run current")), true, "RUN CURRENT"),
        ("query-cancel", Some(json!("Cancel")), false, "CANCEL"),
    ];
    assert_moved(&mut ui, &idle, "idle");
    let cancel = ui.centre("query-cancel");
    ui.rest(cancel);
    let why = "Stop the running statement: not now, nothing is running";
    assert!(ui.texts().iter().any(|(_, g)| g.text() == why), "Cancel, off, says why on hover");
    ui.click("query-run");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    ui.frame();
    let running: [Moved<'_>; 3] = [
        ("query-run", Some(json!("Run")), true, "RUN"),
        ("query-run-current", Some(json!("Run current")), true, "RUN CURRENT"),
        ("query-cancel", Some(json!("Cancel")), true, "CANCEL"),
    ];
    assert_moved(&mut ui, &running, "running");
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
}

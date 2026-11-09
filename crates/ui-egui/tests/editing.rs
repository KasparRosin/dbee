//! Cell editing driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the inline editors (double-click, or Enter on the selected cell), their keys
//! (Enter, Ctrl+Enter, Tab, Escape), the boolean choice, "+ Row", Apply with Ctrl+S, undo and redo,
//! and the card of a failed Apply. Everything goes through control requests and automation ids;
//! assertions read `ui.inspect`, `document.inspect` and the database itself (psql).
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). Tests that write make their own table in
//! `scratch` and drop it; the seed is only read.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_testkit::{TestDb, unique};
use serde_json::{Value as Json, json};

/// A 1400 × 900 window connected to the pg14 test server, on the workspace, its frames 0.25 s
/// apart on egui's clock.
fn connected(db: &TestDb) -> Ui {
    connected_at(db, 0.25, 1400.0)
}

/// [`connected`] with frames `step` seconds apart, in a window `width` wide.
fn connected_at(db: &TestDb, step: f32, width: f32) -> Ui {
    connected_sized(db, step, egui::vec2(width, 900.0))
}

/// [`connected`] with frames `step` seconds apart, in a window of `size`.
fn connected_sized(db: &TestDb, step: f32, size: egui::Vec2) -> Ui {
    Ui::connected_with(db, Setup { size, step: Some(step), ..Setup::default() })
}

/// [`connected`] to `server`.
fn connected_to(db: &TestDb, server: &dbee_testkit::Server) -> Ui {
    Ui::connected_with(db, Setup { step: Some(0.25), config: Some(server_config(server)), ..Setup::default() })
}

impl Ui {
    fn edits(&mut self) -> Json {
        self.table()["edits"].clone()
    }

    /// The staged value of loaded cell `(row, column)`, when one is staged.
    fn staged(&mut self, row: usize, column: usize) -> Option<Json> {
        let edits = self.edits();
        edits["cells"].as_array().unwrap().iter().find(|c| c["row"] == row && c["column"] == column).map(|c| c["value"].clone())
    }

    fn selection(&mut self) -> Json {
        self.document()["selection"].clone()
    }

    /// A paste from the clipboard, as the keyboard's Ctrl+V reaches egui (the control channel
    /// has no paste), then a frame more for the frame that took it to be listed.
    fn paste(&mut self, text: &str) {
        self.harness.input_mut().events.push(egui::Event::Paste(text.to_string()));
        self.frame();
        self.frame();
    }

    /// Scrolls the grid by `delta` points with the wheel over the centre of cell `over`.
    fn wheel(&mut self, over: &str, delta: egui::Vec2) {
        let tag = self.tag(over).unwrap_or_else(|| panic!("no widget `{over}`"));
        let r = &tag["rect"];
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        let at = egui::pos2(f(0) + f(2) / 2.0, f(1) + f(3) / 2.0);
        self.harness.input_mut().events.push(egui::Event::PointerMoved(at));
        self.frame();
        self.harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // egui eases the scroll over a few frames.
        for _ in 0..12 {
            self.frame();
        }
    }

    /// The editor's value, when one is open.
    fn editor(&mut self) -> Option<Json> {
        self.tag("cell-editor").map(|t| t["value"].clone())
    }

    /// The text of the open text editor.
    fn editor_text(&mut self) -> String {
        let v = self.editor().expect("an editor is open");
        v["text"].as_str().unwrap_or_else(|| panic!("not a text editor: {v}")).to_string()
    }

    fn no_notice(&mut self) {
        let n = self.tag("notice");
        assert!(n.is_none(), "no notice was expected: {n:?}");
    }

    fn notice(&mut self) -> String {
        self.value("notice").as_str().unwrap_or_default().to_string()
    }
}

/// Drops `scratch.<name>` on `server` when the test ends, also when it fails.
struct ScratchOn<'a>(&'a dbee_testkit::Server, String);

impl Drop for ScratchOn<'_> {
    fn drop(&mut self) {
        let _ = dbee_testkit::sql(self.0, &format!("DROP TABLE IF EXISTS scratch.{} CASCADE", self.1));
    }
}

/// `scratch.<u> (id int PK, name text, total numeric(12,2), shipped_at date, flag bool, doc
/// jsonb)` with ids 1 to 3, as the engine's edit tests have it. Columns 0 to 5 in that order.
fn orders(db: &TestDb) -> Scratch {
    let name = unique("t_edit_ui");
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, name text, total numeric(12,2), shipped_at date, flag bool, doc jsonb);
             INSERT INTO scratch.{name} VALUES
                 (1, 'Alice', 10.50, '2025-01-02', true, '{{\"a\": 1}}'),
                 (2, 'Bob', 20.00, '2025-02-03', false, '[1, 2]'),
                 (3, 'Cy', 5.25, NULL, NULL, NULL);"
        ),
    )
    .unwrap();
    Scratch(name)
}

fn q(db: &TestDb, sql: &str) -> String {
    dbee_testkit::psql(pg14_server(db), sql).unwrap().trim().to_owned()
}

/// A window with the scratch table `t` open (the orders table: 3 rows).
fn opened(db: &TestDb, t: &Scratch) -> Ui {
    opened_rows(db, t, 3)
}

/// A window with the scratch table `t` open, once its `rows` rows show.
fn opened_rows(db: &TestDb, t: &Scratch, rows: u64) -> Ui {
    let mut ui = connected(db);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == rows);
    ui
}

#[test]
fn enter_commits_in_number_cell_and_newlines_in_text() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    // A number: the editor holds the value, typing replaces it, Enter commits.
    ui.double_click("grid-cell-0-2");
    assert_eq!(ui.editor_text(), "10.50");
    assert_eq!(ui.editor().unwrap()["multiline"], false);
    ui.text("5");
    ui.key("Enter");
    assert!(ui.editor().is_none(), "Enter closed the editor");
    assert_eq!(ui.staged(0, 2), Some(json!("5")));
    // Text: Enter is a newline, Ctrl+Enter commits.
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor().unwrap()["multiline"], true);
    ui.text("a");
    ui.key("Enter");
    ui.text("b");
    assert_eq!(ui.editor_text(), "a\nb", "Enter added a line");
    // Ctrl+Shift+Enter (run the statement) is not run from the editor either.
    ui.ctrl_shift("Enter");
    assert_eq!(ui.editor_text(), "a\nb");
    ui.ctrl("Enter");
    assert!(ui.editor().is_none(), "Ctrl+Enter closed the editor");
    assert_eq!(ui.staged(0, 1), Some(json!("a\nb")));
    // The editor took Ctrl+Enter: the query shortcut did not also run (it would say why not).
    ui.no_notice();
    ui.frame();
    let cell = ui.value("grid-cell-0-1");
    assert_eq!(cell["state"], "staged", "{cell}");
}

#[test]
fn clearing_a_date_stages_null() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-3");
    assert_eq!(ui.editor_text(), "2025-01-02");
    ui.ctrl("A");
    ui.key("Delete");
    assert_eq!(ui.editor_text(), "");
    ui.key("Enter");
    assert_eq!(ui.staged(0, 3), Some(Json::Null), "a date has no empty value: clearing stages NULL");
    ui.frame();
    let cell = ui.value("grid-cell-0-3");
    assert_eq!((cell["text"].clone(), cell["null"].clone(), cell["state"].clone()), (json!("NULL"), json!(true), json!("staged")), "{cell}");
    // Clearing text stages an empty string.
    ui.double_click("grid-cell-1-1");
    ui.ctrl("A");
    ui.key("Delete");
    ui.ctrl("Enter");
    assert_eq!(ui.staged(1, 1), Some(json!("")));
}

#[test]
fn tab_commits_and_moves_right_escape_cancels() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    // Enter on the selected cell opens its editor.
    ui.click("grid-cell-1-1");
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "Bob");
    ui.text("Bobby");
    ui.key("Tab");
    assert!(ui.editor().is_none(), "Tab closed the editor");
    assert_eq!(ui.staged(1, 1), Some(json!("Bobby")));
    assert_eq!(ui.selection(), json!({ "anchor": [1, 2], "focus": [1, 2] }), "and moved right");
    // Escape cancels.
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "20.00");
    ui.text("99");
    ui.key("Escape");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(1, 2), None, "nothing staged");
    assert_eq!(ui.edits()["count"], 1);
    // From the last column to the start of the next row.
    ui.click("grid-cell-1-5");
    ui.key("Enter");
    ui.key("Tab");
    assert_eq!(ui.selection(), json!({ "anchor": [2, 0], "focus": [2, 0] }));
    // On the last cell of the last row it commits and stays.
    ui.click("grid-cell-2-5");
    ui.key("Enter");
    ui.text("{\"b\": 2}");
    ui.key("Tab");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 5), Some(json!("{\"b\": 2}")));
    assert_eq!(ui.selection(), json!({ "anchor": [2, 5], "focus": [2, 5] }));
}

#[test]
fn ctrl_s_applies_all_or_nothing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    // Ctrl+S with the editor still open commits it first: its value is written too.
    ui.double_click("grid-cell-1-2");
    ui.text("7.25");
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(
        q(db, &format!("SELECT string_agg(name || '|' || total, ',' ORDER BY id) FROM scratch.{} WHERE id IN (1, 2)", t.0)),
        "Ann|10.50,Bob|7.25"
    );
    // A bad date fails the Apply: nothing is written, the card says why, the row is marked.
    ui.double_click("grid-cell-0-1");
    ui.text("Annie");
    ui.ctrl("Enter");
    ui.double_click("grid-cell-1-3");
    ui.text("not a date");
    ui.ctrl("S");
    ui.until("the failed Apply", |ui| ui.table()["failed"].is_object());
    ui.frame();
    let card = ui.value("table-error");
    assert!(card.as_str().is_some_and(|c| c.contains("not a date") && c.contains("nothing was written")), "{card}");
    ui.no_notice();
    let cell = ui.value("grid-cell-1-0");
    assert_eq!(cell["state"], "failed", "the failed row is marked: {cell}");
    assert_eq!(ui.value("grid-cell-0-0").get("state"), None, "and only that row");
    assert_eq!(
        q(db, &format!("SELECT string_agg(name || '|' || shipped_at, ',' ORDER BY id) FROM scratch.{} WHERE id IN (1, 2)", t.0)),
        "Ann|2025-01-02,Bob|2025-02-03"
    );
    assert_eq!(ui.edits()["count"], 2, "every edit stays");
    // Reset drops them, and the card goes with them.
    ui.click("edit-reset");
    ui.frame();
    assert!(ui.tag("table-error").is_none());
}

#[test]
fn a_value_too_long_for_its_column_fails_the_apply_and_says_why() {
    let db = dbee_testkit::require_testdb!();
    for server in db.servers_of(&[dbee_testkit::Kind::Postgres]) {
        let name = unique("t_len_ui");
        let _drop = ScratchOn(server, name.clone());
        dbee_testkit::psql(
            server,
            &format!("CREATE TABLE scratch.{name} (id int PRIMARY KEY, code varchar(5)); INSERT INTO scratch.{name} VALUES (1, 'abc');"),
        )
        .unwrap();
        let mut ui = connected_to(db, server);
        ui.open("scratch", &name);
        ui.until("the page", |ui| ui.table()["rows"] == 1);
        ui.double_click("grid-cell-0-1");
        ui.text("abcdefgh");
        ui.key("Enter");
        ui.ctrl("S");
        ui.until("the Apply's end", |ui| {
            let t = ui.table();
            t["applying"].is_null() && (t["failed"].is_object() || t["edits"]["count"] == 0)
        });
        let code = dbee_testkit::psql(server, &format!("SELECT code FROM scratch.{name}")).unwrap();
        assert_eq!(code.trim(), "abc", "{}: nothing was written, not even the first five characters", server.name);
        ui.frame();
        let card = ui.value("table-error");
        let card = card.as_str().unwrap_or_default();
        assert!(card.contains("value too long for type character varying(5)") && card.contains("nothing was written"), "{}: {card}", server.name);
        assert_eq!(ui.value("grid-cell-0-1")["state"], "failed", "{}", server.name);
    }
}

#[test]
fn the_status_bar_cancels_an_apply_that_waits_for_a_lock() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let lock = dbee_testkit::hold(pg14_server(db), &format!("SELECT id FROM scratch.{} WHERE id = 1 FOR UPDATE", t.0)).unwrap();
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("waited");
    ui.ctrl("Enter");
    ui.ctrl("S");
    let waiting = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND wait_event_type = 'Lock' AND query LIKE 'UPDATE \"scratch\".\"{}\"%'",
        t.0
    );
    ui.until("the Apply to wait for the lock", |_| q(db, &waiting) == "1");
    ui.frame();
    assert_eq!(ui.value("status-job"), format!("Applying the changes to {}…", t.0));
    ui.click("status-cancel");
    ui.until("the cancelled Apply", |ui| ui.table()["failed"].is_object());
    ui.frame();
    let card = ui.value("table-error");
    assert_eq!(card, "the Apply was cancelled at change 1, so nothing was written");
    ui.no_notice();
    assert!(ui.tag("status-cancel").is_none(), "nothing runs any more");
    assert_eq!(ui.edits()["count"], 1, "the edit stays");
    drop(lock);
    assert_eq!(q(db, &format!("SELECT name FROM scratch.{} WHERE id = 1", t.0)), "Alice");
}

#[test]
fn the_prompt_after_an_unknown_apply_says_apply_again() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let lock = dbee_testkit::hold(pg14_server(db), &format!("SELECT id FROM scratch.{} WHERE id = 1 FOR UPDATE", t.0)).unwrap();
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("waited");
    ui.ctrl("Enter");
    ui.ctrl("S");
    let waiting = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND wait_event_type = 'Lock' AND query LIKE 'UPDATE \"scratch\".\"{}\"%'",
        t.0
    );
    ui.until("the Apply to wait for the lock", |_| q(db, &waiting) == "1");
    ui.execute("connection.reconnect", json!({}));
    drop(lock);
    ui.ctrl("S");
    let i = ui.inspect();
    assert_eq!(i["dialog"], "confirm", "{i}");
    assert_eq!(tag_in(&i, "dialog-ok").unwrap()["value"], "Apply again");
    let texts = ui.texts();
    assert!(texts.iter().any(|(_, g)| g.text() == "Apply again?"));
    assert!(!texts.iter().any(|(_, g)| g.text().contains("Close anyway")));
}

#[test]
fn cells_that_cannot_be_edited_say_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "order_totals");
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_none(), "no editor on a view");
    let notice = ui.notice();
    assert_eq!(notice, "Views are read-only.");
    let add = ui.tag("row-add").unwrap();
    assert_eq!(add["enabled"], false, "no + Row on a view: {add}");
    ui.until("the notice to go", |ui| ui.tag("notice").is_none());
    ui.click("grid-cell-1-1");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    let notice = ui.notice();
    assert_eq!(notice, "Views are read-only.");
}

#[test]
fn a_generated_column_opens_no_editor_and_says_why() {
    let db = dbee_testkit::require_testdb!();
    let t = Scratch::new(db, "t_generated_ui", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY, a int, twice int GENERATED ALWAYS AS (a * 2) STORED); INSERT INTO scratch.{t} VALUES (1, 2);"
        )
    });
    let mut ui = opened_rows(db, &t, 1);
    ui.double_click("grid-cell-0-2");
    assert!(ui.editor().is_none(), "no editor on a generated column");
    assert_eq!(ui.notice(), "Generated columns are read-only.");
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor_text(), "2", "the column it is computed from edits");
}

#[test]
fn a_value_too_large_to_edit_inline_says_so() {
    let db = dbee_testkit::require_testdb!();
    // Made before the session connects, so its catalog has it.
    let cap = dbee_ui_egui::editors::MAX_INLINE_BYTES;
    let name = unique("t_edit_cap");
    dbee_testkit::psql(
        pg14_server(db),
        &format!("CREATE TABLE scratch.{name} (id int PRIMARY KEY, v text); INSERT INTO scratch.{name} VALUES (1, repeat('x', {cap})), (2, repeat('x', {cap} + 1));"),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = connected(db);
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.table()["rows"] == 1);
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_none(), "20 MB does not open inline");
    let notice = ui.notice();
    assert!(notice.contains("too large to edit inline"), "{notice}");
    // A small value of the same row still opens.
    ui.double_click("grid-cell-0-0");
    assert_eq!(ui.editor_text(), "1");
    ui.key("Escape");
    // The cap itself opens; a byte more does not.
    ui.until("the notice to go", |ui| ui.tag("notice").is_none());
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 2);
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_some(), "{cap} bytes open");
    ui.key("Escape");
    ui.double_click("grid-cell-1-1");
    assert!(ui.editor().is_none(), "{} bytes do not", cap + 1);
    assert!(ui.notice().contains("too large to edit inline"));
}

#[test]
fn null_opens_empty_and_enter_alone_sends_nothing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-2-3");
    assert_eq!(ui.editor_text(), "", "NULL opens empty");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    assert_eq!(ui.edits()["count"], 0, "unchanged: nothing staged");
    // The same with an unchanged value.
    ui.double_click("grid-cell-0-0");
    ui.key("Enter");
    assert_eq!(ui.edits()["count"], 0);
    ui.no_notice();
}

#[test]
fn an_editor_whose_cell_never_comes_into_view_closes() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // A window so low that the grid has no room for a row.
    let mut ui = connected_sized(db, 0.25, egui::vec2(1400.0, 150.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.execute("selection.set", json!({ "anchor": [2, 1], "focus": [2, 1] }));
    ui.key("Enter");
    assert_eq!(ui.inspect()["editor"], json!({ "row": 2, "column": 1 }), "Enter opened an editor");
    assert!(ui.editor().is_none(), "which is not drawn");
    // A second of egui's clock (frames 0.25 s apart), and it is gone, unchanged.
    for _ in 0..6 {
        ui.frame();
    }
    assert_eq!(ui.inspect()["editor"], Json::Null);
    assert_eq!(ui.edits()["count"], 0);
    ui.no_notice();
}

#[test]
fn clicking_another_cell_commits() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-2-1");
    assert_eq!(ui.editor_text(), "Cy");
    ui.text("Cyd");
    ui.click("grid-cell-0-0");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 1), Some(json!("Cyd")));
    assert_eq!(ui.selection(), json!({ "anchor": [0, 0], "focus": [0, 0] }), "the click selected its cell");
}

#[test]
fn quick_double_clicks_on_two_cells_both_open() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // Frames at 60 Hz: the second double-click comes well within egui's double-click time of
    // the first, which egui then counts as a triple click.
    let mut ui = connected_at(db, 1.0 / 60.0, 1400.0);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor_text(), "Alice");
    ui.key("Escape");
    ui.double_click("grid-cell-1-1");
    assert_eq!(ui.editor_text(), "Bob", "the second cell's editor opened");
}

#[test]
fn an_editor_opens_over_its_own_cell_after_a_wider_one() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // As narrow as the app's first window: a several-lines editor on the last column is moved
    // left to stay in view.
    let mut ui = connected_at(db, 0.25, 1100.0);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-5");
    let wide = ui.tag("cell-editor").unwrap();
    let doc = ui.tag("grid-cell-0-5").unwrap();
    assert!(wide["rect"][0].as_f64().unwrap() < doc["rect"][0].as_f64().unwrap(), "moved left: {wide} {doc}");
    ui.key("Escape");
    // The boolean choice that follows is drawn over its own cell from its first frame.
    ui.double_click("grid-cell-1-4");
    let choice = ui.tag("cell-editor").unwrap();
    let flag = ui.tag("grid-cell-1-4").unwrap();
    // egui puts an area on whole points.
    let near = |i: usize| (choice["rect"][i].as_f64().unwrap() - flag["rect"][i].as_f64().unwrap()).abs() <= 1.0;
    assert!(near(0) && near(1), "{choice} {flag}");
    ui.key("Escape");
}

#[test]
fn an_editor_scrolled_out_of_view_sideways_commits() {
    let db = dbee_testkit::require_testdb!();
    // Ten wide columns: far wider than the window.
    let name = unique("t_edit_wide");
    let columns: Vec<String> = (1..=10).map(|c| format!("c{c} text")).collect();
    let values: Vec<String> = (1..=10).map(|c| format!("repeat('{c}', 60)")).collect();
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, {}); INSERT INTO scratch.{name} SELECT g, {} FROM generate_series(1, 5) AS g;",
            columns.join(", "),
            values.join(", ")
        ),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = opened_rows(db, &t, 5);
    ui.double_click("grid-cell-0-1");
    ui.text("abc");
    assert_eq!(ui.editor_text(), "abc");
    // Sideways, until c1 is out of view to the left (the wheel over c2, in view beside the value
    // panel).
    ui.wheel("grid-cell-3-2", egui::vec2(-2000.0, 0.0));
    let cell = ui.tag("grid-cell-0-1").unwrap();
    assert_eq!(cell["visible"], false, "c1 is out of view: {cell}");
    assert!(ui.editor().is_none(), "the editor left with its cell");
    assert_eq!(ui.staged(0, 1), Some(json!("abc")), "and committed into c1");
    assert_eq!(ui.edits()["count"], 1, "and nowhere else");
}

#[test]
fn a_paste_that_would_pass_the_cap_is_dropped_and_says_so() {
    let db = dbee_testkit::require_testdb!();
    let cap = dbee_ui_egui::editors::MAX_INLINE_BYTES;
    let name = unique("t_edit_paste");
    dbee_testkit::psql(
        pg14_server(db),
        &format!("CREATE TABLE scratch.{name} (id int PRIMARY KEY, v text); INSERT INTO scratch.{name} VALUES (1, repeat('p', 20000));"),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = opened_rows(db, &t, 1);
    ui.double_click("grid-cell-0-1");
    // At the end of the 20,000 bytes: 16,000 more would pass the cap, 8,000 would not.
    ui.ctrl("End");
    assert!((20_000 + 8_000..20_000 + 16_000).contains(&cap), "the cap lies between the two");
    ui.paste(&"a".repeat(16_000));
    let notice = ui.notice();
    assert!(notice.contains("too large to edit inline"), "{notice}");
    ui.paste(&"b".repeat(8_000));
    ui.ctrl("Enter");
    let staged = ui.staged(0, 1).and_then(|v| v.as_str().map(str::len));
    assert_eq!(staged, Some(28_000), "the first paste was dropped, the second went in");
    // What a paste replaces counts too: over all 28,000 bytes, 30,000 fit.
    ui.double_click("grid-cell-0-1");
    ui.paste(&"c".repeat(30_000));
    ui.ctrl("Enter");
    let staged = ui.staged(0, 1).and_then(|v| v.as_str().map(str::len));
    assert_eq!(staged, Some(30_000));
}

#[test]
fn edits_changing_under_an_open_editor_discard_it() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    for _ in 0..3 {
        ui.execute("row.add", json!({}));
    }
    ui.double_click("grid-cell-4-1");
    ui.text("Zed");
    // An agent deletes the new row before it: the editor's row would now be another one.
    ui.execute("row.delete", json!({ "rows": [3] }));
    ui.frame();
    assert!(ui.editor().is_none(), "the editor closed");
    let notice = ui.notice();
    assert!(notice.contains("discarded"), "{notice}");
    let edits = ui.edits();
    assert_eq!(edits["newRows"], json!([{ "row": 3, "values": [] }, { "row": 4, "values": [] }]), "nothing was written anywhere: {edits}");
}

#[test]
fn the_boolean_choice_lets_undo_and_redo_through_after_committing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    assert_eq!(ui.edits()["count"], 1);
    // With the choice open there is no text to undo in: Ctrl+Z commits it and undoes.
    ui.double_click("grid-cell-1-4");
    assert!(ui.editor().is_some());
    ui.ctrl("Z");
    assert!(ui.editor().is_none(), "the choice was committed");
    assert_eq!(ui.edits()["count"], 0, "and the staged name undone");
    ui.double_click("grid-cell-1-4");
    ui.ctrl_shift("Z");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 1), Some(json!("Ann")), "Ctrl+Shift+Z redid it");
}

#[test]
fn ctrl_w_commits_the_editor_before_closing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("W");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 1), Some(json!("Ann")), "the typed value was staged first");
    let dialog = ui.ok("ui.inspect", json!({}))["dialog"].clone();
    assert!(dialog.is_string(), "closing a tab with staged changes asks: {dialog}");
    ui.click("dialog-cancel");
}

#[test]
fn a_paging_button_commits_the_editor_first() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.click("page-count");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 1), Some(json!("Ann")));
    ui.until("the count", |ui| ui.table()["total"] == json!({ "exact": 3 }));
}

#[test]
fn a_row_marked_for_deletion_opens_no_editor() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.ok("ui.click", json!({ "id": "grid-cell-1-0", "button": "right" }));
    ui.click("grid-menu-delete");
    assert_eq!(ui.edits()["deleted"], json!([1]));
    ui.double_click("grid-cell-1-1");
    assert!(ui.editor().is_none());
    let notice = ui.notice();
    assert!(notice.contains("marked for deletion"), "{notice}");
}

#[test]
fn shift_tab_commits_and_moves_left() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-1-0");
    ui.text("20");
    ui.ok("ui.key", json!({ "key": "Tab", "shift": true }));
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(1, 0), Some(json!("20")));
    assert_eq!(ui.selection(), json!({ "anchor": [0, 5], "focus": [0, 5] }), "to the end of the row before");
}

#[test]
fn booleans_choose_true_false_or_null() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "true" })), "the choice opens on the value");
    for id in ["cell-editor-true", "cell-editor-false", "cell-editor-null"] {
        assert!(ui.tag(id).is_some(), "{id}");
    }
    ui.click("cell-editor-null");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 4), Some(Json::Null), "NULL sets NULL");
    // The same choice again changes nothing.
    ui.double_click("grid-cell-1-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "false" })));
    ui.click("cell-editor-false");
    assert_eq!(ui.staged(1, 4), None);
    // By keyboard: a NULL cell, up to false, Enter.
    ui.double_click("grid-cell-2-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "null" })));
    ui.key("ArrowUp");
    assert_eq!(ui.editor(), Some(json!({ "choice": "false" })));
    ui.key("Enter");
    assert_eq!(ui.staged(2, 4), Some(json!("f")), "staged as Postgres prints a bool");
    ui.double_click("grid-cell-1-4");
    ui.click("cell-editor-true");
    assert_eq!(ui.staged(1, 4), Some(json!("t")));
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT string_agg(coalesce(flag::text, 'null'), ',' ORDER BY id) FROM scratch.{}", t.0)), "null,true,false");
}

/// A MySQL `BOOLEAN` is a `TINYINT(1)`, printed `1` and `0`: the boolean choice opens on it, and
/// false chosen there writes `0`.
#[test]
fn a_mysql_boolean_set_to_false_in_the_editor_writes_0() {
    let db = dbee_testkit::require_testdb!();
    let server = db.server("mysql80").unwrap();
    let name = unique("t_bool_ui");
    let _drop = ScratchOn(server, name.clone());
    dbee_testkit::mysql(
        server,
        &format!("CREATE TABLE scratch.{name} (id INT PRIMARY KEY, flag BOOLEAN); INSERT INTO scratch.{name} VALUES (1, TRUE);"),
    )
    .unwrap();
    let t = dbee_testkit::target(server);
    let mut ui = Ui::connected_with(db, Setup { step: Some(0.25), config: Some(config_of(&t)), ..Setup::default() });
    ui.open("scratch", &name);
    ui.until("the page", |ui| ui.table()["rows"] == 1);
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor(), Some(json!({ "choice": "true" })), "the choice opens on the 1 MySQL prints");
    ui.click("cell-editor-false");
    assert_eq!(ui.staged(0, 1), Some(json!("0")), "staged as MySQL prints a false BOOLEAN");
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(dbee_testkit::mysql(server, &format!("SELECT flag FROM scratch.{name}")).unwrap().trim(), "0");
}

/// A MySQL `BOOLEAN` holding 5 is no choice: the cell opens as one line on the 5, and Enter
/// alone writes nothing over it.
#[test]
fn a_mysql_boolean_holding_another_number_opens_as_one_line() {
    let db = dbee_testkit::require_testdb!();
    let server = db.server("mysql80").unwrap();
    let name = unique("t_bool_ui");
    let _drop = ScratchOn(server, name.clone());
    dbee_testkit::mysql(
        server,
        &format!("CREATE TABLE scratch.{name} (id INT PRIMARY KEY, flag BOOLEAN); INSERT INTO scratch.{name} VALUES (1, 5);"),
    )
    .unwrap();
    let t = dbee_testkit::target(server);
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.25), config: Some(config_of(&t)), ..Setup::default() });
    ui.open("scratch", &name);
    ui.until("the page", |ui| ui.table()["rows"] == 1);
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor(), Some(json!({ "text": "5", "multiline": false })), "one line, on the 5");
    assert!(ui.tag("cell-editor-true").is_none(), "no choice");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    assert_eq!((ui.staged(0, 1), ui.edits()["count"].clone()), (None, json!(0)), "Enter alone stages nothing");
    assert_eq!(dbee_testkit::mysql(server, &format!("SELECT flag FROM scratch.{name}")).unwrap().trim(), "5");
}

#[test]
fn a_new_row_is_added_selected_and_edited() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    assert_eq!(ui.value("row-add"), "+ Row");
    ui.click("row-add");
    let edits = ui.edits();
    assert_eq!(edits["newRows"], json!([{ "row": 3, "values": [] }]), "{edits}");
    assert_eq!(ui.selection(), json!({ "anchor": [3, 0], "focus": [3, 0] }), "the new row's first cell is selected");
    ui.frame();
    assert_eq!(ui.value("grid-cell-3-0")["state"], "new");
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "", "a new cell opens empty");
    ui.text("11");
    ui.key("Tab");
    ui.key("Enter");
    ui.text("Dee");
    ui.ctrl("Enter");
    let edits = ui.edits();
    assert_eq!(edits["newRows"][0]["values"], json!([{ "column": 0, "value": "11" }, { "column": 1, "value": "Dee" }]), "{edits}");
    ui.click("edit-apply");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT name FROM scratch.{} WHERE id = 11", t.0)), "Dee");
}

#[test]
fn ctrl_z_and_ctrl_shift_z_undo_and_redo_outside_the_editor() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-2");
    ui.text("1");
    ui.key("Enter");
    assert_eq!(ui.edits()["count"], 1);
    ui.ctrl("Z");
    assert_eq!(ui.edits()["count"], 0, "Ctrl+Z undid the staged change");
    ui.ctrl_shift("Z");
    assert_eq!(ui.staged(0, 2), Some(json!("1")), "Ctrl+Shift+Z redid it");
    // In the editor they are the editor's own.
    ui.double_click("grid-cell-1-2");
    ui.text("3");
    assert_eq!(ui.editor_text(), "3");
    ui.ctrl("Z");
    assert!(ui.editor().is_some(), "the editor is still open");
    assert_eq!(ui.editor_text(), "20.00", "Ctrl+Z undid the typing in the editor");
    assert_eq!(ui.edits()["count"], 1, "the staged change stays");
    ui.key("Escape");
}

/// The paging bar's widgets, in their order along it.
const BAR: [&str; 12] = [
    "page-first",
    "page-prev",
    "page-number",
    "page-next",
    "page-last",
    "page-count",
    "table-refresh",
    "page-cut",
    "row-add",
    "edits-count",
    "edit-reset",
    "edit-apply",
];

/// A widget of the paging bar as the last frame drew it: the part a click can reach, or its
/// whole rect when none can.
#[derive(Debug, Clone)]
struct Drawn {
    id: &'static str,
    rect: egui::Rect,
    visible: bool,
}

impl Ui {
    /// The paging bar's widgets drawn in the last frame, in [`BAR`] order.
    fn bar(&mut self) -> Vec<Drawn> {
        let i = self.ok("ui.inspect", json!({}));
        let all = i["automation"].as_array().unwrap();
        BAR.iter()
            .filter_map(|&id| {
                let a = all.iter().find(|a| a["id"] == id)?;
                let r: Vec<f32> = a["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
                let rect = egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]));
                Some(Drawn { id, rect, visible: a.get("visible") != Some(&json!(false)) })
            })
            .collect()
    }

    /// Where the value panel starts: the paging bar ends left of it.
    fn panel_left(&mut self) -> f32 {
        let panel = self.tag("value-panel").expect("the value panel shows");
        panel["rect"][0].as_f64().unwrap() as f32
    }
}

/// No two visible widgets of the bar overlap (sharing an edge is fine).
fn assert_apart(bar: &[Drawn], at: &str) {
    for (i, a) in bar.iter().enumerate().filter(|(_, d)| d.visible) {
        for b in bar.iter().skip(i + 1).filter(|d| d.visible) {
            let o = a.rect.intersect(b.rect);
            assert!(o.width() <= 0.5 || o.height() <= 0.5, "{at}: `{}` {:?} overlaps `{}` {:?}", a.id, a.rect, b.id, b.rect);
        }
    }
}

/// How the bar fits, from what it shows: 0 everything; 1 the row count as an icon; 2 that and
/// no "n changes" text; 3 the right group cut at the bar's edge (Apply not whole). Checks the
/// order on the way: the changes' text goes only after the count's, and the cut comes last.
fn fit_of(bar: &[Drawn], at: &str) -> u8 {
    let get = |id: &str| bar.iter().find(|d| d.id == id);
    let icon = get("page-count").is_some_and(|d| d.rect.width() <= 27.0);
    let text = get("edits-count").is_some_and(|d| d.visible);
    let cut = get("edit-apply").is_none_or(|d| !d.visible || d.rect.width() < 63.5);
    // Once the right group is cut the left one may be cut too, so the count's form shows only
    // before.
    assert!(cut || text || icon, "{at}: the changes' text went before the count's: {bar:?}");
    assert!(!cut || !text, "{at}: the right group is cut while the changes' text could give room up: {bar:?}");
    if cut {
        3
    } else if !text {
        2
    } else {
        u8::from(icon)
    }
}

#[test]
fn the_paging_bar_draws_nothing_over_anything_at_the_default_size() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // The app's window (apps/dbee: 1100 × 720) with the value panel open, its default.
    let mut ui = connected_sized(db, 0.25, egui::vec2(1100.0, 720.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    assert_eq!(ui.edits()["count"], 1);
    ui.frame();
    let bar = ui.bar();
    assert_apart(&bar, "1100 × 720");
    let fit = fit_of(&bar, "1100 × 720");
    let at = |id: &str| bar.iter().find(|d| d.id == id).map(|d| d.rect);
    let between = at("row-add").zip(at("table-refresh")).map(|(add, refresh)| add.left() - refresh.right());
    println!(
        "the paging bar at 1100 × 720 with a change staged: form {fit} (0 all, 1 count icon, 2 no changes text), {between:?} points between the groups, the bar {:?} to the value panel at {}",
        at("page-first").map(|r| r.left()),
        ui.panel_left()
    );
    assert!(fit <= 2, "nothing is cut at the default size: {bar:?}");
    let right = ui.panel_left();
    for id in ["page-first", "page-prev", "page-next", "page-last", "page-count", "table-refresh", "row-add", "edit-reset", "edit-apply"] {
        let d = bar.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("`{id}` is drawn: {bar:?}"));
        assert!(d.visible && d.rect.right() <= right, "`{id}` shows, left of the value panel at {right}: {d:?}");
    }
    for id in ["page-first", "page-prev", "page-next", "page-last", "table-refresh"] {
        let d = bar.iter().find(|d| d.id == id).unwrap();
        assert_eq!(d.rect.size(), egui::vec2(26.0, 26.0), "`{id}` shows whole");
    }
    // They take their clicks: + Row adds a row, Reset drops both changes, Apply writes one.
    ui.click("row-add");
    assert_eq!(ui.edits()["count"], 2, "+ Row added a row");
    ui.click("edit-reset");
    assert_eq!(ui.edits()["count"], 0, "Reset dropped the changes");
    ui.double_click("grid-cell-1-1");
    ui.text("Bea");
    ui.ctrl("Enter");
    ui.frame();
    assert_apart(&ui.bar(), "1100 × 720, staged again");
    ui.click("edit-apply");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT name FROM scratch.{} WHERE id = 2", t.0)), "Bea");
    // With nothing staged, refresh runs again on a click.
    ui.click("table-refresh");
    ui.until("the refreshed page", |ui| ui.table()["loading"].is_null());
    assert_eq!(ui.table()["rows"], 3);
}

#[test]
fn a_narrowing_paging_bar_gives_room_up_in_order_and_never_overlaps() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = connected_sized(db, 0.25, egui::vec2(1400.0, 720.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    assert_eq!(ui.edits()["count"], 1);
    // The window narrows 20 points at a time; the panels keep their widths, so the bar loses it all.
    let mut seen = Vec::new();
    for width in (760..=1400).rev().step_by(20) {
        ui.harness.set_size(egui::vec2(width as f32, 720.0));
        ui.frame();
        ui.frame();
        let at = format!("{width} × 720");
        let bar = ui.bar();
        assert_apart(&bar, &at);
        let fit = fit_of(&bar, &at);
        assert!(seen.last().is_none_or(|&(_, last)| fit >= last), "{at}: the bar took room back while narrowing: {seen:?} then {fit}");
        if fit == 3 {
            // Cut at the bar's edge, never over the left group (`assert_apart`).
            let right = ui.panel_left();
            for d in bar.iter().filter(|d| d.visible) {
                assert!(d.rect.right() <= right + 0.5, "{at}: `{}` runs past the bar into the value panel at {right}: {:?}", d.id, d.rect);
            }
        }
        seen.push((width, fit));
    }
    println!("the paging bar's form by window width (0 all, 1 count icon, 2 no changes text, 3 cut): {seen:?}");
    for fit in 0..=3 {
        assert!(seen.iter().any(|&(_, f)| f == fit), "the sweep shows fit {fit}: {seen:?}");
    }
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

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn keystroke_frame_with_a_value_at_the_cap() {
    let db = dbee_testkit::require_testdb!();
    let cap = dbee_ui_egui::editors::MAX_INLINE_BYTES;
    // One value of each kind just under the cap, each one paragraph (the slow case: egui lays
    // out a paragraph whole on every change): a single-line `varchar` and a multiline `text`.
    let name = unique("t_edit_big");
    let line = cap - 1;
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, v varchar, t text);
             INSERT INTO scratch.{name} VALUES (1, repeat('x', {line}), repeat('y', {line}));"
        ),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = connected(db);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 1);
    for (col, what) in [(1, "single-line varchar"), (2, "multiline text")] {
        ui.double_click(&format!("grid-cell-0-{col}"));
        assert!(ui.editor().is_some(), "{what} opens");
        // The value opens selected: go to its end, so each key adds to all of it.
        ui.ctrl("End");
        let mut frames = Vec::new();
        for i in 0..30 {
            ui.harness.input_mut().events.push(egui::Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
            let start = Instant::now();
            ui.frame();
            // What the window's renderer is handed too: the frame's shapes as triangles.
            let shapes = ui.harness.output().shapes.clone();
            let ppp = ui.harness.ctx.pixels_per_point();
            let _ = ui.harness.ctx.tessellate(shapes, ppp);
            frames.push(start.elapsed());
        }
        frames.sort();
        let median = frames[frames.len() / 2];
        println!(
            "keystroke in a {what} editor holding {} bytes: frame median {median:?} (max {:?}); on {}",
            cap - 1,
            frames.last().unwrap(),
            machine()
        );
        ui.ctrl("Enter");
        let staged = ui.staged(0, col).and_then(|v| v.as_str().map(str::len));
        assert_eq!(staged, Some(line + 30), "{what}: every key went into the whole value");
        // Two frames at 60 Hz.
        assert!(median < Duration::from_millis(33), "{what}: {median:?}");
    }
}

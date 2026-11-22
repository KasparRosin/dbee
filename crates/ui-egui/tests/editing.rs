//! Cell editing driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the inline editors (double-click, or Enter on the selected cell), their keys
//! (Enter, Ctrl+Enter, Tab, Escape), booleans typed as text, "+ Row", Apply with Ctrl+S, undo and redo,
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

/// The carets drawn in the last frame inside `within`: 1 px upright lines in `ink`.
fn carets(ui: &Ui, within: egui::Rect, ink: egui::Color32) -> Vec<[egui::Pos2; 2]> {
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::LineSegment { points, stroke } if stroke == egui::Stroke::new(1.0, ink) && points[0].x == points[1].x => Some(points),
            _ => None,
        })
        .filter(|p| within.expand(2.0).contains(p[0]) && within.expand(2.0).contains(p[1]))
        .collect()
}

/// A double-click, or Enter on the selected cell, opens the editor with the keyboard and its
/// caret showing from its first frame, in light and in dark: in the cell for one line; in the
/// value panel for a text of several lines, whose cell keeps the `editing` outline and has no
/// caret of its own. What is typed next goes into it.
#[test]
fn the_editors_caret_shows_from_its_first_frame() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    for (mode, command, tokens) in [("light", "ui.themeLight", dbee_ui_egui::theme::LIGHT), ("dark", "ui.themeDark", dbee_ui_egui::theme::DARK)] {
        ui.execute(command, json!({}));
        ui.frame();
        for (n, (cell, value, in_cell)) in
            [("grid-cell-0-2", "10.50", true), ("grid-cell-1-3", "2025-02-03", true), ("grid-cell-0-1", "Alice", false)]
                .into_iter()
                .cycle()
                .take(12)
                .enumerate()
        {
            let at = format!("{mode}, opening {n} on {cell}");
            // Every other opening is by the keyboard, where no click falls on the new editor.
            if n % 2 == 0 {
                ui.click(cell);
            }
            for _ in 0..n % 4 {
                ui.frame();
            }
            if n % 2 == 0 {
                ui.key("Enter");
            } else {
                ui.double_click(cell);
            }
            if ui.editor().is_none() {
                ui.frame();
            }
            assert_eq!(ui.editor_text(), value, "{at}");
            let (editor, cell_at) = (ui.drawn("cell-editor"), ui.drawn(cell));
            let seen = carets(&ui, editor, tokens.ink);
            assert_eq!(seen.len(), 1, "{at}: one caret in the editor {editor:?} in its first frame: {seen:?}");
            if in_cell {
                assert!(cell_at.expand(1.0).contains_rect(editor), "{at}: the editor {editor:?} over its cell {cell_at:?}");
            } else {
                assert!(!cell_at.intersects(editor), "{at}: several lines are edited in the panel: {editor:?}, the cell {cell_at:?}");
                assert_eq!(carets(&ui, cell_at, tokens.ink), [] as [[egui::Pos2; 2]; 0], "{at}: the cell has no caret of its own");
            }
            ui.frame();
            assert_eq!(carets(&ui, editor, tokens.ink).len(), 1, "{at}: a frame later");
            ui.text("7");
            assert_eq!(ui.editor_text(), "7", "{at}: typing replaces the value");
            assert_eq!(carets(&ui, editor, tokens.ink).len(), 1, "{at}: the caret after typing");
            ui.key("Escape");
            assert!(ui.editor().is_none(), "{at}");
            // Past egui's double-click time, so the next opening is no triple click.
            for _ in 0..3 {
                ui.frame();
            }
        }
    }
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
    assert_eq!(ui.selection(), json!({ "anchor": [1, 2], "focus": [1, 2], "whole": false }), "and moved right");
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
    assert_eq!(ui.selection(), json!({ "anchor": [2, 0], "focus": [2, 0], "whole": false }));
    // On the last cell of the last row it commits and stays.
    ui.click("grid-cell-2-5");
    ui.key("Enter");
    ui.text("{\"b\": 2}");
    ui.key("Tab");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 5), Some(json!("{\"b\": 2}")));
    assert_eq!(ui.selection(), json!({ "anchor": [2, 5], "focus": [2, 5], "whole": false }));
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
fn saved_cells_show_their_mark_then_fade() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // Frames a sixtieth of a second apart on egui's clock, so the mark's second and a half spans
    // many of them.
    let mut ui = connected_at(db, 1.0 / 60.0, 1400.0);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    let tokens = *dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let state = |ui: &mut Ui, id: &str| ui.value(id).get("state").cloned();
    let now = |ui: &Ui| ui.harness.ctx.input(|i| i.time);
    // A point of the cell's ground: inside its outline, left of its right-aligned number.
    let ground = |ui: &Ui, id: &str| {
        let r = ui.drawn(id);
        colour_at(ui, egui::pos2(r.left() + 4.0, r.center().y))
    };

    ui.double_click("grid-cell-1-2");
    ui.text("7.25");
    ui.ctrl("S");
    ui.until("the saved mark", |ui| state(ui, "grid-cell-1-2") == Some(json!("saved")));
    let shown = now(&ui);
    assert_eq!(ui.table()["saved"], json!([[1, 2]]));
    assert_eq!(ground(&ui, "grid-cell-1-2"), Some(tokens.saved), "at full");
    assert_eq!(state(&mut ui, "grid-cell-1-1"), None, "only the cell written");
    // A refresh takes it off at once.
    ui.click("table-refresh");
    ui.frame();
    assert_eq!(state(&mut ui, "grid-cell-1-2"), None);
    assert!(now(&ui) - shown < 1.0, "before it would have faded");
    ui.until("the page again", |ui| ui.table()["loading"].is_null());

    // Left alone, it shows at full for a second, then fades out over half a second, while the
    // cell stays selected.
    ui.double_click("grid-cell-0-2");
    ui.text("9.75");
    ui.ctrl("S");
    ui.until("the saved mark", |ui| state(ui, "grid-cell-0-2") == Some(json!("saved")));
    let shown = now(&ui);
    ui.hover_away();
    let mut fading = Vec::new();
    ui.until("the mark to go", |ui| {
        let gone = state(ui, "grid-cell-0-2").is_none();
        let (at, colour) = (now(ui) - shown, ground(ui, "grid-cell-0-2"));
        if !gone && at < 0.9 {
            assert_eq!(colour, Some(tokens.saved), "at full at {at:.2} s");
        }
        if !gone && (1.1..1.4).contains(&at) {
            fading.extend(colour);
        }
        gone
    });
    let gone = now(&ui) - shown;
    assert!((1.4..=1.6).contains(&gone), "gone {gone:.2} s after it showed");
    assert!(!fading.is_empty() && fading.iter().all(|c| c.a() > 0 && c.a() < tokens.saved.a()), "it faded: {fading:?}");
    assert_eq!(ground(&ui, "grid-cell-0-2"), Some(tokens.selection), "then the selected cell's own ground");
    assert_eq!(ui.table()["saved"], json!([[0, 2]]), "the engine still lists what the Apply wrote");
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
    ui.until("the Apply's job", |ui| ui.tag("status-job").is_some());
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
    // A one-line cell: its editor is drawn over the cell (several lines go in the value panel).
    ui.execute("selection.set", json!({ "anchor": [2, 2], "focus": [2, 2] }));
    ui.key("Enter");
    assert_eq!(ui.inspect()["editor"], json!({ "row": 2, "column": 2 }), "Enter opened an editor");
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
    assert_eq!(ui.selection(), json!({ "anchor": [0, 0], "focus": [0, 0], "whole": false }), "the click selected its cell");
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
    // A window with no room for the value panel: a several-lines editor floats over its cell,
    // and one on the last column is moved left to stay in view.
    let mut ui = connected_sized(db, 0.25, egui::vec2(640.0, 420.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-5");
    let wide = ui.tag("cell-editor").unwrap();
    let doc = ui.tag("grid-cell-0-5").unwrap();
    assert!(wide["rect"][0].as_f64().unwrap() < doc["rect"][0].as_f64().unwrap(), "moved left: {wide} {doc}");
    ui.key("Escape");
    // The one-line editor that follows is drawn over its own cell from its first frame.
    ui.double_click("grid-cell-1-4");
    let line = ui.tag("cell-editor").unwrap();
    let flag = ui.tag("grid-cell-1-4").unwrap();
    // egui puts an area on whole points.
    let near = |i: usize| (line["rect"][i].as_f64().unwrap() - flag["rect"][i].as_f64().unwrap()).abs() <= 1.0;
    assert!(near(0) && near(1), "{line} {flag}");
    ui.key("Escape");
}

#[test]
fn an_editor_scrolled_out_of_view_sideways_commits() {
    let db = dbee_testkit::require_testdb!();
    // Ten wide one-line columns: far wider than the window. (A several-line editor is in the
    // value panel, which its cell scrolling away leaves open.)
    let name = unique("t_edit_wide");
    let columns: Vec<String> = (1..=10).map(|c| format!("c{c} varchar(200)")).collect();
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
fn ctrl_z_in_a_boolean_editor_undoes_its_typing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    assert_eq!(ui.edits()["count"], 1);
    // A boolean holds text like any one-line cell: Ctrl+Z is its own undo.
    ui.double_click("grid-cell-1-4");
    ui.text("x");
    assert_eq!(ui.editor_text(), "x");
    ui.ctrl("Z");
    assert!(ui.editor().is_some(), "the editor is still open");
    assert_eq!(ui.editor_text(), "f", "Ctrl+Z undid the typing in the editor");
    assert_eq!(ui.edits()["count"], 1, "the staged name stays");
    ui.key("Escape");
    assert!(ui.editor().is_none());
    ui.ctrl("Z");
    assert_eq!(ui.edits()["count"], 0, "outside the editor Ctrl+Z undoes the staged name");
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
    assert_eq!(ui.selection(), json!({ "anchor": [0, 5], "focus": [0, 5], "whole": false }), "to the end of the row before");
}

#[test]
fn booleans_are_typed_true_false_or_emptied() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-4");
    assert_eq!(ui.editor(), Some(json!({ "text": "t", "multiline": false })), "the cell's text, as Postgres prints it");
    assert!(ui.tag("cell-editor-true").is_none(), "no choice");
    // Emptied: NULL.
    ui.ctrl("A");
    ui.key("Delete");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 4), Some(Json::Null), "an emptied boolean is NULL");
    // The same text again changes nothing.
    ui.double_click("grid-cell-1-4");
    assert_eq!(ui.editor_text(), "f");
    ui.key("Enter");
    assert_eq!(ui.staged(1, 4), None);
    // A NULL cell opens empty; a typed word is staged as Postgres prints a bool.
    ui.double_click("grid-cell-2-4");
    assert_eq!(ui.editor_text(), "");
    ui.text("false");
    ui.key("Enter");
    assert_eq!(ui.staged(2, 4), Some(json!("f")), "staged as Postgres prints a bool");
    ui.double_click("grid-cell-1-4");
    ui.text("TRUE");
    ui.key("Enter");
    assert_eq!(ui.staged(1, 4), Some(json!("t")));
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT string_agg(coalesce(flag::text, 'null'), ',' ORDER BY id) FROM scratch.{}", t.0)), "null,true,false");
}

/// Review Focus 2: a boolean is one line of text on every engine. It opens on the cell's text as
/// the database prints it, all selected; a typed word is staged in the database's spelling; NULL
/// opens empty, an emptied editor stages NULL; Ctrl+S writes them.
#[test]
fn a_boolean_is_edited_as_text_on_every_engine() {
    let db = dbee_testkit::require_testdb!();
    let mut targets: Vec<dbee_testkit::TestTarget> =
        ["pg14", "mysql80", "mariadbnew"].into_iter().map(|n| dbee_testkit::target(db.server(n).unwrap())).collect();
    targets.extend(dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]));
    for t in &targets {
        let at = t.name();
        let (schema, name) = (t.schema("scratch"), unique("t_bool_text"));
        t.sql(&format!(
            "CREATE TABLE {schema}.{name} (id int PRIMARY KEY, flag boolean); INSERT INTO {schema}.{name} VALUES (1, true), (2, false), (3, NULL), (4, true);"
        ))
        .unwrap();
        let _drop = t.server().map(|s| ScratchOn(s, name.clone()));
        let (yes, no) = if t.kind() == dbee_testkit::Kind::Postgres { ("t", "f") } else { ("1", "0") };
        let mut ui = Ui::connected_with(db, Setup { step: Some(0.25), config: Some(config_of(t)), ..Setup::default() });
        ui.open(schema, &name);
        ui.until("the page", |ui| ui.table()["rows"] == 4);
        ui.double_click("grid-cell-0-1");
        assert_eq!(ui.editor(), Some(json!({ "text": yes, "multiline": false })), "{at}: the cell's text");
        assert!(ui.tag("cell-editor-true").is_none(), "{at}: no choice");
        // All selected: typing replaces it.
        ui.text("false");
        ui.key("Enter");
        assert!(ui.editor().is_none(), "{at}");
        assert_eq!(ui.staged(0, 1), Some(json!(no)), "{at}: staged in the database's spelling");
        ui.double_click("grid-cell-2-1");
        assert_eq!(ui.editor_text(), "", "{at}: NULL opens empty");
        ui.key("Escape");
        ui.double_click("grid-cell-3-1");
        ui.ctrl("A");
        ui.key("Delete");
        ui.key("Enter");
        assert_eq!(ui.staged(3, 1), Some(Json::Null), "{at}: emptied is NULL");
        ui.ctrl("S");
        ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
        let flag = "CASE WHEN flag IS NULL THEN 'null' WHEN flag THEN 'true' ELSE 'false' END";
        let held = t.sql(&format!("SELECT {flag} FROM {schema}.{name} ORDER BY id")).unwrap();
        assert_eq!(held.lines().map(str::trim).collect::<Vec<_>>().join(","), "false,false,null,null", "{at}");
    }
}

/// A MySQL `BOOLEAN` is a `TINYINT(1)`, printed `1` and `0`: the editor opens on the `1`, and
/// `false` typed there writes `0`.
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
    assert_eq!(ui.editor(), Some(json!({ "text": "1", "multiline": false })), "one line, on the 1 MySQL prints");
    ui.text("false");
    ui.key("Enter");
    assert_eq!(ui.staged(0, 1), Some(json!("0")), "staged as MySQL prints a false BOOLEAN");
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(dbee_testkit::mysql(server, &format!("SELECT flag FROM scratch.{name}")).unwrap().trim(), "0");
}

/// A MySQL `BOOLEAN` holding 5 opens as one line on the 5, as every boolean does, and Enter
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
    assert_eq!(ui.selection(), json!({ "anchor": [3, 0], "focus": [3, 0], "whole": false }), "the new row's first cell is selected");
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

/// The paging bar's widgets, in their order along it. The row count, refresh and + Row are the
/// table bar's.
const BAR: [&str; 9] = ["page-first", "page-prev", "page-number", "page-next", "page-last", "page-cut", "edits-count", "edit-reset", "edit-apply"];

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

    /// Where the paging bar ends: the body's right edge (the value panel stands above it).
    fn bar_right(&mut self) -> f32 {
        self.tab_x().max
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

/// How the bar fits, from what it shows: 0 everything; 1 no "n changes" text; 2 the right group
/// cut at the bar's edge (Apply not whole). Checks the order on the way: the cut comes last.
fn fit_of(bar: &[Drawn], at: &str) -> u8 {
    let get = |id: &str| bar.iter().find(|d| d.id == id);
    let text = get("edits-count").is_some_and(|d| d.visible);
    // Apply whole is 60 wide: 12, APPLY, 12.
    let cut = get("edit-apply").is_none_or(|d| !d.visible || d.rect.width() < 59.5);
    assert!(!cut || !text, "{at}: the right group is cut while the changes' text could give room up: {bar:?}");
    if cut { 2 } else { u8::from(!text) }
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
    let between = at("edits-count").zip(at("page-last")).map(|(changes, last)| changes.left() - last.right());
    println!(
        "the paging bar at 1100 × 720 with a change staged: form {fit} (0 all, 1 no changes text), {between:?} points between the groups, the bar {:?} to the body's edge at {}",
        at("page-first").map(|r| r.left()),
        ui.bar_right()
    );
    assert!(fit <= 1, "nothing is cut at the default size: {bar:?}");
    let right = ui.bar_right();
    for id in ["page-first", "page-prev", "page-next", "page-last", "edit-reset", "edit-apply"] {
        let d = bar.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("`{id}` is drawn: {bar:?}"));
        assert!(d.visible && d.rect.right() <= right, "`{id}` shows, inside the body, to {right}: {d:?}");
    }
    for id in ["page-first", "page-prev", "page-next", "page-last"] {
        let d = bar.iter().find(|d| d.id == id).unwrap();
        assert_eq!(d.rect.size(), egui::vec2(28.0, 28.0), "`{id}` shows whole");
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
    // The window narrows 20 points at a time; the tree keeps its width, so the bar loses it all.
    let mut seen = Vec::new();
    for width in (400..=1400).rev().step_by(20) {
        ui.harness.set_size(egui::vec2(width as f32, 720.0));
        ui.frame();
        ui.frame();
        let at = format!("{width} × 720");
        let bar = ui.bar();
        assert_apart(&bar, &at);
        let fit = fit_of(&bar, &at);
        assert!(seen.last().is_none_or(|&(_, last)| fit >= last), "{at}: the bar took room back while narrowing: {seen:?} then {fit}");
        if fit == 2 {
            // Cut at the bar's edge, never over the left group (`assert_apart`).
            let right = ui.bar_right();
            for d in bar.iter().filter(|d| d.visible) {
                assert!(d.rect.right() <= right + 0.5, "{at}: `{}` runs past the bar's end at {right}: {:?}", d.id, d.rect);
            }
        }
        seen.push((width, fit));
    }
    println!("the paging bar's form by window width (0 all, 1 no changes text, 2 cut): {seen:?}");
    for fit in 0..=2 {
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

/// The paging bar's Reset and Apply, moved to `components::button`, say in `ui.inspect` what they
/// said before: ids, values, enabled flags.
#[test]
fn moved_buttons_keep_their_ids_values_and_flags() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = connected_sized(db, 0.25, egui::vec2(1100.0, 720.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    ui.frame();
    let staged: [Moved<'_>; 2] = [("edit-reset", Some(json!("Reset")), true, "RESET"), ("edit-apply", Some(json!("Apply")), true, "APPLY")];
    assert_moved(&mut ui, &staged, "a change staged");
    ui.click("edit-reset");
    assert_eq!(ui.edits()["count"], 0, "Reset dropped the change");
}

/// What is drawn inside `at`: the colours of its rects' edges, and of its lines and discs (an
/// icon).
fn marks_in(ui: &Ui, at: egui::Rect) -> (Vec<egui::Color32>, Vec<egui::Color32>) {
    let (mut edges, mut lines) = (Vec::new(), Vec::new());
    for (_, s) in ui.shapes() {
        if !at.expand(0.5).contains_rect(s.visual_bounding_rect()) {
            continue;
        }
        match s {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color.a() > 0 => edges.push(r.stroke.color),
            egui::Shape::Path(p) => match p.stroke.color {
                egui::epaint::ColorMode::Solid(c) => lines.push(c),
                egui::epaint::ColorMode::UV(_) => lines.push(egui::Color32::RED),
            },
            egui::Shape::Circle(c) => lines.push(c.fill),
            _ => {}
        }
    }
    lines.retain(|c| *c != egui::Color32::TRANSPARENT);
    lines.dedup();
    (edges, lines)
}

/// Where the last frame drew `id`.
fn rect_of(ui: &mut Ui, id: &str) -> egui::Rect {
    let e = ui.tag(id).unwrap_or_else(|| panic!("`{id}` is drawn"));
    let r: Vec<f32> = (0..4).map(|k| e["rect"][k].as_f64().unwrap() as f32).collect();
    egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]))
}

/// The paging bar's arrows, and refresh and + Row in the table bar, say in `ui.inspect` what
/// they said before: ids, values, enabled flags; the arrows are the page's plain icon buttons,
/// refresh and + Row the table bar's cells.
#[test]
fn moved_icon_buttons_keep_their_ids_values_and_flags() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = connected_sized(db, 0.25, egui::vec2(1100.0, 720.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.frame();
    let check = |ui: &mut Ui, want: &[(&str, &str, bool)], at: &str| {
        for (id, value, enabled) in want {
            let e = ui.tag(id).unwrap_or_else(|| panic!("{at}: `{id}` is drawn"));
            assert_eq!(e["value"], json!(value), "{at}: `{id}`'s value: {e}");
            assert_eq!(e.get("enabled") != Some(&json!(false)), *enabled, "{at}: `{id}` enabled: {e}");
            let size = rect_of(ui, id).size();
            let page = if ["row-add", "table-refresh"].contains(id) { size.y == 35.0 } else { size == egui::vec2(28.0, 28.0) };
            assert!(page, "{at}: `{id}` is the page's size: {size:?}");
        }
    };
    // One page of three rows: the arrows are off, refresh and + Row on.
    let idle = [
        ("page-first", "First page", false),
        ("page-prev", "Previous page", false),
        ("page-next", "Next page", false),
        ("page-last", "Last page (counts the rows)", false),
        ("table-refresh", "Refresh", true),
        ("row-add", "+ Row", true),
    ];
    check(&mut ui, &idle, "one page");
    // A change staged: refresh waits.
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    ui.frame();
    let staged = [("table-refresh", "Refresh", false), ("row-add", "+ Row", true), ("page-first", "First page", false)];
    check(&mut ui, &staged, "a change staged");
    ui.click("row-add");
    assert_eq!(ui.edits()["count"], 2, "+ Row added a row");
    ui.click("edit-reset");
    ui.frame();
    check(&mut ui, &idle, "reset");
    ui.click("table-refresh");
    ui.until("the refreshed page", |ui| ui.table()["loading"].is_null());
    assert_eq!(ui.table()["rows"], 3);
}

/// An arrow or refresh that is off can't be clicked, draws its icon faint with no edge, and
/// hovering it says why, as before the move.
#[test]
fn paging_arrows_that_are_off_still_say_why() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = connected_sized(db, 0.25, egui::vec2(1100.0, 720.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.frame();
    let faint = dbee_ui_egui::theme::tokens(&ui.harness.ctx).text_faint;
    let off = |ui: &mut Ui, id: &str, why: &str| {
        let at = rect_of(ui, id);
        let (edges, lines) = marks_in(ui, at);
        assert_eq!((edges, lines), (vec![], vec![faint]), "`{id}`: off, its icon faint and no edge");
        ui.rest(at.center());
        assert!(ui.texts().iter().any(|(_, g)| g.text() == why), "`{id}`: hovered, it says `{why}`");
        // A press on it does nothing.
        let page = ui.table()["page"].clone();
        ui.frame_with(vec![
            egui::Event::PointerButton { pos: at.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            egui::Event::PointerButton { pos: at.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
        ]);
        ui.frame();
        assert_eq!(ui.table()["page"], page, "`{id}`: a click asked for no page");
        assert!(ui.table()["loading"].is_null(), "`{id}`: nothing loads");
    };
    off(&mut ui, "page-first", "First page: not now, this is the first page");
    off(&mut ui, "page-prev", "Previous page: not now, this is the first page");
    off(&mut ui, "page-next", "Next page: not now, this is the last page");
    off(&mut ui, "page-last", "Last page (counts the rows): not now, this is the last page");
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    ui.frame();
    off(&mut ui, "table-refresh", "Refresh: not now, apply or reset your changes first");
}

/// A failed Apply shows in the error band under the table bar, the body's full width: its heading,
/// the server's message, then DETAIL over the server's detail (Postgres's `Failing row contains`).
/// The failed row stays marked.
#[test]
fn a_failed_apply_shows_the_band_with_its_detail() {
    let db = dbee_testkit::require_testdb!();
    let name = unique("t_customers");
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} AS SELECT id, name, email FROM shop.customers ORDER BY id LIMIT 3;
             ALTER TABLE scratch.{name} ADD PRIMARY KEY (id), ALTER COLUMN name SET NOT NULL;"
        ),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = opened(db, &t);
    ui.right_click("grid-cell-0-1");
    ui.click("grid-menu-null");
    ui.ctrl("S");
    ui.until("the failed Apply", |ui| ui.table()["failed"].is_object());
    ui.hover_away();
    let failed = ui.table()["failed"].clone();
    let message = failed["message"].as_str().unwrap_or_default().to_owned();
    let detail = failed["detail"].as_str().unwrap_or_default().to_owned();
    assert!(detail.starts_with("Failing row contains"), "{failed}");
    assert_eq!(ui.value("table-error"), message.as_str());
    let band = ui.drawn("table-error");
    let body = ui.body_x();
    assert!((band.left() - body.min).abs() <= 0.5 && (band.right() - body.max).abs() <= 0.5, "full width {body:?}: {band:?}");
    assert!(band.top() > ui.drawn("table-refresh").bottom(), "under the table bar: {band:?}");
    assert!(band.bottom() < ui.drawn("grid-header-0").top(), "over the grid, its line between: {band:?}");
    let top = |words: &str| ui.texts().into_iter().find(|(r, g)| g.text() == words && band.contains(r.center())).map_or(f32::NAN, |(r, _)| r.top());
    // The message as the band draws it: the server's words, without the engine's `change 1
    // failed, so nothing was written: ` before them (the heading says it) and without the
    // `Detail: …` line, which shows as a field.
    let shown = message.strip_suffix(&format!("\nDetail: {detail}")).unwrap_or_else(|| panic!("the detail ends the message: {message}"));
    let shown = shown.strip_prefix("change 1 failed, so nothing was written: ").unwrap_or_else(|| panic!("the engine's words first: {message}"));
    let tops = ["The changes were not applied", shown, "DETAIL", detail.as_str()].map(top);
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "the heading, the message, DETAIL, the detail, top to bottom: {tops:?}");
    assert!(ui.texts().iter().all(|(_, g)| !g.text().contains("Detail: ")), "the detail shows once, as a field");
    assert_eq!(ui.value("grid-cell-0-0")["state"], "failed", "the failed row stays marked");
}

// ---- several lines in the value panel (MC+) -------------------------------------------------------

/// The orders table open in a window of `size`, its 3 rows shown.
fn opened_in(db: &TestDb, t: &Scratch, size: egui::Vec2) -> Ui {
    let mut ui = connected_sized(db, 0.25, size);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui
}

/// `inner` lies inside `outer` (to half a point).
fn inside(inner: egui::Rect, outer: egui::Rect) -> bool {
    outer.expand(0.5).contains_rect(inner)
}

/// A text cell opens its editor in the value panel: the cell keeps its old text in the `editing`
/// look, the panel's bar has CANCEL and SAVE, its facts follow the typing, and SAVE stages.
#[test]
fn several_lines_are_edited_in_the_panel() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened_in(db, &t, APP_SIZE);
    let tokens = *dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    ui.double_click("grid-cell-0-1");
    let editor = ui.editor().expect("an editor is open");
    assert_eq!(editor, json!({ "text": "Alice", "multiline": true }));
    let (at, panel, cell) = (rect_of(&mut ui, "cell-editor"), rect_of(&mut ui, "value-panel"), rect_of(&mut ui, "grid-cell-0-1"));
    assert!(inside(at, panel), "the editor is the panel's text: {at:?} in {panel:?}");
    assert!(!at.intersects(cell), "not over the cell: {at:?} {cell:?}");
    assert!(ui.tag("value-save").is_some() && ui.tag("value-cancel").is_some(), "CANCEL and SAVE in the bar");
    for id in ["value-copy", "value-wrap", "value-text"] {
        assert!(ui.tag(id).is_none(), "no `{id}` while the panel edits");
    }
    assert_eq!(ui.value("value-panel")["editing"], true);
    // The cell: `card`, a 2 px `ink` edge inside it, its old text.
    let edges = outlines_in(&ui, cell.expand(0.5), tokens.ink);
    assert!(edges.iter().any(|(r, w)| *w == 2.0 && (r.width() - cell.width()).abs() <= 1.0), "a 2 px ink edge: {edges:?}");
    assert_eq!(colour_at(&ui, cell.center()), Some(tokens.card), "on `card`");
    assert!(ui.text_in(cell, "Alice").is_some(), "its old text");
    // No edge round the panel or its text: the caret shows it is edited.
    for colour in [tokens.ink, tokens.line_strong] {
        let round = outlines_in(&ui, panel.expand(2.0), colour);
        assert!(round.iter().all(|(r, _)| r.width() < panel.width() / 2.0), "no edge round the panel: {round:?}");
    }
    // Typing: the facts follow it; the cell keeps its old text.
    ui.text("a");
    ui.key("Enter");
    ui.text("b");
    assert_eq!(ui.editor_text(), "a\nb");
    ui.frame();
    assert_eq!(ui.value("value-panel")["facts"], "2 lines 3 B");
    assert!(ui.text_in(cell, "Alice").is_some(), "the cell still shows its old text");
    ui.click("value-save");
    assert!(ui.editor().is_none(), "SAVE closed the editor");
    assert_eq!(ui.staged(0, 1), Some(json!("a\nb")));
    ui.frame();
    assert_eq!(ui.value("value-panel")["editing"], false);
    assert_eq!(ui.value("value-text")["text"], "a\nb", "the panel shows the staged value");
}

/// Every way a panel editor closes stages what the floating editor staged in the same case.
#[test]
fn the_panel_editor_commits_and_cancels_as_the_floating_one_did() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened_in(db, &t, APP_SIZE);
    let open = |ui: &mut Ui, cell: &str| {
        ui.double_click(cell);
        assert_eq!(ui.editor().map(|e| e["multiline"].clone()), Some(json!(true)), "{cell} edits in the panel");
        assert_eq!(ui.value("value-panel")["editing"], true);
    };
    // Escape, and CANCEL: nothing.
    open(&mut ui, "grid-cell-0-1");
    ui.text("x");
    ui.key("Escape");
    assert!(ui.editor().is_none());
    open(&mut ui, "grid-cell-0-1");
    ui.text("x");
    ui.click("value-cancel");
    assert!(ui.editor().is_none(), "CANCEL closed it");
    assert_eq!(ui.edits()["count"], 0, "nothing staged");
    // Ctrl+Enter stages.
    open(&mut ui, "grid-cell-0-1");
    ui.text("A1");
    ui.ctrl("Enter");
    assert_eq!(ui.staged(0, 1), Some(json!("A1")));
    // A click on another cell stages and selects it.
    open(&mut ui, "grid-cell-1-1");
    ui.text("B1");
    ui.click("grid-cell-0-0");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(1, 1), Some(json!("B1")));
    assert_eq!(ui.selection(), json!({ "anchor": [0, 0], "focus": [0, 0], "whole": false }));
    // A click on the toggle stages, then hides the panel.
    open(&mut ui, "grid-cell-2-1");
    ui.text("C1");
    ui.click("value-toggle");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 1), Some(json!("C1")));
    ui.click("value-toggle");
    // Ctrl+S stages, then applies everything.
    open(&mut ui, "grid-cell-0-1");
    ui.text("S1");
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.edits()["count"] == 0);
    let names = format!("SELECT string_agg(name, ',' ORDER BY id) FROM scratch.{}", t.0);
    ui.until("the names written", |_| q(db, &names) == "S1,B1,C1");
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    // Ctrl+W stages, then asks.
    open(&mut ui, "grid-cell-0-1");
    ui.text("W1");
    ui.ctrl("W");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 1), Some(json!("W1")));
    assert!(ui.ok("ui.inspect", json!({}))["dialog"].is_string(), "closing with a staged change asks");
    ui.click("dialog-cancel");
    // An agent's `row.delete` under it: discarded, with today's notice.
    for _ in 0..3 {
        ui.execute("row.add", json!({}));
    }
    open(&mut ui, "grid-cell-4-1");
    ui.text("Zed");
    ui.execute("row.delete", json!({ "rows": [3] }));
    ui.frame();
    assert!(ui.editor().is_none(), "the editor closed");
    let notice = ui.notice();
    assert!(notice.contains("discarded"), "{notice}");
    assert_eq!(ui.staged(0, 1), Some(json!("W1")), "the earlier change stays, nothing else was written");
}

/// A panel editor stays open while its cell is scrolled out of view: it is not over the cell.
#[test]
fn a_panel_editor_stays_open_when_its_cell_scrolls_away() {
    let db = dbee_testkit::require_testdb!();
    let name = unique("t_edit_wide_text");
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
    ui.ctrl("A");
    ui.text("abc");
    ui.wheel("grid-cell-3-2", egui::vec2(-2000.0, 0.0));
    let cell = ui.tag("grid-cell-0-1").unwrap();
    assert_eq!(cell["visible"], false, "c1 is out of view: {cell}");
    assert_eq!(ui.editor_text(), "abc", "the editor is still open in the panel");
    assert_eq!(ui.edits()["count"], 0, "nothing staged yet");
    ui.ctrl("Enter");
    assert_eq!(ui.staged(0, 1), Some(json!("abc")));
}

/// Keys in the panel editor are its own: Ctrl+C copies its selection, Ctrl+A selects its text,
/// Ctrl+Enter saves (no query runs), Tab commits and moves, Ctrl+Z undoes typing. After it closes
/// the grid has its keys back.
#[test]
fn keys_in_the_panel_editor_stay_in_it() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = connected_sized(db, 0.25, APP_SIZE);
    // A query tab behind, with a statement Ctrl+Enter would run.
    ui.ctrl("T");
    ui.frame();
    let query_tab = ui.document()["active"].clone();
    ui.text("SELECT 1");
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    ui.key("End");
    for _ in 0..3 {
        ui.key_with("ArrowLeft", json!({ "shift": true }));
    }
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some("ice"), "the editor's selection");
    ui.ctrl("A");
    assert!(ui.editor().is_some(), "Ctrl+A kept the editor open");
    assert_eq!(ui.selection(), json!({ "anchor": [0, 1], "focus": [0, 1], "whole": false }), "the grid's selection is unchanged");
    ui.text("Z");
    assert_eq!(ui.editor_text(), "Z", "Ctrl+A selected all of the editor's text");
    ui.ctrl("Z");
    assert_eq!(ui.editor_text(), "Alice", "Ctrl+Z undid the typing");
    ui.text("Q");
    ui.ctrl("Enter");
    assert!(ui.editor().is_none(), "Ctrl+Enter saved");
    assert_eq!(ui.staged(0, 1), Some(json!("Q")));
    ui.no_notice();
    // Tab commits and selects the next cell.
    ui.double_click("grid-cell-1-1");
    ui.text("T");
    ui.key("Tab");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(1, 1), Some(json!("T")));
    assert_eq!(ui.selection(), json!({ "anchor": [1, 2], "focus": [1, 2], "whole": false }));
    // The grid has its keys back: Ctrl+C copies its cell.
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some("20.00"));
    // The query tab ran nothing.
    ui.click(&format!("tab-{query_tab}"));
    assert_eq!(ui.document()["query"]["results"], json!([]), "no statement ran");
}

/// With the panel hidden, a several-line edit shows it, and it stays shown.
#[test]
fn a_hidden_panel_opens_for_a_several_line_edit() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened_in(db, &t, APP_SIZE);
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({}))["visible"], false);
    ui.frame();
    assert!(ui.tag("value-panel").is_none(), "hidden");
    ui.double_click("grid-cell-0-1");
    ui.frame();
    assert_eq!(ui.value("value-panel")["editing"], true, "the panel shows, editing");
    assert_eq!(ui.value("value-toggle"), true);
    assert!(inside(rect_of(&mut ui, "cell-editor"), rect_of(&mut ui, "value-panel")));
    ui.text("P");
    ui.click("value-save");
    assert_eq!(ui.staged(0, 1), Some(json!("P")));
    ui.frame();
    assert_eq!(ui.value("value-panel")["editing"], false, "the panel stays");
    assert_eq!(ui.value("value-toggle"), true);
}

/// With no room for the panel (640 × 420), several lines are edited over the cell, as before.
#[test]
fn with_no_room_for_the_panel_text_is_edited_over_the_cell() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened_in(db, &t, egui::vec2(640.0, 420.0));
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor(), Some(json!({ "text": "Alice", "multiline": true })));
    assert!(ui.tag("value-panel").is_none(), "no panel");
    let (at, cell) = (rect_of(&mut ui, "cell-editor"), rect_of(&mut ui, "grid-cell-0-1"));
    // Over the cell (moved left as far as it must to stay in view, as before).
    assert!(
        at.left() <= cell.left() + 1.0 && at.right() >= cell.right() - 1.0 && (at.top() - cell.top()).abs() <= 1.0,
        "over the cell: {at:?} {cell:?}"
    );
    assert!((at.width() - 320.0).abs() <= 1.0, "320 wide: {at:?}");
    ui.ctrl("A");
    ui.text("N");
    ui.ctrl("Enter");
    assert_eq!(ui.staged(0, 1), Some(json!("N")));
}

/// The colour of the text drawn for byte range `bytes` of the galley reading `text` inside `at`.
fn colour_of(ui: &Ui, at: egui::Rect, text: &str, bytes: std::ops::Range<usize>) -> Option<egui::Color32> {
    let (_, galley) = ui.texts().into_iter().find(|(r, g)| g.text() == text && at.expand(1.0).intersects(*r))?;
    galley
        .job
        .sections
        .iter()
        .find(|s| usize::from(s.byte_range.start) <= bytes.start && bytes.end <= usize::from(s.byte_range.end))
        .map(|s| s.format.color)
}

/// JSON being edited keeps its colours up to the first spot that doesn't parse; after it the text
/// is plain `text`.
#[test]
fn json_is_coloured_while_it_is_edited() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened_in(db, &t, APP_SIZE);
    let tokens = *dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    ui.double_click("grid-cell-0-5");
    let text = ui.editor_text();
    assert_eq!(text, "{\"a\": 1}", "the value as stored");
    let at = rect_of(&mut ui, "cell-editor");
    assert_eq!(colour_of(&ui, at, &text, 1..4), Some(tokens.syn_keyword), "the key");
    assert_eq!(colour_of(&ui, at, &text, 6..7), Some(tokens.syn_number), "the number");
    ui.key("End");
    ui.text("{");
    let text = ui.editor_text();
    assert_eq!(text, "{\"a\": 1}{");
    assert_eq!(colour_of(&ui, at, &text, 1..4), Some(tokens.syn_keyword), "the key still");
    assert_eq!(colour_of(&ui, at, &text, 8..9), Some(tokens.text), "after the broken spot: `text`");
    ui.key("Escape");
}

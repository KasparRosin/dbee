//! The value panel driven through the control channel, one frame at a time in a kittest harness
//! over [`DbeeApp::ui`]: it follows the selected cell (staged values included), names the JSON it
//! shows, hides and comes back at the width it had, copies its own selection with Ctrl+C, and
//! shows a 20 MB value cut, with a note, without a slow frame. Assertions read `ui.inspect`,
//! `document.inspect` and what the frames put on the clipboard.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read; the test that
//! stages edits makes its own table in `scratch` and drops it.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use dbee_testkit::{TestDb, unique};
use dbee_ui_egui::value_panel::{LINE_H, PAD};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// How far left of the text box's right edge a click goes to land past a row's end but not on
/// the scroll bar.
const BESIDE_BAR: f32 = 30.0;

/// The slowest frame the 20 MB cell may cost.
const FRAME_BOUND: Duration = Duration::from_millis(250);

impl Ui {
    /// Where `id` was drawn.
    fn rect(&mut self, id: &str) -> Rect {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"));
        let r = &tag["rect"];
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        Rect::from_min_size(pos2(f(0), f(1)), vec2(f(2), f(3)))
    }

    /// A click at window point `at`, with Shift held when `shift`.
    fn click_at(&mut self, at: Pos2, shift: bool) {
        self.ok("ui.click", json!({ "x": at.x, "y": at.y, "modifiers": { "shift": shift } }));
    }

    /// A drag with the primary button from `from` to `to`, over several frames.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame_with(vec![Event::PointerMoved(from)]);
        self.frame_with(vec![press(from, true)]);
        for i in 1..=8 {
            self.frame_with(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))]);
        }
        self.frame_with(vec![press(to, false)]);
        self.frame();
    }

    /// The wheel over `at`, `delta` points, then frames for egui to ease the scroll.
    fn wheel(&mut self, at: Pos2, delta: egui::Vec2) {
        self.frame_with(vec![Event::PointerMoved(at)]);
        self.frame_with(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        }]);
        for _ in 0..12 {
            self.frame();
        }
    }

    /// Opens `schema.table` and waits until `rows` rows show.
    fn open_rows(&mut self, schema: &str, table: &str, rows: u64) {
        self.execute("table.open", json!({ "schema": schema, "table": table }));
        self.until("the page", |ui| ui.table()["rows"] == rows);
    }

    /// The text `value-text` shows (its start, for a long value).
    fn panel_text(&mut self) -> String {
        let v = self.value("value-text");
        v["text"].as_str().unwrap_or_else(|| panic!("value-text shows no text: {v}")).to_string()
    }
}

// shop.orders: id 0, customer_id 1, dealer_id 2, status 3, total 4, billing 5 (jsonb), meta 6
// (json: a string holding a JSON object), shipped_at 7, raw_json_text 8 (text holding JSON).

#[test]
fn jsonb_cell_shows_pretty_json_with_label() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    // Nothing selected yet: the panel is there, empty, with a hint.
    assert_eq!(ui.value("value-panel"), Json::Null);
    assert!(ui.tag("value-text").is_none());
    ui.click("grid-cell-0-5");
    let head = ui.value("value-panel");
    assert_eq!((head["column"].clone(), head["type"].clone(), head["label"].clone()), (json!("billing"), json!("jsonb"), json!("jsonb")), "{head}");
    assert_eq!((head["truncated"].clone(), head["note"].clone(), head["null"].clone()), (json!(false), Json::Null, json!(false)), "{head}");
    let text = ui.panel_text();
    assert!(text.starts_with("{\n  \"name\": \"Customer 1\",\n"), "pretty-printed: {text:?}");
    // The same view the engine reports, coloured as JSON.
    assert_eq!(ui.document()["value"]["text"], json!(text));
    let v = ui.value("value-text");
    assert_eq!((v["highlighted"].clone(), v["cut"].clone(), v["selection"].clone()), (json!(true), json!(false), Json::Null), "{v}");
    // No kind chip (K2): `jsonb` is drawn once, as the type; the facts say lines and bytes.
    let panel = ui.rect("value-panel");
    assert_eq!(drawn_in(&ui, panel, "jsonb"), 1, "the type alone, no chip");
    let facts = ui.value("value-panel")["facts"].as_str().unwrap_or_default().to_string();
    assert!(facts_read(&facts, false), "`12 lines 137 B`'s form: {facts:?}");
    let lines = facts.split(' ').take(2).collect::<Vec<_>>().join(" ").to_uppercase();
    assert!(ui.text_in(panel, &lines).is_some(), "{lines} drawn in capitals");
    // Another cell of the row: the panel follows.
    ui.click("grid-cell-0-3");
    assert_eq!(ui.value("value-panel")["column"], "status");
    assert_eq!(ui.panel_text(), ui.value("grid-cell-0-3")["text"].as_str().unwrap());
    assert_eq!(ui.value("value-panel")["label"], Json::Null, "an enum is no JSON");
    assert_eq!(ui.value("value-text")["highlighted"], false);
}

#[test]
fn json_string_containing_json_is_labelled() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    ui.click("grid-cell-0-6");
    let head = ui.value("value-panel");
    assert_eq!((head["type"].clone(), head["label"].clone()), (json!("json"), json!("JSON string containing JSON")), "{head}");
    // The document inside the string, pretty-printed; no chip, the facts say why it differs.
    assert_eq!(ui.panel_text(), "{\n  \"a\": 1\n}");
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, "JSON STRING CONTAINING JSON").is_none(), "no chip");
    let facts = ui.value("value-panel")["facts"].as_str().unwrap_or_default().to_string();
    assert!(facts.starts_with("Formatted as JSON 3 lines ") && facts_read(facts.trim_start_matches("Formatted as JSON "), false), "{facts:?}");
    assert!(ui.text_in(panel, "FORMATTED AS JSON").is_some(), "drawn in capitals");
    // A text column holding JSON says so too.
    ui.click("grid-cell-0-8");
    let head = ui.value("value-panel");
    assert_eq!((head["type"].clone(), head["label"].clone()), (json!("text"), json!("text containing JSON")), "{head}");
    assert_eq!(ui.panel_text(), "{\n  \"k\": [\n    1,\n    2\n  ]\n}");
}

#[test]
fn panel_hides_and_comes_back() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    ui.click("grid-cell-0-5");
    let before = ui.rect("value-panel");
    // At the window's right edge (the harness keeps 8 points round the app).
    assert_eq!(before.right(), 1400.0 - 8.0, "on the right: {before:?}");
    // Widen it by its left edge, so the width it comes back at is its own, not the default. The
    // press goes just inside the edge: a grid column's own edge can lie on the panel's.
    ui.drag(pos2(before.left() + 2.0, before.center().y), pos2(before.left() - 80.0, before.center().y));
    let wide = ui.rect("value-panel");
    assert!((wide.width() - before.width() - 80.0).abs() < 2.0, "dragged 80 points wider: {before:?} → {wide:?}");
    assert_eq!(ui.value("value-toggle"), true);
    ui.click("value-toggle");
    assert!(ui.tag("value-panel").is_none(), "hidden");
    assert!(ui.tag("value-text").is_none());
    assert_eq!(ui.value("value-toggle"), false);
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": true }));
    ui.frame();
    assert_eq!(ui.rect("value-panel"), wide, "back where it was, as wide as it was");
    assert!(ui.panel_text().starts_with("{\n"), "still following the selected cell");
    // The View menu has it too.
    ui.click("menu-View");
    ui.click("menu-ui.toggleValuePanel");
    assert!(ui.tag("value-panel").is_none(), "hidden from the menu");
    ui.click("value-toggle");
    assert_eq!(ui.rect("value-panel"), wide);
}

#[test]
fn ctrl_c_copies_the_panel_selection_not_the_grid() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    ui.click("grid-cell-0-5");
    let text = ui.panel_text();
    let lines: Vec<&str> = text.split('\n').collect();
    let first_two = format!("{}\n{}", lines[0], lines[1]);
    // From the start of the first line to the end of the second: a click, then a shift-click.
    let r = ui.rect("value-text");
    ui.click_at(pos2(r.left() + 1.0, r.top() + LINE_H * 0.5), false);
    ui.click_at(pos2(r.right() - PAD, r.top() + LINE_H * 1.5), true);
    assert_eq!(ui.value("value-text")["selection"], json!([0, first_two.len()]));
    let copies = ui.copied.len();
    ui.ctrl("C");
    assert_eq!(ui.copied.len(), copies + 1, "one copy: {:?}", ui.copied);
    assert_eq!(ui.copied().as_deref(), Some(first_two.as_str()), "the panel's selection, not the grid's");
    // The grid's selection is still the cell, whose copy is the raw jsonb on one line.
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 5], "focus": [0, 5], "whole": false }));
    // A drag selects too: across the third line, from its start.
    let third = r.top() + LINE_H * 2.5;
    ui.drag(pos2(r.left() + 1.0, third), pos2(r.right() - PAD, third));
    assert_eq!(ui.value("value-text")["selection"], json!([first_two.len() + 1, first_two.len() + 1 + lines[2].len()]));
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some(lines[2]));
    // Ctrl+A in the panel selects all of its text, not the grid's cells.
    ui.ctrl("A");
    assert_eq!(ui.value("value-text")["selection"], json!([0, text.len()]));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 5], "focus": [0, 5], "whole": false }), "the grid's selection stays");
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some(text.as_str()));
    assert!(ui.tag("notice").is_none(), "the panel's copy is a text field's: no notice");
    // A click on the grid ends the panel's selection: Ctrl+C copies the grid's again.
    ui.click("grid-cell-0-0");
    assert_eq!(ui.value("value-text")["selection"], Json::Null);
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some("1"), "the grid's cell (order 1)");
    assert_eq!(ui.value("notice"), "Copied 1 row");
}

#[test]
fn the_panel_follows_staged_values_and_null() {
    let db = dbee_testkit::require_testdb!();
    let name = unique("t_value_panel");
    dbee_testkit::psql(
        pg14_server(db),
        &format!("CREATE TABLE scratch.{name} (id int PRIMARY KEY, note text); INSERT INTO scratch.{name} VALUES (1, 'plain'), (2, NULL);"),
    )
    .unwrap();
    let _t = Scratch(name.clone());
    let mut ui = Ui::connected(db);
    ui.open_rows("scratch", &name, 2);
    ui.click("grid-cell-1-1");
    let head = ui.value("value-panel");
    assert_eq!((head["column"].clone(), head["type"].clone(), head["null"].clone()), (json!("note"), json!("text"), json!(true)), "{head}");
    assert_eq!(ui.panel_text(), "", "NULL has no text to select");
    ui.click("grid-cell-0-1");
    assert_eq!(ui.panel_text(), "plain");
    assert_eq!(ui.value("value-panel")["null"], false);
    // A staged value shows as the grid shows it, here a text holding JSON.
    ui.execute("cell.edit", json!({ "row": 0, "column": 1, "value": "{\"b\": true}" }));
    ui.frame();
    assert_eq!(ui.value("grid-cell-0-1")["state"], "staged");
    assert_eq!(ui.value("value-panel")["label"], "text containing JSON");
    assert_eq!(ui.panel_text(), "{\n  \"b\": true\n}");
    ui.execute("edit.undo", json!({}));
    ui.frame();
    assert_eq!(ui.panel_text(), "plain", "undone: the loaded value again");
}

#[test]
fn a_twenty_megabyte_cell_says_first_10_mib_in_its_facts() {
    let db = dbee_testkit::require_testdb!();
    twenty_megabyte_cell(db);
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn a_twenty_megabyte_cell_says_first_10_mib_in_its_facts_without_a_slow_frame() {
    let db = dbee_testkit::require_testdb!();
    for (what, t) in twenty_megabyte_cell(db) {
        assert!(t < FRAME_BOUND, "{what} took a {t:?} frame");
    }
}

/// The 20 MB text and jsonb of the seed's big cell in the value panel: cut, with a note, wrapped,
/// scrolled, coloured. The slowest frame of each step.
fn twenty_megabyte_cell(db: &TestDb) -> [(&'static str, Duration); 4] {
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "big_cell", 1);
    // The 20,000,000-character text: selecting it builds its view in the frame of the click.
    ui.time_frames();
    ui.click("grid-cell-0-1");
    let select = ui.slowest;
    let head = ui.value("value-panel");
    assert_eq!((head["truncated"].clone(), head["bytes"].clone()), (json!(true), json!(20_000_000)), "{head}");
    let note = head["note"].as_str().unwrap_or_default().to_string();
    assert_eq!(note, "Showing the first 10.0 MiB of 19.1 MiB.", "the old sentence stays readable");
    // N2: the facts say it; nothing is painted above the value.
    assert_eq!(head["facts"], "1 line First 10 MiB of 19.1 MiB", "{head}");
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, &note).is_none(), "no note painted");
    assert!(ui.text_in(panel, "FIRST 10 MIB OF 19.1 MIB").is_some(), "the facts in capitals");
    let text = ui.rect("value-text");
    assert!(near(text.top(), panel.top() + 36.0 + 12.0, 0.5), "the value 12 under the bar: {text:?} in {panel:?}");
    let v = ui.value("value-text");
    assert_eq!(v["cut"], true, "the tag carries the start of the text: {}", v["text"]);
    assert!(v["text"].as_str().unwrap().starts_with("0123456789abcdef"));
    // Wrapped at the panel's width into many rows, of which only those in view are laid out.
    let rows = v["rows"].as_u64().unwrap();
    assert!(rows > 100_000, "{rows} rows");
    assert_eq!(v["firstRow"], 0);
    // Scrolling it stays a frame or so.
    let r = ui.rect("value-text");
    ui.time_frames();
    for _ in 0..5 {
        ui.wheel(r.center(), vec2(0.0, -2_000.0));
    }
    let scroll = ui.slowest;
    let first = ui.value("value-text")["firstRow"].as_u64().unwrap();
    assert!(first > 100, "scrolled down: row {first}");
    // The 20 MB jsonb: cut, unformatted, said so; coloured once its colours are worked out.
    ui.time_frames();
    ui.click("grid-cell-0-2");
    let select_json = ui.slowest;
    let head = ui.value("value-panel");
    assert_eq!((head["label"].clone(), head["truncated"].clone()), (json!("jsonb"), json!(true)), "{head}");
    assert!(head["note"].as_str().is_some_and(|n| n.contains("Too long to format")), "{head}");
    assert_eq!(ui.value("value-text")["firstRow"], 0, "a new value starts at its top");
    ui.until("the colours", |ui| ui.value("value-text")["highlighted"] == true);
    let colours = ui.slowest;
    println!("20 MB cell: select text {select:?}, scroll {scroll:?}, select jsonb {select_json:?}, until coloured {colours:?} (slowest frames)");
    [("selecting the text", select), ("scrolling", scroll), ("selecting the jsonb", select_json), ("colouring", colours)]
}

/// `scratch.<u> (id int PK, note text, other text)`: one row whose `note` is 300 numbered lines.
fn long_note(db: &TestDb) -> Scratch {
    let name = unique("t_value_long");
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, note text, other text);
             INSERT INTO scratch.{name} SELECT 1, string_agg('line ' || g, E'\\n' ORDER BY g), 'x' FROM generate_series(1, 300) AS g;"
        ),
    )
    .unwrap();
    Scratch(name)
}

#[test]
fn a_hidden_panel_builds_no_value_view() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": false }));
    let built = |ui: &Ui| ui.harness.state().session().value_view_computations();
    let before = built(&ui);
    ui.click("grid-cell-0-5");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(built(&ui), before, "no view of 10 MiB is worked out for a panel nobody sees");
    assert!(ui.copied().is_some_and(|c| c.starts_with("{\"name\": \"Customer 1\"")), "Ctrl+C copied the grid's cell: {:?}", ui.copied());
    ui.click("value-toggle");
    assert!(built(&ui) > before, "shown again: it shows the cell");
    assert!(ui.panel_text().starts_with("{\n"));
}

#[test]
fn dragging_the_edge_wraps_again_once_released() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "big_cell", 1);
    ui.click("grid-cell-0-1");
    let rows = ui.value("value-text")["rows"].as_u64().unwrap();
    let edge = ui.rect("value-panel");
    let (from, to) = (pos2(edge.left(), edge.center().y), pos2(edge.left() - 120.0, edge.center().y));
    let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![press(from, true)]);
    for i in 1..=8 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))]);
    }
    assert!(ui.rect("value-panel").width() > edge.width() + 100.0, "the panel follows the drag");
    assert_eq!(ui.value("value-text")["rows"].as_u64(), Some(rows), "the text keeps its rows while the edge moves");
    ui.frame_with(vec![press(to, false)]);
    ui.frame();
    let after = ui.value("value-text")["rows"].as_u64().unwrap();
    assert!(after < rows, "wrapped again at the new width once released: {rows} → {after}");
}

#[test]
fn a_staged_edit_elsewhere_keeps_the_panel_where_it_was() {
    let db = dbee_testkit::require_testdb!();
    let t = long_note(db);
    let mut ui = Ui::connected(db);
    ui.open_rows("scratch", &t.0, 1);
    ui.click("grid-cell-0-1");
    let r = ui.rect("value-text");
    ui.wheel(r.center(), vec2(0.0, -1_000.0));
    let first = ui.value("value-text")["firstRow"].as_u64().unwrap();
    assert!(first > 10, "scrolled down: row {first}");
    ui.click_at(pos2(r.left() + 1.0, r.top() + LINE_H * 0.5), false);
    ui.click_at(pos2(r.right() - BESIDE_BAR, r.top() + LINE_H * 2.5), true);
    let selection = ui.value("value-text")["selection"].clone();
    assert!(selection.is_array(), "{selection}");
    // An edit of another cell, then its undo: the engine shows a new view of the same text.
    ui.execute("cell.edit", json!({ "row": 0, "column": 2, "value": "y" }));
    ui.frame();
    let v = ui.value("value-text");
    assert_eq!((v["firstRow"].as_u64(), v["selection"].clone()), (Some(first), selection.clone()), "kept after the edit: {v}");
    ui.execute("edit.undo", json!({}));
    ui.frame();
    let v = ui.value("value-text");
    assert_eq!((v["firstRow"].as_u64(), v["selection"].clone()), (Some(first), selection), "kept after the undo: {v}");
}

#[test]
fn a_selection_whose_start_scrolled_away_extends_and_copies_whole() {
    let db = dbee_testkit::require_testdb!();
    let t = long_note(db);
    let mut ui = Ui::connected(db);
    ui.open_rows("scratch", &t.0, 1);
    ui.click("grid-cell-0-1");
    let text = ui.document()["value"]["text"].as_str().unwrap().to_string();
    let r = ui.rect("value-text");
    // The start: the first line's first character.
    ui.click_at(pos2(r.left() + 1.0, r.top() + LINE_H * 0.5), false);
    // Scrolled until it is far out of view, then extended to the end of a row in view.
    ui.wheel(r.center(), vec2(0.0, -2_000.0));
    let first = ui.value("value-text")["firstRow"].as_u64().unwrap();
    assert!(first > 50, "the start is out of view: row {first}");
    ui.click_at(pos2(r.right() - BESIDE_BAR, r.top() + LINE_H * 3.5), true);
    let selection = ui.value("value-text")["selection"].clone();
    let end = selection[1].as_u64().unwrap() as usize;
    assert_eq!(selection[0], 0, "from the start that scrolled away: {selection}");
    // To the end of a line in view, far below the start.
    let last: u64 = text[..end].rsplit('\n').next().and_then(|l| l.strip_prefix("line ")).and_then(|n| n.parse().ok()).unwrap();
    assert!(last > first && text[end..].starts_with('\n'), "to the end of a line in view: line {last}, first row {first}");
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some(&text[..end]), "the whole range, not the part in view");
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
fn value_panel_frames_within_budget() {
    use dbee_core::{Column, TypeClass};
    use dbee_engine::ValueView;
    use dbee_ui_egui::automation::Tags;
    use dbee_ui_egui::value_panel::{PanelState, contents};

    struct State {
        view: Option<Arc<ValueView>>,
        panel: PanelState,
        tags: Tags,
        width: f32,
    }
    let column = |name: &str, base: &str| {
        let class = if base == "jsonb" { TypeClass::Json } else { TypeClass::LongText };
        Column { name: name.into(), base_type: base.into(), type_label: base.into(), class }
    };
    // The seed's two 20 MB values: one line of text, and a jsonb array of 320,000 objects.
    let text = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_".repeat(312_500);
    let items: Vec<String> = (1..=320_000).map(|i| format!("{{\"i\": {i}, \"s\": \"{}\"}}", "z".repeat(40))).collect();
    let jsonb = format!("[{}]", items.join(", "));
    // Pretty JSON just under the cap: many short lines.
    let small: Vec<String> = (1..=120_000).map(|i| format!("{{\"i\":{i},\"s\":\"{}\"}}", "z".repeat(40))).collect();
    let under = format!("[{}]", small.join(","));
    // Hostile shapes: nothing but line breaks, and one line of two-byte characters.
    let breaks = "\n".repeat(20_000_000);
    let wide = "õ".repeat(10_000_000);
    let tabs = "\tkey\t🐙 value\n".repeat(1_100_000);
    let views = [
        ("20 MB text", Arc::new(ValueView::new(&column("t", "text"), Some(&text)))),
        ("20 MB of line breaks", Arc::new(ValueView::new(&column("t", "text"), Some(&breaks)))),
        ("20 MB of õ", Arc::new(ValueView::new(&column("t", "text"), Some(&wide)))),
        ("20 MB of tabs and emoji", Arc::new(ValueView::new(&column("t", "text"), Some(&tabs)))),
        ("20 MB jsonb", Arc::new(ValueView::new(&column("j", "jsonb"), Some(&jsonb)))),
        ("9 MiB pretty jsonb", Arc::new(ValueView::new(&column("j", "jsonb"), Some(&under)))),
    ];
    let state = State { view: None, panel: PanelState::default(), tags: Tags::default(), width: 360.0 };
    let mut h = Harness::builder().with_size(vec2(400.0, 900.0)).build_ui_state(
        |ui, s: &mut State| {
            s.tags.begin_frame();
            let rect = Rect::from_min_size(ui.max_rect().min, vec2(s.width, ui.max_rect().height()));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            contents(&mut child, s.view.as_ref(), &mut s.panel, &mut s.tags);
        },
        state,
    );
    h.step();
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let timed = |h: &mut Harness<'_, State>| {
        let start = Instant::now();
        h.step();
        start.elapsed()
    };
    let mut report = Vec::new();
    for (name, view) in views {
        h.state_mut().view = Some(view);
        let select = timed(&mut h);
        let mut coloured = 0;
        while h.state().tags.value("value-text").is_some_and(|v| v["highlighted"] == false)
            && h.state().view.as_ref().is_some_and(|v| !v.kind_label.is_empty())
        {
            coloured += 1;
            assert!(coloured < 10_000, "{name}: no colours");
            std::thread::sleep(Duration::from_millis(1));
            h.step();
        }
        let steady = median((0..60).map(|_| timed(&mut h)).collect());
        let at = h.state().tags.rect("value-text").unwrap().center();
        let mut scroll = Vec::new();
        for _ in 0..60 {
            h.input_mut().events.push(Event::PointerMoved(at));
            h.input_mut().events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -400.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            });
            scroll.push(timed(&mut h));
        }
        let first = h.state().tags.value("value-text").unwrap()["firstRow"].as_u64().unwrap();
        // The width changing each frame: wrapped again each time (a width that changes without
        // the edge being dragged), then while the edge is dragged (rows held), then let go.
        let mut rewrap = Vec::new();
        for i in 0..30 {
            h.state_mut().width = 360.0 - (i % 2) as f32 * 40.0;
            rewrap.push(timed(&mut h));
        }
        h.state_mut().panel.hold_rows(true);
        let mut dragging = Vec::new();
        for i in 0..30 {
            h.state_mut().width = 360.0 - ((i + 1) % 2) as f32 * 40.0;
            dragging.push(timed(&mut h));
        }
        h.state_mut().panel.hold_rows(false);
        h.state_mut().width = 360.0;
        let released = timed(&mut h);
        let (scroll_max, steady_ms) = (*scroll.iter().max().unwrap(), steady);
        let scroll = median(scroll);
        let rewrap = median(rewrap);
        let dragging_max = *dragging.iter().max().unwrap();
        let dragging = median(dragging);
        report.push(format!(
            "{name}: select {select:?}, {coloured} frames until coloured, steady median {steady_ms:?}, scrolling median {scroll:?} (max {scroll_max:?}, down to row {first}), width changing median {rewrap:?}, while the edge drags median {dragging:?} (max {dragging_max:?}), let go {released:?}"
        ));
        assert!(dragging < Duration::from_millis(16), "{name}: a frame of an edge drag took {dragging:?}");
        assert!(select < FRAME_BOUND, "{name}: selecting took {select:?}");
        assert!(steady < Duration::from_millis(16) && scroll < Duration::from_millis(16), "{name}: {steady:?} {scroll:?}");
    }
    println!("value panel on {}:\n  {}", machine(), report.join("\n  "));
}

/// A selected row (#17) shows in the panel as JSON: the object Copy as JSON writes for it,
/// pretty-printed and coloured, read-only, under the title `Row <n>` (the number its row shows,
/// the page's offset counted in) with no type.
#[test]
fn the_panel_shows_a_selected_row_as_json() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "orders", 100);
    ui.execute("selection.set", json!({ "anchor": [0, 0], "focus": [0, 99] }));
    let list = ui.execute("copy.selection", json!({ "format": "json" }))["text"].as_str().unwrap().to_string();
    let object = list.strip_prefix("[\n  ").and_then(|l| l.strip_suffix("\n]")).unwrap();
    let pretty = dbee_core::json::pretty(object).unwrap();
    ui.click("grid-row-0");
    let head = ui.value("value-panel");
    assert_eq!(head["row"], 1, "{head}");
    assert_eq!((head["column"].clone(), head["type"].clone(), head["label"].clone()), (json!(""), json!(""), Json::Null), "{head}");
    assert_eq!((head["null"].clone(), head["truncated"].clone(), head["note"].clone()), (json!(false), json!(false), Json::Null), "{head}");
    assert_eq!(head["bytes"], object.len(), "{head}");
    assert!(pretty.starts_with("{\n  \"id\": 1,\n  \"customer_id\": 1,") && pretty.contains("\"billing\": {\n    \""), "{pretty}");
    assert_eq!(ui.panel_text(), pretty);
    assert_eq!(ui.document()["value"]["row"], 1);
    let v = ui.value("value-text");
    assert_eq!((v["highlighted"].clone(), v["cut"].clone(), v["selection"].clone()), (json!(true), json!(false), Json::Null), "{v}");
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, "Row 1").is_some(), "the title");
    assert!(ui.text_in(panel, "id").is_none() && ui.text_in(panel, "bigint").is_none(), "no column name or type in the bar");
    // A cell again: its column.
    ui.click("grid-cell-0-3");
    let head = ui.value("value-panel");
    assert_eq!((head["row"].clone(), head["column"].clone()), (Json::Null, json!("status")), "{head}");
    // The second page's first row is the 101st.
    ui.execute("table.page", json!({ "to": "next" }));
    ui.click("grid-row-0");
    assert_eq!(ui.value("value-panel")["row"], 101);
    assert!(ui.panel_text().starts_with("{\n  \"id\": 101,"), "{}", ui.panel_text());
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, "Row 101").is_some(), "the title counts the page");
}

// ---- the panel inside the tab ---------------------------------------------------------------------

/// The panel's rect while it is drawn this frame (egui keeps its size after it goes).
fn panel_rect(ui: &mut Ui) -> Option<Rect> {
    ui.tag("value-panel")?;
    egui::PanelState::load(&ui.harness.ctx, egui::Id::new("dbee-value-panel")).map(|p| p.outer_rect)
}

/// The panel shows, whatever it did before.
fn panel_open(ui: &mut Ui) {
    if ui.execute("ui.toggleValuePanel", json!({}))["visible"] == false {
        ui.execute("ui.toggleValuePanel", json!({}));
    }
    ui.frame();
}

/// The `value-toggle` entries drawn in the last frame.
fn toggles(ui: &mut Ui) -> Vec<Rect> {
    let i = ui.inspect();
    i["automation"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] == "value-toggle")
        .map(|e| {
            let f = |k: usize| e["rect"][k].as_f64().unwrap() as f32;
            Rect::from_min_size(pos2(f(0), f(1)), vec2(f(2), f(3)))
        })
        .collect()
}

fn near(a: f32, b: f32, by: f32) -> bool {
    (a - b).abs() <= by
}

/// One toggle at most, the active tab's, never in the status bar; the panel beside the results
/// only: in a query tab beside the result bar and the grid (the editor and the run row keep the
/// body's width), in a table tab between the table bar and the paging bar (both keep the body's
/// width). The tab strip spans the work area. A query tab before its first result has neither.
#[test]
fn exactly_one_value_toggle_and_the_panel_beside_the_results() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = Ui::connected_in(db, APP_SIZE);
    panel_open(&mut ui);
    let right = egui::PanelState::load(&ui.harness.ctx, egui::Id::new("dbee-menubar")).unwrap().outer_rect.right();
    let status = egui::PanelState::load(&ui.harness.ctx, egui::Id::new("dbee-status")).unwrap().outer_rect;
    // A query tab with results.
    ui.execute("tab.newQuery", json!({ "text": "select g as id, 'row ' || g as note from generate_series(1, 5) g" }));
    ui.execute("query.run", json!({}));
    ui.until("the rows", |ui| ui.tag("grid-header-0").is_some());
    ui.frame();
    let toggle = toggles(&mut ui);
    assert_eq!(toggle.len(), 1, "one toggle: {toggle:?}");
    let toggle = toggle[0];
    let panel = panel_rect(&mut ui).expect("the panel shows");
    // The result bar is the results column's: its toggle ends where the panel starts.
    assert!(near(toggle.right(), panel.left(), 1.0), "the toggle at the result bar's right end: {toggle:?}, the panel {panel:?}");
    let (run, head) = (ui.rect("query-run"), ui.rect("grid-header-0"));
    assert!(toggle.center().y > run.bottom() && toggle.center().y < head.top(), "in the result bar: {toggle:?}, {run:?}, {head:?}");
    assert!(near(panel.top(), toggle.top(), 1.0), "the panel's top level with the result bar's: {panel:?}, {toggle:?}");
    assert!(near(panel.right(), right, 0.5) && near(panel.bottom(), status.top(), 1.0), "down to the tab's bottom: {panel:?}");
    let ink = painted_at(&ui, pos2(panel.left() - 2.0, head.bottom() - 0.5));
    assert!(ink.is_some_and(|(c, r)| c == t.ink && near(r.right(), panel.left(), 1.0)), "the grid ends at the panel: {ink:?}, {panel:?}");
    for id in ["query-editor", "query-split"] {
        let r = ui.rect(id);
        assert!(near(r.right(), right, 0.5), "`{id}` keeps the body's width: {r:?}");
    }
    let new = ui.rect("tab-new");
    assert!(panel.top() > new.bottom(), "the panel stands under the strip: {panel:?}, {new:?}");
    let strip = painted_at(&ui, pos2(right - 2.0, new.bottom() - 0.5));
    assert!(strip.is_some_and(|(c, r)| c == t.line && near(r.right(), right, 0.5)), "the strip's line to the window's edge: {strip:?}");
    assert!(!status.intersects(toggle), "no toggle in the status bar");
    // Shut, the results take the body's width, the toggle at its right end.
    ui.click("value-toggle");
    assert!(ui.tag("value-panel").is_none(), "shut");
    let toggle = toggles(&mut ui);
    assert!(toggle.len() == 1 && near(toggle[0].right(), right, 0.5), "the toggle at the body's right end: {toggle:?}");
    ui.click("value-toggle");
    // A table tab: the toggle at the table bar's right end, the panel between the bars.
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders" }));
    ui.until("the page", |ui| ui.table()["rows"] == 100);
    ui.frame();
    let toggle = toggles(&mut ui);
    assert_eq!(toggle.len(), 1, "one toggle: {toggle:?}");
    let toggle = toggle[0];
    assert!(near(toggle.right(), right, 0.5), "the toggle at the table bar's right end: {toggle:?}");
    let head = ui.rect("grid-header-0");
    assert!(toggle.center().y < head.top() && toggle.center().y > ui.rect("filter-0-column").bottom(), "in the table bar: {toggle:?}");
    let panel = panel_rect(&mut ui).expect("the panel shows");
    let paging = status.top() - dbee_ui_egui::theme::size::BAR_H;
    assert!(near(panel.top(), toggle.bottom() + 1.0, 1.0), "the panel's top the table bar's bottom: {panel:?}, {toggle:?}");
    assert!(near(panel.bottom(), paging, 1.0), "its bottom the paging bar's top {paging}: {panel:?}");
    let line = painted_at(&ui, pos2(right - 2.0, paging + 0.5));
    assert!(line.is_some_and(|(c, r)| c == t.line && near(r.right(), right, 0.5)), "the paging bar to the body's right: {line:?}");
    assert!(!status.intersects(toggle), "no toggle in the status bar");
    // A new query tab: no result bar, no toggle, no panel.
    ui.ctrl("T");
    ui.frame();
    assert!(toggles(&mut ui).is_empty(), "no toggle before the first result");
    assert!(ui.tag("value-panel").is_none(), "no panel before the first result");
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": false }), "the View menu's command still flips it");
}

/// A fresh window's panel starts 340 wide.
#[test]
fn the_panel_starts_340_wide() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, vec2(1400.0, 900.0));
    panel_open(&mut ui);
    ui.open_rows("shop", "orders", 100);
    ui.frame();
    let panel = panel_rect(&mut ui).expect("the panel shows");
    assert!(near(panel.width(), 340.0, 1.0), "340 wide: {panel:?}");
}

/// The cell clicked shows in the panel in the frame of the click, in a query tab and a table tab.
#[test]
fn a_click_on_a_cell_shows_its_value_in_the_same_frame() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    panel_open(&mut ui);
    let click = |ui: &mut Ui, id: &str| {
        let at = ui.centre(id);
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        ui.frame_with(vec![Event::PointerMoved(at)]);
        ui.frame_with(vec![press(true)]);
        ui.frame_with(vec![press(false)]);
    };
    ui.execute("tab.newQuery", json!({ "text": "select 1 as id, 'x' as note" }));
    ui.execute("query.run", json!({}));
    ui.until("the rows", |ui| ui.tag("grid-cell-0-1").is_some());
    click(&mut ui, "grid-cell-0-1");
    assert_eq!(ui.value("value-panel")["column"], "note", "a query tab");
    ui.open_rows("shop", "orders", 100);
    ui.frame();
    click(&mut ui, "grid-cell-0-3");
    assert_eq!(ui.value("value-panel")["column"], "status", "a table tab");
}

// ---- COPY and the facts ---------------------------------------------------------------------------

/// How often `text` is drawn inside `within`.
fn drawn_in(ui: &Ui, within: Rect, text: &str) -> usize {
    ui.texts().iter().filter(|(r, g)| g.text() == text && within.expand(1.0).contains(r.center())).count()
}

/// `n lines m B` (or `1 line …`), with `cut`: `1 line First 10 MiB of …`.
fn facts_read(facts: &str, cut: bool) -> bool {
    let words: Vec<&str> = facts.split(' ').collect();
    let number = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == ',');
    match words.as_slice() {
        [n, unit, rest @ ..] if number(n) && (*unit == "lines" || (*unit == "line" && *n == "1")) => {
            if cut {
                rest.first() == Some(&"First")
            } else {
                matches!(rest, [b, "B"] if number(b)) || matches!(rest, [_, "KiB" | "MiB"])
            }
        }
        _ => false,
    }
}

/// COPY as `value-copy` says it: its word, and why it is off (`None` while on).
fn copy_state(ui: &mut Ui) -> (String, Option<String>) {
    let tag = ui.tag("value-copy").unwrap_or_else(|| panic!("no `value-copy`"));
    let word = tag["value"]["label"].as_str().unwrap_or_default().to_string();
    let off = tag["enabled"] == false;
    let why = tag["value"]["why"].as_str().map(str::to_string);
    assert_eq!(off, why.is_some(), "off exactly when it says why: {tag}");
    (word, why)
}

/// COPY copies the whole value as the database stores it (staged over loaded), a row as Copy as
/// JSON writes it; off with why for no cell, NULL and past 8 MiB; ✓ COPIED for 1.5 s, no notice.
#[test]
fn copy_copies_the_whole_value_as_stored() {
    let db = dbee_testkit::require_testdb!();
    let t = Scratch::new(db, "t_value_copy", |n| {
        format!(
            "CREATE TABLE scratch.{n} (id int PRIMARY KEY, note text);
             INSERT INTO scratch.{n} VALUES (1, repeat('x', 5 * 1024 * 1024)), (2, repeat('y', 9 * 1024 * 1024)), (3, NULL), (4, ''), (5, 'plain');"
        )
    });
    let mut ui = Ui::connected_in(db, APP_SIZE);
    panel_open(&mut ui);
    ui.open_rows("shop", "orders", 100);
    ui.frame();
    assert_eq!(copy_state(&mut ui), ("Copy".into(), Some("nothing is selected".into())), "no cell: off");

    // A whole row: the object Copy as JSON writes for it.
    ui.execute("selection.set", json!({ "anchor": [0, 0], "focus": [0, 99] }));
    let list = ui.execute("copy.selection", json!({ "format": "json" }))["text"].as_str().unwrap().to_string();
    let object = list.strip_prefix("[\n  ").and_then(|l| l.strip_suffix("\n]")).unwrap().to_string();
    ui.click("grid-row-0");
    ui.click("value-copy");
    assert_eq!(ui.copied().as_deref(), Some(object.as_str()), "the row as JSON");

    // A jsonb: the database's text, not the pretty form.
    ui.click("grid-cell-0-5");
    assert_eq!(copy_state(&mut ui), ("Copy".into(), None));
    let stored = dbee_testkit::psql(pg14_server(db), "SELECT billing::text FROM shop.orders WHERE id = 1").unwrap();
    let stored = stored.trim_end_matches('\n');
    assert!(!stored.contains('\n') && ui.panel_text().contains('\n'), "the panel shows it pretty, the database keeps one line");
    let copies = ui.copied.len();
    ui.click("value-copy");
    assert_eq!(ui.copied.len(), copies + 1, "one copy");
    assert_eq!(ui.copied().as_deref(), Some(stored), "as stored");
    assert_eq!(copy_state(&mut ui).0, "Copied");
    assert!(ui.tag("notice").is_none(), "no notice: COPIED says it");
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, "COPIED").is_some(), "✓ COPIED drawn");
    // 1.5 s of the harness clock (a frame is 0.25 s) and it reads COPY again.
    for _ in 0..8 {
        ui.frame();
    }
    assert_eq!(copy_state(&mut ui).0, "Copy", "back after 1.5 s");

    // The scratch table: 5 MiB whole, 9 MiB off, NULL off, '' copied, a staged value.
    ui.open_rows("scratch", &t.0, 5);
    ui.click("grid-cell-0-1");
    ui.click("value-copy");
    let five = ui.copied().unwrap_or_default();
    assert!(five.len() == 5 * 1024 * 1024 && five.bytes().all(|b| b == b'x'), "5 MiB whole: {} bytes", five.len());
    assert_eq!(ui.execute("clipboard.copyValue", json!({})), json!({ "bytes": 5 * 1024 * 1024 }), "the command says how much");
    ui.click("grid-cell-1-1");
    let (_, why) = copy_state(&mut ui);
    assert!(why.as_deref().is_some_and(|w| w.contains("9.0 MiB") && w.contains("8 MiB")), "9 MiB: off, {why:?}");
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copyValue", "params": {} }));
    assert_eq!((r["code"].clone(), r["data"]["why"].as_str().is_some_and(|w| w.contains("8 MiB"))), (json!("disabled"), true), "{r}");
    ui.click("grid-cell-2-1");
    assert_eq!(copy_state(&mut ui).1.as_deref(), Some("the value is NULL"));
    ui.click("grid-cell-3-1");
    let copies = ui.copied.len();
    ui.click("value-copy");
    assert_eq!((ui.copied.len(), ui.copied().as_deref()), (copies + 1, Some("")), "the empty text");
    ui.execute("cell.edit", json!({ "row": 4, "column": 1, "value": "staged text" }));
    ui.click("grid-cell-4-1");
    ui.click("value-copy");
    assert_eq!(ui.copied().as_deref(), Some("staged text"), "the staged value");
}

/// A text column holding JSON: its facts start `Formatted as JSON` (K2), no chip.
#[test]
fn a_text_column_holding_json_says_formatted_as_json() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    panel_open(&mut ui);
    ui.open_rows("shop", "orders", 100);
    ui.click("grid-cell-0-8");
    let head = ui.value("value-panel");
    assert_eq!(head["label"], "text containing JSON", "the label kept: {head}");
    let facts = head["facts"].as_str().unwrap_or_default();
    assert!(facts.starts_with("Formatted as JSON ") && facts_read(facts.trim_start_matches("Formatted as JSON "), false), "{head}");
    let panel = ui.rect("value-panel");
    assert!(ui.text_in(panel, "TEXT CONTAINING JSON").is_none(), "no chip");
    assert!(ui.text_in(panel, "FORMATTED AS JSON").is_some(), "the fact drawn");
    let text = ui.rect("value-text");
    assert!(near(text.top(), panel.top() + 36.0 + 12.0, 0.5), "the value 12 under the bar: {text:?}");
    // A jsonb isn't formatted: it is JSON.
    ui.click("grid-cell-0-5");
    let facts = ui.value("value-panel")["facts"].as_str().unwrap_or_default().to_string();
    assert!(!facts.starts_with("Formatted"), "{facts:?}");
}

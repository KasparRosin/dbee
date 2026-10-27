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

/// The slowest frame the 20 MB cell may cost (spec Review Focus 3).
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
    // The document inside the string, pretty-printed.
    assert_eq!(ui.panel_text(), "{\n  \"a\": 1\n}");
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
    // Widen it by its left edge, so the width it comes back at is its own, not the default.
    ui.drag(pos2(before.left(), before.center().y), pos2(before.left() - 80.0, before.center().y));
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 5], "focus": [0, 5] }));
    // A drag selects too: across the third line, from its start.
    let third = r.top() + LINE_H * 2.5;
    ui.drag(pos2(r.left() + 1.0, third), pos2(r.right() - PAD, third));
    assert_eq!(ui.value("value-text")["selection"], json!([first_two.len() + 1, first_two.len() + 1 + lines[2].len()]));
    ui.ctrl("C");
    assert_eq!(ui.copied().as_deref(), Some(lines[2]));
    // Ctrl+A in the panel selects all of its text, not the grid's cells.
    ui.ctrl("A");
    assert_eq!(ui.value("value-text")["selection"], json!([0, text.len()]));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 5], "focus": [0, 5] }), "the grid's selection stays");
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
fn a_twenty_megabyte_cell_shows_a_truncation_note_without_a_slow_frame() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open_rows("shop", "big_cell", 1);
    // The 20,000,000-character text: selecting it builds its view in the frame of the click.
    ui.time_frames();
    ui.click("grid-cell-0-1");
    let select = ui.slowest;
    let head = ui.value("value-panel");
    assert_eq!((head["truncated"].clone(), head["bytes"].clone()), (json!(true), json!(20_000_000)), "{head}");
    let note = head["note"].as_str().unwrap_or_default().to_string();
    assert!(note.contains("first 10.0 MiB of 19.1 MiB"), "{note}");
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
    for (what, t) in [("selecting the text", select), ("scrolling", scroll), ("selecting the jsonb", select_json), ("colouring", colours)] {
        assert!(t < FRAME_BOUND, "{what} took a {t:?} frame");
    }
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
#[ignore = "a timing test: cargo test -p dbee-ui-egui --test value_panel -- --ignored --nocapture"]
fn value_panel_frames_within_budget() {
    use dbee_core::Column;
    use dbee_engine::ValueView;
    use dbee_ui_egui::automation::Tags;
    use dbee_ui_egui::value_panel::{PanelState, contents};

    struct State {
        view: Option<Arc<ValueView>>,
        panel: PanelState,
        tags: Tags,
        width: f32,
    }
    let column = |name: &str, base: &str| Column { name: name.into(), base_type: base.into(), type_label: base.into() };
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
    let tabs = "\tkey\t🐝 value\n".repeat(1_100_000);
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

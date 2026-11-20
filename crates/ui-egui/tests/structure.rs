//! The structure view driven through the control channel, one frame at a time in a kittest harness
//! over [`DbeeApp::ui`]: STRUCTURE on the table bar, the bar's structure state, the page's
//! sections, its loading, its error, and DATA kept whole behind it. Assertions read `ui.inspect`,
//! `document.inspect` and what the frames painted.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read: a test that holds a
//! read behind a lock makes its own table in `scratch` and drops it.

mod common;

use common::*;
use dbee_testkit::TestDb;
use egui::{Rect, pos2, vec2};
use serde_json::{Value as Json, json};

/// The page's sections, top to bottom: their headings' ids and words.
const SECTIONS: [(&str, &str); 5] = [
    ("structure-columns", "Columns"),
    ("structure-indexes", "Indexes"),
    ("structure-foreign-keys", "Foreign keys"),
    ("structure-referenced-by", "Referenced by"),
    ("structure-triggers", "Triggers"),
];

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0
}

impl Ui {
    /// Where `id` was laid out in the last frame, in view or not (its tag's rect).
    fn laid(&mut self, id: &str) -> Rect {
        let tag = self.tag(id).unwrap_or_else(|| panic!("`{id}` was drawn"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        Rect::from_min_size(pos2(f(0), f(1)), vec2(f(2), f(3)))
    }

    /// The value `id` was drawn with in the last frame, read from the window itself (a control
    /// request would run frames of its own).
    fn drawn_value(&self, id: &str) -> Option<Json> {
        self.harness.state().tags().value(id).cloned()
    }

    /// Whether `id` was drawn in view in the last frame, read from the window itself.
    fn shows(&self, id: &str) -> bool {
        self.harness.state().tags().rect(id).is_some()
    }
}

/// The tab's page has loaded, with rows.
fn loaded(ui: &mut Ui) -> bool {
    let t = ui.table();
    t["loading"].is_null() && t["rows"].as_u64().is_some_and(|n| n > 0)
}

/// Whether `id` was drawn enabled in the last frame.
fn enabled(ui: &mut Ui, id: &str) -> bool {
    let e = ui.tag(id).unwrap_or_else(|| panic!("`{id}` is drawn"));
    e.get("enabled") != Some(&json!(false))
}

/// The pointer rests on `id`: whether its hover says `words`.
fn hover_says(ui: &mut Ui, id: &str, words: &str) -> bool {
    let at = ui.drawn(id).center();
    ui.rest(at);
    ui.texts().iter().any(|(_, g)| g.text() == words)
}

/// The value panel shut, or open.
fn panel(ui: &mut Ui, on: bool) {
    if ui.execute("ui.toggleValuePanel", json!({}))["visible"] != json!(on) {
        ui.execute("ui.toggleValuePanel", json!({}));
    }
    ui.frame();
}

/// The colours of the shapes other than text painted inside `within`.
fn marks_in(ui: &Ui, within: Rect) -> Vec<egui::Color32> {
    let mut out = Vec::new();
    for (_, s) in ui.shapes() {
        if !within.contains_rect(s.visual_bounding_rect()) {
            continue;
        }
        match s {
            egui::Shape::Path(p) => {
                out.push(p.fill);
                if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                    out.push(c);
                }
            }
            egui::Shape::Circle(c) => out.extend([c.fill, c.stroke.color]),
            egui::Shape::LineSegment { stroke, .. } => out.push(stroke.color),
            egui::Shape::Mesh(m) => out.extend(m.vertices.iter().map(|v| v.color)),
            _ => {}
        }
    }
    out
}

/// The active table tab's page load count (`seq`), read from the window itself.
fn seq(ui: &Ui) -> u64 {
    match ui.harness.state().session().state().active_tab() {
        Some(dbee_engine::Tab::Table(t)) => t.seq,
        _ => panic!("a table tab is active"),
    }
}

/// `shop.orders` open on pg14 at 1100 × 720, its page loaded, then STRUCTURE clicked and its
/// read landed.
fn orders_structure(db: &TestDb) -> Ui {
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    ui.click("table-view-structure");
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    ui.hover_away();
    ui
}

/// SA: the bar's structure state over the page's five sections, COLUMNS filled as the engine
/// gave it, the headers' titles where SA draws them.
#[test]
fn structure_shows_sa() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = orders_structure(db);
    let t = dbee_ui_egui::theme::LIGHT;
    assert_eq!(ui.value("table-view-structure"), true);
    assert_eq!(ui.value("table-view-data"), false);
    assert!(enabled(&mut ui, "table-view-structure"));
    for id in ["filter-0-column", "page-first", "row-add", "table-import", "table-export", "table-note", "value-toggle", "grid-header-0"] {
        assert!(ui.tag(id).is_none(), "no `{id}` in STRUCTURE");
    }
    let facts = ui.value("structure-facts").as_str().unwrap_or_default().to_string();
    assert!(facts.starts_with("9 columns  ") && facts.ends_with(" rows"), "the facts: {facts}");
    assert!(ui.tag("table-loading").is_none(), "the facts are not LOADING");

    // COLUMNS' heading right under the bar, its words 12 in from the body's left.
    let body = ui.tab_x();
    let data = ui.drawn("table-view-data");
    let bar = painted_at(&ui, pos2(body.min + 2.0, data.center().y)).map(|(_, r)| r).expect("the bar's ground");
    assert_eq!(ui.value("structure-columns"), "Columns");
    let heading = ui.drawn("structure-columns");
    assert!(close(heading.top(), bar.bottom()), "COLUMNS starts at the bar's bottom {}: {heading:?}", bar.bottom());
    let words = ui.text_in(heading, "COLUMNS").map(|(r, _)| r).expect("COLUMNS painted in capitals");
    assert!(close(words.left(), body.min + 12.0), "COLUMNS 12 in from {}: {words:?}", body.min);

    // The cells, as the engine gave them.
    assert_eq!(ui.value("structure-columns-0-position"), 1);
    assert_eq!(ui.value("structure-columns-0-name"), "id");
    // The type as the tree and the grid's header write it (`ColumnInfo::label`): Postgres's own
    // words for `int8`.
    assert_eq!(ui.value("structure-columns-0-type"), "bigint");
    let default = ui.value("structure-columns-0-default").as_str().unwrap_or_default().to_string();
    assert!(default.starts_with("nextval("), "id's default as written: {default}");
    assert_eq!(ui.value("structure-columns-0-nullable"), false);
    assert_eq!(ui.value("structure-columns-0-keys"), "primary key");
    let key = ui.drawn("structure-columns-0-keys");
    assert!(marks_in(&ui, key).contains(&t.key), "the primary key's icon in `key`");
    assert_eq!(ui.value("structure-columns-1-name"), "customer_id");
    assert_eq!(ui.value("structure-columns-1-keys"), "shop.customers.id");
    assert!(!marks_in(&ui, ui.drawn("structure-columns-1-keys")).contains(&t.key), "a foreign key's icon is grey");
    assert_eq!(ui.value("structure-columns-2-name"), "dealer_id");
    assert_eq!(ui.value("structure-columns-2-nullable"), true);
    assert_eq!(ui.value("structure-columns-8-position"), 9);
    assert!(ui.tag("structure-columns-9-name").is_none(), "nine columns");

    // The header's titles where SA draws them.
    let header = Rect::from_x_y_ranges(body, heading.bottom()..=heading.bottom() + 30.0);
    for (title, x) in [("Name", 48.0), ("Type", 198.0), ("Nullable", 338.0), ("Default", 414.0), ("Keys", 564.0)] {
        let at = ui.text_in(header, title).map(|(r, _)| r).unwrap_or_else(|| panic!("`{title}` in COLUMNS' header"));
        assert!(close(at.left() - body.min, x), "`{title}` at {x} from the body's left: {}", at.left() - body.min);
    }

    // The five headings in order, each 12 under the last line of the section above it.
    let mut bottom = None;
    for (id, words) in SECTIONS {
        assert_eq!(ui.value(id), words);
        let at = ui.laid(id);
        if let Some(above) = bottom {
            assert!(close(at.top(), above + 12.0), "`{id}` 12 under the section above's last line {above}: {at:?}");
        }
        let section = id.trim_start_matches("structure-");
        let cells: Vec<String> = ids(&ui.inspect()).into_iter().filter(|c| c.starts_with(&format!("{id}-"))).collect();
        let last = cells.iter().map(|c| ui.laid(c).bottom()).fold(at.bottom() + 30.0, f32::max);
        assert!(cells.iter().all(|c| c.trim_start_matches(&format!("structure-{section}-")).starts_with(char::is_numeric)), "{cells:?}");
        bottom = Some(last);
    }
}

/// Review Focus 1: STRUCTURE and back keeps DATA whole: the same page (nothing reloaded), the
/// filter row as typed, the staged change, the selection and the value panel.
#[test]
fn structure_and_back_keeps_the_rows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    ui.execute("table.page", json!({ "to": 3 }));
    ui.until("page 3", |ui| loaded(ui) && ui.table()["page"] == 2);
    panel(&mut ui, true);
    ui.click("filter-0-column");
    ui.click("filter-0-column-status");
    ui.click("filter-0-value");
    ui.text("pending");
    ui.execute("cell.edit", json!({ "row": 0, "column": "status", "value": "staged" }));
    ui.execute("selection.set", json!({ "anchor": [1, 3], "focus": [1, 3] }));
    ui.frame();
    let before = ui.table();
    let seq_before = seq(&ui);
    let panel_value = ui.value("value-panel");
    assert_eq!(ui.value("edits-count"), "1 change");
    ui.click("table-view-structure");
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    assert!(ui.tag("value-panel").is_none(), "no value panel in STRUCTURE");
    ui.click("table-view-data");
    ui.frame();
    let after = ui.table();
    for field in ["page", "selection", "filters", "applied", "rows", "loading", "edits"] {
        assert_eq!(after[field], before[field], "`{field}` as before: {after}");
    }
    assert_eq!(seq(&ui), seq_before, "no page reloaded");
    assert_eq!(ui.value("filter-0-value"), "pending", "the typed filter row");
    assert_eq!(ui.value("edits-count"), "1 change");
    assert_eq!(ui.value("value-panel"), panel_value, "the value panel as before");
}

/// A scratch table of 150 rows; a window whose frames are 50 ms apart on egui's clock, the value
/// panel closed, the table open; and a lock on it that holds the page load REFRESH sends, so a
/// structure read waits its turn behind it.
fn held(db: &TestDb) -> (Scratch, Ui, dbee_testkit::Held) {
    let table = Scratch::new(db, "t_struct", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text, b text); INSERT INTO scratch.{t} SELECT g, 'a' || g, 'b' || g FROM generate_series(1, 150) g;"
        )
    });
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), ..Setup::default() });
    panel(&mut ui, false);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    drop(ui.send("engine.execute", json!({ "command": "table.refresh", "params": {}, "wait": false })));
    ui.until("the held page load", |ui| !ui.table()["loading"].is_null());
    (table, ui, lock)
}

/// Whether the active table tab's structure read runs, read from the window itself.
fn reading(ui: &mut Ui) -> bool {
    match ui.harness.state().session().state().active_tab() {
        Some(dbee_engine::Tab::Table(t)) => t.structure.loading.is_some(),
        _ => false,
    }
}

/// LC on the page: nothing waits on screen before 300 ms; then CANCEL, LOADING and COLUMNS'
/// heading and header over ten rows of bars. CANCEL leaves a first read's page saying so.
#[test]
fn the_first_read_shows_the_skeleton_after_300_ms() {
    let db = dbee_testkit::require_testdb!();
    let (_table, mut ui, lock) = held(db);
    let t = dbee_ui_egui::theme::LIGHT;
    drop(ui.send("engine.execute", json!({ "command": "table.view", "params": { "view": "structure" }, "wait": false })));
    let mut seen = false;
    ui.frames_while(reading, 0.4, |ui, gone| {
        if gone <= 0.25 {
            assert!(ui.shows("table-refresh"), "at {gone:.2} s: REFRESH");
            for id in ["table-cancel", "table-loading", "structure-skeleton", "structure-empty"] {
                assert!(!ui.shows(id), "at {gone:.2} s: no `{id}` yet");
            }
        }
        if gone >= 0.35 {
            assert!(ui.shows("table-cancel") && ui.shows("table-loading") && !ui.shows("table-refresh"), "at {gone:.2} s: CANCEL and LOADING");
            assert_eq!(ui.drawn_value("structure-skeleton"), Some(json!({ "rows": 10 })), "at {gone:.2} s");
            let heading = ui.drawn("structure-columns");
            let rows = ui.drawn("structure-skeleton");
            assert!(close(rows.top(), heading.bottom() + 30.0), "at {gone:.2} s: the bars under COLUMNS' header: {rows:?}");
            assert_eq!(colour_at(ui, pos2(rows.left() + 2.0, rows.top() - 0.5)), Some(t.ink), "at {gone:.2} s: the header's ink line");
            for id in ["structure-indexes", "structure-columns-0-name"] {
                assert!(!ui.shows(id), "at {gone:.2} s: nothing else: `{id}`");
            }
            seen = true;
        }
    });
    assert!(seen, "the skeleton showed");
    ui.click("table-cancel");
    ui.until("the read's end", |ui| ui.table()["structure"]["loading"].is_null());
    ui.frame();
    assert_eq!(ui.value("structure-empty"), "The structure is not loaded.");
    let empty = ui.drawn("structure-empty");
    let (words, _) = ui.text_in(empty, "The structure is not loaded.").expect("the words painted");
    let body = ui.tab_x();
    let data = ui.drawn("table-view-data");
    let bar = painted_at(&ui, pos2(body.min + 2.0, data.center().y)).map(|(_, r)| r).expect("the bar's ground");
    assert!(close(words.left(), body.min + 12.0), "12 in: {words:?}");
    assert!(close(empty.top(), bar.bottom() + 12.0), "12 down from the bar {}: {empty:?}", bar.bottom());
    let colours = text_colours(&ui.shapes().into_iter().filter(|(_, s)| empty.contains_rect(s.visual_bounding_rect())).collect::<Vec<_>>());
    assert!(!colours.is_empty() && colours.iter().all(|c| *c == t.text_muted), "in `text_muted`: {colours:?}");
    assert!(ui.tag("structure-error").is_none(), "a cancel is no error");
    drop(lock);
}

/// A read on a lost connection: the band under the bar, the full width, with LOST's sentence;
/// REFRESH off, saying why.
#[test]
fn a_failed_read_shows_the_band() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, exclusive: true, ..Setup::default() });
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(db.server("pg14").unwrap(), sql).unwrap();
    ui.click("table-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.click("table-view-structure");
    ui.frame();
    let lost = dbee_engine::cmd::LOST;
    assert_eq!(ui.value("structure-error"), lost);
    let band = ui.drawn("structure-error");
    let body = ui.tab_x();
    let data = ui.drawn("table-view-data");
    let bar = painted_at(&ui, pos2(body.min + 2.0, data.center().y)).map(|(_, r)| r).expect("the bar's ground");
    assert!(close(band.top(), bar.bottom()), "the band under the bar {}: {band:?}", bar.bottom());
    assert!(close(band.left(), body.min) && close(band.right(), body.max), "the band the body's width {body:?}: {band:?}");
    assert!(ui.text_in(band, "The structure did not load").is_some(), "the band's heading");
    assert!(!enabled(&mut ui, "table-refresh"), "REFRESH off");
    assert!(hover_says(&mut ui, "table-refresh", &format!("Refresh: not now, {lost}")), "REFRESH says why");
    // DATA keeps its own.
    ui.click("table-view-data");
    ui.frame();
    assert!(ui.tag("structure-error").is_none() && ui.tag("grid-header-0").is_some(), "DATA's rows, not the structure's band");
}

/// Brief §6: the grid's keys don't act on a selection out of sight.
#[test]
fn grid_keys_do_nothing_in_structure() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    ui.click("grid-cell-1-1");
    ui.click("table-view-structure");
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    let copies = ui.copied.len();
    let before = ui.table();
    ui.key("Delete");
    ui.ctrl("C");
    ui.ctrl_shift("C");
    ui.ctrl("A");
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.copied.len(), copies, "nothing copied: {:?}", ui.copied.last());
    let t = ui.table();
    assert_eq!(t["edits"], before["edits"], "nothing staged: {t}");
    assert_eq!(t["selection"], before["selection"], "the selection as it was");
    assert!(ui.toast().is_none(), "no notice");
}

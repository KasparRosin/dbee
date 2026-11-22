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

/// The window for `shop.orders`' page: narrower than the page's widest table (TRIGGERS, 910), which
/// leaves COLUMNS 114 past its least widths: too little for Postgres's `nextval(…)`, so Default
/// still cuts it. REFERENCED BY is below the page's bottom.
const ORDERS_SIZE: egui::Vec2 = APP_SIZE;

/// `shop.orders` open on pg14 at [`ORDERS_SIZE`], its page loaded, then STRUCTURE clicked and its
/// read landed.
fn orders_structure(db: &TestDb) -> Ui {
    let mut ui = Ui::connected_in(db, ORDERS_SIZE);
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

    // The header's titles where SA draws them: Name and Nullable as drawn; the page's 114 past
    // COLUMNS' least widths go to the columns whose content is cut, and Default still cuts
    // Postgres's `nextval(…)`, whole on hover; Keys after it, in view.
    let cell = ui.drawn("structure-columns-0-default");
    let default = cell.width();
    let kind = ui.drawn("structure-columns-0-type").width();
    // The page is as wide as its widest table, TRIGGERS' 910: wider than the body here.
    assert!(body.max - body.min < 910.0, "the body is narrower than TRIGGERS: {body:?}");
    let spare = 910.0 - 796.0;
    assert!(kind >= 140.0 - 0.5 && default > 150.5, "Type and Default take the spare room: {kind}, {default}");
    assert!(close(kind + default, 140.0 + 150.0 + spare), "all of it, and no more: {kind} + {default}, {spare} spare");
    let words = ui.value("structure-columns-0-default").as_str().unwrap_or_default().to_string();
    let (_, cut) = ui.text_in(cell, &words).expect("the default painted");
    assert!(cut, "a long default is cut with …");
    ui.rest(cell.center());
    assert!(ui.texts().into_iter().any(|(r, g)| g.text() == words && !g.elided && !cell.contains_rect(r)), "the whole default on hover");
    ui.hover_away();
    assert!(ui.shows("structure-columns-0-keys"), "Keys in view at {ORDERS_SIZE:?}");
    let header = Rect::from_x_y_ranges(body, heading.bottom()..=heading.bottom() + 30.0);
    for (title, x) in [("Name", 48.0), ("Type", 198.0), ("Nullable", 198.0 + kind), ("Default", 274.0 + kind), ("Keys", 274.0 + kind + default)] {
        let at = ui.text_in(header, title).map(|(r, _)| r).unwrap_or_else(|| panic!("`{title}` in COLUMNS' header"));
        assert!(close(at.left() - body.min, x), "`{title}` at {x} from the body's left: {}", at.left() - body.min);
    }

    // The five headings in order, each 12 under the last line of the section above it.
    let shown = ui.table()["structure"]["shown"].clone();
    let mut bottom = None;
    for ((id, words), count) in SECTIONS.into_iter().zip(["columns", "indexes", "foreignKeys", "referencedBy", "triggers"]) {
        assert_eq!(ui.value(id), words);
        let at = ui.laid(id);
        if let Some(above) = bottom {
            assert!(close(at.top(), above + 12.0), "`{id}` 12 under the section above's last line {above}: {at:?}");
        }
        let section = id.trim_start_matches("structure-");
        let cells: Vec<String> = ids(&ui.inspect()).into_iter().filter(|c| c.starts_with(&format!("{id}-")) && !c.contains("-edge-")).collect();
        let drawn = cells.iter().map(|c| ui.laid(c).bottom()).fold(at.bottom() + 30.0, f32::max);
        assert!(cells.iter().all(|c| c.trim_start_matches(&format!("structure-{section}-")).starts_with(char::is_numeric)), "{cells:?}");
        // Its last row's line, drawn or below the page's bottom.
        let rows = shown[count].as_u64().unwrap_or_else(|| panic!("{count} counted: {shown}"));
        let last = at.bottom() + 30.0 + 26.0 * rows as f32;
        assert!(drawn <= last + 0.5, "`{id}`'s rows end at {last}: {drawn}");
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

/// A schema of its own on pg14 for a test's tables (`st_<hex>`), dropped at the end, also when
/// the test fails.
struct SchemaGuard(String);

impl SchemaGuard {
    /// Makes the schema and runs `sql(<schema>)` in it.
    fn new(db: &TestDb, sql: impl FnOnce(&str) -> String) -> SchemaGuard {
        let name = dbee_testkit::unique("st");
        let guard = SchemaGuard(name.clone());
        dbee_testkit::psql(pg14_server(db), &format!("CREATE SCHEMA {name}; {}", sql(&name))).unwrap();
        guard
    }
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        if let Some(db) = dbee_testkit::testdb() {
            let _ = dbee_testkit::psql(pg14_server(db), &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.0));
        }
    }
}

/// The brief §9's scratch tables: `p` with a unique pair, an expression index, a partial index
/// with a descending part and two triggers; `c` with a one-column and a two-column foreign key
/// to `p`.
fn st_tables(s: &str) -> String {
    format!(
        "CREATE TABLE {s}.p (id int8 PRIMARY KEY, email text, status text, total numeric, a int, b int, UNIQUE (a, b));
         CREATE INDEX p_lower ON {s}.p (lower(email));
         CREATE INDEX p_part ON {s}.p (status, total DESC) WHERE total > 100;
         CREATE TABLE {s}.c (id int PRIMARY KEY, p_id int8 REFERENCES {s}.p ON DELETE CASCADE, a int, b int, FOREIGN KEY (a, b) REFERENCES {s}.p (a, b));
         CREATE FUNCTION {s}.f() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$;
         CREATE TRIGGER t_touch BEFORE UPDATE ON {s}.p FOR EACH ROW EXECUTE FUNCTION {s}.f();
         CREATE TRIGGER t_log AFTER INSERT OR DELETE ON {s}.p FOR EACH STATEMENT EXECUTE FUNCTION {s}.f();"
    )
}

/// `schema.table` open, its STRUCTURE shown and read.
fn structure_of(ui: &mut Ui, schema: &str, table: &str) {
    ui.open(schema, table);
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    ui.click("table-view-structure");
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    ui.hover_away();
}

/// The row of `section` whose `key` reads `value`: its index.
fn row_where(ui: &mut Ui, section: &str, key: &str, value: &str) -> usize {
    (0..64)
        .find(|n| ui.tag(&format!("structure-{section}-{n}-{key}")).is_some_and(|t| t["value"] == value))
        .unwrap_or_else(|| panic!("a row of {section} with {key} `{value}`"))
}

/// Brief §3.2–§3.5: each section's cells as the brief writes them, data as Postgres wrote it.
#[test]
fn the_sections_read_as_the_brief_writes_them() {
    let db = dbee_testkit::require_testdb!();
    let schema = SchemaGuard::new(db, st_tables);
    let s = schema.0.as_str();
    // Tall enough for every section of `p` in view.
    let mut ui = Ui::connected_in(db, vec2(1400.0, 1200.0));
    structure_of(&mut ui, s, "p");
    // INDEXES: the primary key's first.
    assert_eq!(ui.value("structure-indexes-0-name"), "p_pkey");
    assert_eq!(ui.value("structure-indexes-0-keys"), "primary key");
    assert_eq!(ui.value("structure-indexes-0-unique"), true);
    assert_eq!(ui.value("structure-indexes-0-method"), "btree");
    assert_eq!(ui.value("structure-indexes-0-columns"), "id");
    let pair = row_where(&mut ui, "indexes", "name", "p_a_b_key");
    assert_eq!(ui.value(&format!("structure-indexes-{pair}-columns")), "a, b");
    assert_eq!(ui.value(&format!("structure-indexes-{pair}-unique")), true);
    assert_eq!(ui.value(&format!("structure-indexes-{pair}-keys")), "");
    let lower = row_where(&mut ui, "indexes", "name", "p_lower");
    assert_eq!(ui.value(&format!("structure-indexes-{lower}-columns")), "lower(email)");
    assert_eq!(ui.value(&format!("structure-indexes-{lower}-unique")), false);
    let part = row_where(&mut ui, "indexes", "name", "p_part");
    let words = ui.value(&format!("structure-indexes-{part}-columns")).as_str().unwrap_or_default().to_string();
    let (columns, predicate) = words.split_once(" where ").unwrap_or_else(|| panic!("a partial index's predicate: {words}"));
    assert_eq!(columns, "status, total desc");
    assert!(predicate.contains("total") && predicate.contains("100"), "Postgres's own predicate: {predicate}");
    // REFERENCED BY: c's two keys, from c's columns, to p's.
    let one = row_where(&mut ui, "referenced-by", "from", &format!("{s}.c.p_id"));
    assert_eq!(ui.value(&format!("structure-referenced-by-{one}-columns")), "id");
    let two = row_where(&mut ui, "referenced-by", "from", &format!("{s}.c (a, b)"));
    assert_eq!(ui.value(&format!("structure-referenced-by-{two}-columns")), "a, b");
    // TRIGGERS, by name.
    assert_eq!(ui.value("structure-triggers-0-name"), "t_log");
    assert_eq!(ui.value("structure-triggers-0-when"), "after");
    assert_eq!(ui.value("structure-triggers-0-events"), "insert, delete");
    assert_eq!(ui.value("structure-triggers-0-forEach"), "statement");
    assert_eq!(ui.value("structure-triggers-1-name"), "t_touch");
    assert_eq!(ui.value("structure-triggers-1-when"), "before");
    assert_eq!(ui.value("structure-triggers-1-events"), "update");
    assert_eq!(ui.value("structure-triggers-1-forEach"), "row");
    let definition = ui.value("structure-triggers-1-definition").as_str().unwrap_or_default().to_string();
    assert!(definition.starts_with("CREATE TRIGGER t_touch BEFORE UPDATE"), "the definition as written: {definition}");
    // FOREIGN KEYS on c.
    structure_of(&mut ui, s, "c");
    let one = row_where(&mut ui, "foreign-keys", "references", &format!("{s}.p.id"));
    assert_eq!(ui.value(&format!("structure-foreign-keys-{one}-columns")), "p_id");
    assert_eq!(ui.value(&format!("structure-foreign-keys-{one}-onDelete")), "cascade");
    let two = row_where(&mut ui, "foreign-keys", "references", &format!("{s}.p (a, b)"));
    assert_eq!(ui.value(&format!("structure-foreign-keys-{two}-columns")), "a, b");
    assert_eq!(ui.value(&format!("structure-foreign-keys-{two}-onDelete")), "no action");
    assert_eq!(ui.value("structure-foreign-keys-0-name"), "c_a_b_fkey", "the catalog's order, by name");
}

/// Review Focus 3: a trigger whose definition runs to thousands of lines (SQLite keeps the
/// statement as written) draws on one line cut inside its cell, its hover at most 40 lines ending
/// with `…`, and frames stay quick with the page open.
#[test]
fn a_huge_trigger_definition_draws_on_one_line() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite copy");
    let body: String = (0..2000).map(|n| format!("  UPDATE t_big SET a = a + {n} WHERE id = new.id;\n")).collect();
    sqlite
        .sql(&format!("CREATE TABLE t_big (id INTEGER PRIMARY KEY, a int); CREATE TRIGGER t_huge AFTER INSERT ON t_big BEGIN\n{body}END;"))
        .unwrap();
    // Wide enough for TRIGGERS' Definition to start in the body (the page scrolls sideways later).
    let mut ui = connected_to_target(db, &sqlite, None, vec2(1400.0, 900.0));
    structure_of(&mut ui, "main", "t_big");
    assert_eq!(ui.value("structure-triggers-0-name"), "t_huge");
    let line = ui.value("structure-triggers-0-definition").as_str().unwrap_or_default().to_string();
    assert!(line.starts_with("CREATE TRIGGER t_huge AFTER INSERT ON t_big BEGIN UPDATE t_big SET a = a + 0 WHERE"), "on one line: {}", &line[..120]);
    assert!(!line.contains('\n') && line.len() < 4096, "its one line is kept short: {} bytes", line.len());
    let cell = ui.drawn("structure-triggers-0-definition");
    let painted: Vec<_> = ui.texts().into_iter().filter(|(r, g)| cell.contains_rect(*r) && g.text().starts_with("CREATE TRIGGER")).collect();
    let [(at, galley)] = painted.as_slice() else { panic!("one text in the cell: {painted:?}") };
    assert!(galley.elided && galley.rows.len() == 1, "one line, cut");
    assert!(at.right() <= cell.right() - 8.0 + 0.5, "cut inside the cell: {at:?} in {cell:?}");
    // The hover: the definition as written, its first 40 lines, then `…`.
    ui.rest(cell.center());
    let hover =
        ui.texts().into_iter().map(|(_, g)| g.text().to_string()).find(|t| t.starts_with("CREATE TRIGGER t_huge AFTER INSERT ON t_big BEGIN\n"));
    let hover = hover.expect("the hover shows the definition as written");
    assert_eq!(hover.lines().count(), 40, "40 lines: {hover}");
    assert!(hover.ends_with('…') && hover.len() <= 4096 + '…'.len_utf8(), "cut with `…`, at most 4 KiB");
    ui.hover_away();
    for _ in 0..10 {
        let start = std::time::Instant::now();
        ui.frame();
        assert!(start.elapsed() < std::time::Duration::from_millis(200), "a frame with the page open: {:?}", start.elapsed());
    }
}

/// Review B3 M1: on SQLite a key whose REFERENCES spells the table in another case still shows
/// its ON DELETE, as `table.structure` gives it (the catalog names the table its own way).
#[test]
fn on_delete_shows_when_references_spells_the_table_in_another_case() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite copy");
    sqlite
        .sql("CREATE TABLE customers_c (id INTEGER PRIMARY KEY); CREATE TABLE orders_c (id INTEGER PRIMARY KEY, customer_id int REFERENCES Customers_C(id) ON DELETE CASCADE);")
        .unwrap();
    let mut ui = connected_to_target(db, &sqlite, None, vec2(1400.0, 900.0));
    structure_of(&mut ui, "main", "orders_c");
    assert_eq!(ui.value("structure-foreign-keys-0-references"), "main.customers_c.id");
    assert_eq!(ui.value("structure-foreign-keys-0-onDelete"), "cascade");
}

/// Review B3 M2, brief §3.4: a link to the tab's own table (a key that references it) shows that
/// tab's DATA, opening no other tab.
#[test]
fn a_link_to_its_own_table_shows_data() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite copy");
    sqlite.sql("CREATE TABLE staff (id INTEGER PRIMARY KEY, manager_id int REFERENCES staff(id));").unwrap();
    let mut ui = connected_to_target(db, &sqlite, None, vec2(1400.0, 900.0));
    structure_of(&mut ui, "main", "staff");
    let tabs = ui.document()["tabs"].as_array().map(Vec::len);
    for link in ["structure-foreign-keys-0-references", "structure-referenced-by-0-from", "structure-columns-1-keys"] {
        assert_eq!(ui.value("table-view-structure"), true, "{link}: in STRUCTURE");
        ui.click(link);
        ui.until("DATA", |ui| ui.value("table-view-data") == true);
        assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), tabs, "{link}: no other tab");
        ui.click("table-view-structure");
        ui.until("the structure", |ui| ui.tag(link).is_some());
        ui.hover_away();
    }
}

/// Review B3 M4: a cut default thousands of bytes long reads on hover as a definition does: as
/// written, at most 4 KiB, ending with `…`.
#[test]
fn a_huge_default_hovers_at_most_4_kib() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite copy");
    let long = "x".repeat(7000);
    sqlite.sql(&format!("CREATE TABLE t_def (id INTEGER PRIMARY KEY, note text DEFAULT '{long}');")).unwrap();
    let mut ui = connected_to_target(db, &sqlite, None, vec2(1400.0, 900.0));
    structure_of(&mut ui, "main", "t_def");
    let default = ui.value("structure-columns-1-default").as_str().unwrap_or_default().to_string();
    assert!(default.len() > 4096, "the default as the engine keeps it: {} bytes", default.len());
    let cell = ui.drawn("structure-columns-1-default");
    ui.rest(cell.center());
    let hover = ui
        .texts()
        .into_iter()
        .find(|(r, g)| g.text().starts_with("'xxxx") && !cell.contains_rect(*r))
        .map(|(_, g)| g.text().to_string())
        .expect("the hover");
    assert!(hover.ends_with('…') && hover.len() <= 4096 + '…'.len_utf8(), "at most 4 KiB, cut with `…`: {} bytes", hover.len());
}

/// Brief §3.7: a foreign key's link opens its table, by a click or by Enter with the keyboard;
/// an open tab is reused.
#[test]
fn a_link_opens_its_table() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = orders_structure(db);
    let orders = ui.document()["active"].clone();
    let t = dbee_ui_egui::theme::LIGHT;
    // At rest grey; under the pointer `text`, with the pointing hand.
    let link = ui.drawn("structure-columns-1-keys");
    assert!(ui.text_in(link, "shop.customers.id").is_some(), "the link's words");
    ui.hover(link.center());
    ui.frame();
    let colours = text_colours(&ui.shapes().into_iter().filter(|(_, s)| link.contains_rect(s.visual_bounding_rect())).collect::<Vec<_>>());
    assert!(!colours.is_empty() && colours.iter().all(|c| *c == t.text), "hovered words in `text`: {colours:?}");
    assert_eq!(ui.harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    ui.click("structure-columns-1-keys");
    ui.until("customers", |ui| ui.table()["name"] == "customers");
    let customers = ui.document()["active"].clone();
    assert_ne!(customers, orders, "a tab of its own");
    assert_eq!(ui.table()["view"], "data");
    let tabs = ui.document()["tabs"].as_array().map_or(0, Vec::len);
    // Back on orders, still in STRUCTURE: Enter on the foreign key's References.
    ui.execute("tab.activate", json!({ "tab": orders }));
    ui.until("the structure", |ui| ui.tag("structure-foreign-keys-0-references").is_some());
    assert_eq!(ui.table()["view"], "structure");
    assert_eq!(ui.value("structure-foreign-keys-0-references"), "shop.customers.id");
    ui.focus("structure-foreign-keys-0-references");
    ui.key("Enter");
    ui.until("customers again", |ui| ui.table()["name"] == "customers");
    assert_eq!(ui.document()["active"], customers, "the same tab");
    assert_eq!(ui.document()["tabs"].as_array().map_or(0, Vec::len), tabs, "no second one");
    // A link below the page's bottom is listed with its words, and a click scrolls to it first.
    ui.execute("tab.activate", json!({ "tab": orders }));
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    let from = ui.tag("structure-referenced-by-0-from").expect("REFERENCED BY's link listed");
    assert_eq!((from["visible"].clone(), from["value"].clone()), (json!(false), json!("shop.order_items.order_id")), "out of view: {from}");
    ui.click("structure-referenced-by-0-from");
    ui.until("order_items", |ui| ui.table()["name"] == "order_items");
}

/// Brief §3.7: while the connection is lost the links are off and say why.
#[test]
fn links_are_off_on_a_lost_connection() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: ORDERS_SIZE, exclusive: true, ..Setup::default() });
    structure_of(&mut ui, "shop", "orders");
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(db.server("pg14").unwrap(), sql).unwrap();
    ui.click("table-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.frame();
    assert!(!enabled(&mut ui, "structure-columns-1-keys"), "the link off");
    let t = dbee_ui_egui::theme::LIGHT;
    let link = ui.drawn("structure-columns-1-keys");
    let colours = text_colours(&ui.shapes().into_iter().filter(|(_, s)| link.contains_rect(s.visual_bounding_rect())).collect::<Vec<_>>());
    assert!(!colours.is_empty() && colours.iter().all(|c| *c == t.text_faint), "off in `text_faint`: {colours:?}");
    let lost = dbee_engine::cmd::LOST;
    assert!(hover_says(&mut ui, "structure-columns-1-keys", &format!("Can't open shop.customers now: {lost}")), "it says why");
    drop(ui.call("ui.click", json!({ "id": "structure-columns-1-keys" })));
    ui.frame();
    assert_eq!(ui.table()["name"], "orders", "nothing opened");
}

/// Brief §3.6: a view's empty sections keep their heading and header, and say nothing.
#[test]
fn empty_sections_keep_their_header() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    structure_of(&mut ui, "shop", "order_totals");
    for section in ["indexes", "foreign-keys", "referenced-by", "triggers"] {
        assert!(ui.tag(&format!("structure-{section}-0-name")).is_none(), "no rows in {section}");
    }
    let heading = ui.laid("structure-triggers");
    let header = Rect::from_x_y_ranges(ui.tab_x(), heading.bottom()..=heading.bottom() + 30.0);
    for title in ["Name", "When", "Events", "For each", "Definition"] {
        assert!(ui.text_in(header, title).is_some(), "TRIGGERS' header `{title}`");
    }
    // Three rows' room under its header (above the status bar at this size).
    let below = Rect::from_x_y_ranges(ui.tab_x(), heading.bottom() + 31.0..=heading.bottom() + 30.0 + 3.0 * 26.0);
    assert!(below.bottom() < ui.drawn("status-connection").top(), "the room checked is above the status bar");
    let words: Vec<String> = ui.texts().into_iter().filter(|(r, _)| below.intersects(*r)).map(|(_, g)| g.text().to_string()).collect();
    assert!(words.is_empty(), "no phrase under an empty section: {words:?}");
}

/// Spec §7: names, types and indexes are drawn as written, never in capitals.
#[test]
fn data_is_never_uppercased() {
    let db = dbee_testkit::require_testdb!();
    let schema = SchemaGuard::new(db, |s| {
        format!(
            "CREATE TYPE {s}.\"Mood\" AS ENUM ('ok'); CREATE TABLE {s}.\"Mixed Case\" (\"Odd Name\" {s}.\"Mood\"); CREATE INDEX \"Idx Upper\" ON {s}.\"Mixed Case\" (\"Odd Name\");"
        )
    });
    let s = schema.0.as_str();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    structure_of(&mut ui, s, "Mixed Case");
    let ty = ui.value("structure-columns-0-type").as_str().unwrap_or_default().to_string();
    assert!(ty.contains("Mood") && !ty.contains("MOOD"), "the type as written: {ty}");
    for (id, words) in
        [("structure-columns-0-name", "Odd Name"), ("structure-indexes-0-name", "Idx Upper"), ("structure-indexes-0-columns", "Odd Name")]
    {
        assert_eq!(ui.value(id), words);
        let at = ui.drawn(id);
        assert!(ui.text_in(at, words).is_some(), "`{words}` painted as written in `{id}`");
    }
}

/// Brief §2.2, call 6: a section's Name column is at least `max(drawn, widest name + 16)`, up to
/// twice its drawn width; in a body no wider than the tables that is its width, and past it the
/// name is cut with `…` and reads whole on hover. In a body with room to spare the name shows
/// whole.
#[test]
fn long_names_widen_their_column_up_to_twice() {
    let db = dbee_testkit::require_testdb!();
    let long = "a_column_name_of_sixty_characters_that_is_far_too_long_for_it";
    let long = &long[..60];
    let mid = "twenty_chars_name_ab";
    assert_eq!((long.len(), mid.len()), (60, 20));
    let schema = SchemaGuard::new(db, |s| {
        format!("CREATE TABLE {s}.long ({long} int); CREATE TABLE {s}.mid ({mid} int); CREATE TABLE {s}.short (a_short_name int);")
    });
    let s = schema.0.as_str();
    // A body narrower than the tables: every column at its least width.
    let mut ui = Ui::connected_in(db, vec2(900.0, 720.0));
    let body = ui.tab_x();
    assert!(body.max - body.min < 796.0, "the body is narrower than COLUMNS: {body:?}");
    // Twice the drawn 150, the name cut inside it, Type right after it.
    structure_of(&mut ui, s, "long");
    assert_eq!(ui.value("structure-columns-0-name"), long);
    let name = ui.drawn("structure-columns-0-name");
    assert!(close(name.width(), 300.0), "Name twice its 150: {name:?}");
    assert!(close(ui.drawn("structure-columns-0-type").left() - body.min, 40.0 + 300.0), "Type after it");
    let (at, cut) = ui.text_in(name, long).expect("the name painted");
    assert!(cut && at.right() <= name.right() - 8.0 + 0.5, "cut with `…` inside its cell: {at:?} in {name:?}");
    ui.rest(name.center());
    let whole = ui.texts().into_iter().any(|(r, g)| g.text() == long && !g.elided && !name.contains_rect(r));
    assert!(whole, "the whole name on hover");
    ui.hover_away();
    // A name a little wider than 150 widens Name to its width and 16, uncut.
    structure_of(&mut ui, s, "mid");
    let name = ui.drawn("structure-columns-0-name");
    let (at, cut) = ui.text_in(name, mid).expect("the name painted");
    assert!(!cut, "whole");
    assert!(at.width() + 16.0 > 150.0 && close(name.width(), at.width() + 16.0), "Name as wide as its name and 16: {name:?}, the words {at:?}");
    // A short one leaves Name at 150.
    structure_of(&mut ui, s, "short");
    assert!(close(ui.drawn("structure-columns-0-name").width(), 150.0), "Name at its drawn 150");
    // With room to spare the long name shows whole, in a column as wide as it needs.
    ui.harness.set_size(vec2(1600.0, 720.0));
    ui.frame();
    structure_of(&mut ui, s, "long");
    let name = ui.drawn("structure-columns-0-name");
    let (at, cut) = ui.text_in(name, long).expect("the name painted");
    assert!(!cut && name.width() > 300.5 && name.width() <= at.width() + 16.0 + 1.0, "the long name whole: {name:?}, the words {at:?}");
}

/// Brief §7: the page keeps its widths and scrolls sideways; the header's `ink` line runs as wide
/// as the widest table (TRIGGERS: 40 + 240 + 110 + 180 + 100 + 240).
#[test]
fn the_page_scrolls_sideways_when_narrow() {
    const WIDEST: f32 = 910.0;
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, vec2(640.0, 420.0));
    structure_of(&mut ui, "shop", "orders");
    let body = ui.tab_x();
    assert!(body.max - body.min < WIDEST, "the body is narrower than the tables: {body:?}");
    assert!(!ui.shows("structure-columns-0-keys"), "Keys out of view at first");
    // The foreign key's link out of view is listed where it is laid out: COLUMNS' Keys runs to
    // the end of the widest table.
    let link = ui.laid("structure-columns-1-keys");
    assert_eq!(ui.tag("structure-columns-1-keys").map(|t| t["visible"].clone()), Some(json!(false)), "out of view");
    assert!(link.right() - body.min >= WIDEST - 0.5, "COLUMNS as wide as the widest table: {link:?} from {}", body.min);
    let at = ui.drawn("structure-columns-0-name").center();
    ui.hover(at);
    ui.frame_with(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-2000.0, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..12 {
        ui.frame();
    }
    assert!(ui.shows("structure-columns-0-keys"), "Keys reached by scrolling right");
    let keys = ui.drawn("structure-columns-0-keys");
    assert!(keys.left() >= body.min - 0.5 && keys.right() <= body.max + 0.5, "Keys inside the body: {keys:?}");
    assert_eq!(ui.value("structure-columns-0-keys"), "primary key");
    // Scrolled to the end: COLUMNS' header line runs to the body's right edge, as wide as the
    // widest table.
    let t = dbee_ui_egui::theme::LIGHT;
    let heading = ui.drawn("structure-columns");
    let y = heading.bottom() + 30.0;
    let lines: Vec<(Rect, Rect)> = ui
        .shapes()
        .into_iter()
        .filter_map(|(clip, s)| match s {
            egui::Shape::Rect(r) if r.fill == t.ink && (r.rect.height() - 1.0).abs() < 0.1 && (r.rect.bottom() - y).abs() <= 0.5 => {
                Some((r.rect.intersect(clip), r.rect))
            }
            _ => None,
        })
        .collect();
    let seen = lines.iter().fold(Rect::NOTHING, |a, (r, _)| a.union(*r));
    let whole = lines.iter().fold(Rect::NOTHING, |a, (_, r)| a.union(*r));
    assert!(close(seen.left(), body.min) && close(seen.right(), body.max), "the ink line across the body {body:?}: {seen:?}");
    assert!(close(whole.right(), body.max), "scrolled to the page's end, where the line ends: {whole:?}");
    assert!(close(heading.left(), body.min) && close(heading.right(), body.max), "the heading across the body: {heading:?}");
}

/// The window widths Review Focus 5 sweeps, from wide to narrow.
const SWEEP: [f32; 8] = [1400.0, 1100.0, 900.0, 640.0, 420.0, 300.0, 200.0, 120.0];

/// Every tagged `table-*` and `structure-*` control drawn lies inside the tab's body, and no two
/// overlap (but a column's resize handle, which lies on the edge between two columns' cells), at
/// each width of [`SWEEP`]; the window back at 1400 × 720 after.
fn sweep_widths(ui: &mut Ui, state: &str) {
    for width in SWEEP {
        ui.harness.set_size(vec2(width, 720.0));
        ui.frame();
        ui.frame();
        let at = format!("{state}, {width} wide");
        let body = ui.tab_x();
        let ids: Vec<String> = ui
            .harness
            .state()
            .tags()
            .ids()
            .into_iter()
            .filter(|id| id.starts_with("table-") || id.starts_with("structure-"))
            .map(str::to_string)
            .collect();
        assert!(ids.iter().any(|id| id == "table-view-structure"), "{at}: the bar drawn");
        let drawn: Vec<(String, Rect)> = ids.into_iter().filter_map(|id| ui.harness.state().tags().rect(&id).map(|r| (id, r))).collect();
        for (id, r) in &drawn {
            assert!(r.left() >= body.min - 0.5 && r.right() <= body.max + 0.5 && r.bottom() <= 720.5, "{at}: `{id}` {r:?} inside the body {body:?}");
        }
        let cells: Vec<&(String, Rect)> = drawn.iter().filter(|(id, _)| !id.contains("-edge-")).collect();
        for (i, (a, ra)) in cells.iter().enumerate() {
            for (b, rb) in &cells[i + 1..] {
                let o = ra.intersect(*rb);
                assert!(!o.is_positive() || o.area() <= 0.5, "{at}: `{a}` {ra:?} over `{b}` {rb:?}");
            }
        }
    }
    ui.harness.set_size(vec2(1400.0, 720.0));
    ui.frame();
}

/// Review Focus 5: from 1400 to 120 wide, the page shown and while a read reloads it (after
/// 300 ms), nothing is drawn over anything else or past the body.
#[test]
fn the_structure_view_never_draws_one_thing_over_another() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_sweep", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a_rather_long_column_name_for_a_narrow_window text, b text);")
    });
    let mut ui = Ui::connected_with(db, Setup { size: vec2(1400.0, 720.0), step: Some(0.05), ..Setup::default() });
    panel(&mut ui, false);
    structure_of(&mut ui, "scratch", &table.0);
    sweep_widths(&mut ui, "shown");
    // A reload held behind a page load that waits for a lock.
    ui.click("table-view-data");
    ui.until("the rows", |ui| ui.tag("grid-header-0").is_some());
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    drop(ui.send("engine.execute", json!({ "command": "table.refresh", "params": {}, "wait": false })));
    ui.until("the held page load", |ui| !ui.table()["loading"].is_null());
    drop(ui.send("engine.execute", json!({ "command": "table.view", "params": { "view": "structure" }, "wait": false })));
    ui.frames_while(reading, 0.4, |_, _| {});
    assert!(ui.shows("table-cancel") && ui.shows("table-loading"), "the read's loading state shows");
    sweep_widths(&mut ui, "reloading");
    assert!(reading(&mut ui), "still reading");
    drop(lock);
}

/// Tab reaches a link scrolled out of view, and the page scrolls to it.
#[test]
fn tab_brings_a_link_out_of_view_into_view() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = orders_structure(db);
    assert!(!ui.shows("structure-referenced-by-0-from"), "REFERENCED BY's link below the page's bottom");
    ui.focus("structure-foreign-keys-0-references");
    ui.key("Tab");
    ui.frame();
    ui.frame();
    assert!(ui.shows("structure-referenced-by-0-from"), "scrolled into view");
    let from = ui.drawn("structure-referenced-by-0-from");
    let status = ui.drawn("status-connection");
    assert!(from.bottom() <= status.top() + 0.5, "whole above the status bar: {from:?}");
    assert!(ui.focused("structure-referenced-by-0-from"), "it has the keyboard");
    ui.key("Enter");
    ui.until("order_items", |ui| ui.table()["name"] == "order_items");
}

/// The pointer pressed at `from`, moved to `to` and let go there: a drag, a frame for each step.
fn drag(ui: &mut Ui, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    ui.frame_with(vec![egui::Event::PointerMoved(from)]);
    ui.frame_with(vec![button(from, true)]);
    ui.frame_with(vec![egui::Event::PointerMoved(to)]);
    ui.frame();
    ui.frame_with(vec![button(to, false)]);
    ui.frame();
}

/// COLUMNS' cells of row 0 after the position, by key: where each is laid out.
fn columns_row(ui: &mut Ui) -> [Rect; 5] {
    ["name", "type", "nullable", "default", "keys"].map(|key| ui.laid(&format!("structure-columns-0-{key}")))
}

/// A page wider than its tables gives the room to the columns whose content is cut: in a 1400
/// wide window `shop.orders`' long default and its longest type show whole, in columns as wide as
/// they need and no wider; the columns with nothing cut keep their drawn widths, the last takes
/// the rest, and every section runs the body's width with no sideways scroll.
#[test]
fn spare_room_shows_cut_columns_whole() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, vec2(1400.0, 720.0));
    structure_of(&mut ui, "shop", "orders");
    let body = ui.tab_x();
    let [name, kind, nullable, default, keys] = columns_row(&mut ui);
    let words = ui.value("structure-columns-0-default").as_str().unwrap_or_default().to_string();
    assert!(words.starts_with("nextval("), "id's default: {words}");
    let (at, cut) = ui.text_in(default, &words).expect("the default painted");
    assert!(!cut, "the default whole in a wide window: {at:?} in {default:?}");
    assert!(default.width() > 150.5, "Default wider than its drawn 150: {default:?}");
    assert!(default.width() <= at.width() + 16.0 + 1.0, "and no wider than its words and their 16: {default:?}, the words {at:?}");
    for row in 0..9 {
        for key in ["name", "type", "default"] {
            let id = format!("structure-columns-{row}-{key}");
            let (cell, words) = (ui.laid(&id), ui.value(&id).as_str().unwrap_or_default().to_string());
            assert!(words.is_empty() || ui.text_in(cell, &words).is_some_and(|(_, cut)| !cut), "`{id}` whole: {words}");
        }
    }
    assert!(close(name.width(), 150.0) && close(nullable.width(), 76.0), "nothing cut in Name and Nullable: as drawn, {name:?} {nullable:?}");
    assert!(kind.width() >= 140.0 - 0.5, "Type at least as drawn: {kind:?}");
    assert!(close(name.left(), body.min + 40.0) && close(keys.right(), body.max), "COLUMNS from the body's left to its right: {body:?}");
    assert!(keys.width() >= 240.0 - 0.5, "Keys takes the rest, at least its 240: {keys:?}");
    for (id, _) in SECTIONS {
        let heading = ui.laid(id);
        assert!(close(heading.left(), body.min) && close(heading.right(), body.max), "`{id}` the body's width {body:?}: {heading:?}");
    }
}

/// A titled column's right edge is a handle, as in the data grid: the resize cursor and the
/// theme's hovered edge under the pointer; a drag moves the edge with the pointer, the other
/// columns keeping their widths and the last taking the rest; a double click fits the column to
/// its content. The widths last through DATA and back and through a reload.
#[test]
fn a_column_is_resized_at_its_edge() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, vec2(1400.0, 720.0));
    structure_of(&mut ui, "shop", "orders");
    let body = ui.tab_x();
    let before = columns_row(&mut ui);
    let words = ui.value("structure-columns-0-default").as_str().unwrap_or_default().to_string();
    // The handle is on Default's right edge, from the header to the last row.
    let edge = ui.drawn("structure-columns-edge-default");
    assert!(close(edge.center().x, before[3].right()), "the handle on Default's right edge: {edge:?}, {:?}", before[3]);
    assert!(ui.tag("structure-columns-edge-position").is_none(), "the position's column has none");
    let at = pos2(edge.center().x, before[3].center().y);
    ui.hover(at);
    ui.frame();
    assert_eq!(ui.harness.output().platform_output.cursor_icon, egui::CursorIcon::ResizeColumn, "the grid's resize cursor on the edge");
    let hovered = ui.harness.ctx.global_style().visuals.widgets.hovered.bg_stroke;
    let line = |ui: &Ui, x: f32, stroke: egui::Stroke| {
        ui.shapes().into_iter().any(
            |(_, s)| matches!(s, egui::Shape::LineSegment { points, stroke: st } if st == stroke && points.iter().all(|p| (p.x - x).abs() <= 0.5)),
        )
    };
    assert!(line(&ui, at.x, hovered), "the hovered edge's line, as the grid's");
    // A drag 120 to the left: Default 120 narrower and cut, whole on hover; Name, Type and
    // Nullable as they were; Keys starts 120 further left and still ends at the body's right.
    drag(&mut ui, at, at - vec2(120.0, 0.0));
    ui.hover_away();
    let after = columns_row(&mut ui);
    assert!(close(after[3].width(), before[3].width() - 120.0), "Default 120 narrower: {:?}, before {:?}", after[3], before[3]);
    for i in 0..3 {
        assert!(close(after[i].width(), before[i].width()) && close(after[i].left(), before[i].left()), "column {i} as it was: {:?}", after[i]);
    }
    assert!(close(after[4].left(), before[4].left() - 120.0) && close(after[4].right(), body.max), "Keys takes the room: {:?}", after[4]);
    assert!(
        (ui.value("structure-columns-edge-default").as_f64().unwrap_or_default() as f32 - after[3].width()).abs() <= 0.5,
        "the edge's value is the width"
    );
    let (_, cut) = ui.text_in(after[3], &words).expect("the default painted");
    assert!(cut, "the narrowed default is cut");
    ui.rest(after[3].center());
    assert!(ui.texts().into_iter().any(|(r, g)| g.text() == words && !g.elided && !after[3].contains_rect(r)), "the whole default on hover");
    ui.hover_away();
    // Never narrower than the grid's least width.
    let edge = ui.drawn("structure-columns-edge-nullable");
    let at = pos2(edge.center().x, after[2].center().y);
    drag(&mut ui, at, at - vec2(400.0, 0.0));
    ui.hover_away();
    let least = dbee_ui_egui::grid::MIN_WIDTH;
    assert!(close(columns_row(&mut ui)[2].width(), least), "Nullable at the least width {least}");
    // A double click on the edge fits Default to its content.
    ui.double_click("structure-columns-edge-default");
    ui.frame();
    ui.hover_away();
    let fitted = columns_row(&mut ui);
    let (at, cut) = ui.text_in(fitted[3], &words).expect("the default painted");
    assert!(
        !cut && fitted[3].width() >= at.width() + 16.0 - 0.5 && fitted[3].width() <= at.width() + 16.0 + 1.0,
        "fitted: {:?}, the words {at:?}",
        fitted[3]
    );
    // The widths last: DATA and back, then a reload.
    ui.click("table-view-data");
    ui.until("the grid", |ui| ui.tag("grid-cell-0-0").is_some());
    ui.click("table-view-structure");
    ui.until("the structure", |ui| ui.tag("structure-columns-0-name").is_some());
    ui.hover_away();
    let back = columns_row(&mut ui);
    assert!((0..5).all(|i| close(back[i].width(), fitted[i].width())), "the widths after DATA and back: {back:?}, {fitted:?}");
    ui.click("table-refresh");
    ui.frame();
    ui.until("the reload", |ui| ui.table()["structure"]["loading"].is_null() && ui.tag("structure-columns-0-name").is_some());
    ui.hover_away();
    let again = columns_row(&mut ui);
    assert!((0..5).all(|i| close(again[i].width(), fitted[i].width())), "the widths after a reload: {again:?}, {fitted:?}");
    // Another section's columns are its own: INDEXES' Name is untouched, and resizes alone.
    let index = ui.laid("structure-indexes-0-name");
    let edge = ui.drawn("structure-indexes-edge-name");
    let at = pos2(edge.center().x, index.center().y);
    drag(&mut ui, at, at + vec2(60.0, 0.0));
    ui.hover_away();
    assert!(close(ui.laid("structure-indexes-0-name").width(), index.width() + 60.0), "INDEXES' Name 60 wider");
    assert!((0..5).all(|i| close(columns_row(&mut ui)[i].width(), fitted[i].width())), "COLUMNS as it was");
}

/// The chosen view's `ink` ground meets the switch's edge at every scale, in both modes: from the
/// middle of the chosen segment out to the switch's outside, sideways at its end and up and down,
/// every pixel is the ground, the edge or a blend of the two. (The edge is 1 point wide from the
/// outside, 1.25 px at 125 %, and the ground ended on a whole pixel short of it: STRUCTURE chosen
/// showed a sliver of the box's `card` between the two.)
#[test]
fn the_chosen_view_meets_the_switchs_edge_at_every_scale() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = orders_structure(db);
    let themes = [("ui.themeLight", dbee_ui_egui::theme::LIGHT), ("ui.themeDark", dbee_ui_egui::theme::DARK)];
    for (theme, t) in themes {
        ui.execute(theme, json!({}));
        let within = |c: egui::Color32| {
            let (a, b) = (t.ink.to_array(), t.line_strong.to_array());
            (0..3).all(|i| c.a() == 255 && c.to_array()[i] + 3 >= a[i].min(b[i]) && c.to_array()[i] <= a[i].max(b[i]) + 3)
        };
        for scale in [1.0, 1.15, 1.25, 1.5, 1.75, 2.0] {
            ui.harness.ctx.set_pixels_per_point(scale);
            ui.frame();
            for view in ["data", "structure"] {
                let id = format!("table-view-{view}");
                ui.click(&id);
                ui.until("the view", |ui| ui.value(&id) == true);
                ui.hover_away();
                ui.frame();
                let (data, structure) = (ui.drawn("table-view-data"), ui.drawn("table-view-structure"));
                let chosen = if view == "data" { data } else { structure };
                let outer = data.union(structure).expand(1.0);
                let px = ui.pixels(outer.expand(2.0));
                let at = format!("{theme} at {scale}, {view} chosen: the switch {outer:?}");
                let whole = |v: f32| (v * scale).round() as i32;
                let (mid_x, mid_y) = (whole(chosen.center().x), whole(chosen.center().y));
                let across = if view == "data" { whole(outer.left())..mid_x } else { mid_x..whole(outer.right()) };
                let bad: Vec<(i32, egui::Color32)> = across.map(|x| (x, px.at(x, mid_y))).filter(|(_, c)| !within(*c)).collect();
                assert!(bad.is_empty(), "{at}: pixels along row {mid_y} that are neither its ground nor its edge: {bad:?}");
                let bad: Vec<(i32, egui::Color32)> =
                    (whole(outer.top())..whole(outer.bottom())).map(|y| (y, px.at(mid_x, y))).filter(|(_, c)| !within(*c)).collect();
                assert!(bad.is_empty(), "{at}: pixels down column {mid_x} that are neither its ground nor its edge: {bad:?}");
            }
        }
    }
}

//! The table tab driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the grid (cells, headers, selection, the FK link, the right-click menu), the
//! filter bar and the paging bar. Everything goes through control requests and automation ids,
//! the way an agent drives the real window; assertions read `ui.inspect` and `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is read-only: tests that stage
//! edits or need odd rows make their own table in `scratch` and drop it.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_testkit::{TestDb, unique};
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

impl Ui {
    /// Where `id` was drawn in the last frame, in view.
    fn rect(&self, id: &str) -> Option<egui::Rect> {
        self.harness.state().tags().rect(id)
    }

    /// Where each of `ids` was drawn in the last frame.
    fn rects(&self, ids: &[String]) -> Vec<(String, Option<egui::Rect>)> {
        ids.iter().map(|id| (id.clone(), self.rect(id))).collect()
    }

    /// The ids drawn in the last frame that start with `prefix`.
    fn ids_with(&self, prefix: &str) -> Vec<String> {
        self.harness.state().tags().ids().into_iter().filter(|id| id.starts_with(prefix)).map(str::to_string).collect()
    }
}

/// A scratch table `(id, a text, b text)` with one row `(1, NULL, '')` and one `(2, 'x', 'y')`.
fn null_and_empty(db: &TestDb) -> Scratch {
    Scratch::new(db, "t_grid", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text, b text); INSERT INTO scratch.{t} VALUES (1, NULL, ''), (2, 'x', 'y');")
    })
}

#[test]
fn day_filters_through_the_bar() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    // The first row is there to edit; two more come with +.
    ui.click("filter-add");
    ui.click("filter-add");
    let rows = [("dealer_id", "eq", "42"), ("status", "eq", "completed"), ("total", "gt", "100")];
    for (n, (column, op, value)) in rows.iter().enumerate() {
        ui.click(&format!("filter-{n}-column"));
        ui.click(&format!("filter-{n}-column-{column}"));
        ui.click(&format!("filter-{n}-op"));
        ui.click(&format!("filter-{n}-op-{op}"));
        ui.click(&format!("filter-{n}-value"));
        ui.text(value);
    }
    ui.frame();
    assert_eq!(ui.value("filter-2-value"), "100");
    ui.click("filter-apply");
    ui.until("the filtered page", |ui| ui.table()["loading"].is_null());
    let t = ui.table();
    let filters = t["filters"].as_array().unwrap();
    assert_eq!(filters.len(), 3, "{t}");
    assert_eq!(filters[2], json!({ "column": "total", "op": "gt", "value": "100" }));
    assert_eq!(t["applied"]["filters"].as_array().map(Vec::len), Some(3), "{t}");
    assert!(t["rows"].as_u64().is_some_and(|n| n > 0), "dealer 42 has completed orders above 100: {t}");
    assert!(ui.tag("grid-cell-0-0").is_some());
    let tab = ui.document()["active"].as_u64().unwrap();
    assert_eq!(ui.value(&format!("tab-{tab}")), "orders [3]");
    // − takes a row out of the bar; Enter in a value field applies.
    ui.click("filter-2-remove");
    ui.click("filter-1-value");
    ui.key("Enter");
    ui.until("the page with two filters", |ui| ui.table()["applied"]["filters"].as_array().map(Vec::len) == Some(2));
}

#[test]
fn raw_where_through_the_toggle() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    // A filter row first.
    ui.click("filter-0-column");
    ui.click("filter-0-column-name");
    ui.click("filter-0-value");
    ui.text("Customer 2");
    ui.frame();
    // To the raw field, applied there.
    ui.click("filter-raw-toggle");
    assert!(ui.tag("filter-0-column").is_none(), "one raw field instead of the rows");
    ui.click("filter-raw");
    ui.text("id <= 3");
    ui.key("Enter");
    ui.until("the raw filter", |ui| ui.table()["applied"] == json!({ "raw": "id <= 3" }) && ui.table()["loading"].is_null());
    let t = ui.table();
    assert_eq!((t["rawMode"].clone(), t["rows"].clone()), (json!(true), json!(3)), "{t}");
    // And back: the row is as it was left, and applies.
    ui.click("filter-raw-toggle");
    assert_eq!(ui.table()["rawMode"], false);
    assert_eq!((ui.value("filter-0-column"), ui.value("filter-0-op"), ui.value("filter-0-value")), (json!("name"), json!("eq"), json!("Customer 2")));
    ui.click("filter-apply");
    ui.until("the row filter", |ui| ui.table()["applied"]["filters"][0]["value"] == "Customer 2" && ui.table()["loading"].is_null());
    assert_eq!(ui.table()["rows"], 1);
    // The raw field kept its text too.
    ui.click("filter-raw-toggle");
    assert_eq!(ui.value("filter-raw"), "id <= 3");
}

#[test]
fn null_operators_take_no_value() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.click("filter-0-column");
    ui.click("filter-0-column-note");
    ui.click("filter-0-value");
    ui.text("abc");
    ui.frame();
    ui.click("filter-0-op");
    ui.click("filter-0-op-isNull");
    assert!(ui.tag("filter-0-value").is_none(), "is null takes no value: no field");
    assert_eq!(ui.table()["filters"], json!([{ "column": "note", "op": "isNull", "value": "" }]), "and the old value goes");
    ui.click("filter-apply");
    ui.until("the NULL notes", |ui| ui.table()["applied"]["filters"][0]["op"] == "isNull" && ui.table()["loading"].is_null());
    assert_eq!(ui.table()["rows"], 100, "every fifth customer has no note");
    ui.click("filter-0-op");
    ui.click("filter-0-op-eq");
    assert_eq!(ui.value("filter-0-value"), "", "the field is back, empty");
}

#[test]
fn a_focused_filter_value_keeps_its_own_ctrl_a() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.click("filter-0-value");
    ui.text("abc");
    ui.ctrl("A");
    ui.text("Customer 7");
    ui.frame();
    assert_eq!(ui.value("filter-0-value"), "Customer 7", "Ctrl+A selected the field's text");
    assert!(ui.document()["selection"].is_null(), "and selected no cells");
}

#[test]
fn clicking_fk_link_opens_customer() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    assert!(ui.tag("fk-link-0-1").is_none(), "the link shows on the selected row only");
    ui.click("grid-cell-0-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 1], "focus": [0, 1] }));
    let customer = ui.value("grid-cell-0-1")["text"].as_str().unwrap().to_string();
    assert!(ui.tag("fk-link-0-0").is_none(), "id is no foreign key");
    ui.click("fk-link-0-1");
    ui.until("the customers page", |ui| ui.table()["name"] == "customers" && ui.table()["loading"].is_null());
    let t = ui.table();
    assert_eq!(t["filters"], json!([{ "column": "id", "op": "eq", "value": customer }]), "{t}");
    assert_eq!(t["rows"], 1);
}

#[test]
fn null_and_empty_look_different() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    let null = ui.value("grid-cell-0-1");
    assert_eq!((null["text"].clone(), null["null"].clone()), (json!("NULL"), json!(true)), "NULL shows as a dim NULL: {null}");
    let empty = ui.value("grid-cell-0-2");
    assert_eq!((empty["text"].clone(), empty["null"].clone()), (json!(""), json!(false)), "an empty string is an empty cell: {empty}");
    let plain = ui.value("grid-cell-1-1");
    assert_eq!((plain["text"].clone(), plain["null"].clone()), (json!("x"), json!(false)));
}

#[test]
fn header_click_sorts() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    let head = ui.value("grid-header-0");
    assert_eq!((head["name"].clone(), head["key"].clone(), head["sort"].clone()), (json!("id"), json!("primary"), Json::Null), "{head}");
    ui.click("grid-header-0");
    ui.until("the ascending page", |ui| ui.table()["sort"] == json!({ "column": "id", "dir": "asc" }) && ui.table()["loading"].is_null());
    ui.click("grid-header-0");
    ui.until("the descending page", |ui| ui.table()["sort"] == json!({ "column": "id", "dir": "desc" }) && ui.table()["loading"].is_null());
    ui.frame();
    assert_eq!(ui.value("grid-header-0")["sort"], "desc");
    assert_eq!(ui.value("grid-cell-0-0")["text"], "500", "the highest id first");
    ui.click("grid-header-0");
    ui.until("no sort", |ui| ui.table()["sort"].is_null());
}

#[test]
fn context_menu_set_as_null() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.click_with("grid-cell-1-2", json!({ "button": "right" }));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 2], "focus": [1, 2] }), "a right-click selects the cell");
    for id in ["grid-menu-copy", "grid-menu-copy-header", "grid-menu-copy-csv", "grid-menu-copy-json", "grid-menu-copy-markdown", "grid-menu-delete"]
    {
        assert!(ui.tag(id).is_some(), "{id}");
    }
    ui.click("grid-menu-null");
    let t = ui.table();
    assert_eq!(t["edits"]["count"], 1, "{t}");
    assert_eq!(t["edits"]["cells"], json!([{ "row": 1, "column": 2, "value": null }]));
    ui.frame();
    let cell = ui.value("grid-cell-1-2");
    assert_eq!((cell["text"].clone(), cell["state"].clone()), (json!("NULL"), json!("staged")), "{cell}");
    assert_eq!(ui.value("edits-count"), "1 change");
    // Delete row marks it; Reset drops both.
    ui.click_with("grid-cell-0-0", json!({ "button": "right" }));
    ui.click("grid-menu-delete");
    ui.frame();
    assert_eq!(ui.value("grid-cell-0-0")["state"], "deleted");
    ui.click("edit-reset");
    assert_eq!(ui.table()["edits"]["count"], 0);
}

#[test]
fn staged_edits_block_sorting_and_applying_filters_and_say_why() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    ui.click("grid-menu-null");
    assert_eq!(ui.table()["edits"]["count"], 1);
    ui.frame();
    assert_eq!(ui.tag("filter-apply").unwrap()["enabled"], false, "Apply waits for the staged change");
    // A header click sorts nothing, and says why.
    ui.click("grid-header-0");
    ui.frame();
    assert_eq!(ui.document()["jobs"], json!([]));
    assert!(ui.table()["sort"].is_null());
    let notice = ui.value("notice");
    assert_eq!(notice, "Apply or reset your changes first.", "the reason alone, as a sentence");
    // It sits above the paging bar, so Reset and Apply stay in reach.
    let (n, apply) = (ui.tag("notice").unwrap(), ui.tag("edit-apply").unwrap());
    let bottom = n["rect"][1].as_f64().unwrap() + n["rect"][3].as_f64().unwrap();
    assert!(bottom <= apply["rect"][1].as_f64().unwrap(), "{n} {apply}");
    ui.until("the notice to go", |ui| ui.tag("notice").is_none());
    // Typing still works; Enter applies nothing, and says why.
    ui.click("filter-0-value");
    ui.text("2");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.document()["jobs"], json!([]));
    let t = ui.table();
    assert_eq!((t["applied"].clone(), t["filters"][0]["value"].clone()), (json!("none"), json!("2")), "{t}");
    let notice = ui.value("notice");
    assert_eq!(notice, "Apply or reset your changes first.", "the reason alone, as a sentence");
}

#[test]
fn a_failed_delete_stays_struck_through() {
    let db = dbee_testkit::require_testdb!();
    let child = unique("t_child");
    let parent = Scratch::new(db, "t_parent", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY); INSERT INTO scratch.{t} VALUES (1), (2);
             CREATE TABLE scratch.{child} (id int PRIMARY KEY, parent_id int REFERENCES scratch.{t}); INSERT INTO scratch.{child} VALUES (1, 1);"
        )
    });
    let _child = Scratch(child);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &parent.0);
    ui.click_with("grid-cell-0-0", json!({ "button": "right" }));
    ui.click("grid-menu-delete");
    ui.frame();
    assert_eq!(ui.value("grid-cell-0-0")["state"], "deleted");
    // Row 1 is still referenced: the Apply fails on it.
    ui.click("edit-apply");
    ui.until("the failed Apply", |ui| ui.table()["failed"].is_object());
    ui.frame();
    let cell = ui.value("grid-cell-0-0");
    assert_eq!((cell["state"].clone(), cell["deleted"].clone()), (json!("failed"), json!(true)), "failed, and still struck through: {cell}");
    assert_eq!(ui.value("grid-cell-1-0").get("deleted"), None);
}

#[test]
fn context_menu_copy_puts_the_selection_on_the_clipboard() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    ui.click("grid-menu-copy-json");
    let copied = ui.copied().expect("Copy as JSON put text on the clipboard");
    assert!(copied.contains("\"a\"") && copied.contains("\"x\""), "{copied}");
}

#[test]
fn a_page_draws_in_the_frame_its_rows_arrive() {
    let db = dbee_testkit::require_testdb!();
    for t in dbee_testkit::targets() {
        let mut ui = connected_to_target(db, &t, None, APP_SIZE);
        // The reply goes out in the frame that applies the rows; `ui.inspect` reports that frame.
        ui.execute("table.open", json!({ "schema": t.schema("shop"), "table": "big_rows" }));
        let i = ui.inspect();
        assert!(
            visible(&tag_in(&i, "grid-cell-0-0")),
            "{}: the first row is drawn in the frame it arrived: {:?} {:?}",
            t.name(),
            tag_in(&i, "grid-cell-0-0"),
            tag_in(&i, "page-number")
        );
        assert!(visible(&tag_in(&i, "grid-header-1")), "{}", t.name());
        // Only the rows in view are laid out; the others are listed as out of view.
        let far = tag_in(&i, "grid-cell-99-0").expect("rows out of view are listed");
        assert_eq!(far["visible"], false, "{}: {far}", t.name());
    }
}

#[test]
fn rows_out_of_view_scroll_in_when_clicked() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "big_rows");
    ui.click("grid-cell-90-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [90, 1], "focus": [90, 1] }));
    assert!(visible(&ui.tag("grid-cell-90-1")));
    assert_eq!(ui.value("grid-cell-90-1")["text"], "payload 91");
    // A column out of view to the right scrolls in too.
    ui.open("shop", "orders");
    let i = ui.inspect();
    assert!(!visible(&tag_in(&i, "grid-cell-5-8")), "raw_json_text starts out of view: {:?}", tag_in(&i, "grid-cell-5-8"));
    ui.click("grid-cell-5-8");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [5, 8], "focus": [5, 8] }));
    assert!(visible(&ui.tag("grid-cell-5-8")));
}

#[test]
fn shift_click_drag_and_ctrl_a_select() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.click("grid-cell-1-0");
    ui.click_with("grid-cell-3-2", json!({ "modifiers": { "shift": true } }));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [3, 2] }));
    // A drag from one cell to another.
    let (from, to) = (ui.centre("grid-cell-4-1"), ui.centre("grid-cell-6-3"));
    let press =
        |pressed| Event::PointerButton { pos: if pressed { from } else { to }, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![press(true)]);
    for k in 1..=4 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 4.0))]);
    }
    ui.frame_with(vec![press(false)]);
    assert_eq!(ui.document()["selection"], json!({ "anchor": [4, 1], "focus": [6, 3] }));
    // Ctrl+A with no text field focused selects every cell.
    ui.ctrl("A");
    let sel = ui.document()["selection"].clone();
    assert_eq!(sel["anchor"], json!([0, 0]));
    assert_eq!(sel["focus"][0], 99, "{sel}");
}

#[test]
fn resized_columns_stay_resized_per_tab() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    let customers = ui.document()["active"].as_u64().unwrap();
    let width = |ui: &mut Ui, id: &str| ui.tag(id).unwrap()["rect"][2].as_f64().unwrap() as f32;
    let before = width(&mut ui, "grid-header-1");
    // Drag the right edge of the `name` column 60 points to the right.
    let head = ui.tag("grid-header-1").unwrap();
    let r = &head["rect"];
    let f = |i: usize| r[i].as_f64().unwrap() as f32;
    let (from, to) = (Pos2::new(f(0) + f(2), f(1) + f(3) / 2.0), Pos2::new(f(0) + f(2) + 60.0, f(1) + f(3) / 2.0));
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![button(from, true)]);
    for k in 1..=6 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))]);
    }
    ui.frame_with(vec![button(to, false)]);
    ui.frame();
    let after = width(&mut ui, "grid-header-1");
    assert!((after - before - 60.0).abs() < 2.0, "{before} → {after}");
    // Another tab has its own widths; back on this one, the column is as it was left.
    ui.open("shop", "orders");
    ui.click(&format!("tab-{customers}"));
    ui.frame();
    assert_eq!(ui.table()["name"], "customers");
    let again = width(&mut ui, "grid-header-1");
    assert!((again - after).abs() < 1.0, "{after} → {again}");
}

#[test]
fn paging_and_counting_big_rows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "big_rows");
    // The planner's estimate shows until the rows are counted; filter rows with no value do not
    // filter (the engine's rule), so it shows with them applied too.
    ui.click("filter-add");
    ui.click("filter-apply");
    ui.until("the empty filters", |ui| ui.table()["applied"]["filters"].as_array().map(Vec::len) == Some(2) && ui.table()["loading"].is_null());
    let count = ui.value("page-count");
    let text = count.as_str().unwrap().to_string();
    assert!(text.starts_with('~') && text.contains(','), "{count}");
    assert_eq!(ui.value("page-number"), "Page 1");
    ui.click("page-next");
    ui.until("page 2", |ui| ui.table()["page"] == 1);
    ui.frame();
    assert_eq!(ui.value("grid-cell-0-0")["text"], "101");
    assert_eq!(ui.value("page-number"), "Page 2");
    ui.click("page-prev");
    ui.until("page 1", |ui| ui.table()["page"] == 0);
    ui.click("page-last");
    ui.until("the last page", |ui| ui.table()["page"] == 9999);
    ui.frame();
    assert_eq!(ui.table()["total"], json!({ "exact": 1_000_000 }));
    assert_eq!(ui.value("page-count"), "1,000,000 rows");
    assert_eq!(ui.value("page-number"), "Page 10,000 of 10,000");
    ui.click("page-first");
    ui.until("the first page", |ui| ui.table()["page"] == 0);
    ui.click("table-refresh");
    ui.until("the refresh", |ui| ui.table()["loading"].is_null());
}

#[test]
fn count_rows_on_a_view_and_its_read_only_note() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "order_totals");
    assert_eq!(ui.value("table-note"), "These rows can't be edited: views are read-only.");
    // No estimate for a view: never `~0`.
    assert_eq!(ui.value("page-count"), "Count rows");
    ui.click("page-count");
    ui.until("the count", |ui| ui.table()["total"]["exact"].is_u64());
    ui.frame();
    assert_eq!(ui.value("page-count"), "500 rows");
    // An editable table has no note.
    ui.open("shop", "customers");
    assert!(ui.tag("table-note").is_none());
}

/// A table's own reason, a sentence of the driver's (an engine without transactions), shows as
/// written where dbee's own reasons show: one line, cut short in a narrow tab and whole on hover,
/// so the filter bar and the grid stay where a short note leaves them.
#[test]
fn myisam_reason_is_shown() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
    let loaded = |ui: &mut Ui| ui.table()["loading"].is_null() && ui.table()["rows"].as_u64().is_some_and(|n| n > 0);
    // dbee's own reasons keep their lead-in.
    ui.open("shop", "order_totals");
    ui.until("the view's rows", loaded);
    ui.frame();
    assert_eq!(ui.value("table-note"), "These rows can't be edited: views are read-only.");
    let (short, bar) = (ui.rect("table-note").unwrap(), ui.rect("filter-0-column").unwrap());
    ui.open("shop", "audit_log");
    ui.until("the audit_log rows", loaded);
    ui.frame();
    let reason = ui.table()["noEditReason"].as_str().unwrap_or_default().to_owned();
    assert!(reason.starts_with("The MyISAM table engine can't undo a write") && reason.ends_with('.'), "{reason}");
    assert_eq!(ui.value("table-note"), reason.as_str());
    let note = ui.rect("table-note").unwrap();
    assert!((note.height() - short.height()).abs() < 0.5, "one line: {note:?} vs {short:?}");
    assert_eq!(ui.rect("filter-0-column").unwrap().min, bar.min, "the filter bar stays where it was");
    let (_, cut) = ui.text_in(note, &reason).unwrap_or_else(|| panic!("the reason is drawn in {note:?}"));
    assert!(cut, "cut short in the app's first window, with the value panel open");
    // Hovered, it reads whole.
    ui.rest(note.center());
    let whole = ui.texts().into_iter().any(|(r, g)| g.text() == reason && !g.elided && !note.contains(r.center()));
    assert!(whole, "the hover shows the whole reason");
}

/// The filter bar's operator hover adds what the operator does on this engine that its label
/// does not say; on Postgres there is nothing to add.
#[test]
fn operator_note_on_mysql() {
    let db = dbee_testkit::require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let pg = dbee_testkit::target(pg14_server(db));
    for (t, note) in [(&mysql, true), (&pg, false)] {
        let mut ui = connected_to_target(db, t, None, APP_SIZE);
        ui.open("shop", "orders");
        ui.until("the page", |ui| ui.table()["rows"].as_u64().is_some_and(|n| n > 0));
        ui.click("filter-0-op");
        ui.frame();
        // An operator without a note has no hover: resting on it draws no more text.
        let is_null = ui.rect("filter-0-op-isNull").expect("the operators are listed");
        ui.hover(is_null.center());
        let before = ui.texts().len();
        ui.rest(is_null.center());
        assert_eq!(ui.texts().len(), before, "{}: no hover on `is null`", t.name());
        let like = ui.rect("filter-0-op-like").expect("the operators are listed");
        ui.rest(like.center());
        let shown = ui.texts().into_iter().any(|(_, g)| g.text().contains("collation"));
        assert_eq!(shown, note, "{}: the like operator's hover", t.name());
        // The chosen operator's hover says it too.
        ui.click("filter-0-op-like");
        let choice = ui.rect("filter-0-op").unwrap();
        ui.rest(choice.center());
        let shown = ui.texts().into_iter().any(|(_, g)| g.text().contains("collation"));
        assert_eq!(shown, note, "{}: the chosen operator's hover", t.name());
    }
}

#[test]
fn a_bad_filter_value_shows_the_error_over_the_old_rows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.click("filter-0-value");
    ui.text("not a number");
    ui.key("Enter");
    ui.until("the error", |ui| ui.table()["error"].is_string());
    ui.frame();
    let card = ui.value("table-error");
    assert!(card.as_str().is_some_and(|e| e.contains("invalid input syntax")), "{card}");
    // The band says it: its heading, the message as the server wrote it, today's note.
    let band = ui.rect("table-error").unwrap();
    for words in ["The page did not load", card.as_str().unwrap_or_default(), "The rows below are the ones from before."] {
        assert!(ui.text_in(band, words).is_some(), "`{words}` in the band");
    }
    assert_eq!(ui.value("grid-cell-0-0")["text"], "1", "the rows from before stay");
    assert!(ui.tag("notice").is_none(), "the card says it, not a notice too");
    // A good filter clears it.
    ui.click("filter-0-value");
    ui.ctrl("A");
    ui.text("7");
    ui.key("Enter");
    ui.until("the filtered page", |ui| ui.table()["error"].is_null() && ui.table()["rows"] == 1);
    ui.frame();
    assert!(ui.tag("table-error").is_none());
}

#[test]
fn odd_identifiers_filter_sort_and_follow() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("odd", "Odd \"Name\" 🐙");
    assert_eq!(ui.value("grid-header-1")["name"], "Total $");
    assert_eq!(ui.value("grid-header-3")["key"], "foreign");
    ui.click("filter-0-column");
    ui.click("filter-0-column-lower UPPER");
    ui.click("filter-0-op");
    ui.click("filter-0-op-isNotNull");
    ui.click("filter-apply");
    ui.until("the filtered page", |ui| ui.table()["applied"]["filters"][0]["column"] == "lower UPPER" && ui.table()["loading"].is_null());
    assert_eq!(ui.table()["rows"], 3);
    ui.click("grid-header-1");
    ui.until("the sort", |ui| ui.table()["sort"] == json!({ "column": "Total $", "dir": "asc" }) && ui.table()["loading"].is_null());
    ui.frame();
    assert_eq!(ui.value("grid-cell-0-1")["text"], "0.5");
    ui.click("grid-cell-0-3");
    ui.click("fk-link-0-3");
    ui.until("the other table's page", |ui| ui.table()["name"] == "Other Ödd" && ui.table()["loading"].is_null());
    assert_eq!(ui.table()["filters"], json!([{ "column": "id", "op": "eq", "value": "1" }]));
}

/// The grid over the row of two 20 MB cells, one of them selected (the value panel builds its view
/// once); the slowest of the 10 frames after.
fn twenty_megabyte_cells(db: &TestDb) -> Duration {
    let mut ui = Ui::connected(db);
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.table()["rows"] == 1);
    let text = ui.value("grid-cell-0-1")["text"].as_str().unwrap().to_string();
    assert!(text.chars().count() <= 201 && text.ends_with('…'), "the grid shows the cut text: {} chars", text.chars().count());
    ui.click("grid-cell-0-2");
    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let start = Instant::now();
        ui.frame();
        slowest = slowest.max(start.elapsed());
    }
    println!("the slowest of 10 frames with two 20 MB cells: {slowest:?}");
    slowest
}

#[test]
fn twenty_megabyte_cells_show_cut() {
    let db = dbee_testkit::require_testdb!();
    twenty_megabyte_cells(db);
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn twenty_megabyte_cells_keep_frames_under_250_ms() {
    let db = dbee_testkit::require_testdb!();
    let slowest = twenty_megabyte_cells(db);
    assert!(slowest < Duration::from_millis(250), "a frame with two 20 MB cells took {slowest:?}");
}

#[test]
fn a_page_cut_by_the_size_limit_says_so() {
    let db = dbee_testkit::require_testdb!();
    // 14 values of 20,000,000 characters: past a page's 256 MiB, so 13 rows are kept.
    let table = Scratch::new(db, "t_cut", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY, v text); INSERT INTO scratch.{t} SELECT g, repeat('x', 20000000) FROM generate_series(1, 14) AS g;"
        )
    });
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 13);
    let note = ui.value("page-cut");
    assert!(note.as_str().is_some_and(|n| n.contains("13")), "{note}");
    ui.open("shop", "customers");
    assert!(ui.tag("page-cut").is_none());
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
fn grid_frame_within_budget() {
    use dbee_core::{Column, ResultSet, TypeClass};
    use dbee_engine::Selection;
    use dbee_ui_egui::automation::Tags;
    use dbee_ui_egui::grid::{Grid, GridState, RowEdits};

    struct State {
        rows: ResultSet,
        grid: GridState,
        tags: Tags,
        selection: Option<Selection>,
    }
    // 10,000 rows of 20 columns (a query result at its cap), one value 20 MB long.
    let huge = "y".repeat(20_000_000);
    let columns = (0..20)
        .map(|c| Column { name: format!("column_{c}"), base_type: "text".into(), type_label: "text".into(), class: TypeClass::LongText })
        .collect();
    let rows = (0..10_000).map(|r| (0..20).map(|c| Some(if r == 3 && c == 2 { huge.clone() } else { format!("value {r}.{c}") })).collect()).collect();
    let state = State {
        rows: ResultSet { columns, rows, rows_seen: 10_000, complete: true, ..ResultSet::default() },
        grid: GridState::default(),
        tags: Tags::default(),
        selection: None,
    };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, s: &mut State| {
            s.tags.begin_frame();
            let grid = Grid {
                id: egui::Id::new("timing"),
                source: &s.rows,
                keys: &[],
                selection: s.selection,
                sort: None,
                sortable: false,
                blocked: None,
                edits: RowEdits::None,
                links: false,
                first_number: 1,
                editing: None,
            };
            let _ = grid.show(ui, &mut s.grid, &mut s.tags);
        },
        state,
    );
    h.step();
    let mut frames = Vec::new();
    for i in 0..60 {
        // A new selection each frame, as while clicking about.
        h.state_mut().selection = Some(Selection { anchor: (i % 30, i % 20), focus: (i % 30, i % 20) });
        let start = Instant::now();
        h.step();
        frames.push(start.elapsed());
    }
    frames.sort();
    let median = frames[frames.len() / 2];
    let cells = h.state().tags.ids().iter().filter(|id| id.starts_with("grid-cell-")).count();
    println!(
        "grid of 10,000 × 20 (one 20 MB value in view): frame median {median:?} (max {:?}); {cells} cells drawn; on {}",
        frames.last().unwrap(),
        machine()
    );
    assert!(median < Duration::from_millis(16), "{median:?}");
}

/// The operators as the filter bar's choices read, in `Op::ALL` order with their ids: the symbol
/// in brackets after the words, a word operator by its words alone.
const OPS: [(&str, &str); 13] = [
    ("eq", "equals (=)"),
    ("ne", "does not equal (<>)"),
    ("like", "like"),
    ("notLike", "not like"),
    ("ilike", "ilike"),
    ("notIlike", "not ilike"),
    ("lt", "less than (<)"),
    ("le", "less than or equal (<=)"),
    ("gt", "greater than (>)"),
    ("ge", "greater than or equal (>=)"),
    ("in", "in"),
    ("isNull", "is null"),
    ("isNotNull", "is not null"),
];

/// The app's window (apps/dbee: 1100 × 720, the value panel open, its default) and a wide one.
const SIZES: [egui::Vec2; 2] = [egui::vec2(1100.0, 720.0), egui::vec2(1700.0, 720.0)];

/// Every one of `ids` drawn, all as tall as `height` (the first one's when `None`) and level
/// with each other; returns the height.
fn assert_level(ui: &Ui, ids: &[&str], height: Option<f32>, at: &str) -> f32 {
    let rects: Vec<(&str, egui::Rect)> =
        ids.iter().map(|id| (*id, ui.rect(id).unwrap_or_else(|| panic!("{at}: `{id}` is drawn, in view")))).collect();
    let (first, r0) = rects[0];
    let h = height.unwrap_or(r0.height());
    for (id, r) in &rects {
        assert!((r.height() - h).abs() <= 0.5, "{at}: `{id}` is {} tall, not {h} like `{first}`: {rects:?}", r.height());
        assert!((r.top() - r0.top()).abs() <= 0.5, "{at}: `{id}` {r:?} is not level with `{first}` {r0:?}");
    }
    h
}

#[test]
fn the_filter_bar_controls_are_one_height() {
    let db = dbee_testkit::require_testdb!();
    for size in SIZES {
        let at = format!("{} × {}", size.x, size.y);
        let mut ui = Ui::connected_in(db, size);
        ui.open("shop", "orders");
        ui.click("filter-add");
        ui.frame();
        let first = ["filter-raw-toggle", "filter-0-column", "filter-0-op", "filter-0-value", "filter-0-remove", "filter-add", "filter-apply"];
        let h = assert_level(&ui, &first, None, &format!("{at}, the first row"));
        assert_level(&ui, &["filter-1-column", "filter-1-op", "filter-1-value", "filter-1-remove"], Some(h), &format!("{at}, the second row"));
        // An operator without a value: the row keeps its height.
        ui.click("filter-1-op");
        ui.click("filter-1-op-isNull");
        ui.frame();
        assert!(ui.rect("filter-1-value").is_none(), "{at}: is null has no value field");
        assert_level(&ui, &["filter-1-column", "filter-1-op", "filter-1-remove"], Some(h), &format!("{at}, the second row with is null"));
        // The raw WHERE field.
        ui.click("filter-raw-toggle");
        ui.frame();
        assert_level(&ui, &["filter-raw-toggle", "filter-raw", "filter-apply"], Some(h), &format!("{at}, the raw field"));
    }
}

#[test]
fn the_operator_choice_shows_each_label_whole() {
    let db = dbee_testkit::require_testdb!();
    for size in SIZES {
        let at = format!("{} × {}", size.x, size.y);
        let mut ui = Ui::connected_in(db, size);
        ui.open("shop", "orders");
        ui.frame();
        // The open list: every operator, its label whole.
        ui.click("filter-0-op");
        ui.frame();
        for (id, label) in OPS {
            let item = format!("filter-0-op-{id}");
            let rect = ui.rect(&item).unwrap_or_else(|| panic!("{at}: `{item}` shows in the open list"));
            let (text, cut) = ui.text_in(rect, label).unwrap_or_else(|| {
                panic!(
                    "{at}: `{item}` reads `{label}`: {:?}",
                    ui.texts().iter().filter(|(r, _)| rect.contains(r.center())).map(|(_, g)| g.text().to_string()).collect::<Vec<_>>()
                )
            });
            assert!(!cut && rect.expand(0.5).contains_rect(text), "{at}: `{label}` is cut in its item: {text:?} in {rect:?}");
        }
        ui.click("filter-0-op");
        // The closed choice shows the chosen operator's label whole where the row has room for
        // it (the wide window), cut with … inside its box where it hasn't (the app's window with
        // the value panel open: the row at the page's sizes is narrower than every control at its
        // least with the longest label), and the row still fits: left of the value panel, nothing
        // over anything.
        for (id, label) in OPS {
            ui.click("filter-0-op");
            ui.click(&format!("filter-0-op-{id}"));
            ui.frame();
            assert_eq!(ui.value("filter-0-op"), id, "{at}");
            let rect = ui.rect("filter-0-op").unwrap();
            let (text, cut) = ui.text_in(rect, label).unwrap_or_else(|| panic!("{at}: the closed choice reads `{label}`"));
            assert!(rect.contains_rect(text), "{at}: `{label}` runs out of the closed choice: {text:?} in {rect:?}");
            assert!(!cut || size.x < 1700.0, "{at}: `{label}` is cut in the closed choice: {text:?} in {rect:?}");
            let mut row = vec!["filter-raw-toggle", "filter-0-column", "filter-0-op", "filter-0-remove", "filter-add", "filter-apply"];
            if !matches!(id, "isNull" | "isNotNull") {
                row.insert(3, "filter-0-value");
            }
            let panel = ui.rect("value-panel").map_or(size.x, |r| r.left());
            let rects: Vec<(&str, egui::Rect)> =
                row.iter().map(|w| (*w, ui.rect(w).unwrap_or_else(|| panic!("{at}, {label}: `{w}` shows")))).collect();
            for (i, (a, ra)) in rects.iter().enumerate() {
                assert!(ra.right() <= panel + 0.5, "{at}, {label}: `{a}` {ra:?} runs into the value panel at {panel}");
                for (b, rb) in &rects[i + 1..] {
                    assert!(ra.right() <= rb.left() + 0.5, "{at}, {label}: `{a}` {ra:?} runs over `{b}` {rb:?}");
                }
            }
        }
    }
}

/// The widgets that must stay where they are while the pointer moves over the filter row's
/// choices and their lists: the tree, the tab strip, the filter row, the grid's paging bar and
/// the value panel.
const STILL: [&str; 14] = [
    "tree-filter",
    "tree-refresh",
    "tree-schema-shop",
    "tab-new",
    "filter-raw-toggle",
    "filter-0-column",
    "filter-0-op",
    "filter-0-value",
    "filter-0-remove",
    "filter-add",
    "filter-apply",
    "page-first",
    "value-panel",
    "value-toggle",
];

#[test]
fn hovering_the_filter_choices_and_their_lists_moves_nothing() {
    let db = dbee_testkit::require_testdb!();
    for size in SIZES {
        let at = format!("{} × {}", size.x, size.y);
        let mut ui = Ui::connected_in(db, size);
        ui.open("shop", "customers");
        ui.open("shop", "orders");
        ui.until("the page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"].as_u64().is_some_and(|n| n > 0));
        let mut still: Vec<String> = STILL.iter().map(|s| (*s).to_string()).collect();
        still.extend(ui.ids_with("tab-").into_iter().filter(|id| id.trim_start_matches("tab-").parse::<u64>().is_ok()));
        // The pointer away from everything, then the place of each.
        let away = egui::pos2(size.x / 2.0, size.y - 60.0);
        ui.hover(away);
        ui.frame();
        let before = ui.rects(&still);
        assert!(before.iter().all(|(_, r)| r.is_some()), "{at}: all drawn: {before:?}");
        let check = |ui: &mut Ui, what: &str| {
            let now = ui.rects(&still);
            for ((id, was), (_, is)) in before.iter().zip(&now) {
                let moved = match (was, is) {
                    (Some(a), Some(b)) => (a.min - b.min).length() > 0.5 || (a.size() - b.size()).length() > 0.5,
                    _ => true,
                };
                assert!(!moved, "{at}: `{id}` moved from {was:?} to {is:?} while the pointer was over {what}");
            }
        };
        for list in ["column", "op"] {
            let choice = format!("filter-0-{list}");
            // The closed choice, hovered for longer than a tooltip waits (0.5 s; frames are 0.25 s
            // apart in the harness).
            let centre = ui.rect(&choice).unwrap().center();
            for _ in 0..4 {
                ui.hover(centre);
                check(&mut ui, &choice);
            }
            ui.click(&choice);
            ui.frame();
            let items = ui.ids_with(&format!("{choice}-"));
            assert!(!items.is_empty(), "{at}: the {list} list is open");
            // The list itself keeps its items where they are too: none grows, none shifts, so no
            // scroll bar comes and goes.
            let listed = ui.rects(&items);
            for item in &items {
                let Some(r) = ui.rect(item) else { continue };
                for _ in 0..4 {
                    ui.hover(r.center());
                    check(&mut ui, item);
                    for ((id, was), (_, is)) in listed.iter().zip(&ui.rects(&items)) {
                        assert_eq!(was, is, "{at}: the list's `{id}` moved while the pointer was over `{item}`");
                    }
                }
            }
            // Closed again: a click on the choice.
            ui.click(&choice);
            ui.hover(away);
            ui.frame();
            assert!(ui.ids_with(&format!("{choice}-")).is_empty(), "{at}: the {list} list closed");
            check(&mut ui, "nothing");
        }
    }
}

#[test]
fn a_filter_without_a_value_is_muted_left_out_and_not_counted() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    let filters = json!([{ "column": "id", "op": "lt", "value": "3" }, { "column": "status", "op": "eq", "value": "" }]);
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders", "filters": filters }));
    ui.until("the page", |ui| ui.table()["rows"] == 2);
    ui.frame();
    let tab = ui.document()["active"].as_u64().unwrap();
    assert_eq!(ui.value(&format!("tab-{tab}")), "orders [1]", "the empty row is not counted");
    assert!(ui.tag("filter-0-unapplied").is_none(), "a row with a value is applied");
    assert_eq!(ui.value("filter-1-unapplied"), dbee_ui_egui::filter_bar::UNAPPLIED_HINT);
    // A value makes it a filter like the others.
    ui.click("filter-1-value");
    ui.text("paid");
    ui.key("Enter");
    ui.until("the filtered page", |ui| ui.table()["applied"]["filters"][1]["value"] == "paid");
    ui.frame();
    assert!(ui.tag("filter-1-unapplied").is_none());
    assert_eq!(ui.value(&format!("tab-{tab}")), "orders [2]");
    // An operator that takes no value is never left out.
    ui.execute("table.setFilters", json!({ "filters": [{ "column": "shipped_at", "op": "isNull", "value": "" }] }));
    ui.frame();
    assert!(ui.tag("filter-0-unapplied").is_none());
    assert_eq!(ui.value(&format!("tab-{tab}")), "orders [1]");
}

#[test]
fn a_table_keyed_by_its_rowid_shows_no_rowid() {
    let db = dbee_testkit::require_testdb!();
    let seed = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = seed.file().map(std::path::Path::to_path_buf);
    let config = dbee_config::ConnectionConfig { name: "sqlite".into(), kind: dbee_config::DbKind::Sqlite, file, ..Default::default() };
    let mut ui = Ui::connected_with(db, Setup { config: Some(config), ..Setup::default() });
    ui.open("main", "notes_nokey");
    ui.until("the rows", |ui| ui.table()["rows"] == 3);
    assert_eq!(ui.ids_with("grid-header-"), ["grid-header-0", "grid-header-1"]);
    assert_eq!((ui.value("grid-header-0")["name"].clone(), ui.value("grid-header-1")["name"].clone()), (json!("title"), json!("body")));
    assert_eq!(ui.value("grid-header-0")["key"], Json::Null, "no key column shows");
    assert_eq!(ui.value("grid-cell-1-0")["text"], "second");
    // The tree lists the visible columns only.
    ui.click("tree-expand-main.notes_nokey");
    let i = ui.inspect();
    assert!(tag_in(&i, "tree-column-main.notes_nokey.title").is_some(), "{:?}", ids(&i));
    assert!(tag_in(&i, "tree-column-main.notes_nokey.rowid").is_none(), "{:?}", ids(&i));
    // The table takes edits, by the rowid it does not show.
    assert_eq!(ui.document()["table"]["editable"], true);
}

/// Of `ids`, those drawn with the keyboard's focus look in the last frame: their own edge 2 px
/// `ink`.
fn keyboard_focus(ui: &Ui, ids: &[&str]) -> Vec<String> {
    let ink = dbee_ui_egui::theme::LIGHT.ink;
    let shapes = ui.shapes();
    ids.iter()
        .filter(|id| {
            ui.rect(id).is_some_and(|r| {
                shapes.iter().any(|(_, s)| {
                    matches!(s, egui::Shape::Rect(e) if e.stroke.width == 2.0 && e.stroke.color == ink
                        && (e.rect.min - r.min).length() < 0.6 && (e.rect.max - r.max).length() < 0.6)
                })
            })
        })
        .map(|id| (*id).to_string())
        .collect()
}

/// Review Focus 4: the filter row by keyboard. Tab reaches the raw SQL toggle, the column, the
/// operator, the value, remove, add and APPLY in that order, then the next row's controls, each
/// showing the keyboard's focus look; Shift+Tab goes back; Enter in a value applies, as today.
#[test]
fn the_filter_row_by_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    ui.click("filter-add");
    ui.frame();
    let order = [
        "filter-raw-toggle",
        "filter-0-column",
        "filter-0-op",
        "filter-0-value",
        "filter-0-remove",
        "filter-add",
        "filter-apply",
        "filter-1-column",
        "filter-1-op",
        "filter-1-value",
        "filter-1-remove",
    ];
    // A click gives the first value the keyboard (without the look); Shift+Tab goes back to the
    // toggle.
    ui.click("filter-0-value");
    ui.frame();
    assert!(keyboard_focus(&ui, &order).is_empty(), "a click shows no keyboard look");
    for back in ["filter-0-op", "filter-0-column", "filter-raw-toggle"] {
        ui.key_with("Tab", json!({ "shift": true }));
        ui.frame();
        assert_eq!(keyboard_focus(&ui, &order), [back], "Shift+Tab reaches `{back}`");
    }
    for id in &order[1..] {
        ui.key("Tab");
        ui.frame();
        assert_eq!(keyboard_focus(&ui, &order), [*id], "Tab reaches `{id}`");
    }
    // Back to the first value; typing and Enter apply the bar.
    for _ in 0..7 {
        ui.key_with("Tab", json!({ "shift": true }));
    }
    ui.frame();
    assert_eq!(keyboard_focus(&ui, &order), ["filter-0-value"]);
    ui.text("42");
    ui.key("Enter");
    ui.until("the filtered page", |ui| ui.table()["applied"]["filters"][0]["value"] == "42" && ui.table()["loading"].is_null());
}

/// The raw SQL toggle is drawn on in the raw WHERE mode (`button/icon` `style=boxed, state=on`): a
/// `fill` ground inside its edge; off in the filter rows, no ground. Its tag says the same.
#[test]
fn the_raw_toggle_is_drawn_on_in_raw_mode() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    let fill = dbee_ui_egui::theme::tokens(&ui.harness.ctx).fill;
    let ground = |ui: &Ui| {
        let at = ui.rect("filter-raw-toggle").expect("the toggle is drawn");
        ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill == fill && (r.rect.min - at.min).length() < 0.5 && (r.rect.max - at.max).length() < 0.5))
    };
    ui.frame();
    assert_eq!(ui.value("filter-raw-toggle"), false);
    assert!(!ground(&ui), "the filter rows: off, no ground");
    ui.click("filter-raw-toggle");
    ui.frame();
    assert_eq!(ui.value("filter-raw-toggle"), true);
    assert!(ground(&ui), "raw WHERE: on, the fill ground");
    ui.click("filter-raw-toggle");
    ui.frame();
    assert!(!ground(&ui), "back to the rows: off again");
}

/// The filter row's tags keep their ids, values and enabled flags as the row moves to the
/// shared controls (each now the page's 28 tall): the rows, the open lists, the raw field.
#[test]
fn filter_tags_keep_their_ids_and_values() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    let filters = json!([{ "column": "dealer_id", "op": "eq", "value": "42" }, { "column": "status", "op": "eq", "value": "" }]);
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders", "filters": filters }));
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    ui.frame();
    let i = ui.inspect();
    let want = [
        ("filter-raw-toggle", Some(json!(false))),
        ("filter-0-column", Some(json!("dealer_id"))),
        ("filter-0-op", Some(json!("eq"))),
        ("filter-0-value", Some(json!("42"))),
        ("filter-0-remove", Some(json!(false))),
        ("filter-add", Some(json!(false))),
        ("filter-apply", Some(json!("Apply"))),
        ("filter-1-column", Some(json!("status"))),
        ("filter-1-op", Some(json!("eq"))),
        ("filter-1-value", Some(json!(""))),
        ("filter-1-unapplied", Some(json!(dbee_ui_egui::filter_bar::UNAPPLIED_HINT))),
        ("filter-1-remove", Some(json!(false))),
    ];
    for (id, value) in &want {
        let e = tag_in(&i, id).unwrap_or_else(|| panic!("`{id}` drawn: {:?}", ids(&i)));
        assert_eq!(e.get("value"), value.as_ref(), "`{id}`'s value: {e}");
        assert_eq!(e.get("enabled"), None, "`{id}` enabled: {e}");
        assert_eq!(ui.rect(id).map(|r| r.height()), Some(28.0), "`{id}` is the page's 28 tall");
    }
    let filter_ids: Vec<String> = ids(&i).into_iter().filter(|id| id.starts_with("filter-")).collect();
    assert_eq!(filter_ids, want.iter().map(|(id, _)| (*id).to_string()).collect::<Vec<_>>(), "no other filter tags, in this order");
    // The open lists: each column with its type, each operator with its label.
    ui.click("filter-0-column");
    ui.frame();
    assert!(ui.value("filter-0-column-status").as_str().is_some_and(|ty| ty.contains("order_status")), "the column's type");
    ui.click("filter-0-column");
    ui.click("filter-0-op");
    ui.frame();
    assert_eq!(ui.value("filter-0-op-gt"), "greater than");
    ui.click("filter-0-op");
    // The raw field and its toggle.
    ui.click("filter-raw-toggle");
    ui.frame();
    assert_eq!((ui.value("filter-raw-toggle"), ui.value("filter-raw"), ui.value("filter-apply")), (json!(true), json!(""), json!("Apply")));
    assert_eq!(ui.rect("filter-raw").map(|r| r.height()), Some(28.0));
}

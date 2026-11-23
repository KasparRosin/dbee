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

/// Review 2, Minor 8: the filter bar's icon buttons read out by what they do, the hover's words,
/// not by their icons (`code`, `minus`, `plus`); the raw toggle says whether it is on.
#[test]
fn the_filter_bars_icon_buttons_read_out_what_they_do() {
    use egui::accesskit::{Role, Toggled};
    use egui_kittest::kittest::{NodeT, Queryable};
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    for words in ["Raw WHERE condition", "Remove this filter", "Add a filter"] {
        assert!(ui.harness.query_by_role_and_label(Role::Button, words).is_some(), "`{words}` read out");
    }
    for icon in ["code", "minus", "plus"] {
        assert!(ui.harness.query_by_label(icon).is_none(), "not `{icon}`");
    }
    let toggled = |ui: &Ui| ui.harness.get_by_role_and_label(Role::Button, "Raw WHERE condition").accesskit_node().toggled();
    assert_eq!(toggled(&ui), Some(Toggled::False), "off");
    ui.click("filter-raw-toggle");
    ui.frame();
    assert_eq!(toggled(&ui), Some(Toggled::True), "on");
}

/// Review 2, Minor 3: a filter's dropdown chosen from with the keyboard closes its list, as after
/// a click: Enter opens it, the arrow takes the keyboard to its first item, Enter chooses it.
#[test]
fn a_filter_dropdown_closes_when_chosen_by_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    for (dropdown, first) in [("filter-0-column", "filter-0-column-id"), ("filter-0-op", "filter-0-op-eq")] {
        let listed = |ui: &mut Ui| ids(&ui.inspect()).into_iter().filter(|id| id.starts_with(&format!("{dropdown}-"))).count();
        ui.focus(dropdown);
        ui.key("Enter");
        ui.frame();
        assert!(listed(&mut ui) > 0, "`{dropdown}`: Enter opens the list");
        ui.key("ArrowDown");
        assert!(ui.focused(first), "`{dropdown}`: the arrow reaches `{first}`");
        ui.key("Enter");
        ui.frame();
        assert_eq!(listed(&mut ui), 0, "`{dropdown}`: the list closes");
    }
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 1], "focus": [0, 1], "whole": false }));
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 2], "focus": [1, 2], "whole": false }), "a right-click selects the cell");
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [90, 1], "focus": [90, 1], "whole": false }));
    assert!(visible(&ui.tag("grid-cell-90-1")));
    assert_eq!(ui.value("grid-cell-90-1")["text"], "payload 91");
    // A column out of view to the right scrolls in too.
    ui.open("shop", "orders");
    let i = ui.inspect();
    assert!(!visible(&tag_in(&i, "grid-cell-5-8")), "raw_json_text starts out of view: {:?}", tag_in(&i, "grid-cell-5-8"));
    ui.click("grid-cell-5-8");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [5, 8], "focus": [5, 8], "whole": false }));
    assert!(visible(&ui.tag("grid-cell-5-8")));
}

#[test]
fn shift_click_drag_and_ctrl_a_select() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.click("grid-cell-1-0");
    ui.click_with("grid-cell-3-2", json!({ "modifiers": { "shift": true } }));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [3, 2], "whole": false }));
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
    assert_eq!(ui.document()["selection"], json!({ "anchor": [4, 1], "focus": [6, 3], "whole": false }));
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
    // A narrow window: the note has the tab's whole width, the value panel being beside the grid.
    let mut ui = connected_to_target(db, &mysql, None, egui::vec2(800.0, 720.0));
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
    assert!(cut, "cut short in a narrow window");
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
    ui.frame();
    let note = ui.value("page-cut");
    assert!(note.as_str().is_some_and(|n| n.contains("13")), "{note}");
    // In the paging bar, after the page's words and the arrows after them.
    let (cut, page, last) = (ui.drawn("page-cut"), ui.drawn("page-number"), ui.drawn("page-last"));
    assert!(cut.left() > page.right() && cut.left() >= last.right(), "the note {cut:?} after the page {page:?} and LAST {last:?}");
    assert!((cut.center().y - page.center().y).abs() <= 2.0, "on the page's line: {cut:?} {page:?}");
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
        ready: bool,
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
        ready: false,
    };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, s: &mut State| {
            // The first frame sets the fonts; the grid draws from the next.
            if !s.ready {
                dbee_ui_egui::theme::apply(ui.ctx());
                s.ready = true;
                return;
            }
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
                saved: 0.0,
                dimmed: false,
            };
            let _ = grid.show(ui, &mut s.grid, &mut s.tags);
        },
        state,
    );
    h.step();
    h.step();
    let mut frames = Vec::new();
    for i in 0..60 {
        // A new selection each frame, as while clicking about.
        h.state_mut().selection = Some(Selection { anchor: (i % 30, i % 20), focus: (i % 30, i % 20), whole: false });
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
            // The filter row has the tab's whole width: the value panel stands beside the grid.
            let panel = ui.tab_x().max;
            let rects: Vec<(&str, egui::Rect)> =
                row.iter().map(|w| (*w, ui.rect(w).unwrap_or_else(|| panic!("{at}, {label}: `{w}` shows")))).collect();
            for (i, (a, ra)) in rects.iter().enumerate() {
                assert!(ra.right() <= panel + 0.5, "{at}, {label}: `{a}` {ra:?} runs past the body at {panel}");
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

/// 1B: in raw mode the toggle is on and the field reads as SQL: a grey `WHERE` before the
/// condition, not part of it; Enter applies the condition as typed.
#[test]
fn raw_mode_reads_as_sql() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    ui.click("filter-raw-toggle");
    ui.frame();
    assert_eq!(ui.value("filter-raw-toggle"), true);
    let fill = dbee_ui_egui::theme::tokens(&ui.harness.ctx).fill;
    let toggle = ui.drawn("filter-raw-toggle");
    let ground = ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill == fill && (r.rect.min - toggle.min).length() < 0.5 && (r.rect.max - toggle.max).length() < 0.5));
    assert!(ground, "the toggle on: its fill ground");
    ui.click("filter-raw");
    ui.text("total > 100");
    ui.frame();
    assert_eq!(ui.value("filter-raw"), "total > 100", "the value is the condition alone");
    assert_eq!(ui.value("filter-raw-where"), "WHERE");
    let (field, prefix) = (ui.drawn("filter-raw"), ui.drawn("filter-raw-where"));
    let muted = dbee_ui_egui::theme::tokens(&ui.harness.ctx).text_muted;
    let texts = ui.texts();
    let (where_at, where_g) = texts.iter().find(|(r, g)| g.text() == "WHERE" && field.contains_rect(*r)).expect("WHERE drawn in the field");
    let (typed, _) = ui.text_in(field, "total > 100").expect("the condition drawn in the field");
    assert!(prefix.contains_rect(*where_at), "`filter-raw-where` {prefix:?} is where WHERE is drawn {where_at:?}");
    assert!(where_at.right() < typed.left(), "WHERE {where_at:?} left of the condition {typed:?}");
    assert_eq!(where_g.job.sections.first().map(|s| s.format.color), Some(muted), "WHERE in text_muted");
    ui.key("Enter");
    ui.until("the raw filter", |ui| ui.table()["applied"]["raw"] == "total > 100" && ui.table()["loading"].is_null());
    assert_eq!(ui.table()["applied"]["raw"], "total > 100");
}

/// 4B: while the connection is lost every control of the filter bar is off, no line under the bar
/// says why, and each control says it on hover.
#[test]
fn a_lost_connection_greys_the_filters_with_the_reason_on_hover() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { exclusive: true, ..Setup::default() });
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    ui.click("filter-add");
    ui.frame();
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(db.server("pg14").unwrap(), sql).unwrap();
    // The next page load finds the connection gone.
    ui.click("table-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.frame();
    let i = ui.inspect();
    let controls: Vec<String> = ids(&i).into_iter().filter(|id| id.starts_with("filter-")).collect();
    assert!(controls.len() >= 10, "the two rows' controls: {controls:?}");
    for id in &controls {
        assert_eq!(tag_in(&i, id).map(|e| e["enabled"].clone()), Some(json!(false)), "`{id}` is off");
    }
    let reason = format!("The filters can't change now: {}.", dbee_engine::cmd::LOST);
    assert!(!ui.texts().iter().any(|(_, g)| g.text().starts_with("The filters can't change now")), "no line under the bar");
    for id in ["filter-0-column", "filter-raw-toggle", "filter-1-value", "filter-apply"] {
        let at = ui.drawn(id).center();
        ui.rest(at);
        assert!(ui.texts().iter().any(|(_, g)| g.text() == reason), "hovering `{id}` says `{reason}`");
    }
}

/// The operator choice in a narrow window (800 wide, the filter row about as wide as at the app's
/// window when the value panel stood beside the whole tab): closed, a long operator is cut with
/// `…` and reads whole on hover; open, its item reads whole.
#[test]
fn the_operator_reads_whole_when_open_in_a_narrow_window() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, egui::vec2(800.0, 720.0));
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    assert_eq!(ui.value("value-toggle"), true, "the value panel is open");
    ui.click("filter-0-op");
    ui.frame();
    ui.click("filter-0-op-ge");
    ui.frame();
    assert_eq!(ui.value("filter-0-op"), "ge");
    let label = "greater than or equal (>=)";
    let closed = ui.drawn("filter-0-op");
    let cut = ui.texts().into_iter().any(|(r, g)| closed.expand(1.0).contains(r.center()) && g.elided);
    assert!(cut, "closed, the operator is cut");
    ui.rest(closed.center());
    assert!(ui.texts().iter().any(|(r, g)| g.text() == label && !g.elided && !closed.contains(r.center())), "hovered, it reads whole");
    ui.click("filter-0-op");
    ui.frame();
    let item = ui.drawn("filter-0-op-ge");
    let (text, cut) = ui.text_in(item, label).expect("the open item reads the operator");
    assert!(!cut && item.expand(0.5).contains_rect(text), "open, `{label}` reads whole: {text:?} in {item:?}");
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

/// Within half a point.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

/// The grid's row numbers start at the tab body's left edge: the number cell of row 0 and the
/// header's corner run from that edge to the first column.
fn assert_starts_at_the_body(ui: &mut Ui, at: &str) {
    let left = ui.body_x().min;
    let (head, cell) = (ui.drawn("grid-header-0"), ui.drawn("grid-cell-0-0"));
    for (what, y) in [("the header's corner", head.center().y), ("row 0's number", cell.center().y)] {
        let drawn = painted_at(ui, egui::pos2(left + 1.0, y));
        let fits = drawn.is_some_and(|(_, r)| close(r.left(), left) && close(r.right(), cell.left()));
        assert!(fits, "{at}: {what} runs from the body's left edge {left} to the first column at {}: {drawn:?}", cell.left());
    }
}

/// The grid sits edge to edge in the tab's body, no margin: its row numbers
/// start at the body's left edge, in the table tab and in a query's results. The table tab's bars
/// run the body's full width too, the filter bar's first control 12 in (TA); so does the query
/// tab's run row.
#[test]
fn the_grid_runs_edge_to_edge() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"].as_u64().is_some_and(|n| n > 0));
    ui.hover_away();
    assert_starts_at_the_body(&mut ui, "the table tab");
    let left = ui.body_x().min;
    let toggle = ui.drawn("filter-raw-toggle");
    assert!(close(toggle.left(), left + 12.0), "the filter bar's first control 12 in from {left}: {toggle:?}");
    ui.execute("tab.newQuery", json!({ "text": "select id, name from shop.customers order by id limit 5" }));
    ui.execute("query.run", json!({}));
    ui.frame();
    ui.hover_away();
    assert_starts_at_the_body(&mut ui, "a query's results");
    let run = ui.drawn("query-run");
    assert!(close(run.left(), left + 12.0), "Run 12 in from {left}: {run:?}");
}

/// The header shows and nothing is drawn under it, no lines either: at the column boundaries, in
/// the columns and past the last one the tab's `bg` shows; the header's `ink` line runs past the
/// last column.
fn assert_header_only(ui: &mut Ui, at: &str) {
    let t = dbee_ui_egui::theme::LIGHT;
    ui.hover_away();
    assert_eq!(ui.ids_with("grid-cell-"), Vec::<String>::new(), "{at}: no rows");
    let (h0, h1, right) = (ui.drawn("grid-header-0"), ui.drawn("grid-header-1"), ui.body_x().max);
    let past = (h1.right() + right) / 2.0;
    for x in [h0.center().x, past] {
        assert_eq!(colour_at(ui, egui::pos2(x, h0.bottom() - 0.5)), Some(t.ink), "{at}: the header's ink line at {x}");
    }
    for x in [h0.left(), h0.center().x, h0.right(), h1.center().x, h1.right(), past] {
        for dy in [1.0, 13.0, 60.0] {
            let p = egui::pos2(x, h0.bottom() + dy);
            assert_eq!(colour_at(ui, p), Some(t.bg), "{at}: nothing under the header at {p:?}: {:?}", painted_at(ui, p));
        }
    }
}

/// An empty result draws its header only: a query with no rows,
/// an empty table and a filter that matches nothing.
#[test]
fn an_empty_result_draws_the_header_only() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_empty", |t| format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, note text);"));
    let mut ui = Ui::connected(db);
    ui.execute("tab.newQuery", json!({ "text": "select 1 as a, 'x' as b where false" }));
    ui.execute("query.run", json!({}));
    ui.frame();
    assert_header_only(&mut ui, "a query with no rows");
    ui.open("scratch", &table.0);
    ui.until("the empty page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"] == 0);
    ui.frame();
    assert_header_only(&mut ui, "an empty table");
    let none = json!([{ "column": "id", "op": "eq", "value": "-1" }]);
    ui.execute("table.open", json!({ "schema": "shop", "table": "customers", "filters": none }));
    ui.until("the filtered page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"] == 0);
    ui.frame();
    assert_header_only(&mut ui, "a filter that matches nothing");
}

/// A scratch table `(id int, a text, b text)` with `rows` rows.
fn three_columns(db: &TestDb, rows: usize) -> Scratch {
    Scratch::new(db, "t_look", |t| {
        let values: Vec<String> = (1..=rows).map(|i| format!("({i}, 'a{i}', 'b{i}')")).collect();
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text, b text); INSERT INTO scratch.{t} VALUES {};", values.join(", "))
    })
}

/// The grounds of row `row` as the last frame drew them, at points clear of text and lines: its
/// number, each of its `cols` cells, and the room past the last column. Each is a ground (taller
/// than a line).
fn row_grounds(ui: &mut Ui, row: usize, cols: usize) -> Vec<(String, Option<egui::Color32>)> {
    let (left, right) = (ui.body_x().min, ui.body_x().max);
    let first = ui.drawn(&format!("grid-cell-{row}-0"));
    let last = ui.drawn(&format!("grid-cell-{row}-{}", cols - 1));
    let y = first.center().y;
    let mut points = vec![("number".to_string(), left + 3.0)];
    points.extend((0..cols).map(|c| (format!("cell {c}"), ui.drawn(&format!("grid-cell-{row}-{c}")).left() + 4.0)));
    points.push(("past the last column".to_string(), (last.right() + right) / 2.0));
    points
        .into_iter()
        .map(|(what, x)| {
            let drawn = painted_at(ui, egui::pos2(x, y));
            assert!(drawn.is_some_and(|(_, r)| r.height() > 20.0), "row {row}, {what}: a ground, not a line: {drawn:?}");
            (what, drawn.map(|(c, _)| c))
        })
        .collect()
}

/// The rows' part of the grid: from under the header to the bottom of the body.
fn rows_area(ui: &mut Ui) -> egui::Rect {
    let x = ui.body_x();
    let head = ui.drawn("grid-header-0");
    egui::Rect::from_min_max(egui::pos2(x.min, head.bottom()), egui::pos2(x.max, head.bottom() + 2000.0))
}

/// One selected cell: its whole row takes `selection`, the number cell and the
/// room past the last column included, and the cell an `ink` outline (the component's width). A
/// marked cell in that row keeps its mark. The other rows stay plain.
#[test]
fn one_cell_selected_lights_its_row_and_outlines_the_cell() {
    let db = dbee_testkit::require_testdb!();
    let table = three_columns(db, 3);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.execute("cell.edit", json!({ "row": 1, "column": "b", "value": "staged" }));
    for (mode, t) in [("ui.themeLight", dbee_ui_egui::theme::LIGHT), ("ui.themeDark", dbee_ui_egui::theme::DARK)] {
        ui.execute(mode, json!({}));
        ui.click("grid-cell-1-1");
        ui.hover_away();
        assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 1], "focus": [1, 1], "whole": false }), "{mode}");
        assert_eq!(ui.value("grid-cell-1-2")["state"], "staged", "{mode}");
        let s = |c| Some(c);
        let want: Vec<_> = [("number", s(t.selection)), ("cell 0", s(t.selection)), ("cell 1", s(t.selection)), ("cell 2", s(t.staged))]
            .into_iter()
            .chain([("past the last column", s(t.selection))])
            .map(|(w, c)| (w.to_string(), c))
            .collect();
        assert_eq!(row_grounds(&mut ui, 1, 3), want, "{mode}: the selected cell's row");
        for row in [0, 2] {
            let plain: Vec<_> = row_grounds(&mut ui, row, 3).into_iter().filter(|(_, c)| *c != Some(t.card)).collect();
            assert_eq!(plain, [], "{mode}: row {row} stays plain");
        }
        let cell = ui.drawn("grid-cell-1-1");
        let area = rows_area(&mut ui);
        let width = dbee_ui_egui::theme::size::GRID_OUTLINE_W;
        assert_eq!(outlines_in(&ui, area, t.ink), [(cell, width)], "{mode}: the one ink outline, on the selected cell");
    }
}

/// Several rows selected across every column: each row takes `selection` from its number to the
/// area's right edge, with no `ink` outline anywhere; a marked cell in them keeps its mark and
/// takes its strong outline (`staged_strong` on a staged cell). The other rows stay plain.
#[test]
fn several_rows_selected_have_no_outline() {
    let db = dbee_testkit::require_testdb!();
    let table = three_columns(db, 4);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 4);
    ui.execute("cell.edit", json!({ "row": 1, "column": "a", "value": "staged" }));
    for (mode, t) in [("ui.themeLight", dbee_ui_egui::theme::LIGHT), ("ui.themeDark", dbee_ui_egui::theme::DARK)] {
        ui.execute(mode, json!({}));
        ui.execute("selection.set", json!({ "anchor": [1, 0], "focus": [2, 2] }));
        ui.hover_away();
        assert_eq!(ui.value("grid-cell-1-1")["state"], "staged", "{mode}");
        for row in [1, 2] {
            let marked = if row == 1 { t.staged } else { t.selection };
            let want: Vec<_> = [("number", t.selection), ("cell 0", t.selection), ("cell 1", marked), ("cell 2", t.selection)]
                .into_iter()
                .chain([("past the last column", t.selection)])
                .map(|(w, c)| (w.to_string(), Some(c)))
                .collect();
            assert_eq!(row_grounds(&mut ui, row, 3), want, "{mode}: selected row {row}");
        }
        for row in [0, 3] {
            let plain: Vec<_> = row_grounds(&mut ui, row, 3).into_iter().filter(|(_, c)| *c != Some(t.card)).collect();
            assert_eq!(plain, [], "{mode}: row {row} stays plain");
        }
        let area = rows_area(&mut ui);
        assert_eq!(outlines_in(&ui, area, t.ink), [], "{mode}: no ink outline");
        let staged = ui.drawn("grid-cell-1-1");
        let width = dbee_ui_egui::theme::size::GRID_OUTLINE_W;
        assert_eq!(outlines_in(&ui, area, t.staged_strong), [(staged, width)], "{mode}: the staged cell's own outline");
    }
}

/// A failed cell has no tooltip: the band above the grid says the message; the cell's red, edge
/// and corner say where. A header of the same grid shows one, so one would show.
#[test]
fn a_failed_cell_has_no_tooltip() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_failed", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, shipped date); INSERT INTO scratch.{t} VALUES (1, '2025-01-02'), (2, '2025-02-03');")
    });
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 2);
    ui.execute("cell.edit", json!({ "row": 1, "column": "shipped", "value": "not a date" }));
    ui.frame();
    ui.click("edit-apply");
    ui.until("the failed Apply", |ui| ui.table()["failed"].is_object());
    ui.frame();
    assert_eq!(ui.value("grid-cell-1-1")["state"], "failed");
    let tooltip = |ui: &Ui| ui.harness.ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.order == egui::Order::Tooltip));
    let head = ui.drawn("grid-header-1").center();
    ui.rest(head);
    assert!(tooltip(&ui), "a header's tooltip shows");
    for id in ["grid-cell-1-1", "grid-cell-1-0"] {
        let at = ui.drawn(id).center();
        ui.rest(at);
        assert!(!tooltip(&ui), "`{id}`, failed: no tooltip");
    }
}

/// The colours the number of row `row` is drawn in.
fn number_colours(ui: &Ui, row: usize) -> Vec<egui::Color32> {
    let at = ui.drawn(&format!("grid-row-{row}"));
    let on_it: Vec<_> =
        ui.shapes().into_iter().filter(|(_, s)| matches!(s, egui::Shape::Text(_)) && at.contains_rect(s.visual_bounding_rect())).collect();
    text_colours(&on_it)
}

/// Which of the first `rows` row numbers say their row is selected.
fn rows_selected(ui: &mut Ui, rows: usize) -> Vec<bool> {
    (0..rows).map(|r| ui.value(&format!("grid-row-{r}"))["selected"] == true).collect()
}

/// `ui.click` params with Shift held.
fn shift() -> Json {
    json!({ "modifiers": { "shift": true } })
}

/// A click on a row's number selects the whole row (#17): `selection` across it from its number to
/// the area's right edge, the number in `text`, no `ink` outline; a marked cell in it keeps its
/// mark and takes its outline (`staged_strong`). The other rows stay plain.
#[test]
fn a_row_number_click_selects_the_row() {
    let db = dbee_testkit::require_testdb!();
    let table = three_columns(db, 4);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 4);
    ui.execute("cell.edit", json!({ "row": 2, "column": "b", "value": "staged" }));
    for (mode, t) in [("ui.themeLight", dbee_ui_egui::theme::LIGHT), ("ui.themeDark", dbee_ui_egui::theme::DARK)] {
        ui.execute(mode, json!({}));
        ui.execute("selection.set", json!({ "anchor": [0, 1] }));
        ui.click("grid-row-2");
        ui.hover_away();
        assert_eq!(ui.document()["selection"], json!({ "anchor": [2, 0], "focus": [2, 2], "whole": true }), "{mode}");
        assert_eq!(ui.value("grid-row-2"), json!({ "number": 3, "selected": true }), "{mode}");
        for row in [0, 1, 3] {
            assert_eq!(ui.value(&format!("grid-row-{row}")), json!({ "number": row + 1, "selected": false }), "{mode}");
        }
        let want: Vec<_> = [("number", t.selection), ("cell 0", t.selection), ("cell 1", t.selection), ("cell 2", t.staged)]
            .into_iter()
            .chain([("past the last column", t.selection)])
            .map(|(w, c)| (w.to_string(), Some(c)))
            .collect();
        assert_eq!(row_grounds(&mut ui, 2, 3), want, "{mode}: the selected row");
        for row in [1, 3] {
            let plain: Vec<_> = row_grounds(&mut ui, row, 3).into_iter().filter(|(_, c)| *c != Some(t.card)).collect();
            assert_eq!(plain, [], "{mode}: row {row} stays plain");
        }
        let area = rows_area(&mut ui);
        assert_eq!(outlines_in(&ui, area, t.ink), [], "{mode}: no ink outline");
        let (staged, width) = (ui.drawn("grid-cell-2-2"), dbee_ui_egui::theme::size::GRID_OUTLINE_W);
        assert_eq!(outlines_in(&ui, area, t.staged_strong), [(staged, width)], "{mode}: the staged cell's own outline");
        assert_eq!(number_colours(&ui, 2), [t.text], "{mode}: the selected row's number");
        assert_eq!(number_colours(&ui, 1), [t.text_muted], "{mode}: another row's number");
    }
}

/// Shift-click on a row number selects the rows from the anchor's row to it, either way; a click
/// on a cell selects that cell again.
#[test]
fn shift_click_extends_the_rows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.until("the page", |ui| ui.table()["rows"] == 100);
    ui.click("grid-row-1");
    ui.click_with("grid-row-3", shift());
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [3, 4], "whole": true }));
    assert_eq!(rows_selected(&mut ui, 5), [false, true, true, true, false]);
    ui.click_with("grid-row-0", shift());
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [0, 4], "whole": true }), "upwards from the anchor's row");
    assert_eq!(rows_selected(&mut ui, 5), [true, true, false, false, false]);
    // From a selected cell: from its row.
    ui.click("grid-cell-4-2");
    ui.click_with("grid-row-6", shift());
    assert_eq!(ui.document()["selection"], json!({ "anchor": [4, 0], "focus": [6, 4], "whole": true }));
    ui.click("grid-cell-5-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [5, 1], "focus": [5, 1], "whole": false }));
    assert_eq!(rows_selected(&mut ui, 7), [false; 7]);
}

/// A selected row copies through today's paths (Ctrl+C, Ctrl+Shift+C and the menu's formats) what
/// the same cells selected across the row copy.
#[test]
fn a_row_copies_as_today() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.until("the page", |ui| ui.table()["rows"] == 100);
    let keys = |ui: &mut Ui| {
        ui.ctrl("C");
        ui.frame();
        let values = ui.copied().expect("Ctrl+C copied");
        ui.ctrl_shift("C");
        ui.frame();
        (values, ui.copied().expect("Ctrl+Shift+C copied"))
    };
    ui.click("grid-row-2");
    let rows = keys(&mut ui);
    ui.execute("selection.set", json!({ "anchor": [2, 0], "focus": [2, 4] }));
    assert_eq!(keys(&mut ui), rows);
    assert!(rows.0.starts_with("3\tCustomer 3\tcustomer3@example.com\t"), "{rows:?}");
    assert!(rows.1.starts_with("id\tname\temail\tcreated_at\tnote\n3\tCustomer 3\t"), "{rows:?}");
    for (item, format) in [("grid-menu-copy-csv", "csv"), ("grid-menu-copy-json", "json"), ("grid-menu-copy-markdown", "markdown")] {
        ui.execute("selection.set", json!({ "anchor": [2, 0], "focus": [2, 4] }));
        let cells = ui.execute("copy.selection", json!({ "format": format }))["text"].clone();
        ui.click("grid-row-2");
        ui.right_click("grid-row-2");
        ui.click(item);
        ui.frame();
        assert_eq!(ui.copied().map(Json::from), Some(cells), "{item}");
        assert_eq!(ui.document()["selection"]["whole"], true, "{item}: the row stays selected");
    }
}

/// The row's menu is the cell's without Set as NULL: a right-click selects the row, unless it is
/// selected already, and Delete row stages it, or every selected row.
#[test]
fn delete_row_from_the_row_menu_stages_it() {
    let db = dbee_testkit::require_testdb!();
    let table = three_columns(db, 4);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 4);
    ui.click("grid-cell-0-1");
    ui.right_click("grid-row-2");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [2, 0], "focus": [2, 2], "whole": true }), "a right-click selects the row");
    for id in ["grid-menu-copy", "grid-menu-copy-header", "grid-menu-copy-csv", "grid-menu-copy-json", "grid-menu-copy-markdown"] {
        assert!(ui.tag(id).is_some(), "{id}");
    }
    assert!(ui.tag("grid-menu-null").is_none(), "no Set as NULL on a row");
    assert_eq!(ui.value("grid-menu-delete")["label"], "Delete row");
    ui.click("grid-menu-delete");
    ui.frame();
    let deleted = |ui: &mut Ui| -> Vec<bool> { (0..4).map(|r| ui.value(&format!("grid-cell-{r}-0"))["deleted"] == true).collect() };
    assert_eq!(deleted(&mut ui), [false, false, true, false]);
    assert!((0..3).all(|c| ui.value(&format!("grid-cell-2-{c}"))["deleted"] == true), "the whole row");
    // Two selected rows: a right-click on one of them keeps both, and deletes both.
    ui.click("grid-row-0");
    ui.click_with("grid-row-1", shift());
    ui.right_click("grid-row-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [1, 2], "whole": true }), "kept");
    assert_eq!(ui.value("grid-menu-delete")["label"], "Delete 2 rows");
    ui.click("grid-menu-delete");
    ui.frame();
    assert_eq!(deleted(&mut ui), [true, true, true, false]);
}

/// Review Focus 2: a row selection is kept when its page changes (a refresh, a filter, the next
/// page) while its rows are still there, and stays a row selection; when they are not, it goes,
/// as a cell selection does.
#[test]
fn a_row_selection_survives_a_refresh_and_fits_a_shorter_page() {
    let db = dbee_testkit::require_testdb!();
    let table = three_columns(db, 4);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the page", |ui| ui.table()["rows"] == 4);
    let row = |r: usize| json!({ "anchor": [r, 0], "focus": [r, 2], "whole": true });
    ui.click("grid-row-2");
    ui.execute("table.refresh", json!({}));
    ui.frame();
    assert_eq!(ui.document()["selection"], row(2), "after a refresh");
    assert_eq!(ui.value("grid-row-2"), json!({ "number": 3, "selected": true }));
    // A filter that leaves three rows: row 2 is still there.
    ui.execute("table.setFilters", json!({ "filters": [{ "column": "id", "op": "ge", "value": "2" }] }));
    ui.until("three rows", |ui| ui.table()["rows"] == 3);
    ui.frame();
    assert_eq!(ui.document()["selection"], row(2), "on the shorter page");
    assert_eq!(rows_selected(&mut ui, 3), [false, false, true]);
    // Rows 1 and 2, and a filter that leaves two: row 2 is gone, so the selection goes.
    ui.click("grid-row-1");
    ui.click_with("grid-row-2", shift());
    ui.execute("table.setFilters", json!({ "filters": [{ "column": "id", "op": "ge", "value": "3" }] }));
    ui.until("two rows", |ui| ui.table()["rows"] == 2);
    ui.frame();
    assert_eq!(ui.document()["selection"], Json::Null);
    assert_eq!(rows_selected(&mut ui, 2), [false, false]);
    // The next page of a long table: the same row of the page, numbered on.
    ui.open("shop", "customers");
    ui.until("the page", |ui| ui.table()["rows"] == 100);
    ui.click("grid-row-2");
    ui.execute("table.page", json!({ "to": "next" }));
    ui.frame();
    assert_eq!(ui.table()["page"], 1);
    assert_eq!(ui.document()["selection"], json!({ "anchor": [2, 0], "focus": [2, 4], "whole": true }));
    assert_eq!(ui.value("grid-row-2"), json!({ "number": 103, "selected": true }));
}

/// The scenes of the checkpoint pictures where the grid's lines show, each in light and dark: the
/// table tab on `shop.orders`, a query's result and an empty result, at the app's first size.
/// `check` gets the window, where it is, and the mode's tokens.
fn line_scenes(db: &TestDb, mut check: impl FnMut(&mut Ui, &str, &dbee_ui_egui::theme::Tokens)) {
    let modes = [("light", "ui.themeLight", dbee_ui_egui::theme::LIGHT), ("dark", "ui.themeDark", dbee_ui_egui::theme::DARK)];
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"].as_u64().is_some_and(|n| n > 0));
    let queries = [
        ("a query's result", "select id, customer_id, dealer_id, status, total, shipped_at, billing from shop.orders order by id limit 100"),
        ("an empty result", "select id, customer_id, status, total, shipped_at from shop.orders where false"),
    ];
    for (n, scene) in ["the table tab", queries[0].0, queries[1].0].into_iter().enumerate() {
        if let Some((_, sql)) = n.checked_sub(1).and_then(|i| queries.get(i)) {
            ui.execute("tab.newQuery", json!({ "text": sql }));
            ui.execute("query.run", json!({}));
            ui.frame();
        }
        for (mode, command, t) in &modes {
            ui.execute(command, json!({}));
            ui.hover_away();
            check(&mut ui, &format!("{scene}, {mode}"), t);
        }
    }
}

/// The header's 1 px `ink` line runs the grid's area unbroken, from the body's left edge to the
/// value panel: no column line crosses it.
#[test]
fn the_headers_ink_line_runs_unbroken() {
    let db = dbee_testkit::require_testdb!();
    line_scenes(db, |ui, at, t| {
        let (x, head) = (ui.body_x(), ui.drawn("grid-header-0"));
        let y = (head.bottom() - 0.5).floor() as i32;
        let pixels = ui.pixels(egui::Rect::from_x_y_ranges(x, head.y_range()));
        // The body's last pixel is where the value panel's line may fall.
        let (from, to) = (x.min.ceil() as i32, x.max.floor() as i32 - 1);
        let broken: Vec<(i32, egui::Color32)> = (from..to).map(|x| (x, pixels.at(x, y))).filter(|(_, c)| *c != t.ink).collect();
        assert_eq!(broken, [], "{at}: the header's ink line along y {y}, x {from} to {to}");
    });
}

/// Every column line is a 1 px `line`, in the header and in the rows. The handle that resizes a
/// column still answers on its boundary.
#[test]
fn column_lines_are_one_pixel_wide() {
    let db = dbee_testkit::require_testdb!();
    line_scenes(db, |ui, at, t| {
        let (x, head) = (ui.body_x(), ui.drawn("grid-header-0"));
        let range = (x.min.ceil() as i32, x.max.floor() as i32 - 1);
        let mut bands = vec![("the header", head)];
        bands.extend(ui.harness.state().tags().rect("grid-cell-0-0").map(|r| ("row 0", r)));
        for (what, band) in bands {
            // Above the band's words.
            let y = (band.top() + 3.0).floor() as i32;
            let runs = ui.pixels(egui::Rect::from_x_y_ranges(x, band.y_range())).runs(y, range, t.line);
            assert!(runs.len() >= 3, "{at}, {what}: the column lines along y {y}: {runs:?}");
            assert!(runs.iter().all(|&(_, n)| n == 1), "{at}, {what}: each column line 1 px wide along y {y} (start, width): {runs:?}");
        }
    });
    // Resizing: on a boundary the pointer is the column-resize one.
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"].as_u64().is_some_and(|n| n > 0));
    let (head, cell) = (ui.drawn("grid-header-1"), ui.drawn("grid-cell-3-1"));
    ui.hover(egui::pos2(head.right(), cell.center().y));
    ui.frame();
    assert_eq!(ui.harness.output().platform_output.cursor_icon, egui::CursorIcon::ResizeColumn, "the boundary resizes the column");
}

/// `n` with a comma between thousands, as the app writes counts.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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

/// The value panel closed, so the table bar has the room of TA's picture at 1100 wide (with the
/// panel open its count's range gives way first).
fn without_the_panel(ui: &mut Ui) {
    panel(ui, false);
}

/// The pointer rests on `id`: whether its hover says `words`.
fn hover_says(ui: &mut Ui, id: &str, words: &str) -> bool {
    let at = ui.drawn(id).center();
    ui.rest(at);
    ui.texts().iter().any(|(_, g)| g.text() == words)
}

/// The tab's body from top to bottom in TA's order: the filter bar, the table bar, the grid, the
/// bottom bar; the bars' grounds run the body's full width; the table bar starts with REFRESH at
/// the body's left, and the bottom bar's view switch starts 12 in.
#[test]
fn the_table_tab_runs_in_tas_order_edge_to_edge() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    ui.hover_away();
    let body = ui.tab_x();
    let order = ["filter-0-column", "table-refresh", "grid-header-0", "table-view-data"];
    let tops: Vec<f32> = order.iter().map(|id| ui.drawn(id).top()).collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "{order:?} top to bottom: {tops:?}");
    assert!(close(ui.drawn("page-first").center().y, ui.drawn("table-view-data").center().y), "the paging in the switch's bar");
    for id in ["filter-0-column", "table-refresh", "page-first"] {
        let y = ui.drawn(id).center().y;
        let ground = painted_at(&ui, egui::pos2(body.min + 2.0, y)).map(|(_, r)| r);
        assert!(
            ground.is_some_and(|r| close(r.left(), body.min) && close(r.right(), body.max)),
            "`{id}`'s bar runs from the body's left {} to its right {}: {ground:?}",
            body.min,
            body.max
        );
    }
    let data = ui.drawn("table-view-data");
    assert!((data.left() - (body.min + 12.0)).abs() <= 1.0, "DATA 12 in from {}: {data:?}", body.min);
    let refresh = ui.drawn("table-refresh");
    assert!(close(refresh.left(), body.min), "REFRESH's cell at the body's left {}: {refresh:?}", body.min);
}

/// The view switch's place: the two segments' rects.
fn switch_at(ui: &Ui) -> (egui::Rect, egui::Rect) {
    (ui.drawn("table-view-data"), ui.drawn("table-view-structure"))
}

/// The view switch stands in the tab's bottom bar in every state of a table tab, at the same
/// pixels in DATA and in STRUCTURE: with one filter row and with three, with the read-only note,
/// with no page loaded, and on a lost connection.
#[test]
fn the_view_switch_stands_in_the_same_place_in_both_views() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    let body = ui.tab_x();
    // The bottom bar's ground, found at the body's right end on the switch's row.
    let foot = |ui: &Ui, switch: egui::Rect| painted_at(ui, egui::pos2(body.max - 2.0, switch.center().y)).expect("the bottom bar's ground").1;
    // Chooses `view` and waits for it to show.
    let choose = |ui: &mut Ui, view: &str| {
        let id = format!("table-view-{view}");
        ui.click(&id);
        ui.until("the view", |ui| ui.value(&id) == true && ui.table()["structure"]["loading"].is_null() && ui.table()["loading"].is_null());
        ui.hover_away();
        ui.frame();
    };
    let mut places = Vec::new();
    // A table with one filter row, then with three; a view, with the read-only note.
    for (table, filters, note) in [("orders", 1, false), ("orders", 3, false), ("order_totals", 1, true)] {
        ui.open("shop", table);
        ui.until("the page", loaded);
        while (0..filters).any(|n| ui.tag(&format!("filter-{n}-column")).is_none()) {
            ui.click("filter-add");
        }
        choose(&mut ui, "data");
        let at = format!("{table}, {filters} filter rows");
        assert_eq!(ui.tag("table-note").is_some(), note, "{at}: the read-only note");
        let data = switch_at(&ui);
        let top = ui.drawn("table-refresh").top();
        choose(&mut ui, "structure");
        let structure = switch_at(&ui);
        assert_eq!(data, structure, "{at}: the switch at the same pixels in DATA and STRUCTURE");
        assert!(ui.drawn("table-refresh").top() < top, "{at}: the table bar itself moved up by the filters' height");
        let switch = data.0.union(data.1);
        assert!(close(switch.left(), body.min + 13.0), "{at}: 12 in from the body's left: {switch:?}");
        let bar = foot(&ui, switch);
        assert!(
            close(bar.height(), 36.0) && close(switch.center().y, bar.top() + 18.5),
            "{at}: centred under the line of the bottom bar {bar:?}: {switch:?}"
        );
        assert!(close(bar.left(), body.min) && close(bar.right(), body.max), "{at}: the bottom bar the body's width: {bar:?}");
        places.push(data);
        choose(&mut ui, "data");
        assert_eq!(switch_at(&ui), data, "{at}: and back in DATA");
    }
    assert!(places.windows(2).all(|w| w[0] == w[1]), "the same place on every tab: {places:?}");
    // Reading order: Tab goes from the switch on to the paging, and Shift+Tab from the switch
    // back to what is above the bar.
    ui.open("shop", "orders");
    choose(&mut ui, "data");
    ui.focus("table-view-structure");
    ui.key("Tab");
    ui.frame();
    let paging = ["page-first", "page-prev", "page-next", "page-last"];
    assert!(paging.iter().any(|id| ui.focused(id)), "Tab from STRUCTURE reaches the paging");
    ui.focus("table-view-data");
    ui.key_with("Tab", json!({ "shift": true }));
    ui.frame();
    let in_bar = ["table-view-data", "table-view-structure", "page-first", "page-prev", "page-next", "page-last"];
    let still: Vec<&str> = in_bar.into_iter().filter(|id| ui.focused(id)).collect();
    assert!(still.is_empty(), "Shift+Tab from DATA leaves the bottom bar, for what is above it: {still:?}");
}

/// The table bar as TA draws it: DATA chosen, STRUCTURE on, IMPORT and EXPORT off, REFRESH and
/// "+ Row" on, the facts with the estimate and COUNT; a click on COUNT counts, and the facts then
/// read the exact count with no button.
#[test]
fn the_table_bar_matches_ta() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    without_the_panel(&mut ui);
    assert_eq!(ui.value("table-view-data"), true);
    assert_eq!(ui.value("table-view-structure"), false);
    for id in ["table-import", "table-export"] {
        assert!(!enabled(&mut ui, id), "`{id}` off until M4");
    }
    for id in ["table-refresh", "row-add", "table-view-data", "table-view-structure"] {
        assert!(enabled(&mut ui, id), "`{id}` on");
    }
    let estimate = ui.value("page-count").as_str().unwrap_or_default().to_string();
    assert!(estimate.starts_with('~') && estimate.ends_with(" rows"), "the estimate: {estimate}");
    assert_eq!(ui.value("table-rows"), format!("1–100 of {estimate}"));
    assert!(!hover_says(&mut ui, "table-view-structure", "Structure: not now, the structure view comes in a later step."), "STRUCTURE on");
    assert!(hover_says(&mut ui, "table-export", dbee_ui_egui::components::table_bars::EXPORT_LATER), "EXPORT says why");
    ui.click("page-count");
    ui.until("the count", |ui| ui.table()["total"]["exact"].is_u64());
    ui.frame();
    let n = ui.table()["total"]["exact"].as_u64().unwrap();
    let rows = format!("{} rows", thousands(n));
    assert_eq!(ui.value("table-rows"), format!("1–100 of {rows}"));
    assert_eq!(ui.value("page-count"), rows.as_str(), "the count's words");
    assert_eq!(ui.rect("page-count"), ui.rect("table-rows"), "no COUNT: `page-count` is the facts");
}

/// A view has no estimate: the facts read the range and COUNT ROWS; + ROW is off and says why.
#[test]
fn the_table_bar_on_a_view() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "order_totals");
    ui.until("the page", loaded);
    without_the_panel(&mut ui);
    assert_eq!(ui.value("table-rows"), "1–100");
    assert_eq!(ui.value("page-count"), "Count rows");
    assert!(!enabled(&mut ui, "row-add"), "+ ROW off on a view");
    assert!(hover_says(&mut ui, "row-add", "Cannot add a row now: views are read-only"), "+ ROW says why");
}

/// An empty table: its one page tells the count, `0 rows`, and there is nothing to count.
#[test]
fn an_empty_table_reads_0_rows() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_none", |t| format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, note text);"));
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("scratch", &table.0);
    ui.until("the empty page", |ui| ui.table()["loading"].is_null() && ui.table()["rows"] == 0);
    ui.frame();
    assert_eq!(ui.value("table-rows"), "0 rows");
    assert_eq!(ui.value("page-count"), "0 rows");
    assert_eq!(ui.rect("page-count"), ui.rect("table-rows"), "no COUNT ROWS");
}

/// Whether the active table tab's page load (or else its count) runs.
fn running(ui: &Ui, count: bool) -> bool {
    let s = ui.harness.state().session().state();
    match s.active_tab() {
        Some(dbee_engine::Tab::Table(t)) => {
            if count {
                t.counting.is_some()
            } else {
                t.loading.is_some()
            }
        }
        _ => false,
    }
}

/// [`Ui::frames_while`] the active tab's load (or count) runs.
fn frames_while_running(ui: &mut Ui, count: bool, secs: f64, check: impl FnMut(&mut Ui, f64)) {
    ui.frames_while(|ui| running(ui, count), secs, check);
}

/// A scratch table of 150 rows, more than a page, so its first page doesn't tell the count; and a
/// window whose frames are 50 ms apart on egui's clock, the value panel closed.
fn held_table(db: &TestDb) -> (Scratch, Ui) {
    let table = three_columns(db, 150);
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), ..Setup::default() });
    without_the_panel(&mut ui);
    (table, ui)
}

/// Review Focus 3: the table bar shows CANCEL and LOADING only once a load has run 300 ms, and
/// each load's 300 ms count from its own start.
#[test]
fn the_loading_state_waits_300_ms_and_restarts_per_load() {
    let db = dbee_testkit::require_testdb!();
    let (table, mut ui) = held_table(db);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    for load in ["first", "second"] {
        let rx = ui.send("engine.execute", json!({ "command": "table.refresh", "params": {} }));
        let mut shown = None;
        frames_while_running(&mut ui, false, 0.45, |ui, gone| {
            let (cancel, refresh, loading) = (ui.rect("table-cancel").is_some(), ui.rect("table-refresh").is_some(), ui.rect("table-loading"));
            if gone <= 0.25 {
                assert!(!cancel && refresh && loading.is_none(), "{load} load at {gone:.2} s: REFRESH, no CANCEL or LOADING yet");
            }
            if gone >= 0.4 {
                assert!(cancel && !refresh && loading.is_some(), "{load} load at {gone:.2} s: CANCEL and LOADING in place of REFRESH");
                shown = Some(gone);
            }
        });
        assert!(shown.is_some(), "{load} load: the loading state showed");
        assert_eq!(ui.value("table-loading"), "Loading");
        assert!(ui.texts().iter().any(|(_, g)| g.text().starts_with("00:00.")), "{load} load: the timer shows");
        ui.click("table-cancel");
        let r = ui.reply(&rx, "the refresh");
        assert_eq!(r["code"], "cancelled", "{load} load: CANCEL ended it cancelled: {r}");
        ui.until("the load's end", |ui| ui.table()["loading"].is_null());
        ui.frame();
        assert!(ui.rect("table-cancel").is_none() && ui.rect("table-refresh").is_some(), "{load} load: REFRESH back");
    }
    drop(lock);
}

/// Whether a spinner (an arc 3 wide) is painted inside `within`: the tab's running mark.
fn spinner_in(ui: &Ui, within: egui::Rect) -> bool {
    ui.shapes().into_iter().any(|(_, s)| match s {
        egui::Shape::Path(p) => p.stroke.width == 3.0 && p.points.iter().all(|q| within.contains(*q)),
        _ => false,
    })
}

/// The alpha the words `words` were painted with inside `within` (the most opaque of their
/// glyphs: a dim multiplies them all), when drawn.
fn alpha_of(ui: &Ui, within: egui::Rect, words: &str) -> Option<u8> {
    ui.shapes().into_iter().find_map(|(_, s)| match s {
        egui::Shape::Text(t) if t.galley.text() == words && within.expand(1.0).contains(t.galley.rect.translate(t.pos.to_vec2()).center()) => {
            t.galley.rows.iter().flat_map(|r| r.row.visuals.mesh.vertices.iter().map(|v| v.color.a())).max()
        }
        _ => None,
    })
}

/// The active tab's strip tab, where its running mark shows, as the last frame drew it. (Read
/// from the window itself: a control request would run frames of its own.)
fn tab_slot(ui: &Ui) -> egui::Rect {
    let tab = ui.harness.state().session().state().active_tab().map(|t| t.id().0).expect("a tab is active");
    ui.drawn(&format!("tab-{tab}"))
}

/// The value `id` was drawn with in the last frame, read from the window itself.
fn drawn_value(ui: &Ui, id: &str) -> Option<Json> {
    ui.harness.state().tags().value(id).cloned()
}

/// `held_table`'s table opened behind a lock that holds its first page: the window, the lock and
/// the table. The open is only sent: the first frame that runs it starts the load, so its 300 ms
/// count from a frame the test sees.
fn first_load_held(db: &TestDb) -> (Scratch, Ui, dbee_testkit::Held) {
    let (table, mut ui) = held_table(db);
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    drop(ui.send("engine.execute", json!({ "command": "table.open", "params": { "schema": "scratch", "table": table.0 }, "wait": false })));
    (table, ui, lock)
}

/// Review Focus 1: on a first load nothing waits on screen before 300 ms (no skeleton, no LOADING,
/// no CANCEL, no running mark); from then the header over ten rows of bars, LOADING, CANCEL and
/// the mark, and never a footer job. The rows end the skeleton.
#[test]
fn every_wait_on_a_first_load_shows_after_300_ms() {
    let db = dbee_testkit::require_testdb!();
    let (_table, mut ui, lock) = first_load_held(db);
    let t = dbee_ui_egui::theme::LIGHT;
    let mut seen = false;
    frames_while_running(&mut ui, false, 0.45, |ui, gone| {
        let slot = tab_slot(ui);
        if gone <= 0.25 {
            for id in ["grid-skeleton", "table-loading", "table-cancel", "status-job"] {
                assert!(ui.rect(id).is_none(), "at {gone:.2} s: no `{id}` yet");
            }
            assert!(!spinner_in(ui, slot), "at {gone:.2} s: no running mark yet");
        }
        if gone >= 0.4 {
            assert_eq!(drawn_value(ui, "grid-skeleton"), Some(json!({ "rows": 10, "columns": 3 })), "at {gone:.2} s");
            let rows = ui.drawn("grid-skeleton");
            assert_eq!(
                colour_at(ui, egui::pos2(rows.left() + 2.0, rows.top() - 0.5)),
                Some(t.ink),
                "at {gone:.2} s: the rows start under the header's ink line"
            );
            assert!(ui.rect("table-loading").is_some() && ui.rect("table-cancel").is_some(), "at {gone:.2} s: LOADING and CANCEL");
            assert!(spinner_in(ui, slot), "at {gone:.2} s: the running mark");
            for id in ["status-job", "grid-header-0", "page-first"] {
                assert!(ui.rect(id).is_none(), "at {gone:.2} s: no `{id}`");
            }
            seen = true;
        }
        assert!(!ui.texts().iter().any(|(_, g)| g.text().starts_with("Loading")), "at {gone:.2} s: no `Loading …` painted");
    });
    assert!(seen, "the skeleton showed");
    drop(lock);
    ui.until("the page", loaded);
    ui.frame();
    assert!(ui.tag("grid-header-0").is_some() && ui.tag("grid-skeleton").is_none(), "the rows in the skeleton's place");
}

/// The skeleton draws only the header's names and types over its bars: no words in the grid's
/// area.
#[test]
fn the_first_load_says_nothing_but_the_bar() {
    let db = dbee_testkit::require_testdb!();
    let (_table, mut ui, lock) = first_load_held(db);
    let mut header = Vec::new();
    frames_while_running(&mut ui, false, 0.4, |ui, gone| {
        if gone < 0.4 {
            return;
        }
        let rows = ui.drawn("grid-skeleton");
        let area = egui::Rect::from_x_y_ranges(rows.x_range(), egui::Rangef::new(rows.top() - 30.0, rows.bottom() + 200.0));
        for (at, g) in ui.texts().into_iter().filter(|(at, _)| area.contains(at.center())) {
            assert!(at.center().y < rows.top(), "only the header has words, not `{}` at {at:?}", g.text());
            header.push(g.text().to_string());
        }
    });
    for name in ["id", "a", "b"] {
        assert!(header.iter().any(|w| w == name), "the header names `{name}`: {header:?}");
    }
    drop(lock);
}

/// Review Focus 3: the columns take the widths the rows give them, as when no skeleton came first.
#[test]
fn the_skeleton_leaves_the_column_widths_to_the_rows() {
    let db = dbee_testkit::require_testdb!();
    let (table, mut ui, lock) = first_load_held(db);
    ui.until("the skeleton", |ui| ui.tag("grid-skeleton").is_some());
    drop(lock);
    ui.until("the page", loaded);
    ui.frame();
    let widths = |ui: &Ui| (0..3).map(|c| ui.drawn(&format!("grid-header-{c}")).width()).collect::<Vec<f32>>();
    let held = widths(&ui);
    let mut plain = Ui::connected_with(db, Setup { size: APP_SIZE, step: Some(0.05), ..Setup::default() });
    without_the_panel(&mut plain);
    plain.open("scratch", &table.0);
    plain.until("the page", loaded);
    plain.frame();
    let unheld = widths(&plain);
    assert!(held.iter().zip(&unheld).all(|(a, b)| close(*a, *b)), "the same widths: {held:?} after a skeleton, {unheld:?} without");
}

/// Review Focus 4: a reload draws the old rows at half from 300 ms (the header at full; a click
/// still selects), and the dim goes the frame the load ends: with a page, cancelled, or failed.
#[test]
fn the_reload_dims_only_while_it_runs() {
    let db = dbee_testkit::require_testdb!();
    let (table, mut ui) = held_table(db);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    ui.frame();
    let server = pg14_server(db);
    let cell_alpha = |ui: &Ui| {
        let at = ui.drawn("grid-cell-0-1");
        alpha_of(ui, at, "a1").expect("row 0's `a` is drawn")
    };
    for end in ["page", "cancel", "failure"] {
        let marker = dbee_testkit::unique("dbee_test_drop");
        let (mut held, dropper) = match end {
            "failure" => {
                // Holds the table, then drops a column the page asks for and commits.
                let script = format!(
                    "SET application_name = '{marker}'; BEGIN; LOCK TABLE scratch.{t} IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(3); ALTER TABLE scratch.{t} DROP COLUMN b; COMMIT;",
                    t = table.0
                );
                let owned = server.clone();
                let dropper = std::thread::spawn(move || dbee_testkit::psql(&owned, &script));
                let asleep = format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = '{marker}' AND wait_event = 'PgSleep'");
                let start = Instant::now();
                while dbee_testkit::psql(server, &asleep).unwrap_or_default().trim() != "1" {
                    assert!(start.elapsed() < Duration::from_secs(20), "the dropping session holds the table");
                    std::thread::sleep(Duration::from_millis(50));
                }
                (None, Some(dropper))
            }
            _ => (Some(dbee_testkit::hold(server, &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap()), None),
        };
        ui.hover_away();
        let rx = ui.send("engine.execute", json!({ "command": "table.refresh", "params": {} }));
        let mut seen = false;
        frames_while_running(&mut ui, false, 0.45, |ui, gone| {
            if gone <= 0.25 {
                assert!(ui.rect("grid-dimmed").is_none(), "{end}: at {gone:.2} s no dim yet");
                assert_eq!(cell_alpha(ui), 255, "{end}: at {gone:.2} s the rows at full");
            }
            if gone >= 0.4 {
                assert_eq!(drawn_value(ui, "grid-dimmed"), Some(json!(true)), "{end}: at {gone:.2} s");
                assert!(cell_alpha(ui).abs_diff(128) <= 2, "{end}: at {gone:.2} s the rows at half: {}", cell_alpha(ui));
                let head = ui.drawn("grid-header-1");
                assert_eq!(alpha_of(ui, head, "a"), Some(255), "{end}: at {gone:.2} s the header at full");
                seen = true;
            }
        });
        assert!(seen, "{end}: the dim showed");
        if end == "page" {
            ui.click("grid-cell-1-1");
            assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 1], "focus": [1, 1], "whole": false }), "a dimmed row still selects");
        }
        match end {
            "page" => drop(held.take()),
            "cancel" => ui.click("table-cancel"),
            _ => {}
        }
        let r = ui.reply(&rx, "the refresh");
        ui.until("the load's end", |ui| ui.table()["loading"].is_null());
        ui.frame();
        assert!(ui.tag("grid-dimmed").is_none(), "{end}: the dim is gone with the load: {r}");
        assert_eq!(cell_alpha(&ui), 255, "{end}: the rows at full");
        if end == "failure" {
            assert!(ui.tag("table-error").is_some(), "the failed load's band: {r}");
        }
        if let Some(dropper) = dropper {
            dropper.join().expect("the dropping session").expect("the column dropped");
        }
        drop(held);
    }
}

/// A count shows COUNTING and its timer in the facts once it has run 300 ms; the footer's job
/// cancels it.
#[test]
fn counting_shows_its_timer_after_300_ms() {
    let db = dbee_testkit::require_testdb!();
    let (table, mut ui) = held_table(db);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    ui.frame();
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    ui.ok("engine.execute", json!({ "command": "table.count", "params": {}, "wait": false }));
    let counting = |ui: &Ui| ui.texts().iter().any(|(_, g)| g.text() == "COUNTING");
    let timer = |ui: &Ui| ui.texts().iter().any(|(_, g)| g.text().starts_with("00:00."));
    let mut seen = false;
    frames_while_running(&mut ui, true, 0.45, |ui, gone| {
        if gone <= 0.25 {
            assert!(!counting(ui) && !timer(ui), "at {gone:.2} s: no COUNTING yet");
        }
        if gone >= 0.4 {
            assert!(counting(ui) && timer(ui), "at {gone:.2} s: COUNTING and its timer");
            seen = true;
        }
    });
    assert!(seen);
    ui.click("status-cancel");
    ui.until("the cancelled count", |ui| ui.table()["counting"].is_null());
    drop(lock);
}

/// One change staged in `null_and_empty`'s table: its cell (1, 1) set to NULL.
fn one_change_staged(ui: &mut Ui) {
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    ui.click("grid-menu-null");
    assert_eq!(ui.table()["edits"]["count"], 1);
    ui.hover_away();
    ui.frame();
}

/// The paging bar as `bars.png` draws it: from 8 in, FIRST, PREVIOUS, the page's words, NEXT and
/// LAST, the arrows 28 square; with a change staged, at the right end `1 change`, RESET and
/// APPLY, APPLY ending 12 from the body's right.
#[test]
fn the_paging_bar_matches_the_page() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    one_change_staged(&mut ui);
    let body = ui.tab_x();
    let left = ["page-first", "page-prev", "page-number", "page-next", "page-last"];
    let xs: Vec<f32> = left.iter().map(|id| ui.drawn(id).left()).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{left:?} left to right: {xs:?}");
    // The paging in the bar's middle, centred on the body's width, right of the view switch.
    let (first, last) = (ui.drawn("page-first"), ui.drawn("page-last"));
    let centre = (first.left() + last.right()) / 2.0;
    assert!((centre - (body.min + body.max) / 2.0).abs() <= 1.0, "the paging centred on the body {body:?}: {first:?} to {last:?}");
    assert!(first.left() > ui.drawn("table-view-structure").right() + 12.0, "right of the view switch: {first:?}");
    for id in ["page-first", "page-prev", "page-next", "page-last"] {
        assert_eq!(ui.drawn(id).size(), egui::vec2(28.0, 28.0), "`{id}` 28 square");
    }
    assert_eq!(ui.value("page-number"), "Page 1 of 1");
    assert_eq!(ui.value("edits-count"), "1 change");
    let right = ["edits-count", "edit-reset", "edit-apply"];
    let xs: Vec<f32> = right.iter().map(|id| ui.drawn(id).left()).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{right:?} left to right: {xs:?}");
    assert!(ui.drawn("edits-count").left() > ui.drawn("page-last").right(), "the changes after the paging");
    let apply = ui.drawn("edit-apply");
    assert!((body.max - 12.0 - apply.right()).abs() <= 1.0, "APPLY ends 12 from the body's right {}: {apply:?}", body.max);
}

/// With a change staged, paging waits, and the arrows say why on hover.
#[test]
fn staged_changes_hold_paging_and_say_why() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    one_change_staged(&mut ui);
    for id in ["page-first", "page-prev", "page-next", "page-last"] {
        assert!(!enabled(&mut ui, id), "`{id}` waits for the staged change");
    }
    assert!(hover_says(&mut ui, "page-next", "Next page: not now, apply or reset your changes first"), "NEXT says why");
}

/// The words `words` drawn inside `within`: where, and in which colour.
fn ink_of(ui: &Ui, within: egui::Rect, words: &str) -> Option<(egui::Rect, egui::Color32)> {
    ui.shapes().into_iter().find_map(|(_, s)| match s {
        egui::Shape::Text(t) if t.galley.text() == words && within.expand(1.0).contains(t.galley.rect.translate(t.pos.to_vec2()).center()) => {
            let section = t.galley.job.sections.first()?;
            let colour = if section.format.color == egui::Color32::PLACEHOLDER { t.fallback_color } else { section.format.color };
            Some((t.galley.rect.translate(t.pos.to_vec2()), t.override_text_color.unwrap_or(colour)))
        }
        _ => None,
    })
}

/// The note on rows that can't be edited is an area of its own above the filter bar: 28 tall on
/// `card` with a `line` under, its sentence 12 in from the body's left.
#[test]
fn the_read_only_note_is_its_own_area() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("shop", "order_totals");
    ui.until("the view's rows", loaded);
    ui.hover_away();
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    assert_eq!(ui.value("table-note"), "These rows can't be edited: views are read-only.");
    let body = ui.tab_x();
    let note = ui.drawn("table-note");
    assert!((note.left() - (body.min + 12.0)).abs() <= 1.0, "the note 12 in from {}: {note:?}", body.min);
    let (ink, area) = painted_at(&ui, egui::pos2(body.min + 2.0, note.center().y)).expect("the note's area");
    assert_eq!(ink, t.card, "the area on card");
    assert!(close(area.height(), 28.0), "28 tall: {area:?}");
    assert!(close(area.left(), body.min) && close(area.right(), body.max), "full width {body:?}: {area:?}");
    assert!((note.center().y - area.center().y).abs() <= 1.0, "the sentence centred down {area:?}: {note:?}");
    let line = painted_at(&ui, egui::pos2(body.min + 2.0, area.bottom() - 0.5)).map(|(c, _)| c);
    assert_eq!(line, Some(t.line), "a line along the area's bottom");
    let (_, colour) = ink_of(&ui, note, "These rows can't be edited: views are read-only.").expect("the sentence is drawn");
    assert_eq!(colour, t.text_muted, "the sentence in text_muted");
    assert!(area.bottom() <= ui.drawn("filter-0-column").top(), "above the filter bar: {area:?}");
}

/// A first load that fails leaves `No rows are loaded.` in `body` `text_muted`, 12 in from the
/// grid area's left and 12 down from its top.
#[test]
fn no_rows_are_loaded_is_restyled() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = Ui::connected_in(db, APP_SIZE);
    dbee_testkit::psql(pg14_server(db), &format!("DROP TABLE scratch.{}", table.0)).unwrap();
    let opened = ui.call("engine.execute", json!({ "command": "table.open", "params": { "schema": "scratch", "table": table.0 } }));
    assert_eq!(opened["ok"], false, "the first page can't load: {opened}");
    ui.until("the failed load", |ui| ui.table()["loading"].is_null() && ui.table()["error"].is_string());
    ui.hover_away();
    assert!(ui.tag("grid-skeleton").is_none(), "no skeleton after a failed first load");
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let body = ui.body_x();
    let everywhere = egui::Rect::from_x_y_ranges(body, egui::Rangef::new(0.0, 10_000.0));
    let (at, colour) = ink_of(&ui, everywhere, "No rows are loaded.").expect("`No rows are loaded.` is drawn");
    assert_eq!(colour, t.text_muted, "in text_muted");
    assert!(close(at.height(), dbee_ui_egui::theme::Text::Body.line()), "in `body`: {at:?}");
    assert!((at.left() - (body.min + 12.0)).abs() <= 1.0, "12 in from {}: {at:?}", body.min);
    // The grid's area starts under the band's line, or else under the table bar.
    let top = match ui.tag("table-error") {
        Some(_) => ui.drawn("table-error").bottom() + 1.0,
        None => painted_at(&ui, egui::pos2(body.min + 2.0, ui.drawn("table-refresh").center().y)).expect("the table bar").1.bottom(),
    };
    assert!((at.top() - (top + 12.0)).abs() <= 1.0, "12 down from the grid area's top {top}: {at:?}");
    assert_eq!(ui.value("table-empty"), "No rows are loaded.", "tagged");
    let tagged = ui.drawn("table-empty");
    assert!((tagged.left() - at.left()).abs() <= 1.0 && (tagged.top() - at.top()).abs() <= 1.0, "the tag on the words: {tagged:?} {at:?}");
    // With no page loaded the bottom bar still stands, with the view switch alone, where it is
    // with a page: 12 in, centred in the 36 over the status bar.
    let whole = ui.tab_x();
    let switch = ui.drawn("table-view-data").union(ui.drawn("table-view-structure"));
    assert!((switch.left() - (whole.min + 13.0)).abs() <= 1.0, "the switch 12 in: {switch:?}");
    let foot = painted_at(&ui, egui::pos2(whole.max - 2.0, switch.center().y)).expect("the bottom bar's ground").1;
    assert!(
        (foot.height() - 36.0).abs() <= 0.5 && (switch.center().y - (foot.top() + 18.5)).abs() <= 1.0,
        "centred in the bottom bar {foot:?}: {switch:?}"
    );
    assert!((foot.left() - whole.min).abs() <= 1.0 && (foot.right() - whole.max).abs() <= 1.0, "the bottom bar the body's width: {foot:?}");
    for id in ["page-first", "page-number", "edit-apply"] {
        assert!(ui.tag(id).is_none(), "no `{id}` without a page");
    }
    assert!(enabled(&mut ui, "table-view-structure"), "STRUCTURE can be chosen");
}

/// The value panel open or shut (through the View menu's command: a narrow bar may leave its
/// toggle out).
fn panel(ui: &mut Ui, on: bool) {
    if ui.execute("ui.toggleValuePanel", json!({}))["visible"] != json!(on) {
        ui.execute("ui.toggleValuePanel", json!({}));
    }
    ui.frame();
    assert!(on || ui.tag("value-panel").is_none(), "the value panel shut");
}

/// The window widths Review Focus 5 sweeps, from wide to narrow.
const SWEEP: [f32; 10] = [1400.0, 1100.0, 900.0, 760.0, 640.0, 520.0, 420.0, 300.0, 200.0, 120.0];

/// Two tagged controls that are one widget: `page-count` is the facts when no COUNT shows; a row
/// left out tags its value field twice.
fn one_widget(a: &str, b: &str) -> bool {
    let pair = |x: &str, y: &str| (a == x && b == y) || (a == y && b == x);
    pair("page-count", "table-rows")
        || (a.ends_with("-value") && b == a.replace("-value", "-unapplied"))
        || (b.ends_with("-value") && a == b.replace("-value", "-unapplied"))
}

/// Every tagged control of the table tab's bars lies inside the tab's body, and no two overlap.
fn assert_nothing_over_anything(ui: &mut Ui, at: &str) {
    assert_none_of_these_overlap(ui, at, &["table-", "filter-", "page-", "row-add", "edit-", "edits-"]);
}

/// Every tagged control whose id starts with one of `prefixes` lies inside the tab's body, and no
/// two overlap (`value-panel`, the panel itself, holds its controls and is left out).
fn assert_none_of_these_overlap(ui: &mut Ui, at: &str, prefixes: &[&str]) {
    let body = ui.tab_x();
    let height = ui.harness.ctx.content_rect().height();
    let ids: Vec<String> = prefixes.iter().flat_map(|p| ui.ids_with(p)).filter(|id| id != "value-panel").collect();
    let drawn: Vec<(String, egui::Rect)> = ui.rects(&ids).into_iter().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
    for (id, r) in &drawn {
        assert!(
            r.left() >= body.min - 0.5 && r.right() <= body.max + 0.5 && r.top() >= 0.0 && r.bottom() <= height + 0.5,
            "{at}: `{id}` {r:?} inside the body {body:?}"
        );
    }
    for (i, (a, ra)) in drawn.iter().enumerate() {
        for (b, rb) in &drawn[i + 1..] {
            let o = ra.intersect(*rb);
            assert!(one_widget(a, b) || !o.is_positive() || o.area() <= 0.5, "{at}: `{a}` {ra:?} over `{b}` {rb:?}");
        }
    }
}

/// [`assert_nothing_over_anything`] at each width of [`SWEEP`], with the value panel shut and then
/// open; the window back at 1400 × 720 with the panel shut after.
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
    ui.harness.set_size(egui::vec2(1400.0, 720.0));
    ui.frame();
    panel(ui, false);
}

/// Review Focus 5: at every width, with a change staged, while a page loads (after 300 ms) and
/// while the rows are counted, no control of the bars is drawn over another or past the body.
#[test]
fn no_bar_draws_one_control_over_another() {
    let db = dbee_testkit::require_testdb!();
    let staged = null_and_empty(db);
    let (held, mut ui) = held_table(db);
    ui.open("scratch", &staged.0);
    ui.until("the page", loaded);
    one_change_staged(&mut ui);
    sweep_widths(&mut ui, "a change staged");
    ui.click("edit-reset");
    ui.open("scratch", &held.0);
    ui.until("the page", loaded);
    ui.frame();
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", held.0)).unwrap();
    ui.ok("engine.execute", json!({ "command": "table.count", "params": {}, "wait": false }));
    frames_while_running(&mut ui, true, 0.4, |_, _| {});
    assert!(ui.texts().iter().any(|(_, g)| g.text() == "COUNTING"), "the counting state shows");
    sweep_widths(&mut ui, "counting");
    ui.click("status-cancel");
    ui.until("the cancelled count", |ui| ui.table()["counting"].is_null());
    ui.ok("engine.execute", json!({ "command": "table.refresh", "params": {}, "wait": false }));
    frames_while_running(&mut ui, false, 0.4, |_, _| {});
    assert!(ui.rect("table-loading").is_some(), "the loading state shows");
    sweep_widths(&mut ui, "loading");
    ui.ok("engine.execute", json!({ "command": "table.cancel", "params": {} }));
    ui.until("the load's end", |ui| ui.table()["loading"].is_null());
    drop(lock);
}

/// The value panel's place in a table tab: the table bar and the paging bar run the body's
/// width, the panel stands between them (its top on the table bar's bottom, its foot on the paging
/// bar's top), the band and the grid beside it; EXPORT and the toggle stay in the table bar, the
/// toggle and APPLY keep their places at the body's right end.
#[test]
fn the_panel_stands_between_the_table_bar_and_the_paging_bar() {
    let db = dbee_testkit::require_testdb!();
    let staged = null_and_empty(db);
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.open("scratch", &staged.0);
    ui.until("the page", loaded);
    panel(&mut ui, true);
    one_change_staged(&mut ui);
    let body = ui.tab_x();
    let at = egui::PanelState::load(&ui.harness.ctx, egui::Id::new("dbee-value-panel")).expect("the panel shows").outer_rect;
    let bar_at = |ui: &Ui, id: &str| {
        let y = ui.drawn(id).center().y;
        painted_at(ui, egui::pos2(body.min + 2.0, y)).unwrap_or_else(|| panic!("the bar under `{id}`")).1
    };
    let (table_bar, paging_bar) = (bar_at(&ui, "table-refresh"), bar_at(&ui, "page-first"));
    for (name, bar) in [("table", table_bar), ("paging", paging_bar)] {
        assert!(
            (bar.left() - body.min).abs() <= 1.0 && (bar.right() - body.max).abs() <= 1.0,
            "the {name} bar runs the body's width {body:?}: {bar:?}"
        );
    }
    assert!((at.top() - table_bar.bottom()).abs() <= 1.0, "the panel's top {at:?} on the table bar's bottom {table_bar:?}");
    assert!((at.bottom() - paging_bar.top()).abs() <= 1.0, "the panel's foot {at:?} on the paging bar's top {paging_bar:?}");
    assert!((at.right() - body.max).abs() <= 0.5, "the panel at the body's right: {at:?}");
    for id in ["table-export", "value-toggle"] {
        let r = ui.drawn(id);
        assert!(table_bar.expand(0.5).contains_rect(r), "`{id}` {r:?} in the table bar {table_bar:?}");
    }
    let toggle = ui.drawn("value-toggle");
    assert!((toggle.right() - body.max).abs() <= 0.5, "the toggle at the body's right: {toggle:?}");
    let apply = ui.drawn("edit-apply");
    assert!(paging_bar.expand(0.5).contains_rect(apply), "APPLY in the paging bar: {apply:?}");
    assert!((apply.right() - (body.max - 12.0)).abs() <= 0.5, "APPLY 12 from the body's right, under the panel: {apply:?}");
    let head = ui.drawn("grid-header-0");
    assert!((head.top() - at.top()).abs() <= 1.0, "the grid's header level with the panel's top: {head:?}, {at:?}");
    assert!(head.right() <= at.left() + 0.5, "the grid beside the panel: {head:?}, {at:?}");
}

/// A scratch table with a short column and a long-named text column holding several lines.
fn long_name(db: &TestDb) -> Scratch {
    Scratch::new(db, "t_name", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY, a_rather_long_column_name_for_a_narrow_value_panel text); \
             INSERT INTO scratch.{t} VALUES (1, E'one\\ntwo\\nthree'), (2, 'x');"
        )
    })
}

/// Review Focus 6: at 1400, 1100, 900, 760, 640 and 520 wide, the panel open and shut, with a
/// value shown, a long name in the panel's bar and a value edited in the panel, no control of the
/// panel or the table tab's bars lies outside the window or over another.
#[test]
fn no_value_panel_control_overlaps_another() {
    const WIDTHS: [f32; 6] = [1400.0, 1100.0, 900.0, 760.0, 640.0, 520.0];
    const IDS: [&str; 7] = ["value-", "table-", "filter-", "page-", "row-add", "edit-", "edits-"];
    let db = dbee_testkit::require_testdb!();
    let table = long_name(db);
    let mut ui = Ui::connected_in(db, egui::vec2(1400.0, 720.0));
    ui.open("scratch", &table.0);
    ui.until("the page", loaded);
    let sweep = |ui: &mut Ui, state: &str| {
        for width in WIDTHS {
            ui.harness.set_size(egui::vec2(width, 720.0));
            ui.frame();
            ui.frame();
            assert_none_of_these_overlap(ui, &format!("{state}, {width} wide"), &IDS);
        }
        ui.harness.set_size(egui::vec2(1400.0, 720.0));
        ui.frame();
    };
    for on in [false, true] {
        panel(&mut ui, on);
        let open = if on { "open" } else { "shut" };
        ui.click("grid-cell-1-0");
        sweep(&mut ui, &format!("a value, the panel {open}"));
        ui.click("grid-cell-0-1");
        sweep(&mut ui, &format!("a long name, the panel {open}"));
    }
    ui.double_click("grid-cell-0-1");
    ui.frame();
    assert_eq!(ui.value("value-panel")["editing"], true, "the panel edits");
    assert!(ui.tag("value-save").is_some(), "SAVE drawn");
    sweep(&mut ui, "editing in the panel");
    ui.key("Escape");
    ui.frame();
    assert_eq!(ui.table()["edits"]["count"], 0, "nothing staged");
}

/// At 640 × 420 with the value panel shut the table bar keeps its controls: the view switch,
/// REFRESH, + ROW and COUNT are drawn whole (REFRESH and + ROW may lose their words).
#[test]
fn at_640_without_the_panel_the_table_bar_keeps_its_buttons() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, egui::vec2(640.0, 420.0));
    ui.open("shop", "orders");
    ui.until("the page", loaded);
    without_the_panel(&mut ui);
    ui.frame();
    let body = ui.tab_x();
    for id in ["table-view-data", "table-refresh", "row-add", "page-count"] {
        let r = ui.rect(id).unwrap_or_else(|| panic!("`{id}` drawn"));
        assert!(r.left() >= body.min && r.right() < body.max - 1.0, "`{id}` {r:?} whole inside the body {body:?}");
        assert!(r.height() >= 26.0 && r.width() >= 26.0, "`{id}` whole: {r:?}");
    }
    let count = ui.drawn("page-count");
    assert!(ui.text_in(count, "COUNT").is_some_and(|(_, cut)| !cut), "`page-count` is COUNT, whole: {count:?}");
}

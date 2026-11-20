//! The schema tree driven through the control channel, one frame at a time in a kittest harness
//! over [`DbeeApp::ui`]: copying a table's name (`clipboard.copyName`) and the right-click menu
//! on a table row (`tree-menu-*`). Assertions read
//! `ui.inspect`, `document.inspect` and the text the frames put on the clipboard.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read.

mod common;

use common::*;
use serde_json::json;

#[test]
fn copy_name_copies_the_bare_name() {
    let db = dbee_testkit::require_testdb!();
    for (schema, table) in [("shop", "orders"), ("odd", "Other Ödd")] {
        // A window each: a second notice waits behind the first.
        let mut ui = Ui::connected(db);
        let done = ui.execute("clipboard.copyName", json!({ "schema": schema, "table": table }));
        assert_eq!(done, json!({ "text": table, "bytes": table.len() }), "{schema}.{table}");
        ui.frame();
        assert_eq!(ui.copied().as_deref(), Some(table), "the name as written, without its schema");
        assert_eq!(ui.value("notice"), json!(format!("Copied {table}")));
    }
}

#[test]
fn copy_name_needs_a_table_the_catalog_has() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    let call = |ui: &mut Ui, params| ui.call("engine.execute", json!({ "command": "clipboard.copyName", "params": params }));
    let r = call(&mut ui, json!({}));
    assert_eq!(r["code"], "badParams", "{r}");
    let r = call(&mut ui, json!({ "schema": "shop", "table": "nope" }));
    assert_eq!(r["code"], "badParams", "{r}");
    assert!(r.to_string().contains("shop.nope"), "the error names the table: {r}");
    ui.frame();
    assert!(ui.inspect()["dialog"].is_null(), "no dialog opened");
    assert_eq!(ui.copied(), None, "nothing copied");
}

#[test]
fn copy_name_without_a_connection_is_disabled() {
    let mut ui = Ui::fresh(egui::vec2(1400.0, 900.0));
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copyName", "params": { "schema": "shop", "table": "orders" } }));
    assert_eq!(r["code"], "disabled", "{r}");
    assert_eq!(r["data"]["why"], "not connected", "{r}");
    let commands = ui.ok("engine.commands", json!({}));
    let c = commands.as_array().unwrap().iter().find(|c| c["id"] == "clipboard.copyName").cloned().unwrap_or_default();
    assert_eq!((c["menu"].clone(), c["enabled"].clone()), (json!([]), json!(false)), "{c}");
}

const MENU_IDS: [&str; 3] = ["tree-menu-open", "tree-menu-new-query", "tree-menu-copy-name"];

/// The `tree-menu-*` items the last frame drew.
fn menu_items(ui: &mut Ui) -> Vec<String> {
    ids(&ui.inspect()).into_iter().filter(|id| id.starts_with("tree-menu-")).collect()
}

/// A right-click on `row`, and a frame for its menu.
fn menu_on(ui: &mut Ui, row: &str) {
    ui.right_click(row);
    ui.frame();
    ui.until("the menu", |ui| menu_items(ui) == MENU_IDS);
}

/// A 1100 × 720 pg14 window with `shop` open in the tree.
fn shop_open(db: &dbee_testkit::TestDb, setup: Setup) -> Ui {
    let mut ui = Ui::connected_with(db, Setup { size: egui::vec2(1100.0, 720.0), ..setup });
    if ui.tag("tree-table-shop.orders").is_none() {
        ui.click("tree-schema-shop");
        ui.frame();
    }
    ui
}

#[test]
fn the_menu_opens_and_copies_from_the_tree() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    menu_on(&mut ui, "tree-table-shop.order_items");
    ui.click("tree-menu-copy-name");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("order_items"));
    assert_eq!(ui.value("notice"), json!("Copied order_items"));
    assert!(menu_items(&mut ui).is_empty(), "the menu closed");
    menu_on(&mut ui, "tree-table-shop.order_items");
    ui.click("tree-menu-new-query");
    ui.until("a query tab", |ui| ui.document()["query"].is_object());
    menu_on(&mut ui, "tree-table-shop.order_items");
    ui.click("tree-menu-open");
    ui.until("the table tab", |ui| ui.table()["name"] == "order_items");
}

#[test]
fn a_right_click_leaves_the_selection_alone() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    let t = dbee_ui_egui::theme::LIGHT;
    ui.click("tree-table-shop.customers");
    ui.frame();
    menu_on(&mut ui, "tree-table-shop.orders");
    let left = |ui: &Ui, id: &str| {
        let r = ui.drawn(id);
        egui::pos2(r.left() + 4.0, r.center().y)
    };
    assert_eq!(colour_at(&ui, left(&ui, "tree-table-shop.customers")), Some(t.selection), "customers stays selected");
    assert_eq!(colour_at(&ui, left(&ui, "tree-table-shop.orders")), Some(t.hover), "the menu's row shows hover");
    // With the pointer in the menu, too.
    let item = ui.centre("tree-menu-new-query");
    ui.hover(item);
    ui.frame();
    assert_eq!(colour_at(&ui, left(&ui, "tree-table-shop.orders")), Some(t.hover), "the menu's row keeps hover");
    ui.key("Escape");
    ui.frame();
    assert!(menu_items(&mut ui).is_empty(), "Escape closes it");
    ui.hover_away();
    assert_ne!(colour_at(&ui, left(&ui, "tree-table-shop.orders")), Some(t.hover), "hover goes with the menu");
    assert_eq!(colour_at(&ui, left(&ui, "tree-table-shop.customers")), Some(t.selection), "customers still selected");
    assert_eq!(ui.document()["tabs"], json!([]), "nothing opened");
    ui.ctrl("T");
    ui.frame();
    assert!(ui.document()["query"].is_object(), "Ctrl+T opens a query tab on the selection");
}

#[test]
fn a_lost_connection_turns_open_off_with_the_reason() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup { exclusive: true, ..Setup::default() });
    ui.open("shop", "customers");
    ui.until("the page", |ui| ui.table()["loading"].is_null());
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(db.server("pg14").unwrap(), sql).unwrap();
    ui.click("table-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.frame();
    let tabs = ui.document()["tabs"].as_array().map(Vec::len);
    menu_on(&mut ui, "tree-table-shop.orders");
    let open = ui.tag("tree-menu-open").unwrap();
    assert_eq!(open["enabled"], json!(false), "{open}");
    assert_eq!(open["value"]["why"], json!(dbee_engine::cmd::LOST), "{open}");
    let copy = ui.tag("tree-menu-copy-name").unwrap();
    assert!(copy.get("enabled").is_none(), "Copy name stays on: {copy}");
    // Its reason on hover.
    let at = ui.centre("tree-menu-open");
    ui.rest(at);
    let hint = format!("Open: not now, {}", dbee_engine::cmd::LOST);
    assert!(ui.texts().iter().any(|(_, g)| g.text() == hint), "hovering Open says `{hint}`");
    ui.click("tree-menu-open");
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), tabs, "no tab opened");
    if menu_items(&mut ui).is_empty() {
        menu_on(&mut ui, "tree-table-shop.orders");
    }
    ui.click("tree-menu-copy-name");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("orders"), "Copy name still copies");
}

#[test]
fn the_menu_acts_on_its_own_row() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    menu_on(&mut ui, "tree-table-shop.orders");
    // A refresh while it is open: the same table stays under it, or it closes.
    let job = ui.execute("schema.refresh", json!({}));
    ui.until("the refresh", |ui| {
        let jobs = ui.document()["jobs"].clone();
        !jobs.as_array().is_some_and(|j| j.iter().any(|j| j["label"] == "schema.refresh"))
    });
    let _ = job;
    ui.frame();
    if menu_items(&mut ui) == MENU_IDS {
        ui.click("tree-menu-copy-name");
        ui.frame();
        assert_eq!(ui.copied().as_deref(), Some("orders"), "the same table");
    }
    // A filter typed while it is open: focusing the field closes it.
    menu_on(&mut ui, "tree-table-shop.orders");
    ui.click("tree-filter");
    ui.frame();
    assert!(menu_items(&mut ui).is_empty(), "a click outside closes it");
    ui.text("order");
    ui.frame();
    ui.until("the filtered tree", |ui| ui.tag("tree-table-shop.customers").is_none());
    menu_on(&mut ui, "tree-table-shop.orders");
    ui.click("tree-menu-open");
    ui.until("the table tab", |ui| ui.table()["name"].is_string());
    assert_eq!((ui.table()["schema"].clone(), ui.table()["name"].clone()), (json!("shop"), json!("orders")), "never the row that took its place");
}

/// The number of tabs open.
fn tab_count(ui: &mut Ui) -> usize {
    ui.document()["tabs"].as_array().map_or(0, Vec::len)
}

#[test]
fn enter_opens_a_focused_table_once() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    let t = dbee_ui_egui::theme::LIGHT;
    let left = |ui: &mut Ui, id: &str| {
        let r = ui.drawn(id);
        egui::pos2(r.left() + 4.0, r.center().y)
    };
    // Space stays a click: it selects, and opens nothing.
    ui.focus("tree-table-shop.orders");
    ui.key("Space");
    ui.frame();
    ui.hover_away();
    let at = left(&mut ui, "tree-table-shop.orders");
    assert_eq!(colour_at(&ui, at), Some(t.selection), "Space selects the row");
    assert_eq!(tab_count(&mut ui), 0, "Space opens nothing");
    // Enter opens it, once.
    ui.key("Enter");
    ui.until("the table tab", |ui| ui.table()["name"] == "orders");
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(tab_count(&mut ui), 1, "one Enter, one tab");
    assert_eq!(ui.table()["schema"], json!("shop"));
    // On a schema row Enter opens or closes it, as before.
    ui.focus("tree-schema-shop");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.value("tree-schema-shop")["open"], json!(false), "Enter closes the schema");
    assert_eq!(tab_count(&mut ui), 1, "and opens nothing");
}

#[test]
fn ctrl_c_copies_what_has_the_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    ui.open("shop", "customers");
    ui.until("the page", |ui| ui.table()["loading"].is_null() && ui.tag("grid-cell-0-1").is_some());
    ui.click("grid-cell-0-1");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Customer 1"), "with the grid's cell clicked, the grid's copy");
    // A click on a table row gives it the keyboard: Ctrl+C copies its name.
    ui.click("tree-table-shop.orders");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("orders"), "the focused row's name");
    // Ctrl+Shift+C stays the grid's copy with its header.
    ui.ctrl_shift("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("name\nCustomer 1"), "the grid's copy with its header");
    // Back on the grid, Ctrl+C is the grid's again.
    ui.click("grid-cell-1-1");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Customer 2"), "a row that lost the keyboard copies nothing");
    // A schema row with the keyboard is not a table: the grid's copy.
    ui.focus("tree-schema-shop");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Customer 2"), "a schema row leaves Ctrl+C to the grid");
    // The filter field keeps its own.
    ui.click("tree-filter");
    ui.text("ord");
    ui.ctrl("A");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("ord"), "the field's own copy");
}

#[test]
fn ctrl_c_in_the_open_menu_copies_its_row() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    ui.click("tree-table-shop.customers");
    ui.frame();
    menu_on(&mut ui, "tree-table-shop.orders");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("orders"), "the menu's row, not the focused one");
    ui.frame();
    assert!(menu_items(&mut ui).is_empty(), "the menu closed");
}

#[test]
fn enter_in_the_open_menu_runs_one_item() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = shop_open(db, Setup::default());
    menu_on(&mut ui, "tree-table-shop.orders");
    ui.focus("tree-menu-copy-name");
    ui.key("Enter");
    ui.frame();
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.copied().as_deref(), Some("orders"), "Enter chose Copy name");
    assert_eq!(tab_count(&mut ui), 0, "and opened nothing");
}

/// The value `id` was drawn with in the last frame, read from the window itself (a control
/// request would run frames of its own and move the clock).
fn drawn_value(ui: &Ui, id: &str) -> Option<serde_json::Value> {
    ui.harness.state().tags().value(id).cloned()
}

/// Whether `id` was drawn in the last frame, read from the window itself.
fn drawn(ui: &Ui, id: &str) -> bool {
    ui.harness.state().tags().rect(id).is_some()
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

/// Whether a tree refresh runs.
fn refreshing(ui: &mut Ui) -> bool {
    ui.harness.state().session().running("schema.refresh")
}

/// A `shop_open` window whose frames are 50 ms apart, and a scratch table opened behind a lock
/// that holds its first page: the browse connection's turn is taken, so a refresh waits behind it.
fn browse_held(db: &dbee_testkit::TestDb) -> (Ui, Scratch, dbee_testkit::Held) {
    let table = Scratch::new(db, "t_tree", |t| format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY)"));
    let mut ui = shop_open(db, Setup { step: Some(0.05), ..Setup::default() });
    let lock = dbee_testkit::hold(pg14_server(db), &format!("LOCK TABLE scratch.{} IN ACCESS EXCLUSIVE MODE", table.0)).unwrap();
    ui.ok("engine.execute", json!({ "command": "table.open", "params": { "schema": "scratch", "table": table.0 }, "wait": false }));
    (ui, table, lock)
}

/// Review Focus 5: a refresh changes nothing for 300 ms; then the count reads LOADING over the
/// rows at half, which still open; when it ends, the count's words at full.
#[test]
fn a_tree_refresh_reads_loading_over_its_rows_at_half() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, _table, lock) = browse_held(db);
    let before = ui.value("tree-count");
    let words = before.as_str().expect("the count's words").to_owned();
    assert!(words.ends_with(" tables"), "{words}");
    let rx = ui.send("engine.execute", json!({ "command": "schema.refresh", "params": {}, "wait": false }));
    let mut seen = false;
    ui.frames_while(refreshing, 0.45, |ui, gone| {
        let orders = ui.drawn("tree-table-shop.orders");
        if gone <= 0.25 {
            assert_eq!(drawn_value(ui, "tree-count"), Some(json!(words)), "at {gone:.2} s: the count as it was");
            assert!(!drawn(ui, "tree-dimmed"), "at {gone:.2} s: no dim yet");
            assert_eq!(alpha_of(ui, orders, "orders"), Some(255), "at {gone:.2} s: the rows at full");
        }
        if gone >= 0.4 {
            assert_eq!(drawn_value(ui, "tree-count"), Some(json!("Loading")), "at {gone:.2} s");
            assert!(ui.texts().iter().any(|(_, g)| g.text() == "LOADING"), "at {gone:.2} s: LOADING painted");
            assert_eq!(drawn_value(ui, "tree-dimmed"), Some(json!(true)), "at {gone:.2} s");
            let a = alpha_of(ui, orders, "orders").expect("orders is drawn");
            assert!(a.abs_diff(128) <= 2, "at {gone:.2} s: the rows at half: {a}");
            seen = true;
        }
    });
    assert!(seen, "LOADING showed");
    ui.reply(&rx, "the refresh's start");
    let tabs = tab_count(&mut ui);
    ui.double_click("tree-table-shop.orders");
    ui.until("an orders tab", |ui| tab_count(ui) == tabs + 1);
    assert!(refreshing(&mut ui), "still held");
    drop(lock);
    ui.until("the refresh's end", |ui| !refreshing(ui));
    ui.frame();
    assert_eq!(ui.value("tree-count").as_str().map(|w| w.ends_with(" tables")), Some(true), "the count's words back");
    assert!(ui.tag("tree-dimmed").is_none(), "no dim");
    let orders = ui.drawn("tree-table-shop.orders");
    assert_eq!(alpha_of(&ui, orders, "orders"), Some(255), "the rows at full");
}

/// LC4: a refresh of a tree with no rows to show draws eight rows of bars once it has run 300 ms.
#[test]
fn a_refresh_of_a_tree_with_nothing_shows_eight_bar_rows() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, _table, lock) = browse_held(db);
    ui.click("tree-filter");
    ui.text("zzz");
    ui.until("no match", |ui| ui.value("tree-count") == "0 tables");
    let rx = ui.send("engine.execute", json!({ "command": "schema.refresh", "params": {}, "wait": false }));
    let mut seen = false;
    ui.frames_while(refreshing, 0.45, |ui, gone| {
        if gone <= 0.25 {
            assert!(!drawn(ui, "tree-skeleton"), "at {gone:.2} s: no bars yet");
            assert_eq!(drawn_value(ui, "tree-count"), Some(json!("0 tables")), "at {gone:.2} s");
        }
        if gone >= 0.4 {
            assert_eq!(drawn_value(ui, "tree-skeleton"), Some(json!(8)), "at {gone:.2} s");
            assert_eq!(drawn_value(ui, "tree-count"), Some(json!("Loading")), "at {gone:.2} s");
            assert!(!drawn(ui, "tree-empty"), "at {gone:.2} s: no sentence");
            seen = true;
        }
    });
    assert!(seen, "the bars showed");
    ui.reply(&rx, "the refresh's start");
    drop(lock);
    ui.until("the refresh's end", |ui| !refreshing(ui));
    ui.frame();
    assert_eq!(ui.value("tree-count"), "0 tables");
    assert!(ui.tag("tree-skeleton").is_none(), "no bars after");
}

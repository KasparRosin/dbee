//! The workspace driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the menu bar, the schema tree, the tabs strip, the status bar and the quick
//! search. Everything goes through control requests and automation ids, the way an agent drives
//! the real window; assertions read `ui.inspect` and `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`).
//!
//! Killing backends: dbee connects with `application_name = 'dbee'`, so terminating its backends
//! hits every dbee connection on the server, also those of the other tests in this binary. So each
//! window holds the shared harness's lock (`common`): shared, or exclusive for the test that kills
//! backends.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use dbee_config::{ConnectionColor, ConnectionConfig, SslMode};
use dbee_core::{Catalog, ColumnInfo, RelKind, SchemaInfo, TableInfo, TableRef, TypeClass};
use dbee_testkit::TestDb;
use dbee_ui_egui::DbeeApp;
use dbee_ui_egui::automation::Tags;
use dbee_ui_egui::tree::{self, Tree};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// A 1200 × 800 window on a throwaway home connected with `config`, on the workspace;
/// `exclusive` for the test that kills backends.
fn connected_to(db: &TestDb, config: ConnectionConfig, exclusive: bool) -> Ui {
    Ui::connected_with(db, Setup { size: egui::vec2(1200.0, 800.0), exclusive, config: Some(config), ..Setup::default() })
}

/// [`connected_to`] the pg14 test server.
fn connected(db: &TestDb) -> Ui {
    connected_to(db, pg14(db), false)
}

impl Ui {
    /// The id of the active tab.
    fn active(&mut self) -> u64 {
        self.document()["active"].as_u64().expect("a tab is active")
    }

    /// Ctrl+P, `query` typed, and Enter on the best match.
    fn quick_choose(&mut self, query: &str) {
        self.ctrl("P");
        self.text(query);
        self.frame();
        self.key("Enter");
    }

    /// The open connection's id, if one is open.
    fn connection_id(&mut self) -> Option<String> {
        self.document()["connection"]["id"].as_str().map(str::to_string)
    }
}

/// A window at the app's first size connected to pg14, with the newest Postgres saved (with the
/// test servers' password) as "pgnew"; and pgnew's id.
fn with_pgnew(db: &TestDb) -> (Ui, String) {
    with_pgnew_held(db, false)
}

/// [`with_pgnew`], the window holding the lock exclusively when `exclusive` (it kills backends).
fn with_pgnew_held(db: &TestDb, exclusive: bool) -> (Ui, String) {
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, exclusive, ..Setup::default() });
    let pgnew = server_config(db.server("pgnew").unwrap());
    let id = ui.execute("connection.save", json!({ "config": pgnew, "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.frame();
    (ui, id)
}

/// A connection switched to with Ctrl+P shows its tree from the top, not scrolled as the last one was.
#[test]
fn the_tree_starts_at_its_top_after_a_switch() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, _) = with_pgnew(db);
    // The SQLite seed's tree is tall enough (315 rows open) to keep an old offset.
    let file = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let config = json!({ "name": "seed file", "kind": "sqlite", "file": file.file().unwrap() });
    ui.execute("connection.save", json!({ "config": config }));
    ui.click("tree-schema-many");
    ui.click("tree-table-many.t_300");
    ui.frame();
    assert!(!visible(&ui.tag("tree-schema-public")), "scrolled down to `many`'s last table");
    ui.quick_choose("seed file");
    ui.until("the file's workspace", |ui| ui.document()["connection"]["kind"] == "sqlite" && ui.document()["screen"] == "workspace");
    ui.frame();
    assert!(visible(&ui.tag("tree-schema-main")), "the first schema row shows: {:?}", ui.tag("tree-schema-main"));
}

#[test]
fn ctrl_p_opens_orders() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("P");
    assert!(visible(&ui.tag("qs-input")), "Ctrl+P opens the quick search");
    ui.text("ord");
    ui.frame();
    let first = ui.value("qs-item-0");
    assert_eq!(first, "shop.orders", "the best match comes first");
    ui.key("Enter");
    ui.until("the table tab", |ui| ui.document()["table"]["name"] == "orders");
    let doc = ui.document();
    assert_eq!(doc["table"]["schema"], "shop", "{doc}");
    let tab = ui.active();
    assert_eq!(ui.value(&format!("tab-{tab}")), "orders", "the tab shows the table's name");
    assert!(ui.tag("qs-input").is_none(), "Enter closes the quick search");
}

#[test]
fn quick_search_arrows_escape_and_its_own_keys() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("P");
    ui.text("order");
    ui.frame();
    let second = ui.value("qs-item-1");
    assert!(second.is_string(), "two matches at least: {second}");
    // Ctrl+T counts as a dialog's key while the quick search is open: no tab.
    ui.ctrl("T");
    assert_eq!(ui.document()["tabs"], json!([]));
    ui.key("ArrowDown");
    ui.key("ArrowDown");
    ui.key("ArrowUp");
    ui.key("Enter");
    ui.until("the table tab", |ui| ui.document()["table"].is_object());
    let doc = ui.document();
    assert_eq!(json!(format!("{}.{}", doc["table"]["schema"].as_str().unwrap(), doc["table"]["name"].as_str().unwrap())), second, "{doc}");
    // Escape closes without opening anything.
    ui.ctrl("P");
    assert!(ui.tag("qs-input").is_some());
    ui.text("customers");
    ui.key("Escape");
    assert!(ui.tag("qs-input").is_none(), "Escape closes the quick search");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    // Clicking a result opens it too.
    ui.ctrl("P");
    ui.text("customers");
    ui.frame();
    ui.click("qs-item-0");
    ui.until("the customers tab", |ui| ui.document()["table"]["name"] == "customers");
}

#[test]
fn closing_the_quick_search_gives_the_focus_back() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("T");
    ui.click("query-editor");
    ui.text("select");
    ui.ctrl("P");
    assert!(ui.tag("qs-input").is_some());
    ui.text("ord");
    ui.key("Escape");
    assert!(ui.tag("qs-input").is_none());
    // The editor has the keyboard again: typing goes on there, and Ctrl+A is its own.
    ui.text(" 1");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 1");
    ui.ctrl("A");
    ui.text("select 2");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 2", "Ctrl+A selected the editor's text");
}

#[test]
fn enter_opens_the_hit_for_the_text_typed_in_the_same_frame() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("P");
    ui.frame();
    // A paste and Enter in one frame: Enter acts on the hits for the pasted text.
    let input = ui.harness.input_mut();
    input.events.push(egui::Event::Paste("customers".into()));
    input.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
    ui.frame();
    ui.until("the customers tab", |ui| ui.document()["table"]["name"] == "customers");
    assert_eq!(ui.document()["table"]["schema"], "shop");
}

#[test]
fn ctrl_p_switches_connection() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, pgnew) = with_pgnew(db);
    let pg14 = ui.connection_id().unwrap();
    ui.ctrl("P");
    ui.text("pg");
    ui.frame();
    // Tables and connections in one list of twenty rows, every row as high and as wide as the
    // others. At this size not all twenty fit: the list scrolls, the rows in view are whole and
    // inside the window, and the others are listed out of view.
    let i = ui.inspect();
    let rows: Vec<Json> = (0..20).filter_map(|n| tag_in(&i, &format!("qs-item-{n}"))).collect();
    assert_eq!(rows.len(), 20, "{rows:?}");
    let rect = |row: &Json| -> Vec<f64> { row["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let x = rect(&rows[0])[0];
    // A row in view is whole, but for the list's top edge rounded to a pixel.
    let whole = |r: &[f64]| r[3] > 27.0 && r[3] <= 28.0;
    for row in &rows {
        let r = rect(row);
        assert!(r[0] == x && r[2] == 560.0 && whole(&r), "{rows:?}");
        assert!(!visible(&Some(row.clone())) || r[1] + r[3] <= f64::from(APP_SIZE.y), "{row}");
    }
    assert!(!visible(&Some(rows[19].clone())), "the twentieth row waits below the list: {rows:?}");
    let pgnew_row = (0..rows.len()).find(|n| tag_in(&i, &format!("qs-item-{n}-kind")).is_some()).expect("a connection hit");
    assert_eq!(rows[pgnew_row]["value"], "pgnew");
    assert_eq!(tag_in(&i, &format!("qs-item-{pgnew_row}-kind")).unwrap()["value"], "PostgreSQL");
    assert!(rows.iter().any(|r| r["value"].as_str().is_some_and(|v| v.starts_with("pg_catalog."))), "table hits too: {rows:?}");
    assert!(!rows.iter().any(|r| r["value"] == "pg14"), "the open connection is left out");
    // Down to the last row: the list scrolls it into view, whole. Down once more stays there.
    let in_view = |ui: &mut Ui, n: usize| {
        let row = ui.tag(&format!("qs-item-{n}"));
        assert!(visible(&row), "row {n}: {row:?}");
        let r = rect(row.as_ref().unwrap());
        assert!(whole(&r) && r[1] + r[3] <= f64::from(APP_SIZE.y), "row {n}: {r:?}");
    };
    for _ in 0..19 {
        ui.key("ArrowDown");
    }
    in_view(&mut ui, 19);
    assert!(!visible(&ui.tag("qs-item-0")), "the first row scrolled away");
    ui.key("ArrowDown");
    in_view(&mut ui, 19);
    assert!(!visible(&ui.tag("qs-item-0")), "Down at the last row stays there");
    // Up to the first row: it scrolls back into view, whole. Up once more stays there.
    for _ in 0..19 {
        ui.key("ArrowUp");
    }
    in_view(&mut ui, 0);
    assert!(!visible(&ui.tag("qs-item-19")), "the last row scrolled away");
    ui.key("ArrowUp");
    in_view(&mut ui, 0);
    assert!(!visible(&ui.tag("qs-item-19")), "Up at the first row stays there");
    ui.key("Escape");
    ui.quick_choose("pgnew");
    ui.until("connected to pgnew", |ui| ui.connection_id().as_deref() == Some(pgnew.as_str()) && ui.document()["screen"] == "workspace");
    assert!(ui.tag("qs-input").is_none(), "the search closed");
    // And back, which now lists pg14.
    ui.quick_choose("pg14");
    ui.until("connected to pg14", |ui| ui.connection_id().as_deref() == Some(pg14.as_str()) && ui.document()["screen"] == "workspace");
}

#[test]
fn switching_with_unsaved_query_text_asks_first() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, pgnew) = with_pgnew(db);
    let pg14 = ui.connection_id().unwrap();
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": "select 1" }));
    ui.quick_choose("pgnew");
    assert_eq!(ui.inspect()["dialog"], "confirm", "the query text would be lost");
    assert_eq!(ui.value("dialog-text"), "Disconnect from pg14? The text of 1 query tab will be lost.");
    ui.click("dialog-cancel");
    for _ in 0..3 {
        ui.frame();
    }
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"]["id"].clone()), (json!("workspace"), json!(pg14)), "Cancel keeps pg14 open: {d}");
    assert_eq!((d["tabs"].as_array().map(Vec::len), d["query"]["text"].clone()), (Some(1), json!("select 1")), "{d}");
    // Close anyway closes pg14 and connects to pgnew.
    ui.quick_choose("pgnew");
    assert_eq!(ui.inspect()["dialog"], "confirm");
    ui.click("dialog-ok");
    ui.until("connected to pgnew", |ui| ui.connection_id().as_deref() == Some(pgnew.as_str()) && ui.document()["screen"] == "workspace");
    assert_eq!(ui.document()["tabs"], json!([]));
}

#[test]
fn switching_with_staged_edits_asks_first_and_cancel_keeps_them() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, _) = with_pgnew(db);
    let pg14 = ui.connection_id().unwrap();
    ui.open("shop", "customers");
    ui.execute("cell.edit", json!({ "row": 0, "column": "name", "value": "Switched" }));
    ui.quick_choose("pgnew");
    assert_eq!(ui.inspect()["dialog"], "confirm", "the staged edits would be lost");
    assert_eq!(ui.value("dialog-text"), "Disconnect from pg14? The staged changes of 1 table tab will be lost.");
    ui.click("dialog-cancel");
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.connection_id(), Some(pg14), "Cancel keeps pg14 open");
    assert_eq!(ui.table()["edits"]["count"], 1, "and the staged edit");
    // The switch was dropped: a later disconnect connects nowhere.
    ui.execute("connection.close", json!({ "force": true }));
    for _ in 0..3 {
        ui.frame();
    }
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"].clone(), d["jobs"].clone()), (json!("connections"), Json::Null, json!([])), "{d}");
}

#[test]
fn ctrl_p_without_a_stored_password_asks_as_a_double_click_does() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, ..Setup::default() });
    let config = ConnectionConfig { name: "pg14 no password".into(), ..pg14(db) };
    let bare = ui.execute("connection.save", json!({ "config": config }))["id"].as_str().unwrap().to_string();
    // The screen, the dialog and the form the connect asks over.
    let asked = |ui: &mut Ui| {
        ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
        let d = ui.document();
        (d["screen"].clone(), d["connection"].clone(), ui.value("dialog-text"), ui.value("dialog-save"), ui.value("conn-name"), ui.value("conn-host"))
    };
    // From the workspace: pg14 closes, and the form connects.
    ui.quick_choose("no password");
    let by_search = asked(&mut ui);
    assert_eq!((by_search.0.clone(), by_search.4.clone()), (json!("connections"), json!("pg14 no password")), "{by_search:?}");
    ui.click("dialog-cancel");
    ui.double_click(&format!("conn-saved-{bare}"));
    assert_eq!(asked(&mut ui), by_search, "a double-click on the card asks the same");
    ui.click("dialog-cancel");
    // On the connection screen.
    ui.quick_choose("no password");
    assert_eq!(asked(&mut ui), by_search);
    ui.text(&db.password);
    ui.click("dialog-ok");
    ui.until("connected", |ui| ui.connection_id().as_deref() == Some(bare.as_str()) && ui.document()["screen"] == "workspace");
}

#[test]
fn ctrl_p_on_the_connection_screen_lists_and_opens_connections() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::window(Setup { size: APP_SIZE, ..Setup::default() });
    ui.execute("connection.save", json!({ "config": ConnectionConfig { name: "Shop live".into(), color: ConnectionColor::Red, ..pg14(db) } }));
    let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.frame();
    ui.ctrl("P");
    assert!(visible(&ui.tag("qs-input")), "Ctrl+P opens the quick search on the connection screen");
    ui.frame();
    // Every saved connection, connections only, in the saved list's order.
    let saved: Vec<Json> = ui.execute("connection.list", json!({}))["saved"].as_array().unwrap().iter().map(|c| c["name"].clone()).collect();
    assert_eq!(saved, [json!("Shop live"), json!("pg14")]);
    assert_eq!(vec![ui.value("qs-item-0"), ui.value("qs-item-1")], saved);
    assert!(ui.tag("qs-item-2").is_none());
    ui.text("zzz");
    ui.frame();
    assert!(ui.texts().iter().any(|(_, g)| g.text() == "No connection matches."));
    ui.key("Escape");
    ui.quick_choose("pg14");
    ui.until("connected to pg14", |ui| ui.connection_id().as_deref() == Some(id.as_str()) && ui.document()["screen"] == "workspace");
}

#[test]
fn a_failed_switch_ends_on_the_form_with_its_error() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, ..Setup::default() });
    ui.ctrl("T");
    // Nothing listens on port 1: the server refuses.
    ui.execute("connection.save", json!({ "config": ConnectionConfig { name: "nowhere".into(), port: 1, ..pg14(db) } }));
    ui.frame();
    ui.quick_choose("nowhere");
    ui.until("the connect's error", |ui| ui.tag("conn-error").is_some());
    let d = ui.document();
    assert_eq!(
        (d["screen"].clone(), d["connection"].clone(), d["tabs"].clone(), d["jobs"].clone()),
        (json!("connections"), Json::Null, json!([]), json!([])),
        "{d}"
    );
    assert_eq!(ui.value("conn-name"), "nowhere", "the form shows the chosen connection");
    assert!(ui.value("conn-error").as_str().is_some_and(|e| !e.is_empty()), "the error in the form's message slot");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
}

#[test]
fn ctrl_p_leaves_a_lost_connection() {
    let db = dbee_testkit::require_testdb!();
    let (mut ui, pgnew) = with_pgnew_held(db, true);
    ui.ctrl("T");
    dbee_testkit::kill_dbee_connections(pg14_server(db)).unwrap();
    // The next database action finds the connection gone.
    ui.click("tree-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    // While lost, only connections are found.
    ui.ctrl("P");
    ui.text("pg");
    ui.frame();
    let i = ui.inspect();
    let rows: Vec<usize> = (0..20).filter(|n| tag_in(&i, &format!("qs-item-{n}")).is_some()).collect();
    assert!(!rows.is_empty() && rows.iter().all(|n| tag_in(&i, &format!("qs-item-{n}-kind")).is_some()), "{:?}", ids(&i));
    ui.key("Escape");
    ui.quick_choose("pgnew");
    ui.until("connected to pgnew", |ui| ui.connection_id().as_deref() == Some(pgnew.as_str()) && ui.document()["screen"] == "workspace");
    let d = ui.document();
    assert_eq!((d["connection"]["lost"].clone(), d["tabs"].clone()), (json!(false), json!([])), "{d}");
}

#[test]
fn a_single_click_in_the_tree_opens_no_tab() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.click("tree-schema-shop");
    ui.click("tree-table-shop.customers");
    for _ in 0..5 {
        ui.frame();
    }
    assert_eq!(ui.document()["tabs"], json!([]), "a single click only selects");
    assert!(!ui.document()["jobs"].as_array().is_some_and(|j| j.iter().any(|j| j["label"] == "table.open")));
}

#[test]
fn ctrl_w_on_a_dirty_tab_asks_first() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": "select 1" }));
    ui.ctrl("W");
    assert_eq!(ui.inspect()["dialog"], "confirm", "the query text would be lost");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    ui.click("dialog-ok");
    assert_eq!(ui.document()["tabs"], json!([]), "Close anyway closes it");
}

#[test]
fn the_quick_search_does_not_outlive_the_workspace() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    let id = ui.document()["connection"]["id"].as_str().unwrap().to_string();
    ui.click("tree-filter");
    ui.text("order");
    ui.ctrl("P");
    assert!(ui.tag("qs-input").is_some());
    // A disconnect the search did not see (an agent's) leaves for the connection screen.
    ui.execute("connection.close", json!({ "force": true }));
    ui.frame();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert!(ui.tag("qs-input").is_none(), "the search closed with the workspace");
    assert_eq!(ui.value("tree-filter"), "", "the tree starts afresh");
    ui.ctrl("T");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1), "shortcuts work again");
}

#[test]
fn tree_filter_narrows_to_matches() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    let i = ui.inspect();
    // `public` (empty in the seed) comes first and open; the other user schemas start closed.
    let first = i["automation"].as_array().unwrap().iter().find(|a| a["id"].as_str().is_some_and(|id| id.starts_with("tree-schema-"))).cloned();
    assert_eq!(first.as_ref().map(|a| a["id"].clone()), Some(json!("tree-schema-public")), "public is listed first");
    assert_eq!(tag_in(&i, "tree-schema-public").unwrap()["value"]["open"], true);
    for closed in ["many", "odd", "shop"] {
        let row = tag_in(&i, &format!("tree-schema-{closed}")).unwrap();
        assert!(visible(&Some(row.clone())) && row["value"]["open"] == false, "{row}");
    }
    assert!(tag_in(&i, "tree-table-shop.orders").is_none(), "a closed schema lists no tables");
    assert!(tag_in(&i, "tree-table-pg_catalog.pg_class").is_none(), "system schemas start collapsed");
    assert!(tag_in(&i, "tree-system").is_some(), "the system schemas' group is listed");
    // Open `shop`: its tables show in view.
    ui.click("tree-schema-shop");
    let i = ui.inspect();
    assert!(visible(&tag_in(&i, "tree-table-shop.orders")), "{:?}", ids(&i));
    assert!(visible(&tag_in(&i, "tree-table-shop.customers")));
    ui.click("tree-filter");
    ui.text("order");
    ui.frame();
    let i = ui.inspect();
    assert!(visible(&tag_in(&i, "tree-table-shop.orders")), "{:?}", ids(&i));
    assert!(tag_in(&i, "tree-table-shop.order_items").is_some());
    assert!(tag_in(&i, "tree-table-shop.customers").is_none(), "{:?}", ids(&i));
    assert_eq!(tag_in(&i, "tree-filter").unwrap()["value"], "order");
    // A system table matches too: its group opens while the filter is typed.
    ui.ctrl("A");
    ui.text("pg_class");
    ui.frame();
    assert!(visible(&ui.tag("tree-table-pg_catalog.pg_class")));
}

#[test]
fn tree_double_click_opens_and_expands_columns() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.click("tree-schema-shop");
    ui.click("tree-expand-shop.orders");
    let i = ui.inspect();
    let id = tag_in(&i, "tree-column-shop.orders.id").expect("the columns show under an expanded table");
    assert_eq!(id["value"]["key"], "primary", "{id}");
    let fk = tag_in(&i, "tree-column-shop.orders.customer_id").expect("customer_id is a column of orders");
    assert_eq!(fk["value"]["key"], "foreign", "{fk}");
    assert!(fk["value"]["type"].as_str().is_some_and(|t| !t.is_empty()), "{fk}");
    ui.click_with("tree-table-shop.customers", json!({ "count": 2 }));
    ui.until("the customers tab", |ui| ui.document()["table"]["name"] == "customers");
    // Odd names open too.
    ui.click("tree-filter");
    ui.text("Ödd");
    ui.frame();
    ui.click_with("tree-table-odd.Other Ödd", json!({ "count": 2 }));
    ui.until("the odd tab", |ui| ui.document()["table"]["name"] == "Other Ödd");
}

/// A type in a schema on the session's search path shows without that schema, as Postgres shows
/// it, in the tree's column rows and in the filter's column list, as in the grid's header.
#[test]
fn the_tree_and_the_filter_show_a_type_as_postgres_does() {
    let db = dbee_testkit::require_testdb!();
    let mood = dbee_testkit::unique("mood");
    let _type = DropType(db, format!("public.{mood}"));
    let table = Scratch::new(db, "t_label", |t| {
        format!(
            "CREATE TYPE public.{mood} AS ENUM ('ok'); CREATE TABLE scratch.{t} (id int PRIMARY KEY, m public.{mood}); INSERT INTO scratch.{t} VALUES (1, 'ok');"
        )
    });
    let mut ui = Ui::connected(db);
    ui.click("tree-schema-scratch");
    ui.click(&format!("tree-expand-scratch.{}", table.0));
    let column = tag_in(&ui.inspect(), &format!("tree-column-scratch.{}.m", table.0)).expect("the column shows under its table");
    assert_eq!(column["value"]["type"], json!(mood), "{column}");
    ui.open("scratch", &table.0);
    ui.click("filter-0-column");
    assert_eq!(ui.value("filter-0-column-m"), json!(mood));
}

/// Drops the type it names on pg14 when the test ends, after the table that uses it.
struct DropType<'a>(&'a TestDb, String);

impl Drop for DropType<'_> {
    fn drop(&mut self) {
        let _ = dbee_testkit::psql(pg14_server(self.0), &format!("DROP TYPE IF EXISTS {} CASCADE", self.1));
    }
}

#[test]
fn tree_rows_out_of_view_scroll_in_when_clicked() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // `many` holds 300 tables: once it is open most are out of view, listed as such.
    ui.click("tree-schema-many");
    let i = ui.inspect();
    let far = tag_in(&i, "tree-table-many.t_290").expect("rows out of view are listed");
    assert_eq!(far["visible"], false, "{far}");
    ui.click_with("tree-table-many.t_290", json!({ "count": 2 }));
    ui.until("the t_290 tab", |ui| ui.document()["table"]["name"] == "t_290");
}

/// The tree's schema rows in the order drawn: `(schema, open)`.
fn schema_rows(i: &Json) -> Vec<(String, bool)> {
    let rows = i["automation"].as_array().unwrap().iter();
    rows.filter_map(|a| Some((a["id"].as_str()?.strip_prefix("tree-schema-")?.to_string(), a["value"]["open"] == true))).collect()
}

/// The catalog's default schema (Postgres `public`, MySQL the database the settings name, SQLite
/// `main`) is listed first and starts open; without one the first in the catalog's order does.
#[test]
fn the_first_schema_is_the_default_one_on_every_engine() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let pg = dbee_testkit::target(pg14_server(db));
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    for (t, database, first) in [(&pg, None, "public"), (&mysql, Some("shop"), "shop"), (&mysql, None, "many"), (&sqlite, None, "main")] {
        let mut ui = connected_to_target(db, t, database, APP_SIZE);
        let rows = schema_rows(&ui.inspect());
        assert_eq!(rows.first(), Some(&(first.to_string(), true)), "{}: {rows:?}", t.name());
        assert!(rows.iter().skip(1).all(|(_, open)| !open), "{}: only the first starts open: {rows:?}", t.name());
    }
}

/// The tree names the level under a connection with the engine's word: databases on MySQL,
/// schemas on Postgres.
#[test]
fn the_tree_says_databases_on_mysql_and_schemas_on_postgres() {
    let db = dbee_testkit::require_testdb!();
    for (server, label) in [("pg14", "System schemas"), ("mysql80", "System databases")] {
        let t = dbee_testkit::target(db.server(server).unwrap());
        let mut ui = connected_to_target(db, &t, None, APP_SIZE);
        // The open schema closed (MySQL's `many` holds 300 tables), so the group is in view.
        if let Some((open, _)) = schema_rows(&ui.inspect()).into_iter().find(|(_, open)| *open) {
            ui.click(&format!("tree-schema-{open}"));
        }
        let group = ui.harness.state().tags().rect("tree-system").expect("the system group is drawn");
        let texts: Vec<String> = ui.texts().iter().filter(|(r, _)| group.contains(r.center())).map(|(_, g)| g.text().to_string()).collect();
        assert!(ui.text_in(group, label).is_some(), "{server}: {texts:?}");
        // The refresh button's hover says the word too.
        let refresh = ui.harness.state().tags().rect("tree-refresh").expect("the refresh button is drawn");
        ui.rest(refresh.center());
        let hover = if server == "pg14" { "Refresh the schemas" } else { "Refresh the databases" };
        assert!(ui.texts().iter().any(|(_, g)| g.text() == hover), "{server}: no `{hover}`");
    }
}

/// The status bar names the server as it reports itself, MariaDB as MariaDB.
#[test]
fn status_bar_names_the_product() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let maria = dbee_testkit::target(db.server("mariadbnew").unwrap());
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    for (t, product) in [(&maria, "MariaDB 1"), (&mysql, "MySQL 8.0"), (&sqlite, "SQLite 3")] {
        let mut ui = connected_to_target(db, t, None, APP_SIZE);
        let server = ui.value("status-server");
        assert!(server.as_str().is_some_and(|s| s.starts_with(product)), "{}: {server}", t.name());
    }
}

#[test]
fn tree_refresh_reloads_the_catalog() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    assert_eq!(ui.tag("tree-refresh").unwrap().get("enabled"), None);
    ui.click("tree-refresh");
    ui.ok("engine.wait", json!({}));
    ui.frame();
    assert!(ui.tag("tree-schema-shop").is_some());
    assert_eq!(ui.document()["connection"]["lost"], false);
}

#[test]
fn disconnect_with_query_text_asks_first() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("T");
    let tab = ui.active();
    assert_eq!(ui.value(&format!("tab-{tab}")), "Query 1");
    ui.click("query-editor");
    ui.text("select 1");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 1", "the editor's text goes to the tab");
    ui.ctrl_shift("Q");
    assert_eq!(ui.inspect()["dialog"], "confirm", "Ctrl+Shift+Q asks first: the query text would be lost");
    ui.click("dialog-cancel");
    let i = ui.inspect();
    assert_eq!((i["dialog"].clone(), i["screen"].clone()), (Json::Null, json!("workspace")), "Cancel keeps the workspace");
    // From the menu it asks the same way, and Close anyway disconnects.
    ui.click("menu-File");
    ui.click("menu-connection.close");
    assert_eq!(ui.inspect()["dialog"], "confirm");
    ui.click("dialog-ok");
    ui.until("the connection screen", |ui| ui.document()["screen"] == "connections");
}

#[test]
fn tabs_open_close_and_mark_filters_and_text() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // Ctrl+T, Ctrl+W.
    ui.ctrl("T");
    ui.ctrl("T");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(2));
    ui.ctrl("W");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    // A filtered table: the marker counts the applied filters.
    let filters = json!([{ "column": "id", "op": "gt", "value": "1" }, { "column": "id", "op": "lt", "value": "100" }]);
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders", "filters": filters }));
    let orders = ui.active();
    assert_eq!(ui.value(&format!("tab-{orders}")), "orders [2]");
    // Typed but not applied: the marker stays.
    ui.execute("table.setFilters", json!({ "filters": [{ "column": "id", "op": "gt", "value": "1" }], "apply": false }));
    assert_eq!(ui.value(&format!("tab-{orders}")), "orders [2]");
    ui.execute("table.setFilters", json!({ "filters": [{ "column": "id", "op": "gt", "value": "1" }] }));
    assert_eq!(ui.value(&format!("tab-{orders}")), "orders [1]");
    ui.execute("table.setFilters", json!({ "filters": [] }));
    assert_eq!(ui.value(&format!("tab-{orders}")), "orders");
    // Clicking a tab activates it; × closes it; a middle click closes it too.
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    let query = tabs[0];
    ui.click(&format!("tab-{query}"));
    assert_eq!(ui.active(), query);
    // A query tab with text shows the dot and asks before closing.
    ui.execute("query.setText", json!({ "text": "select 2" }));
    ui.frame();
    assert!(ui.tag(&format!("tab-dirty-{query}")).is_some(), "a dot marks the unsaved text");
    ui.click(&format!("tab-close-{query}"));
    assert_eq!(ui.inspect()["dialog"], "confirm");
    ui.click("dialog-cancel");
    ui.execute("query.setText", json!({ "text": "" }));
    ui.frame();
    assert!(ui.tag(&format!("tab-dirty-{query}")).is_none());
    ui.click(&format!("tab-close-{query}"));
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    ui.click_with(&format!("tab-{orders}"), json!({ "button": "middle" }));
    assert_eq!(ui.document()["tabs"], json!([]));
    // The + button opens a query tab.
    ui.click("tab-new");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
}

/// The tabs strip as the last frame drew it (the page's `tab strip`, node 163870): a band on
/// `card` that the tabs fill top to bottom, side by side, each with a 1 px `line` at its right
/// end; the `line` along the band's bottom under everything but the active tab (`active`), which
/// has the 2 px `ink` line along its top; no other tab has that line.
fn assert_band(ui: &Ui, active: u64, at: &str) {
    let t = dbee_ui_egui::theme::LIGHT;
    let tags = ui.harness.state().tags();
    let mut tabs: Vec<(u64, egui::Rect)> = tags
        .ids()
        .into_iter()
        .filter_map(|id| id.strip_prefix("tab-")?.parse::<u64>().ok())
        .filter_map(|n| Some((n, tags.rect(&format!("tab-{n}"))?)))
        .collect();
    tabs.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    let tab = tabs.iter().find(|(n, _)| *n == active).map(|(_, r)| *r).unwrap_or_else(|| panic!("{at}: the active tab shows: {tabs:?}"));
    let fills: Vec<(egui::Rect, egui::Color32)> = ui
        .shapes()
        .iter()
        .filter_map(|(clip, s)| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 => Some((r.rect.intersect(*clip), r.fill)),
            _ => None,
        })
        .filter(|(r, _)| r.width() > 0.0 && r.height() > 0.0)
        .collect();
    // The band: the smallest card fill around the tabs in view (bg is the same colour).
    let band = fills
        .iter()
        .filter(|(r, fill)| *fill == t.card && tabs.iter().all(|(_, tab)| r.expand(0.5).contains_rect(*tab)))
        .map(|(r, _)| *r)
        .min_by(|a, b| a.area().total_cmp(&b.area()))
        .unwrap_or_else(|| panic!("{at}: no band in the card fill behind the tabs {tabs:?}"));
    assert_eq!(band.height(), 36.0, "{at}: the band {band:?} is 36 tall");
    for (n, r) in &tabs {
        assert_eq!((r.top(), r.bottom()), (band.top(), band.bottom()), "{at}: tab {n} {r:?} fills the band {band:?}");
        // A tab cut by the band's end is tagged by its part in view.
        let right = egui::Rect::from_min_max(egui::pos2(r.right() - 1.0, r.top()), r.max);
        assert!(r.right() >= band.right() || fills.contains(&(right, t.line)), "{at}: tab {n} {r:?} has the line at its right end");
    }
    for pair in tabs.windows(2) {
        assert_eq!(pair[0].1.right(), pair[1].1.left(), "{at}: the tabs side by side: {tabs:?}");
    }
    // The line along the band's bottom: 1 px fills end to end, but for one gap under the active tab.
    let mut under: Vec<(f32, f32)> = fills
        .iter()
        .filter(|(r, c)| *c == t.line && r.height() == 1.0 && r.bottom() == band.bottom() && band.contains_rect(*r))
        .map(|(r, _)| (r.left(), r.right()))
        .collect();
    under.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut gaps = Vec::new();
    let mut x = band.left();
    for (l, r) in &under {
        if *l > x {
            gaps.push((x, *l));
        }
        x = x.max(*r);
    }
    if band.right() > x {
        gaps.push((x, band.right()));
    }
    assert_eq!(gaps, [(tab.left(), tab.right())], "{at}: the bottom line {under:?} along {band:?} leaves one gap, under the active tab {tab:?}");
    let ink_line = |r: egui::Rect| fills.contains(&(egui::Rect::from_min_size(r.min, egui::vec2(r.width(), 2.0)), t.ink));
    assert!(ink_line(tab), "{at}: the active tab {tab:?} has its ink line along the top");
    for (n, r) in tabs.iter().filter(|(n, _)| *n != active) {
        assert!(!ink_line(*r), "{at}: tab {n} is not active but has the ink line");
    }
}

#[test]
fn the_tabs_sit_on_their_own_band() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders" }));
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    ui.execute("table.open", json!({ "schema": "shop", "table": "customers" }));
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    // The middle one active: a tab on each side of it.
    ui.click(&format!("tab-{}", tabs[1]));
    assert_eq!(ui.active(), tabs[1]);
    ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 600.0)));
    ui.frame();
    ui.frame();
    assert!(ui.tag(&format!("tab-dirty-{}", tabs[1])).is_some(), "the query tab's dot");
    assert_band(&ui, tabs[1], "three tabs, the middle one active");
    // The first one active.
    ui.click(&format!("tab-{}", tabs[0]));
    ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 600.0)));
    ui.frame();
    assert_band(&ui, tabs[0], "three tabs, the first one active");
    // More tabs than fit: the strip scrolls sideways, to the tab just opened. A tab out of view
    // scrolls into view on a click and becomes the active one, and so does the first one again.
    for n in 1..=14 {
        ui.execute("table.open", json!({ "schema": "many", "table": format!("t_{n:03}") }));
    }
    let last = ui.active();
    ui.until("the new tab in view", |ui| visible(&ui.tag(&format!("tab-{last}"))));
    assert!(!visible(&ui.tag(&format!("tab-{}", tabs[0]))), "the first tab is out of view");
    ui.click(&format!("tab-{}", tabs[1]));
    ui.click(&format!("tab-{last}"));
    assert_eq!(ui.active(), last);
    ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 600.0)));
    ui.frame();
    assert!(visible(&ui.tag(&format!("tab-{last}"))), "the last tab is in view");
    assert!(!visible(&ui.tag(&format!("tab-{}", tabs[0]))), "and the first one out of view");
    assert_band(&ui, last, "17 tabs, the last one active");
    ui.click(&format!("tab-{}", tabs[0]));
    assert_eq!(ui.active(), tabs[0]);
    ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 600.0)));
    ui.frame();
    assert!(visible(&ui.tag(&format!("tab-{}", tabs[0]))), "the first tab is back in view");
    assert_band(&ui, tabs[0], "17 tabs, the first one active");
    // + and × keep working.
    let before = ui.document()["tabs"].as_array().map(Vec::len);
    ui.click("tab-new");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), before.map(|n| n + 1), "+ opens a query tab");
    let last = ui.active();
    ui.click(&format!("tab-close-{last}"));
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), before, "× closes it");
}

/// The smallest window: more tabs than fit scroll sideways, the tab made active is scrolled into
/// view, and the strip keeps the page's look (Review Focus 2 and 5).
#[test]
fn many_tabs_scroll_and_the_active_one_shows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, egui::vec2(640.0, 420.0));
    for n in 1..=8 {
        ui.execute("table.open", json!({ "schema": "many", "table": format!("t_{n:03}") }));
    }
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    let (first, last) = (tabs[0], tabs[tabs.len() - 1]);
    ui.until("the last tab in view", |ui| visible(&ui.tag(&format!("tab-{last}"))));
    assert!(!visible(&ui.tag(&format!("tab-{first}"))), "the strip is wider than the window, and scrolled");
    ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(600.0, 400.0)));
    ui.frame();
    assert_band(&ui, last, "640 wide, the last tab active");
    // Made active elsewhere, the first one comes into view.
    ui.execute("tab.activate", json!({ "tab": first }));
    ui.until("the first tab in view", |ui| visible(&ui.tag(&format!("tab-{first}"))));
    assert!(!visible(&ui.tag(&format!("tab-{last}"))), "and the last one out of view");
    ui.frame();
    assert_band(&ui, first, "640 wide, the first tab active");
    // The strip stays in the content: over neither the tree nor the status bar.
    let strip = ui.tag(&format!("tab-{first}")).map(|e| e["rect"][0].as_f64().unwrap() as f32).unwrap();
    let tree = ui.tag("tree-filter").map(|e| e["rect"][0].as_f64().unwrap() as f32 + e["rect"][2].as_f64().unwrap() as f32).unwrap();
    assert!(strip >= tree, "the first tab at {strip} starts after the tree's filter, which ends at {tree}");
}

/// Every tab keeps its tags: `tab-<id>` with its title as written, `tab-dirty-<id>` while closing
/// it would lose something, `tab-close-<id>` on every tab, also those not active, and `tab-new`.
/// The close of a tab that isn't active closes it, and so does a middle click (Review Focus 2, 3).
#[test]
fn tabs_keep_their_tags_and_middle_click_closes() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.execute("table.open", json!({ "schema": "shop", "table": "orders" }));
    let orders = ui.active();
    ui.execute("table.open", json!({ "schema": "odd", "table": "Odd \"Name\" 🐙" }));
    let odd = ui.active();
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    let query = ui.active();
    ui.frame();
    for (tab, title) in [(orders, "orders"), (odd, "Odd \"Name\" 🐙"), (query, "Query 1")] {
        assert_eq!(ui.value(&format!("tab-{tab}")), title, "the tab's title as written");
        let rect = rect_of(&ui.tag(&format!("tab-{tab}")).unwrap());
        assert!(ui.text_in(rect, title).is_some(), "`{title}` drawn in its tab as written");
        assert!(visible(&ui.tag(&format!("tab-close-{tab}"))), "tab {tab} has its close, active or not");
    }
    assert!(ui.tag(&format!("tab-dirty-{query}")).is_some(), "the query text's dot");
    assert!(ui.tag(&format!("tab-dirty-{orders}")).is_none(), "nothing to lose in orders");
    assert!(ui.tag("tab-new").is_some());
    ui.click(&format!("tab-close-{orders}"));
    assert_eq!(ui.active(), query, "closing a tab that isn't active leaves the active one");
    let left: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    assert_eq!(left, [odd, query], "the close of a tab that isn't active closes it");
    ui.click_with(&format!("tab-{odd}"), json!({ "button": "middle" }));
    let left: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    assert_eq!(left, [query], "a middle click closes it");
}

#[test]
fn menu_bar_lists_commands_with_shortcuts_and_reasons() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    for top in ["File", "Edit", "View", "Query"] {
        assert!(ui.tag(&format!("menu-{top}")).is_some(), "menu-{top}");
    }
    ui.click("menu-File");
    let new = ui.tag("menu-tab.newQuery").expect("File lists New Query Tab");
    assert_eq!(new["value"]["shortcut"], "Ctrl+T", "{new}");
    let connect = ui.tag("menu-connection.open").expect("File lists Connect");
    assert_eq!(connect["enabled"], false, "{connect}");
    assert!(connect["value"]["why"].as_str().is_some_and(|w| w.contains("disconnect")), "{connect}");
    ui.click("menu-tab.newQuery");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    assert!(ui.tag("menu-tab.newQuery").is_none(), "a click closes the menu");
    ui.click("menu-Query");
    let run = ui.tag("menu-query.run").expect("Query lists Run");
    assert_eq!(run["value"]["shortcut"], "Ctrl+Enter", "{run}");
}

#[test]
fn status_bar_shows_colour_and_read_only() {
    let db = dbee_testkit::require_testdb!();
    let config = ConnectionConfig { name: "Shop live".into(), color: ConnectionColor::Red, read_only: true, ..pg14(db) };
    let mut ui = connected_to(db, config, false);
    let status = ui.value("status-connection");
    let text = status.as_str().unwrap_or_default().to_string();
    assert!(text.contains("Shop live") && text.contains("read-only"), "{status}");
    let server = ui.value("status-server");
    assert!(server.as_str().is_some_and(|s| s.starts_with("PostgreSQL 14")), "{server}");
    // prefer, and the test server offers TLS: encrypted, its certificate not checked.
    assert_eq!(ui.value("status-tls"), "Encrypted, server not checked");
    assert!(ui.tag("status-reconnect").is_none(), "no Reconnect while the connection is fine");
    assert!(ui.tag("status-save").is_none(), "no Save connection for a saved one");
}

/// The status bar tells a checked certificate from a checked server: `verify-ca` and `require`
/// with a CA file do not check the name, and say so; `verify-full` does.
#[test]
fn the_status_bar_tells_what_was_checked() {
    let db = dbee_testkit::require_testdb!();
    let ca_file = Some(db.ca_file.clone());
    for (ssl_mode, says) in [
        (SslMode::VerifyCa, "Encrypted, certificate checked"),
        (SslMode::Require, "Encrypted, certificate checked"),
        (SslMode::VerifyFull, "Encrypted, server checked"),
    ] {
        let mut ui = connected_to(db, ConnectionConfig { ssl_mode, ca_file: ca_file.clone(), ..pg14(db) }, false);
        assert_eq!(ui.value("status-tls"), says, "{ssl_mode:?}");
    }
}

#[test]
fn a_dropped_database_file_opens_unsaved_and_save_connection_saves_it() {
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = sqlite.file().unwrap();
    let mut ui = Ui::fresh(APP_SIZE);
    // Of several files, the first opens.
    drop_files(&mut ui, &[file, Path::new("/nonexistent/dbee-test/second.txt")]);
    ui.until("the file to open", |ui| ui.document()["screen"] == "workspace");
    let name = file.file_name().unwrap().to_str().unwrap();
    assert_eq!(ui.value("status-connection"), name);
    assert!(ui.tag("status-tls").is_none(), "a file has no link to protect");
    assert_eq!(ui.document()["connection"]["saved"], false);
    assert!(visible(&ui.tag("status-save")), "{:?}", ids(&ui.inspect()));
    assert!(ui.tag("notice").is_none(), "the second file is left alone");
    let bar = |ui: &mut Ui| ["status-connection", "status-server", "value-toggle"].map(|id| ui.tag(id).map(|t| t["rect"].clone()));
    let before = bar(&mut ui);
    ui.click("status-save");
    ui.until("the save", |ui| ui.tag("status-save").is_none());
    assert_eq!(ui.document()["connection"]["saved"], true);
    assert_eq!(bar(&mut ui), before, "the button fits the status bar: nothing moves when it goes");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!((&list["saved"][0]["file"], &list["saved"][0]["id"]), (&json!(file), &ui.document()["connection"]["id"]), "{list}");
}

#[test]
fn a_file_dropped_while_connected_opens_through_the_disconnect_question() {
    let db = dbee_testkit::require_testdb!();
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = sqlite.file().unwrap();
    let mut ui = Ui::connected_with(db, Setup { size: APP_SIZE, ..Setup::default() });
    let pg14 = ui.connection_id().unwrap();
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": "select 1" }));
    // A file dbee would refuse anyway says so, and the connection stays.
    let notes = ui.home.home.join("notes.txt");
    drop_files(&mut ui, &[&notes]);
    ui.frame();
    assert!(ui.value("notice").as_str().is_some_and(|n| n.starts_with("Can't open notes.txt")), "{:?}", ui.value("notice"));
    assert_eq!((ui.inspect()["dialog"].clone(), ui.connection_id()), (Json::Null, Some(pg14.clone())));
    // A database file asks first, as a switch does: Cancel keeps the connection and the text.
    drop_files(&mut ui, &[file]);
    ui.frame();
    assert_eq!(ui.inspect()["dialog"], "confirm", "the query text would be lost");
    assert_eq!(ui.value("dialog-text"), "Disconnect from pg14? The text of 1 query tab will be lost.");
    ui.click("dialog-cancel");
    for _ in 0..3 {
        ui.frame();
    }
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"]["id"].clone()), (json!("workspace"), json!(pg14)), "Cancel keeps pg14 open: {d}");
    assert_eq!((d["tabs"].as_array().map(Vec::len), d["query"]["text"].clone()), (Some(1), json!("select 1")), "{d}");
    // Disconnect anyway: pg14 closes and the file opens.
    drop_files(&mut ui, &[file]);
    ui.frame();
    assert_eq!(ui.inspect()["dialog"], "confirm");
    ui.click("dialog-ok");
    ui.until("the file to open", |ui| ui.document()["connection"]["kind"] == "sqlite" && ui.document()["screen"] == "workspace");
    assert_eq!(ui.value("status-connection"), file.file_name().unwrap().to_str().unwrap());
    assert_eq!(ui.document()["tabs"], json!([]));
}

#[test]
fn a_dropped_file_of_no_type_dbee_opens_says_which_do() {
    let mut ui = Ui::fresh(APP_SIZE);
    let notes = ui.home.home.join("notes.txt");
    drop_files(&mut ui, &[&notes]);
    ui.frame();
    let notice = ui.value("notice");
    let text = notice.as_str().unwrap_or_default();
    assert!(text.starts_with("Can't open notes.txt: dbee opens ") && text.contains(".sqlite"), "{notice}");
    assert!(text.ends_with("To open a file with another name, use New connection and choose SQLite."), "{notice}");
    assert_eq!(ui.document()["screen"], "connections");
}

#[test]
fn the_file_the_command_line_names_opens_once_the_window_is_up() {
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = sqlite.file().unwrap().to_path_buf();
    let with_file = |path: PathBuf| -> MakeApp { Box::new(move |session, rx| DbeeApp::new(session, Some(rx)).with_file(path)) };
    let mut ui = Ui::window(Setup { size: APP_SIZE, app: Some(with_file(file.clone())), ..Setup::default() });
    ui.until("the file to open", |ui| ui.document()["screen"] == "workspace");
    assert_eq!(ui.document()["connection"]["saved"], false);
    assert!(visible(&ui.tag("status-save")));
    // One that is not there: the plain error, and the window goes on.
    let gone = ui.home.home.join("gone.sqlite");
    drop(ui);
    let mut ui = Ui::window(Setup { size: APP_SIZE, app: Some(with_file(gone.clone())), ..Setup::default() });
    ui.until("the error", |ui| ui.tag("notice").is_some());
    let notice = ui.value("notice");
    assert!(notice.as_str().is_some_and(|n| n.ends_with(&format!(": no file at {}.", gone.display()))), "{notice}");
    assert_eq!(ui.document()["screen"], "connections");
}

#[test]
fn status_bar_lists_only_live_jobs() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.execute("tab.newQuery", json!({ "text": "select pg_sleep(30)" }));
    let tab = ui.active();
    ui.ok("engine.execute", json!({ "command": "query.run", "wait": false }));
    ui.frame();
    assert!(ui.value("status-job").as_str().is_some_and(|s| s.contains("Query 1")), "the running query shows");
    assert!(ui.tag("status-cancel").is_some());
    ui.click("status-cancel");
    ui.until("the cancel", |ui| ui.document()["query"]["running"].is_null());
    ui.frame();
    assert!(ui.tag("status-job").is_none(), "no spinner once the run ended");
    // A tab closed while its job runs leaves no spinner behind.
    ui.ok("engine.execute", json!({ "command": "query.run", "wait": false }));
    ui.frame();
    assert!(ui.tag("status-job").is_some());
    ui.execute("tab.close", json!({ "tab": tab, "force": true }));
    ui.frame();
    assert!(ui.tag("status-job").is_none(), "{:?}", ids(&ui.inspect()));
}

#[test]
fn lost_connection_offers_reconnect() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected_to(db, pg14(db), true);
    let server = db.server("pg14").unwrap();
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(server, sql).unwrap();
    // The next database action finds the connection gone.
    ui.click("tree-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.frame();
    let lost = ui.value("status-lost");
    assert_eq!(lost, "Connection lost");
    assert_eq!(ui.tag("tree-refresh").unwrap()["enabled"], false, "the tree cannot refresh while lost");
    ui.click("status-reconnect");
    ui.until("the reconnect", |ui| ui.document()["connection"]["lost"] == false);
    ui.frame();
    assert!(ui.tag("status-reconnect").is_none());
    assert!(ui.tag("status-lost").is_none());
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

/// A catalog of `n` tables of 10 columns each in 10 schemas, plus the system schemas.
fn synthetic(n: usize) -> Arc<Catalog> {
    let column = |i: usize| ColumnInfo {
        name: format!("column_{i}"),
        type_sql: "integer".into(),
        base_type: "int4".into(),
        class: TypeClass::Number,
        not_null: i == 0,
        has_default: false,
        generated: Default::default(),
        hidden: false,
        type_label: String::new(),
        operators: dbee_core::Operators::Unnamed,
    };
    let table = |schema: &str, name: String| TableInfo {
        table: TableRef { schema: schema.into(), name },
        kind: RelKind::Table,
        columns: (0..10).map(column).collect(),
        primary_key: vec!["column_0".into()],
        unique_keys: vec![],
        foreign_keys: vec![],
        estimated_rows: Some(1000),
        no_edit_reason: None,
    };
    let mut schemas: Vec<SchemaInfo> = (0..10)
        .map(|s| {
            let name = format!("schema_{s}");
            let tables = (0..n / 10).map(|t| table(&name, format!("table_{s}_{t:04}"))).collect();
            SchemaInfo { name, system: false, tables }
        })
        .collect();
    for sys in ["information_schema", "pg_catalog"] {
        schemas.push(SchemaInfo { name: sys.into(), system: true, tables: (0..60).map(|t| table(sys, format!("pg_thing_{t}"))).collect() });
    }
    Arc::new(Catalog { schemas, default_schema: None })
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn tree_filter_1000_tables_within_a_frame() {
    struct State {
        catalog: Arc<Catalog>,
        tree: Tree,
        tags: Tags,
    }
    let state = State { catalog: synthetic(1000), tree: Tree::default(), tags: Tags::default() };
    let mut h = Harness::builder().with_size(egui::vec2(320.0, 900.0)).build_ui_state(
        |ui, s: &mut State| {
            s.tags.begin_frame();
            let _ = tree::show(ui, &s.catalog, dbee_sql::Dialect::Postgres, &mut s.tree, &mut s.tags);
        },
        state,
    );
    h.step();
    // Every keystroke changes the filter: each frame filters 1,000 tables anew.
    let filters = ["t", "ta", "tab", "table_", "table_5", "table_5_", "table_5_00", "table_5_0", "table_", "able"];
    let mut typing = Vec::new();
    for _ in 0..10 {
        for f in filters {
            h.state_mut().tree.filter = f.to_string();
            let start = Instant::now();
            h.step();
            typing.push(start.elapsed());
        }
    }
    // The filter as typed, frames after it: the cached rows are drawn.
    let mut steady = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        h.step();
        steady.push(start.elapsed());
    }
    typing.sort();
    steady.sort();
    let median = |v: &[Duration]| v[v.len() / 2];
    let shown = h.state().tags.ids().iter().filter(|id| id.starts_with("tree-table-")).count();
    println!(
        "1,000 tables: frame with a new filter median {:?} (max {:?}), frame with the filter unchanged median {:?}; {shown} table rows listed; on {}",
        median(&typing),
        typing.last().unwrap(),
        median(&steady),
        machine()
    );
    assert!(median(&typing) < Duration::from_millis(16), "{:?}", median(&typing));
    assert!(median(&steady) < Duration::from_millis(16), "{:?}", median(&steady));
}

#[test]
fn a_new_tab_is_scrolled_into_view_when_the_strip_overflows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // More tabs than the 1200-point window has room for.
    for _ in 0..14 {
        ui.execute("table.open", json!({ "schema": "shop", "table": "customer_counts" }));
    }
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    let (first, last) = (tabs[0], tabs[tabs.len() - 1]);
    assert_eq!(ui.active(), last);
    ui.until("the new tab in view", |ui| visible(&ui.tag(&format!("tab-{last}"))));
    assert!(!visible(&ui.tag(&format!("tab-{first}"))), "the strip is wider than the window, and scrolled");
    // A tab made active elsewhere comes into view too.
    ui.execute("tab.activate", json!({ "tab": first }));
    ui.until("the first tab in view", |ui| visible(&ui.tag(&format!("tab-{first}"))));
}

// ---- menus and popup lists (`components::menu`) ------------------------------------------------

/// The rect of a `ui.inspect` entry.
fn rect_of(e: &Json) -> egui::Rect {
    let f = |i: usize| e["rect"][i].as_f64().unwrap() as f32;
    egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
}

/// Entry `id` of `i` has `value`, is enabled or not, and is the page's 28 tall menu item.
fn assert_item(i: &Json, id: &str, value: &Json, enabled: bool, at: &str) {
    let e = tag_in(i, id).unwrap_or_else(|| panic!("{at}: `{id}` is listed: {:?}", ids(i)));
    assert_eq!(&e["value"], value, "{at}: `{id}`'s value");
    assert_eq!(e.get("enabled") != Some(&json!(false)), enabled, "{at}: `{id}` enabled: {e}");
    assert_eq!(rect_of(&e).height(), 28.0, "{at}: `{id}` is the page's 28 tall item: {e}");
}

/// The colour `words` are drawn in within `within` in the last frame.
fn colour_of(ui: &Ui, within: egui::Rect, words: &str) -> Option<egui::Color32> {
    ui.shapes().into_iter().find_map(|(_, s)| match s {
        egui::Shape::Text(t) if t.galley.text() == words && within.contains_rect(t.galley.rect.translate(t.pos.to_vec2())) => {
            let section = t.galley.job.sections.first()?;
            Some(t.override_text_color.unwrap_or(if section.format.color == egui::Color32::PLACEHOLDER {
                t.fallback_color
            } else {
                section.format.color
            }))
        }
        _ => None,
    })
}

/// Review Focus 1 and 4 of group 1: every menu and popup list keeps its automation ids, values
/// and enabled flags, as `menu.rs`, `grid.rs`, `connections.rs`, `conn_form.rs` and
/// `query_tab.rs` tag them, now on the page's 28 tall items.
#[test]
fn menu_items_keep_their_ids_and_values() {
    let db = dbee_testkit::require_testdb!();
    {
        let mut ui = connected(db);
        // The menu bar: each entry `{label, shortcut, why?}`, enabled as its command is.
        let commands = ui.ok("engine.commands", json!({})).as_array().unwrap().clone();
        for top in ["File", "Edit", "View", "Query"] {
            ui.click(&format!("menu-{top}"));
            ui.frame();
            let i = ui.inspect();
            let listed: Vec<&Json> = commands.iter().filter(|c| c["menu"] == json!([top])).collect();
            assert!(!listed.is_empty(), "{top} lists commands");
            for c in listed {
                let mut value = json!({ "label": c["label"], "shortcut": c["shortcut"] });
                if let Some(why) = c["disabledReason"].as_str() {
                    value["why"] = json!(why);
                }
                assert_item(&i, &format!("menu-{}", c["id"].as_str().unwrap()), &value, c["enabled"] == true, top);
            }
            ui.key("Escape");
            ui.frame();
            assert!(ui.tag(&format!("menu-{}", commands.iter().find(|c| c["menu"] == json!([top])).unwrap()["id"].as_str().unwrap())).is_none());
        }
        // View > Theme: the submenu's button, and its entries with the chosen one `checked`.
        ui.click("menu-View");
        ui.frame();
        ui.click("menu-View-Theme");
        ui.frame();
        ui.frame();
        let i = ui.inspect();
        assert_eq!(rect_of(&tag_in(&i, "menu-View-Theme").expect("the Theme submenu's button")).height(), 28.0);
        assert_item(&i, "menu-ui.themeSystem", &json!({ "label": "System", "shortcut": null, "checked": true }), true, "Theme");
        assert_item(&i, "menu-ui.themeLight", &json!({ "label": "Light", "shortcut": null }), true, "Theme");
        assert_item(&i, "menu-ui.themeDark", &json!({ "label": "Dark", "shortcut": null }), true, "Theme");
        ui.key("Escape");
        ui.frame();

        // The grid's menu on a table that takes edits.
        ui.open("shop", "orders");
        ui.until("the grid", |ui| ui.tag("grid-cell-0-1").is_some());
        ui.right_click("grid-cell-0-1");
        ui.frame();
        let i = ui.inspect();
        for (id, label, shortcut) in [
            ("grid-menu-null", "Set as NULL", Json::Null),
            ("grid-menu-copy", "Copy", json!("Ctrl+C")),
            ("grid-menu-copy-header", "Copy with header", json!("Ctrl+Shift+C")),
            ("grid-menu-copy-csv", "Copy as CSV", Json::Null),
            ("grid-menu-copy-json", "Copy as JSON", Json::Null),
            ("grid-menu-copy-markdown", "Copy as Markdown", Json::Null),
            ("grid-menu-delete", "Delete row", Json::Null),
        ] {
            assert_item(&i, id, &json!({ "label": label, "shortcut": shortcut }), true, "the grid's menu");
        }
        ui.key("Escape");
        ui.frame();
    }
    {
        // The database chip's list and the results list, on MySQL with no database chosen.
        let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
        let mut ui = connected_to_target(db, &mysql, None, APP_SIZE);
        ui.ctrl("T");
        ui.frame();
        ui.click("query-database");
        ui.frame();
        let i = ui.inspect();
        for name in ["many", "odd", "scratch", "shop"] {
            assert_item(&i, &format!("query-database-{name}"), &json!(name), true, "the database chip's list");
        }
        ui.key("Escape");
        ui.frame();
        let sql: String = (1..=40).map(|n| format!("select {n} as n;\n")).collect();
        ui.execute("query.setText", json!({ "text": sql }));
        ui.ctrl("Enter");
        ui.until("the run to end", |ui| ui.document()["query"]["results"].as_array().is_some_and(|r| r.len() == 40));
        ui.frame();
        ui.click("result-more");
        ui.frame();
        let i = ui.inspect();
        // The list opens at its top; the shown result (the last) is further down.
        for n in [1, 2] {
            let e = tag_in(&i, &format!("result-list-{n}")).expect("the results list");
            let value = json!({ "shown": false, "rows": 1, "rowsAffected": e["value"]["rowsAffected"] });
            assert_item(&i, &format!("result-list-{n}"), &value, true, "the results list");
        }
    }
    {
        // A saved connection's menu, and the form's database list.
        let mut ui = Ui::fresh(APP_SIZE);
        let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
        ui.frame();
        ui.right_click(&format!("conn-saved-{id}"));
        ui.frame();
        let i = ui.inspect();
        for (item, label) in
            [("conn-menu-connect", "Connect"), ("conn-menu-edit", "Edit"), ("conn-menu-duplicate", "Duplicate"), ("conn-menu-delete", "Delete")]
        {
            assert_item(&i, item, &json!(label), true, "the connection's menu");
        }
        ui.key("Escape");
        ui.frame();
        ui.click(&format!("conn-saved-{id}"));
        ui.click("conn-database-list");
        ui.until("the list", |ui| ui.tag("conn-database-item-0").is_some());
        ui.frame();
        let i = ui.inspect();
        let names: Vec<(String, Json)> = ids(&i)
            .into_iter()
            .filter(|id| id.starts_with("conn-database-item-"))
            .map(|id| (id.clone(), tag_in(&i, &id).unwrap()["value"].clone()))
            .collect();
        assert!(names.iter().any(|(_, n)| n == "dbee"), "{names:?}");
        for (item, name) in &names {
            assert_item(&i, item, name, true, "the database list");
        }
    }
}

/// A menu's shortcut is drawn as one keycap per key (`CTRL`, `SHIFT`, `Z`) at the item's right
/// end; the entry's `shortcut` value stays the text `shortcuts.rs` writes.
#[test]
fn shortcuts_draw_as_keycaps() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    let raised_line = dbee_ui_egui::theme::LIGHT.raised_line;
    ui.click("menu-Edit");
    ui.frame();
    let redo = ui.tag("menu-edit.redo").expect("Edit lists Redo");
    assert_eq!(redo["value"]["shortcut"], "Ctrl+Shift+Z", "{redo}");
    let at = rect_of(&redo);
    let keys: Vec<egui::Rect> =
        ["CTRL", "SHIFT", "Z"].into_iter().map(|k| ui.text_in(at, k).unwrap_or_else(|| panic!("{k} drawn in Redo's item {at:?}")).0).collect();
    assert!(keys.windows(2).all(|p| p[0].right() < p[1].left()), "in order: {keys:?}");
    let caps: Vec<egui::Rect> = ui
        .shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if at.contains_rect(r.rect) && r.stroke.color == raised_line && r.rect.height() == 18.0 => Some(r.rect),
            _ => None,
        })
        .collect();
    assert_eq!(caps.len(), 3, "three keycaps: {caps:?}");
    for (cap, key) in caps.iter().zip(&keys) {
        assert!(cap.contains_rect(*key), "{key:?} in its keycap {cap:?}");
    }
    assert!((caps[2].right() - (at.right() - 8.0)).abs() < 0.5, "the keys end 8 in: {caps:?} {at:?}");
    assert!(ui.text_in(at, "Ctrl+Shift+Z").is_none(), "no shortcut text");
}

/// Delete row and a saved connection's Delete are destructive: `danger`; the others `text`.
#[test]
fn delete_row_and_delete_connection_are_destructive() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    {
        let mut ui = connected(db);
        ui.open("shop", "orders");
        ui.until("the grid", |ui| ui.tag("grid-cell-0-1").is_some());
        ui.right_click("grid-cell-0-1");
        ui.frame();
        let delete = rect_of(&ui.tag("grid-menu-delete").unwrap());
        let copy = rect_of(&ui.tag("grid-menu-copy").unwrap());
        assert_eq!(colour_of(&ui, delete, "Delete row"), Some(t.danger), "Delete row");
        assert_eq!(colour_of(&ui, copy, "Copy"), Some(t.text), "Copy");
    }
    let mut ui = Ui::fresh(APP_SIZE);
    let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.frame();
    ui.right_click(&format!("conn-saved-{id}"));
    ui.frame();
    let delete = rect_of(&ui.tag("conn-menu-delete").unwrap());
    let edit = rect_of(&ui.tag("conn-menu-edit").unwrap());
    assert_eq!(colour_of(&ui, delete, "Delete"), Some(t.danger), "Delete");
    assert_eq!(colour_of(&ui, edit, "Edit"), Some(t.text), "Edit");
}

/// Review Focus 1: the keys that work in the menu bar work as before. Tab reaches a title, Enter
/// opens its menu, Down moves into it, with the hover look, and Enter runs the entry; a disabled
/// entry can't be chosen and says why on hover.
#[test]
fn the_keyboard_still_works_in_the_menu_bar() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = connected(db);
    ui.key("Tab");
    ui.frame();
    ui.key("Enter");
    ui.frame();
    ui.frame();
    let new = ui.tag("menu-tab.newQuery").expect("Enter opened File");
    ui.key("ArrowDown");
    ui.frame();
    let at = rect_of(&new);
    let grounds: Vec<egui::Color32> = ui
        .shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.rect == at && r.fill.a() > 0 => Some(r.fill),
            _ => None,
        })
        .collect();
    assert_eq!(grounds, [t.raised_hover], "Down moved to New Query Tab, which looks hovered");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1), "Enter ran it");
    assert!(ui.tag("menu-tab.newQuery").is_none(), "and closed the menu");
    // A disabled entry: a click runs nothing, and its hover says why.
    ui.click("menu-File");
    ui.frame();
    let connect = ui.tag("menu-connection.open").expect("File lists Connect");
    let why = connect["value"]["why"].as_str().unwrap().to_string();
    ui.click("menu-connection.open");
    ui.frame();
    assert_eq!(ui.document()["screen"], "workspace", "the disabled Connect ran nothing");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1), "nothing ran");
    // As before, the click closed the menu (egui closes a menu on any click in it).
    if ui.tag("menu-connection.open").is_none() {
        ui.click("menu-File");
        ui.frame();
    }
    let at = rect_of(&ui.tag("menu-connection.open").expect("File lists Connect"));
    ui.rest(at.center());
    assert!(ui.texts().iter().any(|(_, g)| g.text().contains(&why)), "the hover says why: {why}");
}

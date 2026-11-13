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

/// The last frame drew the keyboard's focus ring 2 inside `field`: a 2 px `ink` box.
fn ring_in(ui: &Ui, field: egui::Rect) -> bool {
    let ink = dbee_ui_egui::theme::tokens(&ui.harness.ctx).ink;
    let at = field.shrink(2.0);
    ui.shapes().iter().any(|(_, s)| {
        matches!(s, egui::Shape::Rect(r) if r.stroke.width == 2.0 && r.stroke.color == ink
            && (r.rect.min - at.min).length() < 0.5 && (r.rect.max - at.max).length() < 0.5)
    })
}

/// The tree's header is the sidebar's filter and count (`components::sidebar`). `tree-filter`
/// keeps its id and its value as typed, `tree-refresh` its value, enabled flag and hover. The
/// field is the page's 35 above the row's line with the refresh's 28 square beside it; what is
/// typed is drawn as typed; the count under the row says dbee's word in capitals. A click gives
/// the field focus without the ring, Shift+Tab back to it shows the ring; a refresh keeps the
/// filter.
#[test]
fn the_tree_filter_keeps_its_tags_and_behaviour() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    let i = ui.inspect();
    let filter = tag_in(&i, "tree-filter").expect("the filter is drawn");
    let refresh = tag_in(&i, "tree-refresh").expect("the refresh is drawn");
    assert_eq!((&filter["value"], filter.get("enabled")), (&json!(""), None), "{filter}");
    assert_eq!((&refresh["value"], refresh.get("enabled")), (&json!("Refresh"), None), "{refresh}");
    let tags = |ui: &Ui, id: &str| ui.harness.state().tags().rect(id).unwrap_or_else(|| panic!("`{id}` is drawn"));
    let (f, r) = (tags(&ui, "tree-filter"), tags(&ui, "tree-refresh"));
    assert_eq!(f.height(), 35.0, "the field, 35 above the row's line: {f:?}");
    assert_eq!(r.size(), egui::vec2(28.0, 28.0), "the refresh, a 28 square: {r:?}");
    assert!((r.left() - f.right()).abs() < 0.5 && (r.center().y - f.center().y).abs() < 0.5, "the refresh beside the field: {f:?} {r:?}");
    assert!(ui.text_in(f, "Filter tables").is_some(), "the placeholder as written");
    // The count, under the row: every table, in capitals.
    let count =
        |ui: &Ui| ui.texts().into_iter().map(|(at, g)| (at, g.text().to_string())).find(|(at, w)| w.ends_with(" TABLES") && at.top() >= f.bottom());
    let (at, all) = count(&ui).unwrap_or_else(|| panic!("the count is drawn: {:?}", ui.texts().iter().map(|(_, g)| g.text()).collect::<Vec<_>>()));
    let total = all.strip_suffix(" TABLES").unwrap_or_default().to_string();
    assert!(!total.is_empty() && total.chars().all(|c| c.is_ascii_digit() || c == ','), "{all}");
    assert!(at.top() >= f.bottom() + 1.0, "under the row's line: {at:?}");
    assert!(ui.texts().iter().all(|(_, g)| g.text() != format!("{total} tables")), "dbee's word only in capitals");
    // Clicked, typed: the value and the drawn words as typed, no ring.
    ui.click("tree-filter");
    ui.text("Order");
    ui.frame();
    assert_eq!(ui.value("tree-filter"), "Order");
    assert!(ui.text_in(f, "Order").is_some() && ui.text_in(f, "ORDER").is_none(), "drawn as typed");
    assert!(!ring_in(&ui, f), "a click shows no ring");
    let tables = ids(&ui.inspect()).iter().filter(|id| id.starts_with("tree-table-")).count();
    assert!(tables > 0 && visible(&ui.tag("tree-table-shop.orders")), "the matches show");
    let (_, some) = count(&ui).expect("the count while filtering");
    assert_eq!(some, format!("{} OF {total} TABLES", tree::thousands(tables as u64)));
    // Tab on to the refresh, Shift+Tab back: the ring.
    ui.key("Tab");
    ui.frame();
    ui.key_with("Tab", json!({ "shift": true }));
    ui.frame();
    assert!(ring_in(&ui, f), "Shift+Tab back shows the ring");
    assert_eq!(tags(&ui, "tree-filter"), f, "nothing moved");
    // The refresh's hover, and a refresh keeps the filter.
    ui.rest(r.center());
    assert!(ui.texts().iter().any(|(_, g)| g.text() == "Refresh the schemas"), "the refresh's hover");
    ui.click("tree-refresh");
    ui.ok("engine.wait", json!({}));
    ui.frame();
    assert_eq!(ui.value("tree-filter"), "Order", "the filter stays");
    assert!(ui.tag("tree-table-shop.orders").is_some());
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

/// Opens a query tab running `sql`, which leaves a transaction open; the tab's id.
fn in_transaction(ui: &mut Ui, sql: &str) -> u64 {
    ui.execute("tab.newQuery", json!({ "text": sql }));
    let tab = ui.active();
    ui.execute("query.run", json!({}));
    ui.frame();
    assert_eq!(ui.document()["connection"]["transaction"]["state"], "open", "{sql}");
    tab
}

/// The prompt's buttons: `dialog-ok`, `dialog-rollback` and `dialog-cancel` with their words.
fn assert_three_buttons(ui: &mut Ui) {
    let words = (ui.value("dialog-ok"), ui.value("dialog-rollback"), ui.value("dialog-cancel"));
    assert_eq!(words, (json!("Commit"), json!("Rollback"), json!("Cancel")));
}

/// The notice once one shows.
fn toast_text(ui: &mut Ui) -> String {
    ui.until("a toast", |ui| ui.tag("notice").is_some());
    ui.value("notice").as_str().unwrap_or_default().to_owned()
}

#[test]
fn commit_from_the_prompt_closes_the_tab_and_toasts() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "prompt_commit", |n| format!("CREATE TABLE scratch.{n} (id int)"));
    let mut ui = connected(db);
    let tab = in_transaction(&mut ui, &format!("BEGIN; INSERT INTO scratch.{} VALUES (1)", table.0));
    ui.click(&format!("tab-close-{tab}"));
    assert_eq!(ui.inspect()["dialog"], "transactionEnd", "closing the tab that opened the transaction asks");
    assert_eq!(ui.value("dialog-text"), "Query 1 opened a transaction that isn't committed yet. Its query text will be lost.");
    assert_three_buttons(&mut ui);
    ui.click("dialog-ok");
    ui.until("the tab to close", |ui| ui.document()["tabs"] == json!([]));
    assert_eq!(toast_text(&mut ui), "Committed on pg14");
    let t = dbee_ui_egui::theme::LIGHT;
    ui.toast().unwrap_or_else(|| panic!("the toast")).assert_in(t.ink, t.on_ink, "a success");
    assert_eq!(ui.document()["connection"]["transaction"], Json::Null);
    let seen = dbee_testkit::psql(pg14_server(db), &format!("SELECT count(*) FROM scratch.{}", table.0)).unwrap();
    assert_eq!(seen.trim(), "1", "another session sees the row");
}

#[test]
fn cancel_leaves_everything() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    let tab = in_transaction(&mut ui, "BEGIN");
    ui.click(&format!("tab-close-{tab}"));
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    ui.click("dialog-cancel");
    for _ in 0..3 {
        ui.frame();
    }
    let d = ui.document();
    assert_eq!(d["tabs"].as_array().map(Vec::len), Some(1), "the tab stays: {d}");
    assert_eq!(d["connection"]["transaction"]["state"], "open", "the transaction stays: {d}");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
    assert!(ui.tag("notice").is_none(), "no toast");
}

#[test]
fn a_lost_connection_at_commit_toasts_and_keeps_the_tab() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // The query connection's own backend, which only this window uses.
    ui.execute("tab.newQuery", json!({ "text": "SELECT pg_backend_pid()" }));
    let tab = ui.active();
    ui.execute("query.run", json!({}));
    ui.frame();
    let pid = ui.value("grid-cell-0-0")["text"].as_str().unwrap_or_default().to_owned();
    assert!(pid.parse::<u32>().is_ok(), "the query connection's pid: {pid}");
    ui.execute("query.setText", json!({ "text": "BEGIN" }));
    ui.execute("query.run", json!({}));
    ui.frame();
    ui.click(&format!("tab-close-{tab}"));
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    dbee_testkit::psql(pg14_server(db), &format!("SELECT pg_terminate_backend({pid}, 5000)")).unwrap();
    ui.click("dialog-ok");
    let text = toast_text(&mut ui);
    assert!(text.starts_with("The commit failed: "), "{text}");
    assert!(!text.contains("Run again"), "a close's toast has no run's advice: {text}");
    let t = dbee_ui_egui::theme::LIGHT;
    ui.toast().unwrap_or_else(|| panic!("the toast")).assert_in(t.danger, t.on_danger, "a failure");
    let d = ui.document();
    assert_eq!(d["tabs"].as_array().map(Vec::len), Some(1), "the tab stays: {d}");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
}

impl Ui {
    /// Frames on while the gated end runs: still `connection` on the workspace, its close job
    /// (`label`) running, and no dialog.
    fn still_ending(&mut self, connection: &str, label: &str) {
        for _ in 0..20 {
            self.frame();
        }
        let d = self.document();
        assert_eq!((d["screen"].clone(), d["connection"]["id"].clone()), (json!("workspace"), json!(connection)), "still open: {d}");
        assert!(d["jobs"].as_array().is_some_and(|j| j.iter().any(|j| j["label"] == label)), "the {label} job runs: {d}");
        assert_eq!(self.inspect()["dialog"], Json::Null, "no prompt while the end runs");
    }
}

#[test]
fn a_switch_waits_for_the_end_then_switches() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "prompt_switch", |n| format!("CREATE TABLE scratch.{n} (id int); INSERT INTO scratch.{n} VALUES (1)"));
    let update = format!("BEGIN; UPDATE scratch.{0} SET id = id + 1", table.0);
    let ids = || dbee_testkit::psql(pg14_server(db), &format!("SELECT id FROM scratch.{}", table.0)).unwrap().trim().to_owned();
    // The COMMIT waits at the gate until it is let go, so the end runs as long as the test wants.
    let gate = || dbee_testkit::target(pg14_server(db)).commit_gate(&table.0).unwrap();
    let mut held = gate();
    let (mut ui, pgnew) = with_pgnew(db);
    let pg14 = ui.connection_id().unwrap();
    let on = |ui: &mut Ui, id: &str| ui.connection_id().as_deref() == Some(id) && ui.document()["screen"] == "workspace";
    in_transaction(&mut ui, &update);
    // CANCEL: no switch.
    ui.quick_choose("pgnew");
    assert_eq!(ui.inspect()["dialog"], "transactionEnd", "switching asks about the connection's transaction");
    assert_eq!(ui.value("dialog-text"), "pg14 has a transaction that isn't committed yet. The text of 1 query tab will be lost.");
    assert_three_buttons(&mut ui);
    ui.click("dialog-cancel");
    for _ in 0..5 {
        ui.frame();
    }
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"]["id"].clone()), (json!("workspace"), json!(pg14.clone())), "Cancel keeps pg14: {d}");
    assert_eq!(d["connection"]["transaction"]["state"], "open", "{d}");
    // COMMIT: the switch waits while the end runs, and choosing again meanwhile asks nothing more.
    ui.quick_choose("pgnew");
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    ui.click("dialog-ok");
    held.until_committing().unwrap();
    ui.still_ending(&pg14, "connection.close");
    ui.quick_choose("pgnew");
    ui.still_ending(&pg14, "connection.close");
    // Once the COMMIT is through, pg14 closes and pgnew opens. The toast shows as the end
    // applies, while pgnew connects (it goes after 4 s).
    drop(held);
    assert_eq!(toast_text(&mut ui), "Committed on pg14");
    ui.until("connected to pgnew", |ui| on(ui, &pgnew));
    assert_eq!(ids(), "2", "committed");
    // ROLLBACK, back to pg14.
    in_transaction(&mut ui, "BEGIN");
    ui.quick_choose("pg14");
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    ui.click("dialog-rollback");
    assert_eq!(toast_text(&mut ui), "Rolled back on pgnew");
    ui.until("connected to pg14", |ui| on(ui, &pg14));
    // A switch chosen while the owner tab's close ends the transaction waits for it, then
    // closes pg14 with nothing more to ask.
    let mut held = gate();
    let tab = in_transaction(&mut ui, &update);
    ui.execute("query.setText", json!({ "text": "" }));
    ui.click(&format!("tab-close-{tab}"));
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    ui.click("dialog-ok");
    held.until_committing().unwrap();
    ui.quick_choose("pgnew");
    ui.still_ending(&pg14, "tab.close");
    drop(held);
    assert_eq!(toast_text(&mut ui), "Committed on pg14");
    ui.until("connected to pgnew", |ui| on(ui, &pgnew));
    assert_eq!(ids(), "3", "committed");
}

#[test]
fn disconnect_asks_with_the_text_sentence_once() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    in_transaction(&mut ui, "BEGIN");
    ui.execute("query.setText", json!({ "text": "" }));
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    ui.frame();
    ui.ctrl_shift("Q");
    assert_eq!(ui.inspect()["dialog"], "transactionEnd", "one prompt");
    let text = ui.value("dialog-text");
    assert!(text.as_str().is_some_and(|t| t.ends_with(". The text of 1 query tab will be lost.")), "with today's sentence: {text}");
    ui.click("dialog-ok");
    ui.until("the connection screen", |ui| {
        assert_eq!(ui.inspect()["dialog"], Json::Null, "no second dialog");
        ui.document()["screen"] == "connections"
    });
    for _ in 0..3 {
        ui.frame();
        assert_eq!(ui.inspect()["dialog"], Json::Null, "no second dialog");
    }
    assert_eq!(toast_text(&mut ui), "Committed on pg14");
}

impl Ui {
    /// One frame in which the desktop asks the window to close (its close button, Alt+F4), with
    /// [`Ui::sent`] emptied first, so it holds what the window answers.
    fn close_request(&mut self) {
        self.sent.clear();
        self.harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        self.frame();
    }

    /// The window cancelled the close the desktop asked for.
    fn kept_open(&self) -> bool {
        self.sent.contains(&egui::ViewportCommand::CancelClose)
    }

    /// The window closed itself.
    fn closed(&self) -> bool {
        self.sent.contains(&egui::ViewportCommand::Close)
    }

    /// Frames until the window closes itself.
    fn until_closed(&mut self) {
        self.until("the window to close", |ui| ui.closed());
    }

    /// A few frames, then: the window stays, with no dialog.
    fn assert_stays(&mut self, at: &str) {
        for _ in 0..5 {
            self.frame();
        }
        assert!(!self.closed(), "{at}: the window stays");
        assert_eq!(self.inspect()["dialog"], Json::Null, "{at}: nothing more is asked");
    }
}

#[test]
fn quitting_with_a_transaction_asks() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "quit_commit", |n| format!("CREATE TABLE scratch.{n} (id int)"));
    let mut ui = connected(db);
    in_transaction(&mut ui, &format!("BEGIN; INSERT INTO scratch.{} VALUES (1)", table.0));
    ui.close_request();
    assert!(ui.kept_open(), "the window stays to ask");
    assert_eq!(ui.inspect()["dialog"], "transactionEnd", "the transaction prompt, about the connection");
    assert_eq!(ui.value("dialog-text"), "pg14 has a transaction that isn't committed yet. The text of 1 query tab will be lost.");
    assert_three_buttons(&mut ui);
    assert!(!ui.closed());
    ui.click("dialog-ok");
    ui.until_closed();
    assert_eq!(toast_text(&mut ui), "Committed on pg14");
    assert_eq!(ui.document()["screen"], "connections", "pg14 closed before the window");
    let seen = dbee_testkit::psql(pg14_server(db), &format!("SELECT count(*) FROM scratch.{}", table.0)).unwrap();
    assert_eq!(seen.trim(), "1", "committed");
}

#[test]
fn quitting_with_only_text_says_quit_anyway() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": "select 1" }));
    ui.close_request();
    assert!(ui.kept_open(), "the window stays to ask");
    assert_eq!(ui.inspect()["dialog"], "confirm");
    assert_eq!(ui.value("dialog-text"), "The text of 1 query tab will be lost.");
    assert_eq!((ui.value("dialog-ok"), ui.value("dialog-cancel")), (json!("Quit anyway"), json!("Cancel")));
    let drawn: Vec<String> = ui.texts().iter().map(|(_, g)| g.text().to_owned()).collect();
    assert!(drawn.iter().any(|w| w == "Quit anyway?") && drawn.iter().any(|w| w == "QUIT ANYWAY"), "the title and the button: {drawn:?}");
    let ok = rect_of(&ui.tag("dialog-ok").unwrap_or_default());
    let danger = dbee_ui_egui::theme::LIGHT.danger;
    assert!(ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.rect == ok && r.fill == danger)), "QUIT ANYWAY is a danger button");
    ui.click("dialog-ok");
    ui.until_closed();
    assert_eq!(ui.document()["screen"], "connections", "pg14 closed before the window");
}

/// Review Focus 3, as far as a window reaches it: a window holds one connection (gap 6), so a
/// quit asks about that one. CANCEL, or an end that fails, stops the quit: the window stays and the
/// connection is as the answer left it, and the next close asks afresh.
#[test]
fn quit_stops_at_the_first_cancel() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "quit_stop", |n| format!("CREATE TABLE scratch.{n} (id int UNIQUE DEFERRABLE INITIALLY DEFERRED)"));
    let insert = format!("INSERT INTO scratch.{} VALUES (1)", table.0);
    let mut ui = connected(db);
    in_transaction(&mut ui, &format!("BEGIN; {insert}"));
    // CANCEL: nothing ends, nothing closes.
    ui.close_request();
    assert_eq!(ui.inspect()["dialog"], "transactionEnd");
    ui.click("dialog-cancel");
    ui.assert_stays("cancel");
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"]["transaction"]["state"].clone()), (json!("workspace"), json!("open")), "{d}");
    assert!(ui.tag("notice").is_none(), "no toast");
    // A COMMIT the server refuses (the deferred key): the error toast, and the window stays.
    ui.execute("query.setText", json!({ "text": insert }));
    ui.execute("query.run", json!({}));
    ui.close_request();
    assert_eq!(ui.inspect()["dialog"], "transactionEnd", "the next close asks afresh");
    ui.click("dialog-ok");
    let text = toast_text(&mut ui);
    assert!(text.starts_with("The commit failed: ") && text.contains("duplicate key"), "{text}");
    ui.assert_stays("a failed commit");
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["tabs"].as_array().map(Vec::len)), (json!("workspace"), Some(1)), "{d}");
    let seen = dbee_testkit::psql(pg14_server(db), &format!("SELECT count(*) FROM scratch.{}", table.0)).unwrap();
    assert_eq!(seen.trim(), "0", "nothing committed");
    // `Quit anyway?`: CANCEL keeps the text.
    ui.close_request();
    assert_eq!(ui.inspect()["dialog"], "confirm", "no transaction now, the text still");
    ui.click("dialog-cancel");
    ui.assert_stays("quit anyway");
    assert_eq!(ui.document()["query"]["text"], json!(insert));
}

#[test]
fn quitting_with_a_running_query_asks() {
    let db = dbee_testkit::require_testdb!();
    let marker = dbee_testkit::unique("quit_run");
    let running = || {
        let sql = format!("SELECT count(*) FROM pg_stat_activity WHERE state = 'active' AND query LIKE '%{marker}%' AND pid <> pg_backend_pid()");
        dbee_testkit::psql(pg14_server(db), &sql).unwrap().trim().to_owned()
    };
    let mut ui = connected(db);
    ui.execute("tab.newQuery", json!({ "text": format!("SELECT pg_sleep(60), '{marker}'") }));
    ui.ok("engine.execute", json!({ "command": "query.run", "params": {}, "wait": false }));
    ui.until("the statement to run on the server", |_| running() == "1");
    ui.close_request();
    assert!(ui.kept_open(), "the window stays to ask");
    assert_eq!(ui.inspect()["dialog"], "confirm");
    assert_eq!(ui.value("dialog-text"), "Query 1 is still running. Quitting stops it. The text of 1 query tab will be lost.");
    assert_eq!(ui.value("dialog-ok"), "Quit anyway");
    ui.click("dialog-ok");
    ui.until_closed();
    // As the frame that closed the window left it: no frame has applied a job's end since.
    assert!(!ui.harness.state().session().busy(), "the stopped run had ended when the window closed");
    assert_eq!(running(), "0", "the statement was stopped before the window closed");
    assert_eq!(ui.document()["jobs"], json!([]), "nothing runs");
}

#[test]
fn quitting_with_nothing_to_lose_closes_at_once() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": " \n" }));
    ui.open("shop", "orders");
    ui.close_request();
    assert!(!ui.kept_open(), "nothing would be lost: the close goes ahead");
    assert!(!ui.closed(), "the desktop closes the window, dbee sends nothing");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
    // With no connection open too.
    let mut ui = Ui::fresh(APP_SIZE);
    ui.close_request();
    assert!(!ui.kept_open() && !ui.closed());
    assert_eq!(ui.inspect()["dialog"], Json::Null);
}

/// `app.quit` closes the window without asking, whatever would be lost, and the close request the
/// window's own Close brings goes through.
#[test]
fn the_control_quit_is_unchanged() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    in_transaction(&mut ui, "BEGIN");
    assert_eq!(ui.call("app.quit", json!({}))["ok"], true);
    ui.until_closed();
    assert_eq!(ui.inspect()["dialog"], Json::Null, "it asks nothing");
    ui.close_request();
    assert!(!ui.kept_open(), "the close goes ahead");
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
    // While the tabs don't fit, they scroll in what the band leaves before + and the list button.
    let end = if tags.rect("tab-list").is_some() { tags.rect("tab-new").map_or(band.right(), |r| r.left()) } else { band.right() };
    for (n, r) in &tabs {
        assert_eq!((r.top(), r.bottom()), (band.top(), band.bottom()), "{at}: tab {n} {r:?} fills the band {band:?}");
        // A tab cut by the end of what scrolls is tagged by its part in view.
        let right = egui::Rect::from_min_max(egui::pos2(r.right() - 1.0, r.top()), r.max);
        assert!(r.right() >= end || fills.contains(&(right, t.line)), "{at}: tab {n} {r:?} has the line at its right end");
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
    assert_eq!(ui.value("status-connection"), "Shop live");
    assert_eq!(ui.value("status-read-only"), "Read-only");
    let server = ui.value("status-server");
    assert!(server.as_str().is_some_and(|s| s.starts_with("PostgreSQL 14")), "{server}");
    // prefer, and the test server offers TLS: encrypted, its certificate not checked.
    assert_eq!(ui.value("status-tls"), "Encrypted, server not checked");
    assert!(ui.tag("status-reconnect").is_none(), "no Reconnect while the connection is fine");
    assert!(ui.tag("status-save").is_none(), "no Save connection for a saved one");
}

/// The status bar tells a checked certificate from a checked server: `verify-ca` and `require`
/// with a CA file do not check the name, and say so; under `verify-full` it says nothing.
#[test]
fn the_status_bar_tells_what_was_checked() {
    let db = dbee_testkit::require_testdb!();
    let ca_file = Some(db.ca_file.clone());
    for (ssl_mode, says) in [
        (SslMode::VerifyCa, Some("Encrypted, certificate checked")),
        (SslMode::Require, Some("Encrypted, certificate checked")),
        // Fully checked: the bar names the link only while it is weaker (#9).
        (SslMode::VerifyFull, None),
    ] {
        let mut ui = connected_to(db, ConnectionConfig { ssl_mode, ca_file: ca_file.clone(), ..pg14(db) }, false);
        assert!(ui.document()["connection"]["tls"].is_string(), "{ssl_mode:?}");
        assert_eq!(ui.tag("status-tls").map(|e| e["value"].clone()), says.map(|s| json!(s)), "{ssl_mode:?}");
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
    let refresh = ui.harness.state().tags().rect("tree-refresh").expect("the refresh is drawn");
    ui.rest(refresh.center());
    let why = "Cannot refresh: the connection is lost; reconnect first";
    assert!(ui.texts().iter().any(|(_, g)| g.text() == why), "the refresh says why it is off");
    ui.click("status-reconnect");
    ui.until("the reconnect", |ui| ui.document()["connection"]["lost"] == false);
    ui.frame();
    assert!(ui.tag("status-reconnect").is_none());
    assert!(ui.tag("status-lost").is_none());
}

/// The rect of entry `id` in the last frame, if it was drawn in view.
fn shown_rect(ui: &mut Ui, id: &str) -> Option<egui::Rect> {
    ui.tag(id).filter(|e| visible(&Some(e.clone()))).map(|e| rect_of(&e))
}

/// An unsaved connection to pg14 named "Shop live", red, read-only, under `prefer`: every fact
/// of the status bar at once, in a window of `size` (holding the lock alone when `exclusive`).
fn every_fact(db: &TestDb, size: egui::Vec2, exclusive: bool) -> Ui {
    let mut ui = Ui::window(Setup { size, exclusive, ..Setup::default() });
    let config = ConnectionConfig { name: "Shop live".into(), color: ConnectionColor::Red, read_only: true, ..pg14(db) };
    ui.execute("connection.open", json!({ "config": config, "password": db.password }));
    ui.frame();
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["connection"]["saved"].clone()), (json!("workspace"), json!(false)), "{d}");
    ui
}

/// Review Focus 1: the status bar loses nothing a user relies on. The link's security with its
/// hover, READ-ONLY (its own fact now), the server version, SAVE CONNECTION, a job with its
/// CANCEL, DISCONNECTED with RECONNECT; each `status-*` tag with its value and enabled flag.
#[test]
fn the_status_bar_keeps_every_fact() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = every_fact(db, APP_SIZE, true);
    assert_eq!(ui.value("status-connection"), "Shop live", "the name alone: read-only is a fact of its own");
    assert_eq!(ui.value("status-read-only"), "Read-only");
    assert!(ui.value("status-server").as_str().is_some_and(|s| s.starts_with("PostgreSQL 14.")), "the server version");
    assert_eq!(ui.value("status-tls"), "Encrypted, server not checked");
    let i = ui.inspect();
    for (id, value, words) in [
        ("status-connection", "Shop live", "SHOP LIVE"),
        ("status-read-only", "Read-only", "READ-ONLY"),
        ("status-tls", "Encrypted, server not checked", "ENCRYPTED, SERVER NOT CHECKED"),
        ("status-save", "Save connection", "SAVE CONNECTION"),
    ] {
        let e = tag_in(&i, id).unwrap_or_else(|| panic!("`{id}` drawn: {:?}", ids(&i)));
        assert_eq!((e["value"].clone(), e.get("enabled").cloned()), (json!(value), None), "`{id}`: {e}");
        assert!(ui.text_in(rect_of(&e), words).is_some(), "`{id}` draws `{words}`");
    }
    for (words, colour) in [("READ-ONLY", t.text_muted), ("ENCRYPTED, SERVER NOT CHECKED", t.text_muted)] {
        assert_eq!(colour_of(&ui, egui::Rect::EVERYTHING, words), Some(colour), "`{words}` in grey");
    }
    // The link's hover sentence, in the tooltip.
    let at = ui.centre("status-tls");
    ui.rest(at);
    assert!(ui.texts().iter().any(|(_, g)| g.text().starts_with("The connection is encrypted, but dbee did not check")), "the link's hover");
    // A job with its CANCEL.
    ui.execute("tab.newQuery", json!({ "text": "select pg_sleep(30)" }));
    ui.ok("engine.execute", json!({ "command": "query.run", "wait": false }));
    ui.until("the run", |ui| ui.tag("status-job").is_some());
    assert_eq!(ui.value("status-job"), "Running Query 1…");
    let cancel = ui.tag("status-cancel").expect("the job's CANCEL");
    assert_eq!((cancel["value"].clone(), cancel.get("enabled").cloned()), (json!("Cancel"), None), "{cancel}");
    let job = rect_of(&ui.tag("status-job").unwrap());
    assert!(ui.text_in(job, "RUNNING Query 1…").is_some(), "dbee's words in capitals, the tab's title as written");
    assert!(ui.text_in(rect_of(&cancel), "CANCEL").is_some());
    ui.click("status-cancel");
    ui.until("the cancel", |ui| ui.document()["query"]["running"].is_null());
    ui.frame();
    assert!(ui.tag("status-job").is_none() && ui.tag("status-cancel").is_none(), "the job ended");
    // Lost: DISCONNECTED with RECONNECT.
    let sql = "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE application_name = 'dbee'";
    dbee_testkit::psql(db.server("pg14").unwrap(), sql).unwrap();
    ui.click("tree-refresh");
    ui.until("the lost mark", |ui| ui.document()["connection"]["lost"] == true);
    ui.frame();
    assert_eq!(ui.value("status-lost"), "Connection lost");
    let lost = rect_of(&ui.tag("status-lost").unwrap());
    assert!(ui.text_in(lost, "DISCONNECTED").is_some(), "the page's word");
    assert_eq!(colour_of(&ui, lost, "DISCONNECTED"), Some(t.danger));
    let reconnect = ui.tag("status-reconnect").expect("RECONNECT");
    assert_eq!((reconnect["value"].clone(), reconnect.get("enabled").cloned()), (json!("Reconnect"), None), "{reconnect}");
    assert!(ui.text_in(rect_of(&reconnect), "RECONNECT").is_some());
    ui.click("status-reconnect");
    ui.until("the reconnect", |ui| ui.document()["connection"]["lost"] == false);
    ui.frame();
    assert!(ui.tag("status-lost").is_none() && ui.tag("status-reconnect").is_none());
    // SAVE CONNECTION saves it.
    ui.click("status-save");
    ui.until("the save", |ui| ui.tag("status-save").is_none());
    assert_eq!(ui.document()["connection"]["saved"], true);
}

/// Review Focus 3: a server version and a name dbee made from the user, host and database are
/// data: drawn as written, never in capitals.
#[test]
fn a_server_version_keeps_its_case() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected_to(db, ConnectionConfig { name: String::new(), ..pg14(db) }, false);
    let server = ui.value("status-server").as_str().unwrap().to_string();
    assert!(server.starts_with("PostgreSQL 14."), "{server}");
    let name = ui.value("status-connection").as_str().unwrap().to_string();
    assert_eq!(name, "postgres@127.0.0.1/dbee");
    for (id, words) in [("status-server", &server), ("status-connection", &name)] {
        let at = rect_of(&ui.tag(id).unwrap());
        assert!(ui.text_in(at, words).is_some(), "`{words}` drawn as written");
        assert!(ui.text_in(at, &words.to_uppercase()).is_none(), "`{words}` not in capitals");
    }
}

/// Review Focus 5: at the smallest window nothing in the status bar is drawn over anything
/// else; the facts give way in the order the bar states (the server version, then the link's
/// security, then the address, then READ-ONLY), and the name, SAVE CONNECTION, the job with its
/// CANCEL and the value panel's toggle stay.
#[test]
fn the_status_bar_gives_way_at_the_smallest_window() {
    let db = dbee_testkit::require_testdb!();
    // Review 1, I1: a job on a table with a 63-letter name, the most Postgres takes.
    let long = Scratch::new(db, "customer_success_quarterly_revenue_reconciliation_2026q4", |t| format!("CREATE TABLE scratch.{t} (id int)"));
    assert_eq!(long.0.len(), 63);
    for size in [APP_SIZE, egui::vec2(640.0, 420.0)] {
        let at = format!("{} wide", size.x);
        let mut ui = every_fact(db, size, false);
        ui.execute("tab.newQuery", json!({ "text": "select pg_sleep(30)" }));
        ui.ok("engine.execute", json!({ "command": "query.run", "wait": false }));
        ui.until("the run", |ui| ui.tag("status-job").is_some());
        let drawn = status_gives_way(&mut ui, size, &at);
        let has = |id: &str| drawn.iter().any(|(d, _)| *d == id);
        let facts = ["status-server", "status-tls", "status-address", "status-read-only"];
        for (k, id) in facts.iter().enumerate() {
            if has(id) {
                assert!(facts[k..].iter().all(|later| has(later)), "{at}: `{id}` shows, so the facts after it in the order do: {drawn:?}");
            }
        }
        if size == APP_SIZE {
            assert!(facts[1..].iter().all(|id| has(id)), "{at}: every fact but the server version fits: {drawn:?}");
        } else {
            assert!(!has("status-server"), "{at}: the server version gave way first: {drawn:?}");
        }
        ui.click("status-cancel");
        ui.until("the cancel", |ui| ui.document()["query"]["running"].is_null());
        if size == APP_SIZE {
            continue;
        }
        // The query session locks the table, so its page load waits.
        ui.execute("tab.newQuery", json!({ "text": format!("begin; lock table scratch.{} in access exclusive mode", long.0) }));
        let query = ui.active();
        ui.execute("query.run", json!({}));
        ui.ok("engine.execute", json!({ "command": "table.open", "params": { "schema": "scratch", "table": long.0 }, "wait": false }));
        ui.until("the load", |ui| ui.tag("status-job").is_some_and(|e| e["value"].as_str().is_some_and(|v| v.starts_with("Loading "))));
        let at = format!("{at}, loading a table with a 63-letter name");
        status_gives_way(&mut ui, size, &at);
        let block = rect_of(&ui.tag("status-connection").unwrap());
        let name: String = ui
            .texts()
            .into_iter()
            .filter(|(r, _)| block.expand(1.0).contains(r.center()))
            .flat_map(|(_, g)| g.rows.iter().map(|r| r.text()).collect::<Vec<_>>())
            .collect();
        assert!(name.chars().filter(|c| c.is_alphabetic()).count() >= 3, "{at}: the connection's name keeps letters: `{name}`");
        ui.click("status-cancel");
        ui.until("the cancelled load", |ui| ui.table()["loading"].is_null());
        ui.execute("query.rollback", json!({ "tab": query }));
    }
}

/// The status bar's parts in a window of `size` (`at` says which case): each inside the window,
/// none over another, and the name, SAVE CONNECTION, the job, its CANCEL and the value toggle
/// shown. The parts drawn, with their rects.
fn status_gives_way(ui: &mut Ui, size: egui::Vec2, at: &str) -> Vec<(&'static str, egui::Rect)> {
    let parts = [
        "status-connection",
        "status-address",
        "status-server",
        "status-tls",
        "status-read-only",
        "status-save",
        "status-job",
        "status-cancel",
        "value-toggle",
    ];
    let drawn: Vec<(&str, egui::Rect)> = parts.iter().filter_map(|id| shown_rect(ui, id).map(|r| (*id, r))).collect();
    for (id, r) in &drawn {
        assert!(r.left() >= 0.0 && r.right() <= size.x, "{at}: `{id}` {r:?} inside the window");
    }
    for (k, (a, ra)) in drawn.iter().enumerate() {
        for (b, rb) in &drawn[k + 1..] {
            assert!(!ra.intersects(*rb) || ra.intersect(*rb).area() == 0.0, "{at}: `{a}` {ra:?} and `{b}` {rb:?} overlap");
        }
    }
    for id in ["status-connection", "status-save", "status-job", "status-cancel", "value-toggle"] {
        assert!(drawn.iter().any(|(d, _)| *d == id), "{at}: `{id}` stays: {drawn:?}");
    }
    drawn
}

/// Review 1: Tab from the last menu title reaches one stop before the tree's filter, the status
/// bar's value toggle. Reached by the keyboard, it shows the focus look of an icon button with no
/// edge, a 2 px `ink` box 4 out; a click shows none.
#[test]
fn the_value_toggle_shows_keyboard_focus() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = connected(db);
    let toggle = rect_of(&ui.tag("value-toggle").unwrap());
    let ring = |ui: &Ui| {
        ui.shapes()
            .into_iter()
            .any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.rect == toggle.expand(4.0) && r.stroke.width == 2.0 && r.stroke.color == t.ink))
    };
    ui.click("tree-filter");
    ui.key_with("Tab", json!({ "shift": true }));
    ui.frame();
    use egui_kittest::kittest::{NodeT, Queryable};
    let focused = ui.harness.query_by(|n| n.is_focused()).map(|n| {
        let n = n.accesskit_node();
        (n.role(), n.label(), n.raw_bounds())
    });
    assert!(ring(&ui), "the stop before the filter shows its focus look: {focused:?}, the toggle at {toggle:?}");
    // Enter on it toggles the panel, as a click does.
    let open = ui.value("value-toggle");
    ui.key("Enter");
    ui.frame();
    assert_ne!(ui.value("value-toggle"), open, "Enter toggles it");
    ui.click("value-toggle");
    ui.frame();
    assert!(!ring(&ui), "a click shows no focus look");
}

/// The menu bar is the page's: 30 tall on `bg` with a `line` along its bottom, its titles 24 tall
/// side by side from 4 in, centred above the line; what is under it starts at 30. Its tags stay.
#[test]
fn the_menu_bar_is_30_tall_and_keeps_its_tags() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = connected(db);
    ui.open("shop", "orders");
    let titles: Vec<egui::Rect> = ["File", "Edit", "View", "Query"]
        .iter()
        .map(|top| rect_of(&ui.tag(&format!("menu-{top}")).unwrap_or_else(|| panic!("menu-{top}"))))
        .collect();
    let fills: Vec<(egui::Rect, egui::Color32)> = ui
        .shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 => Some((r.rect, r.fill)),
            _ => None,
        })
        .collect();
    // The bar: the bg fill 30 tall round the titles (the harness holds the window 8 in).
    let bar = fills
        .iter()
        .find(|(r, c)| *c == t.bg && r.height() == 30.0 && r.contains_rect(titles[0]))
        .map(|(r, _)| *r)
        .unwrap_or_else(|| panic!("the bar on bg, 30 tall, round {:?}", titles[0]));
    assert_eq!(titles[0].left() - bar.left(), 4.0, "the first title 4 in: {titles:?} in {bar:?}");
    for r in &titles {
        assert_eq!((r.top() - bar.top(), r.height()), (2.5, 24.0), "a title 24 tall, centred in the 29 above the line: {r:?}");
    }
    for pair in titles.windows(2) {
        assert_eq!(pair[0].right(), pair[1].left(), "side by side: {titles:?}");
    }
    let line = egui::Rect::from_min_max(egui::pos2(bar.left(), bar.bottom() - 1.0), bar.max);
    assert!(fills.contains(&(line, t.line)), "the line along its bottom: {line:?}");
    let tab = ui.active();
    assert_eq!(rect_of(&ui.tag(&format!("tab-{tab}")).unwrap()).top(), bar.bottom(), "the tab strip starts under it");
    // A click opens its menu, which lists its entries as before.
    ui.click("menu-File");
    assert!(ui.tag("menu-tab.newQuery").is_some(), "File's entries");
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

/// A tab in view in the last frame: its id, the rect it is drawn in whole, the part of it the
/// strip shows, and the strip's own rect (what the tabs are clipped to).
#[derive(Clone, Copy, Debug, PartialEq)]
struct InView {
    tab: u64,
    whole: egui::Rect,
    shown: egui::Rect,
    strip: egui::Rect,
}

/// The tabs in view in the last frame, left to right. A tab's whole rect is that of its ground,
/// the 36 tall fill the strip clips to the part it shows. `None` while the tags and the shapes
/// read are of different frames (the strip moved between them).
fn tabs_in_view(ui: &mut Ui) -> Option<Vec<InView>> {
    let i = ui.inspect();
    let shapes = ui.shapes();
    let near = |a: egui::Rect, b: egui::Rect| (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01;
    let mut out = Vec::new();
    for e in i["automation"].as_array().unwrap() {
        let Some(tab) = e["id"].as_str().and_then(|id| id.strip_prefix("tab-")).and_then(|n| n.parse::<u64>().ok()) else { continue };
        if e.get("visible") == Some(&json!(false)) {
            continue;
        }
        let shown = rect_of(e);
        let (whole, strip) = shapes.iter().find_map(|(clip, s)| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 && r.rect.height() == 36.0 && near(r.rect.intersect(*clip), shown) => Some((r.rect, *clip)),
            _ => None,
        })?;
        out.push(InView { tab, whole, shown, strip });
    }
    out.sort_by(|a, b| a.shown.left().total_cmp(&b.shown.left()));
    Some(out)
}

/// Frames until the strip's tabs are where they were the frame before; where they are then.
fn settle(ui: &mut Ui) -> Vec<InView> {
    let mut last: Option<Vec<InView>> = None;
    let mut seen = Vec::new();
    ui.until("the strip to come to rest", |ui| {
        let now = tabs_in_view(ui);
        let same = now.is_some() && last == now;
        last = now;
        if same {
            seen = last.clone().unwrap_or_default();
        }
        same
    });
    seen
}

/// The strip rests at a tab's edge: a tab's left edge at its left, or at the end, the last tab's
/// right edge against + (no empty room after it); and `active` is whole in view, or, wider than
/// what scrolls, at one of its edges.
fn assert_rests(ui: &mut Ui, active: u64, at: &str) {
    let tabs = settle(ui);
    let first = tabs.first().unwrap_or_else(|| panic!("{at}: no tab in view"));
    let last = tabs.last().unwrap_or_else(|| panic!("{at}: no tab in view"));
    let open: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    let at_edge = first.whole.left() == first.strip.left();
    let at_end = open.last() == Some(&last.tab) && last.whole.right() == last.strip.right();
    assert!(at_edge || at_end, "{at}: a tab's left edge at the strip's left, or the last one against +: {tabs:?}");
    let a = tabs.iter().find(|t| t.tab == active).unwrap_or_else(|| panic!("{at}: the active tab {active} in view: {tabs:?}"));
    let wider = a.whole.width() > a.strip.width() && (a.whole.left() == a.strip.left() || a.whole.right() == a.strip.right());
    assert!(a.whole == a.shown || wider, "{at}: the active tab {active} is whole in view: {a:?}");
}

/// The rule until the overflow of tabs is decided (2026-11-11): a strip that
/// scrolls rests with the first tab in view whole at its left edge; each tab made active comes
/// into view whole.
#[test]
fn the_strip_rests_at_a_tab_edge() {
    let db = dbee_testkit::require_testdb!();
    let tables = ["customer_counts", "order_items", "shipments", "all_types", "customers", "orders", "big_rows", "big_cell"];
    for (size, n) in [(APP_SIZE, 4), (APP_SIZE, 8), (egui::vec2(640.0, 420.0), 4), (egui::vec2(640.0, 420.0), 8)] {
        let at = format!("{n} tabs, {} wide", size.x);
        let mut ui = Ui::connected_in(db, size);
        if size == APP_SIZE && ui.value("value-toggle") != true {
            // With the value panel open, as the design draws it: four tabs overflow.
            ui.click("value-toggle");
            assert_eq!(ui.value("value-toggle"), true, "{at}: the value panel shows");
        }
        for table in &tables[..n] {
            ui.execute("table.open", json!({ "schema": "shop", "table": table }));
        }
        let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
        ui.harness.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(size.x / 2.0, size.y - 60.0)));
        let (first, last) = (tabs[0], tabs[n - 1]);
        assert_rests(&mut ui, last, &format!("{at}, the last one just opened"));
        let first_whole = tabs_in_view(&mut ui).is_some_and(|v| v.iter().any(|t| t.tab == first && t.whole == t.shown));
        assert!(ui.tag("tab-list").is_some() && !first_whole, "{at}: the strip overflows: the list button shows, the first tab isn't whole");
        for (k, tab) in tabs.iter().chain(tabs.iter().rev()).enumerate() {
            ui.execute("tab.activate", json!({ "tab": tab }));
            assert_rests(&mut ui, *tab, &format!("{at}, step {k}: tab {tab} made active"));
        }
    }
}

/// While the tabs don't fit, + and the list button stand whole at the strip's right end and stay
/// there while the tabs scroll: the wheel moves them a notch at a time, each coming to rest at the
/// next tab's edge, the last at the end with the last tab against + (no empty room after it).
#[test]
fn new_and_list_stay_at_the_strip_end() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    for n in 1..=8 {
        ui.execute("table.open", json!({ "schema": "many", "table": format!("t_{n:03}") }));
    }
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    ui.execute("tab.activate", json!({ "tab": tabs[0] }));
    let start = settle(&mut ui)[0];
    let (over, strip) = (start.shown.center(), start.strip);
    let pinned = |ui: &mut Ui| ["tab-new", "tab-list"].map(|id| ui.tag(id).filter(|e| visible(&Some(e.clone()))).map(|e| rect_of(&e)));
    let [new, list] = pinned(&mut ui);
    let new = new.unwrap_or_else(|| panic!("+ in view"));
    assert_eq!(new, egui::Rect::from_min_size(egui::pos2(strip.right(), strip.top()), egui::vec2(32.0, 36.0)), "+ right after what scrolls");
    assert_eq!(list, Some(egui::Rect::from_min_size(new.right_top(), egui::vec2(32.0, 36.0))), "the list button after it");
    let at_end = |tabs_now: &[InView]| tabs_now.last().is_some_and(|l| Some(&l.tab) == tabs.last() && l.whole.right() == l.strip.right());
    let mut notches = 0;
    let mut now = settle(&mut ui);
    while !at_end(&now) {
        notches += 1;
        assert!(notches <= tabs.len(), "the wheel never brought the strip to its end");
        ui.hover(over);
        ui.frame_with(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-40.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }]);
        now = settle(&mut ui);
        assert_eq!(pinned(&mut ui), [Some(new), list], "notch {notches}: + and the list button stay put");
        let first = now.first().unwrap_or_else(|| panic!("notch {notches}: a tab in view"));
        let one_further = tabs.get(notches) == Some(&first.tab) && first.whole.left() == first.strip.left();
        assert!(one_further || at_end(&now), "notch {notches}: one tab further, at its edge, or at the end: {now:?}");
    }
    let last = now.last().map(|l| (l.whole, l.shown));
    assert!(last.is_some_and(|(whole, shown)| whole == shown && whole.right() == new.left()), "the last tab whole against +: {last:?}");
    ui.click("tab-new");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(tabs.len() + 1), "+ opens a query tab");
    let active = ui.active();
    assert_rests(&mut ui, active, "the new query tab");
}

/// A window of `size` on a copy of the seeded SQLite file, which needs no test server, on the
/// workspace; the file goes with the target.
fn on_a_file(size: egui::Vec2) -> (Ui, dbee_testkit::TestTarget) {
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let mut ui = Ui::fresh(size);
    let id = ui.execute("connection.save", json!({ "config": config_of(&sqlite) }))["id"].as_str().unwrap().to_string();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert_eq!(ui.document()["screen"], "workspace");
    (ui, sqlite)
}

/// While the tabs don't fit, `tab-list` (⌄) stands after `tab-new` at the strip's right end
/// (value: the number of tabs). It lists every tab in strip order as `tab-list-<n>` (value `{tab,
/// title, active}`); choosing one makes it the active tab and brings it into view. While the tabs
/// fit there is none.
#[test]
fn the_tab_list_lists_every_tab_and_activates_the_chosen_one() {
    let (mut ui, _file) = on_a_file(APP_SIZE);
    ui.ctrl("T");
    ui.frame();
    assert!(ui.tag("tab-list").is_none(), "one tab fits: no list button");
    for _ in 0..7 {
        ui.ctrl("T");
    }
    ui.frame();
    let tabs: Vec<u64> = ui.document()["tabs"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    let (first, last) = (tabs[0], tabs[7]);
    assert_eq!(ui.value("tab-list"), 8, "the list button, with the number of tabs");
    let (new, list) = (rect_of(&ui.tag("tab-new").unwrap()), rect_of(&ui.tag("tab-list").unwrap()));
    assert!(visible(&ui.tag("tab-new")) && visible(&ui.tag("tab-list")), "both in view");
    assert_eq!(list, egui::Rect::from_min_size(new.right_top(), egui::vec2(32.0, 36.0)), "the list button after +");
    ui.until("the last tab in view", |ui| visible(&ui.tag(&format!("tab-{last}"))));
    assert!(!visible(&ui.tag(&format!("tab-{first}"))), "the first one out of view");
    ui.click("tab-list");
    ui.frame();
    for (n, tab) in tabs.iter().enumerate() {
        let title = ui.value(&format!("tab-{tab}"));
        let want = json!({ "tab": tab, "title": title, "active": *tab == last });
        assert_eq!(ui.value(&format!("tab-list-{}", n + 1)), want, "item {} lists tab {tab}", n + 1);
    }
    ui.click("tab-list-1");
    ui.frame();
    assert_eq!(ui.active(), first, "choosing the first makes it active");
    assert!(ui.tag("tab-list-1").is_none(), "the list closed");
    ui.until("the first tab in view", |ui| visible(&ui.tag(&format!("tab-{first}"))));
    assert_rests(&mut ui, first, "chosen from the list");
    ui.click("tab-new");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(9), "+ opens a query tab from the strip's end");
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

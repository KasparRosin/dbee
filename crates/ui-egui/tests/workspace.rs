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
//! window holds a lock: shared, or exclusive for the test that kills backends.

use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionColor, ConnectionConfig, MemoryStore, SecretStore, SslMode};
use dbee_core::{Catalog, ColumnInfo, RelKind, SchemaInfo, TableInfo, TableRef};
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TempHome, TestDb, temp_home};
use dbee_ui_egui::automation::Tags;
use dbee_ui_egui::tree::{self, Tree};
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for anything.
const WAIT: Duration = Duration::from_secs(30);

/// Shared by every window; exclusive while a test kills backends.
static DB_LOCK: RwLock<()> = RwLock::new(());

enum Hold {
    Shared(#[allow(dead_code, reason = "held for its drop")] RwLockReadGuard<'static, ()>),
    Exclusive(#[allow(dead_code, reason = "held for its drop")] RwLockWriteGuard<'static, ()>),
}

struct Ui {
    // Fields drop in order: the window (and its connections) before the lock goes.
    harness: Harness<'static, DbeeApp>,
    tx: mpsc::Sender<ControlRequest>,
    _home: TempHome,
    _hold: Hold,
}

/// A window on a throwaway home with a memory secret store.
fn window(exclusive: bool) -> Ui {
    let hold = if exclusive {
        Hold::Exclusive(DB_LOCK.write().unwrap_or_else(PoisonError::into_inner))
    } else {
        Hold::Shared(DB_LOCK.read().unwrap_or_else(PoisonError::into_inner))
    };
    let home = temp_home(None, None);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let session = Session::new(SessionOptions {
        config_dir: Some(home.home.join("config")),
        secrets: Some(secrets),
        ssh: SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None },
        notify: None,
        allowed_targets: None,
    })
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let app = DbeeApp::new(session, Some(rx));
    let harness = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app);
    let mut ui = Ui { harness, tx, _home: home, _hold: hold };
    ui.frame();
    ui
}

fn pg14(db: &TestDb) -> ConnectionConfig {
    let pg = db.servers.iter().find(|s| s.major == 14).unwrap();
    ConnectionConfig {
        name: "pg14".into(),
        host: "127.0.0.1".into(),
        port: pg.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

/// A window connected to `config` (saved with the test password), on the workspace.
fn connected_to(db: &TestDb, config: ConnectionConfig, exclusive: bool) -> Ui {
    let mut ui = window(exclusive);
    let id = ui.execute("connection.save", json!({ "config": config, "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert_eq!(ui.document()["screen"], "workspace");
    ui
}

fn connected(db: &TestDb) -> Ui {
    connected_to(db, pg14(db), false)
}

impl Ui {
    /// One frame, with the app's synthetic input fed in first.
    fn frame(&mut self) {
        let mut raw = self.harness.input().clone();
        self.harness.state_mut().feed_input(&mut raw);
        *self.harness.input_mut() = raw;
        self.harness.step();
    }

    fn call(&mut self, method: &str, params: Json) -> Json {
        let (req, rx) = ControlRequest::new(method, params);
        self.tx.send(req).unwrap();
        let start = Instant::now();
        loop {
            self.frame();
            if let Ok(r) = rx.try_recv() {
                return r;
            }
            assert!(start.elapsed() < WAIT, "no reply to {method} after {WAIT:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn ok(&mut self, method: &str, params: Json) -> Json {
        let r = self.call(method, params);
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    fn execute(&mut self, command: &str, params: Json) -> Json {
        self.ok("engine.execute", json!({ "command": command, "params": params }))
    }

    fn inspect(&mut self) -> Json {
        self.ok("ui.inspect", json!({}))
    }

    fn document(&mut self) -> Json {
        self.ok("document.inspect", json!({}))
    }

    /// The automation entry `id` of the last drawn frame.
    fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        tag_in(&i, id)
    }

    /// The `value` of automation entry `id`.
    fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
    }

    fn click_with(&mut self, id: &str, params: Json) {
        let mut p = json!({ "id": id });
        if let (Some(p), Json::Object(extra)) = (p.as_object_mut(), params) {
            p.extend(extra);
        }
        self.ok("ui.click", p);
    }

    fn key(&mut self, key: &str, params: Json) {
        let mut p = json!({ "key": key });
        if let (Some(p), Json::Object(extra)) = (p.as_object_mut(), params) {
            p.extend(extra);
        }
        self.ok("ui.key", p);
    }

    fn text(&mut self, text: &str) {
        self.ok("ui.text", json!({ "text": text }));
    }

    /// Runs frames until `done` holds.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Ui) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < WAIT, "gave up waiting for {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The id of the active tab.
    fn active(&mut self) -> u64 {
        self.document()["active"].as_u64().expect("a tab is active")
    }
}

fn tag_in(inspect: &Json, id: &str) -> Option<Json> {
    inspect["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
}

fn ids(inspect: &Json) -> Vec<String> {
    inspect["automation"].as_array().unwrap().iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect()
}

fn visible(entry: &Option<Json>) -> bool {
    entry.as_ref().is_some_and(|e| e.get("visible") != Some(&json!(false)))
}

#[test]
fn ctrl_p_opens_orders() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.key("P", json!({ "cmd": true }));
    assert!(visible(&ui.tag("qs-input")), "Ctrl+P opens the quick search");
    ui.text("ord");
    ui.frame();
    let first = ui.value("qs-item-0");
    assert_eq!(first, "shop.orders", "the best match comes first");
    ui.key("Enter", json!({}));
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
    ui.key("P", json!({ "cmd": true }));
    ui.text("order");
    ui.frame();
    let second = ui.value("qs-item-1");
    assert!(second.is_string(), "two matches at least: {second}");
    // Ctrl+T counts as a dialog's key while the quick search is open: no tab.
    ui.key("T", json!({ "cmd": true }));
    assert_eq!(ui.document()["tabs"], json!([]));
    ui.key("ArrowDown", json!({}));
    ui.key("ArrowDown", json!({}));
    ui.key("ArrowUp", json!({}));
    ui.key("Enter", json!({}));
    ui.until("the table tab", |ui| ui.document()["table"].is_object());
    let doc = ui.document();
    assert_eq!(json!(format!("{}.{}", doc["table"]["schema"].as_str().unwrap(), doc["table"]["name"].as_str().unwrap())), second, "{doc}");
    // Escape closes without opening anything.
    ui.key("P", json!({ "cmd": true }));
    assert!(ui.tag("qs-input").is_some());
    ui.text("customers");
    ui.key("Escape", json!({}));
    assert!(ui.tag("qs-input").is_none(), "Escape closes the quick search");
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(1));
    // Clicking a result opens it too.
    ui.key("P", json!({ "cmd": true }));
    ui.text("customers");
    ui.frame();
    ui.click("qs-item-0");
    ui.until("the customers tab", |ui| ui.document()["table"]["name"] == "customers");
}

#[test]
fn closing_the_quick_search_gives_the_focus_back() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.key("T", json!({ "cmd": true }));
    ui.click("query-editor");
    ui.text("select");
    ui.key("P", json!({ "cmd": true }));
    assert!(ui.tag("qs-input").is_some());
    ui.text("ord");
    ui.key("Escape", json!({}));
    assert!(ui.tag("qs-input").is_none());
    // The editor has the keyboard again: typing goes on there, and Ctrl+A is its own.
    ui.text(" 1");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 1");
    ui.key("A", json!({ "cmd": true }));
    ui.text("select 2");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 2", "Ctrl+A selected the editor's text");
}

#[test]
fn enter_opens_the_hit_for_the_text_typed_in_the_same_frame() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.key("P", json!({ "cmd": true }));
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
    ui.key("T", json!({ "cmd": true }));
    ui.execute("query.setText", json!({ "text": "select 1" }));
    ui.key("W", json!({ "cmd": true }));
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
    ui.key("P", json!({ "cmd": true }));
    assert!(ui.tag("qs-input").is_some());
    // A disconnect the search did not see (an agent's) leaves for the connection screen.
    ui.execute("connection.close", json!({ "force": true }));
    ui.frame();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert!(ui.tag("qs-input").is_none(), "the search closed with the workspace");
    assert_eq!(ui.value("tree-filter"), "", "the tree starts afresh");
    ui.key("T", json!({ "cmd": true }));
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
    ui.key("A", json!({ "cmd": true }));
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
    ui.key("T", json!({ "cmd": true }));
    let tab = ui.active();
    assert_eq!(ui.value(&format!("tab-{tab}")), "Query 1");
    ui.click("query-editor");
    ui.text("select 1");
    ui.frame();
    assert_eq!(ui.document()["query"]["text"], "select 1", "the editor's text goes to the tab");
    ui.key("Q", json!({ "cmd": true, "shift": true }));
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
    ui.key("T", json!({ "cmd": true }));
    ui.key("T", json!({ "cmd": true }));
    assert_eq!(ui.document()["tabs"].as_array().map(Vec::len), Some(2));
    ui.key("W", json!({ "cmd": true }));
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

/// The shapes of the last frame in paint order, each with the rect it is clipped to.
fn shapes(ui: &Ui) -> Vec<(egui::Rect, egui::Shape)> {
    fn walk(clip: egui::Rect, shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Shape)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(clip, s, out)),
            s => out.push((clip, s.clone())),
        }
    }
    let mut out = Vec::new();
    for c in &ui.harness.output().shapes {
        walk(c.clip_rect, &c.shape, &mut out);
    }
    out
}

/// The tabs strip as the last frame drew it: a band of its own with a line along its bottom, the
/// tabs on it, and the active one (`active`) in the content's fill, open into the content (no line
/// under it) with its sides, top and accent drawn.
fn assert_band(ui: &Ui, active: u64, at: &str) {
    let t = dbee_ui_egui::theme::TOKENS;
    let tags = ui.harness.state().tags();
    let tabs: Vec<(u64, egui::Rect)> = tags
        .ids()
        .into_iter()
        .filter_map(|id| id.strip_prefix("tab-")?.parse::<u64>().ok())
        .filter_map(|n| Some((n, tags.rect(&format!("tab-{n}"))?)))
        .collect();
    let tab = tabs.iter().find(|(n, _)| *n == active).map(|(_, r)| *r).unwrap_or_else(|| panic!("{at}: the active tab shows: {tabs:?}"));
    let shapes = shapes(ui);
    let fills: Vec<(egui::Rect, egui::Color32)> = shapes
        .iter()
        .filter_map(|(clip, s)| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 => Some((r.rect.intersect(*clip), r.fill)),
            _ => None,
        })
        .collect();
    // The band: as wide as the strip, behind every tab, which sits on its bottom edge.
    let band = fills
        .iter()
        .filter(|(r, fill)| *fill == t.tab_band && tabs.iter().all(|(_, tab)| r.expand(0.5).contains_rect(*tab)))
        .map(|(r, _)| *r)
        .next()
        .unwrap_or_else(|| panic!("{at}: no band in the tab_band fill behind the tabs {tabs:?}"));
    for (n, r) in &tabs {
        assert!((r.bottom() - band.bottom()).abs() <= 0.5, "{at}: tab {n} {r:?} does not end on the band's bottom edge {band:?}");
        assert!(r.top() > band.top() + 1.0, "{at}: tab {n} {r:?} fills the band {band:?} to its top");
    }
    // The fill on top at a point: the last rect filled there, at least a tab's height.
    let top_fill = |p: egui::Pos2| fills.iter().rev().find(|(r, _)| r.contains(p) && r.height() >= tab.height() - 1.0).map(|(_, f)| *f);
    for (n, r) in &tabs {
        let fill = top_fill(r.center());
        if *n == active {
            assert_eq!(fill, Some(t.bg), "{at}: the active tab takes the content's fill");
        } else {
            assert!(fill != Some(t.bg), "{at}: tab {n} is not active but has the content's fill");
        }
    }
    // The line along the band's bottom: in the border colour, everywhere but under the active tab.
    let y = band.bottom() - 0.5;
    let mut covered: Vec<(f32, f32)> = shapes
        .iter()
        .filter_map(|(clip, s)| match s {
            egui::Shape::LineSegment { points: [a, b], stroke } if stroke.color == t.border && (a.y - y).abs() <= 0.6 && (b.y - y).abs() <= 0.6 => {
                let (l, r) = (a.x.min(b.x).max(clip.left()), a.x.max(b.x).min(clip.right()));
                (r > l).then_some((l, r))
            }
            _ => None,
        })
        .collect();
    covered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let gaps = {
        let mut gaps = Vec::new();
        let mut x = band.left();
        for (l, r) in &covered {
            if *l > x + 1.0 {
                gaps.push((x, *l));
            }
            x = x.max(*r);
        }
        if band.right() > x + 1.0 {
            gaps.push((x, band.right()));
        }
        gaps
    };
    assert_eq!(gaps.len(), 1, "{at}: the bottom line {covered:?} along {band:?} leaves one gap, under the active tab {tab:?}: {gaps:?}");
    let (l, r) = gaps[0];
    assert!(
        (l - tab.left()).abs() <= 1.5 && (r - tab.right()).abs() <= 1.5,
        "{at}: the line's gap {:?} is not the active tab's width {tab:?}",
        (l, r)
    );
    // Its sides and top in the border colour, its accent along the top.
    let along = |a: egui::Pos2, b: egui::Pos2, colour: egui::Color32| {
        shapes.iter().any(|(_, s)| match s {
            egui::Shape::LineSegment { points: [p, q], stroke } => {
                stroke.color == colour && ((p.distance(a) <= 2.5 && q.distance(b) <= 2.5) || (p.distance(b) <= 2.5 && q.distance(a) <= 2.5))
            }
            egui::Shape::Rect(rs) => {
                rs.stroke.color == colour && rs.stroke.width >= 1.0 && rs.rect.expand(1.0).contains(a) && rs.rect.expand(1.0).contains(b) && {
                    let r = rs.rect;
                    let on_edge = |p: egui::Pos2| (p.x - r.left()).abs() <= 1.0 || (p.x - r.right()).abs() <= 1.0 || (p.y - r.top()).abs() <= 1.0;
                    on_edge(a) && on_edge(b)
                }
            }
            _ => false,
        })
    };
    let (tl, tr, bl, br) = (tab.left_top(), tab.right_top(), tab.left_bottom(), tab.right_bottom());
    assert!(along(egui::pos2(tl.x + 0.5, tl.y + 4.0), egui::pos2(bl.x + 0.5, bl.y - 1.0), t.border), "{at}: the active tab's left side is drawn");
    assert!(along(egui::pos2(tr.x - 0.5, tr.y + 4.0), egui::pos2(br.x - 0.5, br.y - 1.0), t.border), "{at}: the active tab's right side is drawn");
    let accent = shapes.iter().any(|(_, s)| match s {
        egui::Shape::LineSegment { points: [p, q], stroke } => {
            stroke.color == t.accent && (p.y - tab.top()).abs() <= 2.5 && (q.x - p.x).abs() >= tab.width() - 8.0
        }
        _ => false,
    });
    assert!(accent, "{at}: the active tab keeps its accent along the top");
}

#[test]
fn the_tabs_sit_on_their_own_band() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    let t = dbee_ui_egui::theme::TOKENS;
    assert_ne!(t.tab_band, t.bg, "the band has a fill of its own");
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
    // More tabs than fit: the strip scrolls sideways. A tab out of view scrolls into view on a
    // click and becomes the active one, and so does the first one again.
    for n in 1..=14 {
        ui.execute("table.open", json!({ "schema": "many", "table": format!("t_{n:03}") }));
    }
    ui.frame();
    let last = ui.active();
    assert!(!visible(&ui.tag(&format!("tab-{last}"))), "the last tab is out of view");
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
    assert!(ui.tag("status-reconnect").is_none(), "no Reconnect while the connection is fine");
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
    let server = db.servers.iter().find(|s| s.major == 14).unwrap();
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
        type_oid: 23,
        base_type: "int4".into(),
        category: 'N',
        not_null: i == 0,
        has_default: false,
    };
    let table = |schema: &str, name: String| TableInfo {
        table: TableRef { schema: schema.into(), name },
        kind: RelKind::Table,
        columns: (0..10).map(column).collect(),
        primary_key: vec!["column_0".into()],
        unique_keys: vec![],
        foreign_keys: vec![],
        estimated_rows: Some(1000),
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
    Arc::new(Catalog { schemas })
}

#[test]
#[ignore = "a timing test: cargo test -p dbee-ui-egui --test workspace -- --ignored --nocapture"]
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
            let _ = tree::show(ui, &s.catalog, &mut s.tree, &mut s.tags);
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

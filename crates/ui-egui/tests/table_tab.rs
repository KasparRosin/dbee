//! The table tab driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the grid (cells, headers, selection, the FK link, the right-click menu), the
//! filter bar and the paging bar. Everything goes through control requests and automation ids,
//! the way an agent drives the real window; assertions read `ui.inspect` and `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is read-only: tests that stage
//! edits or need odd rows make their own table in `scratch` and drop it.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, MemoryStore, SecretStore, SslMode};
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TempHome, TestDb, temp_home, unique};
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for anything.
const WAIT: Duration = Duration::from_secs(60);

struct Ui {
    harness: Harness<'static, DbeeApp>,
    tx: mpsc::Sender<ControlRequest>,
    /// Every text a frame put on the clipboard.
    copied: Vec<String>,
    _home: TempHome,
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

/// A window on a throwaway home, connected to the pg14 test server, on the workspace.
fn connected(db: &TestDb) -> Ui {
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
    let harness = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app);
    let mut ui = Ui { harness, tx, copied: Vec::new(), _home: home };
    ui.frame();
    let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert_eq!(ui.document()["screen"], "workspace");
    ui
}

impl Ui {
    /// One frame, with the app's synthetic input fed in first.
    fn frame(&mut self) {
        let mut raw = self.harness.input().clone();
        self.harness.state_mut().feed_input(&mut raw);
        *self.harness.input_mut() = raw;
        self.harness.step();
        for c in &self.harness.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                self.copied.push(t.clone());
            }
        }
    }

    /// One frame with `events` in its input.
    fn frame_with(&mut self, events: Vec<Event>) {
        self.harness.input_mut().events.extend(events);
        self.frame();
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

    /// The `table` part of `document.inspect`.
    fn table(&mut self) -> Json {
        self.document()["table"].clone()
    }

    fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        tag_in(&i, id)
    }

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

    /// Opens `schema.table` and waits for its first page.
    fn open(&mut self, schema: &str, table: &str) {
        self.execute("table.open", json!({ "schema": schema, "table": table }));
        self.frame();
    }

    /// The centre of automation entry `id`.
    fn centre(&mut self, id: &str) -> Pos2 {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let r = &tag["rect"];
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        Pos2::new(f(0) + f(2) / 2.0, f(1) + f(3) / 2.0)
    }

    /// The text a frame last put on the clipboard, if any did.
    fn copied(&self) -> Option<String> {
        self.copied.last().cloned()
    }
}

fn tag_in(inspect: &Json, id: &str) -> Option<Json> {
    inspect["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
}

fn visible(entry: &Option<Json>) -> bool {
    entry.as_ref().is_some_and(|e| e.get("visible") != Some(&json!(false)))
}

/// Drops the scratch table when the test ends, also when it fails.
struct Scratch(String);

impl Scratch {
    fn new(db: &TestDb, prefix: &str, sql: impl Fn(&str) -> String) -> Scratch {
        let name = unique(prefix);
        dbee_testkit::psql(pg14_server(db), &sql(&name)).unwrap();
        Scratch(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(db) = dbee_testkit::testdb() {
            let _ = dbee_testkit::psql(pg14_server(db), &format!("DROP TABLE IF EXISTS scratch.{} CASCADE", self.0));
        }
    }
}

fn pg14_server(db: &TestDb) -> &dbee_testkit::PgServer {
    db.servers.iter().find(|s| s.major == 14).unwrap()
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
    let mut ui = connected(db);
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
    ui.key("Enter", json!({}));
    ui.until("the page with two filters", |ui| ui.table()["applied"]["filters"].as_array().map(Vec::len) == Some(2));
}

#[test]
fn raw_where_through_the_toggle() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "customers");
    ui.click("filter-raw-toggle");
    assert!(ui.tag("filter-0-column").is_none(), "one raw field instead of the rows");
    ui.click("filter-raw");
    ui.text("id <= 3");
    ui.key("Enter", json!({}));
    ui.until("the raw filter", |ui| ui.table()["applied"] == json!({ "raw": "id <= 3" }));
    let t = ui.table();
    assert_eq!((t["rawMode"].clone(), t["rows"].clone()), (json!(true), json!(3)), "{t}");
    // The toggle goes back to the rows.
    ui.click("filter-raw-toggle");
    assert!(ui.tag("filter-0-column").is_some());
    assert_eq!(ui.table()["rawMode"], false);
}

#[test]
fn a_focused_filter_value_keeps_its_own_ctrl_a() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "customers");
    ui.click("filter-0-value");
    ui.text("abc");
    ui.key("A", json!({ "cmd": true }));
    ui.text("Customer 7");
    ui.frame();
    assert_eq!(ui.value("filter-0-value"), "Customer 7", "Ctrl+A selected the field's text");
    assert!(ui.document()["selection"].is_null(), "and selected no cells");
}

#[test]
fn clicking_fk_link_opens_customer() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
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
    let mut ui = connected(db);
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
    let mut ui = connected(db);
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
    let mut ui = connected(db);
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
fn context_menu_copy_puts_the_selection_on_the_clipboard() {
    let db = dbee_testkit::require_testdb!();
    let table = null_and_empty(db);
    let mut ui = connected(db);
    ui.open("scratch", &table.0);
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    ui.click("grid-menu-copy-json");
    let copied = ui.copied().expect("Copy as JSON put text on the clipboard");
    assert!(copied.contains("\"a\"") && copied.contains("\"x\""), "{copied}");
}

#[test]
fn a_page_draws_in_the_frame_its_rows_arrive() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // The reply goes out in the frame that applies the rows; `ui.inspect` reports that frame.
    ui.execute("table.open", json!({ "schema": "shop", "table": "big_rows" }));
    let i = ui.inspect();
    assert!(
        visible(&tag_in(&i, "grid-cell-0-0")),
        "the first row is drawn in the frame it arrived: {:?} {:?}",
        tag_in(&i, "grid-cell-0-0"),
        tag_in(&i, "page-number")
    );
    assert!(visible(&tag_in(&i, "grid-header-1")));
    // Only the rows in view are laid out; the others are listed as out of view.
    let far = tag_in(&i, "grid-cell-99-0").expect("rows out of view are listed");
    assert_eq!(far["visible"], false, "{far}");
}

#[test]
fn rows_out_of_view_scroll_in_when_clicked() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
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
    let mut ui = connected(db);
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
    ui.key("A", json!({ "cmd": true }));
    let sel = ui.document()["selection"].clone();
    assert_eq!(sel["anchor"], json!([0, 0]));
    assert_eq!(sel["focus"][0], 99, "{sel}");
}

#[test]
fn resized_columns_stay_resized_per_tab() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
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
    let mut ui = connected(db);
    ui.open("shop", "big_rows");
    // The planner's estimate shows until the rows are counted.
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
    let mut ui = connected(db);
    ui.open("shop", "order_totals");
    let note = ui.value("table-note");
    assert!(note.as_str().is_some_and(|n| n.contains("views are read-only")), "{note}");
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

#[test]
fn a_bad_filter_value_shows_the_error_over_the_old_rows() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "customers");
    ui.click("filter-0-value");
    ui.text("not a number");
    ui.key("Enter", json!({}));
    ui.until("the error", |ui| ui.table()["error"].is_string());
    ui.frame();
    let card = ui.value("table-error");
    assert!(card.as_str().is_some_and(|e| e.contains("invalid input syntax")), "{card}");
    assert_eq!(ui.value("grid-cell-0-0")["text"], "1", "the rows from before stay");
    assert!(ui.tag("notice").is_none(), "the card says it, not a notice too");
    // A good filter clears it.
    ui.click("filter-0-value");
    ui.key("A", json!({ "cmd": true }));
    ui.text("7");
    ui.key("Enter", json!({}));
    ui.until("the filtered page", |ui| ui.table()["error"].is_null() && ui.table()["rows"] == 1);
    ui.frame();
    assert!(ui.tag("table-error").is_none());
}

#[test]
fn odd_identifiers_filter_sort_and_follow() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("odd", "Odd \"Name\" 🐝");
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

#[test]
fn twenty_megabyte_cells_cost_a_normal_frame() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.table()["rows"] == 1);
    let text = ui.value("grid-cell-0-1")["text"].as_str().unwrap().to_string();
    assert!(text.chars().count() <= 201 && text.ends_with('…'), "the grid shows the cut text: {} chars", text.chars().count());
    // Selecting it (the value panel builds its view once) and drawing it stay cheap.
    ui.click("grid-cell-0-2");
    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let start = Instant::now();
        ui.frame();
        slowest = slowest.max(start.elapsed());
    }
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
    let mut ui = connected(db);
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
#[ignore = "a timing test: cargo test -p dbee-ui-egui --test table_tab -- --ignored --nocapture"]
fn grid_frame_within_budget() {
    use dbee_core::{Column, ResultSet};
    use dbee_engine::Selection;
    use dbee_ui_egui::automation::Tags;
    use dbee_ui_egui::grid::{Grid, GridState};

    struct State {
        rows: ResultSet,
        grid: GridState,
        tags: Tags,
        selection: Option<Selection>,
    }
    // 10,000 rows of 20 columns (a query result at its cap), one value 20 MB long.
    let huge = "y".repeat(20_000_000);
    let columns = (0..20).map(|c| Column { name: format!("column_{c}"), base_type: "text".into(), type_label: "text".into() }).collect();
    let rows = (0..10_000).map(|r| (0..20).map(|c| Some(if r == 3 && c == 2 { huge.clone() } else { format!("value {r}.{c}") })).collect()).collect();
    let state = State {
        rows: ResultSet { columns, rows, rows_seen: 10_000, complete: true, rows_affected: None },
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
                editable: false,
                links: false,
                first_number: 1,
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

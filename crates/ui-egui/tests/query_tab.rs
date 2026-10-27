//! The query tab driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the SQL editor (keys, Tab, the cursor), Run, Run current and Cancel, the result
//! selector, the read-only result grid with its cap notice and rows-affected line, the error card
//! and the split between editor and results. Everything goes through control requests and
//! automation ids, the way an agent drives the real window; assertions read `ui.inspect` and
//! `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is read-only: a test that writes
//! makes its own table in `scratch` and drops it.

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

fn pg14_server(db: &TestDb) -> &dbee_testkit::PgServer {
    db.servers.iter().find(|s| s.major == 14).unwrap()
}

fn pg14(db: &TestDb) -> ConnectionConfig {
    ConnectionConfig {
        name: "pg14".into(),
        host: "127.0.0.1".into(),
        port: pg14_server(db).port,
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

/// [`connected`], with a new query tab whose editor has the keyboard.
fn query_tab(db: &TestDb) -> Ui {
    let mut ui = connected(db);
    ui.key("T", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.editor()["focused"], true, "a new query tab's editor has the keyboard: {}", ui.editor());
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

    /// The `query` part of `document.inspect`: the active query tab.
    fn query(&mut self) -> Json {
        self.document()["query"].clone()
    }

    fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        tag_in(&i, id)
    }

    fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    /// The editor's tag value: `{cursor, focused, lines, length}`.
    fn editor(&mut self) -> Json {
        self.value("query-editor")
    }

    /// The tags list `enabled: false` for a disabled widget only.
    fn enabled(&mut self, id: &str) -> bool {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["enabled"] != false
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

    /// Runs frames until the active query tab's run has ended, then one more to draw it.
    fn ran(&mut self) {
        self.until("the run to end", |ui| {
            let q = ui.query();
            q["running"].is_null() && (q["results"].as_array().is_some_and(|r| !r.is_empty()) || !q["error"].is_null())
        });
        self.frame();
    }

    /// The rect of automation entry `id`: `[x, y, width, height]`.
    fn rect(&mut self, id: &str) -> [f32; 4] {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        [f(0), f(1), f(2), f(3)]
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

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(db) = dbee_testkit::testdb() {
            let _ = dbee_testkit::psql(pg14_server(db), &format!("DROP TABLE IF EXISTS scratch.{} CASCADE", self.0));
        }
    }
}

const JOIN: &str = "select o.id, c.name, o.total\nfrom shop.orders o\njoin shop.customers c on c.id = o.customer_id\norder by o.id\nlimit 50";

#[test]
fn ctrl_enter_runs_and_shows_results() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    // Pasted straight into the new tab: its editor has the keyboard.
    ui.text(JOIN);
    ui.frame();
    assert_eq!(ui.query()["text"], JOIN);
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    let q = ui.query();
    assert_eq!(q["text"], JOIN, "Ctrl+Enter ran the text and put no newline in it");
    assert!(q["error"].is_null(), "{q}");
    assert_eq!(q["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(q["results"][0]["rows"], 50, "{q}");
    assert!(visible(&ui.tag("grid-cell-0-1")), "the result grid shows its rows");
    assert_eq!(ui.value("grid-header-1")["name"], "name");
    assert_eq!(ui.value("query-rows"), "50 rows");
    assert!(ui.tag("result-1").is_none(), "one result: no selector");
    assert!(ui.tag("query-error").is_none());
    assert!(ui.tag("query-hint").is_none(), "the hint goes once there are results");
    // The editor keeps the keyboard, and the text is highlighted.
    let ed = ui.editor();
    assert_eq!((ed["focused"].clone(), ed["lines"].clone(), ed["highlighted"].clone()), (json!(true), json!(5), json!(true)), "{ed}");
}

#[test]
fn ctrl_shift_enter_runs_current_statement() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql = "select 1 as first;\nselect 2 as second, 3 as third;";
    ui.text(sql);
    // The cursor into the first statement.
    ui.key("Home", json!({ "cmd": true }));
    ui.key("ArrowRight", json!({}));
    ui.key("ArrowRight", json!({}));
    ui.frame();
    assert_eq!(ui.editor()["cursor"], 2, "a byte offset into the text");
    ui.key("Enter", json!({ "cmd": true, "shift": true }));
    ui.ran();
    let q = ui.query();
    assert_eq!(q["text"], sql, "Ctrl+Shift+Enter put no newline in");
    let results = q["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "only the statement under the cursor ran: {q}");
    assert_eq!(results[0]["columns"][0]["name"], "first");
    // Run current with the cursor at the end runs the last statement.
    ui.key("End", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.editor()["cursor"], sql.len());
    ui.click("query-run-current");
    ui.until("the second statement's result", |ui| ui.query()["results"][0]["columns"][0]["name"] == "second");
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(1));
    // Run runs both.
    ui.click("query-run");
    ui.until("both results", |ui| ui.query()["results"].as_array().map(Vec::len) == Some(2));
}

#[test]
fn error_card_and_cursor_jump() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select nope");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    let card = ui.value("query-error");
    assert_eq!(card["code"], "server", "{card}");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("nope")), "{card}");
    assert_eq!((card["position"].clone(), card["line"].clone(), card["column"].clone()), (json!(7), json!(1), json!(8)), "{card}");
    assert!(ui.tag("notice").is_none(), "the card says it, not a notice too");
    // Elsewhere first: the cursor at the start, and the keyboard taken away.
    ui.key("Home", json!({ "cmd": true }));
    ui.key("Escape", json!({}));
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(0), json!(false)), "{ed}");
    // A click on the card puts the cursor where the server points, with the keyboard.
    ui.click("query-error");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(7), json!(true)), "{ed}");
    ui.text("x");
    ui.frame();
    assert_eq!(ui.query()["text"], "select xnope", "typing goes in at the error");
}

#[test]
fn an_error_without_a_position_offers_no_jump() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1/0");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    let card = ui.value("query-error");
    assert!(card["message"].as_str().is_some_and(|m| m.contains("division by zero")), "{card}");
    assert_eq!((card["position"].clone(), card["jump"].clone()), (Json::Null, json!(false)), "{card}");
    ui.key("Home", json!({ "cmd": true }));
    ui.key("Escape", json!({}));
    ui.click("query-error");
    ui.frame();
    let ed = ui.editor();
    assert_eq!((ed["cursor"].clone(), ed["focused"].clone()), (json!(0), json!(false)), "a click does nothing: {ed}");
}

#[test]
fn cancel_button_stops_pg_sleep() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.frame();
    assert!(!ui.enabled("query-cancel"), "nothing runs: Cancel is disabled");
    assert!(ui.enabled("query-run"));
    ui.click("query-run");
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    ui.frame();
    assert!(ui.enabled("query-cancel"), "Cancel is enabled while the run goes");
    assert!(ui.enabled("query-run"), "Run follows the engine's enabled check; a run going is its Busy answer");
    let start = Instant::now();
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
    let took = start.elapsed();
    assert!(took < Duration::from_secs(5), "cancelled in {took:?}");
    ui.frame();
    let card = ui.value("query-error");
    assert_eq!(card["code"], "cancelled", "{card}");
    assert!(!ui.enabled("query-cancel"));
    assert!(ui.tag("notice").is_none(), "the card says it: {:?}", ui.tag("notice"));
}

#[test]
fn ctrl_enter_while_a_run_goes_says_the_engine_is_busy() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select pg_sleep(30)");
    ui.key("Enter", json!({ "cmd": true }));
    ui.until("the run to start", |ui| !ui.query()["running"].is_null());
    let running = ui.query()["running"].clone();
    ui.key("Enter", json!({ "cmd": true }));
    ui.frame();
    let notice = ui.value("notice");
    assert!(notice.as_str().is_some_and(|n| n.contains("busy") && n.contains("a query is running in Query 1")), "{notice}");
    assert_eq!(ui.query()["running"], running, "the run goes on, and no second one started");
    ui.click("query-cancel");
    ui.until("the cancel", |ui| ui.query()["error"]["code"] == "cancelled");
}

#[test]
fn several_results_have_a_selector() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as a;\nselect 'x' as b, 2 as c;\nselect 3 as d");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    assert_eq!(ui.query()["shown"], 2, "the last result shows");
    for n in 1..=3 {
        let v = ui.value(&format!("result-{n}"));
        assert_eq!(v["shown"], n == 3, "result-{n}: {v}");
    }
    assert!(ui.tag("result-0").is_none() && ui.tag("result-4").is_none(), "the ids count from 1");
    assert_eq!(ui.value("grid-header-0")["name"], "d");
    ui.click("result-1");
    assert_eq!(ui.query()["shown"], 0, "result-1 is the engine's index 0");
    ui.frame();
    assert_eq!(ui.value("grid-header-0")["name"], "a");
    assert_eq!(ui.value("result-1")["shown"], true);
    ui.click("result-2");
    ui.frame();
    assert_eq!((ui.value("grid-header-0")["name"].clone(), ui.value("grid-header-1")["name"].clone()), (json!("b"), json!("c")));
    assert_eq!(ui.value("grid-cell-0-0")["text"], "x");
}

#[test]
fn many_results_fit_the_toolbar() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let sql: String = (1..=1200).map(|n| format!("select {n} as n;\n")).collect();
    ui.text(&sql);
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(1200));
    // Not all fit: those around the shown one (the last) do, clear of the row count.
    assert!(ui.tag("result-1").is_none());
    let last = ui.rect("result-1200");
    let rows = ui.rect("query-rows");
    assert!(last[0] + last[2] <= rows[0], "the selector stays clear of the row count: {last:?} {rows:?}");
    assert_eq!(ui.value("query-rows"), "1 row");
    ui.click("result-1199");
    assert_eq!(ui.query()["shown"], 1198);
    ui.frame();
    assert!(ui.tag("result-1200").is_some(), "the neighbours stay in reach");
}

#[test]
fn a_result_cut_at_the_cap_says_so() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select generate_series(1, 25000) as n");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    let r = ui.query()["results"][0].clone();
    assert_eq!((r["rows"].clone(), r["rowsSeen"].clone(), r["complete"].clone()), (json!(10_000), json!(25_000), json!(false)), "{r}");
    assert_eq!(ui.value("query-rows"), "First 10,000 of 25,000 rows");
    // A whole result says how many rows it has.
    ui.key("A", json!({ "cmd": true }));
    ui.text("select generate_series(1, 3) as n");
    ui.key("Enter", json!({ "cmd": true }));
    ui.until("the small result", |ui| ui.query()["results"][0]["rows"] == 3);
    ui.frame();
    assert_eq!(ui.value("query-rows"), "3 rows");
}

#[test]
fn statements_without_rows_show_rows_affected() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch(unique("t_query"));
    let t = &table.0;
    let mut ui = query_tab(db);
    ui.text(&format!("create table scratch.{t} (id int);\ninsert into scratch.{t} values (1), (2);\nupdate scratch.{t} set id = 3 where id = 1"));
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    assert_eq!(ui.query()["results"].as_array().map(Vec::len), Some(3), "{}", ui.query());
    assert_eq!(ui.value("query-affected"), "1 row affected");
    assert!(ui.tag("grid-header-0").is_none(), "no rows, no grid");
    assert!(ui.tag("query-rows").is_none(), "no row count for a statement without rows");
    ui.click("result-2");
    ui.frame();
    assert_eq!(ui.value("query-affected"), "2 rows affected");
}

#[test]
fn a_failed_statement_keeps_the_results_before_it() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as a;\nselect 2 as b;\nselect nope");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    let q = ui.query();
    assert_eq!((q["error"]["statement"].clone(), q["results"].as_array().map(Vec::len)), (json!(2), Some(2)), "{q}");
    let card = ui.value("query-error");
    assert!(card["heading"].as_str().is_some_and(|h| h.contains('3')), "the card names the third statement: {card}");
    assert_eq!(card["line"], 3);
    // The card sits above the results, which stay selectable.
    let (card_rect, grid_rect) = (ui.rect("query-error"), ui.rect("grid-header-0"));
    assert!(card_rect[1] + card_rect[3] <= grid_rect[1], "{card_rect:?} {grid_rect:?}");
    assert_eq!(ui.value("grid-header-0")["name"], "b");
    ui.click("result-1");
    ui.frame();
    assert_eq!(ui.value("grid-header-0")["name"], "a");
    assert!(ui.tag("query-error").is_some(), "the card stays");
}

#[test]
fn tab_inserts_four_spaces_and_keeps_the_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select");
    ui.key("Tab", json!({}));
    ui.text("1");
    ui.frame();
    assert_eq!(ui.query()["text"], "select    1");
    assert_eq!(ui.editor()["focused"], true, "Tab kept the keyboard in the editor");
    // Shift+Tab (no outdent in M1) and Ctrl+Tab do nothing to the text, and keep the keyboard.
    ui.key("Home", json!({}));
    ui.text("    ");
    ui.key("Tab", json!({ "shift": true }));
    ui.key("Tab", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.query()["text"], "    select    1");
    assert_eq!(ui.editor()["focused"], true);
}

#[test]
fn the_value_panel_follows_the_result_grid() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1 as id, '{\"a\": [1, 2]}'::jsonb as doc union all select 2, null");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    ui.click("grid-cell-0-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 1], "focus": [0, 1] }));
    let panel = ui.value("value-panel");
    assert_eq!((panel["column"].clone(), panel["label"].clone(), panel["null"].clone()), (json!("doc"), json!("jsonb"), json!(false)), "{panel}");
    ui.click("grid-cell-1-1");
    assert_eq!(ui.value("value-panel")["null"], true);
    ui.click("grid-cell-1-0");
    assert_eq!(ui.value("value-panel")["column"], "id");
    // The grid's cells copy, once the editor no longer has the keyboard.
    ui.key("C", json!({ "cmd": true }));
    assert_eq!(ui.copied().as_deref(), Some("2"));
}

#[test]
fn the_result_grid_is_read_only() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select id, name from shop.customers order by id limit 5");
    ui.key("Enter", json!({ "cmd": true }));
    ui.ran();
    ui.click("grid-header-0");
    ui.frame();
    assert_eq!(ui.document()["jobs"], json!([]), "a header click sorts nothing");
    ui.click_with("grid-cell-1-1", json!({ "count": 2 }));
    ui.frame();
    assert!(ui.tag("cell-editor").is_none(), "a double-click edits nothing");
    assert!(ui.tag("notice").is_none(), "{:?}", ui.tag("notice"));
    ui.click_with("grid-cell-1-1", json!({ "button": "right" }));
    assert_eq!(ui.tag("grid-menu-copy").map(|t| t["enabled"] != false), Some(true));
    for id in ["grid-menu-null", "grid-menu-delete"] {
        assert!(ui.tag(id).is_none(), "a query's results list no {id}");
    }
    ui.click("grid-menu-copy");
    assert_eq!(ui.copied().as_deref(), Some("Customer 2"));
}

#[test]
fn text_set_elsewhere_reaches_the_editor() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    ui.text("select 1");
    ui.frame();
    // An agent replaces the text: the editor takes it, and typing goes on from there.
    ui.execute("query.setText", json!({ "text": "select 2" }));
    ui.frame();
    assert_eq!(ui.editor()["length"], 8);
    ui.key("End", json!({ "cmd": true }));
    ui.text(" + 1");
    ui.frame();
    assert_eq!(ui.query()["text"], "select 2 + 1");
    // Each tab keeps its own text.
    let first = ui.document()["active"].as_u64().unwrap();
    ui.key("T", json!({ "cmd": true }));
    ui.text("select 'other'");
    ui.frame();
    ui.click(&format!("tab-{first}"));
    ui.frame();
    assert_eq!(ui.query()["text"], "select 2 + 1");
    assert_eq!(ui.editor()["focused"], true, "the editor of the tab switched to has the keyboard");
}

#[test]
fn run_buttons_say_why_they_cannot_run() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    assert!(ui.tag("query-hint").is_some(), "an empty results area says how to run");
    // The buttons follow the engine's own enabled check for `query.run`: connected, a query tab
    // active. Whatever the run itself refuses (no statement, a run going) it says in a notice.
    assert!(ui.enabled("query-run") && ui.enabled("query-run-current"));
    ui.text("-- nothing here");
    ui.click("query-run");
    ui.frame();
    let notice = ui.value("notice");
    assert!(notice.as_str().is_some_and(|n| n.contains("no statement to run")), "{notice}");
}

#[test]
fn the_split_between_editor_and_results_drags() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = query_tab(db);
    let before = ui.rect("query-editor")[3];
    let split = ui.rect("query-split");
    let from = Pos2::new(split[0] + split[2] / 2.0, split[1] + split[3] / 2.0);
    let to = from + egui::vec2(0.0, 120.0);
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    ui.frame_with(vec![Event::PointerMoved(from)]);
    ui.frame_with(vec![button(from, true)]);
    for k in 1..=6 {
        ui.frame_with(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))]);
    }
    ui.frame_with(vec![button(to, false)]);
    ui.frame();
    let after = ui.rect("query-editor")[3];
    assert!((after - before - 120.0).abs() < 2.0, "{before} → {after}");
    // It stays for the window's run: another query tab has the same split.
    ui.key("T", json!({ "cmd": true }));
    ui.frame();
    let other = ui.rect("query-editor")[3];
    assert!((other - after).abs() < 1.0, "{after} vs {other}");
}

#[test]
fn the_layouter_tokenizes_only_when_the_text_changes() {
    use dbee_ui_egui::query_tab::{EditorState, editor};
    let ctx = egui::Context::default();
    let mut ed = EditorState::new("select 1;\n-- two\nselect 'x' from t".into());
    let frame = |ed: &mut EditorState| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            editor(ui, egui::Id::new("timing-editor"), ed);
        })
        .drop_without_applying_deltas();
    };
    for _ in 0..5 {
        frame(&mut ed);
    }
    assert_eq!(ed.tokenized(), 1, "the same text is tokenized once");
    ed.set_text("select 2".into());
    frame(&mut ed);
    frame(&mut ed);
    assert_eq!(ed.tokenized(), 2, "a new text once more");
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

/// SQL of about `bytes` bytes: a script of short lines, or (`one_line`) the same on one line.
fn script(bytes: usize, one_line: bool) -> String {
    let line = "select o.id, c.name, 'shipped' as status, 42.5 from shop.orders o join shop.customers c on c.id = o.customer_id; -- note\n";
    let mut s = String::with_capacity(bytes + line.len());
    while s.len() < bytes {
        s.push_str(line);
    }
    if one_line { s.replace('\n', " ") } else { s }
}

/// The median and the slowest of `frames`.
fn median_max(mut frames: Vec<Duration>) -> (Duration, Duration) {
    frames.sort();
    (frames[frames.len() / 2], frames[frames.len() - 1])
}

/// Keystroke frames (with their tessellation) of an editor alone in a plain egui context, the
/// cursor at the end of `text`: the query editor when `highlighted`, else egui's plain code editor.
fn editor_keystrokes(text: &str, highlighted: bool, keys: usize) -> (Duration, Duration) {
    use dbee_ui_egui::query_tab::{EditorState, editor};
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::light());
    let id = egui::Id::new("timing-editor");
    let mut ed = EditorState::new(text.to_string());
    let mut plain = text.to_string();
    let end = text.chars().count();
    let mut frames = Vec::new();
    for i in 0..=keys {
        let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 700.0))), ..Default::default() };
        if i > 0 {
            raw.events.push(Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
        }
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| {
            if i == 0 {
                // The keyboard, with the cursor at the end.
                ui.memory_mut(|m| m.request_focus(id));
                let mut state = egui::text_edit::TextEditState::default();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(end))));
                egui::TextEdit::store_state(ui.ctx(), id, state);
            }
            if highlighted {
                editor(ui, id, &mut ed);
            } else {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    egui::TextEdit::multiline(&mut plain).id(id).code_editor().desired_width(f32::INFINITY).show(ui);
                });
            }
        });
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        if i > 0 {
            frames.push(start.elapsed());
        }
    }
    let typed = if highlighted { &ed.text } else { &plain };
    assert_eq!(typed.len(), text.len() + keys, "every key went in");
    median_max(frames)
}

/// Keystroke frames (with their tessellation) of the whole window in a plain egui context, which
/// has no AccessKit tree (kittest always builds one; the app builds one only for a screen reader),
/// typing at the end of a query tab holding `text`.
fn window_keystrokes(db: &TestDb, text: &str, keys: usize) -> (Duration, Duration) {
    use dbee_engine::{Outcome, Screen, TabId};
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
    let mut app = DbeeApp::new(session, None);
    let ctx = egui::Context::default();
    let frame = |app: &mut DbeeApp, events: Vec<Event>| {
        let mut raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 900.0))), events, ..Default::default() };
        app.feed_input(&mut raw);
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        start.elapsed()
    };
    let Ok(Outcome::Done(saved)) = app.run("connection.save", json!({ "config": pg14(db), "password": db.password })) else { panic!("saved") };
    app.run("connection.open", json!({ "id": saved["id"] })).unwrap();
    let start = Instant::now();
    while app.session().state().screen != Screen::Workspace {
        assert!(start.elapsed() < WAIT, "connected");
        frame(&mut app, Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
    let Ok(Outcome::Done(tab)) = app.run("tab.newQuery", json!({ "text": text })) else { panic!("a query tab") };
    let id = dbee_ui_egui::query_tab::editor_id(TabId(tab["tab"].as_u64().unwrap()));
    frame(&mut app, Vec::new());
    ctx.memory_mut(|m| m.request_focus(id));
    let mut state = egui::text_edit::TextEditState::default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(text.chars().count()))));
    egui::TextEdit::store_state(&ctx, id, state);
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    let frames = (0..keys).map(|i| frame(&mut app, vec![Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() })])).collect();
    let Some(dbee_engine::Tab::Query(q)) = app.session().state().active_tab() else { panic!("the query tab") };
    assert_eq!(q.text.len(), text.len() + keys, "every key reached the tab");
    median_max(frames)
}

#[test]
#[ignore = "a timing test: cargo test --workspace --test query_tab -- --ignored --nocapture"]
fn keystroke_frames_in_big_scripts() {
    let keys = 20;
    let mut slow_32k = Duration::ZERO;
    println!("keystroke frames, editor alone (median, max), on {}", machine());
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        for one_line in [false, true] {
            let text = script(size, one_line);
            let shape = if one_line { "one line" } else { "short lines" };
            let (hm, hx) = editor_keystrokes(&text, true, keys);
            let (pm, px) = editor_keystrokes(&text, false, keys);
            println!("  {:>5} KiB, {shape:<11}: highlighted {hm:?} ({hx:?}); plain TextEdit {pm:?} ({px:?})", text.len() / 1024);
            if size == 32 * 1024 && !one_line {
                slow_32k = hm;
            }
        }
    }
    // The whole window, typing into a query tab: without an AccessKit tree, then with one (kittest).
    let db = dbee_testkit::require_testdb!();
    println!("keystroke frames, whole window without AccessKit (median, max):");
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        let text = script(size, false);
        let (m, x) = window_keystrokes(db, &text, keys);
        println!("  {:>5} KiB, short lines: {m:?} ({x:?})", text.len() / 1024);
    }
    let mut ui = connected(db);
    println!("keystroke frames, whole window with AccessKit, kittest (median, max):");
    for size in [32 * 1024, 256 * 1024, 1024 * 1024] {
        let text = script(size, false);
        ui.execute("tab.newQuery", json!({ "text": text }));
        ui.frame();
        ui.click("query-editor");
        ui.key("End", json!({ "cmd": true }));
        let mut frames = Vec::new();
        for i in 0..keys {
            ui.harness.input_mut().events.push(Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
            let start = Instant::now();
            ui.frame();
            let shapes = ui.harness.output().shapes.clone();
            let ppp = ui.harness.ctx.pixels_per_point();
            let _ = ui.harness.ctx.tessellate(shapes, ppp);
            frames.push(start.elapsed());
        }
        let (m, x) = median_max(frames);
        println!("  {:>5} KiB, short lines: {m:?} ({x:?})", text.len() / 1024);
        assert_eq!(ui.query()["text"].as_str().map(str::len), Some(text.len() + keys), "every key reached the tab");
        ui.execute("tab.close", json!({ "force": true }));
    }
    // A script of 32 KiB keeps a keystroke within two frames at 60 Hz.
    assert!(slow_32k < Duration::from_millis(33), "{slow_32k:?}");
}

/// `select` of `columns` columns over 10,000 rows: NULLs, or (`text`) short numbers as text.
fn wide(columns: usize, text: bool) -> String {
    let cells: Vec<String> = (1..=columns).map(|i| if text { format!("(g * {i})::text as c{i:04}") } else { format!("null as c{i:04}") }).collect();
    format!("select {} from generate_series(1, 10000) g", cells.join(", "))
}

/// Frames (with their tessellation) of the whole window in a plain egui context, which has no
/// AccessKit tree (as the app without a screen reader), showing the result of `sql` in a query
/// tab: the median and slowest of `n` frames with nothing changing, then of `n` frames that each
/// scroll the result grid down.
fn result_frames(db: &TestDb, sql: &str, n: usize) -> ((Duration, Duration), (Duration, Duration)) {
    use dbee_engine::{Outcome, Screen};
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
    let mut app = DbeeApp::new(session, None);
    let ctx = egui::Context::default();
    let size = egui::vec2(1400.0, 900.0);
    let frame = |app: &mut DbeeApp, events: Vec<Event>| {
        let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        app.feed_input(&mut raw);
        let start = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        let (shapes, ppp) = (std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.drop_without_applying_deltas();
        let _ = ctx.tessellate(shapes, ppp);
        start.elapsed()
    };
    let Ok(Outcome::Done(saved)) = app.run("connection.save", json!({ "config": pg14(db), "password": db.password })) else { panic!("saved") };
    app.run("connection.open", json!({ "id": saved["id"] })).unwrap();
    let start = Instant::now();
    while app.session().state().screen != Screen::Workspace {
        assert!(start.elapsed() < WAIT, "connected");
        frame(&mut app, Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
    app.run("tab.newQuery", json!({ "text": sql })).unwrap();
    app.run("query.run", json!({})).unwrap();
    let start = Instant::now();
    loop {
        frame(&mut app, Vec::new());
        if let Some(dbee_engine::Tab::Query(q)) = app.session().state().active_tab()
            && q.running.is_none()
        {
            break;
        }
        assert!(start.elapsed() < WAIT, "the run ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    let idle = (0..n).map(|_| frame(&mut app, Vec::new())).collect();
    // The wheel over the result grid (the window's lower half).
    let over = Pos2::new(size.x * 0.6, size.y * 0.8);
    let wheel = || Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    frame(&mut app, vec![Event::PointerMoved(over)]);
    let scrolling = (0..n).map(|_| frame(&mut app, vec![wheel()])).collect();
    (median_max(idle), median_max(scrolling))
}

/// How long a frame takes with a wide result in view, by the number of columns (50, 100, 300 and
/// 1,664, Postgres's most in a result), 10,000 rows of NULLs or of short texts: the whole window
/// without an AccessKit tree (the app's case), unchanged and scrolling, and in kittest, which
/// builds one. Reported, not bounded: a wide result's frame grows with its columns
/// (`docs/gaps.md`).
#[test]
#[ignore = "a timing test: cargo test --workspace --test query_tab -- --ignored --nocapture"]
fn wide_result_frames() {
    let db = dbee_testkit::require_testdb!();
    let frames = 15;
    println!("frames with a wide result in view (median, max), on {}", machine());
    for columns in [50, 100, 300, 1_664] {
        for text in [false, true] {
            let sql = wide(columns, text);
            let ((idle, idle_max), (scroll, scroll_max)) = result_frames(db, &sql, frames);
            let mut ui = connected(db);
            ui.execute("tab.newQuery", json!({ "text": sql }));
            ui.execute("query.run", json!({}));
            ui.ran();
            let kittest: Vec<Duration> = (0..frames)
                .map(|_| {
                    let start = Instant::now();
                    ui.frame();
                    start.elapsed()
                })
                .collect();
            let (k, kx) = median_max(kittest);
            let cells = if text { "texts" } else { "NULLs" };
            println!(
                "  {columns:>5} columns of {cells}: window unchanged {idle:?} ({idle_max:?}), scrolling {scroll:?} ({scroll_max:?}); kittest unchanged {k:?} ({kx:?})"
            );
        }
    }
}

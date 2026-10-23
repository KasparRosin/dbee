//! Cell editing driven through the control channel, one frame at a time in a kittest harness over
//! [`DbeeApp::ui`]: the inline editors (double-click, or Enter on the selected cell), their keys
//! (Enter, Ctrl+Enter, Tab, Escape), the boolean choice, "+ Row", Apply with Ctrl+S, undo and redo,
//! and the card of a failed Apply. Everything goes through control requests and automation ids;
//! assertions read `ui.inspect`, `document.inspect` and the database itself (psql).
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). Tests that write make their own table in
//! `scratch` and drop it; the seed is only read.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, MemoryStore, SecretStore, SslMode};
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TempHome, TestDb, temp_home, unique};
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for anything.
const WAIT: Duration = Duration::from_secs(60);

struct Ui {
    harness: Harness<'static, DbeeApp>,
    tx: mpsc::Sender<ControlRequest>,
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
    connected_at(db, 0.25, 1400.0)
}

/// [`connected`] with frames `step` seconds apart (egui's clock in the harness), in a window
/// `width` wide.
fn connected_at(db: &TestDb, step: f32, width: f32) -> Ui {
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
    let harness = Harness::builder().with_size(egui::vec2(width, 900.0)).with_step_dt(step).build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app);
    let mut ui = Ui { harness, tx, _home: home };
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

    fn document(&mut self) -> Json {
        self.ok("document.inspect", json!({}))
    }

    /// The `table` part of `document.inspect`.
    fn table(&mut self) -> Json {
        self.document()["table"].clone()
    }

    fn edits(&mut self) -> Json {
        self.table()["edits"].clone()
    }

    /// The staged value of loaded cell `(row, column)`, when one is staged.
    fn staged(&mut self, row: usize, column: usize) -> Option<Json> {
        let edits = self.edits();
        edits["cells"].as_array().unwrap().iter().find(|c| c["row"] == row && c["column"] == column).map(|c| c["value"].clone())
    }

    fn selection(&mut self) -> Json {
        self.document()["selection"].clone()
    }

    fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.ok("ui.inspect", json!({}));
        i["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
    }

    fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
    }

    fn double_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "count": 2 }));
    }

    fn key(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key }));
    }

    fn ctrl(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true }));
    }

    fn ctrl_shift(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true, "shift": true }));
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

    /// The editor's value, when one is open.
    fn editor(&mut self) -> Option<Json> {
        self.tag("cell-editor").map(|t| t["value"].clone())
    }

    /// The text of the open text editor.
    fn editor_text(&mut self) -> String {
        let v = self.editor().expect("an editor is open");
        v["text"].as_str().unwrap_or_else(|| panic!("not a text editor: {v}")).to_string()
    }

    fn no_notice(&mut self) {
        let n = self.tag("notice");
        assert!(n.is_none(), "no notice was expected: {n:?}");
    }

    fn notice(&mut self) -> String {
        self.value("notice").as_str().unwrap_or_default().to_string()
    }
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

/// `scratch.<u> (id int PK, name text, total numeric(12,2), shipped_at date, flag bool, doc
/// jsonb)` with ids 1 to 3, as the engine's edit tests have it. Columns 0 to 5 in that order.
fn orders(db: &TestDb) -> Scratch {
    let name = unique("t_edit_ui");
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, name text, total numeric(12,2), shipped_at date, flag bool, doc jsonb);
             INSERT INTO scratch.{name} VALUES
                 (1, 'Alice', 10.50, '2025-01-02', true, '{{\"a\": 1}}'),
                 (2, 'Bob', 20.00, '2025-02-03', false, '[1, 2]'),
                 (3, 'Cy', 5.25, NULL, NULL, NULL);"
        ),
    )
    .unwrap();
    Scratch(name)
}

fn q(db: &TestDb, sql: &str) -> String {
    dbee_testkit::psql(pg14_server(db), sql).unwrap().trim().to_owned()
}

/// A window with the scratch table `t` open.
fn opened(db: &TestDb, t: &Scratch) -> Ui {
    let mut ui = connected(db);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui
}

#[test]
fn enter_commits_in_number_cell_and_newlines_in_text() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    // A number: the editor holds the value, typing replaces it, Enter commits.
    ui.double_click("grid-cell-0-2");
    assert_eq!(ui.editor_text(), "10.50");
    assert_eq!(ui.editor().unwrap()["multiline"], false);
    ui.text("5");
    ui.key("Enter");
    assert!(ui.editor().is_none(), "Enter closed the editor");
    assert_eq!(ui.staged(0, 2), Some(json!("5")));
    // Text: Enter is a newline, Ctrl+Enter commits.
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor().unwrap()["multiline"], true);
    ui.text("a");
    ui.key("Enter");
    ui.text("b");
    assert_eq!(ui.editor_text(), "a\nb", "Enter added a line");
    // Ctrl+Shift+Enter (run the statement) is not run from the editor either.
    ui.ctrl_shift("Enter");
    assert_eq!(ui.editor_text(), "a\nb");
    ui.ctrl("Enter");
    assert!(ui.editor().is_none(), "Ctrl+Enter closed the editor");
    assert_eq!(ui.staged(0, 1), Some(json!("a\nb")));
    // The editor took Ctrl+Enter: the query shortcut did not also run (it would say why not).
    ui.no_notice();
    ui.frame();
    let cell = ui.value("grid-cell-0-1");
    assert_eq!(cell["state"], "staged", "{cell}");
}

#[test]
fn clearing_a_date_stages_null() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-3");
    assert_eq!(ui.editor_text(), "2025-01-02");
    ui.ctrl("A");
    ui.key("Delete");
    assert_eq!(ui.editor_text(), "");
    ui.key("Enter");
    assert_eq!(ui.staged(0, 3), Some(Json::Null), "a date has no empty value: clearing stages NULL");
    ui.frame();
    let cell = ui.value("grid-cell-0-3");
    assert_eq!((cell["text"].clone(), cell["null"].clone(), cell["state"].clone()), (json!("NULL"), json!(true), json!("staged")), "{cell}");
    // Clearing text stages an empty string.
    ui.double_click("grid-cell-1-1");
    ui.ctrl("A");
    ui.key("Delete");
    ui.ctrl("Enter");
    assert_eq!(ui.staged(1, 1), Some(json!("")));
}

#[test]
fn tab_commits_and_moves_right_escape_cancels() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    // Enter on the selected cell opens its editor.
    ui.click("grid-cell-1-1");
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "Bob");
    ui.text("Bobby");
    ui.key("Tab");
    assert!(ui.editor().is_none(), "Tab closed the editor");
    assert_eq!(ui.staged(1, 1), Some(json!("Bobby")));
    assert_eq!(ui.selection(), json!({ "anchor": [1, 2], "focus": [1, 2] }), "and moved right");
    // Escape cancels.
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "20.00");
    ui.text("99");
    ui.key("Escape");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(1, 2), None, "nothing staged");
    assert_eq!(ui.edits()["count"], 1);
    // From the last column to the start of the next row.
    ui.click("grid-cell-1-5");
    ui.key("Enter");
    ui.key("Tab");
    assert_eq!(ui.selection(), json!({ "anchor": [2, 0], "focus": [2, 0] }));
    // On the last cell of the last row it commits and stays.
    ui.click("grid-cell-2-5");
    ui.key("Enter");
    ui.text("{\"b\": 2}");
    ui.key("Tab");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 5), Some(json!("{\"b\": 2}")));
    assert_eq!(ui.selection(), json!({ "anchor": [2, 5], "focus": [2, 5] }));
}

#[test]
fn ctrl_s_applies_all_or_nothing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-1");
    ui.text("Ann");
    ui.ctrl("Enter");
    // Ctrl+S with the editor still open commits it first: its value is written too.
    ui.double_click("grid-cell-1-2");
    ui.text("7.25");
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(
        q(db, &format!("SELECT string_agg(name || '|' || total, ',' ORDER BY id) FROM scratch.{} WHERE id IN (1, 2)", t.0)),
        "Ann|10.50,Bob|7.25"
    );
    // A bad date fails the Apply: nothing is written, the card says why, the row is marked.
    ui.double_click("grid-cell-0-1");
    ui.text("Annie");
    ui.ctrl("Enter");
    ui.double_click("grid-cell-1-3");
    ui.text("not a date");
    ui.ctrl("S");
    ui.until("the failed Apply", |ui| ui.table()["failed"].is_object());
    ui.frame();
    let card = ui.value("table-error");
    assert!(card.as_str().is_some_and(|c| c.contains("not a date") && c.contains("nothing was written")), "{card}");
    ui.no_notice();
    let cell = ui.value("grid-cell-1-0");
    assert_eq!(cell["state"], "failed", "the failed row is marked: {cell}");
    assert_eq!(ui.value("grid-cell-0-0").get("state"), None, "and only that row");
    assert_eq!(
        q(db, &format!("SELECT string_agg(name || '|' || shipped_at, ',' ORDER BY id) FROM scratch.{} WHERE id IN (1, 2)", t.0)),
        "Ann|2025-01-02,Bob|2025-02-03"
    );
    assert_eq!(ui.edits()["count"], 2, "every edit stays");
    // Reset drops them, and the card goes with them.
    ui.click("edit-reset");
    ui.frame();
    assert!(ui.tag("table-error").is_none());
}

#[test]
fn cells_that_cannot_be_edited_say_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    ui.open("shop", "order_totals");
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_none(), "no editor on a view");
    let notice = ui.notice();
    assert!(notice.contains("views are read-only"), "{notice}");
    let add = ui.tag("row-add").unwrap();
    assert_eq!(add["enabled"], false, "no + Row on a view: {add}");
    ui.until("the notice to go", |ui| ui.tag("notice").is_none());
    ui.click("grid-cell-1-1");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    let notice = ui.notice();
    assert!(notice.contains("views are read-only"), "{notice}");
}

#[test]
fn a_value_too_large_to_edit_inline_says_so() {
    let db = dbee_testkit::require_testdb!();
    // Made before the session connects, so its catalog has it.
    let cap = dbee_ui_egui::editors::MAX_INLINE_BYTES;
    let name = unique("t_edit_cap");
    dbee_testkit::psql(
        pg14_server(db),
        &format!("CREATE TABLE scratch.{name} (id int PRIMARY KEY, v text); INSERT INTO scratch.{name} VALUES (1, repeat('x', {cap})), (2, repeat('x', {cap} + 1));"),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = connected(db);
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.table()["rows"] == 1);
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_none(), "20 MB does not open inline");
    let notice = ui.notice();
    assert!(notice.contains("too large to edit inline"), "{notice}");
    // A small value of the same row still opens.
    ui.double_click("grid-cell-0-0");
    assert_eq!(ui.editor_text(), "1");
    ui.key("Escape");
    // The cap itself opens; a byte more does not.
    ui.until("the notice to go", |ui| ui.tag("notice").is_none());
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 2);
    ui.double_click("grid-cell-0-1");
    assert!(ui.editor().is_some(), "{cap} bytes open");
    ui.key("Escape");
    ui.double_click("grid-cell-1-1");
    assert!(ui.editor().is_none(), "{} bytes do not", cap + 1);
    assert!(ui.notice().contains("too large to edit inline"));
}

#[test]
fn null_opens_empty_and_enter_alone_sends_nothing() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-2-3");
    assert_eq!(ui.editor_text(), "", "NULL opens empty");
    ui.key("Enter");
    assert!(ui.editor().is_none());
    assert_eq!(ui.edits()["count"], 0, "unchanged: nothing staged");
    // The same with an unchanged value.
    ui.double_click("grid-cell-0-0");
    ui.key("Enter");
    assert_eq!(ui.edits()["count"], 0);
    ui.no_notice();
}

#[test]
fn clicking_another_cell_commits() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-2-1");
    assert_eq!(ui.editor_text(), "Cy");
    ui.text("Cyd");
    ui.click("grid-cell-0-0");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(2, 1), Some(json!("Cyd")));
    assert_eq!(ui.selection(), json!({ "anchor": [0, 0], "focus": [0, 0] }), "the click selected its cell");
}

#[test]
fn quick_double_clicks_on_two_cells_both_open() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // Frames at 60 Hz: the second double-click comes well within egui's double-click time of
    // the first, which egui then counts as a triple click.
    let mut ui = connected_at(db, 1.0 / 60.0, 1400.0);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-1");
    assert_eq!(ui.editor_text(), "Alice");
    ui.key("Escape");
    ui.double_click("grid-cell-1-1");
    assert_eq!(ui.editor_text(), "Bob", "the second cell's editor opened");
}

#[test]
fn an_editor_opens_over_its_own_cell_after_a_wider_one() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    // As narrow as the app's first window: a several-lines editor on the last column is moved
    // left to stay in view.
    let mut ui = connected_at(db, 0.25, 1100.0);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 3);
    ui.double_click("grid-cell-0-5");
    let wide = ui.tag("cell-editor").unwrap();
    let doc = ui.tag("grid-cell-0-5").unwrap();
    assert!(wide["rect"][0].as_f64().unwrap() < doc["rect"][0].as_f64().unwrap(), "moved left: {wide} {doc}");
    ui.key("Escape");
    // The boolean choice that follows is drawn over its own cell from its first frame.
    ui.double_click("grid-cell-1-4");
    let choice = ui.tag("cell-editor").unwrap();
    let flag = ui.tag("grid-cell-1-4").unwrap();
    // egui puts an area on whole points.
    let near = |i: usize| (choice["rect"][i].as_f64().unwrap() - flag["rect"][i].as_f64().unwrap()).abs() <= 1.0;
    assert!(near(0) && near(1), "{choice} {flag}");
    ui.key("Escape");
}

#[test]
fn booleans_choose_true_false_or_null() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "true" })), "the choice opens on the value");
    for id in ["cell-editor-true", "cell-editor-false", "cell-editor-null"] {
        assert!(ui.tag(id).is_some(), "{id}");
    }
    ui.click("cell-editor-null");
    assert!(ui.editor().is_none());
    assert_eq!(ui.staged(0, 4), Some(Json::Null), "NULL sets NULL");
    // The same choice again changes nothing.
    ui.double_click("grid-cell-1-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "false" })));
    ui.click("cell-editor-false");
    assert_eq!(ui.staged(1, 4), None);
    // By keyboard: a NULL cell, up to false, Enter.
    ui.double_click("grid-cell-2-4");
    assert_eq!(ui.editor(), Some(json!({ "choice": "null" })));
    ui.key("ArrowUp");
    assert_eq!(ui.editor(), Some(json!({ "choice": "false" })));
    ui.key("Enter");
    assert_eq!(ui.staged(2, 4), Some(json!("false")));
    ui.double_click("grid-cell-1-4");
    ui.click("cell-editor-true");
    assert_eq!(ui.staged(1, 4), Some(json!("true")));
    ui.ctrl("S");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT string_agg(coalesce(flag::text, 'null'), ',' ORDER BY id) FROM scratch.{}", t.0)), "null,true,false");
}

#[test]
fn a_new_row_is_added_selected_and_edited() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    assert_eq!(ui.value("row-add"), "+ Row");
    ui.click("row-add");
    let edits = ui.edits();
    assert_eq!(edits["newRows"], json!([{ "row": 3, "values": [] }]), "{edits}");
    assert_eq!(ui.selection(), json!({ "anchor": [3, 0], "focus": [3, 0] }), "the new row's first cell is selected");
    ui.frame();
    assert_eq!(ui.value("grid-cell-3-0")["state"], "new");
    ui.key("Enter");
    assert_eq!(ui.editor_text(), "", "a new cell opens empty");
    ui.text("11");
    ui.key("Tab");
    ui.key("Enter");
    ui.text("Dee");
    ui.ctrl("Enter");
    let edits = ui.edits();
    assert_eq!(edits["newRows"][0]["values"], json!([{ "column": 0, "value": "11" }, { "column": 1, "value": "Dee" }]), "{edits}");
    ui.click("edit-apply");
    ui.until("the Apply", |ui| ui.table()["applying"].is_null() && ui.table()["edits"]["count"] == 0);
    assert_eq!(q(db, &format!("SELECT name FROM scratch.{} WHERE id = 11", t.0)), "Dee");
}

#[test]
fn ctrl_z_and_ctrl_shift_z_undo_and_redo_outside_the_editor() {
    let db = dbee_testkit::require_testdb!();
    let t = orders(db);
    let mut ui = opened(db, &t);
    ui.double_click("grid-cell-0-2");
    ui.text("1");
    ui.key("Enter");
    assert_eq!(ui.edits()["count"], 1);
    ui.ctrl("Z");
    assert_eq!(ui.edits()["count"], 0, "Ctrl+Z undid the staged change");
    ui.ctrl_shift("Z");
    assert_eq!(ui.staged(0, 2), Some(json!("1")), "Ctrl+Shift+Z redid it");
    // In the editor they are the editor's own.
    ui.double_click("grid-cell-1-2");
    ui.text("3");
    ui.ctrl("Z");
    assert!(ui.editor().is_some(), "the editor is still open");
    assert_eq!(ui.edits()["count"], 1, "the staged change stays");
    ui.key("Escape");
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
#[ignore = "a timing test: cargo test -p dbee-ui-egui --test editing -- --ignored --nocapture"]
fn keystroke_frame_with_a_value_at_the_cap() {
    let db = dbee_testkit::require_testdb!();
    let cap = dbee_ui_egui::editors::MAX_INLINE_BYTES;
    // One value of each kind just under the cap, each one paragraph (the slow case: egui lays
    // out a paragraph whole on every change): a single-line `varchar` and a multiline `text`.
    let name = unique("t_edit_big");
    let line = cap - 1;
    dbee_testkit::psql(
        pg14_server(db),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, v varchar, t text);
             INSERT INTO scratch.{name} VALUES (1, repeat('x', {line}), repeat('y', {line}));"
        ),
    )
    .unwrap();
    let t = Scratch(name);
    let mut ui = connected(db);
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 1);
    for (col, what) in [(1, "single-line varchar"), (2, "multiline text")] {
        ui.double_click(&format!("grid-cell-0-{col}"));
        assert!(ui.editor().is_some(), "{what} opens");
        // The value opens selected: go to its end, so each key adds to all of it.
        ui.ctrl("End");
        let mut frames = Vec::new();
        for i in 0..30 {
            ui.harness.input_mut().events.push(egui::Event::Text(if i % 2 == 0 { "a".into() } else { "b".into() }));
            let start = Instant::now();
            ui.frame();
            // What the window's renderer is handed too: the frame's shapes as triangles.
            let shapes = ui.harness.output().shapes.clone();
            let ppp = ui.harness.ctx.pixels_per_point();
            let _ = ui.harness.ctx.tessellate(shapes, ppp);
            frames.push(start.elapsed());
        }
        frames.sort();
        let median = frames[frames.len() / 2];
        println!(
            "keystroke in a {what} editor holding {} bytes: frame median {median:?} (max {:?}); on {}",
            cap - 1,
            frames.last().unwrap(),
            machine()
        );
        ui.ctrl("Enter");
        let staged = ui.staged(0, col).and_then(|v| v.as_str().map(str::len));
        assert_eq!(staged, Some(line + 30), "{what}: every key went into the whole value");
        // Two frames at 60 Hz.
        assert!(median < Duration::from_millis(33), "{what}: {median:?}");
    }
}

//! Copying grid cells to the clipboard, driven through the control channel one frame at a time in
//! a kittest harness over [`DbeeApp::ui`]: Ctrl+C (values only), Ctrl+Shift+C (with the header)
//! and the right-click menu's formats, in table and query grids; the "Copied n rows" notice; a
//! grid click taking the keyboard from a text field; the copy cap; the in-memory clipboard of
//! isolated runs; and how long a large copy takes. Assertions read `ui.inspect`,
//! `document.inspect` and the text the frames put on the clipboard (egui's `CopyText` output,
//! which kittest hands back and never reaches the system clipboard).
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read; a test that needs
//! its own rows makes a table in `scratch` and drops it.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, MemoryStore, SecretStore, SslMode};
use dbee_engine::{COPY_MAX_BYTES, Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{TempHome, TestDb, temp_home, unique};
use dbee_ui_egui::app::Clipboard;
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for anything.
const WAIT: Duration = Duration::from_secs(60);

/// The slowest frame a copy may cost (the bound the 20 MB cell tests use).
const FRAME_BOUND: Duration = Duration::from_millis(250);

struct Ui {
    harness: Harness<'static, DbeeApp>,
    tx: mpsc::Sender<ControlRequest>,
    /// Every text a frame put on the clipboard (egui's `CopyText`), oldest first.
    copied: Vec<String>,
    /// The slowest frame since [`Ui::time_frames`].
    slowest: Duration,
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

/// A window on a throwaway home with `app` made from its session, connected to the pg14 test
/// server, on the workspace.
fn connected_with(db: &TestDb, app: impl FnOnce(Session, mpsc::Receiver<ControlRequest>) -> DbeeApp) -> Ui {
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
    let harness = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app(session, rx));
    let mut ui = Ui { harness, tx, copied: Vec::new(), slowest: Duration::ZERO, _home: home };
    ui.frame();
    let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.execute("connection.open", json!({ "id": id }));
    ui.frame();
    assert_eq!(ui.document()["screen"], "workspace");
    ui
}

/// [`connected_with`] the system clipboard (egui's `CopyText`).
fn connected(db: &TestDb) -> Ui {
    connected_with(db, |session, rx| DbeeApp::new(session, Some(rx)))
}

/// [`connected`], with a new query tab that ran `sql` and shows its results. The tab's editor
/// still has the keyboard, as after Ctrl+Enter.
fn ran(db: &TestDb, sql: &str) -> Ui {
    let mut ui = connected(db);
    ui.key("T", json!({ "cmd": true }));
    ui.frame();
    ui.text(sql);
    ui.key("Enter", json!({ "cmd": true }));
    ui.until("the run to end", |ui| {
        let q = ui.document()["query"].clone();
        q["running"].is_null() && (q["results"].as_array().is_some_and(|r| !r.is_empty()) || !q["error"].is_null())
    });
    ui.frame();
    assert!(ui.document()["query"]["error"].is_null(), "{}", ui.document()["query"]);
    assert_eq!(ui.editor()["focused"], true, "the editor keeps the keyboard after a run");
    ui
}

impl Ui {
    /// One frame, with the app's synthetic input fed in first.
    fn frame(&mut self) {
        let start = Instant::now();
        let mut raw = self.harness.input().clone();
        self.harness.state_mut().feed_input(&mut raw);
        *self.harness.input_mut() = raw;
        self.harness.step();
        self.slowest = self.slowest.max(start.elapsed());
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

    /// Starts timing frames afresh: [`Ui::slowest`] from now on.
    fn time_frames(&mut self) {
        self.slowest = Duration::ZERO;
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

    fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        i["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
    }

    fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    /// The query editor's tag value: `{cursor, focused, lines, length, highlighted}`.
    fn editor(&mut self) -> Json {
        self.value("query-editor")
    }

    /// The notice shown, if any.
    fn notice(&mut self) -> Option<String> {
        self.tag("notice").and_then(|t| t["value"].as_str().map(str::to_string))
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
    }

    fn right_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "button": "right" }));
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

    /// The centre of automation entry `id`.
    fn centre(&mut self, id: &str) -> Pos2 {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        Pos2::new(f(0) + f(2) / 2.0, f(1) + f(3) / 2.0)
    }

    /// A primary-button drag from the centre of `from` to the centre of `to`, in small steps.
    fn drag(&mut self, from: &str, to: &str) {
        let (a, b) = (self.centre(from), self.centre(to));
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame_with(vec![Event::PointerMoved(a)]);
        self.frame_with(vec![button(a, true)]);
        for k in 1..=6 {
            self.frame_with(vec![Event::PointerMoved(a + (b - a) * (k as f32 / 6.0))]);
        }
        self.frame_with(vec![button(b, false)]);
        self.frame();
    }

    /// The text a frame last put on the clipboard, if any did.
    fn copied(&self) -> Option<String> {
        self.copied.last().cloned()
    }

    fn open(&mut self, schema: &str, table: &str) {
        self.execute("table.open", json!({ "schema": schema, "table": table }));
        self.frame();
    }
}

/// Drops the scratch table when the test ends, also when it fails.
struct Scratch(String);

impl Scratch {
    fn new(db: &TestDb, prefix: &str, sql: impl FnOnce(&str) -> String) -> Scratch {
        let name = unique(prefix);
        let guard = Scratch(name.clone());
        dbee_testkit::psql(pg14_server(db), &sql(&name)).unwrap();
        guard
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(db) = dbee_testkit::testdb() {
            let _ = dbee_testkit::psql(pg14_server(db), &format!("DROP TABLE IF EXISTS scratch.{} CASCADE", self.0));
        }
    }
}

const CUSTOMERS: &str = "select id, name from shop.customers order by id limit 5";

#[test]
fn ctrl_c_copies_values_only() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_clip", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text, b text); INSERT INTO scratch.{t} VALUES (1, NULL, ''), (2, 'x', 'y');")
    });
    let mut ui = connected(db);
    ui.open("scratch", &table.0);
    ui.key("A", json!({ "cmd": true }));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [1, 2] }), "Ctrl+A selects every cell");
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    let copied = ui.copied().expect("Ctrl+C put text on the clipboard");
    // NULL and the empty string are both empty in TSV; no header line.
    assert_eq!(copied, "1\t\t\n2\tx\ty");
    assert!(!copied.starts_with("id"), "{copied}");
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));
}

#[test]
fn ctrl_shift_c_copies_with_header() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    // A click on a result cell takes the keyboard from the editor: Ctrl+A and Ctrl+Shift+C are
    // the grid's from then on.
    ui.click("grid-cell-1-1");
    assert_eq!(ui.editor()["focused"], false, "the grid took the keyboard");
    ui.key("A", json!({ "cmd": true }));
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [4, 1] }));
    ui.key("C", json!({ "cmd": true, "shift": true }));
    ui.frame();
    let copied = ui.copied().expect("Ctrl+Shift+C put text on the clipboard");
    assert!(copied.starts_with("id\tname\n1\tCustomer 1\n"), "{copied}");
    assert_eq!(copied.lines().count(), 6, "the header and five rows: {copied}");
    assert_eq!(ui.notice().as_deref(), Some("Copied 5 rows"));
    assert_eq!(ui.query_text(), CUSTOMERS, "the editor's text is unchanged");
}

#[test]
fn menu_copy_as_markdown() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    ui.right_click("grid-cell-2-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [2, 1], "focus": [2, 1] }), "a right-click selects the cell");
    ui.click("grid-menu-copy-markdown");
    ui.frame();
    let copied = ui.copied().expect("Copy as Markdown put text on the clipboard");
    assert!(copied.starts_with("| "), "{copied}");
    assert_eq!(copied, "| name |\n|---|\n| Customer 3 |");
    assert_eq!(ui.notice().as_deref(), Some("Copied 1 row"));
}

#[test]
fn every_menu_format_copies_from_a_table_tab() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_clip", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text); INSERT INTO scratch.{t} VALUES (1, NULL), (2, 'x, \"y\"');")
    });
    let mut ui = connected(db);
    ui.open("scratch", &table.0);
    ui.key("A", json!({ "cmd": true }));
    let expected = [
        ("grid-menu-copy", "1\t\n2\tx, \"y\""),
        ("grid-menu-copy-header", "id\ta\n1\t\n2\tx, \"y\""),
        ("grid-menu-copy-csv", "id,a\n1,\n2,\"x, \"\"y\"\"\""),
        ("grid-menu-copy-json", "[\n  {\"id\": 1, \"a\": null},\n  {\"id\": 2, \"a\": \"x, \\\"y\\\"\"}\n]"),
        ("grid-menu-copy-markdown", "| id | a |\n|---|---|\n| 1 | NULL |\n| 2 | x, \"y\" |"),
    ];
    for (id, text) in expected {
        // The selection covers the cell: the right-click keeps it.
        ui.right_click("grid-cell-1-1");
        ui.click(id);
        ui.frame();
        assert_eq!(ui.copied().as_deref(), Some(text), "{id}");
    }
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));
}

#[test]
fn a_text_field_with_the_keyboard_keeps_ctrl_c_until_the_grid_is_clicked() {
    let db = dbee_testkit::require_testdb!();
    // The query editor has the keyboard after a run: Ctrl+A and Ctrl+C are its own.
    let mut ui = ran(db, CUSTOMERS);
    ui.execute("selection.set", json!({ "anchor": [0, 0], "focus": [1, 1] }));
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied(), None, "the editor has nothing selected, and the grid's copy did not run");
    assert_eq!(ui.notice(), None);
    ui.key("A", json!({ "cmd": true }));
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some(CUSTOMERS), "Ctrl+A, Ctrl+C copy the editor's text");
    assert_eq!(ui.notice(), None, "no grid copy ran");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [1, 1] }), "Ctrl+A left the grid's selection alone");
    // A drag over the cells takes the keyboard as a click does.
    ui.drag("grid-cell-1-0", "grid-cell-2-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [2, 1] }));
    assert_eq!(ui.editor()["focused"], false, "the drag took the keyboard from the editor");
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("2\tCustomer 2\n3\tCustomer 3"));
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));

    // A filter input with the keyboard keeps Ctrl+A and Ctrl+C too.
    let mut ui = connected(db);
    ui.open("shop", "customers");
    ui.execute("selection.set", json!({ "anchor": [0, 0] }));
    ui.click("filter-0-value");
    ui.text("Ann");
    ui.key("A", json!({ "cmd": true }));
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Ann"), "the input's own copy");
    assert_eq!(ui.notice(), None, "no grid copy ran");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [0, 0] }));
    ui.click("grid-cell-2-1");
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Customer 3"), "after a click on a cell, Ctrl+C is the grid's");
    assert_eq!(ui.notice().as_deref(), Some("Copied 1 row"));
}

#[test]
fn result_grids_offer_no_edits_and_read_only_tables_say_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    ui.right_click("grid-cell-1-1");
    for id in ["grid-menu-copy", "grid-menu-copy-header", "grid-menu-copy-csv", "grid-menu-copy-json", "grid-menu-copy-markdown"] {
        assert!(ui.tag(id).is_some_and(|t| t["enabled"] != false), "{id} is offered");
    }
    for id in ["grid-menu-null", "grid-menu-delete"] {
        assert!(ui.tag(id).is_none(), "a query's results list no {id}");
    }
    // A view is read-only: the entries are listed, disabled, with the engine's reason.
    let mut ui = connected(db);
    ui.open("shop", "order_totals");
    ui.right_click("grid-cell-0-0");
    for (id, label) in [("grid-menu-null", "Set as NULL"), ("grid-menu-delete", "Delete row")] {
        let tag = ui.tag(id).unwrap_or_else(|| panic!("a table tab lists {id}"));
        assert_eq!(tag["enabled"], false, "{tag}");
        assert_eq!(tag["value"]["label"], label, "{tag}");
        assert_eq!(tag["value"]["why"], "views are read-only", "{tag}");
    }
    let copy = ui.value("grid-menu-copy");
    assert_eq!((copy["label"].clone(), copy["shortcut"].clone(), copy.get("why")), (json!("Copy"), json!("Ctrl+C"), None), "{copy}");
}

#[test]
fn a_copy_over_the_cap_copies_nothing_and_says_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected(db);
    // Its two big cells hold about 40 MB of text together, more than a copy takes.
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.document()["table"]["rows"] == 1);
    ui.execute("selection.set", json!({ "anchor": [0, 1], "focus": [0, 2] }));
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copy", "params": { "format": "tsv" } }));
    assert_eq!(r["code"], "disabled", "{r}");
    let why = r["data"]["why"].as_str().unwrap_or_default().to_string();
    assert!(why.contains("32 MiB") && why.contains("select fewer rows"), "{why}");
    ui.click("grid-cell-0-1");
    ui.key("A", json!({ "cmd": true }));
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied(), None, "nothing went to the clipboard");
    let notice = ui.notice().unwrap_or_default();
    assert!(notice.contains(&why), "the shortcut shows the reason: {notice}");
    // One of them fits.
    ui.click("grid-cell-0-1");
    ui.key("C", json!({ "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied().map(|t| t.len()), Some(20_000_000));
}

#[test]
fn the_memory_clipboard_keeps_the_copy_in_the_app() {
    let db = dbee_testkit::require_testdb!();
    // By default nothing is kept, and inspect has no clipboard.
    let mut ui = connected(db);
    assert_eq!(ui.document().get("clipboard"), None);
    let mut ui = connected_with(db, |session, rx| DbeeApp::new(session, Some(rx)).with_clipboard(Clipboard::Memory));
    assert_eq!(ui.document()["clipboard"], Json::Null, "nothing copied yet");
    ui.key("T", json!({ "cmd": true }));
    ui.execute("query.setText", json!({ "text": CUSTOMERS }));
    ui.execute("query.run", json!({}));
    ui.until("the results", |ui| ui.document()["query"]["results"].as_array().is_some_and(|r| !r.is_empty()));
    ui.frame();
    ui.click("grid-cell-0-0");
    ui.key("A", json!({ "cmd": true }));
    ui.key("C", json!({ "cmd": true, "shift": true }));
    ui.frame();
    let r = ui.execute("clipboard.copy", json!({ "format": "csv" }));
    assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(5), json!(2)), "{r}");
    let kept = ui.document()["clipboard"].clone();
    assert!(kept.as_str().is_some_and(|t| t.starts_with("id,name\n1,Customer 1\n")), "the last copy: {kept}");
    assert_eq!(r["bytes"].as_u64(), kept.as_str().map(|t| t.len() as u64), "{r}");
    assert_eq!(ui.notice().as_deref(), Some("Copied 5 rows"));
    assert!(ui.copied.is_empty(), "no copy reached egui: {:?}", ui.copied.iter().map(String::len).collect::<Vec<_>>());
}

/// How long a large copy takes, from the key press through to the clipboard (the frame that runs
/// `clipboard.copy` and hands the text to egui): a full 10,000-row result, and 10,000 rows
/// holding one byte less than the cap. Prints each format's slowest frame.
#[test]
fn large_copies_stay_under_the_frame_bound() {
    let db = dbee_testkit::require_testdb!();
    let full = "select o.*, i.line_no, i.product, i.qty from shop.order_items i join shop.orders o on o.id = i.order_id order by i.order_id, i.line_no limit 10000";
    let pad = COPY_MAX_BYTES / 10_000;
    let rest = COPY_MAX_BYTES - pad * 10_000 - 1;
    let under = format!("select repeat('x', {pad}) || case when n = 1 then repeat('y', {rest}) else '' end as pad from generate_series(1, 10000) n");
    for (what, sql) in [("10,000 rows", full.to_string()), ("1 byte under the cap", under)] {
        let mut ui = ran(db, &sql);
        assert_eq!(ui.document()["query"]["results"][0]["rows"], 10_000);
        ui.click("grid-cell-0-0");
        ui.key("A", json!({ "cmd": true }));
        for format in ["tsv", "tsvHeader", "csv", "json", "markdown"] {
            ui.copied.clear();
            ui.time_frames();
            let r = if format == "tsv" {
                ui.key("C", json!({ "cmd": true }));
                ui.frame();
                Json::Null
            } else {
                ui.execute("clipboard.copy", json!({ "format": format }))
            };
            let slowest = ui.slowest;
            let bytes = ui.copied().map(|t| t.len()).unwrap_or_else(|| panic!("{what} {format}: nothing copied {r}"));
            eprintln!("copy {what} as {format}: {bytes} bytes, slowest frame {slowest:?}");
            assert!(slowest < FRAME_BOUND, "copying {what} as {format} took a {slowest:?} frame");
        }
    }
}

impl Ui {
    /// The active query tab's text.
    fn query_text(&mut self) -> String {
        self.document()["query"]["text"].as_str().unwrap_or_default().to_string()
    }
}

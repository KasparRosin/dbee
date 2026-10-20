//! The app shell driven through the control channel, one frame at a time in a kittest harness
//! over [`DbeeApp::ui`]. Requests go in as [`ControlRequest`]s, as the control server sends them,
//! and the synthetic input `ui.click`/`ui.key` queue is fed into each frame's raw input the way
//! eframe's `raw_input_hook` does.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and has no SSH directory and no agent. Tests that need a database connect only
//! to the Docker test servers (`require_testdb!`).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, MemoryStore, SslMode};
use dbee_engine::{Session, SessionOptions};
use dbee_testkit::{TempHome, TestDb, temp_home};
use dbee_ui_egui::shortcuts::{self, SHORTCUTS};
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for a reply.
const WAIT: Duration = Duration::from_secs(30);

struct Ui {
    harness: Harness<'static, DbeeApp>,
    tx: mpsc::Sender<ControlRequest>,
    /// Every text the app put on the clipboard, oldest first.
    copied: Vec<String>,
    _home: TempHome,
}

fn ui() -> Ui {
    let home = temp_home(None, None);
    let session = Session::new(SessionOptions {
        config_dir: Some(home.home.join("config")),
        secrets: Some(Arc::new(MemoryStore::default())),
        // No SSH directory and no agent.
        ..SessionOptions::default()
    })
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let app = DbeeApp::new(session, Some(rx));
    let harness = Harness::builder().with_size(egui::vec2(1100.0, 720.0)).build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app);
    Ui { harness, tx, copied: Vec::new(), _home: home }
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

    fn send(&mut self, method: &str, params: Json) -> mpsc::Receiver<Json> {
        let (req, rx) = ControlRequest::new(method, params);
        self.tx.send(req).unwrap();
        rx
    }

    /// Runs frames until `rx` has the reply.
    fn reply(&mut self, rx: &mpsc::Receiver<Json>, what: &str) -> Json {
        let start = Instant::now();
        loop {
            self.frame();
            if let Ok(r) = rx.try_recv() {
                return r;
            }
            assert!(start.elapsed() < WAIT, "no reply to {what} after {WAIT:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn call(&mut self, method: &str, params: Json) -> Json {
        let rx = self.send(method, params);
        self.reply(&rx, method)
    }

    /// `call` that must succeed; its `result`.
    fn ok(&mut self, method: &str, params: Json) -> Json {
        let r = self.call(method, params);
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    fn execute(&mut self, command: &str, params: Json) -> Json {
        self.ok("engine.execute", json!({ "command": command, "params": params }))
    }
}

fn pg14_config(db: &TestDb) -> Json {
    let pg = db.servers.iter().find(|s| s.major == 14).unwrap();
    let config = ConnectionConfig {
        name: "pg14".into(),
        host: "127.0.0.1".into(),
        port: pg.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    json!({ "config": config, "password": db.password })
}

fn automation_ids(inspect: &Json) -> Vec<String> {
    inspect["automation"].as_array().unwrap().iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect()
}

#[test]
fn starts_on_connection_screen() {
    let mut ui = ui();
    ui.frame();
    let i = ui.ok("ui.inspect", json!({}));
    assert_eq!(i["screen"], "connections", "{i}");
    assert_eq!(i["dialog"], Json::Null);
    assert!(i["frame"].as_u64().unwrap() >= 1, "{i}");
    assert_eq!(i["window"], json!([1100.0, 720.0]), "{i}");
    let ids = automation_ids(&i);
    assert!(ids.iter().any(|id| id == "conn-new"), "{ids:?}");
    let new = i["automation"].as_array().unwrap().iter().find(|a| a["id"] == "conn-new").unwrap();
    let rect: Vec<f64> = new["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(rect.len() == 4 && rect[2] > 0.0 && rect[3] > 0.0 && rect[0] + rect[2] <= 1100.0, "{rect:?}");
}

#[test]
fn job_reply_is_deferred_until_the_job_finishes() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    let rx = ui.send("engine.execute", json!({ "command": "connection.open", "params": pg14_config(db) }));
    ui.frame();
    assert!(rx.try_recv().is_err(), "the reply waits for the job");
    let start = Instant::now();
    let reply = loop {
        ui.frame();
        if let Ok(r) = rx.try_recv() {
            break r;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "connection.open did not finish within 10 s of frames");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(reply["result"]["server"].is_string(), "{reply}");
    assert_eq!(ui.ok("ui.inspect", json!({}))["screen"], "workspace");
}

#[test]
fn wait_false_returns_the_job_and_engine_wait_answers_when_idle() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    // Nothing runs: answered at once.
    assert_eq!(ui.call("engine.wait", json!({}))["ok"], true);
    let r = ui.ok("engine.execute", json!({ "command": "connection.open", "params": pg14_config(db), "wait": false }));
    assert!(r["job"].is_u64(), "{r}");
    assert_eq!(ui.call("engine.wait", json!({ "timeoutMs": 20_000 }))["ok"], true);
    let d = ui.ok("document.inspect", json!({}));
    assert_eq!((d["screen"].clone(), d["jobs"].clone()), (json!("workspace"), json!([])), "{d}");
}

#[test]
fn shortcut_table_names_registered_commands() {
    let mut ui = ui();
    let commands = ui.ok("engine.commands", json!({}));
    let ids: HashSet<&str> = commands.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    for (keys, id, params) in SHORTCUTS.iter() {
        assert!(ids.contains(id), "{keys} runs {id}, which is not a registered command");
        assert!(params.is_object(), "{keys}: params must be an object");
        assert!(shortcuts::parse(keys).is_some(), "{keys} does not parse");
    }
    for id in ["ui.quickSearch", "ui.toggleValuePanel", "clipboard.copy"] {
        assert!(ids.contains(id), "engine.commands lists the UI-only command {id}");
    }
    // Every shortcut a menu shows has a binding with the same keys.
    for c in commands.as_array().unwrap() {
        if let Some(keys) = c["shortcut"].as_str() {
            assert!(SHORTCUTS.iter().any(|(k, id, _)| *k == keys && *id == c["id"]), "{} shows {keys}, and no SHORTCUTS row binds it", c["id"]);
        }
    }
}

#[test]
fn commands_search_matches_id_label_and_menu() {
    let mut ui = ui();
    let found = ui.ok("engine.commands", json!({ "search": "COPY" }));
    let ids: Vec<&str> = found.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"copy.selection") && ids.contains(&"clipboard.copy"), "{ids:?}");
    assert!(!ids.contains(&"tab.newQuery"), "{ids:?}");
    let file = ui.ok("engine.commands", json!({ "search": "file" }));
    assert!(file.as_array().unwrap().iter().any(|c| c["id"] == "tab.newQuery"), "{file}");
    assert_eq!(ui.call("engine.commands", json!({ "search": 7 }))["code"], "badParams");
}

#[test]
fn ui_key_runs_the_bound_command() {
    let mut ui = ui();
    ui.frame();
    assert_eq!(ui.call("ui.key", json!({ "key": "T", "cmd": true }))["ok"], true);
    let d = ui.ok("document.inspect", json!({}));
    assert_eq!(d["tabs"].as_array().unwrap().len(), 1, "{d}");
    // A key the table does not bind does nothing.
    assert_eq!(ui.call("ui.key", json!({ "key": "Y", "cmd": true }))["ok"], true);
    assert_eq!(ui.call("ui.key", json!({ "key": "W", "cmd": true }))["ok"], true);
    let d = ui.ok("document.inspect", json!({}));
    assert_eq!(d["tabs"], json!([]), "{d}");
    assert_eq!(ui.call("ui.key", json!({ "key": "NoSuchKey" }))["code"], "badParams");
}

#[test]
fn the_journal_lists_what_ran() {
    let mut ui = ui();
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    let j = ui.ok("engine.journal", json!({}));
    assert_eq!(j, json!([{ "command": "tab.newQuery", "params": { "text": "select 1" } }]));
}

#[test]
fn ui_commands_run_through_engine_execute() {
    let mut ui = ui();
    let r = ui.execute("ui.toggleValuePanel", json!({}));
    assert_eq!(r, json!({ "visible": false }));
    assert_eq!(ui.execute("ui.toggleValuePanel", json!({})), json!({ "visible": true }));
    // Quick search needs a connection; copying needs a selection.
    assert_eq!(ui.call("engine.execute", json!({ "command": "ui.quickSearch" }))["code"], "disabled");
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copy", "params": { "format": "tsv" } }));
    assert_eq!(r["code"], "disabled", "{r}");
}

#[test]
fn clipboard_copy_puts_the_selection_on_the_clipboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    ui.execute("connection.open", pg14_config(db));
    ui.execute("tab.newQuery", json!({ "text": "select 1 as a, 'x' as b" }));
    ui.execute("query.run", json!({}));
    ui.execute("selection.all", json!({}));
    let r = ui.execute("clipboard.copy", json!({ "format": "tsvHeader" }));
    assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(1), json!(2)), "{r}");
    ui.frame();
    assert_eq!(ui.copied.last().map(String::as_str), Some("a\tb\n1\tx"), "{:?}", ui.copied);
    // Ctrl+C with no text field focused copies the values only.
    ui.call("ui.key", json!({ "key": "C", "cmd": true }));
    ui.frame();
    assert_eq!(ui.copied.last().map(String::as_str), Some("1\tx"), "{:?}", ui.copied);
    assert_eq!(ui.call("engine.execute", json!({ "command": "clipboard.copy", "params": { "format": "png" } }))["code"], "badParams");
}

#[test]
fn input_methods_check_their_params() {
    let mut ui = ui();
    ui.frame();
    assert_eq!(ui.call("ui.click", json!({ "id": "no-such-widget" }))["code"], "notFound");
    assert_eq!(ui.call("ui.click", json!({ "x": "left" }))["code"], "badParams");
    assert_eq!(ui.call("ui.click", json!({}))["code"], "badParams");
    assert_eq!(ui.call("ui.text", json!({ "text": 5 }))["code"], "badParams");
    assert_eq!(ui.call("ui.click", json!({ "x": 10.0, "y": 10.0, "button": "middle", "count": 2 }))["ok"], true);
    assert_eq!(ui.call("ui.click", json!({ "id": "conn-new", "count": 1_000_000 }))["code"], "badParams");
    assert_eq!(ui.call("ui.text", json!({ "text": "typed nowhere" }))["ok"], true);
}

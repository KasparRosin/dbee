//! The harness the UI tests share (`mod common;`): a window over [`DbeeApp::ui`] in egui_kittest,
//! run one frame at a time and driven only through control requests and automation ids, the way
//! an agent drives the real window. Each frame first sets the mode chosen and feeds the app's
//! synthetic input ([`DbeeApp::feed_input`]), as eframe's raw-input hook does, and keeps what the frame put on the
//! clipboard (egui's `CopyText`, which kittest hands back and which never reaches the system
//! clipboard).
//!
//! Every window's session gets every driver dbee ships (`dbee_drivers::all()`), unless its
//! [`Setup`] gives fewer.
//!
//! Safety: every window keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore` (or none), and reads SSH keys and `known_hosts` from `<temp home>/.ssh` with no
//! agent, so the real home, `~/.ssh`, SSH agent and keyring are never touched. Windows connect
//! only to the Docker test servers: call `require_testdb!()` before any helper that names one.
//!
//! Killing backends: dbee connects with `application_name = 'dbee'`, so terminating its backends
//! hits every dbee connection on the server, also those of the other tests of the same binary. So
//! every window holds a process-wide lock until it drops: shared, or exclusive
//! ([`Setup::exclusive`]) for a test that kills backends. Hold at most one window per thread at a
//! time while a test of the binary takes the exclusive lock.
#![allow(dead_code, reason = "each test binary uses some of these helpers")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};
use std::time::{Duration, Instant};

use dbee_config::{ConnectionConfig, DbKind, MemoryStore, SecretStore, SslMode};
use dbee_driver::Drivers;
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::{AgentSource, SshEnv};
use dbee_testkit::{Server, TempHome, TestDb, TestTarget, temp_home, unique};
use dbee_ui_egui::{ControlRequest, DbeeApp};
use egui::{Event, Pos2};
use egui_kittest::Harness;
use serde_json::{Value as Json, json};

/// The longest a test waits for a reply or a state.
pub const WAIT: Duration = Duration::from_secs(60);

/// Shared by every window; exclusive while a test kills backends.
static DB_LOCK: RwLock<()> = RwLock::new(());

/// The lock a window holds until it drops.
enum Hold {
    Shared(#[allow(dead_code, reason = "held for its drop")] RwLockReadGuard<'static, ()>),
    Exclusive(#[allow(dead_code, reason = "held for its drop")] RwLockWriteGuard<'static, ()>),
}

/// Makes the app from the window's session and its control receiver (another clipboard, say).
pub type MakeApp = Box<dyn FnOnce(Session, mpsc::Receiver<ControlRequest>) -> DbeeApp>;

/// How a window is made. [`Setup::default`]: 1400 × 900, kittest's frame step, a new throwaway
/// home, a memory secret store, a shared lock, the pg14 test server, every driver, `DbeeApp::new`.
pub struct Setup {
    pub size: egui::Vec2,
    /// Seconds between frames on egui's clock; `None` is kittest's default.
    pub step: Option<f32>,
    /// The home to use; `None` makes a new empty one.
    pub home: Option<TempHome>,
    /// A memory secret store, or none.
    pub secrets: bool,
    /// Hold the lock exclusively (see the module docs).
    pub exclusive: bool,
    /// What [`Ui::connected_with`] connects with; `None` is [`pg14`].
    pub config: Option<ConnectionConfig>,
    /// The session's drivers; `None` is every driver dbee ships.
    pub drivers: Option<Drivers>,
    pub app: Option<MakeApp>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup { size: egui::vec2(1400.0, 900.0), step: None, home: None, secrets: true, exclusive: false, config: None, drivers: None, app: None }
    }
}

/// A window over [`DbeeApp::ui`].
pub struct Ui {
    // Fields drop in this order: the window (closing its connections) before the lock goes.
    pub harness: Harness<'static, DbeeApp>,
    pub tx: mpsc::Sender<ControlRequest>,
    /// Every text a frame put on the clipboard (egui's `CopyText`), oldest first.
    pub copied: Vec<String>,
    /// The slowest frame since [`Ui::time_frames`].
    pub slowest: Duration,
    /// The window's secret store, when it has one.
    pub secrets: Option<Arc<MemoryStore>>,
    /// Connections file in `home/config`, SSH files in `home/.ssh`.
    pub home: TempHome,
    hold: Hold,
}

impl Ui {
    /// A window made as `setup` says, on the connection screen, after its first frame.
    pub fn window(setup: Setup) -> Ui {
        let hold = if setup.exclusive {
            Hold::Exclusive(DB_LOCK.write().unwrap_or_else(PoisonError::into_inner))
        } else {
            Hold::Shared(DB_LOCK.read().unwrap_or_else(PoisonError::into_inner))
        };
        let home = setup.home.unwrap_or_else(|| temp_home(None, None));
        let secrets = setup.secrets.then(|| Arc::new(MemoryStore::default()));
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: secrets.clone().map(|s| s as Arc<dyn SecretStore>),
            ssh: SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None },
            notify: None,
            allowed_targets: None,
            allowed_dirs: None,
            drivers: setup.drivers.unwrap_or_else(dbee_drivers::all),
        })
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let app = match setup.app {
            Some(make) => make(session, rx),
            None => DbeeApp::new(session, Some(rx)),
        };
        let mut builder = Harness::builder().with_size(setup.size);
        if let Some(step) = setup.step {
            builder = builder.with_step_dt(step);
        }
        let harness = builder.build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), app);
        let mut ui = Ui { harness, tx, copied: Vec::new(), slowest: Duration::ZERO, secrets, home, hold };
        ui.frame();
        ui
    }

    /// A window of `size` on a new throwaway home with a memory secret store, on the connection
    /// screen.
    pub fn fresh(size: egui::Vec2) -> Ui {
        Ui::window(Setup { size, ..Setup::default() })
    }

    /// A 1400 × 900 window connected to the pg14 test server, on the workspace.
    pub fn connected(db: &TestDb) -> Ui {
        Ui::connected_with(db, Setup::default())
    }

    /// [`Ui::connected`] in a window of `size`.
    pub fn connected_in(db: &TestDb, size: egui::Vec2) -> Ui {
        Ui::connected_with(db, Setup { size, ..Setup::default() })
    }

    /// A window made as `setup` says that saved `setup.config` (pg14 when `None`) with the test
    /// servers' password and opened it, on the workspace.
    pub fn connected_with(db: &TestDb, mut setup: Setup) -> Ui {
        let config = setup.config.take().unwrap_or_else(|| pg14(db));
        let mut ui = Ui::window(setup);
        let id = ui.execute("connection.save", json!({ "config": config, "password": db.password }))["id"].as_str().unwrap().to_string();
        ui.execute("connection.open", json!({ "id": id }));
        ui.frame();
        assert_eq!(ui.document()["screen"], "workspace");
        ui
    }

    /// One frame, with the app's synthetic input fed in first. Keeps the frame's time in
    /// [`Ui::slowest`] and what it copied in [`Ui::copied`].
    pub fn frame(&mut self) {
        let start = Instant::now();
        let mut raw = self.harness.input().clone();
        let ctx = self.harness.ctx.clone();
        self.harness.state_mut().feed_input(&ctx, &mut raw);
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
    pub fn frame_with(&mut self, events: Vec<Event>) {
        self.harness.input_mut().events.extend(events);
        self.frame();
    }

    /// Starts timing frames afresh: [`Ui::slowest`] from now on.
    pub fn time_frames(&mut self) {
        self.slowest = Duration::ZERO;
    }

    /// Sends a control request; the reply comes on the receiver once frames have run.
    pub fn send(&mut self, method: &str, params: Json) -> mpsc::Receiver<Json> {
        let (req, rx) = ControlRequest::new(method, params);
        self.tx.send(req).unwrap();
        rx
    }

    /// Runs frames until `rx` has the reply.
    pub fn reply(&mut self, rx: &mpsc::Receiver<Json>, what: &str) -> Json {
        let start = Instant::now();
        loop {
            self.frame();
            if let Ok(r) = rx.try_recv() {
                return r;
            }
            assert!(start.elapsed() < WAIT, "no reply to {what} after {WAIT:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A control request and its reply.
    pub fn call(&mut self, method: &str, params: Json) -> Json {
        let rx = self.send(method, params);
        self.reply(&rx, method)
    }

    /// [`Ui::call`] that must succeed; its `result`.
    pub fn ok(&mut self, method: &str, params: Json) -> Json {
        let r = self.call(method, params);
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    /// `engine.execute` of `command`, which must succeed.
    pub fn execute(&mut self, command: &str, params: Json) -> Json {
        self.ok("engine.execute", json!({ "command": command, "params": params }))
    }

    pub fn inspect(&mut self) -> Json {
        self.ok("ui.inspect", json!({}))
    }

    pub fn document(&mut self) -> Json {
        self.ok("document.inspect", json!({}))
    }

    /// The `table` part of `document.inspect`: the active table tab.
    pub fn table(&mut self) -> Json {
        self.document()["table"].clone()
    }

    /// The automation entry `id` of the last drawn frame.
    pub fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        tag_in(&i, id)
    }

    /// The `value` of automation entry `id`, which must have been drawn.
    pub fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    /// The centre of automation entry `id`.
    pub fn centre(&mut self, id: &str) -> Pos2 {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        Pos2::new(f(0) + f(2) / 2.0, f(1) + f(3) / 2.0)
    }

    pub fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
    }

    /// `ui.click` on `id` with more params (`count`, `button`, `modifiers`).
    pub fn click_with(&mut self, id: &str, params: Json) {
        let mut p = json!({ "id": id });
        if let (Some(p), Json::Object(extra)) = (p.as_object_mut(), params) {
            p.extend(extra);
        }
        self.ok("ui.click", p);
    }

    pub fn right_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "button": "right" }));
    }

    pub fn double_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "count": 2 }));
    }

    pub fn key(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key }));
    }

    /// `ui.key` with more params (`shift`, `alt`, …).
    pub fn key_with(&mut self, key: &str, params: Json) {
        let mut p = json!({ "key": key });
        if let (Some(p), Json::Object(extra)) = (p.as_object_mut(), params) {
            p.extend(extra);
        }
        self.ok("ui.key", p);
    }

    /// `key` with Ctrl (Cmd on macOS).
    pub fn ctrl(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true }));
    }

    pub fn ctrl_shift(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true, "shift": true }));
    }

    pub fn text(&mut self, text: &str) {
        self.ok("ui.text", json!({ "text": text }));
    }

    /// Runs frames until `done` holds.
    pub fn until(&mut self, what: &str, mut done: impl FnMut(&mut Ui) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < WAIT, "gave up waiting for {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Opens `schema.table` in a new tab, and draws a frame.
    pub fn open(&mut self, schema: &str, table: &str) {
        self.execute("table.open", json!({ "schema": schema, "table": table }));
        self.frame();
    }

    /// The text a frame last put on the clipboard, if any did.
    pub fn copied(&self) -> Option<String> {
        self.copied.last().cloned()
    }

    /// The pointer moves to `at` and stays there for a frame.
    pub fn hover(&mut self, at: Pos2) {
        self.frame_with(vec![Event::PointerMoved(at)]);
    }

    /// The pointer moves to `at` and rests there for a second (four frames), longer than a
    /// tooltip waits: each move starts that wait again.
    pub fn rest(&mut self, at: Pos2) {
        self.hover(at);
        for _ in 0..4 {
            self.frame();
        }
    }

    /// The shapes of the last frame in paint order, each with the rect it is clipped to.
    pub fn shapes(&self) -> Vec<(egui::Rect, egui::Shape)> {
        fn walk(clip: egui::Rect, shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Shape)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(clip, s, out)),
                s => out.push((clip, s.clone())),
            }
        }
        let mut out = Vec::new();
        for c in &self.harness.output().shapes {
            walk(c.clip_rect, &c.shape, &mut out);
        }
        out
    }

    /// The text shapes of the last frame: each galley where it was drawn.
    pub fn texts(&self) -> Vec<(egui::Rect, Arc<egui::Galley>)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, Arc<egui::Galley>)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                egui::Shape::Text(t) => out.push((t.galley.rect.translate(t.pos.to_vec2()), Arc::clone(&t.galley))),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &self.harness.output().shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// The text drawn inside `within` that reads `text`: where it was drawn and whether it was cut
    /// short (an ellipsis).
    pub fn text_in(&self, within: egui::Rect, text: &str) -> Option<(egui::Rect, bool)> {
        self.texts().into_iter().find(|(r, g)| g.text() == text && within.expand(1.0).contains(r.center())).map(|(r, g)| (r, g.elided))
    }
}

/// A file dropped on the window, as eframe hands one over.
#[derive(Debug)]
pub struct Dropped(pub PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("the test reads no file".into())
    }
}

/// One frame with `paths` dropped on the window.
pub fn drop_files(ui: &mut Ui, paths: &[&Path]) {
    for p in paths {
        ui.harness.input_mut().dropped_files.push(Arc::new(Dropped(p.to_path_buf())));
    }
    ui.frame();
}

/// The settings for `t`: its server on `127.0.0.1` as the kind's user, on the kind's test
/// database (none on MySQL's servers), TLS `prefer`; or its file. Named after the target.
pub fn config_of(t: &TestTarget) -> ConnectionConfig {
    let info = t.kind().info();
    let kind = DbKind::from_id(t.kind().id()).expect("the test kinds are dbee's");
    let base = ConnectionConfig { name: t.name(), kind, ..ConnectionConfig::default() };
    match t.server() {
        Some(s) => ConnectionConfig {
            host: "127.0.0.1".into(),
            port: s.port,
            user: info.user.into(),
            database: info.database.into(),
            ssl_mode: SslMode::Prefer,
            ..base
        },
        None => ConnectionConfig { file: t.file().map(std::path::Path::to_path_buf), ..base },
    }
}

/// A window of `size` connected to `t` (with `database` instead of the kind's, when given), on
/// the workspace. A SQLite target's file goes with the target, which must outlive the window.
pub fn connected_to_target(db: &TestDb, t: &TestTarget, database: Option<&str>, size: egui::Vec2) -> Ui {
    let mut config = config_of(t);
    if let Some(d) = database {
        config.database = d.into();
    }
    Ui::connected_with(db, Setup { size, config: Some(config), ..Setup::default() })
}

/// The app's first window size (`apps/dbee`), for tests where layout matters.
pub const APP_SIZE: egui::Vec2 = egui::vec2(1100.0, 720.0);

/// The automation entry `id` in a `ui.inspect` result.
pub fn tag_in(inspect: &Json, id: &str) -> Option<Json> {
    inspect["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
}

/// Every automation id in a `ui.inspect` result, in drawing order.
pub fn ids(inspect: &Json) -> Vec<String> {
    inspect["automation"].as_array().unwrap().iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect()
}

/// An entry was drawn and is in view (entries list `visible: false` only when scrolled away).
pub fn visible(entry: &Option<Json>) -> bool {
    entry.as_ref().is_some_and(|e| e.get("visible") != Some(&json!(false)))
}

/// The Postgres 14 test server.
pub fn pg14_server(db: &TestDb) -> &Server {
    db.server("pg14").unwrap()
}

/// `postgres@127.0.0.1:<port>/dbee` on `server`, named after it, TLS `prefer`.
pub fn server_config(server: &Server) -> ConnectionConfig {
    ConnectionConfig {
        name: server.name.clone(),
        host: "127.0.0.1".into(),
        port: server.port,
        user: "postgres".into(),
        database: "dbee".into(),
        ssl_mode: SslMode::Prefer,
        ..ConnectionConfig::default()
    }
}

/// [`server_config`] for pg14, named "pg14".
pub fn pg14(db: &TestDb) -> ConnectionConfig {
    server_config(pg14_server(db))
}

/// Drops `scratch.<name>` on pg14 when the test ends, also when it fails.
pub struct Scratch(pub String);

impl Scratch {
    /// Runs `sql(<unique name with prefix>)` on pg14; the table it makes is dropped at the end.
    pub fn new(db: &TestDb, prefix: &str, sql: impl FnOnce(&str) -> String) -> Scratch {
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

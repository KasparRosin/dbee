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
    /// Every command a frame sent the window (`Close`, `CancelClose`, …), oldest first.
    pub sent: Vec<egui::ViewportCommand>,
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
        let mut ui = Ui { harness, tx, copied: Vec::new(), sent: Vec::new(), slowest: Duration::ZERO, secrets, home, hold };
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

    /// Egui's clock in the last frame, in seconds.
    pub fn clock(&self) -> f64 {
        self.harness.ctx.input(|i| i.time)
    }

    /// Runs frames until `running` holds (the first frame that sees the job), then on until egui's
    /// clock is `secs` past that frame, `running` still holding; `check` sees each of those frames
    /// with the time gone.
    pub fn frames_while(&mut self, running: impl Fn(&mut Ui) -> bool, secs: f64, mut check: impl FnMut(&mut Ui, f64)) {
        let start = Instant::now();
        while !running(self) {
            assert!(start.elapsed() < WAIT, "it runs");
            self.frame();
        }
        let first = self.clock();
        loop {
            self.frame();
            let gone = self.clock() - first;
            assert!(running(self), "still running at {gone:.2} s");
            check(self, gone);
            if gone >= secs {
                return;
            }
        }
    }

    /// One frame, with the app's synthetic input fed in first. Keeps the frame's time in
    /// [`Ui::slowest`], what it copied in [`Ui::copied`] and what it sent the window in
    /// [`Ui::sent`].
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
        self.sent.extend(self.harness.output().viewport_output.values().flat_map(|v| v.commands.iter().cloned()));
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

    /// Gives the keyboard to the widget tagged `id`, as Tab would (through AccessKit's focus
    /// action), and draws a frame.
    pub fn focus(&mut self, id: &str) {
        use egui_kittest::kittest::Queryable;
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
        let f = |i: usize| tag["rect"][i].as_f64().unwrap();
        let (x, y, w, h) = (f(0), f(1), f(2), f(3));
        let near = |a: f64, b: f64| (a - b).abs() < 0.5;
        let node = self.harness.query_all_by(move |n| {
            n.data().supports_action(egui::accesskit::Action::Focus)
                && n.raw_bounds().is_some_and(|b| near(b.x0, x) && near(b.y0, y) && near(b.x1, x + w) && near(b.y1, y + h))
        });
        node.into_iter().next().unwrap_or_else(|| panic!("`{id}` takes no focus")).focus();
        self.frame();
        assert!(self.focused(id), "`{id}` has the keyboard");
    }

    /// The widget tagged `id` has the keyboard (AccessKit's focused node lies on its rect).
    pub fn focused(&mut self, id: &str) -> bool {
        use egui_kittest::kittest::{NodeT, Queryable};
        let Some(tag) = self.tag(id) else { return false };
        let f = |i: usize| tag["rect"][i].as_f64().unwrap_or(f64::NAN);
        let (x, y) = (f(0), f(1));
        let near = |a: f64, b: f64| (a - b).abs() < 0.5;
        self.harness.query_by(|n| n.is_focused()).is_some_and(|n| n.accesskit_node().raw_bounds().is_some_and(|b| near(b.x0, x) && near(b.y0, y)))
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
        flat(&self.harness.output().shapes)
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

    /// The notice's toast as the last frame drew it, once it shows (`None` before).
    pub fn toast(&mut self) -> Option<Toast> {
        let tag = self.tag("notice")?;
        let f = |i: usize| tag["rect"][i].as_f64().unwrap_or(0.0) as f32;
        let rect = egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)));
        let shapes = self.shapes();
        let at = shapes.iter().rposition(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.rect == rect && r.fill.a() > 0))?;
        let ground = match &shapes[at].1 {
            egui::Shape::Rect(r) => r.fill,
            _ => return None,
        };
        let on_it = &shapes[at + 1..];
        let inside = |s: &egui::Shape| rect.contains_rect(s.visual_bounding_rect());
        let words = text_colours(&on_it.iter().filter(|(_, s)| inside(s)).cloned().collect::<Vec<_>>());
        let mut marks = Vec::new();
        for (_, s) in on_it.iter().filter(|(_, s)| inside(s)) {
            let drawn = match s {
                egui::Shape::Path(p) => vec![p.fill, if let egui::epaint::ColorMode::Solid(c) = p.stroke.color { c } else { egui::Color32::RED }],
                egui::Shape::Circle(c) => vec![c.fill, c.stroke.color],
                egui::Shape::LineSegment { stroke, .. } => vec![stroke.color],
                _ => continue,
            };
            marks.extend(drawn.into_iter().filter(|c| *c != egui::Color32::TRANSPARENT));
        }
        let blurred = shapes.iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.blur_width > 0.0 && r.rect.intersects(rect)));
        Some(Toast { rect, ground, words, marks, shadow: blurred })
    }
}

impl Ui {
    /// Where `id` was drawn in the last frame (its part in view); it must have been.
    pub fn drawn(&self, id: &str) -> egui::Rect {
        self.harness.state().tags().rect(id).unwrap_or_else(|| panic!("`{id}` was drawn"))
    }

    /// The x range of the tab's results (the grid, the band): from the sidebar to the value panel
    /// while it is drawn beside them, else to the window's edge.
    pub fn body_x(&mut self) -> egui::Rangef {
        let panel =
            |ui: &Ui, id: &str| egui::PanelState::load(&ui.harness.ctx, egui::Id::new(id)).unwrap_or_else(|| panic!("`{id}` is drawn")).outer_rect;
        let whole = self.tab_x();
        let right = if self.tag("value-panel").is_some() { panel(self, "dbee-value-panel").left() } else { whole.max };
        egui::Rangef::new(whole.min, right)
    }

    /// The x range of the tab's body: from the sidebar to the window's edge (the strip, the editor,
    /// the bars).
    pub fn tab_x(&mut self) -> egui::Rangef {
        let panel =
            |ui: &Ui, id: &str| egui::PanelState::load(&ui.harness.ctx, egui::Id::new(id)).unwrap_or_else(|| panic!("`{id}` is drawn")).outer_rect;
        egui::Rangef::new(panel(self, "dbee-tree").right(), panel(self, "dbee-menubar").right())
    }

    /// The pointer moves off the grid, to the empty right end of the menu bar, for a frame.
    pub fn hover_away(&mut self) {
        let bar = egui::PanelState::load(&self.harness.ctx, egui::Id::new("dbee-menubar")).expect("the menu bar").outer_rect;
        self.hover(egui::pos2(bar.right() - 20.0, bar.center().y));
        self.frame();
    }
}

/// What a pixel at `at` shows in the last frame: the colour and rect of the last fill, inside
/// stroke or line drawn over it (within its clip). Text, icons, shadows and fully transparent
/// shapes don't count.
pub fn painted_at(ui: &Ui, at: Pos2) -> Option<(egui::Color32, egui::Rect)> {
    let mut top = None;
    for (clip, shape) in ui.shapes() {
        if !clip.contains(at) {
            continue;
        }
        match shape {
            egui::Shape::Rect(r) if r.blur_width == 0.0 => {
                if r.fill.a() > 0 && r.rect.contains(at) {
                    top = Some((r.fill, r.rect.intersect(clip)));
                }
                let w = r.stroke.width;
                if w > 0.0 && r.stroke.color.a() > 0 {
                    let (outer, inner) = match r.stroke_kind {
                        egui::StrokeKind::Inside => (r.rect, r.rect.shrink(w)),
                        egui::StrokeKind::Middle => (r.rect.expand(w / 2.0), r.rect.shrink(w / 2.0)),
                        egui::StrokeKind::Outside => (r.rect.expand(w), r.rect),
                    };
                    if outer.contains(at) && !inner.contains(at) {
                        top = Some((r.stroke.color, outer.intersect(clip)));
                    }
                }
            }
            egui::Shape::LineSegment { points: [a, b], stroke } if stroke.width > 0.0 && stroke.color.a() > 0 => {
                let line = egui::Rect::from_two_pos(a, b).expand(stroke.width / 2.0);
                if line.contains(at) {
                    top = Some((stroke.color, line.intersect(clip)));
                }
            }
            _ => {}
        }
    }
    top
}

/// The colour a pixel at `at` shows in the last frame ([`painted_at`]).
pub fn colour_at(ui: &Ui, at: Pos2) -> Option<egui::Color32> {
    painted_at(ui, at).map(|(c, _)| c)
}

/// The rects of the last frame's outlines (inside strokes of rects) in `colour` that lie in `within`,
/// with their widths.
pub fn outlines_in(ui: &Ui, within: egui::Rect, colour: egui::Color32) -> Vec<(egui::Rect, f32)> {
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color == colour && within.contains_rect(r.rect) => {
                Some((r.rect, r.stroke.width))
            }
            _ => None,
        })
        .collect()
}

/// The last frame's pixels in a region, as the renderer puts them ([`Ui::pixels`]).
pub struct Pixels {
    x0: i32,
    y0: i32,
    w: usize,
    h: usize,
    /// Premultiplied RGBA, 0 to 1, row by row.
    rgba: Vec<[f32; 4]>,
}

impl Pixels {
    /// The pixel whose top left corner is at `(x, y)`, in pixels (points times the scale, so whole
    /// points at ppp 1); outside the region, transparent.
    pub fn at(&self, x: i32, y: i32) -> egui::Color32 {
        let (dx, dy) = (x - self.x0, y - self.y0);
        if dx < 0 || dy < 0 || dx as usize >= self.w || dy as usize >= self.h {
            return egui::Color32::TRANSPARENT;
        }
        let [r, g, b, a] = self.rgba[dy as usize * self.w + dx as usize].map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
        egui::Color32::from_rgba_premultiplied(r, g, b, a)
    }

    /// The runs of pixels in `colour` along row `y`, from `x0` to `x1` (not included): where each
    /// starts and how many pixels it is wide.
    pub fn runs(&self, y: i32, (x0, x1): (i32, i32), colour: egui::Color32) -> Vec<(i32, i32)> {
        let mut runs: Vec<(i32, i32)> = Vec::new();
        for x in x0..x1 {
            if self.at(x, y) != colour {
                continue;
            }
            match runs.last_mut() {
                Some((start, n)) if *start + *n == x => *n += 1,
                _ => runs.push((x, 1)),
            }
        }
        runs
    }
}

/// Where `p` lies against the edge from `a` to `b`: twice the signed area of the triangle.
fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// A point on the edge from `a` to `b` belongs to the one of the two triangles sharing the edge
/// that runs it this way round, so no pixel is blended twice.
fn owns(a: Pos2, b: Pos2) -> bool {
    let d = b - a;
    d.y > 0.0 || (d.y == 0.0 && d.x < 0.0)
}

impl Ui {
    /// The last frame's pixels in `region` (given in points, at the window's scale) as egui-wgpu draws them: the shapes tessellated
    /// as egui does (thin rects and lines snapped to whole pixels, edges feathered), each mesh cut to
    /// its clip rect rounded to whole pixels, blended in paint order over transparent black. Only
    /// plain fills count: text and images are left out.
    pub fn pixels(&self, region: egui::Rect) -> Pixels {
        let ppp = self.harness.ctx.pixels_per_point();
        let (x0, y0) = ((region.min.x * ppp).floor() as i32, (region.min.y * ppp).floor() as i32);
        let (x1, y1) = ((region.max.x * ppp).ceil() as i32, (region.max.y * ppp).ceil() as i32);
        let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
        let mut rgba = vec![[0.0f32; 4]; w * h];
        for prim in self.harness.ctx.tessellate(self.harness.output().shapes.clone(), ppp) {
            let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else { continue };
            if mesh.texture_id != egui::TextureId::default() {
                continue;
            }
            let clip = prim.clip_rect * ppp;
            let (cx0, cy0) = ((clip.min.x.round() as i32).max(x0), (clip.min.y.round() as i32).max(y0));
            let (cx1, cy1) = ((clip.max.x.round() as i32).min(x1), (clip.max.y.round() as i32).min(y1));
            for tri in mesh.indices.as_chunks::<3>().0 {
                let mut v = tri.map(|i| mesh.vertices[i as usize]);
                for v in &mut v {
                    v.pos = (v.pos.to_vec2() * ppp).to_pos2();
                }
                if v.iter().any(|v| v.uv != egui::epaint::WHITE_UV) {
                    continue;
                }
                let mut area = edge(v[0].pos, v[1].pos, v[2].pos);
                if area == 0.0 {
                    continue;
                }
                if area < 0.0 {
                    v.swap(1, 2);
                    area = -area;
                }
                let [a, b, c] = v.map(|v| v.pos);
                let left = (a.x.min(b.x).min(c.x).floor() as i32).max(cx0);
                let right = (a.x.max(b.x).max(c.x).ceil() as i32).min(cx1);
                let top = (a.y.min(b.y).min(c.y).floor() as i32).max(cy0);
                let bottom = (a.y.max(b.y).max(c.y).ceil() as i32).min(cy1);
                for py in top..bottom {
                    for px in left..right {
                        let p = egui::pos2(px as f32 + 0.5, py as f32 + 0.5);
                        let wts = [(edge(b, c, p), (b, c)), (edge(c, a, p), (c, a)), (edge(a, b, p), (a, b))];
                        if !wts.iter().all(|&(e, (s, t))| e > 0.0 || (e == 0.0 && owns(s, t))) {
                            continue;
                        }
                        let mut src = [0.0f32; 4];
                        for (k, (e, _)) in wts.iter().enumerate() {
                            let col = v[k].color.to_array();
                            for (s, ch) in src.iter_mut().zip(col) {
                                *s += e / area * f32::from(ch) / 255.0;
                            }
                        }
                        let dst = &mut rgba[(py - y0) as usize * w + (px - x0) as usize];
                        let keep = 1.0 - src[3];
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s + *d * keep;
                        }
                    }
                }
            }
        }
        Pixels { x0, y0, w, h, rgba }
    }
}

/// The notice's toast as drawn ([`Ui::toast`]): where, its ground, the colours of the words and of
/// the marks (its icon, its close) drawn on it, and whether a shadow falls round it.
#[derive(Debug, Clone)]
pub struct Toast {
    pub rect: egui::Rect,
    pub ground: egui::Color32,
    pub words: Vec<egui::Color32>,
    pub marks: Vec<egui::Color32>,
    pub shadow: bool,
}

impl Toast {
    /// It is on `ground`, with words and marks all in `fg`, and both drawn.
    pub fn assert_in(&self, ground: egui::Color32, fg: egui::Color32, at: &str) {
        assert_eq!(self.ground, ground, "{at}: the toast's ground: {self:?}");
        assert!(!self.words.is_empty() && self.words.iter().all(|c| *c == fg), "{at}: its words in {fg:?}: {self:?}");
        assert!(self.marks.len() >= 2 && self.marks.iter().all(|c| *c == fg), "{at}: its icon and close in {fg:?}: {self:?}");
        assert!(!self.shadow, "{at}: no shadow: {self:?}");
    }
}

/// `shapes` in paint order, nested lists flattened, each with the rect it is clipped to.
pub fn flat(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Shape)> {
    fn walk(clip: egui::Rect, shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Shape)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(clip, s, out)),
            s => out.push((clip, s.clone())),
        }
    }
    let mut out = Vec::new();
    for c in shapes {
        walk(c.clip_rect, &c.shape, &mut out);
    }
    out
}

/// The colours `shapes` draw their texts in.
pub fn text_colours(shapes: &[(egui::Rect, egui::Shape)]) -> Vec<egui::Color32> {
    let mut out = Vec::new();
    for (_, shape) in shapes {
        if let egui::Shape::Text(t) = shape {
            match t.override_text_color {
                Some(c) => out.push(c),
                None => out.extend(
                    t.galley
                        .job
                        .sections
                        .iter()
                        .map(|s| if s.format.color == egui::Color32::PLACEHOLDER { t.fallback_color } else { s.format.color }),
                ),
            }
        }
    }
    out
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

/// A button that moved to `components::button`: its automation id, the value it is tagged with
/// (`None` for none), whether it is enabled, and the words it draws.
pub type Moved<'a> = (&'a str, Option<Json>, bool, &'a str);

/// Each of `want` is in the last frame's `ui.inspect` with its value and its enabled flag as
/// before the move, 28 tall, its words drawn in capitals inside it.
pub fn assert_moved(ui: &mut Ui, want: &[Moved<'_>], at: &str) {
    let i = ui.inspect();
    for (id, value, enabled, words) in want {
        let e = tag_in(&i, id).unwrap_or_else(|| panic!("{at}: `{id}` is drawn: {:?}", ids(&i)));
        assert_eq!(e.get("value"), value.as_ref(), "{at}: `{id}`'s value: {e}");
        assert_eq!(e.get("enabled") != Some(&json!(false)), *enabled, "{at}: `{id}` enabled: {e}");
        let r: Vec<f32> = (0..4).map(|k| e["rect"][k].as_f64().unwrap() as f32).collect();
        let rect = egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]));
        assert_eq!(rect.height(), 28.0, "{at}: `{id}` is a button of the page, 28 tall: {rect:?}");
        assert!(ui.text_in(rect, words).is_some(), "{at}: `{id}` draws `{words}` in {rect:?}");
    }
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

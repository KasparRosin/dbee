//! The app shell: [`DbeeApp`] runs one window's frames over a [`Session`].
//!
//! Frame order ([`DbeeApp::ui`]):
//! 1. `session.pump()`: the results of finished jobs are applied;
//! 2. the control replies that waited (for a job, idleness, input or a picture) are answered;
//! 3. the control requests that arrived are handled ([`crate::control`]);
//! 4. the keyboard shortcuts run ([`crate::shortcuts`]), before any widget sees the keys;
//! 5. the screen is drawn, tagging its widgets ([`crate::automation`]);
//! 6. the session's UI requests (dialogs, notices) are taken, to show from the next frame.
//!
//! While a job runs the window repaints every 100 ms, so waits time out and progress shows even
//! without input. The UI is thin: screens read `session.state()` and act through
//! [`DbeeApp::run`]. Errors of commands the user started (a shortcut, a button) show as a notice;
//! a command that is disabled right now does nothing. Logs name command ids and error codes only.
//!
//! The connection screen is a stub for now (a "New connection" button and an empty list); the
//! workspace shows the connection and its tabs.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use dbee_engine::{CommandInfo, EngineError, JobId, Json, Outcome, Screen, Session};
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, vec2};

use crate::automation::Tags;
use crate::control::{self, ControlRequest, Deferred, Input};
use crate::theme::{RADIUS, TOKENS};
use crate::{icons, shortcuts, theme, ui_commands};

/// How often a busy session repaints.
const BUSY_REPAINT: Duration = Duration::from_millis(100);

/// How long a notice shows, in seconds.
const NOTICE_SECS: f64 = 4.0;

/// Most control requests handled in one frame; the rest wait for the next.
const REQUESTS_PER_FRAME: usize = 64;

/// The window over one session.
pub struct DbeeApp {
    pub(crate) session: Session,
    control: Option<Receiver<ControlRequest>>,
    pub(crate) tags: Tags,
    pub(crate) view: View,
    pub(crate) input: Input,
    pub(crate) deferred: Vec<Deferred>,
    /// The last screenshot token handed out.
    pub(crate) shots: u64,
    /// `app.quit` was answered when this many frames were drawn: close the window at a later
    /// frame, from this instant on.
    pub(crate) quit: Option<(u64, std::time::Instant)>,
    frames: u64,
    /// Jobs started by the user's own actions, whose errors show as notices.
    ui_jobs: Vec<(JobId, &'static str)>,
    clipboard: Vec<String>,
    themed: bool,
}

/// What only the window keeps.
#[derive(Debug)]
pub(crate) struct View {
    pub value_panel: bool,
    pub quick_search: bool,
    /// The dialog a command asked for (`UiRequest::Dialog`).
    pub dialog: Option<Dialog>,
    /// Notices with the time they were first shown (`None` until then).
    pub notices: VecDeque<(String, Option<f64>)>,
    /// The connection screen shows a new, empty connection.
    pub new_connection: bool,
    /// The query editor's cursor, a byte offset into the active query tab's text, for
    /// Ctrl+Shift+Enter (run the statement under the cursor).
    pub query_cursor: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Dialog {
    pub name: &'static str,
    #[allow(dead_code, reason = "the dialogs that read it come with the connection form")]
    pub data: Json,
}

impl Default for View {
    fn default() -> Self {
        View { value_panel: true, quick_search: false, dialog: None, notices: VecDeque::new(), new_connection: false, query_cursor: None }
    }
}

/// Most notices kept; older ones are dropped.
const MAX_NOTICES: usize = 8;

impl std::fmt::Debug for DbeeApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbeeApp").field("session", &self.session).field("frames", &self.frames).finish_non_exhaustive()
    }
}

impl DbeeApp {
    /// The app over `session`; `control` brings the control channel's requests
    /// ([`crate::control_server::start`]).
    pub fn new(session: Session, control: Option<Receiver<ControlRequest>>) -> DbeeApp {
        DbeeApp {
            session,
            control,
            tags: Tags::default(),
            view: View::default(),
            input: Input::default(),
            deferred: Vec::new(),
            shots: 0,
            quit: None,
            frames: 0,
            ui_jobs: Vec::new(),
            clipboard: Vec::new(),
            themed: false,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Frames drawn so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// Runs command `id`: a UI-only command ([`crate::ui_commands`]) or the engine's.
    pub fn run(&mut self, id: &str, params: Json) -> Result<Outcome, EngineError> {
        match ui_commands::find(id) {
            Some(c) => ui_commands::run(self, c, params),
            None => self.session.execute(id, params),
        }
    }

    /// Every command, the engine's then the UI-only ones, with whether it can run now.
    pub fn commands(&self) -> Vec<CommandInfo> {
        let mut list = self.session.commands();
        let ui = ui_commands::infos(self, &list);
        list.extend(ui);
        list
    }

    /// Feeds the synthetic input of `ui.click`, `ui.key` and `ui.text` (one batch per frame) into
    /// the raw input of the frame about to run. eframe calls it through `raw_input_hook`; a test
    /// harness calls it before each frame.
    pub fn feed_input(&mut self, raw: &mut egui::RawInput) {
        self.input.feed(raw, self.frames);
    }

    /// Puts `text` on the clipboard at the end of this frame.
    pub(crate) fn copy_text(&mut self, text: String) {
        self.clipboard.push(text);
    }

    /// Shows `text` as a notice for a few seconds.
    pub(crate) fn notice(&mut self, text: String) {
        if self.view.notices.len() >= MAX_NOTICES {
            self.view.notices.pop_front();
        }
        self.view.notices.push_back((text, None));
    }

    /// Runs command `id` for a shortcut; `query.run` with `current` gets the editor's cursor.
    pub(crate) fn run_shortcut(&mut self, id: &'static str, mut params: Json) {
        if id == "query.run"
            && params.get("current") == Some(&Json::Bool(true))
            && let (Some(cursor), Some(p)) = (self.view.query_cursor, params.as_object_mut())
        {
            p.entry("cursor").or_insert(Json::from(cursor));
        }
        self.run_from_ui(id, params);
    }

    /// Runs command `id` because the user asked (a shortcut, a button): its error shows as a
    /// notice, a job's error too once it ends; a disabled command does nothing.
    pub(crate) fn run_from_ui(&mut self, id: &'static str, params: Json) {
        match self.run(id, params) {
            Ok(Outcome::Done(_)) => {}
            Ok(Outcome::Started(job)) => self.ui_jobs.push((job, id)),
            Err(e) => self.ui_error(id, &e),
        }
    }

    fn ui_error(&mut self, id: &str, e: &EngineError) {
        log::debug!("command {id} from the UI: {}", e.code());
        if !matches!(e, EngineError::Disabled { .. }) {
            self.notice(e.to_string());
        }
    }

    /// One frame (see the module docs). Usable from a test harness: call [`DbeeApp::feed_input`]
    /// before it when the test drives input through the control channel.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.themed {
            // Panels started this frame keep the old style: draw again with the theme.
            theme::apply(&ctx);
            self.themed = true;
            ctx.request_repaint();
        }
        // 1. Job results.
        let finished = self.session.pump();
        // 2. Waiting replies.
        let unclaimed = control::answer_deferred(self, &ctx, finished);
        for (job, result) in unclaimed {
            if let Some(i) = self.ui_jobs.iter().position(|(j, _)| *j == job) {
                let (_, id) = self.ui_jobs.swap_remove(i);
                if let Err(e) = result {
                    self.ui_error(id, &e);
                }
            }
        }
        // 3. Control requests.
        if let Some(rx) = self.control.take() {
            for _ in 0..REQUESTS_PER_FRAME {
                let Ok(req) = rx.try_recv() else { break };
                control::handle(self, &ctx, req);
            }
            self.control = Some(rx);
        }
        // 4. Shortcuts.
        shortcuts::handle(self, &ctx);
        // 5. Draw.
        self.tags.begin_frame();
        self.draw(ui);
        // 6. Dialogs and notices for the next frame.
        let requests = self.session.take_ui_requests();
        if !requests.is_empty() {
            ctx.request_repaint();
        }
        for r in requests {
            match r {
                dbee_engine::UiRequest::Dialog { name, data } => self.view.dialog = Some(Dialog { name, data }),
                dbee_engine::UiRequest::Notice(text) => self.notice(text),
            }
        }
        for text in self.clipboard.drain(..) {
            ctx.copy_text(text);
        }
        if self.session.busy() {
            ctx.request_repaint_after(BUSY_REPAINT);
        }
        if control::needs_frames(self) {
            ctx.request_repaint();
        }
        self.frames = self.frames.saturating_add(1);
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        match self.session.state().screen {
            Screen::Connections => self.connections(ui),
            Screen::Workspace => self.workspace(ui),
        }
        self.notices(ui);
    }

    /// The connection screen (a stub): the saved and recent lists on the left, empty for now, and
    /// a card in the middle.
    fn connections(&mut self, ui: &mut egui::Ui) {
        let t = TOKENS;
        egui::Panel::left("connections-side")
            .resizable(false)
            .exact_size(300.0)
            .frame(Frame::new().fill(t.panel).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                let new = icon_button(ui, "New connection", icons::plus);
                self.tags.tag("conn-new", &new);
                if new.clicked() {
                    self.view.new_connection = true;
                }
                if let Some(w) = &self.session.state().store_warning {
                    ui.add_space(8.0);
                    ui.label(RichText::new(w).color(t.danger).small());
                }
                ui.add_space(18.0);
                section(ui, "SAVED");
                ui.label(RichText::new("No saved connections yet.").color(t.text_muted));
                ui.add_space(18.0);
                section(ui, "RECENT");
                ui.label(RichText::new("Connections you open show here.").color(t.text_muted));
            });
        egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(24))).show(ui, |ui| {
            let (title, body) = if self.view.new_connection {
                ("New connection", "The connection form opens here.")
            } else {
                ("Connect to Postgres", "Start a new connection, or pick a saved one on the left.")
            };
            let height = ui.available_height();
            ui.add_space((height * 0.28).max(0.0));
            ui.vertical_centered(|ui| {
                Frame::new()
                    .fill(t.card)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(RADIUS.saturating_mul(2)))
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(460.0_f32.min(ui.available_width()));
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                                icons::database(ui.painter(), rect, t.accent);
                                ui.label(RichText::new(title).size(20.0).strong());
                            });
                            ui.add_space(6.0);
                            ui.label(RichText::new(body).color(t.text_muted));
                        });
                    });
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.label(RichText::new(format!("dbee {}", env!("CARGO_PKG_VERSION"))).small().color(t.text_muted));
            });
        });
    }

    /// The workspace (a placeholder): the connection and its tabs.
    fn workspace(&mut self, ui: &mut egui::Ui) {
        let t = TOKENS;
        let st = self.session.state();
        let name = st.connected.as_ref().map(|c| c.config.display_name()).unwrap_or_default();
        let server = st.connected.as_ref().map(|c| c.info.version.clone()).unwrap_or_default();
        let tabs: Vec<String> = st.tabs.iter().map(|tab| tab.title()).collect();
        egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(24))).show(ui, |ui| {
            ui.label(RichText::new(name).size(20.0).strong());
            ui.label(RichText::new(server).color(t.text_muted));
            ui.add_space(12.0);
            for title in tabs {
                ui.label(title);
            }
        });
    }

    /// The newest notice, bottom right, for [`NOTICE_SECS`].
    fn notices(&mut self, ui: &mut egui::Ui) {
        let now = ui.ctx().input(|i| i.time);
        while let Some((_, Some(shown))) = self.view.notices.front() {
            if now - shown < NOTICE_SECS {
                break;
            }
            self.view.notices.pop_front();
        }
        let Some((text, shown)) = self.view.notices.front_mut() else { return };
        let since = *shown.get_or_insert(now);
        let t = TOKENS;
        let area = egui::Area::new(egui::Id::new("dbee-notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .movable(false)
            .fade_in(false);
        let r = area
            .show(ui.ctx(), |ui| {
                Frame::new().fill(t.text).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(RichText::new(text.as_str()).color(t.card));
                })
            })
            .response;
        self.tags.tag("notice", &r);
        let left = Duration::try_from_secs_f64((NOTICE_SECS - (now - since)).clamp(0.05, NOTICE_SECS)).unwrap_or(Duration::from_millis(50));
        ui.ctx().request_repaint_after(left);
    }
}

/// A full-width button with a code-drawn icon left of its label.
fn icon_button(ui: &mut egui::Ui, label: &str, icon: fn(&egui::Painter, egui::Rect, Color32)) -> egui::Response {
    let t = TOKENS;
    let size = vec2(ui.available_width(), 36.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        t.selection
    } else if response.hovered() {
        t.hover
    } else {
        t.card
    };
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0));
    icon(p, icon_rect, t.accent);
    p.text(egui::pos2(rect.left() + 36.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(14.0), t.text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

/// A small, muted section heading.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).small().strong().color(TOKENS.text_muted));
    ui.add_space(2.0);
}

impl eframe::App for DbeeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        DbeeApp::ui(self, ui);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.feed_input(raw);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        TOKENS.bg.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;

    use dbee_config::MemoryStore;
    use dbee_engine::{Session, SessionOptions};
    use dbee_testkit::temp_home;
    use egui_kittest::Harness;
    use serde_json::json;

    use super::*;
    use crate::ControlRequest;

    #[test]
    fn ui_click_by_id_reaches_the_widget() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            ..SessionOptions::default()
        })
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), DbeeApp::new(session, Some(rx)));
        let frame = |h: &mut Harness<'_, DbeeApp>| {
            let mut raw = h.input().clone();
            h.state_mut().feed_input(&mut raw);
            *h.input_mut() = raw;
            h.step();
        };
        frame(&mut h);
        assert!(!h.state().view.new_connection);
        let (req, reply) = ControlRequest::new("ui.click", json!({ "id": "conn-new" }));
        tx.send(req).unwrap();
        let mut answer = None;
        for _ in 0..10 {
            frame(&mut h);
            if let Ok(r) = reply.try_recv() {
                answer = Some(r);
                break;
            }
        }
        let answer = answer.expect("ui.click replies once the click was delivered");
        assert_eq!(answer["ok"], true, "{answer}");
        assert!(answer["result"]["x"].as_f64().is_some(), "{answer}");
        assert!(h.state().view.new_connection, "the click reached the button");
        assert!(h.state().frames() >= 3);
    }

    #[test]
    fn quit_closes_the_window_after_the_reply() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut h = Harness::builder().build_ui_state(|ui, app: &mut DbeeApp| app.ui(ui), DbeeApp::new(session, Some(rx)));
        h.step();
        let closes = |h: &Harness<'_, DbeeApp>| {
            h.output().viewport_output.values().flat_map(|v| v.commands.iter()).any(|c| *c == egui::ViewportCommand::Close)
        };
        let (req, reply) = ControlRequest::new("app.quit", json!({}));
        tx.send(req).unwrap();
        h.step();
        assert_eq!(reply.try_recv().unwrap()["ok"], true);
        assert!(!closes(&h), "the window stays open in the frame that queued the reply");
        let start = std::time::Instant::now();
        let mut closed = false;
        while !closed && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.step();
            closed = closes(&h);
        }
        assert!(closed, "a later frame closes the window");
    }

    #[test]
    fn ui_commands_run_through_run() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui)).drop_without_applying_deltas();
        assert_eq!(app.frames(), 1);
        assert!(!app.session().busy());
        // A UI command runs through `run`, like the panels' clicks.
        assert_eq!(app.run("ui.toggleValuePanel", json!({})).unwrap(), Outcome::Done(json!({ "visible": false })));
        assert!(app.run("no.such", json!({})).is_err());
    }
}

//! The app shell: [`DbeeApp`] runs one window's frames over a [`Session`].
//!
//! Frame order ([`DbeeApp::ui`]):
//! 1. `session.pump()`: the results of finished jobs are applied;
//! 2. the control replies that waited (for a job, idleness, input or a picture) are answered;
//! 3. the control requests that arrived are handled ([`crate::control`]); then the file the
//!    command line named ([`DbeeApp::with_file`]), in the first frame, or else the first file
//!    dropped on the window opens (`connection.openFile`; a file of no type dbee opens shows the
//!    refusal as a notice). A drop while a dialog shows or a connect runs is let go, not kept;
//! 4. the open cell editor takes its keys and commits on a press outside it, or Enter over a
//!    selection opens one ([`crate::editors`]); then the keyboard shortcuts run
//!    ([`crate::shortcuts`]; the value panel's copy first), before any widget sees the keys (not
//!    while a dialog or the quick search is open: the quick search handles its own keys);
//! 5. a connection chosen in the quick search connects once the one open has closed
//!    ([`connections::switch_pending`]); the screen is drawn, tagging its widgets
//!    ([`crate::automation`]), then the quick search over it while it is open
//!    ([`crate::quick_search`]), then the dialog, if any ([`crate::dialogs`]); then the cell
//!    editor's key acts, on the text it has now;
//! 6. the session's UI requests (dialogs, notices) are taken, to show from the next frame.
//!
//! While a job runs the window repaints every 100 ms, so waits time out and progress shows even
//! without input. The UI is thin: screens read `session.state()` and act through
//! [`DbeeApp::run`]. A command the user started (a shortcut, a button, a dialog) goes through
//! [`DbeeApp::start`], which gives a new query tab the tree's selected schema and hands its
//! outcome back to where it came from ([`Origin`]), at once
//! or when its job ends: a `needs…` error opens a dialog that runs it again with the answer; other
//! errors show as a notice (a command that cannot run now gives its reason alone, as a sentence:
//! [`notice_text`]), or under the connection form's buttons; the database error of a page load's
//! job or an Apply's shows only in the table tab's card, and a query run's error that the engine
//! recorded on its tab only in the query tab's card ([`crate::query_tab`]). Logs name command ids
//! and error codes only.
//!
//! The connection screen is [`crate::connections`]; the workspace (menu bar, tree, tabs, status
//! bar, quick search) is [`crate::workspace`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use dbee_engine::{CommandInfo, EngineError, JobId, Json, Outcome, Screen, Session};
use egui::{CornerRadius, Frame, Margin, RichText, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::connections::{self, ConnScreen};
use crate::control::{self, ControlRequest, Deferred, Input};
use crate::dialogs::{self, Dialog, Rerun};
use crate::quick_search::{self, QuickSearch};
use crate::theme::{RADIUS, TOKENS};
use crate::workspace::Workspace;
use crate::{editors, shortcuts, theme, ui_commands, workspace};

/// The Apply command, whose job errors the table tab's card shows.
const APPLY: &str = "edit.apply";

/// The command that opens a query tab.
const NEW_QUERY: &str = "tab.newQuery";

/// The command that opens a database file: one dropped on the window, or named on the command line.
const OPEN_FILE: &str = "connection.openFile";

/// The jobs that connect: while one runs, a dropped file is let go.
const CONNECTING: [&str; 2] = ["connection.open", "connection.reconnect"];

/// How often a busy session repaints.
const BUSY_REPAINT: Duration = Duration::from_millis(100);

/// How long a notice shows, in seconds.
const NOTICE_SECS: f64 = 4.0;

/// Most control requests handled in one frame; the rest wait for the next.
const REQUESTS_PER_FRAME: usize = 64;

/// What a notice says for `e`. A command that cannot run now (`Disabled`), is busy or got bad
/// params gives its reason alone, as a sentence: `No staged changes.`, not "`edit.apply` cannot
/// run now: no staged changes". Every other error reads as its own text. [`EngineError`]'s
/// `Display`, which names the command, stays for the control channel and the logs.
pub(crate) fn notice_text(e: &EngineError) -> String {
    match e {
        EngineError::Disabled { why: text, .. } | EngineError::BadParams { msg: text, .. } | EngineError::Busy(text) => sentence(text),
        other => other.to_string(),
    }
}

/// `text` trimmed, its first letter upper-cased, ending in a full stop (unless it ends in one
/// already, or in `?`, `!` or `…`).
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return String::new() };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '?', '!', '…']) {
        out.push('.');
    }
    out
}

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
    /// Jobs started by the user's own actions ([`DbeeApp::start`]), whose outcome goes back to
    /// where they came from.
    ui_jobs: Vec<UiJob>,
    /// Texts the app copied this frame, for egui at its end.
    clipboard: Vec<String>,
    /// With [`Clipboard::Memory`], the last text copied (`Some(None)` before any copy).
    memory: Option<Option<String>>,
    themed: bool,
    /// Keyboard events held back for the next frame: those after a cell editor's key in the same
    /// frame ([`crate::editors::before_frame`]).
    pub(crate) carry: Vec<egui::Event>,
    /// The file to open in the first frame ([`DbeeApp::with_file`]).
    file: Option<PathBuf>,
}

/// Where copied text goes ([`DbeeApp::with_clipboard`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clipboard {
    /// The system clipboard, through egui (`Context::copy_text`) at the end of the frame.
    #[default]
    System,
    /// The app only (`DBEE_CLIPBOARD=memory`, which `cargo xtask run-isolated` sets): at the end of
    /// each frame every copy and cut egui was asked for, the grid's and the text fields' alike, is
    /// taken out of its output and the last one kept, so none reaches the system clipboard.
    /// `document.inspect` shows the start of the last as `clipboard`, and its length.
    Memory,
}

/// Where a command the user started came from, so its outcome goes back there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A shortcut or a menu: errors show as notices.
    Menu,
    /// A table tab's page load (filters, paging, sort, refresh): as [`Origin::Menu`], except that
    /// a database error of the load's job shows only in the tab's own card (`TableTab::error`).
    /// An error before any job started (a lost connection found at once) still shows as a notice.
    Page,
    /// The connection form while it showed this generation ([`ConnScreen::generation`]):
    /// results and errors show under its buttons.
    Form(u64),
    /// The connection form's SSH login line (`connection.sshKey`).
    KeyLine,
}

/// A job [`DbeeApp::start`] started, with the params it ran with (for running it again after a
/// dialog). The params can hold secrets: `Debug` leaves them out.
struct UiJob {
    job: JobId,
    command: &'static str,
    params: Json,
    origin: Origin,
}

impl std::fmt::Debug for UiJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiJob").field("job", &self.job).field("command", &self.command).field("origin", &self.origin).finish_non_exhaustive()
    }
}

/// What only the window keeps.
#[derive(Debug)]
pub(crate) struct View {
    /// The value panel shows ([`crate::value_panel`]).
    pub value_panel: bool,
    /// The value panel's value, its rows and the selection in it.
    pub panel: crate::value_panel::PanelState,
    /// The workspace's tree, tabs and editors.
    pub work: Workspace,
    /// The quick search ([`crate::quick_search`]).
    pub quick: QuickSearch,
    /// The widget to give the keyboard back to in the next frame, once the quick search has
    /// closed, while the window still shows what the search opened over.
    pub quick_refocus: Option<(egui::Id, (Screen, Option<String>))>,
    /// The dialog on screen ([`crate::dialogs`]).
    pub dialog: Option<Dialog>,
    /// Notices with the time they were first shown (`None` until then).
    pub notices: VecDeque<(String, Option<f64>)>,
    /// The connection screen's form, lists and jobs.
    pub conn: ConnScreen,
    /// The query editor's cursor, a byte offset into the active query tab's text, for
    /// Ctrl+Shift+Enter (run the statement under the cursor).
    pub query_cursor: Option<usize>,
    /// The query editor's share of a query tab's height: the split between it and the results,
    /// the same for every query tab, for the window's run ([`crate::query_tab`]).
    pub query_split: f32,
}

impl View {
    /// The window's own state; `has_store` says whether the session keeps secrets.
    fn new(has_store: bool) -> View {
        View {
            value_panel: true,
            panel: crate::value_panel::PanelState::default(),
            work: Workspace::default(),
            quick: QuickSearch::default(),
            quick_refocus: None,
            dialog: None,
            notices: VecDeque::new(),
            conn: ConnScreen::new(has_store),
            query_cursor: None,
            query_split: crate::query_tab::DEFAULT_SPLIT,
        }
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
        let has_store = !session.inspect()["secretStore"].is_null();
        DbeeApp {
            session,
            control,
            tags: Tags::default(),
            view: View::new(has_store),
            input: Input::default(),
            deferred: Vec::new(),
            shots: 0,
            quit: None,
            frames: 0,
            ui_jobs: Vec::new(),
            clipboard: Vec::new(),
            memory: None,
            themed: false,
            carry: Vec::new(),
            file: None,
        }
    }

    /// This app opening the database file at `path` once the window is up
    /// (`connection.openFile`): the file the command line names.
    pub fn with_file(mut self, path: PathBuf) -> DbeeApp {
        self.file = Some(path);
        self
    }

    /// This app with copied text going to `clipboard` (the system clipboard by default).
    pub fn with_clipboard(mut self, clipboard: Clipboard) -> DbeeApp {
        self.memory = (clipboard == Clipboard::Memory).then_some(None);
        self
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
    /// the raw input of the frame about to run, after the keyboard events held back from the last
    /// frame (they came first). eframe calls it through `raw_input_hook`; a test harness calls it
    /// before each frame.
    pub fn feed_input(&mut self, raw: &mut egui::RawInput) {
        if !self.carry.is_empty() {
            raw.events.splice(0..0, self.carry.drain(..));
        }
        self.input.feed(raw, self.frames);
    }

    /// Puts `text` on the clipboard at the end of this frame (with [`Clipboard::Memory`], in the
    /// app's own).
    pub(crate) fn copy_text(&mut self, text: String) {
        self.clipboard.push(text);
    }

    /// With [`Clipboard::Memory`], the last text copied (`Some(None)` before any copy); `None`
    /// with the system clipboard.
    pub(crate) fn kept_clipboard(&self) -> Option<Option<&str>> {
        self.memory.as_ref().map(Option::as_deref)
    }

    /// The end of the frame: the app's copies go to egui, and with [`Clipboard::Memory`] every
    /// copy egui was asked for this frame (a text field's too) is taken back out, the last kept.
    fn hand_over_copies(&mut self, ctx: &egui::Context) {
        for text in self.clipboard.drain(..) {
            ctx.copy_text(text);
        }
        let Some(kept) = &mut self.memory else { return };
        let mut last = None;
        ctx.output_mut(|o| {
            o.commands.retain_mut(|c| match c {
                egui::OutputCommand::CopyText(text) => {
                    last = Some(std::mem::take(text));
                    false
                }
                egui::OutputCommand::CopyImage(_) => false,
                egui::OutputCommand::OpenUrl(_) => true,
            });
        });
        if let Some(text) = last {
            *kept = Some(text);
        }
    }

    /// Shows `text` as a notice for a few seconds.
    pub(crate) fn notice(&mut self, text: String) {
        // The same text waiting or showing already (a button clicked twice): once is enough.
        if self.view.notices.iter().any(|(t, _)| *t == text) {
            return;
        }
        if self.view.notices.len() >= MAX_NOTICES {
            self.view.notices.pop_front();
        }
        self.view.notices.push_back((text, None));
    }

    /// [`DbeeApp::notice`], except that the same text waiting or showing already starts its time
    /// again: a second copy of as many rows says so again.
    pub(crate) fn notice_again(&mut self, text: String) {
        match self.view.notices.iter_mut().find(|(t, _)| *t == text) {
            Some((_, shown)) => *shown = None,
            None => self.notice(text),
        }
    }

    /// Opens the database file at `path` as the user asked: dropped on the window, or named on the
    /// command line. A refusal (a file of no type dbee opens, one that is not there) is a notice.
    fn open_file(&mut self, path: &Path) {
        self.start(OPEN_FILE, json!({ "path": path.to_string_lossy() }), Origin::Menu);
    }

    /// Runs command `id` for a shortcut; `query.run` with `current` gets the editor's cursor,
    /// within the active query tab's text.
    pub(crate) fn run_shortcut(&mut self, id: &'static str, mut params: Json) {
        if id == "query.run"
            && params.get("current") == Some(&Json::Bool(true))
            && let Some(cursor) = self.view.query_cursor
            && let Some(dbee_engine::Tab::Query(q)) = self.session.state().active_tab()
            && let Some(p) = params.as_object_mut()
        {
            let mut at = cursor.min(q.text.len());
            while !q.text.is_char_boundary(at) {
                at = at.saturating_sub(1);
            }
            p.entry("cursor").or_insert(Json::from(at));
        }
        self.run_from_ui(id, params);
    }

    /// `params` of a command the user started, with what the tree has selected where the command
    /// takes it: a new query tab (from a shortcut, the menu or `+` alike) opens on the selected
    /// schema, or the selected table's, while the catalog holds it, unless `params` name one. The
    /// engine uses it where the dialect has `USE`.
    fn with_selection(&self, command: &str, mut params: Json) -> Json {
        if command == NEW_QUERY
            && let Some(c) = self.session.state().connected.as_ref()
            && let Some(schema) = self.view.work.tree.selected_schema(&c.catalog)
            && let Some(p) = params.as_object_mut()
        {
            p.entry("schema").or_insert_with(|| Json::from(schema));
        }
        params
    }

    /// Runs command `id` because the user asked (a shortcut, a menu): a `needs…` error opens a
    /// dialog, other errors show as a notice, a job's too once it ends; a command that cannot run
    /// now says why in a notice (buttons that show the reason themselves are disabled instead).
    pub(crate) fn run_from_ui(&mut self, id: &'static str, params: Json) {
        self.start(id, params, Origin::Menu);
    }

    /// Runs `command` because the user asked, and hands its outcome to `origin`: now, or when
    /// its job ends (then the job is returned).
    pub(crate) fn start(&mut self, command: &'static str, params: Json, origin: Origin) -> Option<JobId> {
        let params = self.with_selection(command, params);
        // The params are kept to run the command again after a dialog.
        match self.run(command, params.clone()) {
            Ok(Outcome::Started(job)) => {
                self.ui_jobs.push(UiJob { job, command, params, origin });
                if let Origin::Form(_) = origin {
                    connections::started(self, job, command);
                }
                Some(job)
            }
            Ok(Outcome::Done(v)) => {
                self.outcome(command, params, origin, None, Ok(v));
                None
            }
            Err(e) => {
                self.outcome(command, params, origin, None, Err(e));
                None
            }
        }
    }

    /// The outcome of a command [`DbeeApp::start`] ran (`job` when it ran as one).
    fn outcome(&mut self, command: &'static str, params: Json, origin: Origin, job: Option<JobId>, result: Result<Json, EngineError>) {
        if let Err(e) = &result {
            log::debug!("command {command} from the UI: {}", e.code());
        }
        match origin {
            Origin::KeyLine => connections::key_line_done(self, job, result),
            Origin::Form(generation) => connections::form_done(self, command, params, generation, job, result),
            Origin::Menu | Origin::Page => {
                // The card above the grid shows a page load's job error (`TableTab::error`), and
                // an Apply's however it was started (`Edits::failed`).
                let carded = origin == Origin::Page || command == APPLY;
                let in_card =
                    (carded && job.is_some() && matches!(result, Err(EngineError::Db { .. }))) || crate::query_tab::in_card(command, &result);
                if let Err(e) = result
                    && !dialogs::ask(self, &e, Rerun { command, params, origin })
                    && !in_card
                {
                    self.notice(notice_text(&e));
                }
            }
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
            if let Some(i) = self.ui_jobs.iter().position(|j| j.job == job) {
                let UiJob { job, command, params, origin } = self.ui_jobs.swap_remove(i);
                self.outcome(command, params, origin, Some(job), result);
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
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        let busy = self.view.dialog.is_some() || CONNECTING.iter().any(|c| self.session.running(c));
        if let Some(path) = self.file.take().or(dropped.filter(|_| !busy)) {
            self.open_file(&path);
        }
        // 4. The cell editor, then shortcuts, unless a dialog or the quick search is open.
        editors::before_frame(self, &ctx);
        if self.view.dialog.is_none() && !self.view.quick.open {
            shortcuts::handle(self, &ctx);
        }
        // 5. Draw, then the cell editor's key.
        self.tags.begin_frame();
        self.draw(ui);
        editors::after_draw(self, &ctx);
        // 6. Dialogs and notices for the next frame.
        let requests = self.session.take_ui_requests();
        if !requests.is_empty() {
            ctx.request_repaint();
        }
        for r in requests {
            match r {
                dbee_engine::UiRequest::Dialog { name, .. } => self.view.dialog = Some(Dialog::Requested { name }),
                dbee_engine::UiRequest::Notice(text) => self.notice(text),
            }
        }
        self.hand_over_copies(&ctx);
        if self.session.busy() {
            ctx.request_repaint_after(BUSY_REPAINT);
        }
        if control::needs_frames(self) || !self.carry.is_empty() {
            ctx.request_repaint();
        }
        self.frames = self.frames.saturating_add(1);
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        connections::switch_pending(self);
        match self.session.state().screen {
            Screen::Connections => {
                self.view.work.leave();
                self.view.panel.clear();
                connections::screen(self, ui);
            }
            Screen::Workspace => {
                self.view.conn.forget_list();
                workspace::screen(self, ui);
            }
        }
        quick_search::show(self, ui.ctx());
        dialogs::show(self, ui.ctx());
        self.notices(ui);
    }

    /// The newest notice, bottom right (above the workspace's status bar and a table tab's paging
    /// bar), for [`NOTICE_SECS`].
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
        // Above the status bar, and above a table tab's paging bar (its Apply and Reset).
        let lift = match (self.session.state().screen, self.session.state().active_tab()) {
            (Screen::Workspace, Some(dbee_engine::Tab::Table(_))) => workspace::STATUS_H + crate::paging::BAR_H,
            (Screen::Workspace, _) => workspace::STATUS_H,
            _ => 0.0,
        };
        let area = egui::Area::new(egui::Id::new("dbee-notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0 - lift))
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
        self.tags.tag_value("notice", &r, text.as_str());
        let left = Duration::try_from_secs_f64((NOTICE_SECS - (now - since)).clamp(0.05, NOTICE_SECS)).unwrap_or(Duration::from_millis(50));
        ui.ctx().request_repaint_after(left);
    }
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
        h.state_mut().view.conn.form.name = "typed".into();
        let generation = h.state().view.conn.generation;
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
        let conn = &h.state().view.conn;
        assert!(conn.form.name.is_empty() && conn.generation != generation, "the click reached the button");
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
    fn a_disabled_action_says_why_and_only_job_errors_of_a_page_load_stay_in_its_card() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let shown = |app: &DbeeApp| app.view.notices.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>();
        // A user's click on something that cannot run now says why.
        let disabled = EngineError::Disabled { id: "table.sort".into(), why: "apply or reset your changes first".into() };
        app.outcome("table.sort", json!({}), Origin::Page, None, Err(disabled));
        assert_eq!(shown(&app).last().map(String::as_str), Some("Apply or reset your changes first."), "the reason alone, as a sentence");
        // A page load's job error is the card's; the same error before any job started is not.
        let lost = || EngineError::Db { code: "connectionLost", message: "connection lost".into(), data: json!({}) };
        let before = shown(&app).len();
        app.outcome("table.page", json!({}), Origin::Page, Some(JobId(7)), Err(lost()));
        assert_eq!(shown(&app).len(), before, "the card shows it");
        app.outcome("table.page", json!({}), Origin::Page, None, Err(lost()));
        assert_eq!(shown(&app).last().map(String::as_str), Some("connection lost"));
        // From a menu or a shortcut, job errors show as before.
        let failed = EngineError::Db { code: "server", message: "the count failed".into(), data: json!({}) };
        app.outcome("table.count", json!({}), Origin::Menu, Some(JobId(8)), Err(failed));
        assert_eq!(shown(&app).last().map(String::as_str), Some("the count failed"));
        // The same text twice (a button clicked again) shows once.
        let n = shown(&app).len();
        app.notice("the count failed".into());
        assert_eq!(shown(&app).len(), n);
    }

    #[test]
    fn the_same_notice_again_starts_its_time_again() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        app.notice_again("Copied 2 rows".into());
        app.view.notices[0].1 = Some(10.0);
        app.notice("Copied 2 rows".into());
        assert_eq!(app.view.notices, [("Copied 2 rows".to_string(), Some(10.0))], "notice: once is enough");
        app.notice_again("Copied 2 rows".into());
        assert_eq!(app.view.notices, [("Copied 2 rows".to_string(), None)], "shown afresh, still once");
        app.notice_again("Copied 1 row".into());
        assert_eq!(app.view.notices.len(), 2);
    }

    #[test]
    fn a_query_runs_errors_on_its_tab_show_in_the_card_only() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let shown = |app: &DbeeApp| app.view.notices.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>();
        let db = |code, message: &str, data| EngineError::Db { code, message: message.into(), data };
        // The engine records these on the tab (their data names the statement): the card shows them.
        let server = db("server", "column \"nope\" does not exist", json!({ "serverCode": "42703", "position": 7, "statement": 0 }));
        app.outcome("query.run", json!({}), Origin::Menu, Some(JobId(3)), Err(server));
        let cancelled = db("cancelled", "the statement was cancelled", json!({ "position": null, "statement": 1 }));
        app.outcome("query.run", json!({}), Origin::Menu, Some(JobId(4)), Err(cancelled));
        let gone = db("connectionLost", "the query connection was lost", json!({ "position": null, "statement": 0 }));
        app.outcome("query.run", json!({}), Origin::Menu, None, Err(gone));
        assert_eq!(shown(&app), Vec::<String>::new());
        // Not on the tab: a lost browse connection, nothing to run.
        app.outcome(
            "query.run",
            json!({}),
            Origin::Menu,
            None,
            Err(db("connectionLost", "connection lost", json!({ "action": "connection.reconnect" }))),
        );
        let nothing = EngineError::Disabled { id: "query.run".into(), why: "there is no statement to run".into() };
        app.outcome("query.run", json!({}), Origin::Menu, None, Err(nothing));
        let notices = shown(&app);
        assert_eq!(notices, ["connection lost", "There is no statement to run."]);
    }

    #[test]
    fn a_notice_gives_the_reason_alone_as_a_sentence() {
        let disabled = |why: &str| EngineError::Disabled { id: "edit.apply".into(), why: why.into() };
        assert_eq!(notice_text(&disabled("no staged changes")), "No staged changes.");
        assert_eq!(notice_text(&disabled("connection lost — reconnect")), "Connection lost — reconnect.");
        assert_eq!(notice_text(&disabled("This row is marked for deletion.")), "This row is marked for deletion.", "one period");
        assert_eq!(notice_text(&disabled("éclair? ")), "Éclair?");
        assert_eq!(notice_text(&disabled("")), "");
        assert_eq!(notice_text(&EngineError::Busy("a query is running in Query 1".into())), "A query is running in Query 1.");
        let bad = EngineError::BadParams { id: "cell.edit".into(), msg: "`value` must be text or null".into() };
        assert_eq!(notice_text(&bad), "`value` must be text or null.");
        // Every other error reads as its own text; Display keeps the command for the control channel.
        let db = EngineError::Db { code: "server", message: "duplicate key (SQLSTATE 23505)".into(), data: json!({}) };
        assert_eq!(notice_text(&db), "duplicate key (SQLSTATE 23505)");
        assert_eq!(notice_text(&EngineError::Timeout), EngineError::Timeout.to_string());
        assert_eq!(disabled("no staged changes").to_string(), "`edit.apply` cannot run now: no staged changes");
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

//! The app shell: [`DbeeApp`] runs one window's frames over a [`Session`].
//!
//! The first frame only applies the theme and the fonts, and draws nothing: egui loads fonts at
//! the start of the next frame, and the screens name them. Before every frame
//! ([`DbeeApp::feed_input`]) the mode chosen is set and the synthetic input fed in. Every later
//! frame runs in this order ([`DbeeApp::ui`]):
//! 1. `session.pump()`: the results of finished jobs are applied;
//! 2. the control replies that waited (for a job, idleness, input or a picture) are answered;
//! 3. the control requests that arrived are handled ([`crate::control`]); then the window's
//!    close request, which starts a quit when something would be lost or left unfinished
//!    ([`DbeeApp::close_requested`]); then the file the
//!    command line named ([`DbeeApp::with_file`]), in the first frame that draws, or else the
//!    first file dropped on the window opens (`connection.openFile`; a file of no type dbee opens shows the
//!    refusal as a notice; while a connection is open, it closes first, through its questions,
//!    as a quick search switch does). A drop while a dialog shows or a connect runs is let go, not kept;
//! 4. the open cell editor takes its keys and commits on a press outside it, or Enter over a
//!    selection opens one ([`crate::editors`]); then the keyboard shortcuts run
//!    ([`crate::shortcuts`]; the value panel's copy first), before any widget sees the keys (not
//!    while a dialog or the quick search is open: the quick search handles its own keys);
//! 5. a quit closes the next connection, or the window ([`DbeeApp::quit_pending`]); a connection
//!    chosen in the quick search connects once the one open has closed
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
//! or when its job ends: a `needs…` error opens a dialog that runs it again with the answer; a
//! close that the transaction prompt ran again with `end` says how it went in a toast
//! ([`dialogs::ended`]); other errors show as a notice (a command that cannot run now gives its
//! reason alone, as a sentence: [`notice_text`]), or under the connection form's buttons; the
//! database error of a page load's job or an Apply's shows only in the table tab's card, and a
//! query run's error that the engine recorded on its tab only in the query tab's card
//! ([`crate::query_tab`]). Logs name command ids and error codes only.
//!
//! The connection screen is [`crate::connections`]; the workspace (menu bar, tree, tabs, status
//! bar, quick search) is [`crate::workspace`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use dbee_engine::cmd::connection::{OPEN, OPEN_FILE, RECONNECT};
use dbee_engine::{CommandInfo, ConnectionId, EngineError, JobId, Json, Outcome, Screen, Session, Which};
use serde_json::json;

use crate::automation::Tags;
use crate::components::toast::{self, Act, Kind, Toast};
use crate::connections::{self, ConnScreen};
use crate::control::{self, ControlRequest, Deferred, Input};
use crate::dialogs::{self, Dialog, Ending, Rerun};
use crate::quick_search::{self, QuickSearch};
use crate::quiet::Watch;
use crate::theme::{self, Words};
use crate::workspace::Workspace;
use crate::{editors, shortcuts, ui_commands, workspace};

/// The Apply command, whose job errors the table tab's card shows.
const APPLY: &str = "edit.apply";

/// The command that opens a query tab.
const NEW_QUERY: &str = "tab.newQuery";

/// The jobs that connect: while one runs, a dropped file is let go.
const CONNECTING: [&str; 2] = [OPEN, RECONNECT];

/// How often a busy session repaints.
const BUSY_REPAINT: Duration = Duration::from_millis(100);

/// How long a notice shows, in seconds.
const NOTICE_SECS: f64 = 4.0;
/// Most toasts on screen at once; another notice waits until one goes.
const MAX_TOASTS: usize = 3;
/// The tags of the toasts on screen and of their closes, the oldest first.
const TOAST_TAGS: [(&str, &str); MAX_TOASTS] = [("notice", "notice-close"), ("notice-2", "notice-close-2"), ("notice-3", "notice-close-3")];

/// Most control requests handled in one frame; the rest wait for the next.
const REQUESTS_PER_FRAME: usize = 64;

/// How long a quit waits, once it has closed every connection, for the work those closes stopped
/// to end, so the servers get the cancel requests before the window goes.
const QUIT_WAIT: Duration = Duration::from_secs(3);

/// What a notice says for `e`, as a sentence. A command that cannot run now (`Disabled`), is busy
/// or got bad params gives its reason alone: `No staged changes.`, not "`edit.apply` cannot run
/// now: no staged changes". Every other error reads as its own text. [`EngineError`]'s `Display`,
/// which names the command, stays for the control channel and the logs.
pub(crate) fn notice_text(e: &EngineError) -> String {
    sentence(&reason(e))
}

/// Why `e` failed, in the words a notice uses ([`notice_text`]), not yet made a sentence.
pub(crate) fn reason(e: &EngineError) -> String {
    match e {
        EngineError::Disabled { why: text, .. } | EngineError::BadParams { msg: text, .. } | EngineError::Busy(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `text` trimmed, its first letter upper-cased, ending in a full stop (unless it ends in one
/// already, or in `?`, `!` or `…`).
pub(crate) fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return String::new() };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '?', '!', '…']) {
        out.push('.');
    }
    out
}

/// What the notice says when a theme choice couldn't be saved: the file, and what to do.
fn not_saved(e: &dbee_config::SaveError) -> Words {
    match e {
        dbee_config::SaveError::Newer(file) => {
            Words::from("The theme is set for now, but a newer dbee wrote ").name(file.display().to_string()).plain(", so it isn't changed.")
        }
        dbee_config::SaveError::Io(file, why) => {
            // The system's words, without their number: "Permission denied (os error 13)".
            let why = why.to_string();
            let why = why.split(" (os error").next().unwrap_or_default().trim_end_matches('.');
            let folder = file.parent().unwrap_or(file);
            Words::from("The theme is set for now, but it couldn't be saved in ")
                .name(file.display().to_string())
                .plain(format!(" ({why}): check that you can write to "))
                .name(folder.display().to_string())
                .plain(".")
        }
    }
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
    /// The one folder `ui.screenshot` writes in ([`DbeeApp::with_screenshot_dir`]).
    pub(crate) screenshot_dir: Option<PathBuf>,
    /// `app.quit` was answered when this many frames were drawn: close the window at a later
    /// frame, from this instant on.
    pub(crate) quit: Option<(u64, std::time::Instant)>,
    /// The window's close request is being answered ([`DbeeApp::quit_pending`]).
    quitting: Option<Quitting>,
    /// The window closes itself ([`DbeeApp::close_window`]): the close request that brings goes
    /// through.
    closing: bool,
    frames: u64,
    /// Jobs started by the user's own actions ([`DbeeApp::start`]), whose outcome goes back to
    /// where they came from.
    ui_jobs: Vec<UiJob>,
    /// Texts the app copied this frame, for egui at its end.
    clipboard: Vec<String>,
    /// With [`Clipboard::Memory`], the last text copied (`Some(None)` before any copy).
    memory: Option<Option<String>>,
    themed: bool,
    /// The theme choice and the folder that keeps it ([`DbeeApp::with_settings_dir`]).
    pub(crate) theme_choice: dbee_config::ThemeChoice,
    settings_dir: Option<PathBuf>,
    /// What the desktop said at startup ([`DbeeApp::with_desktop_mode`]), and, once the first frame
    /// has settled it, the mode `System` means.
    desktop_mode: Option<egui::Theme>,
    pub(crate) system_mode: Option<egui::Theme>,
    /// How the window reaches the screen ([`DbeeApp::with_window_backend`]), for the log.
    window_backend: &'static str,
    /// The scale factor and zoom last written to the log ([`DbeeApp::window_note`]).
    window_noted: Option<(Option<u32>, u32)>,
    /// Keyboard events held back for the next frame: those after a cell editor's key in the same
    /// frame ([`crate::editors::before_frame`]).
    pub(crate) carry: Vec<egui::Event>,
    /// The file to open in the first frame that draws ([`DbeeApp::with_file`]).
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
    /// A quit's close of a connection ([`DbeeApp::quit_pending`]): as [`Origin::Menu`], with its
    /// questions worded for quitting.
    Quit,
}

/// A quit under way: the connections it still closes, in the order they opened, and the one it
/// asked about last; once none is left, until when it waits for the work the closes stopped.
#[derive(Debug)]
struct Quitting {
    left: VecDeque<ConnectionId>,
    asked: Option<ConnectionId>,
    until: Option<Instant>,
}

/// A job [`DbeeApp::start`] started, with the params it ran with (for running it again after a
/// dialog), and what it ends when it is a close that ends a transaction. The params can hold
/// secrets: `Debug` leaves them out.
struct UiJob {
    job: JobId,
    command: &'static str,
    params: Json,
    origin: Origin,
    ending: Option<Ending>,
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
    /// The value panel wraps its text (WRAP, `ui.toggleValueWrap`); off, it scrolls sideways.
    pub value_wrap: bool,
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
    /// Notices, oldest first; the first [`MAX_TOASTS`] show.
    pub notices: VecDeque<Notice>,
    /// The connection screen's form, lists and jobs.
    pub conn: ConnScreen,
    /// When the window first saw each job it shows waiting for, for both screens: nothing shows a
    /// job waits before it has run 300 ms ([`crate::quiet`]).
    pub quiet: crate::quiet::Quiet,
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
            value_wrap: true,
            panel: crate::value_panel::PanelState::default(),
            work: Workspace::default(),
            quick: QuickSearch::default(),
            quick_refocus: None,
            dialog: None,
            notices: VecDeque::new(),
            conn: ConnScreen::new(has_store),
            quiet: crate::quiet::Quiet::default(),
            query_cursor: None,
            query_split: crate::query_tab::DEFAULT_SPLIT,
        }
    }
}

/// Most notices kept; older ones are dropped.
const MAX_NOTICES: usize = 8;

/// A notice: what it says (its names marked), whether it reports a result or a failure, and when
/// it goes (on egui's clock; `None` until it first shows).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Notice {
    pub text: Words,
    pub kind: Kind,
    pub until: Option<f64>,
}

impl std::fmt::Debug for DbeeApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbeeApp").field("session", &self.session).field("frames", &self.frames).finish_non_exhaustive()
    }
}

impl DbeeApp {
    /// The app over `session`; `control` brings the control channel's requests
    /// ([`crate::control_server::listen`]).
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
            screenshot_dir: None,
            quit: None,
            quitting: None,
            closing: false,
            frames: 0,
            ui_jobs: Vec::new(),
            clipboard: Vec::new(),
            memory: None,
            themed: false,
            theme_choice: dbee_config::ThemeChoice::System,
            settings_dir: None,
            desktop_mode: None,
            system_mode: None,
            window_backend: "unknown",
            window_noted: None,
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

    /// This app with `ui.screenshot` writing its files in `dir`; without one, it only sends the
    /// picture back.
    pub fn with_screenshot_dir(mut self, dir: Option<PathBuf>) -> DbeeApp {
        self.screenshot_dir = dir;
        self
    }

    /// This app keeping the theme choice in `dir` (`settings.json`), and starting with the one
    /// kept there; without a folder it keeps nothing.
    pub fn with_settings_dir(mut self, dir: Option<PathBuf>) -> DbeeApp {
        self.theme_choice = dbee_config::load_theme(dir.as_deref());
        self.settings_dir = dir;
        self
    }

    /// This app taking `mode` as the desktop's, for `System`; without one, egui's own reading.
    pub fn with_desktop_mode(mut self, mode: Option<egui::Theme>) -> DbeeApp {
        self.desktop_mode = mode;
        self
    }

    /// This app naming `backend` in the log as the way its window reaches the screen (Wayland,
    /// X11, …).
    pub fn with_window_backend(mut self, backend: &'static str) -> DbeeApp {
        self.window_backend = backend;
        self
    }

    /// What the window got from the desktop, the first time and whenever it changes (another
    /// monitor, another scale): text is drawn for this many pixels per point, so a report of soft
    /// text starts from this line.
    fn window_note(&mut self, ctx: &egui::Context) -> Option<String> {
        let (native, zoom) = (ctx.native_pixels_per_point(), ctx.zoom_factor());
        let key = (native.map(f32::to_bits), zoom.to_bits());
        if self.window_noted == Some(key) {
            return None;
        }
        self.window_noted = Some(key);
        let scale = native.map_or_else(|| "unknown".to_owned(), |n| n.to_string());
        let points = ctx.content_rect().size();
        let ppp = ctx.pixels_per_point();
        Some(format!(
            "window: {}, scale factor {scale}, zoom {zoom}, {ppp} pixels per point, {} x {} points ({} x {} pixels)",
            self.window_backend,
            points.x,
            points.y,
            (points.x * ppp).round(),
            (points.y * ppp).round()
        ))
    }

    /// The mode the window draws in now.
    pub(crate) fn mode(&self) -> egui::Theme {
        theme::mode_for(self.theme_choice, self.system_mode)
    }

    /// Takes `choice` and keeps it; the next frame draws it. A choice that cannot be kept still
    /// applies, and a notice says so.
    pub(crate) fn choose_theme(&mut self, choice: dbee_config::ThemeChoice) {
        self.theme_choice = choice;
        if let Err(e) = dbee_config::save_theme(self.settings_dir.as_deref(), choice) {
            self.notice_again(Kind::Error, not_saved(&e));
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Marks the theme as set, for a test whose context has it already.
    #[cfg(test)]
    pub(crate) fn themed_for_test(&mut self) {
        self.themed = true;
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

    /// Before the frame about to run: sets the mode chosen since the last frame, so this frame draws
    /// wholly in it (egui makes a frame's widgets' style when the frame starts), then feeds the
    /// synthetic input of `ui.click`, `ui.key` and `ui.text` (one batch per frame) into its raw
    /// input, after the keyboard events held back from the last frame (they came first). eframe
    /// calls it through `raw_input_hook`; a test harness calls it before each frame.
    pub fn feed_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.themed && ctx.theme() != self.mode() {
            ctx.set_theme(self.mode());
        }
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

    /// Shows `text` as a notice of `kind` (a result or a failure) for a few seconds.
    pub(crate) fn notice(&mut self, kind: Kind, text: impl Into<Words>) {
        let text = text.into();
        // The same text waiting or showing already (a button clicked twice): once is enough.
        if self.view.notices.iter().any(|n| n.text == text) {
            return;
        }
        if self.view.notices.len() >= MAX_NOTICES {
            self.view.notices.pop_front();
        }
        self.view.notices.push_back(Notice { text, kind, until: None });
    }

    /// [`DbeeApp::notice`], except that the same text waiting or showing already starts its time
    /// again: a second copy of as many rows says so again.
    pub(crate) fn notice_again(&mut self, kind: Kind, text: impl Into<Words>) {
        let text = text.into();
        match self.view.notices.iter_mut().find(|n| n.text == text) {
            Some(n) => n.until = None,
            None => self.notice(kind, text),
        }
    }

    /// Opens the database file at `path` as the user asked: dropped on the window, or named on the
    /// command line. A refusal (a file of no type dbee opens, one that is not there) is a notice.
    pub(crate) fn open_file(&mut self, path: &Path) {
        self.start(OPEN_FILE, json!({ "path": path.to_string_lossy() }), Origin::Menu);
    }

    /// A database file dropped on the window: while a connection is open, through the questions
    /// of `connection.close` first, as a switch in the quick search is ([`connections::switch_to`]),
    /// unless the file would be refused anyway (that refusal is the notice, and nothing closes).
    fn dropped_file(&mut self, path: &Path) {
        if self.session.state().connection(Which::Active).is_none() {
            self.open_file(path);
            return;
        }
        match dbee_engine::cmd::connection::file_config(&self.session, path) {
            Ok(_) => connections::switch_to(self, connections::SwitchTo::File(path.to_path_buf())),
            Err(e) => self.notice(Kind::Error, notice_text(&e)),
        }
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
            && let Some(c) = self.session.state().connection(Which::Active)
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
        let ending = dialogs::ending(self, command, &params);
        // The params are kept to run the command again after a dialog.
        match self.run(command, params.clone()) {
            Ok(Outcome::Started(job)) => {
                self.ui_jobs.push(UiJob { job, command, params, origin, ending });
                if let Origin::Form(_) = origin {
                    connections::started(self, job, command);
                }
                Some(job)
            }
            Ok(Outcome::Done(v)) => {
                self.answer(command, params, origin, None, ending, Ok(v));
                None
            }
            Err(e) => {
                self.answer(command, params, origin, None, ending, Err(e));
                None
            }
        }
    }

    /// The outcome of a command [`DbeeApp::start`] ran: a toast for a close that ends a
    /// transaction ([`dialogs::ended`]); any other goes back where it came from.
    fn answer(
        &mut self,
        command: &'static str,
        params: Json,
        origin: Origin,
        job: Option<JobId>,
        ending: Option<Ending>,
        result: Result<Json, EngineError>,
    ) {
        match ending {
            Some(ending) => {
                if let Err(e) = &result {
                    log::debug!("command {command} from the UI: {}", e.code());
                }
                dialogs::ended(self, &ending, result);
            }
            None => self.outcome(command, params, origin, job, result),
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
            Origin::Menu | Origin::Page | Origin::Quit => {
                // The card above the grid shows a page load's job error (`TableTab::error`), and
                // an Apply's however it was started (`Edits::failed`).
                let carded = origin == Origin::Page || command == APPLY;
                let in_card =
                    (carded && job.is_some() && matches!(result, Err(EngineError::Db { .. }))) || crate::query_tab::in_card(command, &result);
                if let Err(e) = result
                    && !dialogs::ask(self, &e, Rerun { command, params, origin })
                    && !in_card
                {
                    self.notice(Kind::Error, notice_text(&e));
                }
            }
        }
    }

    /// Closes the window. The close request that brings goes through without asking.
    pub(crate) fn close_window(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The desktop asks the window to close (its close button, Alt+F4, logging out). With nothing
    /// to lose or leave unfinished ([`dialogs::quit_asks`]) it closes at once. Else the close waits
    /// and a quit starts ([`DbeeApp::quit_pending`]); a dialog that was showing goes unanswered,
    /// and a switch waiting for its close is dropped. Asked again during the quit, the close waits
    /// again.
    fn close_requested(&mut self, ctx: &egui::Context) {
        if self.closing || self.quit.is_some() || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting.is_none() && !dialogs::quit_asks(self.session.state()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        ctx.request_repaint();
        if self.quitting.is_none() {
            let left = self.session.state().connections.iter().map(|c| c.id).collect();
            self.quitting = Some(Quitting { left, asked: None, until: None });
            self.view.dialog = None;
            self.view.conn.switch = None;
        }
    }

    /// Each frame of a quit, once no dialog shows and no close is ending a transaction: the
    /// connection asked about last still open (CANCEL, an end that failed) stops the quit, and the
    /// window stays. Else the next connection closes with its questions ([`dialogs::quit_close`]).
    /// With none left the window closes, once the work the closes stopped has ended, or after
    /// [`QUIT_WAIT`].
    fn quit_pending(&mut self, ctx: &egui::Context) {
        let waiting = self.view.dialog.is_some() || self.session.running(connections::CLOSE) || self.session.running(dialogs::TAB_CLOSE);
        let state = self.session.state();
        let Some(quitting) = self.quitting.as_mut().filter(|_| !waiting) else { return };
        if quitting.asked.take().is_some_and(|id| state.connection(Which::Id(id)).is_some()) {
            self.quitting = None;
            return;
        }
        while let Some(id) = quitting.left.pop_front() {
            if state.connection(Which::Id(id)).is_some() {
                quitting.asked = Some(id);
                dialogs::quit_close(self, id);
                // A close with nothing to ask is done already: the next frame goes on.
                ctx.request_repaint();
                return;
            }
        }
        let now = Instant::now();
        let until = *quitting.until.get_or_insert(now.checked_add(QUIT_WAIT).unwrap_or(now));
        if self.session.busy() && now < until {
            return;
        }
        self.quitting = None;
        self.close_window(ctx);
    }

    /// One frame (see the module docs). Usable from a test harness: call [`DbeeApp::feed_input`]
    /// before it, as eframe does.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(note) = self.window_note(&ctx) {
            log::info!("{note}");
        }
        if !self.themed {
            // The fonts load at the start of the next frame, and the screens name them: draw
            // nothing until then.
            theme::apply(&ctx);
            self.system_mode = self.desktop_mode.or_else(|| ctx.system_theme());
            ctx.set_theme(self.mode());
            self.themed = true;
            ctx.request_repaint();
            return;
        }
        // 1. Job results.
        let finished = self.session.pump();
        // 2. Waiting replies.
        let unclaimed = control::answer_deferred(self, &ctx, finished);
        for (job, result) in unclaimed {
            if let Some(i) = self.ui_jobs.iter().position(|j| j.job == job) {
                let UiJob { job, command, params, origin, ending } = self.ui_jobs.swap_remove(i);
                self.answer(command, params, origin, Some(job), ending, result);
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
        self.close_requested(&ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        let busy = self.view.dialog.is_some() || CONNECTING.iter().any(|c| self.session.running(c));
        if let Some(path) = self.file.take() {
            self.open_file(&path);
        } else if let Some(path) = dropped.filter(|_| !busy) {
            self.dropped_file(&path);
        }
        // 4. The cell editor, then shortcuts, unless a dialog or the quick search is open.
        editors::before_frame(self, &ctx);
        if self.view.dialog.is_none() && !self.view.quick.open {
            shortcuts::handle(self, &ctx);
        }
        // 5. Forget the jobs that no longer run, so the next starts its own 300 ms; draw, then the
        // cell editor's key.
        self.forget_ended_jobs();
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
                // What the engine says this way went wrong or needs care (a secret not kept, a
                // file not written, a connection's warning).
                dbee_engine::UiRequest::Notice(text) => self.notice(Kind::Error, text),
            }
        }
        self.hand_over_copies(&ctx);
        if self.session.busy() {
            ctx.request_repaint_after(BUSY_REPAINT);
        }
        // A mode chosen in this frame is set before the next one (`feed_input`).
        if control::needs_frames(self) || !self.carry.is_empty() || ctx.theme() != self.mode() {
            ctx.request_repaint();
        }
        self.frames = self.frames.saturating_add(1);
    }

    /// [`crate::quiet::Quiet::keep`]: a tab's job lives while the tab has it as a marker
    /// (`loading`, `counting`, `applying`, `running`), the form's while the form runs it, a
    /// command's while the session runs it.
    fn forget_ended_jobs(&mut self) {
        let state = self.session.state();
        let form = self.view.conn.running.map(|(job, _)| job);
        self.view.quiet.keep(|watch| match *watch {
            Watch::Tab(id, job) => state.tabs.iter().find(|t| t.id() == id).is_some_and(|tab| workspace::jobs(tab).contains(&job)),
            Watch::Form(job) => form == Some(job),
            Watch::Command(_, job) => self.session.job_running(job),
        });
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        self.quit_pending(ui.ctx());
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

    /// The oldest [`MAX_TOASTS`] notices as toasts, bottom right (above the workspace's status bar
    /// and a table tab's paging bar), the oldest lowest, each for [`NOTICE_SECS`] from when it
    /// first showed or until its close is clicked. Tagged `notice`, `notice-2` and `notice-3` with
    /// their texts, their closes `notice-close`, `notice-close-2` and `notice-close-3`.
    fn notices(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // Only the notices shown have a clock; each goes when its own runs out.
        self.view.notices.retain(|n| n.until.is_none_or(|until| now < until));
        // Above the bars at the window's bottom: the status bar, and a table tab's paging bar (its
        // Apply and Reset); on the connection screen the version bar, and the form's bar (its TEST
        // and CONNECT) while the form shows.
        let lift = match (self.session.state().screen, self.session.state().active_tab()) {
            (Screen::Workspace, Some(dbee_engine::Tab::Table(_))) => workspace::STATUS_H + crate::paging::BAR_H,
            (Screen::Workspace, _) => workspace::STATUS_H,
            (Screen::Connections, _) if self.view.conn.step == connections::Step::Form => theme::size::SLIM_BAR_H + theme::size::BAR_H,
            (Screen::Connections, _) => theme::size::SLIM_BAR_H,
        };
        let mut soonest = f64::INFINITY;
        let mut toasts = Vec::with_capacity(MAX_TOASTS);
        for notice in self.view.notices.iter_mut().take(MAX_TOASTS) {
            soonest = soonest.min(*notice.until.get_or_insert(now + NOTICE_SECS));
            toasts.push(Toast::new(notice.kind, notice.text.clone()));
        }
        if toasts.is_empty() {
            return;
        }
        let shown = toast::show(&ctx, &toasts, lift);
        for (((tag, close_tag), notice), (r, close)) in
            TOAST_TAGS.iter().zip(self.view.notices.iter()).zip(shown.toasts.iter().zip(shown.closes.iter()))
        {
            self.tags.tag_value(tag, r, notice.text.to_string());
            self.tags.tag(close_tag, close);
        }
        if let Some((i, Act::Close)) = shown.act {
            self.view.notices.remove(i);
            ctx.request_repaint();
            return;
        }
        let left = Duration::try_from_secs_f64((soonest - now).clamp(0.05, NOTICE_SECS)).unwrap_or(Duration::from_millis(50));
        ctx.request_repaint_after(left);
    }
}

impl eframe::App for DbeeApp {
    /// While the window is hidden (minimized) eframe runs only this: a close asked for then asks
    /// first too.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.close_requested(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        DbeeApp::ui(self, ui);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.feed_input(ctx, raw);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
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

    /// The log says what the window got, once, and again when the scale changes.
    #[test]
    fn the_window_note_is_said_once_per_scale() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            ..SessionOptions::default()
        })
        .unwrap();
        let mut app = DbeeApp::new(session, None).with_window_backend("wayland");
        let ctx = egui::Context::default();
        let frame = |ctx: &egui::Context, scale: f32| {
            let mut raw =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
            ctx.run_ui(raw, |_| {}).textures_delta.clear();
        };
        frame(&ctx, 1.0);
        let first = app.window_note(&ctx);
        assert_eq!(first.as_deref(), Some("window: wayland, scale factor 1, zoom 1, 1 pixels per point, 800 x 600 points (800 x 600 pixels)"));
        frame(&ctx, 1.0);
        assert_eq!(app.window_note(&ctx), None, "nothing changed: said once");
        frame(&ctx, 1.25);
        let moved = app.window_note(&ctx);
        assert_eq!(moved.as_deref(), Some("window: wayland, scale factor 1.25, zoom 1, 1.25 pixels per point, 800 x 600 points (1000 x 750 pixels)"));
        assert_eq!(app.window_note(&ctx), None);
    }

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
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_ui_state(
            |ui, app: &mut DbeeApp| {
                if crate::theme::applied(ui.ctx()) {
                    app.ui(ui);
                }
            },
            DbeeApp::new(session, Some(rx)),
        );
        let frame = |h: &mut Harness<'_, DbeeApp>| {
            let mut raw = h.input().clone();
            let ctx = h.ctx.clone();
            h.state_mut().feed_input(&ctx, &mut raw);
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
        let mut h = Harness::builder().build_ui_state(
            |ui, app: &mut DbeeApp| {
                if crate::theme::applied(ui.ctx()) {
                    app.ui(ui);
                }
            },
            DbeeApp::new(session, Some(rx)),
        );
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
        let shown = |app: &DbeeApp| app.view.notices.iter().map(|n| n.text.to_string()).collect::<Vec<_>>();
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
        assert_eq!(shown(&app).last().map(String::as_str), Some("Connection lost."));
        // From a menu or a shortcut, job errors show as before.
        let failed = EngineError::Db { code: "server", message: "the count failed".into(), data: json!({}) };
        app.outcome("table.count", json!({}), Origin::Menu, Some(JobId(8)), Err(failed));
        assert_eq!(shown(&app).last().map(String::as_str), Some("The count failed."));
        assert!(app.view.notices.iter().all(|n| n.kind == Kind::Error), "failures: {:?}", app.view.notices);
        // The same text twice (a button clicked again) shows once.
        let n = shown(&app).len();
        app.notice(Kind::Error, "The count failed.");
        assert_eq!(shown(&app).len(), n);
    }

    #[test]
    fn the_same_notice_again_starts_its_time_again() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let copied = |until| Notice { text: "Copied 2 rows".into(), kind: Kind::Success, until };
        app.notice_again(Kind::Success, "Copied 2 rows");
        app.view.notices[0].until = Some(10.0);
        app.notice(Kind::Success, "Copied 2 rows");
        assert_eq!(app.view.notices, [copied(Some(10.0))], "notice: once is enough");
        app.notice_again(Kind::Success, "Copied 2 rows");
        assert_eq!(app.view.notices, [copied(None)], "shown afresh, still once");
        app.notice_again(Kind::Success, "Copied 1 row");
        assert_eq!(app.view.notices.len(), 2);
    }

    /// A window over a session with no database (notices need none), its first frame drawn, and
    /// the sender of its control requests.
    fn window() -> (Harness<'static, DbeeApp>, mpsc::Sender<ControlRequest>, dbee_testkit::TempHome) {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_ui_state(
            |ui, app: &mut DbeeApp| {
                if crate::theme::applied(ui.ctx()) {
                    app.ui(ui);
                }
            },
            DbeeApp::new(session, Some(rx)),
        );
        frame(&mut h);
        (h, tx, home)
    }

    /// One frame, with the app's input fed in first, as eframe runs it.
    fn frame(h: &mut Harness<'_, DbeeApp>) {
        let mut raw = h.input().clone();
        let ctx = h.ctx.clone();
        h.state_mut().feed_input(&ctx, &mut raw);
        *h.input_mut() = raw;
        h.step();
    }

    /// Frames until the toast of the notice shows and takes the pointer (its first frame is
    /// egui's unseen sizing pass).
    fn until_shown(h: &mut Harness<'_, DbeeApp>) {
        for _ in 0..3 {
            frame(h);
        }
        assert!(h.state().tags.rect("notice").is_some(), "the notice shows");
    }

    /// On the connection screen a toast stands above the version bar, and above the form's bar
    /// while the form shows: never over TEST, CONNECT or the version.
    #[test]
    fn a_toast_on_the_connection_screen_stays_off_its_bars() {
        let (mut h, _tx, _home) = window();
        h.state_mut().notice(Kind::Success, "Copied 13 rows");
        until_shown(&mut h);
        let clear = |h: &Harness<'_, DbeeApp>, ids: &[&str], at: &str| {
            let toast = h.state().tags.rect("notice").unwrap_or_else(|| panic!("{at}: the toast"));
            for id in ids {
                let r = h.state().tags.rect(id).unwrap_or_else(|| panic!("{at}: `{id}` drawn"));
                assert!(toast.bottom() <= r.top() || !toast.intersects(r), "{at}: the toast {toast:?} over `{id}` {r:?}");
            }
        };
        clear(&h, &["conn-version"], "the cards");
        // The form of a new connection (this session has no drivers, so no engine cards to click).
        h.state_mut().view.conn.step = connections::Step::Form;
        for _ in 0..4 {
            frame(&mut h);
        }
        clear(&h, &["conn-version", "conn-test", "conn-connect"], "the form");
    }

    #[test]
    fn a_notice_closes_on_its_close_button() {
        // Decided by the design: a notice can be closed. Its close is tagged `notice-close`, so
        // an agent can close it too; the next notice is the oldest shown then.
        let (mut h, tx, _home) = window();
        h.state_mut().notice(Kind::Error, "The count failed.");
        h.state_mut().notice(Kind::Success, "Copied 2 rows");
        until_shown(&mut h);
        assert_eq!(h.state().tags.value("notice"), Some(&json!("The count failed.")));
        let (req, reply) = ControlRequest::new("ui.click", json!({ "id": "notice-close" }));
        tx.send(req).unwrap();
        let mut answer = None;
        for _ in 0..10 {
            frame(&mut h);
            if let Ok(r) = reply.try_recv() {
                answer = Some(r);
                break;
            }
        }
        assert_eq!(answer.map(|a| a["ok"].clone()), Some(json!(true)), "the close was clicked");
        frame(&mut h);
        let texts: Vec<String> = h.state().view.notices.iter().map(|n| n.text.to_string()).collect();
        assert_eq!(texts, ["Copied 2 rows"], "the closed notice is gone, the next one is lowest");
        assert_eq!(h.state().tags.value("notice"), Some(&json!("Copied 2 rows")));
    }

    #[test]
    fn a_notice_still_goes_after_four_seconds() {
        // On egui's clock, moved here rather than waited for: a notice shows until four seconds
        // after it first showed.
        let (mut h, _tx, _home) = window();
        h.state_mut().notice(Kind::Error, "The count failed.");
        frame(&mut h);
        let first = h.ctx.input(|i| i.time);
        let until = h.state().view.notices.front().and_then(|n| n.until).unwrap_or_else(|| panic!("the notice showed"));
        assert_eq!(until, first + 4.0, "four seconds from when it first showed");
        h.input_mut().time = Some(until - 0.01);
        frame(&mut h);
        assert_eq!(h.state().tags.value("notice"), Some(&json!("The count failed.")), "still there just before");
        h.input_mut().time = Some(until);
        frame(&mut h);
        assert!(h.state().view.notices.is_empty() && h.state().tags.value("notice").is_none(), "gone at four seconds");
    }

    /// The text each toast's tag says, the oldest first (`notice`, `notice-2`, `notice-3`).
    fn toasts_shown(h: &Harness<'_, DbeeApp>) -> Vec<String> {
        ["notice", "notice-2", "notice-3", "notice-4"]
            .iter()
            .filter_map(|id| h.state().tags.value(id).and_then(Json::as_str).map(str::to_owned))
            .collect()
    }

    #[test]
    fn toasts_stack_and_the_first_stays_where_it_is() {
        // Two notices in quick succession show at once, 8 apart, the newer above; the one shown
        // already does not move.
        let (mut h, _tx, _home) = window();
        h.state_mut().notice(Kind::Success, "Copied 2 rows");
        until_shown(&mut h);
        let first = h.state().tags.rect("notice").unwrap_or_else(|| panic!("the first toast"));
        h.state_mut().notice(Kind::Error, "The count failed.");
        for _ in 0..3 {
            frame(&mut h);
        }
        assert_eq!(toasts_shown(&h), ["Copied 2 rows", "The count failed."], "both show at once");
        assert_eq!(h.state().tags.rect("notice"), Some(first), "the first has not moved");
        let second = h.state().tags.rect("notice-2").unwrap_or_else(|| panic!("the second toast"));
        assert_eq!(first.top() - second.bottom(), 8.0, "8 apart, the second above: {first:?} {second:?}");
        assert_eq!(second.right(), first.right(), "both 12 from the right");
        assert!(h.state().tags.rect("notice-close-2").is_some(), "the second has its own close");
    }

    #[test]
    fn a_fourth_toast_waits_until_one_goes() {
        // The first a second before the others: it goes first, and the fourth takes its place.
        let (mut h, _tx, _home) = window();
        h.state_mut().notice(Kind::Success, "one");
        until_shown(&mut h);
        let until = h.state().view.notices.front().and_then(|n| n.until).unwrap_or_else(|| panic!("the first showed"));
        h.input_mut().time = Some(until - 3.0);
        for text in ["two", "three", "four"] {
            h.state_mut().notice(Kind::Success, text);
        }
        for _ in 0..3 {
            frame(&mut h);
        }
        assert_eq!(toasts_shown(&h), ["one", "two", "three"], "three at most");
        assert_eq!(h.state().view.notices.get(3).map(|n| n.until), Some(None), "the fourth's clock has not started");
        h.input_mut().time = Some(until);
        for _ in 0..3 {
            frame(&mut h);
        }
        assert_eq!(toasts_shown(&h), ["two", "three", "four"], "the first gone, the fourth shows");
        let fourth = h.state().view.notices.get(2).and_then(|n| n.until).unwrap_or_else(|| panic!("the fourth showed"));
        assert!(fourth >= until + 4.0, "its own four seconds from when it showed: {fourth} after {until}");
    }

    #[test]
    fn closing_the_second_toast_leaves_the_others() {
        let (mut h, tx, _home) = window();
        for text in ["one", "two", "three"] {
            h.state_mut().notice(Kind::Success, text);
        }
        until_shown(&mut h);
        let (req, reply) = ControlRequest::new("ui.click", json!({ "id": "notice-close-2" }));
        tx.send(req).unwrap();
        let mut answer = None;
        for _ in 0..10 {
            frame(&mut h);
            if let Ok(r) = reply.try_recv() {
                answer = Some(r);
                break;
            }
        }
        assert_eq!(answer.map(|a| a["ok"].clone()), Some(json!(true)), "the close was clicked");
        frame(&mut h);
        let texts: Vec<String> = h.state().view.notices.iter().map(|n| n.text.to_string()).collect();
        assert_eq!(texts, ["one", "three"], "the second alone is gone");
        assert_eq!(toasts_shown(&h), ["one", "three"]);
    }

    #[test]
    fn a_long_notice_wraps() {
        // Review Focus 4: a long text wraps inside the toast, which stays at most 480 wide and
        // 12 from the window's right edge, and grows by its lines.
        let long = "The theme is set for now, but it couldn't be saved in /home/someone/.config/dbee/settings.json (Permission denied): \
                    check that you can write to /home/someone/.config/dbee.";
        let (mut h, _tx, _home) = window();
        h.state_mut().notice(Kind::Error, long);
        until_shown(&mut h);
        let toast = h.state().tags.rect("notice").unwrap_or(egui::Rect::NOTHING);
        assert!(toast.width() <= 480.0 + 0.5 && toast.width() > 420.0, "at most 480 wide, the words filling it: {toast:?}");
        assert_eq!(toast.right(), 1000.0 - 12.0, "12 from the right: {toast:?}");
        let words = h.output().shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == long => Some((t.galley.rows.len(), t.galley.rect.translate(t.pos.to_vec2()))),
            _ => None,
        });
        let (rows, at) = words.unwrap_or_else(|| panic!("the words drawn"));
        assert!(rows >= 2, "wrapped: {rows} lines");
        assert!(toast.contains_rect(at), "inside the toast: {at:?} in {toast:?}");
        assert!(toast.height() > 44.0, "taller than one line's 44: {toast:?}");
    }

    #[test]
    fn numbers_and_paths_in_toasts_are_mono() {
        // The design: the names, paths and numbers dbee puts in a toast's words are drawn in
        // `data` as written, the rest in `body`; the `notice` tag still says it all as plain words.
        use crate::theme::Text::{Body, Data};
        let (_, _, home) = window();
        let folder = home.home.join("Settings-Is-A-File");
        std::fs::write(&folder, "").unwrap();
        let file = folder.join("settings.json");
        let cases: [(&str, Box<dyn Fn(&mut DbeeApp)>); 4] = [
            ("copied", Box::new(|app| app.notice_again(Kind::Success, crate::ui_commands::copied_notice(1_250)))),
            ("pasted", Box::new(|app| app.notice(Kind::Error, crate::editors::pasted_too_large(3 * 1024 * 1024)))),
            ("too large", Box::new(|app| app.notice(Kind::Error, crate::editors::too_large(2 * 1024 * 1024)))),
            (
                "theme",
                Box::new(move |app| {
                    app.settings_dir = Some(folder.clone());
                    app.choose_theme(dbee_config::ThemeChoice::Light);
                }),
            ),
        ];
        for (case, push) in cases {
            let (mut h, _tx, _home) = window();
            push(h.state_mut());
            until_shown(&mut h);
            let whole = h.state().tags.value("notice").and_then(Json::as_str).unwrap_or_else(|| panic!("{case}: the notice")).to_owned();
            let drawn = crate::components::sections(&h.output().shapes, &whole);
            let colour = drawn.first().map(|(_, _, c)| *c);
            assert!(drawn.iter().all(|(_, _, c)| Some(*c) == colour), "{case}: one colour: {drawn:?}");
            let got: Vec<(&str, egui::FontId)> = drawn.iter().map(|(w, f, _)| (w.as_str(), f.clone())).collect();
            let names: Vec<&str> = got.iter().filter(|(_, f)| *f == Data.font()).map(|(w, _)| *w).collect();
            assert!(got.iter().all(|(_, f)| *f == Data.font() || *f == Body.font()), "{case}: only body and data: {got:?}");
            let shown = file.display().to_string();
            let want: Vec<&str> = match case {
                "copied" => vec!["1,250"],
                "pasted" => vec!["3.0 MiB", "32 KiB"],
                "too large" => vec!["2.0 MiB", "32 KiB"],
                _ => vec![shown.as_str(), shown.strip_suffix("/settings.json").unwrap_or_default()],
            };
            assert_eq!(names, want, "{case}: the names in data: {got:?}");
            assert_eq!(got.iter().map(|(w, _)| *w).collect::<String>(), whole, "{case}: every word drawn, as tagged");
        }
    }

    #[test]
    fn a_query_runs_errors_on_its_tab_show_in_the_card_only() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let shown = |app: &DbeeApp| app.view.notices.iter().map(|n| n.text.to_string()).collect::<Vec<_>>();
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
        assert_eq!(notices, ["Connection lost.", "There is no statement to run."]);
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
        // Every other error reads as its own text, as a sentence; Display keeps the command for the
        // control channel.
        let db = EngineError::Db { code: "server", message: "duplicate key (SQLSTATE 23505)".into(), data: json!({}) };
        assert_eq!(notice_text(&db), "Duplicate key (SQLSTATE 23505).");
        let lost = EngineError::Db { code: "connectionLost", message: "the connection was lost".into(), data: json!({}) };
        assert_eq!(notice_text(&lost), "The connection was lost.");
        assert_eq!(notice_text(&EngineError::Timeout), sentence(&EngineError::Timeout.to_string()));
        assert_eq!(disabled("no staged changes").to_string(), "`edit.apply` cannot run now: no staged changes");
    }

    #[test]
    fn ui_commands_run_through_run() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui)).drop_without_applying_deltas();
        ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui)).drop_without_applying_deltas();
        assert_eq!(app.frames(), 1, "the first frame only sets the theme");
        assert!(!app.session().busy());
        // A UI command runs through `run`, like the panels' clicks.
        assert_eq!(app.run("ui.toggleValuePanel", json!({})).unwrap(), Outcome::Done(json!({ "visible": false })));
        assert!(app.run("no.such", json!({})).is_err());
    }
}

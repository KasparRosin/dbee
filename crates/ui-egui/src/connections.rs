//! The connection screen: the workspace's frame (the menu bar on top, the version bar along the
//! bottom), the connections in a sidebar on the left, the engine cards or the connection form on
//! the right ([`screen`]).
//!
//! - The menu bar is the workspace's; its File > Connect runs the form's Connect here, and is off
//!   on the engine cards.
//! - Left: `+ NEW CONNECTION`, a filter, the connections file's warning, then one list: the saved
//!   connections in the file's order, then the recent ones that are not saved, newest first (a
//!   saved one is not listed twice), each row with its colour, its engine's logo and its address
//!   under the name. A single click loads a connection into the form, a double-click connects;
//!   right-click offers Connect · Edit · Duplicate · Delete (Delete asks first: it deletes the
//!   saved passwords too).
//! - Right, the pane, full bleed: the title row (the title, FILL FROM URL; the connection's colour
//!   as a 2 px edge along its top). A new connection starts with the engine cards (K2), one cell
//!   per database type ([`DbKind::ALL`]) with its logo; a type this build has no driver for (not
//!   in the session's `kinds`) is greyed with UPCOMING and cannot be chosen. A card, or a URL (its
//!   scheme names the type), opens the form under the engine row: the type's logo and name, and
//!   CHANGE ENGINE, which goes back to the cards (the form keeps what was typed, and a port that still
//!   holds the old type's default takes the new one's; not while a test or connect runs). A
//!   connection opened from the saved list or Recent opens straight into the form, its type fixed;
//!   Fill from URL then takes that type's URLs only. One saved during the edit, or a Duplicate,
//!   keeps Change engine.
//! - The form ([`crate::conn_form`]) has the rows of the type's settings, then the form's bar
//!   along the pane's bottom: SAVE, `…` (a saved connection's Duplicate and Delete), TEST and
//!   CONNECT; above it the strip shows what came of the last command (an error band under a
//!   heading naming what failed, a result or a note), taking its room from the fields' view.
//!   Every row has a fixed height, so typing, a message or another login choice never moves the
//!   form; only turning the SSH tunnel on or off does. The right side is as tall with the cards
//!   as with the form. In a short window only the fields scroll: the title row, the engine row,
//!   the strip and the bar stay in view. At the app's first size (1100 × 720) every row of a
//!   server form is in view, the SSH tunnel's too.
//! - Test and Connect run as jobs (`connection.test`, `connection.open`); while one runs both
//!   buttons are disabled and say so, and a double-click on a card does nothing. CONNECT saves a
//!   connection that isn't saved yet once it connects (`save: true`); a saved one keeps its saved
//!   settings until SAVE. A `needs…` answer opens a dialog ([`crate::dialogs`]) that runs the
//!   command again; other errors show in the strip with the server's message.
//! - A server type's Database field has a list button: it runs `connection.listDatabases` with
//!   the form's settings, as Test runs, and says so while it lists (disabled then, and while a
//!   test or connect runs). The names open under the field; a click or Enter on the highlighted
//!   one (Down and Up move it) fills the field in, and Escape closes the list. When it opens, the
//!   field takes the keyboard if no widget has it. It takes those keys only while the keyboard is
//!   on the field, on its button or nowhere: the keyboard going to another widget, typing in the
//!   field, a click elsewhere or a dialog closes it. A list that
//!   comes after the form changed its server (the type, host, port, user, TLS, SSH or the type's
//!   options, another connection, Change engine) or left the screen is dropped.
//! - Ctrl+P (the quick search) finds the saved connections here too; choosing one connects as a
//!   double-click on its card does. Chosen over the workspace, it does so once the open
//!   connection has closed through `connection.close` and its questions ([`switch_to`]).
//! - The file pickers (CHOOSE for the key and the CA file, CHOOSE and NEW FILE for a database
//!   file) run rfd on a thread of their own and send the path back; a picker that cannot open
//!   leaves a note in its row ([`crate::picker`]). After NEW FILE, every test and connect asks
//!   for the file to be made (`createFile`) until a connect succeeds; CHOOSE,
//!   another connection or another type forget it.
//!
//! The form (its fields, their validation, and drawing it) is [`crate::conn_form`]; this module
//! draws the screen around it and acts on both.
//!
//! Secrets: the typed password lives only in the form (and in the params of the command it goes
//! to). `ui.inspect` reports the field as masked with whether it is set; `Debug` and the logs
//! never show it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind, Settings, SshAuth, SshConfig, SslMode};
use dbee_engine::{EngineError, JobId, Json, Screen, State, Which};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, CornerRadius, Frame, Label, Layout, Margin, Rect, ScrollArea, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::DbeeApp;
use crate::app::{Origin, sentence};
use crate::automation::Tags;
use crate::components::Look;
use crate::components::button::BarButton;
use crate::components::chip::Chip;
use crate::components::empty::{Phrase, Place};
use crate::components::icon::Icon;
use crate::components::logo::Logo;
use crate::components::menu::{self, Item};
use crate::components::rows::ConnectionRow;
use crate::components::sidebar;
use crate::components::status::Address;
use crate::components::toast;
use crate::components::tooltip::Hint;
use crate::conn_form::{self, Form};
use crate::dialogs::{self, Dialog};
use crate::picker::{self, Pick, Picker};
use crate::quiet::Watch;
use crate::theme::{
    self, Text, connection_color,
    size::{
        BAR_H, BAR_LINE, CONN_COLOUR_EDGE, CONN_WARNING_PAD_Y, CONNECTION_ROW_H, ENGINE_CARD_CHIP_INSET, ENGINE_CARD_H, ENGINE_CARD_LOGO,
        ENGINE_CARD_MIN_W, ENGINE_CARD_NAME_GAP, ENGINE_CARD_PAD, ENGINE_CARDS_TOP, ENGINE_HEADING_TOP, INSET, RADIUS_CONTROL, SIDEBAR_MAX_W,
        SIDEBAR_MIN_W, SIDEBAR_PAD_X, SIDEBAR_W, SLIM_BAR_H,
    },
};

pub(crate) const OPEN: &str = "connection.open";
pub(crate) const TEST: &str = "connection.test";
pub(crate) const SAVE: &str = "connection.save";
pub(crate) const PARSE_URL: &str = "connection.parseUrl";
pub(crate) const LIST_DATABASES: &str = "connection.listDatabases";
pub(crate) const CLOSE: &str = "connection.close";
const SSH_KEY: &str = "connection.sshKey";
const DELETE: &str = "connection.delete";
const DUPLICATE: &str = "connection.duplicate";
const CANCEL: &str = "connection.cancel";

/// Which form job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    Test,
    Connect,
}

/// What the strip above the form's bar is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Something failed (`conn-error`).
    Error,
    /// A test passed, the connection was saved (`conn-result`).
    Result,
    /// Something to know that is neither (`conn-note`), e.g. about a file picker.
    Note,
}

/// What the strip above the form's bar shows: an error band under its heading, or a result or a
/// note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub kind: Kind,
    /// An error's heading: what failed ([`heading`]); empty for a result or a note.
    pub heading: &'static str,
    pub text: String,
}

impl Message {
    pub(crate) fn error(command: &str, text: impl Into<String>) -> Message {
        Message { kind: Kind::Error, heading: heading(command), text: text.into() }
    }

    pub(crate) fn result(text: impl Into<String>) -> Message {
        Message { kind: Kind::Result, heading: "", text: text.into() }
    }

    pub(crate) fn note(text: impl Into<String>) -> Message {
        Message { kind: Kind::Note, heading: "", text: text.into() }
    }
}

/// An error band's heading by the command that failed.
fn heading(command: &str) -> &'static str {
    match command {
        TEST => "The test failed",
        SAVE => "The connection was not saved",
        LIST_DATABASES => "The databases could not be listed",
        _ => "The connection failed",
    }
}

/// The line under the SSH login choice: what the login will use.
#[derive(Debug, Default)]
pub(crate) struct KeyLine {
    /// The login `text` describes; `None` until known (or to look again).
    pub for_auth: Option<SshAuth>,
    pub text: String,
    /// The key file the login will use, when it is one (the key picker starts in its folder).
    pub path: Option<PathBuf>,
    /// The `connection.sshKey` job running, and the login it is about.
    pub pending: Option<(JobId, SshAuth)>,
}

/// The Database field's list of the server's databases.
#[derive(Debug, Default)]
pub(crate) struct DatabaseList {
    /// The `connection.listDatabases` job the list waits for.
    pub pending: Option<JobId>,
    /// What the last listing gave.
    pub names: Vec<String>,
    /// The list shows, under the field.
    pub open: bool,
    /// The name Enter picks, which Down and Up move.
    pub highlight: usize,
    /// The list's button as last drawn: the keyboard on it keeps the list open.
    pub button: Option<egui::Id>,
    /// The list's scroll offset and the height in view, as last drawn.
    pub offset: f32,
    pub view_h: f32,
    /// Scroll the highlighted name into view when the list is next drawn.
    pub reveal: bool,
    /// The list just opened: the Database field takes the keyboard if no widget has it.
    pub take_keyboard: bool,
}

impl DatabaseList {
    /// The outcome of the listing `job` (`None`: it ended before any job started) is the one the
    /// list waits for; it then waits no more.
    fn takes(&mut self, job: Option<JobId>) -> bool {
        match job {
            None => true,
            Some(job) if self.pending == Some(job) => {
                self.pending = None;
                true
            }
            Some(_) => false,
        }
    }
}

/// Where a config goes: the server, the login, TLS, the tunnel and the type's options, which a
/// database list is of.
type ServerOf<'a> = (DbKind, &'a str, u16, &'a str, SslMode, Option<&'a std::path::Path>, Option<&'a SshConfig>, &'a BTreeMap<String, String>);

fn server_of(c: &ConnectionConfig) -> ServerOf<'_> {
    (c.kind, &c.host, c.port, &c.user, c.ssl_mode, c.ca_file.as_deref(), c.ssh.as_ref(), &c.options)
}

/// The form still goes to the server a listing ran with `params` for.
fn same_server(params: &Json, form: &Form) -> bool {
    let then = params.get("config").and_then(|c| serde_json::from_value::<ConnectionConfig>(c.clone()).ok());
    match (then, form.to_config()) {
        (Some(then), Ok(now)) => server_of(&then) == server_of(&now),
        _ => false,
    }
}

/// What the right side shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The engine cards of a new connection.
    Engines,
    Form,
}

/// The error code of an attempt `connection.cancel` stopped.
const CANCELLED: &str = "cancelled";

/// What only the connection screen keeps.
#[derive(Debug)]
pub(crate) struct ConnScreen {
    pub step: Step,
    pub form: Form,
    /// Counts the connections the form has shown: a result for an earlier one is not shown.
    pub generation: u64,
    pub filter: String,
    /// The test or connect started from the form, until its result arrives.
    pub running: Option<(JobId, Running)>,
    pub message: Option<Message>,
    pub key: KeyLine,
    pub picker: Option<Picker>,
    /// The Database field's list.
    pub databases: DatabaseList,
    /// The session has a secret store (`document.inspect`'s `secretStore`).
    pub has_store: bool,
    /// What to open once `connection.close` closed the connection that was open: the saved
    /// connection chosen in the quick search, or a database file dropped on the window
    /// ([`switch_pending`]).
    pub switch: Option<SwitchTo>,
    /// The switch has run its own `connection.close`: a connection still open once that is done
    /// closed nothing (Cancel, an error).
    pub switch_closed: bool,
}

/// What a switch opens ([`ConnScreen::switch`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchTo {
    /// A saved connection, by its id.
    Saved(String),
    /// A database file.
    File(PathBuf),
}

impl ConnScreen {
    pub(crate) fn new(has_store: bool) -> ConnScreen {
        ConnScreen {
            step: Step::Engines,
            form: Form::new(has_store),
            generation: 0,
            filter: String::new(),
            running: None,
            message: None,
            key: KeyLine::default(),
            picker: None,
            databases: DatabaseList::default(),
            has_store,
            switch: None,
            switch_closed: false,
        }
    }

    /// Shows a new, empty connection: the engine cards first.
    fn new_connection(&mut self) {
        self.form = Form::new(self.has_store);
        self.step = Step::Engines;
        self.forget_shown();
    }

    /// The form shows another connection, or another type: its message, key line and database
    /// list are gone, and a late result of what ran before does not show.
    fn forget_shown(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.message = None;
        self.key.for_auth = None;
        self.databases = DatabaseList::default();
    }

    /// The workspace shows: the database list goes, so it never opens again on its own.
    pub(crate) fn forget_list(&mut self) {
        self.databases = DatabaseList::default();
    }

    /// Loads `c` into the form, its type fixed when it was opened from the saved list or Recent
    /// (`fixed`). The same connection again keeps the typed password and what the form shows under
    /// its buttons.
    fn load(&mut self, c: &ConnectionConfig, fixed: bool) {
        let same = !c.id.is_empty() && c.id == self.form.id;
        let password = if same { std::mem::take(&mut self.form.password) } else { String::new() };
        self.form = Form::from_config(c);
        self.form.password = password;
        self.form.engine_fixed = fixed;
        self.step = Step::Form;
        if same {
            self.key.for_auth = None;
        } else {
            self.forget_shown();
        }
    }
}

/// `path` with the home directory as `~`.
pub(crate) fn tilde(path: &str) -> String {
    tilde_at(path, dbee_config::home_dir().as_deref())
}

/// `path` with `home` as `~`.
pub(crate) fn tilde_at(path: &str, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home
        && let Some(home) = home.to_str().filter(|h| !h.is_empty())
        && let Some(rest) = path.strip_prefix(home)
        && (rest.starts_with('/') || rest.starts_with('\\'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// `text` with each path in the home directory written with `~`.
fn tilde_in(text: &str) -> String {
    match dbee_config::home_dir().and_then(|h| h.to_str().map(str::to_string)).filter(|h| !h.is_empty()) {
        Some(home) => text.replace(&format!("{home}/"), "~/").replace(&format!("{home}\\"), "~\\"),
        None => text.to_string(),
    }
}

/// An error as the form shows it: bad params by their message alone, the rest as they are.
pub(crate) fn error_text(e: &EngineError) -> String {
    match e {
        EngineError::BadParams { msg, .. } => msg.clone(),
        other => other.to_string(),
    }
}

// ---- results ---------------------------------------------------------------------------------

/// A form job started (`app.start` with [`Origin::Form`]): Test and Connect, or the database
/// list's button, now show it runs.
pub(crate) fn started(app: &mut DbeeApp, job: JobId, command: &str) {
    let conn = &mut app.view.conn;
    match command {
        TEST => conn.running = Some((job, Running::Test)),
        OPEN => conn.running = Some((job, Running::Connect)),
        LIST_DATABASES => conn.databases.pending = Some(job),
        _ => {}
    }
}

/// The outcome of a form command (`job` when it ran as one). A result for a connection the
/// form no longer shows is dropped; so is a database list the form no longer waits for (Change
/// engine), whose server it no longer names, or that comes when the form is not on screen (a
/// connect succeeded meanwhile).
pub(crate) fn form_done(
    app: &mut DbeeApp,
    command: &'static str,
    params: Json,
    generation: u64,
    job: Option<JobId>,
    result: Result<Json, EngineError>,
) {
    let on_screen = app.session.state().screen == Screen::Connections && app.view.conn.step == Step::Form;
    let conn = &mut app.view.conn;
    if let (Some((running, _)), Some(job)) = (conn.running, job)
        && running == job
    {
        conn.running = None;
    }
    let listing = command == LIST_DATABASES;
    if listing && !conn.databases.takes(job) {
        return;
    }
    if generation != conn.generation || (listing && !(on_screen && same_server(&params, &conn.form))) {
        return;
    }
    match result {
        Ok(v) => match command {
            SAVE => {
                if let Some(id) = v.get("id").and_then(Json::as_str) {
                    conn.form.id = id.to_string();
                }
                conn.message = Some(Message::result("Saved."));
            }
            TEST => conn.message = Some(Message::result(test_text(&v))),
            LIST_DATABASES => {
                let names: Vec<String> = v
                    .get("databases")
                    .and_then(Json::as_array)
                    .map(|a| a.iter().filter_map(Json::as_str).map(str::to_owned).collect())
                    .unwrap_or_default();
                if names.is_empty() {
                    conn.message = Some(Message::note("No databases to list."));
                }
                let list = &mut conn.databases;
                list.highlight = names.iter().position(|n| *n == conn.form.database).unwrap_or(0);
                list.reveal = true;
                list.take_keyboard = true;
                list.open = !names.is_empty();
                list.names = names;
            }
            OPEN => {
                conn.message = None;
                // The file is there now.
                conn.form.create_file = false;
                // Saved as it connected: the form is that saved connection now, so the next
                // CONNECT doesn't save it again. (A file that could not be written says so in a
                // notice of the engine's.)
                let connection = v.get("connection");
                if connection.and_then(|c| c.get("saved")).and_then(Json::as_bool) == Some(true)
                    && let Some(id) = connection.and_then(|c| c.get("id")).and_then(Json::as_str)
                {
                    conn.form.id = id.to_string();
                }
                if let Some(note) = v.get("secretsNote").and_then(Json::as_str) {
                    app.notice(toast::Kind::Error, sentence(&format!("The password was not saved: {note}")));
                }
            }
            _ => {}
        },
        // A cancel the user asked for shows nothing: the form is as it was before.
        Err(e) if e.code() == CANCELLED => {}
        Err(e) => {
            let rerun = dialogs::Rerun { command, params, origin: Origin::Form(generation) };
            if !dialogs::ask(app, &e, rerun) {
                app.view.conn.message = Some(Message::error(command, error_text(&e)));
            }
        }
    }
}

/// `{server, via, warnings}` of a test, as one line; without `via` for a type that takes no tunnel.
fn test_text(v: &Json) -> String {
    let server = v.get("server").and_then(Json::as_str).unwrap_or("the database");
    let mut text = match v.get("via").and_then(Json::as_str) {
        Some(via) => format!("Test passed: {server}, {}.", tilde_in(via)),
        None => format!("Test passed: {server}."),
    };
    if let Some(warnings) = v.get("warnings").and_then(Json::as_array) {
        for w in warnings.iter().filter_map(Json::as_str) {
            text.push(' ');
            text.push_str(w);
        }
    }
    text
}

/// The outcome of a `connection.sshKey` job (`job`), or of one that could not start (`None`).
pub(crate) fn key_line_done(app: &mut DbeeApp, job: Option<JobId>, result: Result<Json, EngineError>) {
    let key = &mut app.view.conn.key;
    let auth = match (key.pending.take(), job) {
        (Some((pending, auth)), Some(job)) if pending == job => auth,
        (pending, Some(_)) => {
            // Not the job the line waits for.
            key.pending = pending;
            return;
        }
        (pending, None) => {
            key.pending = pending;
            if let Err(e) = result {
                key.text = format!("Could not tell which key: {}", error_text(&e));
            }
            return;
        }
    };
    key.path = None;
    key.text = match result {
        Ok(v) => match v.get("method").and_then(Json::as_str) {
            Some("file") => {
                let path = v.get("path").and_then(Json::as_str).unwrap_or_default();
                key.path = Some(PathBuf::from(path));
                format!("Uses {}", tilde(path))
            }
            Some("agent") => {
                let n = v.get("identities").and_then(Json::as_u64).unwrap_or(0);
                format!("Uses the SSH agent ({n} {})", if n == 1 { "key" } else { "keys" })
            }
            Some("password") => "Asks for the SSH password when connecting".into(),
            _ if auth == SshAuth::Agent => "No SSH agent was found".into(),
            _ => "No key found: no agent key and no id_ed25519, id_ecdsa or id_rsa in ~/.ssh".into(),
        },
        Err(e) => format!("Could not tell which key: {}", error_text(&e)),
    };
    key.for_auth = Some(auth);
}

/// The database types whose URLs can fill the form: those of the session's `kinds` that have a
/// URL, and of them only the form's own once its type is fixed.
pub(crate) fn url_kinds(form: &Form, kinds: &[DbKind]) -> Vec<DbKind> {
    kinds.iter().copied().filter(|k| !k.info().url_schemes.is_empty() && (!form.engine_fixed || *k == form.kind)).collect()
}

/// Fills the form from `connection.parseUrl`'s result: the type its scheme names, and the parts
/// the URL has; the password is the one the dialog read from the URL, as the reply leaves it out.
/// The form shows, past the engine cards.
pub(crate) fn fill_from_url(app: &mut DbeeApp, parts: &Json, password: Option<String>) {
    let conn = &mut app.view.conn;
    if let Some(kind) = parts.get("kind").and_then(Json::as_str).and_then(DbKind::from_id).filter(|k| *k != conn.form.kind) {
        conn.form.switch_kind(kind);
        conn.forget_shown();
    }
    let form = &mut conn.form;
    let text = |k: &str| parts.get(k).and_then(Json::as_str).map(str::to_string);
    if let Some(v) = text("host") {
        form.host = v;
    }
    if let Some(v) = parts.get("port").and_then(Json::as_u64) {
        form.port = v.to_string();
    }
    if let Some(v) = text("user") {
        form.user = v;
    }
    if let Some(v) = password {
        form.password = v;
    }
    if let Some(v) = text("database") {
        form.database = v;
    }
    if let Some(mode) = parts.get("sslMode").filter(|m| !m.is_null()).and_then(|m| serde_json::from_value::<SslMode>(m.clone()).ok()) {
        form.ssl_mode = mode;
    }
    app.view.conn.step = Step::Form;
    app.view.conn.message = None;
}

/// Opens `to`: the saved connection the quick search chose, or a database file dropped on the
/// window. With none open, a saved connection connects as a double-click on its card does, and a
/// file opens (`connection.openFile`); else `connection.close` runs first, with its questions, and
/// the open follows once it closed ([`switch_pending`]).
pub(crate) fn switch_to(app: &mut DbeeApp, to: SwitchTo) {
    if app.session.state().connection(Which::Active).is_none() {
        open_switch(app, &to);
        return;
    }
    app.view.conn.switch = Some(to);
    app.view.conn.switch_closed = false;
    switch_pending(app);
}

/// Each frame: a switch waits while a dialog shows or a close ends a transaction (its own, or one
/// already running when it was chosen, which may close the connection for it). Then, with the
/// connection closed, it opens; with it still open, it runs `connection.close` with its questions
/// once, and after that is dropped, so the open connection and its tabs stay as they were.
pub(crate) fn switch_pending(app: &mut DbeeApp) {
    if app.view.dialog.is_some() || app.session.running(CLOSE) || app.session.running(dialogs::TAB_CLOSE) {
        return;
    }
    let Some(to) = app.view.conn.switch.take() else { return };
    if app.session.state().connection(Which::Active).is_none() {
        open_switch(app, &to);
    } else if !std::mem::replace(&mut app.view.conn.switch_closed, true) {
        app.view.conn.switch = Some(to);
        app.start(CLOSE, json!({}), Origin::Menu);
        switch_pending(app);
    }
}

fn open_switch(app: &mut DbeeApp, to: &SwitchTo) {
    match to {
        SwitchTo::Saved(id) => connect_saved(app, id),
        SwitchTo::File(path) => app.open_file(path),
    }
}

/// Connects to the saved connection `id` as a double-click on its card does: it loads into the
/// form, which connects (not while a test or connect runs).
fn connect_saved(app: &mut DbeeApp, id: &str) {
    match app.session.state().store.get(id).cloned() {
        Some(c) => apply_connect(app, &c),
        None => app.notice(toast::Kind::Error, "That connection is not saved any more."),
    }
}

/// [`Act::Connect`]: loads `c` into the form and connects, unless a test or connect runs.
fn apply_connect(app: &mut DbeeApp, c: &ConnectionConfig) {
    if app.view.conn.running.is_none() {
        app.view.conn.load(c, true);
        run_form(app, OPEN);
    }
}

/// Deletes the saved connection `id` (the user confirmed); the form empties when it showed it.
pub(crate) fn delete(app: &mut DbeeApp, id: &str) {
    match app.run(DELETE, json!({ "id": id })) {
        Ok(_) => {
            if app.view.conn.form.id == id {
                app.view.conn.new_connection();
            }
        }
        Err(e) => app.notice(toast::Kind::Error, crate::app::notice_text(&e)),
    }
}

// ---- each frame ------------------------------------------------------------------------------

/// What the user did on the screen this frame, applied once it is drawn.
pub(crate) enum Act {
    New,
    /// An engine card: the form for that type.
    ChooseKind(DbKind),
    /// Back to the engine cards.
    ChangeEngine,
    Load(ConnectionConfig),
    /// A double-click or the menu's Connect: load, then connect.
    Connect(ConnectionConfig),
    Duplicate(String),
    AskDelete {
        id: String,
        name: String,
    },
    Test,
    ConnectForm,
    /// CANCEL beside CONNECT: stops the test or connect that runs.
    Cancel,
    Save,
    /// The Database field's list button: list the server's databases.
    ListDatabases,
    /// A name chosen from the Database field's list.
    ChooseDatabase(String),
    FillFromUrl,
    Pick(Pick),
    StopPicking,
    ClearCa,
    AutoKey,
}

/// The connection screen (step 5 of the frame).
pub(crate) fn screen(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    tick(app, &ctx);
    if app.view.dialog.is_some() || app.view.quick.open {
        // The dialog or the quick search has the keyboard: its Enter and Escape are its own.
        app.view.conn.databases.open = false;
    }
    let mut acts = Vec::new();
    // The workspace's menu bar; File > Connect is the form's Connect here.
    let why = if app.view.conn.step != Step::Form {
        Some("Connect: not now, choose a database first")
    } else if app.view.conn.running.is_some() {
        Some(BUSY_CONNECT)
    } else {
        None
    };
    let t = theme::tokens(&ctx);
    egui::Panel::top("dbee-menubar").exact_size(SLIM_BAR_H).show_separator_line(false).frame(Frame::NONE).show(ui, |ui| {
        if crate::menu::bar_taking(app, ui, &[(OPEN, why)]) == Some(OPEN) {
            acts.push(Act::ConnectForm);
        }
    });
    egui::Panel::bottom("conn-version-bar").exact_size(SLIM_BAR_H).show_separator_line(false).frame(Frame::NONE).show(ui, |ui| {
        version_bar(ui, t, &mut app.tags);
    });
    {
        let DbeeApp { session, view, tags, .. } = app;
        let kinds = session.kinds();
        let saved = !view.conn.form.id.is_empty() && session.state().store.is_saved(&view.conn.form.id);
        // What runs, once it has run 300 ms: the connecting state shows then, so a fast connect
        // never flickers.
        let shown = view.conn.running.and_then(|(job, kind)| view.quiet.past(ui.ctx(), Watch::Form(job)).map(|ran| (kind, ran)));
        side(ui, session.state(), &mut view.conn, tags, &mut acts);
        main(ui, &mut view.conn, shown, &kinds, saved, tags, &mut acts);
    }
    for act in acts {
        apply(app, act, &ctx);
    }
}

/// Before drawing: the picker's answer, and the key line's question when the login changed.
fn tick(app: &mut DbeeApp, ctx: &egui::Context) {
    picker::poll(&mut app.view.conn, ctx);
    let conn = &mut app.view.conn;
    if !conn.form.ssh_on || !conn.form.kind.takes_ssh() || conn.step != Step::Form {
        return;
    }
    let want = conn.form.ssh_auth();
    if conn.key.for_auth.as_ref() == Some(&want) || conn.key.pending.is_some() {
        return;
    }
    if want == SshAuth::Password {
        conn.key = KeyLine { for_auth: Some(want), text: "Asks for the SSH password when connecting".into(), path: None, pending: None };
        return;
    }
    let ssh = SshConfig { host: conn.form.ssh_host.clone(), port: 22, user: conn.form.ssh_user.clone(), auth: want.clone(), ..SshConfig::default() };
    let params = json!({ "config": { "ssh": ssh } });
    match app.start(SSH_KEY, params, Origin::KeyLine) {
        Some(job) => app.view.conn.key.pending = Some((job, want)),
        // Failed at once; the line says why, and it is not asked again until the login changes.
        None => app.view.conn.key.for_auth = Some(want),
    }
}

fn apply(app: &mut DbeeApp, act: Act, ctx: &egui::Context) {
    let busy = app.view.conn.running.is_some();
    match act {
        Act::New => app.view.conn.new_connection(),
        Act::ChooseKind(kind) => {
            let conn = &mut app.view.conn;
            if !conn.form.engine_fixed && app.session.kinds().contains(&kind) {
                if kind != conn.form.kind {
                    conn.form.switch_kind(kind);
                    conn.forget_shown();
                }
                conn.step = Step::Form;
            }
        }
        Act::ChangeEngine if !app.view.conn.form.engine_fixed && !busy => {
            app.view.conn.step = Step::Engines;
            app.view.conn.databases = DatabaseList::default();
        }
        Act::ChangeEngine => {}
        Act::Load(c) => app.view.conn.load(&c, true),
        // While a test or connect runs, a double-click starts nothing more.
        Act::Connect(c) => apply_connect(app, &c),
        Act::Duplicate(id) => match app.run(DUPLICATE, json!({ "id": id })) {
            Ok(dbee_engine::Outcome::Done(v)) => {
                let copy = v.get("id").and_then(Json::as_str).and_then(|id| app.session.state().store.get(id)).cloned();
                // A copy is a new connection: its type can still change.
                if let Some(c) = copy {
                    app.view.conn.load(&c, false);
                }
            }
            Ok(_) => {}
            Err(e) => app.notice(toast::Kind::Error, crate::app::notice_text(&e)),
        },
        Act::AskDelete { id, name } => app.view.dialog = Some(Dialog::DeleteConnection { id, name }),
        Act::Test if !busy => run_form(app, TEST),
        Act::ConnectForm if !busy => run_form(app, OPEN),
        Act::Test | Act::ConnectForm => {}
        Act::Cancel if busy => {
            let generation = app.view.conn.generation;
            app.start(CANCEL, json!({}), Origin::Form(generation));
        }
        Act::Cancel => {}
        Act::Save => run_form(app, SAVE),
        Act::ListDatabases if !busy && app.view.conn.databases.pending.is_none() => {
            app.view.conn.databases.open = false;
            run_form(app, LIST_DATABASES);
        }
        Act::ListDatabases => {}
        Act::ChooseDatabase(name) => {
            app.view.conn.form.database = name;
            app.view.conn.databases.open = false;
        }
        Act::FillFromUrl => {
            let kinds = url_kinds(&app.view.conn.form, &app.session.kinds());
            app.view.dialog = Some(Dialog::fill_from_url(kinds));
        }
        Act::Pick(target) => picker::start(&mut app.view.conn, target, ctx),
        Act::StopPicking => picker::stop(&mut app.view.conn),
        Act::ClearCa => app.view.conn.form.ca_file = None,
        Act::AutoKey => app.view.conn.form.ssh_key = None,
    }
}

/// Runs `command` (save, test, open or the database list) on the form's connection; a form that
/// is not usable says why under the buttons and runs nothing.
fn run_form(app: &mut DbeeApp, command: &'static str) {
    let conn = &mut app.view.conn;
    conn.message = None;
    let config = match conn.form.to_config() {
        Ok(c) => c,
        Err(why) => {
            conn.message = Some(Message::error(command, why));
            return;
        }
    };
    // Only a server takes a password; one typed for another type stays in the form.
    let password = if config.kind.uses_file() { String::new() } else { conn.form.password.clone() };
    let typed = !password.is_empty();
    let create_file = conn.form.create_file && config.kind.uses_file() && command != SAVE;
    let saved = !config.id.is_empty() && app.session.state().store.is_saved(&config.id);
    let conn = &app.view.conn;
    let mut params = json!({ "config": config });
    let keep = conn.has_store && conn.form.save_secrets;
    let send_password = match command {
        SAVE => keep && typed,
        _ => typed,
    };
    if let Some(p) = params.as_object_mut() {
        if send_password {
            p.insert("password".into(), json!(password));
        }
        // "Save passwords": a password typed for a saved connection is kept when it connects.
        if command == OPEN && keep && typed && saved {
            p.insert("savePassword".into(), json!(true));
        }
        // CONNECT saves a connection that isn't saved yet, once it connects; a saved one keeps
        // its saved settings (SAVE changes them).
        if command == OPEN && !saved {
            p.insert("save".into(), json!(true));
        }
        if create_file {
            p.insert("createFile".into(), json!(true));
        }
    }
    let generation = conn.generation;
    app.start(command, params, Origin::Form(generation));
}

// ---- drawing ---------------------------------------------------------------------------------

/// Whether `c` matches the filter `q` (lower case) by what its card shows: its name and its type's
/// label; a server's host, user, database and SSH host, or a file type's path.
fn matches(c: &ConnectionConfig, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let mut shown = vec![c.display_name(), c.kind.label().to_string()];
    match c.kind.info().settings {
        Settings::Server { .. } => {
            shown.extend([c.host.clone(), c.user.clone(), c.database.clone()]);
            shown.extend(c.ssh.as_ref().filter(|_| c.kind.takes_ssh()).map(|s| s.host.clone()));
        }
        Settings::File { .. } => shown.extend(c.file.as_ref().map(|f| f.display().to_string())),
    }
    shown.iter().any(|f| f.to_lowercase().contains(q))
}

/// `text` on one line in `style` and `color`, cut with "…" to `width`.
pub(crate) fn line_galley(ui: &egui::Ui, text: &str, style: Text, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    line_job(ui, style.job(text, color), width)
}

/// `job` on one line, cut with "…" to `width`.
pub(crate) fn line_job(ui: &egui::Ui, mut job: LayoutJob, width: f32) -> std::sync::Arc<egui::Galley> {
    job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// The row menu's and File > Connect's reason while a test or connect runs.
pub(crate) const BUSY_CONNECT: &str = "Connect: not now, a test or connect is running";

/// The version bar along the window's bottom: `dbee <version>` in `label` caps, 12 in, on `card`
/// with a 1 px `line` along its top. Tagged `conn-version` (the whole bar) with the words as written.
fn version_bar(ui: &mut egui::Ui, t: &theme::Tokens, tags: &mut Tags) {
    let (bar, r) = ui.allocate_exact_size(vec2(ui.available_width(), SLIM_BAR_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::ZERO, t.card);
    p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width(), BAR_LINE)), CornerRadius::ZERO, t.line);
    let words = format!("dbee {}", env!("CARGO_PKG_VERSION"));
    let centre = pos2(bar.left() + INSET, bar.center().y + BAR_LINE / 2.0);
    Text::Label.paint(p, centre, egui::Align2::LEFT_CENTER, &words, t.text_muted);
    tags.tag_value("conn-version", &r, words);
}

/// The sidebar ([`SIDEBAR_W`], resizable as the workspace's): `+ NEW CONNECTION` in a bar row,
/// the filter, the connections file's warning, then one list of connections: the saved ones in
/// the file's order, then the recent ones that are not saved, newest first.
fn side(ui: &mut egui::Ui, st: &State, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = theme::tokens(ui.ctx());
    let panel = egui::Panel::left("connections-side").resizable(true).default_size(SIDEBAR_W).size_range(SIDEBAR_MIN_W..=SIDEBAR_MAX_W);
    panel.frame(Frame::new().fill(t.bg)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        let button = BarButton::label(Icon::Plus, "New connection").divider(false);
        let new = ui.put(Rect::from_min_size(row.min, vec2(button.width(ui), BAR_H)), button);
        ui.painter().rect_filled(Rect::from_min_max(pos2(row.left(), row.bottom() - BAR_LINE), row.max), CornerRadius::ZERO, t.line);
        tags.tag("conn-new", &new);
        if new.clicked() {
            acts.push(Act::New);
        }
        let filter = sidebar::Filter::new(&mut conn.filter, "Filter connections").id(egui::Id::new("conn-filter")).show(ui).field;
        tags.tag_value("conn-filter", &filter, conn.filter.as_str());
        if let Some(w) = &st.store_warning {
            // Wrapped, never cut: it says what happened to the user's file.
            let pad =
                Margin { left: SIDEBAR_PAD_X as i8, right: SIDEBAR_PAD_X as i8, top: CONN_WARNING_PAD_Y as i8, bottom: CONN_WARNING_PAD_Y as i8 };
            let shown = Frame::NONE.inner_margin(pad).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(Label::new(Text::Small.rich_as_written(w.as_str()).color(t.danger)).wrap())
            });
            let area = shown.response.rect;
            ui.painter().rect_filled(Rect::from_min_max(pos2(area.left(), area.bottom() - BAR_LINE), area.max), CornerRadius::ZERO, t.line);
            tags.tag_value("conn-store-warning", &shown.inner, w.as_str());
        }
        let q = conn.filter.trim().to_lowercase();
        let selected = conn.form.id.clone();
        let busy = conn.running.is_some();
        let saved = st.store.saved.iter().filter(|c| matches(c, &q)).map(|c| (format!("conn-saved-{}", c.id), c));
        // A recent entry keeps its index in the file's list as its id; a saved one shows as saved.
        let recent = st.store.recent.iter().enumerate().filter(|(_, r)| !st.store.is_saved(&r.config.id) && matches(&r.config, &q));
        let rows: Vec<(String, &ConnectionConfig)> = saved.chain(recent.map(|(n, r)| (format!("conn-recent-{n}"), &r.config))).collect();
        let any = !st.store.saved.is_empty() || st.store.recent.iter().any(|r| !st.store.is_saved(&r.config.id));
        ScrollArea::vertical().id_salt("conn-lists").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if rows.is_empty() && any {
                Phrase::new("No connection matches.", Place::Top).show(ui, ui.available_rect_before_wrap());
            }
            for (id, c) in rows {
                card(ui, tags, &id, c, c.id == selected, busy, acts);
            }
        });
    });
}

/// One connection's row (`sidebar/connection`): its colour on the left edge, the name and where
/// it goes.
fn card(ui: &mut egui::Ui, tags: &mut Tags, id: &str, c: &ConnectionConfig, selected: bool, busy: bool, acts: &mut Vec<Act>) {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), CONNECTION_ROW_H), Sense::click());
    let name = c.display_name();
    let look = Look::of(ui, &r);
    let row = ConnectionRow {
        name: &name,
        engine: c.kind,
        address: card_address(c),
        colour: connection_color(c.color),
        selected,
        focused: look.focused,
        hovered: look.hovered,
    };
    let r = match row.paint(ui, rect) {
        Some(whole) => r.hint(whole),
        None => r,
    };
    r.widget_info(|| row.widget_info());
    tags.tag_value(id, &r, name.as_str());
    tags.tag_value(&format!("{id}-kind"), &r, c.kind.label());
    if r.clicked() {
        acts.push(Act::Load(c.clone()));
    }
    if r.double_clicked() {
        acts.push(Act::Connect(c.clone()));
    }
    egui::Popup::context_menu(&r).style(menu::style(&r.ctx)).show(|ui| {
        let items = [
            ("Connect", "conn-menu-connect", false),
            ("Edit", "conn-menu-edit", false),
            ("Duplicate", "conn-menu-duplicate", false),
            ("Delete", "conn-menu-delete", true),
        ];
        for (label, item, destructive) in items {
            if destructive {
                menu::separator(ui);
            }
            let enabled = !(busy && item == "conn-menu-connect");
            let mut b = ui.add_enabled(enabled, Item::new(label).destructive(destructive));
            if !enabled {
                b = b.hint_disabled(BUSY_CONNECT);
            }
            // The menu's first frame is a sizing pass, which reads every widget as disabled.
            tags.tag_value_as(item, &b, label, enabled);
            if b.clicked() {
                acts.push(match item {
                    "conn-menu-connect" => Act::Connect(c.clone()),
                    "conn-menu-edit" => Act::Load(c.clone()),
                    "conn-menu-duplicate" => Act::Duplicate(c.id.clone()),
                    _ => Act::AskDelete { id: c.id.clone(), name: name.clone() },
                });
                ui.close();
            }
        }
    });
}

/// The right pane, everything right of the sidebar between the menu bar and the version bar, on
/// `card` (tagged `conn-card`): the title row, with the connection's colour as an edge along its
/// top; then the engine cards, or the engine row and the form (`saved`: a saved connection's).
fn main(
    ui: &mut egui::Ui,
    conn: &mut ConnScreen,
    shown: Option<(Running, Duration)>,
    kinds: &[DbKind],
    saved: bool,
    tags: &mut Tags,
    acts: &mut Vec<Act>,
) {
    let t = theme::tokens(ui.ctx());
    egui::CentralPanel::default().frame(Frame::new().fill(t.card)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let pane = ui.max_rect();
        tags.tag("conn-card", &ui.interact(pane, egui::Id::new("conn-card"), Sense::hover()));
        let url = !url_kinds(&conn.form, kinds).is_empty();
        conn_form::title_row(ui, conn, url, tags, acts);
        if let Some(color) = connection_color(conn.form.color) {
            let edge = Rect::from_min_size(pane.min, vec2(pane.width(), CONN_COLOUR_EDGE));
            ui.painter().rect_filled(edge, CornerRadius::ZERO, color);
        }
        match conn.step {
            Step::Engines => engines(ui, kinds, tags, acts),
            Step::Form => {
                conn_form::engine_row(ui, conn, tags, acts);
                conn_form::body(ui, conn, shown, saved, tags, acts);
            }
        }
    });
}

/// The engine cards (K2) under `Choose a database`: one per database type, as many in a row as
/// fit at [`ENGINE_CARD_MIN_W`] each, each row one box; scrolling when the window is short. Those
/// of `kinds` can be chosen.
fn engines(ui: &mut egui::Ui, kinds: &[DbKind], tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = theme::tokens(ui.ctx());
    ScrollArea::vertical().id_salt("conn-engines").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let inset = Margin { left: INSET as i8, right: INSET as i8, top: ENGINE_HEADING_TOP as i8, bottom: INSET as i8 };
        Frame::NONE.inner_margin(inset).show(ui, |ui| {
            let w = ui.available_width().max(1.0);
            ui.add(Label::new(Text::Subtitle.rich_as_written("Choose a database").color(t.text)).truncate());
            ui.add_space(ENGINE_CARDS_TOP);
            let per_row = ((w / ENGINE_CARD_MIN_W).floor() as usize).clamp(1, DbKind::ALL.len());
            let cell_w = w / per_row as f32;
            for (n, row) in DbKind::ALL.chunks(per_row).enumerate() {
                if n > 0 {
                    ui.add_space(ENGINE_CARDS_TOP);
                }
                let (line, _) = ui.allocate_exact_size(vec2(w, ENGINE_CARD_H), Sense::hover());
                let radius = CornerRadius::same(RADIUS_CONTROL);
                for (k, kind) in row.iter().enumerate() {
                    let cell = Rect::from_min_size(pos2(line.left() + cell_w * k as f32, line.top()), vec2(cell_w, ENGINE_CARD_H));
                    // The ground's corners follow the box's at the row's ends.
                    let corners = CornerRadius {
                        nw: if k == 0 { radius.nw } else { 0 },
                        sw: if k == 0 { radius.sw } else { 0 },
                        ne: if k + 1 == row.len() { radius.ne } else { 0 },
                        se: if k + 1 == row.len() { radius.se } else { 0 },
                    };
                    engine_card(ui, *kind, kinds.contains(kind), cell, corners, tags, acts);
                    if k > 0 {
                        let hairline = Rect::from_min_size(cell.min, vec2(BAR_LINE, cell.height()));
                        ui.painter().rect_filled(hairline, CornerRadius::ZERO, t.line);
                    }
                }
                let boxed = Rect::from_min_size(line.min, vec2(cell_w * row.len() as f32, ENGINE_CARD_H));
                ui.painter().rect_stroke(boxed, radius, Stroke::new(BAR_LINE, t.line_strong), egui::StrokeKind::Inside);
            }
        });
    });
}

/// The cell of the database type `kind` at `cell`: its logo 28 square 16 in and 16 down, its name
/// under it, and an UPCOMING chip in the top right corner when it is not `available` (this build
/// has no driver for it), greyed and not clickable.
fn engine_card(ui: &mut egui::Ui, kind: DbKind, available: bool, cell: Rect, corners: CornerRadius, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = theme::tokens(ui.ctx());
    let id = egui::Id::new(("conn-kind", kind.id()));
    let r = ui.interact(cell, id, if available { Sense::click() } else { Sense::hover() });
    let ground = if !available {
        None
    } else if r.is_pointer_button_down_on() {
        Some(t.selection)
    } else if r.hovered() {
        Some(t.hover)
    } else {
        None
    };
    let p = ui.painter();
    if let Some(ground) = ground {
        p.rect_filled(cell, corners, ground);
    }
    let color = if available { t.text } else { t.text_muted };
    let logo = Rect::from_min_size(cell.min + vec2(ENGINE_CARD_PAD, ENGINE_CARD_PAD), vec2(ENGINE_CARD_LOGO, ENGINE_CARD_LOGO));
    Logo::of(kind).paint(p, logo, color);
    let name = line_galley(ui, kind.label(), Text::Body, color, cell.width() - 2.0 * ENGINE_CARD_PAD);
    p.galley(pos2(cell.left() + ENGINE_CARD_PAD, logo.bottom() + ENGINE_CARD_NAME_GAP), name, color);
    if !available {
        let corner = egui::UiBuilder::new().max_rect(cell.shrink(ENGINE_CARD_CHIP_INSET)).layout(Layout::right_to_left(Align::Min));
        ui.new_child(corner).add(Chip::next("Upcoming"));
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, available, kind.label()));
    tags.tag_value_as(&format!("conn-kind-{}", kind.id()), &r, json!({ "label": kind.label(), "upcoming": !available }), available);
    if available && r.clicked() {
        acts.push(Act::ChooseKind(kind));
    }
}

/// A row's second line (its logo names the type): a server's `user@host:port/database` (no `/`
/// without a database), `  SSH` after it through a tunnel, as the page writes it; or a file's
/// path, with the home folder as `~`.
fn card_address(c: &ConnectionConfig) -> Address {
    match c.kind.info().settings {
        Settings::Server { .. } => {
            let ssh = if c.ssh.is_some() && c.kind.takes_ssh() { "  SSH" } else { "" };
            Address::Server(format!("{}@{}:{}{}{ssh}", c.user, c.host, c.port, c.database_suffix()))
        }
        Settings::File { .. } => Address::File(c.file.as_ref().map(|f| tilde(&f.display().to_string())).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use dbee_config::MemoryStore;
    use dbee_engine::{Session, SessionOptions};
    use dbee_testkit::{TempHome, temp_home};
    use egui::Key;

    use super::*;

    /// A context whose fonts are loaded, as the app's second frame has them.
    fn themed_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        ctx
    }

    fn app() -> (DbeeApp, TempHome) {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            ..SessionOptions::default()
        })
        .unwrap();
        let mut app = DbeeApp::new(session, None);
        app.themed_for_test();
        (app, home)
    }

    /// [`app`] with every driver dbee ships.
    fn app_with_drivers() -> (DbeeApp, TempHome) {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            drivers: dbee_drivers::all(),
            ..SessionOptions::default()
        })
        .unwrap();
        let mut app = DbeeApp::new(session, None);
        app.themed_for_test();
        (app, home)
    }

    /// Frames until `done` holds.
    fn until(app: &mut DbeeApp, ctx: &egui::Context, what: &str, done: impl Fn(&DbeeApp) -> bool) {
        let start = std::time::Instant::now();
        while !done(app) {
            assert!(start.elapsed() < std::time::Duration::from_secs(30), "gave up waiting for {what}");
            frame(app, ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The params of every `connection.open` in the session's journal, oldest first.
    fn opens(app: &DbeeApp) -> Vec<Json> {
        app.session.journal().iter().filter(|(id, _)| id == OPEN).map(|(_, params)| params.clone()).collect()
    }

    fn frame(app: &mut DbeeApp, ctx: &egui::Context) {
        frame_with(app, ctx, Vec::new());
    }

    /// A frame with `events` in its input.
    fn frame_with(app: &mut DbeeApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 900.0));
        let raw = egui::RawInput { screen_rect: Some(screen), events, ..egui::RawInput::default() };
        ctx.run_ui(raw, |ui| app.ui(ui)).drop_without_applying_deltas();
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// The form with the SSH tunnel on and a key line that says which key it uses.
    fn with_key_line(app: &mut DbeeApp) {
        let conn = &mut app.view.conn;
        conn.step = Step::Form;
        conn.form.ssh_on = true;
        conn.key = KeyLine { for_auth: Some(conn.form.ssh_auth()), text: "Uses /k/id_ed25519".into(), path: None, pending: None };
    }

    #[test]
    fn the_test_line_says_how_only_for_a_type_with_a_tunnel() {
        let line = |v: serde_json::Value| test_text(&v);
        assert_eq!(
            line(serde_json::json!({ "server": "PostgreSQL 14.12", "via": "direct", "warnings": [] })),
            "Test passed: PostgreSQL 14.12, direct."
        );
        assert_eq!(line(serde_json::json!({ "server": "SQLite 3.46.0", "via": null, "warnings": [] })), "Test passed: SQLite 3.46.0.");
        assert_eq!(line(serde_json::json!({})), "Test passed: the database.");
    }

    #[test]
    fn a_quick_empty_answer_keeps_the_key_line_and_says_so_neutrally() {
        let (mut app, _home) = app();
        let ctx = themed_ctx();
        with_key_line(&mut app);
        let (tx, picker) = Picker::for_test(Pick::Key, app.view.conn.generation);
        app.view.conn.picker = Some(picker);
        tx.send(None).unwrap();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(app.view.conn.picker.is_none());
        assert_eq!(app.tags().value("conn-ssh-key"), Some(&Json::from("Uses /k/id_ed25519")));
        assert_eq!(app.tags().value("conn-error"), None, "nothing failed");
        if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
            let note = app.tags().value("conn-note").and_then(Json::as_str).unwrap_or_default().to_string();
            assert!(note.starts_with("No file chosen.") && note.contains("xdg-desktop-portal"), "{note}");
        }
    }

    #[test]
    fn a_stopped_picker_still_lands_its_choice_and_only_one_opens() {
        let (mut app, _home) = app();
        let ctx = themed_ctx();
        with_key_line(&mut app);
        let (tx, picker) = Picker::for_test(Pick::Key, app.view.conn.generation);
        app.view.conn.picker = Some(picker);
        apply(&mut app, Act::StopPicking, &ctx);
        assert!(app.view.conn.picker.as_ref().is_some_and(|p| p.stopped), "Stop keeps the picker's answer coming");
        // While it is open, another picker does not start.
        apply(&mut app, Act::Pick(Pick::Ca), &ctx);
        assert_eq!(app.view.conn.picker.as_ref().map(|p| p.target), Some(Pick::Key));
        let note = app.view.conn.message.clone().unwrap();
        assert!(note.kind == Kind::Note && note.text.contains("already open"), "{note:?}");
        // A late choice still lands.
        tx.send(Some(PathBuf::from("/k/other"))).unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.view.conn.form.ssh_key, Some(PathBuf::from("/k/other")));
        assert!(app.view.conn.picker.is_none());
        // A choice for a connection the form no longer shows is dropped.
        let (tx, picker) = Picker::for_test(Pick::Ca, app.view.conn.generation);
        app.view.conn.picker = Some(picker);
        apply(&mut app, Act::New, &ctx);
        tx.send(Some(PathBuf::from("/k/ca.pem"))).unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.view.conn.form.ca_file, None);
        assert!(app.view.conn.picker.is_none());
    }

    #[test]
    fn new_file_is_asked_for_until_a_connect_succeeds() {
        let (mut app, home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::Sqlite), &ctx);
        let file = home.home.join("new.sqlite");
        let pick = |app: &mut DbeeApp, target: Pick| {
            let (tx, picker) = Picker::for_test(target, app.view.conn.generation);
            app.view.conn.picker = Some(picker);
            tx.send(Some(file.clone())).unwrap();
            frame(app, &ctx);
            assert!(app.view.conn.picker.is_none());
        };
        let asks = |app: &DbeeApp| app.view.conn.form.create_file;
        // CHOOSE takes a file as it is; NEW FILE asks for it to be made; another type forgets it.
        pick(&mut app, Pick::NewFile);
        pick(&mut app, Pick::File);
        assert_eq!((app.view.conn.form.file.as_deref(), asks(&app)), (Some(file.as_path()), false));
        pick(&mut app, Pick::NewFile);
        assert!(asks(&app));
        apply(&mut app, Act::ChangeEngine, &ctx);
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        apply(&mut app, Act::ChangeEngine, &ctx);
        apply(&mut app, Act::ChooseKind(DbKind::Sqlite), &ctx);
        assert!(!asks(&app) && app.view.conn.form.file.is_some());
        pick(&mut app, Pick::NewFile);
        // Saving makes no file and keeps the ask.
        apply(&mut app, Act::Save, &ctx);
        assert!(asks(&app) && !file.exists(), "{:?}", app.view.conn.message);
        // Read-only: the engine refuses to make a file, and the form still asks.
        app.view.conn.form.read_only = true;
        apply(&mut app, Act::ConnectForm, &ctx);
        let refused = app.view.conn.message.clone().unwrap();
        assert_eq!(
            (refused.kind, refused.text.as_str()),
            (Kind::Error, "A read-only connection can't make a new file. Untick Read-only for the first connect.")
        );
        assert!(asks(&app) && !file.exists());
        // A test makes the file and still asks; the connect asks again, and that ends it.
        app.view.conn.form.read_only = false;
        apply(&mut app, Act::Test, &ctx);
        until(&mut app, &ctx, "the test", |app| app.view.conn.running.is_none());
        assert_eq!(app.view.conn.message.as_ref().map(|m| m.kind), Some(Kind::Result), "{:?}", app.view.conn.message);
        assert!(file.is_file() && asks(&app));
        apply(&mut app, Act::ConnectForm, &ctx);
        until(&mut app, &ctx, "the workspace", |app| app.session.state().screen == dbee_engine::Screen::Workspace);
        assert!(!asks(&app));
        assert_eq!(opens(&app).len(), 1);
        assert_eq!(opens(&app)[0]["createFile"], true, "{:?}", opens(&app));
        // The next connect opens the file as it is.
        app.run("connection.close", json!({})).unwrap();
        apply(&mut app, Act::ConnectForm, &ctx);
        until(&mut app, &ctx, "the workspace again", |app| app.session.state().screen == dbee_engine::Screen::Workspace);
        assert_eq!(opens(&app).len(), 2);
        assert!(opens(&app)[1].get("createFile").is_none(), "{:?}", opens(&app));
    }

    /// A listing started now, as job `n`, with the form's params; returns them and the generation.
    fn listing(app: &mut DbeeApp, n: u64) -> (Json, u64) {
        let params = json!({ "config": app.view.conn.form.to_config().unwrap() });
        started(app, dbee_engine::JobId(n), LIST_DATABASES);
        (params, app.view.conn.generation)
    }

    fn listed(names: &[&str]) -> Result<Json, EngineError> {
        Ok(json!({ "databases": names }))
    }

    #[test]
    fn a_late_database_list_is_dropped_when_the_form_changed() {
        let (mut app, _home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        let job = |n| Some(dbee_engine::JobId(n));
        // The form as it was at the click: the list opens.
        let (params, generation) = listing(&mut app, 1);
        assert_eq!(app.view.conn.databases.pending, job(1));
        form_done(&mut app, LIST_DATABASES, params, generation, job(1), listed(&["dbee", "shop"]));
        let list = &app.view.conn.databases;
        assert!(list.open && list.pending.is_none() && list.names == ["dbee", "shop"], "{list:?}");
        // Another host, port, user, TLS mode, CA file or tunnel since the click: dropped, errors too.
        let changes: [fn(&mut Form); 6] = [
            |f| f.host = "elsewhere".into(),
            |f| f.port = "6543".into(),
            |f| f.user = "ann".into(),
            |f| f.ssl_mode = SslMode::Require,
            |f| f.ca_file = Some(PathBuf::from("/x/ca.pem")),
            |f| {
                f.ssh_on = true;
                f.ssh_host = "jump".into();
                f.ssh_user = "ann".into();
            },
        ];
        for (n, change) in (2..).zip(changes) {
            apply(&mut app, Act::New, &ctx);
            apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
            let (params, generation) = listing(&mut app, n);
            change(&mut app.view.conn.form);
            form_done(&mut app, LIST_DATABASES, params.clone(), generation, job(n), listed(&["dbee"]));
            assert!(!app.view.conn.databases.open && app.view.conn.databases.pending.is_none(), "change {n}");
            let refused = Err(EngineError::Db { code: "connect", message: "refused".into(), data: json!({}) });
            started(&mut app, dbee_engine::JobId(100 + n), LIST_DATABASES);
            form_done(&mut app, LIST_DATABASES, params, generation, job(100 + n), refused);
            assert_eq!(app.view.conn.message, None, "change {n}");
        }
        // Another type, or Change engine (back to the same type): dropped too.
        apply(&mut app, Act::New, &ctx);
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        let (params, generation) = listing(&mut app, 20);
        apply(&mut app, Act::ChangeEngine, &ctx);
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        assert!(app.view.conn.databases.pending.is_none(), "Change engine forgets the listing");
        form_done(&mut app, LIST_DATABASES, params, generation, job(20), listed(&["dbee"]));
        assert!(!app.view.conn.databases.open);
        let (params, generation) = listing(&mut app, 21);
        apply(&mut app, Act::ChangeEngine, &ctx);
        apply(&mut app, Act::ChooseKind(DbKind::MySql), &ctx);
        assert_eq!(app.view.conn.form.kind, DbKind::MySql);
        form_done(&mut app, LIST_DATABASES, params, generation, job(21), listed(&["dbee"]));
        assert!(!app.view.conn.databases.open && app.view.conn.databases.pending.is_none());
        let (params, generation) = listing(&mut app, 23);
        fill_from_url(&mut app, &json!({ "kind": "mariadb" }), None);
        form_done(&mut app, LIST_DATABASES, params, generation, job(23), listed(&["dbee"]));
        assert!(!app.view.conn.databases.open && app.view.conn.databases.pending.is_none(), "a URL of another type");
        // The type's own options count too.
        let (params, generation) = listing(&mut app, 24);
        app.view.conn.form.options.insert(dbee_config::ALLOW_PUBLIC_KEY.id.into(), "true".into());
        form_done(&mut app, LIST_DATABASES, params, generation, job(24), listed(&["dbee"]));
        assert!(!app.view.conn.databases.open, "another option");
        // Typing a database or a password since the click is no change of server.
        let (params, generation) = listing(&mut app, 22);
        app.view.conn.form.database = "sh".into();
        app.view.conn.form.password = "typed".into();
        form_done(&mut app, LIST_DATABASES, params, generation, job(22), listed(&["shop"]));
        assert!(app.view.conn.databases.open);
    }

    #[test]
    fn a_server_that_lists_nothing_says_so() {
        let (mut app, _home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::MySql), &ctx);
        let (params, generation) = listing(&mut app, 1);
        form_done(&mut app, LIST_DATABASES, params, generation, Some(dbee_engine::JobId(1)), listed(&[]));
        assert!(!app.view.conn.databases.open);
        assert_eq!(app.view.conn.message, Some(Message::note("No databases to list.")));
        // A choice fills the field and closes the list.
        let (params, generation) = listing(&mut app, 2);
        form_done(&mut app, LIST_DATABASES, params, generation, Some(dbee_engine::JobId(2)), listed(&["shop"]));
        assert!(app.view.conn.databases.open);
        apply(&mut app, Act::ChooseDatabase("shop".into()), &ctx);
        assert_eq!((app.view.conn.form.database.as_str(), app.view.conn.databases.open), ("shop", false));
    }

    /// A MySQL form for `127.0.0.1:1`, which refuses at once, whose list holds `names`, open as a
    /// listing's result opens it.
    fn open_list(names: &[String]) -> (DbeeApp, TempHome, egui::Context) {
        let (mut app, home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::MySql), &ctx);
        app.view.conn.form.host = "127.0.0.1".into();
        app.view.conn.form.port = "1".into();
        let (params, generation) = listing(&mut app, 1);
        form_done(&mut app, LIST_DATABASES, params, generation, Some(dbee_engine::JobId(1)), Ok(json!({ "databases": names })));
        assert!(app.view.conn.databases.open);
        (app, home, ctx)
    }

    /// The items of the list drawn in the last frame: their indexes.
    fn drawn_items(app: &DbeeApp, n: usize) -> Vec<usize> {
        (0..n).filter(|i| app.tags().rect(&format!("conn-database-item-{i}")).is_some()).collect()
    }

    #[test]
    fn many_names_scroll_in_a_list_of_bounded_height() {
        let names: Vec<String> = (0..500).map(|n| format!("db{n:03}")).collect();
        let (mut app, _home, ctx) = open_list(&names);
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        let drawn: Vec<Rect> = drawn_items(&app, 500).iter().filter_map(|i| app.tags().rect(&format!("conn-database-item-{i}"))).collect();
        assert!(app.tags().value("conn-database-item-0") == Some(&Json::from("db000")) && drawn.len() < 20, "{} drawn", drawn.len());
        let (top, bottom) = (drawn[0].top(), drawn.iter().map(|r| r.bottom()).fold(0.0, f32::max));
        assert!(bottom - top <= 260.0, "{top} … {bottom}");
        // Down moves the highlight, and the list scrolls to keep it in view; Enter picks it.
        frame_with(&mut app, &ctx, vec![key(Key::ArrowDown); 40]);
        frame(&mut app, &ctx);
        assert_eq!(app.view.conn.databases.highlight, 40);
        assert!(drawn_items(&app, 500).contains(&40) && !drawn_items(&app, 500).contains(&0), "{:?}", drawn_items(&app, 500));
        frame_with(&mut app, &ctx, vec![key(Key::ArrowUp); 3]);
        frame_with(&mut app, &ctx, vec![key(Key::Enter)]);
        assert_eq!((app.view.conn.form.database.as_str(), app.view.conn.databases.open), ("db037", false));
        assert_eq!(app.session.journal().len(), 0, "Enter picked a name and connected nowhere");
    }

    /// The list keeps the keys only while the keyboard is on the Database field, on the list's
    /// button or nowhere (a click on the button leaves no widget focused).
    #[test]
    fn the_keyboard_elsewhere_closes_the_list() {
        let names = ["many".to_string(), "shop".to_string()];
        for elsewhere in ["conn-host", "conn-port", "conn-user", "conn-password", "conn-name"] {
            let (mut app, _home, ctx) = open_list(&names);
            frame(&mut app, &ctx);
            ctx.memory_mut(|m| m.request_focus(conn_form::field_id(elsewhere)));
            frame_with(&mut app, &ctx, vec![key(Key::ArrowDown)]);
            let list = &app.view.conn.databases;
            assert!(!list.open && list.highlight == 0 && drawn_items(&app, 2).is_empty(), "{elsewhere}");
        }
        let (mut app, _home, ctx) = open_list(&names);
        frame(&mut app, &ctx);
        let button = app.view.conn.databases.button.expect("the button was drawn");
        let field = conn_form::field_id("conn-database");
        assert_eq!(ctx.memory(|m| m.focused()), Some(field), "the field takes the keyboard when the list opens");
        for on in [None, Some(field), Some(button)] {
            ctx.memory_mut(|m| match on {
                Some(id) => m.request_focus(id),
                None => m.surrender_focus(field),
            });
            frame_with(&mut app, &ctx, vec![key(Key::ArrowDown)]);
            assert!(app.view.conn.databases.open && app.view.conn.databases.highlight == 1, "{on:?}");
            frame_with(&mut app, &ctx, vec![key(Key::ArrowUp)]);
        }
        // Typing in the Database field closes it, and the text stays.
        ctx.memory_mut(|m| m.request_focus(conn_form::field_id("conn-database")));
        frame_with(&mut app, &ctx, vec![egui::Event::Text("sh".into())]);
        frame(&mut app, &ctx);
        assert!(!app.view.conn.databases.open && drawn_items(&app, 2).is_empty());
        assert_eq!(app.view.conn.form.database, "sh");
    }

    /// A dialog over the form closes the list, which then takes none of its keys.
    #[test]
    fn a_dialog_keeps_its_keys_from_the_list() {
        let names = ["many".to_string(), "shop".to_string()];
        let (mut app, _home, ctx) = open_list(&names);
        frame(&mut app, &ctx);
        let rerun = dialogs::Rerun { command: TEST, params: json!({}), origin: Origin::Form(app.view.conn.generation) };
        app.view.dialog = Some(Dialog::Confirm { text: "Go on?".into(), what: String::new(), rerun });
        frame_with(&mut app, &ctx, vec![key(Key::ArrowDown)]);
        assert!(!app.view.conn.databases.open && app.view.conn.databases.highlight == 0, "the list closed and took no key");
        frame_with(&mut app, &ctx, vec![key(Key::Escape)]);
        assert!(app.view.dialog.is_none(), "Escape reached the dialog");
        assert_eq!(app.view.conn.form.database, "");
    }

    #[test]
    fn the_highlight_starts_on_the_typed_name_and_escape_closes() {
        let names: Vec<String> = ["many", "odd", "shop", "sys"].map(String::from).to_vec();
        let (mut app, _home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::MySql), &ctx);
        app.view.conn.form.database = "shop".into();
        let (params, generation) = listing(&mut app, 1);
        form_done(&mut app, LIST_DATABASES, params, generation, Some(dbee_engine::JobId(1)), Ok(json!({ "databases": names })));
        assert_eq!(app.view.conn.databases.highlight, 2);
        // At the ends the highlight stays.
        frame_with(&mut app, &ctx, vec![key(Key::ArrowDown); 5]);
        assert_eq!(app.view.conn.databases.highlight, 3);
        frame_with(&mut app, &ctx, vec![key(Key::ArrowUp); 9]);
        assert_eq!(app.view.conn.databases.highlight, 0);
        frame_with(&mut app, &ctx, vec![key(Key::Escape)]);
        assert!(!app.view.conn.databases.open);
        assert_eq!(app.view.conn.form.database, "shop", "nothing was chosen");
    }

    /// A listing that lands after a connect succeeded (here an agent's, not the form's) never
    /// opens, and the workspace lets an open list go.
    #[test]
    fn a_list_that_lands_after_a_connect_never_opens() {
        let (mut app, _home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        let (params, generation) = listing(&mut app, 1);
        let seed = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
        let file = ConnectionConfig {
            kind: DbKind::Sqlite,
            file: seed.file().map(PathBuf::from),
            host: String::new(),
            port: 0,
            ..ConnectionConfig::default()
        };
        app.session.run_blocking("connection.open", json!({ "config": file }), std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(app.session.state().screen, dbee_engine::Screen::Workspace);
        form_done(&mut app, LIST_DATABASES, params, generation, Some(dbee_engine::JobId(1)), listed(&["dbee"]));
        assert!(!app.view.conn.databases.open && app.view.conn.databases.pending.is_none());
        // A list open when the workspace showed is gone when the connection screen comes back.
        app.view.conn.databases.open = true;
        frame(&mut app, &ctx);
        app.session.run_blocking("connection.close", json!({ "force": true }), std::time::Duration::from_secs(30)).unwrap();
        frame(&mut app, &ctx);
        assert!(!app.view.conn.databases.open && app.tags().rect("conn-database-item-0").is_none());
    }

    #[test]
    fn a_cards_line_leaves_out_an_unset_database() {
        let server =
            ConnectionConfig { kind: DbKind::MariaDb, user: "root".into(), host: "127.0.0.1".into(), port: 32821, ..ConnectionConfig::default() };
        // The address or path alone, as written: the row's logo names the engine.
        assert_eq!(card_address(&server), Address::Server("root@127.0.0.1:32821".into()));
        let shop = ConnectionConfig { database: "shop".into(), ..server.clone() };
        assert_eq!(card_address(&shop), Address::Server("root@127.0.0.1:32821/shop".into()));
        let file = ConnectionConfig { kind: DbKind::Sqlite, file: Some(PathBuf::from("/data/shop.sqlite")), ..ConnectionConfig::default() };
        assert_eq!(card_address(&file), Address::File("/data/shop.sqlite".into()));
        // Through a tunnel, as the page writes it.
        let tunnel = ConnectionConfig { ssh: Some(Default::default()), ..shop };
        assert_eq!(card_address(&tunnel), Address::Server("root@127.0.0.1:32821/shop  SSH".into()));
        // A file in the home folder, with `~`.
        if let Some(home) = dbee_config::home_dir().filter(|h| h.to_str().is_some_and(|h| !h.is_empty())) {
            let notes = ConnectionConfig { file: Some(home.join("data").join("notes.sqlite")), ..file };
            let at = format!("~{}data{}notes.sqlite", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR);
            assert_eq!(card_address(&notes), Address::File(at));
        }
    }

    #[test]
    fn a_saved_type_is_fixed_only_where_the_form_says_so() {
        let (mut app, _home) = app_with_drivers();
        let ctx = themed_ctx();
        apply(&mut app, Act::ChooseKind(DbKind::MySql), &ctx);
        // Neither act moves a fixed type, whatever sends it.
        app.view.conn.form.engine_fixed = true;
        apply(&mut app, Act::ChangeEngine, &ctx);
        assert_eq!(app.view.conn.step, Step::Form);
        app.view.conn.step = Step::Engines;
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        assert_eq!(app.view.conn.form.kind, DbKind::MySql);
        // Another type forgets what showed under the buttons and the run it came from.
        app.view.conn.form.engine_fixed = false;
        app.view.conn.message = Some(Message::error(OPEN, "MySQL said no"));
        let generation = app.view.conn.generation;
        apply(&mut app, Act::ChooseKind(DbKind::Postgres), &ctx);
        assert_eq!((app.view.conn.form.kind, &app.view.conn.message), (DbKind::Postgres, &None));
        assert_ne!(app.view.conn.generation, generation);
        // While a test or connect runs, the type stays.
        app.view.conn.running = Some((dbee_engine::JobId(1), Running::Test));
        apply(&mut app, Act::ChangeEngine, &ctx);
        assert_eq!(app.view.conn.step, Step::Form);
    }
}

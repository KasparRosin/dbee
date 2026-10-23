//! The connection screen: the saved and recent connections on the left, the connection form on
//! the right ([`screen`]).
//!
//! - Left: "New connection", a filter, then the SAVED and RECENT cards, each with its colour. A
//!   single click loads a connection into the form, a double-click connects; right-click offers
//!   Connect · Edit · Duplicate · Delete (Delete asks first: it deletes the saved passwords too).
//! - Right: the form, one page: name and colour, host and port, user and password ("Save
//!   passwords", off and disabled when there is no keyring), database, SSL mode and CA file,
//!   read-only, the SSH tunnel (host, port, user, then Key · Agent · Password, with a line saying
//!   what the login will use, from `connection.sshKey`), then Save · Test · Connect, and under
//!   them a reserved slot for the error or the test result. Every row has a fixed height, so
//!   typing, a message or another login choice never moves the form; only turning the SSH
//!   tunnel on or off does. In a short window only the fields scroll: the title row, the buttons
//!   and the message slot stay in view.
//! - Test and Connect run as jobs (`connection.test`, `connection.open`); while one runs both
//!   buttons are disabled and say so, and a double-click on a card does nothing. A `needs…`
//!   answer opens a dialog ([`crate::dialogs`]) that runs the command again; other errors show
//!   under the buttons with the server's message.
//! - The file pickers ("Change…" for the key, "Choose…" for the CA file) run rfd on a thread of
//!   their own and send the path back; a picker that cannot open leaves a note in its row.
//!
//! Secrets: the typed password lives only in the form (and in the params of the command it goes
//! to). `ui.inspect` reports the field as masked with whether it is set; `Debug` and the logs
//! never show it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dbee_config::{ConnectionColor, ConnectionConfig, SshAuth, SshConfig, SslMode};
use dbee_engine::{EngineError, JobId, Json, State};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Button, Checkbox, Color32, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit,
    TextFormat, UiBuilder, pos2, vec2,
};
use serde_json::json;

use crate::app::Origin;
use crate::automation::Tags;
use crate::dialogs::{self, Dialog};
use crate::theme::{RADIUS, TOKENS, connection_color};
use crate::{DbeeApp, icons};

pub(crate) const OPEN: &str = "connection.open";
pub(crate) const TEST: &str = "connection.test";
pub(crate) const SAVE: &str = "connection.save";
pub(crate) const PARSE_URL: &str = "connection.parseUrl";
const SSH_KEY: &str = "connection.sshKey";
const DELETE: &str = "connection.delete";
const DUPLICATE: &str = "connection.duplicate";

/// The form's width when the window allows it.
const FORM_WIDTH: f32 = 600.0;
/// The height of a label above a field.
const LABEL_H: f32 = 16.0;
/// The height of a field.
const FIELD_H: f32 = 28.0;
/// A field with its label.
const ROW_H: f32 = LABEL_H + FIELD_H;
/// The slot under the buttons for an error or a test result.
const MESSAGE_H: f32 = 52.0;
/// A card in the lists.
const CARD_H: f32 = 46.0;
/// A picker that answers "nothing" faster than this did not show (no portal, no zenity).
const PICKER_GAVE_UP: Duration = Duration::from_millis(800);

/// The SSH login choices of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    Key,
    Agent,
    Password,
}

/// The connection form's fields, as typed.
#[derive(Clone, PartialEq)]
pub(crate) struct Form {
    /// The saved connection this is, or empty for a new one.
    pub id: String,
    pub name: String,
    pub color: ConnectionColor,
    pub host: String,
    pub port: String,
    pub user: String,
    /// Typed by the user; never loaded from the keyring.
    pub password: String,
    pub show_password: bool,
    pub database: String,
    pub ssl_mode: SslMode,
    pub ca_file: Option<PathBuf>,
    pub read_only: bool,
    pub save_secrets: bool,
    pub ssh_on: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_user: String,
    pub ssh_auth: Auth,
    /// The key file chosen with "Change…"; `None` picks automatically.
    pub ssh_key: Option<PathBuf>,
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &if self.password.is_empty() { "" } else { "<redacted>" })
            .field("database", &self.database)
            .field("ssh_on", &self.ssh_on)
            .finish_non_exhaustive()
    }
}

/// A port as typed: a number from 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

impl Form {
    /// An empty form for a new connection.
    pub(crate) fn new(save_secrets: bool) -> Form {
        Form::from_config(&ConnectionConfig { save_secrets, ..ConnectionConfig::default() })
    }

    /// The form showing `c` (no password: stored ones stay in the keyring).
    pub(crate) fn from_config(c: &ConnectionConfig) -> Form {
        let ssh = c.ssh.clone().unwrap_or_default();
        let (ssh_auth, ssh_key) = match &ssh.auth {
            SshAuth::Key { path } => (Auth::Key, path.clone()),
            SshAuth::Agent => (Auth::Agent, None),
            SshAuth::Password => (Auth::Password, None),
        };
        Form {
            id: c.id.clone(),
            name: c.name.clone(),
            color: c.color,
            host: c.host.clone(),
            port: c.port.to_string(),
            user: c.user.clone(),
            password: String::new(),
            show_password: false,
            database: c.database.clone(),
            ssl_mode: c.ssl_mode,
            ca_file: c.ca_file.clone(),
            read_only: c.read_only,
            save_secrets: c.save_secrets,
            ssh_on: c.ssh.is_some(),
            ssh_host: ssh.host,
            ssh_port: ssh.port.to_string(),
            ssh_user: ssh.user,
            ssh_auth,
            ssh_key,
        }
    }

    /// The SSH login as a config stores it.
    pub(crate) fn ssh_auth(&self) -> SshAuth {
        match self.ssh_auth {
            Auth::Key => SshAuth::Key { path: self.ssh_key.clone() },
            Auth::Agent => SshAuth::Agent,
            Auth::Password => SshAuth::Password,
        }
    }

    /// The connection the form describes, or why it is not usable yet.
    pub(crate) fn to_config(&self) -> Result<ConnectionConfig, String> {
        let port = parse_port(&self.port).ok_or("The port must be a number from 1 to 65535.")?;
        let ssh = if self.ssh_on {
            let port = parse_port(&self.ssh_port).ok_or("The SSH port must be a number from 1 to 65535.")?;
            Some(SshConfig { host: self.ssh_host.trim().into(), port, user: self.ssh_user.trim().into(), auth: self.ssh_auth() })
        } else {
            None
        };
        let config = ConnectionConfig {
            id: self.id.clone(),
            name: self.name.trim().into(),
            color: self.color,
            host: self.host.trim().into(),
            port,
            user: self.user.trim().into(),
            database: self.database.trim().into(),
            ssl_mode: self.ssl_mode,
            ca_file: self.ca_file.clone(),
            ssh,
            read_only: self.read_only,
            save_secrets: self.save_secrets,
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    /// What the title line says.
    fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.id.is_empty() {
            "New connection".into()
        } else {
            format!("{}@{}/{}", self.user, self.host, self.database)
        }
    }
}

/// Which form job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    Test,
    Connect,
}

/// What the slot under the buttons shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub error: bool,
    pub text: String,
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

/// Which field a file picker fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    Key,
    Ca,
}

/// A file picker running on its own thread.
#[derive(Debug)]
pub(crate) struct Picker {
    pub target: Pick,
    rx: Receiver<Option<PathBuf>>,
    started: Instant,
}

/// Notes the file pickers leave in their rows.
#[derive(Debug, Default)]
pub(crate) struct Notes {
    pub key: Option<String>,
    pub ca: Option<String>,
}

/// What only the connection screen keeps.
#[derive(Debug)]
pub(crate) struct ConnScreen {
    pub form: Form,
    /// Counts the connections the form has shown: a result for an earlier one is not shown.
    pub generation: u64,
    pub filter: String,
    /// The test or connect started from the form, until its result arrives.
    pub running: Option<(JobId, Running)>,
    pub message: Option<Message>,
    pub key: KeyLine,
    pub picker: Option<Picker>,
    pub notes: Notes,
    /// The session has a secret store (`document.inspect`'s `secretStore`).
    pub has_store: bool,
}

impl ConnScreen {
    pub(crate) fn new(has_store: bool) -> ConnScreen {
        ConnScreen {
            form: Form::new(has_store),
            generation: 0,
            filter: String::new(),
            running: None,
            message: None,
            key: KeyLine::default(),
            picker: None,
            notes: Notes::default(),
            has_store,
        }
    }

    /// Shows a new, empty connection.
    fn new_connection(&mut self) {
        self.form = Form::new(self.has_store);
        self.forget_shown();
    }

    /// The form shows another connection: its message, notes and key line are gone.
    fn forget_shown(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.message = None;
        self.notes = Notes::default();
        self.key.for_auth = None;
    }

    /// Loads `c` into the form. The same connection again keeps the typed password and what the
    /// form shows under its buttons.
    fn load(&mut self, c: &ConnectionConfig) {
        let same = !c.id.is_empty() && c.id == self.form.id;
        let password = if same { std::mem::take(&mut self.form.password) } else { String::new() };
        self.form = Form::from_config(c);
        self.form.password = password;
        if same {
            self.key.for_auth = None;
        } else {
            self.forget_shown();
        }
    }
}

/// `path` with the home directory as `~`.
pub(crate) fn tilde(path: &str) -> String {
    if let Some(home) = dbee_config::home_dir()
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

/// A form job started (`app.start` with [`Origin::Form`]): Test and Connect now show it runs.
pub(crate) fn started(app: &mut DbeeApp, job: JobId, command: &str) {
    let kind = match command {
        TEST => Running::Test,
        OPEN => Running::Connect,
        _ => return,
    };
    app.view.conn.running = Some((job, kind));
}

/// The outcome of a form command (`job` when it ran as one). A result for a connection the
/// form no longer shows is dropped.
pub(crate) fn form_done(
    app: &mut DbeeApp,
    command: &'static str,
    params: Json,
    generation: u64,
    job: Option<JobId>,
    result: Result<Json, EngineError>,
) {
    let conn = &mut app.view.conn;
    if let (Some((running, _)), Some(job)) = (conn.running, job)
        && running == job
    {
        conn.running = None;
    }
    if generation != conn.generation {
        return;
    }
    match result {
        Ok(v) => match command {
            SAVE => {
                if let Some(id) = v.get("id").and_then(Json::as_str) {
                    conn.form.id = id.to_string();
                }
                conn.message = Some(Message { error: false, text: "Saved.".into() });
            }
            TEST => conn.message = Some(Message { error: false, text: test_text(&v) }),
            OPEN => {
                conn.message = None;
                if let Some(note) = v.get("secretsNote").and_then(Json::as_str) {
                    app.notice(format!("The password was not saved: {note}"));
                }
            }
            _ => {}
        },
        Err(e) => {
            let rerun = dialogs::Rerun { command, params, origin: Origin::Form(generation) };
            if !dialogs::ask(app, &e, rerun) {
                app.view.conn.message = Some(Message { error: true, text: error_text(&e) });
            }
        }
    }
}

/// `{server, via, warnings}` of a test, as one line.
fn test_text(v: &Json) -> String {
    let server = v.get("server").and_then(Json::as_str).unwrap_or("the server");
    let via = v.get("via").and_then(Json::as_str).unwrap_or("direct");
    let mut text = format!("Test passed: {server}, {}.", tilde_in(via));
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

/// Fills the form from `connection.parseUrl`'s result: the parts the URL has.
pub(crate) fn fill_from_url(app: &mut DbeeApp, parts: &Json) {
    let form = &mut app.view.conn.form;
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
    if let Some(v) = text("password") {
        form.password = v;
    }
    if let Some(v) = text("database") {
        form.database = v;
    }
    if let Some(mode) = parts.get("sslMode").filter(|m| !m.is_null()).and_then(|m| serde_json::from_value::<SslMode>(m.clone()).ok()) {
        form.ssl_mode = mode;
    }
    app.view.conn.message = None;
}

/// Deletes the saved connection `id` (the user confirmed); the form empties when it showed it.
pub(crate) fn delete(app: &mut DbeeApp, id: &str) {
    match app.run(DELETE, json!({ "id": id })) {
        Ok(_) => {
            if app.view.conn.form.id == id {
                app.view.conn.new_connection();
            }
        }
        Err(e) => app.notice(e.to_string()),
    }
}

// ---- each frame ------------------------------------------------------------------------------

/// What the user did on the screen this frame, applied once it is drawn.
enum Act {
    New,
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
    Save,
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
    let mut acts = Vec::new();
    {
        let DbeeApp { session, view, tags, .. } = app;
        side(ui, session.state(), &mut view.conn, tags, &mut acts);
        main(ui, &mut view.conn, tags, &mut acts);
    }
    for act in acts {
        apply(app, act, &ctx);
    }
}

/// Before drawing: the picker's answer, and the key line's question when the login changed.
fn tick(app: &mut DbeeApp, ctx: &egui::Context) {
    poll_picker(&mut app.view.conn, ctx);
    let conn = &mut app.view.conn;
    if !conn.form.ssh_on {
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
    let ssh = SshConfig { host: conn.form.ssh_host.clone(), port: 22, user: conn.form.ssh_user.clone(), auth: want.clone() };
    let params = json!({ "config": { "ssh": ssh } });
    match app.start(SSH_KEY, params, Origin::KeyLine) {
        Some(job) => app.view.conn.key.pending = Some((job, want)),
        // Failed at once; the line says why, and it is not asked again until the login changes.
        None => app.view.conn.key.for_auth = Some(want),
    }
}

fn poll_picker(conn: &mut ConnScreen, ctx: &egui::Context) {
    let Some(p) = &conn.picker else { return };
    let target = p.target;
    let answer = p.rx.try_recv();
    let quick = p.started.elapsed() < PICKER_GAVE_UP;
    let note = match answer {
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
        Ok(Some(path)) => {
            match target {
                Pick::Key => {
                    conn.form.ssh_key = Some(path);
                    conn.form.ssh_auth = Auth::Key;
                }
                Pick::Ca => conn.form.ca_file = Some(path),
            }
            None
        }
        // Linux: rfd answers "nothing" at once when neither the desktop portal nor zenity could
        // show a picker; a person takes longer than that to cancel one.
        Ok(None) if quick && cfg!(not(any(target_os = "macos", target_os = "windows"))) => {
            Some("No file picker opened: it needs the desktop portal (xdg-desktop-portal) or zenity.".to_string())
        }
        Ok(None) => None,
        Err(TryRecvError::Disconnected) => Some("The file picker stopped without an answer.".to_string()),
    };
    conn.picker = None;
    match target {
        Pick::Key => conn.notes.key = note,
        Pick::Ca => conn.notes.ca = note,
    }
}

/// Starts a file picker for `target` on its own thread; it never runs on the UI thread.
fn start_picker(conn: &mut ConnScreen, target: Pick, ctx: &egui::Context) {
    if conn.picker.is_some() {
        return;
    }
    let ssh_dir = dbee_config::home_dir().map(|h| h.join(".ssh"));
    let (title, dir) = match target {
        // ~/.ssh, or the folder of the key the login uses when there is no ~/.ssh.
        Pick::Key => {
            ("Choose an SSH key", ssh_dir.filter(|d| d.is_dir()).or_else(|| conn.key.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)))
        }
        Pick::Ca => {
            ("Choose a CA certificate", conn.form.ca_file.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(dbee_config::home_dir))
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new().name("dbee-file-picker".into()).spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let _ = tx.send(dialog.pick_file());
        repaint.request_repaint();
    });
    let note = match spawned {
        Ok(_) => {
            conn.picker = Some(Picker { target, rx, started: Instant::now() });
            None
        }
        Err(e) => Some(format!("The file picker could not be started: {e}")),
    };
    match target {
        Pick::Key => conn.notes.key = note,
        Pick::Ca => conn.notes.ca = note,
    }
}

fn apply(app: &mut DbeeApp, act: Act, ctx: &egui::Context) {
    let busy = app.view.conn.running.is_some();
    match act {
        Act::New => app.view.conn.new_connection(),
        Act::Load(c) => app.view.conn.load(&c),
        Act::Connect(c) => {
            // While a test or connect runs, a double-click starts nothing more.
            if !busy {
                app.view.conn.load(&c);
                run_form(app, OPEN);
            }
        }
        Act::Duplicate(id) => match app.run(DUPLICATE, json!({ "id": id })) {
            Ok(dbee_engine::Outcome::Done(v)) => {
                let copy = v.get("id").and_then(Json::as_str).and_then(|id| app.session.state().store.get(id)).cloned();
                if let Some(c) = copy {
                    app.view.conn.load(&c);
                }
            }
            Ok(_) => {}
            Err(e) => app.notice(e.to_string()),
        },
        Act::AskDelete { id, name } => app.view.dialog = Some(Dialog::DeleteConnection { id, name }),
        Act::Test if !busy => run_form(app, TEST),
        Act::ConnectForm if !busy => run_form(app, OPEN),
        Act::Test | Act::ConnectForm => {}
        Act::Save => run_form(app, SAVE),
        Act::FillFromUrl => app.view.dialog = Some(Dialog::fill_from_url()),
        Act::Pick(target) => start_picker(&mut app.view.conn, target, ctx),
        Act::StopPicking => app.view.conn.picker = None,
        Act::ClearCa => {
            app.view.conn.form.ca_file = None;
            app.view.conn.notes.ca = None;
        }
        Act::AutoKey => {
            app.view.conn.form.ssh_key = None;
            app.view.conn.notes.key = None;
        }
    }
}

/// Runs `command` (save, test or open) on the form's connection; a form that is not usable says
/// why under the buttons and runs nothing.
fn run_form(app: &mut DbeeApp, command: &'static str) {
    let conn = &mut app.view.conn;
    conn.message = None;
    let config = match conn.form.to_config() {
        Ok(c) => c,
        Err(why) => {
            conn.message = Some(Message { error: true, text: why });
            return;
        }
    };
    let password = conn.form.password.clone();
    let typed = !password.is_empty();
    let saved = !config.id.is_empty() && app.session.state().store.saved.iter().any(|c| c.id == config.id);
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
    }
    let generation = conn.generation;
    app.start(command, params, Origin::Form(generation));
}

// ---- drawing ---------------------------------------------------------------------------------

/// Whether `c` matches the filter `q` (lower case): its name, host, user, database or SSH host.
fn matches(c: &ConnectionConfig, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let ssh_host = c.ssh.as_ref().map(|s| s.host.as_str()).unwrap_or_default();
    [c.display_name().as_str(), c.host.as_str(), c.user.as_str(), c.database.as_str(), ssh_host].iter().any(|f| f.to_lowercase().contains(q))
}

/// `3 min ago` for a Unix time.
fn ago(used_at: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let s = now.saturating_sub(used_at);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// One line of text cut to `width` with an ellipsis.
fn line_galley(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// A full-width button with a code-drawn icon left of its label.
fn icon_button(ui: &mut egui::Ui, label: &str, icon: fn(&egui::Painter, Rect, Color32)) -> egui::Response {
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
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0));
    icon(p, icon_rect, t.accent);
    p.text(pos2(rect.left() + 36.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), t.text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

/// A small, muted section heading with a count.
fn section(ui: &mut egui::Ui, title: &str, count: usize) {
    let t = TOKENS;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).small().strong().color(t.text_muted));
        ui.label(RichText::new(count.to_string()).small().color(t.text_muted));
    });
    ui.add_space(2.0);
}

/// The left side: New connection, the filter, the store warning, the saved and recent cards.
fn side(ui: &mut egui::Ui, st: &State, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = TOKENS;
    egui::Panel::left("connections-side").resizable(false).exact_size(300.0).frame(Frame::new().fill(t.panel).inner_margin(Margin::same(14))).show(
        ui,
        |ui| {
            let new = icon_button(ui, "New connection", icons::plus);
            tags.tag("conn-new", &new);
            if new.clicked() {
                acts.push(Act::New);
            }
            ui.add_space(10.0);
            let filter = ui.add(
                TextEdit::singleline(&mut conn.filter)
                    .id(egui::Id::new("conn-filter"))
                    .hint_text("Filter")
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(6, 5)),
            );
            tags.tag_value("conn-filter", &filter, conn.filter.as_str());
            if let Some(w) = &st.store_warning {
                ui.add_space(8.0);
                let r = ui.add(Label::new(RichText::new(w.as_str()).size(12.0).color(t.danger)).wrap());
                tags.tag_value("conn-store-warning", &r, w.as_str());
            }
            ui.add_space(14.0);
            let q = conn.filter.trim().to_lowercase();
            let selected = conn.form.id.clone();
            let busy = conn.running.is_some();
            ScrollArea::vertical().id_salt("conn-lists").auto_shrink([false, false]).show(ui, |ui| {
                let saved: Vec<&ConnectionConfig> = st.store.saved.iter().filter(|c| matches(c, &q)).collect();
                section(ui, "SAVED", saved.len());
                if saved.is_empty() {
                    let empty = if st.store.saved.is_empty() { "No saved connections yet." } else { "No saved connection matches." };
                    ui.label(RichText::new(empty).color(t.text_muted));
                }
                for c in saved {
                    card(ui, tags, &format!("conn-saved-{}", c.id), c, None, c.id == selected, busy, acts);
                }
                ui.add_space(14.0);
                let recent: Vec<(usize, &dbee_config::RecentEntry)> =
                    st.store.recent.iter().enumerate().filter(|(_, r)| matches(&r.config, &q)).collect();
                section(ui, "RECENT", recent.len());
                if recent.is_empty() {
                    let empty = if st.store.recent.is_empty() { "Connections you open show here." } else { "No recent connection matches." };
                    ui.label(RichText::new(empty).color(t.text_muted));
                }
                for (n, r) in recent {
                    card(ui, tags, &format!("conn-recent-{n}"), &r.config, Some(r.used_at), false, busy, acts);
                }
            });
        },
    );
}

/// One connection card: its colour on the left edge, the name and where it goes.
fn card(ui: &mut egui::Ui, tags: &mut Tags, id: &str, c: &ConnectionConfig, used_at: Option<u64>, selected: bool, busy: bool, acts: &mut Vec<Act>) {
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H), Sense::click());
    let fill = if selected {
        t.selection
    } else if r.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS), fill);
    let bar = Rect::from_min_size(pos2(rect.left() + 3.0, rect.top() + 7.0), vec2(4.0, rect.height() - 14.0));
    p.rect_filled(bar, CornerRadius::same(2), connection_color(c.color).unwrap_or(t.border));
    let text_w = rect.width() - 22.0;
    let name = c.display_name();
    p.galley(pos2(rect.left() + 16.0, rect.top() + 6.0), line_galley(ui, &name, FontId::proportional(14.0), t.text, text_w), t.text);
    let mut sub = format!("{}@{}:{}/{}", c.user, c.host, c.port, c.database);
    if c.ssh.is_some() {
        sub.push_str(" · SSH");
    }
    if let Some(at) = used_at {
        sub = format!("{} · {sub}", ago(at));
    }
    p.galley(pos2(rect.left() + 16.0, rect.top() + 25.0), line_galley(ui, &sub, FontId::proportional(12.0), t.text_muted, text_w), t.text_muted);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    tags.tag_value(id, &r, name.as_str());
    if r.clicked() {
        acts.push(Act::Load(c.clone()));
    }
    if r.double_clicked() {
        acts.push(Act::Connect(c.clone()));
    }
    r.context_menu(|ui| {
        let items =
            [("Connect", "conn-menu-connect"), ("Edit", "conn-menu-edit"), ("Duplicate", "conn-menu-duplicate"), ("Delete", "conn-menu-delete")];
        for (label, item) in items {
            let b = ui.add_enabled(!(busy && item == "conn-menu-connect"), Button::new(label));
            tags.tag(item, &b);
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

/// The height of everything in the form's card but its fields, as drawn: the card's margins and
/// border, the title row with its spacing, Save · Test · Connect with their spacing, the message
/// slot, and the version line under the card.
const FIXED_H: f32 = 46.0 + 42.0 + 46.0 + MESSAGE_H + 31.0;

/// Room for the fields' scroll bar, right of them.
const SCROLL_GAP: i8 = 12;

/// The right side: the form in a card. The fields scroll when the window is short; the title
/// row, the buttons and the message slot under them stay in view.
fn main(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = TOKENS;
    egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::symmetric(24, 20))).show(ui, |ui| {
        // The card adds its margins and border to the form's width.
        let width = (ui.available_width() - 46.0).clamp(240.0, FORM_WIDTH);
        let fields_h = (ui.available_height() - FIXED_H).max(120.0);
        ui.vertical_centered(|ui| {
            let card = Frame::new()
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(RADIUS.saturating_mul(2)))
                // Less on the right: the fields' scroll bar sits there, beside them.
                .inner_margin(Margin { left: 22, right: 22 - SCROLL_GAP, top: 22, bottom: 22 })
                .show(ui, |ui| {
                    ui.set_width(width + f32::from(SCROLL_GAP));
                    ui.with_layout(Layout::top_down(Align::Min), |ui| form(ui, conn, tags, acts, width, fields_h));
                });
            // The connection's colour as a strip across the top of the card.
            if let Some(color) = connection_color(conn.form.color) {
                let r = card.response.rect;
                let strip = Rect::from_min_size(r.min, vec2(r.width(), 5.0));
                let radius = RADIUS.saturating_mul(2);
                ui.painter().rect_filled(strip, CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 }, color);
            }
            ui.add_space(10.0);
            ui.label(RichText::new(format!("dbee {}", env!("CARGO_PKG_VERSION"))).small().color(t.text_muted));
        });
    });
}

/// A field's label and the field under it, `width` wide.
fn column<R>(ui: &mut egui::Ui, width: f32, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(width, ROW_H), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        let (rect, _) = ui.allocate_exact_size(vec2(width, LABEL_H), Sense::hover());
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.0), TOKENS.text_muted);
        add(ui)
    })
    .inner
}

/// The form's text fields: Enter in one of them connects.
const FORM_FIELDS: [&str; 9] =
    ["conn-name", "conn-host", "conn-port", "conn-user", "conn-password", "conn-database", "conn-ssh-host", "conn-ssh-port", "conn-ssh-user"];

fn field_id(id: &str) -> egui::Id {
    egui::Id::new(("dbee-form", id))
}

/// A one-line text field tagged `id` with its text.
fn text_field(ui: &mut egui::Ui, tags: &mut Tags, id: &'static str, text: &mut String, hint: &str) -> egui::Response {
    let r = ui.add(
        TextEdit::singleline(text)
            .id(field_id(id))
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .min_size(vec2(0.0, FIELD_H))
            .margin(Margin::symmetric(6, 5)),
    );
    tags.tag_value(id, &r, text.as_str());
    r
}

/// A row of exclusive choices, tagged `id` (with the chosen one's name) and `<id>-<name>` each.
fn segments<T: Copy + PartialEq>(ui: &mut egui::Ui, tags: &mut Tags, id: &str, value: &mut T, options: &[(T, &str, &str)], width: f32) {
    let n = options.len().max(1) as f32;
    let gap = 4.0;
    let each = ((width - gap * (n - 1.0)) / n).max(20.0);
    let t = TOKENS;
    let frame = Rect::from_min_size(ui.cursor().min, vec2(width, FIELD_H));
    ui.painter().rect(frame, CornerRadius::same(RADIUS), t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let row = ui.allocate_ui_with_layout(vec2(width, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (option, label, name) in options {
            let r = ui.add_sized(vec2(each, FIELD_H), Button::selectable(*value == *option, *label));
            tags.tag_value(&format!("{id}-{name}"), &r, *value == *option);
            if r.clicked() {
                *value = *option;
            }
        }
    });
    let chosen = options.iter().find(|(o, ..)| *o == *value).map_or("", |(_, _, name)| *name);
    tags.tag_value(id, &row.response, chosen);
}

/// A read-only path box (`id`) with what it shows, or a note in its place.
fn path_box(ui: &mut egui::Ui, tags: &mut Tags, id: &str, width: f32, path: Option<&Path>, note: Option<&str>, waiting: bool) {
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let shown = path.map(|p| tilde(&p.display().to_string()));
    let (text, color) = if waiting {
        ("Waiting for the file picker…".to_string(), t.text_muted)
    } else if let Some(n) = note {
        (n.to_string(), t.danger)
    } else {
        match &shown {
            Some(s) => (s.clone(), t.text),
            None => ("None".to_string(), t.text_muted),
        }
    };
    let galley = line_galley(ui, &text, FontId::proportional(13.0), color, width - 14.0);
    let y = rect.center().y - galley.size().y / 2.0;
    p.galley(pos2(rect.left() + 7.0, y), galley, color);
    let r = if text.chars().count() > 40 { r.on_hover_text(text.as_str()) } else { r };
    tags.tag_value(id, &r, shown.map_or(Json::Null, Json::from));
}

/// The form, `w` wide; its fields scroll past `fields_h`.
fn form(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32, fields_h: f32) {
    let t = TOKENS;
    let gap = 12.0;
    let running = conn.running.map(|(_, kind)| kind);
    let picking = conn.picker.as_ref().map(|p| p.target);
    let has_store = conn.has_store;
    let in_field = ui.ctx().memory(|m| m.focused()).is_some_and(|focused| FORM_FIELDS.iter().any(|id| field_id(id) == focused));
    if in_field && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        acts.push(Act::ConnectForm);
    }
    let f = &mut conn.form;

    // Title, and Fill from URL.
    ui.allocate_ui_with_layout(vec2(w, 30.0), Layout::left_to_right(Align::Center), |ui| {
        let url_w = 120.0;
        let title = f.title();
        let galley = line_galley(ui, &title, FontId::proportional(19.0), t.text, w - url_w - gap);
        let (rect, r) = ui.allocate_exact_size(vec2(w - url_w - gap, 30.0), Sense::hover());
        ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, t.text);
        tags.tag_value("conn-title", &r, title.as_str());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui.add(Button::new(RichText::new("Fill from URL…").color(t.accent)).frame(false));
            tags.tag("conn-url", &r);
            if r.clicked() {
                acts.push(Act::FillFromUrl);
            }
        });
    });
    ui.add_space(4.0);

    // The fields; only they scroll.
    ScrollArea::vertical().id_salt("conn-form-scroll").max_height(fields_h).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(w);
        // Name and colour.
        let colors: [(ConnectionColor, &str); 8] = [
            (ConnectionColor::None, "none"),
            (ConnectionColor::Red, "red"),
            (ConnectionColor::Orange, "orange"),
            (ConnectionColor::Yellow, "yellow"),
            (ConnectionColor::Green, "green"),
            (ConnectionColor::Blue, "blue"),
            (ConnectionColor::Purple, "purple"),
            (ConnectionColor::Gray, "gray"),
        ];
        let dot = 20.0;
        let dots_w = dot * colors.len() as f32 + 6.0 * (colors.len() as f32 - 1.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            column(ui, w - dots_w - gap, "Name", |ui| text_field(ui, tags, "conn-name", &mut f.name, "Optional"));
            column(ui, dots_w, "Colour", |ui| {
                let row = ui.allocate_ui_with_layout(vec2(dots_w, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (color, name) in colors {
                        let (rect, r) = ui.allocate_exact_size(vec2(dot, dot), Sense::click());
                        let chosen = f.color == color;
                        let p = ui.painter();
                        match connection_color(color) {
                            Some(c) => {
                                p.circle_filled(rect.center(), dot / 2.0 - 2.0, c);
                            }
                            None => {
                                p.circle_stroke(rect.center(), dot / 2.0 - 3.0, Stroke::new(1.5, t.border));
                            }
                        }
                        if chosen || r.hovered() {
                            p.circle_stroke(rect.center(), dot / 2.0, Stroke::new(1.5, if chosen { t.text } else { t.border }));
                        }
                        let r = r.on_hover_text(name);
                        r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, chosen, name));
                        tags.tag_value(&format!("conn-color-{name}"), &r, chosen);
                        if r.clicked() {
                            f.color = color;
                        }
                    }
                });
                let chosen = colors.iter().find(|(c, _)| *c == f.color).map_or("none", |(_, n)| *n);
                tags.tag_value("conn-color", &row.response, chosen);
            });
        });

        // Host and port.
        let port_w = 110.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            column(ui, w - port_w - gap, "Host", |ui| text_field(ui, tags, "conn-host", &mut f.host, "localhost"));
            column(ui, port_w, "Port", |ui| text_field(ui, tags, "conn-port", &mut f.port, "5432"));
        });

        // User and password.
        let half = (w - gap) / 2.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            column(ui, half, "User", |ui| text_field(ui, tags, "conn-user", &mut f.user, ""));
            column(ui, half, "Password", |ui| {
                ui.allocate_ui_with_layout(vec2(half, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let show_w = 52.0;
                    let r = ui.add(
                        TextEdit::singleline(&mut f.password)
                            .id(field_id("conn-password"))
                            .password(!f.show_password)
                            .desired_width(half - show_w - 6.0)
                            .min_size(vec2(0.0, FIELD_H))
                            .margin(Margin::symmetric(6, 5)),
                    );
                    tags.tag_secret("conn-password", &r, !f.password.is_empty());
                    let label = if f.show_password { "Hide" } else { "Show" };
                    let s = ui.add_sized(vec2(show_w, FIELD_H), Button::new(label));
                    tags.tag_value("conn-password-show", &s, f.show_password);
                    if s.clicked() {
                        f.show_password = !f.show_password;
                    }
                });
            });
        });

        // Save passwords, or why it cannot.
        ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center), |ui| {
            if has_store {
                let r = ui.checkbox(&mut f.save_secrets, "Save passwords in the keyring");
                tags.tag_value("conn-save-secrets", &r, f.save_secrets);
            } else {
                let mut off = false;
                let r = ui.add_enabled(false, Checkbox::new(&mut off, "Save passwords"));
                tags.tag_value("conn-save-secrets", &r, false);
                let note = "No keyring found, so passwords are asked each time.";
                let r = ui.label(RichText::new(note).size(12.0).color(t.text_muted));
                tags.tag_value("conn-no-keyring", &r, note);
            }
        });
        ui.add_space(4.0);

        // Database.
        column(ui, w, "Database", |ui| text_field(ui, tags, "conn-database", &mut f.database, ""));

        // SSL mode and CA file.
        let modes = [
            (SslMode::Disable, "disable", "disable"),
            (SslMode::Prefer, "prefer", "prefer"),
            (SslMode::Require, "require", "require"),
            (SslMode::VerifyCa, "verify-ca", "verify-ca"),
            (SslMode::VerifyFull, "verify-full", "verify-full"),
        ];
        column(ui, w, "SSL mode", |ui| segments(ui, tags, "conn-ssl", &mut f.ssl_mode, &modes, w));
        column(ui, w, "CA file (optional)", |ui| {
            ui.allocate_ui_with_layout(vec2(w, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let button_w = 84.0;
                let waiting = picking == Some(Pick::Ca);
                path_box(ui, tags, "conn-ca", w - 2.0 * (button_w + 6.0), f.ca_file.as_deref(), conn.notes.ca.as_deref(), waiting);
                let choose = ui.add_enabled(
                    picking.is_none() || waiting,
                    Button::new(if waiting { "Stop" } else { "Choose…" }).min_size(vec2(button_w, FIELD_H)),
                );
                tags.tag("conn-ca-choose", &choose);
                if choose.clicked() {
                    acts.push(if waiting { Act::StopPicking } else { Act::Pick(Pick::Ca) });
                }
                let clear = ui.add_enabled(f.ca_file.is_some(), Button::new("Clear").min_size(vec2(button_w, FIELD_H)));
                tags.tag("conn-ca-clear", &clear);
                if clear.clicked() {
                    acts.push(Act::ClearCa);
                }
            });
        });
        ui.add_space(4.0);

        // Read-only.
        ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center), |ui| {
            let r = ui.checkbox(&mut f.read_only, "Read-only");
            tags.tag_value("conn-readonly", &r, f.read_only);
            ui.label(RichText::new("Sessions start read-only and the grid offers no editing.").size(12.0).color(t.text_muted));
        });
        ui.add_space(6.0);

        // SSH tunnel.
        Frame::new().fill(t.panel).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::same(10)).show(ui, |ui| {
            let w = w - 20.0;
            ui.set_width(w);
            ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center), |ui| {
                let r = ui.checkbox(&mut f.ssh_on, RichText::new("SSH tunnel").strong());
                tags.tag_value("conn-ssh-on", &r, f.ssh_on);
            });
            if !f.ssh_on {
                return;
            }
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                column(ui, w - port_w - gap, "SSH host", |ui| text_field(ui, tags, "conn-ssh-host", &mut f.ssh_host, "bastion.example.com"));
                column(ui, port_w, "SSH port", |ui| text_field(ui, tags, "conn-ssh-port", &mut f.ssh_port, "22"));
            });
            column(ui, w, "SSH user", |ui| text_field(ui, tags, "conn-ssh-user", &mut f.ssh_user, ""));
            let auths = [(Auth::Key, "Key", "key"), (Auth::Agent, "Agent", "agent"), (Auth::Password, "Password", "password")];
            column(ui, w, "Login", |ui| segments(ui, tags, "conn-ssh-auth", &mut f.ssh_auth, &auths, w));
            // What the login will use: one fixed slot for every choice.
            ui.add_space(2.0);
            ui.allocate_ui_with_layout(vec2(w, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let button_w = 92.0;
                let line_w = w - 2.0 * (button_w + 6.0);
                let waiting = picking == Some(Pick::Key);
                let wanted = f.ssh_auth();
                let (text, color) = if waiting {
                    ("Waiting for the file picker…".to_string(), t.text_muted)
                } else if let (Some(note), Auth::Key) = (&conn.notes.key, f.ssh_auth) {
                    (note.clone(), t.danger)
                } else if conn.key.for_auth.as_ref() == Some(&wanted) {
                    (conn.key.text.clone(), t.text)
                } else {
                    ("Looking for the key…".to_string(), t.text_muted)
                };
                let (rect, r) = ui.allocate_exact_size(vec2(line_w, FIELD_H), Sense::hover());
                let galley = line_galley(ui, &text, FontId::proportional(13.0), color, line_w);
                ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, color);
                let r = if text.chars().count() > 50 { r.on_hover_text(text.as_str()) } else { r };
                tags.tag_value("conn-ssh-key", &r, text.as_str());
                if f.ssh_auth == Auth::Key {
                    let change = ui.add_enabled(
                        picking.is_none() || waiting,
                        Button::new(if waiting { "Stop" } else { "Change…" }).min_size(vec2(button_w, FIELD_H)),
                    );
                    tags.tag("conn-ssh-change", &change);
                    if change.clicked() {
                        acts.push(if waiting { Act::StopPicking } else { Act::Pick(Pick::Key) });
                    }
                    if f.ssh_key.is_some() {
                        let auto = ui
                            .add(Button::new("Automatic").min_size(vec2(button_w, FIELD_H)))
                            .on_hover_text("Use the agent, then ~/.ssh/id_ed25519, id_ecdsa or id_rsa");
                        tags.tag("conn-ssh-auto", &auto);
                        if auto.clicked() {
                            acts.push(Act::AutoKey);
                        }
                    }
                }
            });
        });
    });
    ui.add_space(8.0);

    // Save · Test · Connect.
    ui.allocate_ui_with_layout(vec2(w, 32.0), Layout::left_to_right(Align::Center), |ui| {
        let save = ui.add(Button::new("Save").min_size(vec2(88.0, 30.0)));
        tags.tag_value("conn-save", &save, "Save");
        if save.clicked() {
            acts.push(Act::Save);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if running == Some(Running::Connect) { "Connecting…" } else { "Connect" };
            let connect = Button::new(RichText::new(label).color(t.accent_text)).fill(t.accent).min_size(vec2(112.0, 30.0));
            let connect = ui.add_enabled(running.is_none(), connect);
            tags.tag_value("conn-connect", &connect, label);
            if connect.clicked() {
                acts.push(Act::ConnectForm);
            }
            let label = if running == Some(Running::Test) { "Testing…" } else { "Test" };
            let test = ui.add_enabled(running.is_none(), Button::new(label).min_size(vec2(96.0, 30.0)));
            tags.tag_value("conn-test", &test, label);
            if test.clicked() {
                acts.push(Act::Test);
            }
        });
    });
    ui.add_space(6.0);

    // The reserved slot for an error or a result.
    let (rect, slot) = ui.allocate_exact_size(vec2(w, MESSAGE_H), Sense::hover());
    if let Some(m) = &conn.message {
        let color = if m.error { t.danger } else { t.success };
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), color.gamma_multiply(0.08));
        let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 6.0))).layout(Layout::top_down(Align::Min)));
        ScrollArea::vertical().id_salt("conn-message").max_height(MESSAGE_H - 12.0).auto_shrink([false, true]).show(&mut inner, |ui| {
            ui.add(Label::new(RichText::new(m.text.as_str()).color(color)).wrap().selectable(true));
        });
        tags.tag_value(if m.error { "conn-error" } else { "conn-result" }, &slot, m.text.as_str());
    }
}

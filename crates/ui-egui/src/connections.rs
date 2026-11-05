//! The connection screen: the saved and recent connections on the left, the engine cards or the
//! connection form on the right ([`screen`]).
//!
//! - Left: "New connection", a filter, then the SAVED and RECENT cards, each with its colour, its
//!   database type's label under the name. A single click loads a connection into the form, a
//!   double-click connects; right-click offers Connect · Edit · Duplicate · Delete (Delete asks
//!   first: it deletes the saved passwords too).
//! - Right, under the title row (the title, Fill from URL): a new connection starts with the
//!   engine cards, one per database type ([`DbKind::ALL`]); a type this build has no driver for
//!   (not in the session's `kinds`) is greyed with "Upcoming" and cannot be chosen. A card, or a
//!   URL (its scheme names the type), opens the form, and the title row gains the type and Change
//!   engine, which goes back to the cards (the form keeps what was typed, and a port that still
//!   holds the old type's default takes the new one's). A saved or recent connection opens
//!   straight into the form, its type fixed; Fill from URL then takes that type's URLs only.
//! - The form ([`crate::conn_form`]) has the rows of the type's settings, then Save · Test ·
//!   Connect at the bottom, and under them a reserved slot for the error or the test result.
//!   Every row has a fixed height, so typing, a message or another login choice never moves the
//!   form; only turning the SSH tunnel on or off does. The right side is as tall with the cards
//!   as with the form. In a short window only the fields scroll: the title row, the buttons and
//!   the message slot stay in view. At the app's first size (1100 × 720) every row of a server
//!   form is in view while the SSH tunnel is off.
//! - Test and Connect run as jobs (`connection.test`, `connection.open`); while one runs both
//!   buttons are disabled and say so, and a double-click on a card does nothing. A `needs…`
//!   answer opens a dialog ([`crate::dialogs`]) that runs the command again; other errors show
//!   under the buttons with the server's message.
//! - The file pickers ("Change…" for the key, "Choose…" for the CA file, "Choose…" and "New file…"
//!   for a database file) run rfd on a thread of their own and send the path back; a picker that
//!   cannot open leaves a note in its row ([`crate::picker`]). New file… makes the next test or
//!   connect ask for the file to be made (`createFile`), once.
//!
//! The form (its fields, their validation, and drawing it) is [`crate::conn_form`]; this module
//! draws the screen around it and acts on both.
//!
//! Secrets: the typed password lives only in the form (and in the params of the command it goes
//! to). `ui.inspect` reports the field as masked with whether it is set; `Debug` and the logs
//! never show it.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dbee_config::{ConnectionConfig, DbKind, Settings, SshAuth, SshConfig, SslMode};
use dbee_engine::{EngineError, JobId, Json, State};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Button, Color32, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, TextFormat,
    pos2, vec2,
};
use serde_json::json;

use crate::app::Origin;
use crate::automation::Tags;
use crate::conn_form::{self, Form};
use crate::dialogs::{self, Dialog};
use crate::picker::{self, Pick, Picker};
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
/// A card in the lists.
const CARD_H: f32 = 46.0;

/// Which form job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    Test,
    Connect,
}

/// What a message under the buttons is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Something failed (`conn-error`).
    Error,
    /// A test passed, the connection was saved (`conn-result`).
    Result,
    /// Something to know that is neither (`conn-note`), e.g. about a file picker.
    Note,
}

/// What the slot under the buttons shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub kind: Kind,
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

/// What the right side shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The engine cards of a new connection.
    Engines,
    Form,
}

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
    /// The session has a secret store (`document.inspect`'s `secretStore`).
    pub has_store: bool,
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
            has_store,
        }
    }

    /// Shows a new, empty connection: the engine cards first.
    fn new_connection(&mut self) {
        self.form = Form::new(self.has_store);
        self.step = Step::Engines;
        self.forget_shown();
    }

    /// The form shows another connection: its message and key line are gone.
    fn forget_shown(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.message = None;
        self.key.for_auth = None;
    }

    /// Loads `c` into the form. The same connection again keeps the typed password and what the
    /// form shows under its buttons.
    fn load(&mut self, c: &ConnectionConfig) {
        let same = !c.id.is_empty() && c.id == self.form.id;
        let password = if same { std::mem::take(&mut self.form.password) } else { String::new() };
        self.form = Form::from_config(c);
        self.form.password = password;
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
                conn.message = Some(Message { kind: Kind::Result, text: "Saved.".into() });
            }
            TEST => conn.message = Some(Message { kind: Kind::Result, text: test_text(&v) }),
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
                app.view.conn.message = Some(Message { kind: Kind::Error, text: error_text(&e) });
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

/// The database types whose URLs can fill the form: those of the session's `kinds` that have a
/// URL, and of them only the form's own once its type is fixed.
pub(crate) fn url_kinds(form: &Form, kinds: &[DbKind]) -> Vec<DbKind> {
    kinds.iter().copied().filter(|k| !k.info().url_schemes.is_empty() && (!form.engine_fixed() || *k == form.kind)).collect()
}

/// Fills the form from `connection.parseUrl`'s result: the type its scheme names, and the parts
/// the URL has. The form shows, past the engine cards.
pub(crate) fn fill_from_url(app: &mut DbeeApp, parts: &Json) {
    let form = &mut app.view.conn.form;
    if let Some(kind) = parts.get("kind").and_then(Json::as_str).and_then(DbKind::from_id) {
        form.switch_kind(kind);
    }
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
    app.view.conn.step = Step::Form;
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
        Err(e) => app.notice(crate::app::notice_text(&e)),
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
        let kinds = session.kinds();
        side(ui, session.state(), &mut view.conn, tags, &mut acts);
        main(ui, &mut view.conn, &kinds, tags, &mut acts);
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
    let ssh = SshConfig { host: conn.form.ssh_host.clone(), port: 22, user: conn.form.ssh_user.clone(), auth: want.clone() };
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
            if app.session.kinds().contains(&kind) {
                app.view.conn.form.switch_kind(kind);
                app.view.conn.step = Step::Form;
            }
        }
        Act::ChangeEngine => app.view.conn.step = Step::Engines,
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
            Err(e) => app.notice(crate::app::notice_text(&e)),
        },
        Act::AskDelete { id, name } => app.view.dialog = Some(Dialog::DeleteConnection { id, name }),
        Act::Test if !busy => run_form(app, TEST),
        Act::ConnectForm if !busy => run_form(app, OPEN),
        Act::Test | Act::ConnectForm => {}
        Act::Save => run_form(app, SAVE),
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

/// Runs `command` (save, test or open) on the form's connection; a form that is not usable says
/// why under the buttons and runs nothing.
fn run_form(app: &mut DbeeApp, command: &'static str) {
    let conn = &mut app.view.conn;
    conn.message = None;
    let config = match conn.form.to_config() {
        Ok(c) => c,
        Err(why) => {
            conn.message = Some(Message { kind: Kind::Error, text: why });
            return;
        }
    };
    // Only a server takes a password; one typed for another type stays in the form.
    let password = if config.kind.uses_file() { String::new() } else { conn.form.password.clone() };
    let typed = !password.is_empty();
    // New file…: the next test or open asks for the file to be made, once.
    let create_file = conn.form.create_file && command != SAVE;
    if create_file {
        conn.form.create_file = false;
    }
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
        if create_file {
            p.insert("createFile".into(), json!(true));
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
/// `text` on one line in `font` and `color`, cut with "…" to `width`.
pub(crate) fn line_galley(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
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
    let label = c.kind.label();
    let mut sub = match c.kind.info().settings {
        Settings::Server { .. } => format!("{label} · {}@{}:{}/{}", c.user, c.host, c.port, c.database),
        Settings::File { .. } => format!("{label} · {}", c.file.as_ref().map(|f| tilde(&f.display().to_string())).unwrap_or_default()),
    };
    if c.ssh.is_some() && c.kind.takes_ssh() {
        sub.push_str(" · SSH");
    }
    if let Some(at) = used_at {
        sub = format!("{} · {sub}", ago(at));
    }
    p.galley(pos2(rect.left() + 16.0, rect.top() + 25.0), line_galley(ui, &sub, FontId::proportional(12.0), t.text_muted, text_w), t.text_muted);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    tags.tag_value(id, &r, name.as_str());
    tags.tag_value(&format!("{id}-kind"), &r, label);
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
            let enabled = !(busy && item == "conn-menu-connect");
            let b = ui.add_enabled(enabled, Button::new(label));
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

/// The version line under the card, with its spacing.
const VERSION_H: f32 = 31.0;

/// The least height of the card's body (the engine cards, or the form under the title row).
const MIN_BODY_H: f32 = 240.0;

/// Room for the fields' scroll bar, right of them.
const SCROLL_GAP: i8 = 12;

/// The right side: the title row, then the engine cards or the form, in a card as tall as the
/// window allows either way.
fn main(ui: &mut egui::Ui, conn: &mut ConnScreen, kinds: &[DbKind], tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = TOKENS;
    egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::symmetric(24, 20))).show(ui, |ui| {
        // The card adds its margins and border to the form's width.
        let width = (ui.available_width() - 46.0).clamp(240.0, FORM_WIDTH);
        ui.vertical_centered(|ui| {
            let card = Frame::new()
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(RADIUS.saturating_mul(2)))
                // Less on the right: the fields' scroll bar sits there, beside them.
                .inner_margin(Margin { left: 22, right: 22 - SCROLL_GAP, top: 22, bottom: 22 })
                .show(ui, |ui| {
                    ui.set_width(width + f32::from(SCROLL_GAP));
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        let url = !url_kinds(&conn.form, kinds).is_empty();
                        conn_form::header(ui, conn, url, conn.step == Step::Form, tags, acts, width);
                        let body_h = (ui.available_height() - VERSION_H).max(MIN_BODY_H);
                        ui.allocate_ui_with_layout(vec2(width + f32::from(SCROLL_GAP), body_h), Layout::top_down(Align::Min), |ui| {
                            ui.set_min_height(body_h);
                            match conn.step {
                                Step::Engines => engines(ui, kinds, tags, acts, width),
                                Step::Form => conn_form::fields_and_footer(ui, conn, tags, acts, width),
                            }
                        });
                    });
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

/// An engine card's height.
const ENGINE_H: f32 = 104.0;
/// The narrowest an engine card gets before the cards wrap.
const ENGINE_MIN_W: f32 = 120.0;

/// The engine cards, `w` wide: one per database type, as many in a row as fit; those of `kinds`
/// can be chosen.
fn engines(ui: &mut egui::Ui, kinds: &[DbKind], tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    ui.add_space(4.0);
    ui.label(RichText::new("Choose a database").size(15.0));
    ui.add_space(6.0);
    let gap = 8.0;
    let per_row = (((w + gap) / (ENGINE_MIN_W + gap)).floor() as usize).clamp(1, DbKind::ALL.len());
    let card_w = (w - gap * (per_row as f32 - 1.0)) / per_row as f32;
    for row in DbKind::ALL.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for kind in row {
                engine_card(ui, *kind, kinds.contains(kind), card_w, tags, acts);
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// The card of the database type `kind`, `w` wide: its name, and "Upcoming" in the corner when
/// it is not `available` (this build has no driver for it), greyed and not clickable.
fn engine_card(ui: &mut egui::Ui, kind: DbKind, available: bool, w: f32, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(w, ENGINE_H), if available { Sense::click() } else { Sense::hover() });
    let fill = if !available {
        t.card
    } else if r.is_pointer_button_down_on() {
        t.selection
    } else if r.hovered() {
        t.hover
    } else {
        t.card
    };
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let color = if available { t.text } else { t.text_muted };
    let name = line_galley(ui, kind.label(), FontId::proportional(16.0), color, w - 28.0);
    p.galley(pos2(rect.left() + 14.0, rect.center().y - name.size().y / 2.0), name, color);
    if !available {
        let chip = line_galley(ui, "Upcoming", FontId::proportional(11.0), t.text_muted, w);
        let size = vec2(chip.size().x + 12.0, 18.0);
        let pill = Rect::from_min_size(pos2(rect.right() - 8.0 - size.x, rect.top() + 8.0), size);
        p.rect_filled(pill, CornerRadius::same(RADIUS.saturating_mul(2)), t.panel);
        p.galley(pos2(pill.left() + 6.0, pill.center().y - chip.size().y / 2.0), chip, t.text_muted);
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, available, kind.label()));
    tags.tag_value_as(&format!("conn-kind-{}", kind.id()), &r, json!({ "label": kind.label(), "upcoming": !available }), available);
    if available && r.clicked() {
        acts.push(Act::ChooseKind(kind));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use dbee_config::MemoryStore;
    use dbee_engine::{Session, SessionOptions};
    use dbee_testkit::{TempHome, temp_home};

    use super::*;

    fn app() -> (DbeeApp, TempHome) {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            ..SessionOptions::default()
        })
        .unwrap();
        (DbeeApp::new(session, None), home)
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
        (DbeeApp::new(session, None), home)
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
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 900.0))), ..egui::RawInput::default() };
        ctx.run_ui(raw, |ui| app.ui(ui)).drop_without_applying_deltas();
    }

    /// The form with the SSH tunnel on and a key line that says which key it uses.
    fn with_key_line(app: &mut DbeeApp) {
        let conn = &mut app.view.conn;
        conn.step = Step::Form;
        conn.form.ssh_on = true;
        conn.key = KeyLine { for_auth: Some(conn.form.ssh_auth()), text: "Uses /k/id_ed25519".into(), path: None, pending: None };
    }

    #[test]
    fn a_quick_empty_answer_keeps_the_key_line_and_says_so_neutrally() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
    fn new_file_makes_the_next_test_or_open_create_it_once() {
        let (mut app, home) = app_with_drivers();
        let ctx = egui::Context::default();
        apply(&mut app, Act::ChooseKind(DbKind::Sqlite), &ctx);
        let file = home.home.join("new.sqlite");
        let pick = |app: &mut DbeeApp, target: Pick, path: &std::path::Path| {
            let (tx, picker) = Picker::for_test(target, app.view.conn.generation);
            app.view.conn.picker = Some(picker);
            tx.send(Some(path.to_path_buf())).unwrap();
            frame(app, &ctx);
            assert!(app.view.conn.picker.is_none());
        };
        // Choose… takes a file as it is; New file… asks for it to be made.
        pick(&mut app, Pick::NewFile, &file);
        pick(&mut app, Pick::File, &file);
        assert_eq!((app.view.conn.form.file.as_deref(), app.view.conn.form.create_file), (Some(file.as_path()), false));
        pick(&mut app, Pick::NewFile, &file);
        assert_eq!((app.view.conn.form.file.as_deref(), app.view.conn.form.create_file), (Some(file.as_path()), true));
        // Saving makes no file, and keeps the ask for the next test or open.
        apply(&mut app, Act::Save, &ctx);
        assert!(app.view.conn.form.create_file && !file.exists(), "{:?}", app.view.conn.message);
        apply(&mut app, Act::ConnectForm, &ctx);
        assert!(!app.view.conn.form.create_file, "asked once");
        until(&mut app, &ctx, "the workspace", |app| app.session.state().screen == dbee_engine::Screen::Workspace);
        assert!(file.is_file());
        assert_eq!(opens(&app).len(), 1);
        assert_eq!(opens(&app)[0]["createFile"], true, "{:?}", opens(&app));
        // The next connect opens the file as it is.
        app.run("connection.close", json!({})).unwrap();
        apply(&mut app, Act::ConnectForm, &ctx);
        until(&mut app, &ctx, "the workspace again", |app| app.session.state().screen == dbee_engine::Screen::Workspace);
        assert_eq!(opens(&app).len(), 2);
        assert!(opens(&app)[1].get("createFile").is_none(), "{:?}", opens(&app));
    }
}

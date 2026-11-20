//! The connection form: its fields as typed ([`Form`]), turning them into a [`ConnectionConfig`]
//! (port numbers, validation), and drawing it in parts, each of fixed height but the fields: the
//! pane's title row ([`title_row`]), the engine row with the database type ([`engine_row`]), then
//! the fields in one or two [`columns`], which scroll, then the strip (an error band, a result or
//! a note) and the form's bar along the pane's bottom: SAVE, `…`, TEST and CONNECT ([`body`]).
//! The screen around it, with the engine cards a new connection starts with, is
//! [`crate::connections`].
//!
//! The rows follow the database type's settings ([`dbee_config::KindInfo`]), never the type
//! itself: a server type has host and port, user and password, the database, the SSL rows when
//! it takes TLS and the SSH tunnel when it takes one; a file type has its file, with CHOOSE and
//! NEW FILE. Every type has the name and colour, Read-only, and its own on/off options as check
//! boxes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, Settings, SshAuth, SshConfig, SslMode};
use dbee_engine::Json;
use egui::{
    Align, CornerRadius, Key, Label, Layout, Modifiers, Popup, PopupCloseBehavior, RectAlign, ScrollArea, Sense, Stroke, UiBuilder, pos2, vec2,
};

use crate::automation::Tags;
use crate::components::button::{BarButton, BoxButton, TextButton};
use crate::components::checkbox::Checkbox;
use crate::components::error_band::Band;
use crate::components::icon::Icon;
use crate::components::input::{self, Field, Input};
use crate::components::logo::Logo;
use crate::components::menu::{self, Item};
use crate::components::segmented::Segmented;
use crate::components::strip::{self, Strip, Tone};
use crate::components::swatch::{self, Swatch};
use crate::components::tooltip::Hint;
use crate::connections::{Act, ConnScreen, DatabaseList, Kind, Message, Running, line_galley, tilde};
use crate::picker::Pick;
use crate::theme::size::{
    BAR_H, BAR_LINE, BUTTON_H, CONN_BESIDE_GAP, CONN_COLUMN_GAP, CONN_COLUMN_MAX_W, CONN_FIELD_GAP, CONN_FIELDS_TOP, CONN_PORT_W, CONN_SECTION_GAP,
    CONN_TITLE_MIN_W, CONN_TWO_COLUMNS_MIN, ENGINE_ROW_GAP, ENGINE_ROW_LOGO, ERROR_BAND_EDGE, ERROR_BAND_GAP, ERROR_BAND_PAD, FIELD_GAP, INPUT_H,
    INPUT_PAD_X, INSET, MENU_ITEM_H, RADIUS_CONTROL, SWATCH, SWATCH_GAP,
};
use crate::theme::{self, Text};

/// The most of the Database field's list that shows; more names scroll.
const LIST_H: f32 = 240.0;

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
    /// The database type, whose settings decide the rows.
    pub kind: DbKind,
    /// A file type's database file.
    pub file: Option<PathBuf>,
    /// NEW FILE chose `file`: every test and connect asks for it to be made, until a connect
    /// succeeds.
    pub create_file: bool,
    /// Opened from the saved list or Recent: its database type is fixed text (design E2).
    pub engine_fixed: bool,
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
    /// The key file chosen with CHOOSE; `None` picks automatically.
    pub ssh_key: Option<PathBuf>,
    /// The type's own on/off settings, `"true"` when on; keys this build does not know are kept.
    pub options: BTreeMap<String, String>,
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
            .field("kind", &self.kind)
            .field("database", &self.database)
            .field("ssh_on", &self.ssh_on)
            .finish_non_exhaustive()
    }
}

/// A port as typed: a number from 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// `kind`'s default port as the form shows it; empty for a type without one.
fn default_port(kind: DbKind) -> String {
    kind.default_port().map(|p| p.to_string()).unwrap_or_default()
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
            kind: c.kind,
            file: c.file.clone(),
            create_file: false,
            engine_fixed: false,
            color: c.color,
            host: c.host.clone(),
            port: if c.port == 0 { String::new() } else { c.port.to_string() },
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
            options: c.options.clone(),
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

    /// The connection the form describes, or why it is not usable yet. It holds only what its
    /// type's settings use: a server's host, port, user and database (the TLS fields when it takes
    /// TLS, the tunnel when it takes one), a file type's file; and of the options, the type's own
    /// and those no type of this build knows. The form keeps the rest for a switch back.
    pub(crate) fn to_config(&self) -> Result<ConnectionConfig, String> {
        let settings = self.kind.info().settings;
        let (server, tls) = match settings {
            Settings::Server { tls, .. } => (true, tls),
            Settings::File { .. } => (false, false),
        };
        let port = match parse_port(&self.port) {
            Some(p) if server => p,
            _ if !server => 0,
            _ => return Err("The port must be a number from 1 to 65535.".into()),
        };
        let text = |s: &str| if server { s.trim().to_string() } else { String::new() };
        let ssh = if self.ssh_on && self.kind.takes_ssh() {
            let port = parse_port(&self.ssh_port).ok_or("The SSH port must be a number from 1 to 65535.")?;
            Some(SshConfig {
                host: self.ssh_host.trim().into(),
                port,
                user: self.ssh_user.trim().into(),
                auth: self.ssh_auth(),
                ..SshConfig::default()
            })
        } else {
            None
        };
        let config = ConnectionConfig {
            id: self.id.clone(),
            name: self.name.trim().into(),
            kind: self.kind,
            file: if server { None } else { self.file.clone() },
            color: self.color,
            host: text(&self.host),
            port,
            user: text(&self.user),
            database: text(&self.database),
            ssl_mode: if tls { self.ssl_mode } else { SslMode::default() },
            ca_file: if tls { self.ca_file.clone() } else { None },
            ssh,
            read_only: self.read_only,
            save_secrets: self.save_secrets,
            options: self.options_of_kind(),
            ..ConnectionConfig::default()
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    /// What the title line says: the name; else "New connection", or for a saved one what the
    /// connection cards call it (a file type's file name, a server's `user@host/database`).
    pub(crate) fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.id.is_empty() {
            "New connection".into()
        } else {
            let c = ConnectionConfig {
                kind: self.kind,
                file: self.file.clone(),
                user: self.user.clone(),
                host: self.host.clone(),
                database: self.database.clone(),
                ..ConnectionConfig::default()
            };
            c.display_name()
        }
    }

    /// The options a config of the form's type keeps: the type's own, and keys no type of this
    /// build knows (a later build's).
    fn options_of_kind(&self) -> BTreeMap<String, String> {
        let own = |id: &str| self.kind.info().options.iter().any(|o| o.id == id);
        let known = |id: &str| DbKind::ALL.iter().any(|k| k.info().options.iter().any(|o| o.id == id));
        self.options.iter().filter(|(id, _)| own(id) || !known(id)).map(|(id, v)| (id.clone(), v.clone())).collect()
    }

    /// Makes the form one of type `to`. Everything typed stays; a port that still holds the old
    /// type's default becomes the new one's. Another type forgets NEW FILE.
    pub(crate) fn switch_kind(&mut self, to: DbKind) {
        if to == self.kind {
            return;
        }
        if self.port.trim() == default_port(self.kind) {
            self.port = default_port(to);
        }
        self.kind = to;
        self.create_file = false;
    }

    /// The type's option `id` is on.
    pub(crate) fn option_on(&self, id: &str) -> bool {
        self.options.get(id).is_some_and(|v| v == "true")
    }

    /// Turns the type's option `id` on, or off (then it is left out).
    fn set_option(&mut self, id: &str, on: bool) {
        if on {
            self.options.insert(id.to_string(), "true".into());
        } else {
            self.options.remove(id);
        }
    }
}

// ---- drawing ---------------------------------------------------------------------------------

/// A field's label over what `add` draws under it (the colours, the choices, a file box, an input
/// with a button beside it), `width` wide.
fn column<R>(ui: &mut egui::Ui, width: f32, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        input::label(ui, label, |_| {});
        add(ui)
    })
    .inner
}

/// [`column`] with `note` after the label, on the label's row, cut to the room there is with the
/// whole note on hover; tagged `id`.
fn column_noted<R>(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    note: &str,
    tags: &mut Tags,
    id: &str,
    add: impl FnOnce(&mut egui::Ui, &mut Tags) -> R,
) -> R {
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        input::label(ui, label, |ui| {
            ui.add_space(8.0);
            let r = ui.add(Label::new(Text::Small.rich(note).color(theme::tokens(ui.ctx()).text_muted)).truncate());
            tags.tag_value(id, &r, note);
        });
        add(ui, tags)
    })
    .inner
}

/// The form's text fields: Enter in one of them connects.
const FORM_FIELDS: [&str; 9] =
    ["conn-name", "conn-host", "conn-port", "conn-user", "conn-password", "conn-database", "conn-ssh-host", "conn-ssh-port", "conn-ssh-user"];

pub(crate) fn field_id(id: &str) -> egui::Id {
    egui::Id::new(("dbee-form", id))
}

/// A field `width` wide: `label` over a one-line input of `text` (`font` is `Input::body` or
/// `Input::data`) showing `hint` while empty; tagged `id` with its text, its text edit's id from
/// [`field_id`].
fn text_field(
    ui: &mut egui::Ui,
    tags: &mut Tags,
    width: f32,
    label: &str,
    id: &'static str,
    font: fn(&mut String) -> Input<'_>,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    let r = ui.add(Field::new(label, font(text).id(field_id(id)).placeholder(hint).desired_width(width)));
    tags.tag_value(id, &r, text.as_str());
    r
}

/// `text` in `style` and `color` on one line in `slot`, from its left, centred in its height, cut
/// with "…" to its width (then the whole text shows on hover); tagged `id` with `value`.
fn slot_text(ui: &mut egui::Ui, tags: &mut Tags, id: &str, (text, style): (&str, Text), value: &str, slot: egui::Rect) {
    let t = theme::tokens(ui.ctx());
    let galley = line_galley(ui, text, style, t.text, slot.width());
    let elided = galley.elided;
    ui.painter().galley(pos2(slot.left(), slot.center().y - galley.size().y / 2.0), galley, t.text);
    let r = ui.interact(slot, egui::Id::new(id), Sense::hover());
    let r = if elided { r.hint(text) } else { r };
    tags.tag_value(id, &r, value);
}

/// A row of the pane, [`BAR_H`] tall across it with a 1 px `line` along its bottom; the room
/// above the line.
fn bar_row(ui: &mut egui::Ui) -> egui::Rect {
    let t = theme::tokens(ui.ctx());
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    let line = egui::Rect::from_min_max(pos2(row.left(), row.bottom() - BAR_LINE), row.max);
    ui.painter().rect_filled(line, CornerRadius::ZERO, t.line);
    egui::Rect::from_min_max(row.min, pos2(row.right(), line.top()))
}

/// The pane's title row: the form's title in `subtitle`, 12 in, cut with "…" to the room there
/// is; at the right end FILL FROM URL when `url` (a type the form can take has a URL), after its
/// divider. When the title would get narrower than [`CONN_TITLE_MIN_W`], FILL FROM URL draws its
/// icon alone, its words on hover.
pub(crate) fn title_row(ui: &mut egui::Ui, conn: &ConnScreen, url: bool, tags: &mut Tags, acts: &mut Vec<Act>) {
    let row = bar_row(ui);
    let mut end = row.right();
    if url {
        let full = BarButton::label(Icon::Link, "Fill from URL").over_line();
        let words = row.width() - INSET - full.width(ui) - INSET >= CONN_TITLE_MIN_W;
        let button = full.words(words);
        let w = button.width(ui);
        end = row.right() - w;
        // In a child: the row has taken its room already.
        let r = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_max(pos2(end, row.top()), row.max))).add(button);
        let r = if words { r } else { r.hint("Fill from URL…") };
        tags.tag("conn-fill-url", &r);
        if r.clicked() {
            acts.push(Act::FillFromUrl);
        }
    }
    let title = conn.form.title();
    let slot = egui::Rect::from_min_max(pos2(row.left() + INSET, row.top()), pos2((end - INSET).max(row.left() + INSET + 1.0), row.bottom()));
    slot_text(ui, tags, "conn-title", (&title, Text::Subtitle), &title, slot);
}

/// The form's engine row: the engine's logo and name, 12 in, the name cut with "…" to the room
/// there is; at the right end CHANGE ENGINE after its divider (not for a type that is fixed; off
/// while a test or connect runs).
pub(crate) fn engine_row(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>) {
    let t = theme::tokens(ui.ctx());
    let row = bar_row(ui);
    let mut end = row.right();
    if !conn.form.engine_fixed {
        let button = BarButton::text("Change engine").over_line();
        end = row.right() - button.width(ui);
        let at = egui::Rect::from_min_max(pos2(end, row.top()), row.max);
        let enabled = conn.running.is_none();
        let mut cell = ui.new_child(UiBuilder::new().max_rect(at));
        if !enabled {
            cell.disable();
        }
        let r = cell.add(button);
        let r = if enabled { r } else { r.hint_disabled("Change engine: not now, a test or connect is running") };
        tags.tag("conn-change-engine", &r);
        if r.clicked() {
            acts.push(Act::ChangeEngine);
        }
    }
    let kind = conn.form.kind;
    let logo = egui::Rect::from_center_size(pos2(row.left() + INSET + ENGINE_ROW_LOGO / 2.0, row.center().y), vec2(ENGINE_ROW_LOGO, ENGINE_ROW_LOGO));
    Logo::of(kind).paint(ui.painter(), logo, t.text);
    let left = logo.right() + ENGINE_ROW_GAP;
    let slot = egui::Rect::from_min_max(pos2(left, row.top()), pos2((end - INSET).max(left + 1.0), row.bottom()));
    slot_text(ui, tags, "conn-kind", (kind.label(), Text::Subtitle), kind.id(), slot);
}

/// How the form's sections fit in a pane `pane_w` wide: in two columns from
/// [`CONN_TWO_COLUMNS_MIN`], else in one; and a column's width, never more than
/// [`CONN_COLUMN_MAX_W`] (the room past the columns stays empty). The columns start [`INSET`] in
/// from the pane's left, [`CONN_COLUMN_GAP`] apart.
pub(crate) fn columns(pane_w: f32) -> (usize, f32) {
    if pane_w >= CONN_TWO_COLUMNS_MIN {
        (2, ((pane_w - 2.0 * INSET - CONN_COLUMN_GAP) / 2.0).min(CONN_COLUMN_MAX_W))
    } else {
        (1, (pane_w - 2.0 * INSET).clamp(1.0, CONN_COLUMN_MAX_W))
    }
}

/// What is under the engine row, across the pane: the fields in [`columns`] (one column holds the
/// second's sections under the first's), which scroll when they need more room; then the strip
/// (an error, a result or a note), and the form's bar along the bottom. The strip takes its room
/// from the fields' view, so neither the fields nor the bar move. `saved`: the form shows a saved
/// connection (its bar has `…`). Enter in a field connects.
pub(crate) fn body(ui: &mut egui::Ui, conn: &mut ConnScreen, saved: bool, tags: &mut Tags, acts: &mut Vec<Act>) {
    if conn.databases.open && keyboard_left(ui, &conn.databases) {
        conn.databases.open = false;
    }
    if conn.databases.open {
        list_keys(ui, &mut conn.databases, acts);
    }
    let in_field = ui.ctx().memory(|m| m.focused()).is_some_and(|focused| FORM_FIELDS.iter().any(|id| field_id(id) == focused));
    if in_field && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        acts.push(Act::ConnectForm);
    }
    let area = ui.available_rect_before_wrap();
    let bar = egui::Rect::from_min_max(pos2(area.left(), (area.bottom() - BAR_H).max(area.top())), area.max);
    let strip_h = conn.message.as_ref().map_or(0.0, |m| strip_h(ui, m, area.width()));
    let strip = egui::Rect::from_min_max(pos2(area.left(), (bar.top() - strip_h).max(area.top())), pos2(area.right(), bar.top()));
    let fields = egui::Rect::from_min_max(area.min, pos2(area.right(), strip.top()));
    fields_in(ui, conn, tags, acts, fields);
    if let Some(m) = &conn.message {
        strip_in(&mut cell_ui(ui, strip), m, tags);
    }
    form_bar(ui, conn, saved, tags, acts, bar);
    ui.advance_cursor_after_rect(area);
}

/// The fields in `at`, [`INSET`] in from its left and [`CONN_FIELDS_TOP`] under its top; their
/// scroll bar runs along its right edge.
fn fields_in(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, at: egui::Rect) {
    let (n, w) = columns(at.width());
    let room = egui::Rect::from_min_max(pos2(at.left() + INSET, (at.top() + CONN_FIELDS_TOP).min(at.bottom())), at.max);
    let mut ui = ui.new_child(UiBuilder::new().id_salt("conn-fields").max_rect(room).layout(Layout::top_down(Align::Min)));
    ui.set_clip_rect(at.intersect(ui.clip_rect()));
    ScrollArea::vertical().id_salt("conn-form-scroll").max_height(room.height()).auto_shrink([false, false]).show(&mut ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if n == 2 {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = CONN_COLUMN_GAP;
                stack(ui, w, |ui| first_column(ui, conn, tags, acts, w));
                stack(ui, w, |ui| second_column(ui, conn, tags, acts, w));
            });
        } else {
            stack(ui, w, |ui| {
                first_column(ui, conn, tags, acts, w);
                ui.add_space(CONN_SECTION_GAP);
                second_column(ui, conn, tags, acts, w);
            });
        }
        ui.add_space(INSET);
    });
}

/// How tall the strip for `m` is across `w`: an error band's heading and up to three lines of its
/// message, or a [`Strip`]'s.
fn strip_h(ui: &egui::Ui, m: &Message, w: f32) -> f32 {
    match m.kind {
        Kind::Error => {
            let words_w = w - ERROR_BAND_EDGE - 2.0 * ERROR_BAND_PAD;
            2.0 * ERROR_BAND_PAD + Text::Subtitle.line() + ERROR_BAND_GAP + strip::lines_h(ui, Text::Body, &m.text, words_w)
        }
        Kind::Result | Kind::Note => strip_of(m).height(ui, w),
    }
}

/// A result or a note as a [`Strip`].
fn strip_of(m: &Message) -> Strip<'_> {
    let tone = if m.kind == Kind::Result { Tone::Success } else { Tone::Neutral };
    Strip { tone, text: &m.text }
}

/// The strip across `ui`: an error as the error band under its heading (`conn-error`, and
/// `conn-error-heading`), a result (`conn-result`) or a note (`conn-note`) as a [`Strip`]; each
/// tagged with its words. A long error scrolls inside it.
fn strip_in(ui: &mut egui::Ui, m: &Message, tags: &mut Tags) {
    let height = ui.max_rect().height();
    let id = egui::Id::new("conn-strip");
    let r = match m.kind {
        Kind::Error => {
            ScrollArea::vertical()
                .id_salt("conn-band")
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let band = Band { heading: m.heading, message: &m.text, ..Band::default() };
                    band.show(ui, id).response
                })
                .inner
        }
        Kind::Result | Kind::Note => strip_of(m).show(ui, id),
    };
    // The strip's place, also when its band scrolls.
    let r = r.with_new_rect(ui.max_rect());
    let tag = match m.kind {
        Kind::Error => {
            tags.tag_value("conn-error-heading", &r, m.heading);
            "conn-error"
        }
        Kind::Result => "conn-result",
        Kind::Note => "conn-note",
    };
    tags.tag_value(tag, &r, m.text.as_str());
}

/// The form's bar in `at`, along the pane's bottom on `card` with a 1 px `line` on top: SAVE (a
/// text button) [`INSET`] in from the left; at the right, ending [`INSET`] from the pane's edge,
/// `…` (a saved connection's: Duplicate, Delete), TEST and CONNECT, [`CONN_BESIDE_GAP`] apart.
/// TEST and CONNECT are off while a test or connect runs, and as wide for both their words.
fn form_bar(ui: &mut egui::Ui, conn: &ConnScreen, saved: bool, tags: &mut Tags, acts: &mut Vec<Act>, at: egui::Rect) {
    let t = theme::tokens(ui.ctx());
    let p = ui.painter();
    p.rect_filled(at, CornerRadius::ZERO, t.card);
    p.rect_filled(egui::Rect::from_min_size(at.min, vec2(at.width(), BAR_LINE)), CornerRadius::ZERO, t.line);
    let row = egui::Rect::from_min_max(pos2(at.left(), at.top() + BAR_LINE), at.max);
    let running = conn.running.map(|(_, kind)| kind);
    let place = |x: f32, w: f32| egui::Rect::from_min_size(pos2(x, row.center().y - BUTTON_H / 2.0), vec2(w, BUTTON_H));

    let save = TextButton::new("Save");
    let save_at = place(row.left() + INSET, save.width(ui));
    let r = ui.new_child(UiBuilder::new().max_rect(save_at).layout(Layout::left_to_right(Align::Center))).add(save);
    tags.tag_value("conn-save", &r, "Save");
    if r.clicked() {
        acts.push(Act::Save);
    }

    // From the right: CONNECT, TEST, then `…`; drawn left to right so Tab reads them so.
    let connect_w = BoxButton::primary("Connect").width(ui).max(BoxButton::primary("Connecting…").width(ui));
    let test_w = BoxButton::outline("Test").width(ui).max(BoxButton::outline("Testing…").width(ui));
    let connect_at = place(row.right() - INSET - connect_w, connect_w);
    let test_at = place(connect_at.left() - CONN_BESIDE_GAP - test_w, test_w);
    if saved {
        let more = BoxButton::outline("…");
        let more_at = place(test_at.left() - CONN_BESIDE_GAP - more.width(ui), more.width(ui));
        let r = button_in(ui, more_at, true, more).hint("Duplicate or delete this connection");
        tags.tag("conn-more", &r);
        more_menu(&r, conn, tags, acts);
    }
    let label = if running == Some(Running::Test) { "Testing…" } else { "Test" };
    let r = button_in(ui, test_at, running.is_none(), BoxButton::outline(label));
    tags.tag_value("conn-test", &r, label);
    if r.clicked() {
        acts.push(Act::Test);
    }
    let label = if running == Some(Running::Connect) { "Connecting…" } else { "Connect" };
    let r = button_in(ui, connect_at, running.is_none(), BoxButton::primary(label));
    tags.tag_value("conn-connect", &r, label);
    if r.clicked() {
        acts.push(Act::ConnectForm);
    }
}

/// `…`'s menu, under it and right-aligned: Duplicate, then Delete (which asks first), as the
/// row's menu has them.
fn more_menu(button: &egui::Response, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>) {
    let id = conn.form.id.clone();
    Popup::menu(button).align(RectAlign::BOTTOM_END).style(menu::style(&button.ctx)).show(|ui| {
        for (label, item, destructive) in [("Duplicate", "conn-more-duplicate", false), ("Delete", "conn-more-delete", true)] {
            if destructive {
                menu::separator(ui);
            }
            let r = ui.add(Item::new(label).destructive(destructive));
            // The menu's first frame is a sizing pass, which reads every widget as disabled.
            tags.tag_value_as(item, &r, label, true);
            if r.clicked() {
                acts.push(if destructive { Act::AskDelete { id: id.clone(), name: conn.form.title() } } else { Act::Duplicate(id.clone()) });
                ui.close();
            }
        }
    });
}

/// A column `w` wide of what `add` draws, top to bottom.
fn stack(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(w);
        add(ui);
    });
}

/// A section's label (`label` caps, `text_muted`), its first field [`CONN_FIELD_GAP`] under it.
fn section(ui: &mut egui::Ui, words: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add(Label::new(Text::Label.rich(words).color(t.text_muted)).truncate());
    ui.add_space(CONN_FIELD_GAP);
}

/// A field's height: its label, the gap, its input.
fn field_h() -> f32 {
    Text::Label.line() + FIELD_GAP + INPUT_H
}

/// A row `w` wide and `h` tall, split into a cell that takes what is left and, at the row's right
/// end, a cell for each of `widths`, `gap` apart: the first cell and the others.
fn cells<const N: usize>(ui: &mut egui::Ui, w: f32, h: f32, gap: f32, widths: [f32; N]) -> (egui::Rect, [egui::Rect; N]) {
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut right = row.right();
    let mut beside = [egui::Rect::NOTHING; N];
    for (cell, width) in beside.iter_mut().zip(widths).rev() {
        *cell = egui::Rect::from_min_max(pos2(right - width, row.top()), pos2(right, row.bottom()));
        right -= width + gap;
    }
    (egui::Rect::from_min_max(row.min, pos2(right.max(row.left()), row.bottom())), beside)
}

/// A child of `ui` drawing in `at`, top to bottom.
fn cell_ui(ui: &mut egui::Ui, at: egui::Rect) -> egui::Ui {
    ui.new_child(UiBuilder::new().max_rect(at).layout(Layout::top_down(Align::Min)))
}

/// `button` in the cell `at`, as wide as it, on while `enabled`.
fn button_in(ui: &mut egui::Ui, at: egui::Rect, enabled: bool, button: BoxButton<'_>) -> egui::Response {
    cell_ui(ui, at).add_enabled(enabled, button.min_width(at.width()))
}

/// A button's width for the longer of its two words, so nothing beside it moves when they change.
fn either_w(ui: &egui::Ui, a: &str, b: &str) -> f32 {
    BoxButton::outline(a).width(ui).max(BoxButton::outline(b).width(ui))
}

/// A read-only box in `at` showing `text` in `color` on one line, cut with "…" (the whole text on
/// hover); its response, as `id`.
fn path_box(ui: &mut egui::Ui, at: egui::Rect, id: &str, text: &str, color: egui::Color32) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let p = ui.painter();
    p.rect(at, CornerRadius::same(RADIUS_CONTROL), t.bg, Stroke::new(1.0, t.line_strong), egui::StrokeKind::Inside);
    let galley = line_galley(ui, text, Text::Body, color, at.width() - 2.0 * (INPUT_PAD_X + 1.0));
    let elided = galley.elided;
    ui.painter().galley(pos2(at.left() + INPUT_PAD_X + 1.0, at.center().y - galley.size().y / 2.0), galley, color);
    let r = ui.interact(at, egui::Id::new(("dbee-form-box", id)), Sense::hover());
    if elided { r.hint(text) } else { r }
}

/// A file box's words: `Waiting for the file picker…` while its picker is open, else the path (the
/// home folder as `~`), else `None`.
fn path_words(path: Option<&Path>, waiting: bool, t: &theme::Tokens) -> (String, egui::Color32) {
    if waiting {
        return ("Waiting for the file picker…".to_string(), t.text_muted);
    }
    match path {
        Some(p) => (tilde(&p.display().to_string()), t.text),
        None => ("None".to_string(), t.text_muted),
    }
}

/// CONNECTION's rows (name, colour, then a server's host, port and database and AUTHENTICATION,
/// or a file type's file).
fn first_column(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    section(ui, "Connection");
    let f = &mut conn.form;
    text_field(ui, tags, w, "Name", "conn-name", Input::body, &mut f.name, "Optional");
    ui.add_space(CONN_FIELD_GAP);
    colour_row(ui, f, tags, w);
    ui.add_space(CONN_FIELD_GAP);
    match conn.form.kind.info().settings {
        Settings::Server { .. } => server_rows(ui, conn, tags, acts, w),
        Settings::File { .. } => file_row(ui, conn, tags, acts, w),
    }
}

/// SECURITY (a server's SSL rows when it takes TLS, Read-only and the type's options), then SSH
/// TUNNEL when the type takes one.
fn second_column(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    section(ui, "Security");
    match conn.form.kind.info().settings {
        Settings::Server { tls, ssh, .. } => {
            if tls {
                tls_rows(ui, conn, tags, acts, w);
                ui.add_space(CONN_FIELD_GAP);
            }
            read_only_and_options(ui, &mut conn.form, tags, w);
            if ssh {
                ui.add_space(CONN_SECTION_GAP);
                section(ui, "SSH tunnel");
                ssh_rows(ui, conn, tags, acts, w);
            }
        }
        Settings::File { .. } => read_only_and_options(ui, &mut conn.form, tags, w),
    }
}

/// The picker the form waits for (not one the user stopped waiting for).
fn picking(conn: &ConnScreen) -> Option<Pick> {
    conn.picker.as_ref().filter(|p| !p.stopped).map(|p| p.target)
}

/// The colours: none and the seven, as swatches.
fn colour_row(ui: &mut egui::Ui, f: &mut Form, tags: &mut Tags, w: f32) {
    let n = swatch::COLOURS.len() as f32;
    let swatches_w = SWATCH * n + SWATCH_GAP * (n - 1.0);
    column(ui, w, "Colour", |ui| {
        let row = ui.allocate_ui_with_layout(vec2(swatches_w, INPUT_H), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = SWATCH_GAP;
            for colour in swatch::COLOURS {
                let (name, chosen) = (swatch::name(colour), f.color == colour);
                let r = ui.add(Swatch::new(colour, chosen)).hint(name);
                tags.tag_value(&format!("conn-color-{name}"), &r, chosen);
                if r.clicked() {
                    f.color = colour;
                }
            }
        });
        tags.tag_value("conn-color", &row.response, swatch::name(f.color));
    });
}

/// `label` over an input in the cell `at` (as [`text_field`]).
#[allow(clippy::too_many_arguments)]
fn field_in(
    ui: &mut egui::Ui,
    at: egui::Rect,
    tags: &mut Tags,
    label: &str,
    id: &'static str,
    font: fn(&mut String) -> Input<'_>,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    text_field(&mut cell_ui(ui, at), tags, at.width(), label, id, font, text, hint)
}

/// Host and port, the database, then AUTHENTICATION: user, password with SHOW, "Save passwords".
/// Tab reaches them in that order, as they read.
fn server_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = theme::tokens(ui.ctx());
    let has_store = conn.has_store;
    let f = &mut conn.form;
    let (host, [port]) = cells(ui, w, field_h(), CONN_FIELD_GAP, [CONN_PORT_W]);
    field_in(ui, host, tags, "Host", "conn-host", Input::data, &mut f.host, "localhost");
    field_in(ui, port, tags, "Port", "conn-port", Input::data, &mut f.port, &default_port(f.kind));
    ui.add_space(CONN_FIELD_GAP);
    let (database, _) = ui.allocate_exact_size(vec2(w, field_h()), Sense::hover());
    database_row(&mut cell_ui(ui, database), conn, tags, acts, w);
    let f = &mut conn.form;

    ui.add_space(CONN_SECTION_GAP);
    section(ui, "Authentication");
    text_field(ui, tags, w, "User", "conn-user", Input::data, &mut f.user, "");
    ui.add_space(CONN_FIELD_GAP);
    column(ui, w, "Password", |ui| {
        // As wide for both words, so the field keeps its width; it ends where the column ends.
        let show_w = either_w(ui, "Show", "Hide");
        let (field, [show]) = cells(ui, w, INPUT_H, CONN_BESIDE_GAP, [show_w]);
        let password = Input::data(&mut f.password).id(field_id("conn-password")).password(!f.show_password);
        let r = cell_ui(ui, field).add(password.desired_width(field.width()));
        tags.tag_secret("conn-password", &r, !f.password.is_empty());
        let label = if f.show_password { "Hide" } else { "Show" };
        let s = button_in(ui, show, true, BoxButton::outline(label));
        tags.tag_value("conn-password-show", &s, f.show_password);
        if s.clicked() {
            f.show_password = !f.show_password;
        }
    });
    ui.add_space(CONN_FIELD_GAP);
    // Save passwords, or why it cannot.
    ui.horizontal(|ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.x = CONN_BESIDE_GAP;
        if has_store {
            let r = ui.add(Checkbox::new(&mut f.save_secrets, "Save passwords in the keyring"));
            tags.tag_value("conn-save-secrets", &r, f.save_secrets);
        } else {
            let mut off = false;
            let r = ui.add_enabled(false, Checkbox::new(&mut off, "Save passwords"));
            tags.tag_value("conn-save-secrets", &r, false);
            let note = "No keyring found, so passwords are asked each time.";
            let r = ui.add(Label::new(Text::Small.rich(note).color(t.text_muted)).truncate());
            tags.tag_value("conn-no-keyring", &r, note);
        }
    });
}

/// The keyboard is on a widget other than the Database field and the list's button. On none at
/// all it has not left: a click on the button leaves no widget with it.
fn keyboard_left(ui: &egui::Ui, list: &DatabaseList) -> bool {
    ui.ctx().memory(|m| m.focused()).is_some_and(|id| id != field_id("conn-database") && Some(id) != list.button)
}

/// The open database list's keys, taken before the fields see them, as the quick search takes its
/// own: Down and Up move the highlight (it stays at the ends), Enter picks the highlighted name,
/// Escape closes the list.
fn list_keys(ui: &egui::Ui, list: &mut DatabaseList, acts: &mut Vec<Act>) {
    let (down, up, enter, escape) = ui.input_mut(|i| {
        (
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if escape {
        list.open = false;
        return;
    }
    if down > 0 || up > 0 {
        // egui would also move the keyboard to the widget above or below.
        ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        let last = list.names.len().saturating_sub(1);
        list.highlight = list.highlight.saturating_add(down).saturating_sub(up).min(last);
        list.reveal = true;
    }
    if enter && let Some(name) = list.names.get(list.highlight) {
        acts.push(Act::ChooseDatabase(name.clone()));
    }
}

/// The scroll offset that brings row `n` of a list into a view `view_h` tall, scrolled to
/// `offset`, moving it as little as it can.
fn reveal_offset(n: usize, offset: f32, view_h: f32) -> f32 {
    let top = n as f32 * MENU_ITEM_H;
    let bottom = top + MENU_ITEM_H;
    if top < offset {
        top
    } else if bottom > offset + view_h {
        bottom - view_h
    } else {
        offset
    }
}

/// The database, and beside it the button that lists the server's databases (not while a test,
/// a connect or a listing runs). The names open under the field, over the rows below; a click
/// or Enter on the highlighted one fills it in. Escape, a click elsewhere or typing in the field
/// closes them. The names take no keyboard focus: Down and Up move through them, and when they
/// open the field takes the keyboard if no widget has it.
fn database_row(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let listing = conn.databases.pending.is_some();
    let can_list = conn.running.is_none() && !listing;
    let ConnScreen { form, databases, .. } = conn;
    column(ui, w, "Database", |ui| {
        let list_w = either_w(ui, "List…", "Listing…");
        let (cell, [list]) = cells(ui, w, INPUT_H, CONN_BESIDE_GAP, [list_w]);
        let field_w = cell.width();
        let field = cell_ui(ui, cell).add(Input::data(&mut form.database).id(field_id("conn-database")).desired_width(field_w));
        tags.tag_value("conn-database", &field, form.database.as_str());
        if field.changed() {
            databases.open = false;
        }
        if std::mem::take(&mut databases.take_keyboard) && databases.open && ui.memory(|m| m.focused()).is_none() {
            field.request_focus();
        }
        let label = if listing { "Listing…" } else { "List…" };
        let button = button_in(ui, list, can_list, BoxButton::outline(label)).hint("Choose from the server's databases");
        tags.tag_value("conn-database-list", &button, label);
        databases.button = Some(button.id);
        if button.clicked() {
            if databases.open {
                databases.open = false;
            } else {
                acts.push(Act::ListDatabases);
            }
        }
        let DatabaseList { names, open, highlight, offset, view_h, reveal, .. } = databases;
        let mut scroll = ScrollArea::vertical().max_height(LIST_H).auto_shrink([false, true]);
        if std::mem::take(reveal) {
            let view = if *view_h > 0.0 { *view_h } else { LIST_H };
            scroll = scroll.vertical_scroll_offset(reveal_offset(*highlight, *offset, view));
        }
        Popup::from_response(&field)
            .open_bool(open)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .sense(Sense::CLICK)
            .align(RectAlign::BOTTOM_START)
            .width(field_w)
            .style(menu::style(ui.ctx()))
            .show(|ui| {
                ui.set_min_width(ui.available_width());
                let shown = scroll.show_rows(ui, MENU_ITEM_H, names.len(), |ui, rows| {
                    for (n, name) in names.iter().enumerate().skip(rows.start).take(rows.len()) {
                        let r = ui.add(Item::new(name).highlighted(n == *highlight).focusable(false));
                        // A popup's first frame is a sizing pass, which reads every widget as disabled.
                        tags.tag_value_as(&format!("conn-database-item-{n}"), &r, name.as_str(), true);
                        if r.clicked() {
                            acts.push(Act::ChooseDatabase(name.clone()));
                        }
                    }
                });
                *offset = shown.state.offset.y;
                *view_h = shown.inner_rect.height();
            });
    });
}

/// What each SSL mode protects against, shown on its label's row (`conn-ssl-note`).
fn ssl_note(mode: SslMode) -> &'static str {
    match mode {
        SslMode::Disable => "No encryption: anyone on the network can read and change what is sent.",
        SslMode::Prefer => "Encrypts when the server offers TLS, but does not check who answers.",
        SslMode::Require => "Encrypts, but does not check who answers; with a CA file, checks the certificate against it, but not the server's name.",
        SslMode::VerifyCa => "Encrypts, and checks the certificate against the CA file, but not the server's name.",
        SslMode::VerifyFull => "Encrypts, and checks the server's certificate and its name.",
    }
}

/// SSL mode, what it checks, and the CA file with CHOOSE (STOP while its picker is open) and
/// CLEAR.
fn tls_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = theme::tokens(ui.ctx());
    let picking = picking(conn);
    let f = &mut conn.form;
    // Each mode's words are its name.
    let modes = [
        (SslMode::Disable, "disable"),
        (SslMode::Prefer, "prefer"),
        (SslMode::Require, "require"),
        (SslMode::VerifyCa, "verify-ca"),
        (SslMode::VerifyFull, "verify-full"),
    ];
    let note = ssl_note(f.ssl_mode);
    column_noted(ui, w, "SSL mode", note, tags, "conn-ssl-note", |ui, tags| {
        let shown = Segmented::small(&mut f.ssl_mode, &modes).width(w).show(ui);
        for ((mode, name), r) in modes.iter().zip(&shown.segments) {
            tags.tag_value(&format!("conn-ssl-{name}"), r, f.ssl_mode == *mode);
        }
        let chosen = modes.iter().find(|(m, _)| *m == f.ssl_mode).map_or("", |(_, name)| *name);
        tags.tag_value("conn-ssl", &shown.row, chosen);
    });
    ui.add_space(CONN_FIELD_GAP);
    column(ui, w, "CA file (optional)", |ui| {
        let waiting = picking == Some(Pick::Ca);
        let (cell, [choose, clear]) = cells(ui, w, INPUT_H, CONN_BESIDE_GAP, [either_w(ui, "Choose", "Stop"), BoxButton::outline("Clear").width(ui)]);
        let (text, color) = path_words(f.ca_file.as_deref(), waiting, t);
        let r = path_box(ui, cell, "conn-ca", &text, color);
        tags.tag_value("conn-ca", &r, f.ca_file.as_ref().map_or(Json::Null, |p| Json::from(tilde(&p.display().to_string()))));
        let label = if waiting { "Stop" } else { "Choose" };
        let b = button_in(ui, choose, picking.is_none() || waiting, BoxButton::outline(label));
        tags.tag("conn-ca-choose", &b);
        if b.clicked() {
            acts.push(if waiting { Act::StopPicking } else { Act::Pick(Pick::Ca) });
        }
        let b = button_in(ui, clear, f.ca_file.is_some(), BoxButton::outline("Clear"));
        tags.tag("conn-ca-clear", &b);
        if b.clicked() {
            acts.push(Act::ClearCa);
        }
    });
}

/// The database file, with CHOOSE and NEW FILE (each STOP while its picker is open): the pickers
/// show the type's extensions first.
fn file_row(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = theme::tokens(ui.ctx());
    let picking = picking(conn);
    let f = &conn.form;
    column(ui, w, "File", |ui| {
        let waiting = picking.filter(|p| matches!(p, Pick::File | Pick::NewFile));
        let (cell, buttons) = cells(ui, w, INPUT_H, CONN_BESIDE_GAP, [either_w(ui, "Choose", "Stop"), either_w(ui, "New file", "Stop")]);
        let (text, color) = path_words(f.file.as_deref(), waiting.is_some(), t);
        let r = path_box(ui, cell, "conn-file", &text, color);
        tags.tag_value("conn-file", &r, f.file.as_ref().map_or(Json::Null, |p| Json::from(tilde(&p.display().to_string()))));
        for ((pick, id, label), at) in
            [(Pick::File, "conn-file-choose", "Choose"), (Pick::NewFile, "conn-file-new", "New file")].into_iter().zip(buttons)
        {
            let this = waiting == Some(pick);
            let b = button_in(ui, at, picking.is_none() || this, BoxButton::outline(if this { "Stop" } else { label }));
            tags.tag(id, &b);
            if b.clicked() {
                acts.push(if this { Act::StopPicking } else { Act::Pick(pick) });
            }
        }
    });
}

/// What Read-only does, and what it is not.
const READ_ONLY_NOTE: &str = "Read-only guards against mistakes: dbee refuses edits, and the statements that would turn read-only off. It is \
                              not a lock: to be sure nothing is written, connect as a database user without write rights.";

/// Read-only, while it is ticked what it guards against (`conn-readonly-note`), then the type's
/// own on/off settings, one check box a line (`conn-option-<id>`).
fn read_only_and_options(ui: &mut egui::Ui, f: &mut Form, tags: &mut Tags, w: f32) {
    let r = ui.add(Checkbox::new(&mut f.read_only, "Read-only"));
    tags.tag_value("conn-readonly", &r, f.read_only);
    if f.read_only {
        ui.add_space(FIELD_GAP);
        let note = Label::new(Text::Small.rich(READ_ONLY_NOTE).color(theme::tokens(ui.ctx()).text_muted)).wrap();
        let r = ui.allocate_ui(vec2(w, 0.0), |ui| ui.add(note)).inner;
        tags.tag_value("conn-readonly-note", &r, READ_ONLY_NOTE);
    }
    for option in f.kind.info().options {
        ui.add_space(CONN_FIELD_GAP);
        let mut on = f.option_on(option.id);
        let r = ui.add(Checkbox::new(&mut on, option.label));
        if r.changed() {
            f.set_option(option.id, on);
        }
        tags.tag_value(&format!("conn-option-{}", option.id), &r, on);
    }
}

/// The SSH tunnel: on or off, then host, port, user, the login, and one row of fixed height for
/// what the login uses: KEY FILE with CHOOSE and CLEAR for a key, USES and the key line for the
/// agent and a password. `conn-ssh-key` holds the key line's words whatever the login.
fn ssh_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = theme::tokens(ui.ctx());
    let picking = picking(conn);
    let ConnScreen { form: f, key, .. } = conn;
    let r = ui.add(Checkbox::new(&mut f.ssh_on, "Connect through an SSH tunnel"));
    tags.tag_value("conn-ssh-on", &r, f.ssh_on);
    if !f.ssh_on {
        return;
    }
    ui.add_space(CONN_FIELD_GAP);
    let (host, [port]) = cells(ui, w, field_h(), CONN_FIELD_GAP, [CONN_PORT_W]);
    field_in(ui, host, tags, "SSH host", "conn-ssh-host", Input::data, &mut f.ssh_host, "bastion.example.com");
    field_in(ui, port, tags, "Port", "conn-ssh-port", Input::data, &mut f.ssh_port, "22");
    ui.add_space(CONN_FIELD_GAP);
    text_field(ui, tags, w, "User", "conn-ssh-user", Input::data, &mut f.ssh_user, "");
    ui.add_space(CONN_FIELD_GAP);
    // Each login's words are its name.
    let auths = [(Auth::Key, "key"), (Auth::Agent, "agent"), (Auth::Password, "password")];
    column(ui, w, "Login", |ui| {
        let shown = Segmented::small(&mut f.ssh_auth, &auths).width(w).show(ui);
        for ((auth, name), r) in auths.iter().zip(&shown.segments) {
            tags.tag_value(&format!("conn-ssh-auth-{name}"), r, f.ssh_auth == *auth);
        }
        let chosen = auths.iter().find(|(a, _)| *a == f.ssh_auth).map_or("", |(_, name)| *name);
        tags.tag_value("conn-ssh-auth", &shown.row, chosen);
    });
    ui.add_space(CONN_FIELD_GAP);
    let waiting = picking == Some(Pick::Key);
    let wanted = f.ssh_auth();
    let line = if waiting {
        "Waiting for the file picker…".to_string()
    } else if key.for_auth.as_ref() == Some(&wanted) {
        key.text.clone()
    } else {
        "Looking for the key…".to_string()
    };
    if f.ssh_auth != Auth::Key {
        column(ui, w, "Uses", |ui| {
            let (slot, _) = ui.allocate_exact_size(vec2(w, INPUT_H), Sense::hover());
            let galley = line_galley(ui, &line, Text::Body, t.text_muted, slot.width());
            let elided = galley.elided;
            ui.painter().galley(pos2(slot.left(), slot.center().y - galley.size().y / 2.0), galley, t.text_muted);
            let r = ui.interact(slot, egui::Id::new(("dbee-form-box", "conn-ssh-key")), Sense::hover());
            let r = if elided { r.hint(line.as_str()) } else { r };
            tags.tag_value("conn-ssh-key", &r, line.as_str());
        });
        return;
    }
    column(ui, w, "Key file", |ui| {
        let (cell, [choose, clear]) = cells(ui, w, INPUT_H, CONN_BESIDE_GAP, [either_w(ui, "Choose", "Stop"), BoxButton::outline("Clear").width(ui)]);
        // The chosen file as it is; with none chosen, the key that will be found.
        let (text, color) = match (&f.ssh_key, waiting) {
            (Some(p), false) => (tilde(&p.display().to_string()), t.text),
            _ => (line.clone(), t.text_muted),
        };
        let r = path_box(ui, cell, "conn-ssh-key", &text, color);
        tags.tag_value("conn-ssh-key", &r, line.as_str());
        let b = button_in(ui, choose, picking.is_none() || waiting, BoxButton::outline(if waiting { "Stop" } else { "Choose" }));
        tags.tag("conn-ssh-change", &b);
        if b.clicked() {
            acts.push(if waiting { Act::StopPicking } else { Act::Pick(Pick::Key) });
        }
        let chosen = f.ssh_key.is_some();
        let b = button_in(ui, clear, chosen, BoxButton::outline("Clear"));
        let auto = "Use the agent, then ~/.ssh/id_ed25519, id_ecdsa or id_rsa";
        let b = if chosen { b.hint(auto) } else { b.hint_disabled(auto) };
        tags.tag("conn-ssh-auto", &b);
        if b.clicked() {
            acts.push(Act::AutoKey);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_takes_two_columns_from_664() {
        assert_eq!(columns(840.0), (2, 396.0));
        assert_eq!(columns(664.0), (2, 308.0));
        assert_eq!(columns(663.0), (1, 480.0));
        assert_eq!(columns(380.0), (1, 356.0));
        assert_eq!(columns(2000.0).1, 480.0);
    }

    #[test]
    fn saving_keeps_the_options() {
        let mut mysql = ConnectionConfig { id: "a".into(), kind: DbKind::MySql, port: 3306, ..ConnectionConfig::default() };
        mysql.options.insert(dbee_config::ALLOW_PUBLIC_KEY.id.into(), "true".into());
        mysql.options.insert("aLaterOption".into(), "x".into());
        assert_eq!(Form::from_config(&mysql).to_config().unwrap().options, mysql.options);
    }

    #[test]
    fn saving_keeps_the_type_and_the_file() {
        let mysql = ConnectionConfig { id: "a".into(), kind: DbKind::MySql, port: 3306, ..ConnectionConfig::default() };
        let back = Form::from_config(&mysql).to_config().unwrap();
        assert_eq!((back.kind, back.file), (DbKind::MySql, None));

        let file = Some(PathBuf::from("/x/shop.sqlite"));
        let sqlite = ConnectionConfig {
            id: "b".into(),
            kind: DbKind::Sqlite,
            file: file.clone(),
            host: String::new(),
            port: 0,
            ..ConnectionConfig::default()
        };
        let back = Form::from_config(&sqlite).to_config().unwrap();
        assert_eq!((back.kind, back.file), (DbKind::Sqlite, file));
    }

    #[test]
    fn a_type_switch_moves_only_a_default_port() {
        let mut f = Form::new(true);
        f.name = "Shop".into();
        f.color = ConnectionColor::Red;
        assert_eq!((f.kind, f.port.as_str()), (DbKind::Postgres, "5432"));
        f.switch_kind(DbKind::MySql);
        assert_eq!((f.kind, f.port.as_str()), (DbKind::MySql, "3306"));
        f.switch_kind(DbKind::MariaDb);
        assert_eq!(f.port, "3306");
        f.port = "5544".into();
        f.switch_kind(DbKind::Postgres);
        assert_eq!(f.port, "5544", "a typed port stays");
        f.port = " 5432 ".into();
        f.switch_kind(DbKind::Sqlite);
        assert_eq!(f.port, "", "a file has no port");
        f.switch_kind(DbKind::MySql);
        assert_eq!(f.port, "3306");
        assert_eq!((f.name.as_str(), f.color), ("Shop", ConnectionColor::Red));
    }

    #[test]
    fn the_title_names_a_saved_connection_as_its_card_does() {
        assert_eq!(Form::new(true).title(), "New connection");
        let file = ConnectionConfig {
            id: "a".into(),
            kind: DbKind::Sqlite,
            file: Some(PathBuf::from("/x/shop.sqlite")),
            host: String::new(),
            port: 0,
            ..ConnectionConfig::default()
        };
        let form = Form::from_config(&file);
        assert_eq!((form.title(), form.port.as_str()), ("shop.sqlite".to_string(), ""));
        let server =
            ConnectionConfig { id: "b".into(), user: "ann".into(), host: "db".into(), database: "shop".into(), ..ConnectionConfig::default() };
        assert_eq!(Form::from_config(&server).title(), "ann@db/shop");
        assert_eq!(Form::from_config(&ConnectionConfig { name: " Shop ".into(), ..server }).title(), "Shop");
    }

    #[test]
    fn an_option_is_true_or_left_out() {
        let mut f = Form::new(true);
        f.options.insert("aLaterOption".into(), "x".into());
        f.set_option(dbee_config::ALLOW_PUBLIC_KEY.id, true);
        assert!(f.option_on(dbee_config::ALLOW_PUBLIC_KEY.id));
        assert_eq!(f.options.get("allowPublicKey").map(String::as_str), Some("true"));
        f.set_option(dbee_config::ALLOW_PUBLIC_KEY.id, false);
        assert!(!f.option_on(dbee_config::ALLOW_PUBLIC_KEY.id));
        assert_eq!(f.options.keys().collect::<Vec<_>>(), ["aLaterOption"], "unknown keys stay");
    }

    #[test]
    fn a_config_holds_only_what_its_type_uses() {
        let mut f = Form::new(true);
        f.name = "Shop".into();
        f.user = "ann".into();
        f.database = "shop".into();
        f.file = Some(PathBuf::from("/x/shop.sqlite"));
        f.ca_file = Some(PathBuf::from("/x/ca.pem"));
        f.ssl_mode = SslMode::Require;
        f.options.insert(dbee_config::ALLOW_PUBLIC_KEY.id.into(), "true".into());
        f.options.insert("aLaterOption".into(), "x".into());
        f.switch_kind(DbKind::Sqlite);
        let file = f.to_config().unwrap();
        assert_eq!((file.host.as_str(), file.port, file.user.as_str(), file.database.as_str()), ("", 0, "", ""));
        assert_eq!((file.ssl_mode, file.ca_file, file.ssh, file.file.as_deref()), (SslMode::default(), None, None, f.file.as_deref()));
        assert_eq!(file.options.keys().collect::<Vec<_>>(), ["aLaterOption"], "another type's option goes, a later one stays");
        f.switch_kind(DbKind::Postgres);
        let server = f.to_config().unwrap();
        assert_eq!((server.host.as_str(), server.user.as_str(), server.ssl_mode, server.file), ("localhost", "ann", SslMode::Require, None));
        assert_eq!(server.ca_file.as_deref(), f.ca_file.as_deref());
        assert_eq!(server.options.keys().collect::<Vec<_>>(), ["aLaterOption"]);
        f.switch_kind(DbKind::MySql);
        assert_eq!(f.to_config().unwrap().options.len(), 2, "MySQL keeps its own");
        assert!(f.file.is_some() && f.options.len() == 2, "the form keeps everything for a switch back");
    }
}

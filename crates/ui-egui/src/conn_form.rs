//! The connection form: its fields as typed ([`Form`]), turning them into a [`ConnectionConfig`]
//! (port numbers, validation), and drawing it in parts, each of fixed height but the fields: the
//! title row with the database type ([`header`]), then the fields, which scroll, and Save · Test ·
//! Connect with the message slot under them at the bottom ([`fields_and_footer`]).
//! The screen around it, with the engine cards a new connection starts with, is
//! [`crate::connections`].
//!
//! The rows follow the database type's settings ([`dbee_config::KindInfo`]), never the type
//! itself: a server type has host and port, user and password, the database, the SSL rows when
//! it takes TLS and the SSH tunnel when it takes one; a file type has its file, with Choose… and
//! New file…. Every type has the name and colour, Read-only, and its own on/off options as check
//! boxes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, Settings, SshAuth, SshConfig, SslMode};
use dbee_engine::Json;
use egui::{
    Align, Button, Checkbox, CornerRadius, FontId, Frame, Key, Label, Layout, Margin, Modifiers, Popup, PopupCloseBehavior, Rect, RectAlign,
    RichText, ScrollArea, Sense, Stroke, TextEdit, UiBuilder, pos2, vec2,
};

use crate::automation::Tags;
use crate::connections::{Act, ConnScreen, DatabaseList, Kind, Running, line_galley, tilde};
use crate::filter_bar;
use crate::picker::Pick;
use crate::theme::{RADIUS, TOKENS, connection_color};

/// The height of a label above a field.
const LABEL_H: f32 = 16.0;
/// The height of a field.
const FIELD_H: f32 = 28.0;
/// A field with its label.
const ROW_H: f32 = LABEL_H + FIELD_H;
/// The slot under the buttons for an error or a test result.
const MESSAGE_H: f32 = 52.0;
/// Save · Test · Connect and the message slot under them, with the space between.
const FOOTER_H: f32 = 32.0 + 6.0 + 6.0 + MESSAGE_H;
/// Fill from URL and Change engine, side by side.
const SIDE_BUTTON_W: f32 = 128.0;
/// The Database field's list button.
const LIST_BUTTON_W: f32 = 84.0;
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
    /// New file… chose `file`: every test and connect asks for it to be made, until a connect
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
    /// The key file chosen with "Change…"; `None` picks automatically.
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
    /// type's default becomes the new one's. Another type forgets New file….
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

pub(crate) fn field_id(id: &str) -> egui::Id {
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

/// A read-only path box (`id`) with the path it shows.
fn path_box(ui: &mut egui::Ui, tags: &mut Tags, id: &str, width: f32, path: Option<&Path>, waiting: bool) {
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let shown = path.map(|p| tilde(&p.display().to_string()));
    let (text, color) = if waiting {
        ("Waiting for the file picker…".to_string(), t.text_muted)
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

/// The space between fields side by side.
const GAP: f32 = 12.0;
/// The port fields' width.
const PORT_W: f32 = 110.0;

/// A button of the title row, as wide as the other one beside it, tagged `id`; whether it was
/// clicked.
fn side_button(ui: &mut egui::Ui, tags: &mut Tags, id: &str, label: &str, enabled: bool) -> bool {
    let r = ui.add_enabled_ui(enabled, |ui| ui.add_sized(vec2(SIDE_BUTTON_W, FIELD_H), Button::new(label))).inner;
    tags.tag(id, &r);
    r.clicked()
}

/// The narrowest the title gets before the type and Change engine move to a row of their own.
const TITLE_MIN_W: f32 = 160.0;

/// `text` in `font`, cut to `width` with "…" (then the whole text shows on hover), in a slot of
/// that width, on its left or its `right`; tagged `id` with `value`.
fn row_text(ui: &mut egui::Ui, tags: &mut Tags, id: &str, (text, font): (&str, FontId), value: &str, width: f32, right: bool) {
    let t = TOKENS;
    let galley = line_galley(ui, text, font, t.text, width);
    let elided = galley.elided;
    let (rect, r) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let x = if right { rect.right() - galley.size().x } else { rect.left() };
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, t.text);
    let r = if elided { r.on_hover_text(text) } else { r };
    tags.tag_value(id, &r, value);
}

/// The title row, `w` wide: the form's title on the left; at the right end Fill from URL when
/// `url` (a type the form can take has a URL), and on the `form`, left of it, the database type
/// and Change engine (unless the type is fixed; not while a test or connect runs). When they
/// would leave the title less than [`TITLE_MIN_W`], the type and Change engine take a row of their
/// own under it, Change engine under Fill from URL.
pub(crate) fn header(ui: &mut egui::Ui, conn: &ConnScreen, url: bool, form: bool, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = TOKENS;
    let kind_font = FontId::proportional(15.0);
    let change = form && !conn.form.engine_fixed;
    // As wide as the widest type's name, so the layout is the same whichever type the form has.
    let kind_w = DbKind::ALL.iter().map(|k| line_galley(ui, k.label(), kind_font.clone(), t.text, w).size().x).fold(0.0, f32::max).min(w / 3.0);
    let spacing = ui.spacing().item_spacing.x;
    let buttons = f32::from(u8::from(url) + u8::from(change)) * (SIDE_BUTTON_W + spacing);
    let one_row = form && TITLE_MIN_W + GAP + kind_w + buttons <= w;
    let kind = |ui: &mut egui::Ui, tags: &mut Tags, width: f32, right: bool| {
        row_text(ui, tags, "conn-kind", (conn.form.kind.label(), kind_font.clone()), conn.form.kind.id(), width, right);
    };
    let change_engine = |ui: &mut egui::Ui, tags: &mut Tags, acts: &mut Vec<Act>| {
        if change && side_button(ui, tags, "conn-change-engine", "Change engine", conn.running.is_none()) {
            acts.push(Act::ChangeEngine);
        }
    };
    ui.allocate_ui_with_layout(vec2(w, 30.0), Layout::right_to_left(Align::Center), |ui| {
        if url && side_button(ui, tags, "conn-fill-url", "Fill from URL…", true) {
            acts.push(Act::FillFromUrl);
        }
        if one_row {
            change_engine(ui, tags, acts);
            kind(ui, tags, kind_w, true);
        }
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let title = conn.form.title();
            let width = ui.available_width().max(1.0);
            row_text(ui, tags, "conn-title", (&title, FontId::proportional(19.0)), &title, width, false);
        });
    });
    if form && !one_row {
        ui.allocate_ui_with_layout(vec2(w, 30.0), Layout::right_to_left(Align::Center), |ui| {
            change_engine(ui, tags, acts);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let width = ui.available_width().max(1.0);
                kind(ui, tags, width, false);
            });
        });
    }
}

/// The fields, `w` wide, then the footer at the bottom of the room left: the fields scroll when
/// they need more. Enter in one of them connects.
pub(crate) fn fields_and_footer(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
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
    let gap = 8.0 + 2.0 * ui.spacing().item_spacing.y;
    let fields_h = (ui.available_height() - FOOTER_H - gap).max(60.0);
    ScrollArea::vertical().id_salt("conn-form-scroll").max_height(fields_h).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(w);
        name_row(ui, &mut conn.form, tags, w);
        match conn.form.kind.info().settings {
            Settings::Server { tls, ssh } => {
                server_rows(ui, conn, tags, acts, w);
                if tls {
                    tls_rows(ui, conn, tags, acts, w);
                }
                read_only_and_options(ui, &mut conn.form, tags, w);
                if ssh {
                    ssh_box(ui, conn, tags, acts, w);
                }
            }
            Settings::File { .. } => {
                file_row(ui, conn, tags, acts, w);
                read_only_and_options(ui, &mut conn.form, tags, w);
            }
        }
    });
    ui.add_space(8.0);
    // The footer keeps its place at the bottom, however tall the fields are.
    let above = ui.available_height() - FOOTER_H - ui.spacing().item_spacing.y;
    if above > 0.0 {
        ui.add_space(above);
    }
    footer(ui, conn, tags, acts, w);
}

/// The picker the form waits for (not one the user stopped waiting for).
fn picking(conn: &ConnScreen) -> Option<Pick> {
    conn.picker.as_ref().filter(|p| !p.stopped).map(|p| p.target)
}

/// Name and colour.
fn name_row(ui: &mut egui::Ui, f: &mut Form, tags: &mut Tags, w: f32) {
    let t = TOKENS;
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
        ui.spacing_mut().item_spacing.x = GAP;
        column(ui, w - dots_w - GAP, "Name", |ui| text_field(ui, tags, "conn-name", &mut f.name, "Optional"));
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
}

/// Host and port, user and password, "Save passwords", and the database.
fn server_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = TOKENS;
    let has_store = conn.has_store;
    let f = &mut conn.form;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        column(ui, w - PORT_W - GAP, "Host", |ui| text_field(ui, tags, "conn-host", &mut f.host, "localhost"));
        column(ui, PORT_W, "Port", |ui| text_field(ui, tags, "conn-port", &mut f.port, &default_port(f.kind)));
    });

    let half = (w - GAP) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
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

    database_row(ui, conn, tags, acts, w);
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
    let top = n as f32 * (filter_bar::ITEM_H + filter_bar::ITEM_GAP);
    let bottom = top + filter_bar::ITEM_H;
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
        ui.allocate_ui_with_layout(vec2(w, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let field_w = w - LIST_BUTTON_W - 6.0;
            let field = ui.add(
                TextEdit::singleline(&mut form.database)
                    .id(field_id("conn-database"))
                    .desired_width(field_w)
                    .min_size(vec2(0.0, FIELD_H))
                    .margin(Margin::symmetric(6, 5)),
            );
            tags.tag_value("conn-database", &field, form.database.as_str());
            if field.changed() {
                databases.open = false;
            }
            if std::mem::take(&mut databases.take_keyboard) && databases.open && ui.memory(|m| m.focused()).is_none() {
                field.request_focus();
            }
            let label = if listing { "Listing…" } else { "List…" };
            let button = ui
                .add_enabled(can_list, Button::new(label).min_size(vec2(LIST_BUTTON_W, FIELD_H)))
                .on_hover_text("Choose from the server's databases");
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
                .show(|ui| {
                    ui.spacing_mut().item_spacing.y = filter_bar::ITEM_GAP;
                    let shown = scroll.show_rows(ui, filter_bar::ITEM_H, names.len(), |ui, rows| {
                        for (n, name) in names.iter().enumerate().skip(rows.start).take(rows.len()) {
                            let r = filter_bar::item_sensing(ui, n == *highlight, name, Sense::CLICK);
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
    });
}

/// SSL mode and CA file.
fn tls_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let picking = picking(conn);
    let f = &mut conn.form;
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
            path_box(ui, tags, "conn-ca", w - 2.0 * (button_w + 6.0), f.ca_file.as_deref(), waiting);
            let choose =
                ui.add_enabled(picking.is_none() || waiting, Button::new(if waiting { "Stop" } else { "Choose…" }).min_size(vec2(button_w, FIELD_H)));
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
}

/// The database file, with Choose… and New file…: the pickers show the type's extensions first.
fn file_row(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let picking = picking(conn);
    let f = &conn.form;
    column(ui, w, "File", |ui| {
        ui.allocate_ui_with_layout(vec2(w, FIELD_H), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let button_w = 84.0;
            let waiting = picking.filter(|p| matches!(p, Pick::File | Pick::NewFile));
            path_box(ui, tags, "conn-file", w - 2.0 * (button_w + 6.0), f.file.as_deref(), waiting.is_some());
            for (pick, id, label) in [(Pick::File, "conn-file-choose", "Choose…"), (Pick::NewFile, "conn-file-new", "New file…")] {
                let this = waiting == Some(pick);
                let b = ui.add_enabled(picking.is_none() || this, Button::new(if this { "Stop" } else { label }).min_size(vec2(button_w, FIELD_H)));
                tags.tag(id, &b);
                if b.clicked() {
                    acts.push(if this { Act::StopPicking } else { Act::Pick(pick) });
                }
            }
        });
    });
    ui.add_space(4.0);
}

/// Read-only, then the type's own on/off settings, one check box each (`conn-option-<id>`), on
/// one line while they fit.
fn read_only_and_options(ui: &mut egui::Ui, f: &mut Form, tags: &mut Tags, w: f32) {
    ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0 * GAP;
        let r = ui.checkbox(&mut f.read_only, "Read-only");
        tags.tag_value("conn-readonly", &r, f.read_only);
        for option in f.kind.info().options {
            let mut on = f.option_on(option.id);
            let r = ui.checkbox(&mut on, option.label);
            if r.changed() {
                f.set_option(option.id, on);
            }
            tags.tag_value(&format!("conn-option-{}", option.id), &r, on);
        }
    });
}

/// The SSH tunnel: on or off, then host, port, user, the login and what it will use.
fn ssh_box(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = TOKENS;
    let picking = picking(conn);
    let ConnScreen { form: f, key, .. } = conn;
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
            ui.spacing_mut().item_spacing.x = GAP;
            column(ui, w - PORT_W - GAP, "SSH host", |ui| text_field(ui, tags, "conn-ssh-host", &mut f.ssh_host, "bastion.example.com"));
            column(ui, PORT_W, "SSH port", |ui| text_field(ui, tags, "conn-ssh-port", &mut f.ssh_port, "22"));
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
            } else if key.for_auth.as_ref() == Some(&wanted) {
                (key.text.clone(), t.text)
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
}

/// Save · Test · Connect, `w` wide, then the reserved slot under them for an error, a result or
/// a note.
fn footer(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = TOKENS;
    let running = conn.running.map(|(_, kind)| kind);
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

    let (rect, slot) = ui.allocate_exact_size(vec2(w, MESSAGE_H), Sense::hover());
    if let Some(m) = &conn.message {
        let (color, tint, id) = match m.kind {
            Kind::Error => (t.danger, t.danger.gamma_multiply(0.08), "conn-error"),
            Kind::Result => (t.success, t.success.gamma_multiply(0.08), "conn-result"),
            Kind::Note => (t.text_muted, t.panel, "conn-note"),
        };
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), tint);
        let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 6.0))).layout(Layout::top_down(Align::Min)));
        ScrollArea::vertical().id_salt("conn-message").max_height(MESSAGE_H - 12.0).auto_shrink([false, true]).show(&mut inner, |ui| {
            ui.add(Label::new(RichText::new(m.text.as_str()).color(color)).wrap().selectable(true));
        });
        tags.tag_value(id, &slot, m.text.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

//! The connection form: its fields as typed ([`Form`]), turning them into a [`ConnectionConfig`]
//! (port numbers, validation), and drawing it in three parts, each of fixed height but the
//! fields: the title row ([`header`]), the fields ([`fields`], which scroll), and Save · Test ·
//! Connect with the message slot under them ([`footer`]). The screen around it is
//! [`crate::connections`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, SshAuth, SshConfig, SslMode};
use dbee_engine::Json;
use egui::{
    Align, Button, Checkbox, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, UiBuilder,
    pos2, vec2,
};

use crate::automation::Tags;
use crate::connections::{Act, ConnScreen, Kind, Running, line_galley, tilde};
use crate::picker::Pick;
use crate::theme::{RADIUS, TOKENS, connection_color};

/// The height of a label above a field.
const LABEL_H: f32 = 16.0;
/// The height of a field.
const FIELD_H: f32 = 28.0;
/// A field with its label.
const ROW_H: f32 = LABEL_H + FIELD_H;
/// The slot under the buttons for an error or a test result.
pub(crate) const MESSAGE_H: f32 = 52.0;

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
    /// Not editable in the form yet; kept so saving does not change it.
    pub kind: DbKind,
    pub file: Option<PathBuf>,
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
    /// Not editable in the form yet; kept so saving does not change them.
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
            kind: c.kind,
            file: c.file.clone(),
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

    /// The connection the form describes, or why it is not usable yet.
    pub(crate) fn to_config(&self) -> Result<ConnectionConfig, String> {
        let port = match parse_port(&self.port) {
            Some(p) => p,
            None if self.kind.uses_file() => 0,
            None => return Err("The port must be a number from 1 to 65535.".into()),
        };
        let ssh = if self.ssh_on && self.kind.takes_ssh() {
            let port = parse_port(&self.ssh_port).ok_or("The SSH port must be a number from 1 to 65535.")?;
            Some(SshConfig { host: self.ssh_host.trim().into(), port, user: self.ssh_user.trim().into(), auth: self.ssh_auth() })
        } else {
            None
        };
        let config = ConnectionConfig {
            id: self.id.clone(),
            name: self.name.trim().into(),
            kind: self.kind,
            file: self.file.clone(),
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
            options: self.options.clone(),
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    /// What the title line says.
    pub(crate) fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.id.is_empty() {
            "New connection".into()
        } else {
            format!("{}@{}/{}", self.user, self.host, self.database)
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

/// The title row, `w` wide: the form's title, and Fill from URL.
pub(crate) fn header(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
    let t = TOKENS;
    ui.allocate_ui_with_layout(vec2(w, 30.0), Layout::left_to_right(Align::Center), |ui| {
        let url_w = 120.0;
        let title = conn.form.title();
        let galley = line_galley(ui, &title, FontId::proportional(19.0), t.text, w - url_w - GAP);
        let (rect, r) = ui.allocate_exact_size(vec2(w - url_w - GAP, 30.0), Sense::hover());
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
}

/// The fields, `w` wide; they scroll past `fields_h`. Enter in one of them connects.
pub(crate) fn fields(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32, fields_h: f32) {
    let in_field = ui.ctx().memory(|m| m.focused()).is_some_and(|focused| FORM_FIELDS.iter().any(|id| field_id(id) == focused));
    if in_field && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        acts.push(Act::ConnectForm);
    }
    ScrollArea::vertical().id_salt("conn-form-scroll").max_height(fields_h).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(w);
        name_row(ui, &mut conn.form, tags, w);
        server_rows(ui, conn, tags, w);
        tls_rows(ui, conn, tags, acts, w);
        read_only_row(ui, &mut conn.form, tags, w);
        ui.add_space(6.0);
        ssh_box(ui, conn, tags, acts, w);
    });
    ui.add_space(8.0);
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
fn server_rows(ui: &mut egui::Ui, conn: &mut ConnScreen, tags: &mut Tags, w: f32) {
    let t = TOKENS;
    let has_store = conn.has_store;
    let f = &mut conn.form;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        column(ui, w - PORT_W - GAP, "Host", |ui| text_field(ui, tags, "conn-host", &mut f.host, "localhost"));
        column(ui, PORT_W, "Port", |ui| text_field(ui, tags, "conn-port", &mut f.port, "5432"));
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

    column(ui, w, "Database", |ui| text_field(ui, tags, "conn-database", &mut f.database, ""));
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

/// Read-only.
fn read_only_row(ui: &mut egui::Ui, f: &mut Form, tags: &mut Tags, w: f32) {
    let t = TOKENS;
    ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center), |ui| {
        let r = ui.checkbox(&mut f.read_only, "Read-only");
        tags.tag_value("conn-readonly", &r, f.read_only);
        ui.label(RichText::new("Sessions start read-only and the grid offers no editing.").size(12.0).color(t.text_muted));
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
pub(crate) fn footer(ui: &mut egui::Ui, conn: &ConnScreen, tags: &mut Tags, acts: &mut Vec<Act>, w: f32) {
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
}

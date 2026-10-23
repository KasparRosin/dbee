//! Dialogs: the modal cards the window shows over either screen, one at a time
//! ([`crate::app::View::dialog`]).
//!
//! A command the user started (a button, a card, a shortcut) that needs an answer returns a
//! `needs…` error; [`ask`] turns it into a dialog that keeps the command and its params
//! ([`Rerun`]), and the dialog's OK runs it again with the answer added:
//! - `needsHostKey` → [`Dialog::HostKey`]: the fingerprint, Trust and connect (`trustHostKey`);
//! - `needsPassphrase`, `needsPassword`, `needsSshPassword` → [`Dialog::Secret`]: a secret field
//!   (`sshPassphrase`, `password`, `sshPassword`) and, for `connection.open`, "Save in keyring"
//!   (`savePassword`), which the engine honours for saved connections only;
//! - `needsConfirm` → [`Dialog::Confirm`]: Close anyway (`force`).
//!
//! The connection screen also opens [`Dialog::FillFromUrl`] (runs `connection.parseUrl`) and
//! [`Dialog::DeleteConnection`]. Programmatic calls (the control channel's `engine.execute`)
//! never come here: they get the `needs…` error back.
//!
//! Every dialog is a fixed-width card at a fixed place (centred, near the top), so its widgets are
//! where `ui.inspect` says from its first frame on. Automation ids: `dialog-ok`, `dialog-cancel`,
//! `dialog-text`, and per dialog `dialog-url`, `dialog-error`, `dialog-fingerprint`,
//! `dialog-secret`, `dialog-save`. The URL and secret fields are tagged as secrets: `ui.inspect`
//! says whether they are filled in, never what they hold, and neither `Debug` nor the logs show
//! them.

use dbee_engine::{EngineError, Json, Need, Outcome};
use egui::{Align, Align2, Button, Checkbox, CornerRadius, Frame, Key, Layout, Margin, RichText, Stroke, TextEdit, vec2};
use serde_json::json;

use crate::app::Origin;
use crate::automation::Tags;
use crate::connections;
use crate::theme::{RADIUS, TOKENS};
use crate::{DbeeApp, connections::tilde};

/// The width of a dialog's content.
const WIDTH: f32 = 460.0;
/// The space around a dialog's content.
const MARGIN: i8 = 20;
/// How far below the top of the window dialogs sit, at most.
const TOP: f32 = 140.0;

/// A command to run again once the user has answered, with the params it ran with.
pub(crate) struct Rerun {
    pub command: &'static str,
    /// May hold secrets: never logged, never in `Debug`.
    pub params: Json,
    pub origin: Origin,
}

impl std::fmt::Debug for Rerun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rerun").field("command", &self.command).field("origin", &self.origin).finish_non_exhaustive()
    }
}

/// The secret a [`Dialog::Secret`] asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SecretAsk {
    /// The passphrase of this key file.
    Passphrase { key: String },
    /// The database password.
    Password,
    /// The SSH password.
    SshPassword,
}

impl SecretAsk {
    /// The param the answer goes in.
    fn param(&self) -> &'static str {
        match self {
            SecretAsk::Passphrase { .. } => "sshPassphrase",
            SecretAsk::Password => "password",
            SecretAsk::SshPassword => "sshPassword",
        }
    }
}

/// Whether a secret dialog offers "Save in keyring".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Not for this command (only `connection.open` can keep a secret).
    NotOffered,
    /// Offered, checked or not at first.
    Yes { checked: bool },
    /// Shown off and disabled, for this reason.
    No(&'static str),
}

/// The dialog on screen.
pub(crate) enum Dialog {
    /// "Fill from URL…": a URL to parse into the connection form.
    FillFromUrl { url: String, error: Option<String>, focused: bool },
    /// An SSH server not in `known_hosts`.
    HostKey { host: String, key_type: String, fingerprint: String, rerun: Rerun },
    /// A password or passphrase.
    Secret { ask: SecretAsk, value: String, keep: Keep, save: bool, focused: bool, rerun: Rerun },
    /// Something will be lost.
    Confirm { message: String, rerun: Rerun },
    /// Deleting a saved connection (and its secrets).
    DeleteConnection { id: String, name: String },
    /// A dialog a command asked for (`UiRequest::Dialog`) that this window does not draw.
    Requested { name: &'static str },
}

impl std::fmt::Debug for Dialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The fields can hold secrets (a URL, a password): only the name.
        f.debug_tuple("Dialog").field(&self.name()).finish()
    }
}

impl Dialog {
    /// The name `ui.inspect` reports.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Dialog::FillFromUrl { .. } => "fillFromUrl",
            Dialog::HostKey { .. } => "hostKey",
            Dialog::Secret { ask: SecretAsk::Passphrase { .. }, .. } => "passphrase",
            Dialog::Secret { ask: SecretAsk::Password, .. } => "password",
            Dialog::Secret { ask: SecretAsk::SshPassword, .. } => "sshPassword",
            Dialog::Confirm { .. } => "confirm",
            Dialog::DeleteConnection { .. } => "deleteConnection",
            Dialog::Requested { name } => name,
        }
    }

    /// The empty "Fill from URL" dialog.
    pub(crate) fn fill_from_url() -> Dialog {
        Dialog::FillFromUrl { url: String::new(), error: None, focused: false }
    }
}

/// Opens the dialog for `e`, a `needs…` error of `rerun`'s command; `false` (and nothing opens)
/// for any other error.
pub(crate) fn ask(app: &mut DbeeApp, e: &EngineError, rerun: Rerun) -> bool {
    let EngineError::Needs { need, message } = e else { return false };
    let secret = |ask: SecretAsk, keep: Keep, rerun: Rerun| Dialog::Secret {
        ask,
        value: String::new(),
        keep,
        save: matches!(keep, Keep::Yes { checked: true }),
        focused: false,
        rerun,
    };
    let dialog = match need {
        Need::HostKey { host, key_type, fingerprint } => {
            Dialog::HostKey { host: host.clone(), key_type: key_type.clone(), fingerprint: fingerprint.clone(), rerun }
        }
        Need::Passphrase { key } => {
            let keep = keep_of(app, &rerun);
            secret(SecretAsk::Passphrase { key: key.clone() }, keep, rerun)
        }
        Need::Password => {
            let keep = keep_of(app, &rerun);
            secret(SecretAsk::Password, keep, rerun)
        }
        Need::SshPassword => {
            let keep = keep_of(app, &rerun);
            secret(SecretAsk::SshPassword, keep, rerun)
        }
        Need::Confirm { .. } => Dialog::Confirm { message: message.clone(), rerun },
    };
    app.view.dialog = Some(dialog);
    true
}

/// "Save in keyring" for a secret `rerun` asks for: offered for `connection.open`, and possible
/// only with a secret store and for a saved connection (the engine keeps nothing for an unsaved
/// one), checked when that connection saves its secrets.
fn keep_of(app: &DbeeApp, rerun: &Rerun) -> Keep {
    if rerun.command != connections::OPEN {
        return Keep::NotOffered;
    }
    if !app.view.conn.has_store {
        return Keep::No("There is no keyring, so dbee asks each time.");
    }
    let id = rerun.params.get("config").and_then(|c| c.get("id")).or_else(|| rerun.params.get("id")).and_then(Json::as_str).unwrap_or_default();
    match app.session.state().store.saved.iter().find(|c| !id.is_empty() && c.id == id) {
        Some(saved) => Keep::Yes { checked: saved.save_secrets },
        None => Keep::No("Save the connection to keep its passwords."),
    }
}

/// What the user did in a dialog this frame.
enum Act {
    None,
    Cancel,
    Rerun(Rerun),
    ParseUrl(String),
    Delete(String),
}

/// Draws the dialog, if any, and acts on its buttons.
pub(crate) fn show(app: &mut DbeeApp, ctx: &egui::Context) {
    let act = {
        let DbeeApp { view, tags, .. } = app;
        let Some(dialog) = view.dialog.as_mut() else { return };
        draw(ctx, dialog, tags)
    };
    match act {
        Act::None => {}
        Act::Cancel => app.view.dialog = None,
        Act::Rerun(rerun) => {
            app.view.dialog = None;
            app.start(rerun.command, rerun.params, rerun.origin);
        }
        Act::ParseUrl(url) => match app.run(connections::PARSE_URL, json!({ "url": url })) {
            Ok(Outcome::Done(parts)) => {
                app.view.dialog = None;
                connections::fill_from_url(app, &parts);
            }
            Ok(Outcome::Started(_)) => app.view.dialog = None,
            Err(e) => {
                if let Some(Dialog::FillFromUrl { error, .. }) = app.view.dialog.as_mut() {
                    *error = Some(connections::error_text(&e));
                }
            }
        },
        Act::Delete(id) => {
            app.view.dialog = None;
            connections::delete(app, &id);
        }
    }
}

/// `params` with `key` set to `value`.
fn with(mut params: Json, key: &str, value: Json) -> Json {
    if let Some(p) = params.as_object_mut() {
        p.insert(key.to_string(), value);
    }
    params
}

fn draw(ctx: &egui::Context, dialog: &mut Dialog, tags: &mut Tags) -> Act {
    let t = TOKENS;
    let screen = ctx.content_rect();
    let outer = WIDTH + 2.0 * f32::from(MARGIN) + 2.0;
    // Fixed left-top corner: the first frame (egui's sizing pass) puts every widget where the
    // following frames do, so a click on a rect from `ui.inspect` lands.
    let offset = vec2(((screen.width() - outer) / 2.0).max(0.0), (screen.height() * 0.18).min(TOP));
    let id = egui::Id::new("dbee-dialog").with(dialog.name());
    // The sizing pass lays the content out in the default size: all the height below the corner,
    // so no row gets less room than in the frames after it.
    let below = (screen.height() - offset.y).max(1.0);
    let area = egui::Modal::default_area(id).anchor(Align2::LEFT_TOP, offset).default_size(vec2(outer, below));
    let frame = Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(RADIUS.saturating_mul(2)))
        .inner_margin(Margin::same(MARGIN));
    let modal = egui::Modal::new(id).area(area).frame(frame).backdrop_color(t.text.gamma_multiply(0.35)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.with_layout(Layout::top_down(Align::Min), |ui| body(ui, dialog, tags)).inner
    });
    match modal.inner {
        Act::None if modal.should_close() => Act::Cancel,
        act => act,
    }
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong());
    ui.add_space(6.0);
}

fn text(ui: &mut egui::Ui, tags: &mut Tags, id: &str, text: &str) {
    let r = ui.add(egui::Label::new(text).wrap());
    tags.tag_value(id, &r, text);
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(TOKENS.text_muted)).wrap());
}

/// The OK and Cancel buttons, right-aligned: `(ok clicked, cancel clicked)`. Without `cancel`
/// only OK shows.
fn buttons(ui: &mut egui::Ui, tags: &mut Tags, ok: &str, ok_enabled: bool, danger: bool, cancel: bool) -> (bool, bool) {
    let t = TOKENS;
    ui.add_space(12.0);
    // Top-aligned: egui centres nothing in a dialog's first frame (its sizing pass), so a centred
    // row would sit a pixel off there.
    ui.allocate_ui_with_layout(vec2(WIDTH, 30.0), Layout::right_to_left(Align::Min), |ui| {
        let fill = if danger { t.danger } else { t.accent };
        let button = Button::new(RichText::new(ok).color(t.accent_text)).fill(fill).min_size(vec2(96.0, 30.0));
        let ok_r = ui.add_enabled(ok_enabled, button);
        tags.tag_value("dialog-ok", &ok_r, ok);
        let cancelled = if cancel {
            let r = ui.add(Button::new("Cancel").min_size(vec2(80.0, 30.0)));
            tags.tag_value("dialog-cancel", &r, "Cancel");
            r.clicked()
        } else {
            false
        };
        (ok_r.clicked(), cancelled)
    })
    .inner
}

/// Asks for focus until the field has it (the first frame of a dialog may not grant it).
fn focus(r: &egui::Response, focused: &mut bool) {
    if !*focused {
        r.request_focus();
        *focused = r.has_focus();
    }
}

fn body(ui: &mut egui::Ui, dialog: &mut Dialog, tags: &mut Tags) -> Act {
    let t = TOKENS;
    match dialog {
        Dialog::FillFromUrl { url, error, focused } => {
            title(ui, "Fill from URL");
            text(ui, tags, "dialog-text", "Paste a postgres:// or postgresql:// URL. The parts it has replace those in the form.");
            ui.add_space(8.0);
            let r = ui.add(
                TextEdit::singleline(url)
                    .id(egui::Id::new("dialog-url"))
                    .hint_text("postgresql://user:password@host:5432/database")
                    .desired_width(WIDTH)
                    .margin(Margin::symmetric(6, 5)),
            );
            // The URL can carry a password.
            tags.tag_secret("dialog-url", &r, !url.is_empty());
            focus(&r, focused);
            let entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            // A reserved line for the reason a URL did not parse.
            let (rect, line) = ui.allocate_exact_size(vec2(WIDTH, 20.0), egui::Sense::hover());
            if let Some(e) = error {
                let mut job = egui::text::LayoutJob::single_section(e.clone(), egui::TextFormat::simple(egui::FontId::proportional(12.0), t.danger));
                job.wrap = egui::text::TextWrapping::truncate_at_width(WIDTH);
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, t.danger);
                tags.tag_value("dialog-error", &line, e.as_str());
            }
            let (ok, cancel) = buttons(ui, tags, "Fill", !url.trim().is_empty(), false, true);
            if cancel {
                Act::Cancel
            } else if (ok || entered) && !url.trim().is_empty() {
                Act::ParseUrl(url.clone())
            } else {
                Act::None
            }
        }
        Dialog::HostKey { host, key_type, fingerprint, rerun } => {
            title(ui, "Unknown SSH server");
            text(ui, tags, "dialog-text", &format!("dbee has not connected to {host} before. Its {key_type} host key has this fingerprint:"));
            ui.add_space(6.0);
            let r = ui.add(egui::Label::new(RichText::new(fingerprint.as_str()).monospace()).selectable(true));
            tags.tag_value("dialog-fingerprint", &r, fingerprint.as_str());
            ui.add_space(6.0);
            muted(ui, "Trust it only if it is the fingerprint the server's administrator gave you. dbee then adds it to ~/.ssh/known_hosts.");
            let (ok, cancel) = buttons(ui, tags, "Trust and connect", true, false, true);
            if cancel {
                Act::Cancel
            } else if ok {
                let params = with(rerun.params.take(), "trustHostKey", json!(fingerprint));
                Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
            } else {
                Act::None
            }
        }
        Dialog::Secret { ask, value, keep, save, focused, rerun } => {
            let config = rerun.params.get("config").cloned().unwrap_or(Json::Null);
            let field = |k: &str| config.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
            let ssh = |k: &str| config.get("ssh").and_then(|s| s.get(k)).and_then(Json::as_str).unwrap_or_default().to_string();
            let (heading, line) = match ask {
                SecretAsk::Passphrase { key } => ("Key passphrase", format!("The key {} is protected by a passphrase.", tilde(key))),
                SecretAsk::Password => ("Database password", format!("Enter the password of {} on {}.", field("user"), field("host"))),
                SecretAsk::SshPassword => ("SSH password", format!("Enter the SSH password of {} on {}.", ssh("user"), ssh("host"))),
            };
            title(ui, heading);
            text(ui, tags, "dialog-text", &line);
            ui.add_space(8.0);
            let r = ui.add(
                TextEdit::singleline(value).id(egui::Id::new("dialog-secret")).password(true).desired_width(WIDTH).margin(Margin::symmetric(6, 5)),
            );
            tags.tag_secret("dialog-secret", &r, !value.is_empty());
            focus(&r, focused);
            let entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(4.0);
            match keep {
                Keep::NotOffered => {}
                Keep::Yes { .. } => {
                    let r = ui.checkbox(save, "Save in keyring");
                    tags.tag_value("dialog-save", &r, *save);
                }
                Keep::No(why) => {
                    let mut off = false;
                    ui.horizontal_top(|ui| {
                        let r = ui.add_enabled(false, Checkbox::new(&mut off, "Save in keyring"));
                        tags.tag_value("dialog-save", &r, false);
                        ui.label(RichText::new(*why).size(12.0).color(t.text_muted));
                    });
                }
            }
            let ok_label = if rerun.command == connections::TEST { "Test" } else { "Connect" };
            let (ok, cancel) = buttons(ui, tags, ok_label, !value.is_empty(), false, true);
            if cancel {
                Act::Cancel
            } else if (ok || entered) && !value.is_empty() {
                let mut params = with(rerun.params.take(), ask.param(), json!(std::mem::take(value)));
                if *save && matches!(keep, Keep::Yes { .. }) {
                    params = with(params, "savePassword", json!(true));
                }
                Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
            } else {
                Act::None
            }
        }
        Dialog::Confirm { message, rerun } => {
            title(ui, "Close anyway?");
            text(ui, tags, "dialog-text", message);
            let (ok, cancel) = buttons(ui, tags, "Close anyway", true, true, true);
            if cancel {
                Act::Cancel
            } else if ok {
                let params = with(rerun.params.take(), "force", json!(true));
                Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
            } else {
                Act::None
            }
        }
        Dialog::DeleteConnection { id, name } => {
            title(ui, "Delete connection");
            text(ui, tags, "dialog-text", &format!("Delete “{name}”? Its saved passwords are deleted with it."));
            let (ok, cancel) = buttons(ui, tags, "Delete", true, true, true);
            if cancel {
                Act::Cancel
            } else if ok {
                Act::Delete(id.clone())
            } else {
                Act::None
            }
        }
        Dialog::Requested { name } => {
            title(ui, name);
            text(ui, tags, "dialog-text", &format!("A command asked for the “{name}” dialog, which this version of dbee does not have."));
            let (ok, _) = buttons(ui, tags, "Close", true, false, false);
            if ok { Act::Cancel } else { Act::None }
        }
    }
}

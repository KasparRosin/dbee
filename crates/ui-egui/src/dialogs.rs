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
//! - `needsConfirm` → [`Dialog::Confirm`]: Close anyway, or Apply again after an unknown outcome (`force`);
//! - `needsTransactionEnd` → [`Dialog::EndTransaction`], the transaction prompt: COMMIT (the main
//!   button, Enter), ROLLBACK or CANCEL, or only ROLLBACK and CANCEL when the transaction failed or
//!   a run still goes in it (`end`). The close it runs again says how it went in a toast
//!   ([`ended`]).
//!
//! The connection screen also opens [`Dialog::FillFromUrl`] (runs `connection.parseUrl`, and takes
//! only a URL of the database types it was opened for) and [`Dialog::DeleteConnection`].
//! Programmatic calls (the control channel's `engine.execute`) never come here: they get the
//! `needs…` error back.
//!
//! Every dialog is drawn by [`crate::components::dialog`]: a fixed-width card at a fixed place
//! (centred, near the top) over the scrim, so its widgets are where `ui.inspect` says from its
//! first frame on. Escape and Cancel close it; a click on the scrim does nothing. Automation ids:
//! `dialog-ok`, `dialog-cancel`, `dialog-text`, and per dialog `dialog-rollback`, `dialog-url`,
//! `dialog-error`, `dialog-fingerprint`, `dialog-secret`, `dialog-save`. The URL and secret
//! fields are tagged as secrets: `ui.inspect` says whether they are filled in, never what they
//! hold, and neither `Debug` nor the logs show them.

use dbee_config::{ConnectionConfig, DbKind};
use dbee_engine::cmd::query::{FAILED_COMMIT, NO_TRANSACTION};
use dbee_engine::{ConnectionId, EndAbout, EngineError, Json, Need, Outcome, Running, TabId, TransactionState, Which};
use egui::Key;
use serde_json::json;

use crate::app::{Origin, reason, sentence};
use crate::automation::Tags;
use crate::components::checkbox::Checkbox;
use crate::components::dialog::{self, Button};
use crate::components::input::{Field, Input};
use crate::components::toast::Kind;
use crate::connections;
use crate::theme::{self, Text, Words};
use crate::{DbeeApp, connections::tilde};

/// Where a dialog's main button and its Cancel are among its buttons (the first on the right).
const OK: usize = 0;
const CANCEL: usize = 1;

/// The command that closes a tab.
pub(crate) const TAB_CLOSE: &str = "tab.close";

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

/// What a [`Dialog::EndTransaction`] asks about: the need's data, and the display name of the
/// connection whose transaction it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EndAsk {
    pub state: TransactionState,
    pub about: EndAbout,
    /// The tab's title or the connection's name, as `about` says.
    pub name: String,
    pub connection: String,
    /// What the close loses besides, as a sentence.
    pub lost: Option<String>,
    pub running: Option<Running>,
}

impl EndAsk {
    /// Only a rollback can end it: it failed, or a run still goes in it.
    fn only_rollback(&self) -> bool {
        self.running.is_some() || self.state == TransactionState::Failed
    }

    /// The prompt's title and text: why it asks, then what the close loses besides.
    fn words(&self) -> (&'static str, Words) {
        let (name, connection) = (self.name.as_str(), self.connection.as_str());
        let why = match (&self.running, self.state, self.about) {
            (Some(run), _, _) => Words::default()
                .name(run.title.as_str())
                .plain(" is still running in the transaction on ")
                .name(connection)
                .plain(". Closing stops it, so the transaction can only be rolled back."),
            (None, TransactionState::Failed, _) => {
                Words::from("The transaction on ").name(connection).plain(" failed, so it can only be rolled back.")
            }
            (None, TransactionState::Open, EndAbout::Tab) => Words::default().name(name).plain(" opened a transaction that isn't committed yet."),
            (None, TransactionState::Open, EndAbout::Connection) => Words::default().name(name).plain(" has a transaction that isn't committed yet."),
            (None, TransactionState::Unknown | TransactionState::Idle, _) => {
                Words::from("dbee can't tell whether ").name(name).plain(" still has a transaction open.")
            }
        };
        let text = match &self.lost {
            Some(lost) => why.plain(" ").plain(lost.as_str()),
            None => why,
        };
        let title = if self.only_rollback() { "Roll back the transaction?" } else { "Commit the transaction?" };
        (title, text)
    }
}

/// A close run again with `end`: whether it commits, and the name of the connection whose
/// transaction it ends, taken before it runs (the close may take the connection with it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ending {
    commit: bool,
    connection: String,
}

/// The [`Ending`] of `command` run with `params`, when it is `tab.close` or `connection.close`
/// with `end`.
pub(crate) fn ending(app: &DbeeApp, command: &str, params: &Json) -> Option<Ending> {
    if command != TAB_CLOSE && command != connections::CLOSE {
        return None;
    }
    let commit = match params.get("end").and_then(Json::as_str)? {
        "commit" => true,
        "rollback" => false,
        _ => return None,
    };
    Some(Ending { commit, connection: closed_connection(app, command, params)? })
}

/// Says in a toast how a close that ends a transaction went: `Committed on <connection>` or
/// `Rolled back on <connection>`, or why it failed (nothing closed then). Nothing when no
/// transaction was open any more, or the connection went meanwhile.
pub(crate) fn ended(app: &mut DbeeApp, ending: &Ending, result: Result<Json, EngineError>) {
    let (done, what) = if ending.commit { ("Committed on ", "commit") } else { ("Rolled back on ", "rollback") };
    let failed = |why: &str| sentence(&format!("The {what} failed: {why}"));
    match result {
        Ok(r) if r.get("ignored") == Some(&Json::Bool(true)) => {}
        Ok(r) => match r.get("note").and_then(Json::as_str) {
            Some(NO_TRANSACTION) => {}
            // The server rolled a failed transaction back: nothing was committed.
            Some(FAILED_COMMIT) => app.notice_again(Kind::Error, failed(FAILED_COMMIT)),
            _ => app.notice_again(Kind::Success, Words::from(done).name(ending.connection.as_str())),
        },
        Err(e) => app.notice_again(Kind::Error, failed(&reason(&e))),
    }
}

/// The dialog on screen.
pub(crate) enum Dialog {
    /// "Fill from URL…": a URL of one of `kinds` to parse into the connection form.
    FillFromUrl { url: String, error: Option<String>, focused: bool, kinds: Vec<DbKind> },
    /// An SSH server not in `known_hosts`.
    HostKey { host: String, key_type: String, fingerprint: String, rerun: Rerun },
    /// A password or passphrase; `line` says whose.
    Secret { ask: SecretAsk, line: Words, value: String, keep: Keep, save: bool, focused: bool, rerun: Rerun },
    /// Something will be lost.
    Confirm { message: String, what: String, rerun: Rerun },
    /// Closing would drop a transaction: COMMIT, ROLLBACK or CANCEL.
    EndTransaction { ask: EndAsk, rerun: Rerun },
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
            Dialog::EndTransaction { .. } => "transactionEnd",
            Dialog::DeleteConnection { .. } => "deleteConnection",
            Dialog::Requested { name } => name,
        }
    }

    /// The empty "Fill from URL" dialog, for a URL of one of `kinds`.
    pub(crate) fn fill_from_url(kinds: Vec<DbKind>) -> Dialog {
        Dialog::FillFromUrl { url: String::new(), error: None, focused: false, kinds }
    }
}

/// Opens the dialog for `e`, a `needs…` error of `rerun`'s command; `false` (and nothing opens)
/// for any other error.
pub(crate) fn ask(app: &mut DbeeApp, e: &EngineError, rerun: Rerun) -> bool {
    let EngineError::Needs { need, message } = e else { return false };
    let secret = |ask: SecretAsk, keep: Keep, rerun: Rerun, app: &DbeeApp| Dialog::Secret {
        line: secret_line(app, &ask, &rerun),
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
            secret(SecretAsk::Passphrase { key: key.clone() }, keep, rerun, app)
        }
        Need::Password => {
            let keep = keep_of(app, &rerun);
            secret(SecretAsk::Password, keep, rerun, app)
        }
        Need::SshPassword => {
            let keep = keep_of(app, &rerun);
            secret(SecretAsk::SshPassword, keep, rerun, app)
        }
        Need::Confirm { what } => Dialog::Confirm { message: message.clone(), what: what.clone(), rerun },
        Need::EndTransaction { state, about, name, lost, running } => {
            let connection = closed_connection(app, rerun.command, &rerun.params).unwrap_or_else(|| name.clone());
            let ask = EndAsk { state: *state, about: *about, name: name.clone(), connection, lost: lost.clone(), running: running.clone() };
            Dialog::EndTransaction { ask, rerun }
        }
    };
    app.view.dialog = Some(dialog);
    true
}

/// The title and button of a confirm dialog; both re-run the command with `force`.
fn confirm_words(what: &str) -> (&'static str, &'static str) {
    match what {
        "applyAfterUnknownOutcome" => ("Apply again?", "Apply again"),
        _ => ("Close anyway?", "Close anyway"),
    }
}

/// The display name of the connection a close acts on: `tab.close`'s tab's, `connection.close`'s
/// connection's, else the active one's.
fn closed_connection(app: &DbeeApp, command: &str, params: &Json) -> Option<String> {
    let which = match command {
        TAB_CLOSE => params.get("tab").and_then(Json::as_u64).map(|t| Which::Tab(TabId(t))),
        connections::CLOSE => params.get("connection").and_then(Json::as_u64).map(|c| Which::Id(ConnectionId(c))),
        _ => None,
    };
    app.session.state().connection(which.unwrap_or(Which::Active)).map(|c| c.config.display_name())
}

/// The connection `rerun`'s command is about: its `config`, or the saved or recent connection
/// its `id` names.
fn config_of(app: &DbeeApp, rerun: &Rerun) -> Option<ConnectionConfig> {
    if let Some(c) = rerun.params.get("config").filter(|c| c.is_object()) {
        return serde_json::from_value::<ConnectionConfig>(c.clone()).ok();
    }
    let id = rerun.params.get("id").and_then(Json::as_str)?;
    app.session.state().store.get(id).cloned()
}

/// What the secret dialog says it asks for: whose password, or plain wording when the command
/// names no connection dbee knows (never blanks).
fn secret_line(app: &DbeeApp, ask: &SecretAsk, rerun: &Rerun) -> Words {
    let config = config_of(app, rerun);
    let named = |user: &str, host: &str| !user.trim().is_empty() && !host.trim().is_empty();
    match ask {
        SecretAsk::Passphrase { key } => Words::from("The key ").name(tilde(key)).plain(" is protected by a passphrase."),
        SecretAsk::Password => match config.filter(|c| named(&c.user, &c.host)) {
            Some(c) => Words::from("Enter the password of ").name(c.user).plain(" on ").name(c.host).plain("."),
            None => "Enter the database password.".into(),
        },
        SecretAsk::SshPassword => match config.and_then(|c| c.ssh).filter(|s| named(&s.user, &s.host)) {
            Some(s) => Words::from("Enter the SSH password of ").name(s.user).plain(" on ").name(s.host).plain("."),
            None => "Enter the SSH password.".into(),
        },
    }
}

/// "Save in keyring" for a secret `rerun` asks for: offered for `connection.open`, and possible
/// only with a secret store and for a saved connection (the engine keeps nothing for an unsaved
/// one). Checked as the form says ("Save passwords") when the form asked, else as the saved
/// connection says.
fn keep_of(app: &DbeeApp, rerun: &Rerun) -> Keep {
    if rerun.command != connections::OPEN {
        return Keep::NotOffered;
    }
    if !app.view.conn.has_store {
        return Keep::No("There is no keyring, so dbee asks each time.");
    }
    let id = rerun.params.get("config").and_then(|c| c.get("id")).or_else(|| rerun.params.get("id")).and_then(Json::as_str).unwrap_or_default();
    let form_says = match rerun.origin {
        Origin::Form(_) => rerun.params.get("config").and_then(|c| c.get("saveSecrets")).and_then(Json::as_bool),
        Origin::Menu | Origin::Page | Origin::KeyLine => None,
    };
    match app.session.state().store.saved.iter().find(|c| !id.is_empty() && c.id == id) {
        Some(saved) => Keep::Yes { checked: form_says.unwrap_or(saved.save_secrets) },
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
                let kind = parts.get("kind").and_then(Json::as_str).and_then(DbKind::from_id);
                match app.view.dialog.as_mut() {
                    Some(Dialog::FillFromUrl { error, kinds, .. }) if !kind.is_some_and(|k| kinds.contains(&k)) => {
                        *error = Some(format!("Paste a {} URL.", schemes(kinds)));
                    }
                    _ => {
                        app.view.dialog = None;
                        let password = dbee_config::parse_url(&url).ok().and_then(|u| u.password);
                        connections::fill_from_url(app, &parts, password);
                    }
                }
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

/// The URL schemes of `kinds`: `postgres://, postgresql:// or mysql://`.
fn schemes(kinds: &[DbKind]) -> String {
    let all: Vec<String> = kinds.iter().flat_map(|k| k.info().url_schemes).map(|s| format!("{s}://")).collect();
    match all.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// An example URL of the first of `kinds`, with its default port.
fn url_hint(kinds: &[DbKind]) -> String {
    let Some(kind) = kinds.first() else { return String::new() };
    let scheme = kind.info().url_schemes.last().copied().unwrap_or_default();
    let port = kind.default_port().map(|p| format!(":{p}")).unwrap_or_default();
    format!("{scheme}://user:password@host{port}/database")
}

/// `params` with `key` set to `value`.
fn with(mut params: Json, key: &str, value: Json) -> Json {
    if let Some(p) = params.as_object_mut() {
        p.insert(key.to_string(), value);
    }
    params
}

fn draw(ctx: &egui::Context, dialog: &mut Dialog, tags: &mut Tags) -> Act {
    let id = egui::Id::new("dbee-dialog").with(dialog.name());
    let ok = |label| Button::primary(label, "dialog-ok");
    let cancel = Button::outline("Cancel", "dialog-cancel");
    match dialog {
        Dialog::FillFromUrl { url, error, focused, kinds } => {
            let hint = url_hint(kinds);
            let mut entered = false;
            let shown = dialog::Dialog::new(id, "Fill from URL")
                .body(|ui, buttons| {
                    let input = Input::data(url).id(egui::Id::new("dialog-url")).placeholder(&hint).error(error.is_some());
                    // The line for the reason a URL did not parse keeps its room.
                    let field = Field::unlabelled(input).error(error.as_deref()).keep_error_room(true).show(ui);
                    // The URL can carry a password.
                    tags.tag_secret_as("dialog-url", &field.input, !url.is_empty(), true);
                    focus(&field.input, focused);
                    entered = field.input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if let (Some(e), Some(line)) = (error.as_ref(), field.error.as_ref()) {
                        tags.tag_value_as("dialog-error", line, e.as_str(), true);
                    }
                    buttons[OK].enabled = !url.trim().is_empty();
                })
                .buttons([ok("Fill"), cancel])
                .show(ctx);
            let pressed = tagged(tags, &shown, "");
            if pressed == Some(CANCEL) {
                Act::Cancel
            } else if (pressed == Some(OK) || entered) && !url.trim().is_empty() {
                Act::ParseUrl(url.clone())
            } else {
                Act::None
            }
        }
        Dialog::HostKey { host, key_type, fingerprint, rerun } => {
            let text = Words::from("dbee has not connected to ")
                .name(host.as_str())
                .plain(" before. Its ")
                .name(key_type.as_str())
                .plain(" host key has this fingerprint:");
            let shown = dialog::Dialog::new(id, "Unknown SSH server")
                .text(text.clone())
                .body(|ui, _| {
                    let r = ui.add(egui::Label::new(Text::Data.rich(fingerprint.as_str())).selectable(true));
                    tags.tag_value_as("dialog-fingerprint", &r, fingerprint.as_str(), true);
                    muted(ui, "Trust it only if it is the fingerprint the server's administrator gave you. dbee then adds it to ~/.ssh/known_hosts.");
                })
                .buttons([ok("Trust and connect"), cancel])
                .show(ctx);
            match tagged(tags, &shown, &text) {
                Some(OK) => {
                    let params = with(rerun.params.take(), "trustHostKey", json!(fingerprint));
                    Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
                }
                Some(_) => Act::Cancel,
                None => Act::None,
            }
        }
        Dialog::Secret { ask, line, value, keep, save, focused, rerun } => {
            let heading = match ask {
                SecretAsk::Passphrase { .. } => "Key passphrase",
                SecretAsk::Password => "Database password",
                SecretAsk::SshPassword => "SSH password",
            };
            let ok_label = if rerun.command == connections::TEST { "Test" } else { "Connect" };
            let t = theme::tokens(ctx);
            let mut entered = false;
            let shown = dialog::Dialog::new(id, heading)
                .text(line.clone())
                .body(|ui, buttons| {
                    let r = ui.add(Input::body(value).id(egui::Id::new("dialog-secret")).password(true));
                    tags.tag_secret_as("dialog-secret", &r, !value.is_empty(), true);
                    focus(&r, focused);
                    entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    match keep {
                        Keep::NotOffered => {}
                        Keep::Yes { .. } => {
                            let r = ui.add(Checkbox::new(save, "Save in keyring"));
                            tags.tag_value_as("dialog-save", &r, *save, true);
                        }
                        Keep::No(why) => {
                            let mut off = false;
                            ui.horizontal_top(|ui| {
                                let r = ui.add_enabled(false, Checkbox::new(&mut off, "Save in keyring"));
                                tags.tag_value_as("dialog-save", &r, false, false);
                                ui.label(Text::Small.rich(*why).color(t.text_muted));
                            });
                        }
                    }
                    buttons[OK].enabled = !value.is_empty();
                })
                .buttons([ok(ok_label), cancel])
                .show(ctx);
            let pressed = tagged(tags, &shown, line);
            if pressed == Some(CANCEL) {
                Act::Cancel
            } else if (pressed == Some(OK) || entered) && !value.is_empty() {
                let mut params = with(rerun.params.take(), ask.param(), json!(std::mem::take(value)));
                if *save && matches!(keep, Keep::Yes { .. }) {
                    params = with(params, "savePassword", json!(true));
                }
                Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
            } else {
                Act::None
            }
        }
        Dialog::Confirm { message, what, rerun } => {
            let (heading, button) = confirm_words(what);
            let shown = dialog::Dialog::new(id, heading).text(message.as_str()).buttons([Button::danger(button, "dialog-ok"), cancel]).show(ctx);
            match tagged(tags, &shown, message) {
                Some(OK) => {
                    let params = with(rerun.params.take(), "force", json!(true));
                    Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
                }
                Some(_) => Act::Cancel,
                None => Act::None,
            }
        }
        Dialog::EndTransaction { ask, rerun } => {
            let (title, text) = ask.words();
            let only_rollback = ask.only_rollback();
            let buttons =
                if only_rollback { vec![ok("Rollback"), cancel] } else { vec![ok("Commit"), Button::outline("Rollback", "dialog-rollback"), cancel] };
            let last = buttons.len().saturating_sub(1);
            let shown = dialog::Dialog::new(id, title).text(text.clone()).buttons(buttons).show(ctx);
            let end = match tagged(tags, &shown, &text).or_else(|| entered(ctx, &shown).then_some(OK)) {
                None => return Act::None,
                Some(i) if i == last => return Act::Cancel,
                Some(OK) if !only_rollback => "commit",
                Some(_) => "rollback",
            };
            let params = with(rerun.params.take(), "end", json!(end));
            Act::Rerun(Rerun { command: rerun.command, params, origin: rerun.origin })
        }
        Dialog::DeleteConnection { id: connection, name } => {
            let text = Words::from("Delete “").name(name.as_str()).plain("”? Its saved passwords are deleted with it.");
            let shown =
                dialog::Dialog::new(id, "Delete connection").text(text.clone()).buttons([Button::danger("Delete", "dialog-ok"), cancel]).show(ctx);
            match tagged(tags, &shown, &text) {
                Some(OK) => Act::Delete(connection.clone()),
                Some(_) => Act::Cancel,
                None => Act::None,
            }
        }
        Dialog::Requested { name } => {
            let text = Words::from("A command asked for the “").name(*name).plain("” dialog, which this version of dbee does not have.");
            // Its one button, Close, is its cancel too: Escape presses it.
            let shown = dialog::Dialog::new(id, name).text(text.clone()).buttons([ok("Close")]).show(ctx);
            if tagged(tags, &shown, &text).is_some() { Act::Cancel } else { Act::None }
        }
    }
}

/// Tags what `shown` drew, its text (`text`, as plain words) and its buttons; the button pressed.
fn tagged(tags: &mut Tags, shown: &dialog::Shown, text: impl std::fmt::Display) -> Option<usize> {
    if let Some(r) = &shown.text {
        tags.tag_value_as("dialog-text", r, text.to_string(), true);
    }
    for (button, r) in &shown.buttons {
        tags.tag_value_as(button.tag, r, button.label, button.enabled);
    }
    shown.pressed
}

/// Enter while no widget has the keyboard presses the main button, once the dialog takes input
/// (egui draws its first frame disabled). On a button the keyboard reached, Enter presses that one.
fn entered(ctx: &egui::Context, shown: &dialog::Shown) -> bool {
    let live = shown.buttons.first().is_some_and(|(_, r)| r.enabled());
    live && ctx.memory(|m| m.focused().is_none()) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(Text::Small.rich(text).color(theme::tokens(ui.ctx()).text_muted)).wrap());
}

/// Asks for focus until the field has it (the first frame of a dialog may not grant it).
fn focus(r: &egui::Response, focused: &mut bool) {
    if !*focused {
        r.request_focus();
        *focused = r.has_focus();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dbee_config::MemoryStore;
    use dbee_engine::{Session, SessionOptions};
    use dbee_testkit::temp_home;
    use serde_json::json;

    use super::*;

    fn line_of(app: &DbeeApp) -> Words {
        match &app.view.dialog {
            Some(Dialog::Secret { line, .. }) => line.clone(),
            other => panic!("not a secret dialog: {other:?}"),
        }
    }

    use egui::{Event, Key, Rect, Theme, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::components::{Edge, both, edges_in, fills_in, sections, showcase, texts};

    /// What a dialog's answer does, as `draw` says it (the params with any secret in them).
    #[derive(Debug, PartialEq)]
    enum Done {
        Cancel,
        Rerun(&'static str, Json),
        ParseUrl(String),
        Delete(String),
    }

    /// A dialog drawn by `draw` each frame, with its tags and what it answered.
    struct Open {
        dialog: Dialog,
        tags: Tags,
        done: Vec<Done>,
    }

    fn open(theme: Theme, dialog: Dialog) -> Harness<'static, Open> {
        let mut ready = false;
        let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).build_ui_state(
            move |ui, s: &mut Open| {
                if !ready {
                    showcase::prepare(ui.ctx(), theme);
                    ready = true;
                    ui.ctx().request_repaint();
                    return;
                }
                s.tags.begin_frame();
                let done = match draw(ui.ctx(), &mut s.dialog, &mut s.tags) {
                    Act::None => None,
                    Act::Cancel => Some(Done::Cancel),
                    Act::Rerun(r) => Some(Done::Rerun(r.command, r.params)),
                    Act::ParseUrl(url) => Some(Done::ParseUrl(url)),
                    Act::Delete(id) => Some(Done::Delete(id)),
                };
                s.done.extend(done);
            },
            Open { dialog, tags: Tags::default(), done: Vec::new() },
        );
        h.run();
        h
    }

    fn rerun() -> Rerun {
        Rerun { command: connections::OPEN, params: json!({ "id": "c1" }), origin: Origin::Menu }
    }

    fn secret(ask: SecretAsk, keep: Keep) -> Dialog {
        let save = matches!(keep, Keep::Yes { checked: true });
        Dialog::Secret {
            ask,
            line: "Enter the password of ann on db.example.".into(),
            value: String::new(),
            keep,
            save,
            focused: false,
            rerun: rerun(),
        }
    }

    fn confirm(what: &str) -> Dialog {
        Dialog::Confirm { message: "Close Query 1? Its query text will be lost.".into(), what: what.into(), rerun: rerun() }
    }

    /// Every dialog, by what `ui.inspect` names it.
    fn every() -> Vec<(&'static str, Dialog)> {
        vec![
            ("fillFromUrl", Dialog::fill_from_url(vec![DbKind::Postgres])),
            (
                "hostKey",
                Dialog::HostKey { host: "db.example:22".into(), key_type: "ssh-ed25519".into(), fingerprint: "SHA256:abcdef".into(), rerun: rerun() },
            ),
            ("password", secret(SecretAsk::Password, Keep::Yes { checked: true })),
            (
                "passphrase",
                secret(SecretAsk::Passphrase { key: "/home/ann/.ssh/id_ed25519".into() }, Keep::No("There is no keyring, so dbee asks each time.")),
            ),
            ("sshPassword", secret(SecretAsk::SshPassword, Keep::NotOffered)),
            ("confirm", confirm("closeTab")),
            ("confirm", confirm("applyAfterUnknownOutcome")),
            ("deleteConnection", Dialog::DeleteConnection { id: "c1".into(), name: "Shop live".into() }),
            ("transactionEnd", end(TransactionState::Open, EndAbout::Tab, false, None)),
            ("transactionEnd", end(TransactionState::Failed, EndAbout::Connection, false, None)),
            ("export", Dialog::Requested { name: "export" }),
        ]
    }

    /// The transaction prompt for closing Query 1 (about the tab) or Shop live (about the
    /// connection), the transaction in `state` on Shop live; `running` when Query 1's run still
    /// goes in it.
    fn end(state: TransactionState, about: EndAbout, running: bool, lost: Option<&str>) -> Dialog {
        let name = match about {
            EndAbout::Tab => "Query 1",
            EndAbout::Connection => "Shop live",
        };
        let ask = EndAsk {
            state,
            about,
            name: name.into(),
            connection: "Shop live".into(),
            lost: lost.map(Into::into),
            running: running.then(|| Running { title: "Query 1".into(), count: 1 }),
        };
        Dialog::EndTransaction { ask, rerun: Rerun { command: TAB_CLOSE, params: json!({ "tab": 1 }), origin: Origin::Menu } }
    }

    /// The prompt's cases: the dialog, its title, its text by part, and its buttons' words from
    /// the right.
    fn prompts() -> Vec<(Dialog, &'static str, Vec<(&'static str, Text)>, Vec<&'static str>)> {
        use EndAbout::{Connection, Tab};
        use Text::{Body, Data};
        use TransactionState::{Failed, Open, Unknown};
        let three = || vec!["Commit", "Rollback", "Cancel"];
        let two = || vec!["Rollback", "Cancel"];
        vec![
            (
                end(Open, Tab, false, None),
                "Commit the transaction?",
                vec![("Query 1", Data), (" opened a transaction that isn't committed yet.", Body)],
                three(),
            ),
            (
                end(Open, Connection, false, None),
                "Commit the transaction?",
                vec![("Shop live", Data), (" has a transaction that isn't committed yet.", Body)],
                three(),
            ),
            (
                end(Failed, Connection, false, None),
                "Roll back the transaction?",
                vec![("The transaction on ", Body), ("Shop live", Data), (" failed, so it can only be rolled back.", Body)],
                two(),
            ),
            (
                end(Open, Tab, true, None),
                "Roll back the transaction?",
                vec![
                    ("Query 1", Data),
                    (" is still running in the transaction on ", Body),
                    ("Shop live", Data),
                    (". Closing stops it, so the transaction can only be rolled back.", Body),
                ],
                two(),
            ),
            (
                end(Unknown, Connection, false, None),
                "Commit the transaction?",
                vec![("dbee can't tell whether ", Body), ("Shop live", Data), (" still has a transaction open.", Body)],
                three(),
            ),
            // What the close loses besides follows the prompt's own words, after one space.
            (
                end(Open, Tab, false, Some("Its query text will be lost.")),
                "Commit the transaction?",
                vec![("Query 1", Data), (" opened a transaction that isn't committed yet. Its query text will be lost.", Body)],
                three(),
            ),
        ]
    }

    /// The tag `id`'s entry in `ui.inspect`'s list.
    fn entry(h: &Harness<'_, Open>, id: &str) -> Option<Json> {
        h.state().tags.to_json().as_array().and_then(|a| a.iter().find(|e| e["id"] == id).cloned())
    }

    fn rect(h: &Harness<'_, Open>, id: &str) -> Rect {
        h.state().tags.rect(id).unwrap_or_else(|| panic!("`{id}` drawn: {:?}", h.state().tags.ids()))
    }

    /// Types `text` into the focused field, then a frame.
    fn type_in(h: &mut Harness<'_, Open>, text: &str) {
        h.event(Event::Text(text.into()));
        h.run();
    }

    #[test]
    fn escape_cancels_every_dialog() {
        for (name, dialog) in every() {
            let mut h = open(Theme::Light, dialog);
            assert!(h.state().done.is_empty(), "{name}: open");
            h.key_press(Key::Escape);
            h.run();
            assert_eq!(h.state().done, [Done::Cancel], "{name}: Escape cancels");
        }
    }

    #[test]
    fn a_click_on_the_scrim_does_nothing() {
        // The design: a click on the scrim leaves the dialog open and answers nothing; Escape and
        // Cancel still close it.
        for (name, dialog) in every() {
            let mut h = open(Theme::Light, dialog);
            let ok = rect(&h, "dialog-ok");
            for at in [egui::pos2(20.0, 20.0), egui::pos2(500.0, 690.0), egui::pos2(ok.right() + 60.0, ok.center().y)] {
                h.hover_at(at);
                h.drag_at(at);
                h.drop_at(at);
                h.run();
            }
            assert!(h.state().done.is_empty(), "{name}: the scrim answered nothing: {:?}", h.state().done);
            assert_eq!(rect(&h, "dialog-ok"), ok, "{name}: still open, where it was");
        }
    }

    #[test]
    fn enter_submits_the_field_dialogs() {
        let mut h = open(Theme::Light, Dialog::fill_from_url(vec![DbKind::Postgres]));
        type_in(&mut h, "postgres://ann@db.example/shop");
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().done, [Done::ParseUrl("postgres://ann@db.example/shop".into())], "Enter in the URL field fills");
        let mut h = open(Theme::Light, secret(SecretAsk::Password, Keep::Yes { checked: true }));
        type_in(&mut h, "pw-4");
        h.key_press(Key::Enter);
        h.run();
        let params = json!({ "id": "c1", "password": "pw-4", "savePassword": true });
        assert_eq!(h.state().done, [Done::Rerun(connections::OPEN, params)], "Enter in the password field connects");
        // Nothing typed: Enter does nothing.
        let mut h = open(Theme::Light, secret(SecretAsk::SshPassword, Keep::NotOffered));
        h.key_press(Key::Enter);
        h.run();
        assert!(h.state().done.is_empty(), "nothing typed, nothing sent");
    }

    #[test]
    fn the_url_error_keeps_its_line_and_takes_the_red_edge() {
        // Node 162794: the reason a URL didn't parse in `small` `danger` under the field, which
        // takes a red edge even while it has focus; with no reason the line's room stays, so the
        // buttons don't move.
        for (theme, t) in both() {
            let mut h = open(theme, Dialog::fill_from_url(vec![DbKind::Postgres]));
            let (url, ok) = (rect(&h, "dialog-url"), rect(&h, "dialog-ok"));
            let shapes = h.output().shapes.clone();
            assert_eq!(
                edges_in(&shapes, url, 0.0),
                [Edge { outer: url, width: 2.0, colour: t.ink, radius: egui::CornerRadius::same(2) }],
                "{theme:?}: focused"
            );
            if let Dialog::FillFromUrl { error, .. } = &mut h.state_mut().dialog {
                *error = Some("Not a database URL".into());
            }
            h.run();
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-url")), "{theme:?}: still focused");
            let shapes = h.output().shapes.clone();
            let red = Edge { outer: url, width: 1.0, colour: t.danger, radius: egui::CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, url, 0.0), [red], "{theme:?}: the red edge wins over the focus edge");
            let line = rect(&h, "dialog-error");
            assert_eq!(h.state().tags.value("dialog-error"), Some(&json!("Not a database URL")), "{theme:?}");
            let drawn = texts(&shapes).into_iter().find(|(w, _, _)| w == "Not a database URL");
            let (_, at, colour) = drawn.unwrap_or_else(|| panic!("{theme:?}: the reason drawn"));
            assert_eq!(colour, t.danger, "{theme:?}: in danger");
            assert!((at.top() - url.bottom() - 4.0).abs() < 0.5 && line.left() == url.left(), "{theme:?}: under the field: {at:?}");
            assert_eq!((rect(&h, "dialog-url"), rect(&h, "dialog-ok")), (url, ok), "{theme:?}: nothing moved");
        }
    }

    #[test]
    fn fill_from_url_has_no_intro_line() {
        // The design's dialog is its title, the field and the error line: the field 20 under the
        // title, no words between them and no `dialog-text` (the error names the schemes when a
        // paste is wrong).
        let h = open(Theme::Light, Dialog::fill_from_url(vec![DbKind::Postgres, DbKind::MySql]));
        let mut ids = h.state().tags.ids();
        ids.sort_unstable();
        assert_eq!(ids, ["dialog-cancel", "dialog-ok", "dialog-url"], "no dialog-text");
        let drawn = texts(&h.output().shapes);
        let title = drawn.iter().find(|(w, _, _)| w == "Fill from URL").map(|(_, r, _)| *r).unwrap_or_else(|| panic!("the title: {drawn:?}"));
        let url = rect(&h, "dialog-url");
        assert!((url.top() - title.bottom() - 20.0).abs() < 0.5, "the field 20 under the title: {title:?} {url:?}");
        let between: Vec<&String> =
            drawn.iter().filter(|(_, r, _)| r.top() >= title.bottom() && r.bottom() <= url.top()).map(|(w, _, _)| w).collect();
        assert!(between.is_empty(), "nothing between the title and the field: {between:?}");
        assert!(!drawn.iter().any(|(w, _, _)| w.contains("Paste")), "no intro sentence: {drawn:?}");
    }

    #[test]
    fn a_password_dialog_focuses_its_field_with_the_focus_edge() {
        // Review Focus 3: the field has focus as the dialog opens, given by code, and shows it.
        for (theme, t) in both() {
            let h = open(theme, secret(SecretAsk::Password, Keep::Yes { checked: false }));
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-secret")), "{theme:?}: the field has focus");
            let at = rect(&h, "dialog-secret");
            let ink = Edge { outer: at, width: 2.0, colour: t.ink, radius: egui::CornerRadius::same(2) };
            assert_eq!(edges_in(&h.output().shapes, at, 0.0), [ink], "{theme:?}: and shows it");
        }
    }

    #[test]
    fn tab_reaches_every_part_of_the_secret_dialog() {
        // Review Focus 3: from the field, Tab goes to the checkbox (Space ticks it), Cancel, then
        // Connect, in the order they read.
        let mut h = open(Theme::Light, secret(SecretAsk::Password, Keep::Yes { checked: false }));
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-secret")), "the field has focus");
        type_in(&mut h, "pw");
        for label in ["Save in keyring", "Cancel", "Connect"] {
            h.key_press(Key::Tab);
            h.run();
            assert!(h.get_by_label(label).is_focused(), "Tab reached {label}");
            if label == "Save in keyring" {
                h.key_press(Key::Space);
                h.run();
                assert_eq!(h.state().tags.value("dialog-save"), Some(&json!(true)), "Space ticks it");
            }
        }
        h.key_press(Key::Enter);
        h.run();
        let params = json!({ "id": "c1", "password": "pw", "savePassword": true });
        assert_eq!(h.state().done, [Done::Rerun(connections::OPEN, params)], "Enter on Connect connects");
    }

    #[test]
    fn dialogs_keep_their_tags_and_outcomes() {
        let host_text = "dbee has not connected to db.example:22 before. Its ssh-ed25519 host key has this fingerprint:";
        let rows: Vec<(Dialog, Option<&str>, Vec<&str>, &str, Option<&str>, Done)> = vec![
            (
                Dialog::fill_from_url(vec![DbKind::Postgres]),
                None,
                vec!["dialog-url", "dialog-ok", "dialog-cancel"],
                "Fill",
                Some("postgres://h/db"),
                Done::ParseUrl("postgres://h/db".into()),
            ),
            (
                Dialog::HostKey { host: "db.example:22".into(), key_type: "ssh-ed25519".into(), fingerprint: "SHA256:abcdef".into(), rerun: rerun() },
                Some(host_text),
                vec!["dialog-fingerprint", "dialog-text", "dialog-ok", "dialog-cancel"],
                "Trust and connect",
                None,
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "trustHostKey": "SHA256:abcdef" })),
            ),
            (
                secret(SecretAsk::Password, Keep::Yes { checked: true }),
                Some("Enter the password of ann on db.example."),
                vec!["dialog-secret", "dialog-save", "dialog-text", "dialog-ok", "dialog-cancel"],
                "Connect",
                Some("pw"),
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "password": "pw", "savePassword": true })),
            ),
            (
                secret(SecretAsk::Passphrase { key: "/k".into() }, Keep::No("There is no keyring, so dbee asks each time.")),
                Some("Enter the password of ann on db.example."),
                vec!["dialog-secret", "dialog-save", "dialog-text", "dialog-ok", "dialog-cancel"],
                "Connect",
                Some("pp"),
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "sshPassphrase": "pp" })),
            ),
            (
                secret(SecretAsk::SshPassword, Keep::NotOffered),
                Some("Enter the password of ann on db.example."),
                vec!["dialog-secret", "dialog-text", "dialog-ok", "dialog-cancel"],
                "Connect",
                Some("sp"),
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "sshPassword": "sp" })),
            ),
            (
                confirm("closeTab"),
                Some("Close Query 1? Its query text will be lost."),
                vec!["dialog-text", "dialog-ok", "dialog-cancel"],
                "Close anyway",
                None,
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "force": true })),
            ),
            (
                confirm("applyAfterUnknownOutcome"),
                Some("Close Query 1? Its query text will be lost."),
                vec!["dialog-text", "dialog-ok", "dialog-cancel"],
                "Apply again",
                None,
                Done::Rerun(connections::OPEN, json!({ "id": "c1", "force": true })),
            ),
            (
                Dialog::DeleteConnection { id: "c1".into(), name: "Shop live".into() },
                Some("Delete “Shop live”? Its saved passwords are deleted with it."),
                vec!["dialog-text", "dialog-ok", "dialog-cancel"],
                "Delete",
                None,
                Done::Delete("c1".into()),
            ),
            (
                Dialog::Requested { name: "export" },
                Some("A command asked for the “export” dialog, which this version of dbee does not have."),
                vec!["dialog-text", "dialog-ok"],
                "Close",
                None,
                Done::Cancel,
            ),
        ];
        for (dialog, text, ids, ok, typed, done) in rows {
            let name = dialog.name();
            let at = format!("{name} {ok}");
            // OK.
            let mut h = open(Theme::Light, dialog);
            let mut drawn: Vec<&str> = h.state().tags.ids();
            drawn.sort_unstable();
            let mut want = ids.clone();
            want.sort_unstable();
            assert_eq!(drawn, want, "{at}: its tags");
            assert_eq!(h.state().tags.value("dialog-text"), text.map(|t| json!(t)).as_ref(), "{at}: dialog-text");
            assert_eq!(h.state().tags.value("dialog-ok"), Some(&json!(ok)), "{at}: dialog-ok");
            if ids.contains(&"dialog-cancel") {
                assert_eq!(h.state().tags.value("dialog-cancel"), Some(&json!("Cancel")), "{at}: dialog-cancel");
                assert!(entry(&h, "dialog-cancel").is_some_and(|e| e.get("enabled").is_none()), "{at}: Cancel is on");
            }
            let on = entry(&h, "dialog-ok").is_some_and(|e| e.get("enabled").is_none());
            assert_eq!(on, typed.is_none(), "{at}: OK is off until something is typed");
            if let Some(typed) = typed {
                type_in(&mut h, typed);
                assert!(entry(&h, "dialog-ok").is_some_and(|e| e.get("enabled").is_none()), "{at}: typed, OK is on");
            }
            h.get_by_label(ok).click();
            h.run();
            assert_eq!(h.state().done.first(), Some(&done), "{at}: what OK does");
        }
        // Cancel, where there is one.
        for (name, dialog) in every() {
            let mut h = open(Theme::Light, dialog);
            if h.state().tags.rect("dialog-cancel").is_none() {
                continue;
            }
            h.get_by_label("Cancel").click();
            h.run();
            assert_eq!(h.state().done, [Done::Cancel], "{name}: Cancel");
        }
    }

    #[test]
    fn close_anyway_apply_again_and_delete_are_danger_buttons() {
        // The design: Close anyway, Apply again and Delete are `danger` with `on_danger` words;
        // Fill, Trust and connect, Connect and Close are the `ink` main button; Cancel an outline.
        for (theme, t) in both() {
            for (name, dialog) in every() {
                let h = open(theme, dialog);
                let shapes = h.output().shapes.clone();
                let ok = rect(&h, "dialog-ok");
                let label = h.state().tags.value("dialog-ok").and_then(Json::as_str).unwrap_or_default().to_owned();
                let at = format!("{theme:?} {name} {label}");
                let danger = ["Close anyway", "Apply again", "Delete"].contains(&label.as_str());
                let enabled = entry(&h, "dialog-ok").is_some_and(|e| e.get("enabled").is_none());
                let (ground, words) = match (danger, enabled) {
                    (true, _) => (t.danger, t.on_danger),
                    (false, true) => (t.ink, t.on_ink),
                    (false, false) => (t.fill, t.text_faint),
                };
                assert_eq!(fills_in(&shapes, ok), [(ok, ground)], "{at}: its ground");
                let caps = label.to_uppercase();
                let drawn = texts(&shapes).into_iter().find(|(w, _, _)| *w == caps).map(|(_, _, c)| c);
                assert_eq!(drawn, Some(words), "{at}: its words");
                if let Some(cancel) = h.state().tags.rect("dialog-cancel") {
                    assert_eq!(fills_in(&shapes, cancel), [], "{at}: Cancel has no ground");
                    let line = Edge { outer: cancel, width: 1.0, colour: t.line_strong, radius: egui::CornerRadius::same(2) };
                    assert_eq!(edges_in(&shapes, cancel, 0.0), [line], "{at}: Cancel's outline");
                }
            }
        }
    }

    #[test]
    fn the_transaction_prompt_matches_the_page() {
        // Nodes 163357 (light) and 163678 (dark), the dialog with three buttons: the title, the
        // text in `body` with the names in `data` as written, then CANCEL, ROLLBACK and COMMIT
        // from the left, 8 apart, COMMIT the main button in `ink` and the others outlined. A
        // failed transaction, or a run still going in it, offers CANCEL and ROLLBACK (main).
        for (theme, t) in both() {
            for (dialog, title, text, labels) in prompts() {
                let whole: String = text.iter().map(|(w, _)| *w).collect();
                let at = format!("{theme:?} `{whole}`");
                let h = open(theme, dialog);
                let shapes = h.output().shapes.clone();
                assert_eq!(h.state().tags.value("dialog-text"), Some(&json!(whole)), "{at}: dialog-text as plain words");
                let want: Vec<_> = text.iter().map(|(w, style)| ((*w).to_owned(), style.font(), t.text)).collect();
                assert_eq!(sections(&shapes, &whole), want, "{at}: the names in data, the rest in body");
                let drawn = texts(&shapes);
                assert!(drawn.iter().any(|(w, _, c)| w == title && *c == t.text), "{at}: the title `{title}`: {drawn:?}");
                assert_eq!(font_of(&shapes, title), Some(Text::Title.font()), "{at}: the title in title");
                let tags = if labels.len() == 3 { vec!["dialog-ok", "dialog-rollback", "dialog-cancel"] } else { vec!["dialog-ok", "dialog-cancel"] };
                let mut ids = h.state().tags.ids();
                ids.sort_unstable();
                let mut want_ids = tags.clone();
                want_ids.push("dialog-text");
                want_ids.sort_unstable();
                assert_eq!(ids, want_ids, "{at}: its tags");
                let mut left_of: Option<f32> = None;
                for (i, (tag, label)) in tags.iter().zip(&labels).enumerate() {
                    let at = format!("{at} {tag}");
                    assert_eq!(h.state().tags.value(tag), Some(&json!(label)), "{at}: its words");
                    let r = rect(&h, tag);
                    if let Some(x) = left_of {
                        assert_eq!(r.right(), x - 8.0, "{at}: 8 left of the button before it: {r:?}");
                    }
                    left_of = Some(r.left());
                    let caps = label.to_uppercase();
                    let words = drawn.iter().find(|(w, on, _)| *w == caps && r.expand(0.5).contains_rect(*on)).map(|(_, _, c)| *c);
                    if i == OK {
                        assert_eq!(fills_in(&shapes, r), [(r, t.ink)], "{at}: the main button, on ink");
                        assert_eq!(words, Some(t.on_ink), "{at}: its words in on_ink");
                    } else {
                        assert_eq!(fills_in(&shapes, r), [], "{at}: no ground");
                        let line = Edge { outer: r, width: 1.0, colour: t.line_strong, radius: egui::CornerRadius::same(2) };
                        assert_eq!(edges_in(&shapes, r, 0.0), [line], "{at}: outlined");
                        assert_eq!(words, Some(t.text), "{at}: its words in text");
                    }
                }
            }
        }
    }

    /// The font `words` were drawn in.
    fn font_of(shapes: &[egui::epaint::ClippedShape], words: &str) -> Option<egui::FontId> {
        crate::components::flat(shapes).into_iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text() == words => t.galley.job.sections.first().map(|s| s.format.font_id.clone()),
            _ => None,
        })
    }

    #[test]
    fn the_transaction_prompt_runs_the_close_again_with_the_end() {
        // COMMIT and Enter run the close again with `end: "commit"`, ROLLBACK with `"rollback"`;
        // CANCEL and Escape leave everything. Enter on a button the keyboard reached presses that
        // one. With only a rollback offered, its main button rolls back.
        use EndAbout::Tab;
        use Key::{Enter, Escape, Tab as Next};
        use TransactionState::{Failed, Open};
        enum Do {
            Click(&'static str),
            Keys(&'static [Key]),
        }
        let rerun = |end: &str| Done::Rerun(TAB_CLOSE, json!({ "tab": 1, "end": end }));
        let rows = [
            (end(Open, Tab, false, None), Do::Click("Commit"), rerun("commit")),
            (end(Open, Tab, false, None), Do::Keys(&[Enter]), rerun("commit")),
            (end(Open, Tab, false, None), Do::Click("Rollback"), rerun("rollback")),
            (end(Open, Tab, false, None), Do::Keys(&[Next, Next, Enter]), rerun("rollback")),
            (end(Open, Tab, false, None), Do::Click("Cancel"), Done::Cancel),
            (end(Open, Tab, false, None), Do::Keys(&[Escape]), Done::Cancel),
            (end(Failed, Tab, false, None), Do::Click("Rollback"), rerun("rollback")),
            (end(Failed, Tab, false, None), Do::Keys(&[Enter]), rerun("rollback")),
            (end(Open, Tab, true, None), Do::Keys(&[Enter]), rerun("rollback")),
            (end(Open, Tab, true, None), Do::Click("Cancel"), Done::Cancel),
        ];
        for (n, (dialog, act, want)) in rows.into_iter().enumerate() {
            let mut h = open(Theme::Light, dialog);
            match act {
                Do::Click(label) => h.get_by_label(label).click(),
                Do::Keys(keys) => {
                    for key in keys {
                        h.key_press(*key);
                        h.run();
                    }
                }
            }
            h.run();
            assert_eq!(h.state().done, [want], "row {n}");
        }
    }

    #[test]
    fn a_close_that_ends_a_transaction_says_how_it_went() {
        // The design's toasts: `Committed on <connection>` or `Rolled back on <connection>`, the
        // name in `data`; an error toast with the reason when the end failed; none when no
        // transaction was open any more, or the connection went meanwhile.
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        let shown = |app: &mut DbeeApp| app.view.notices.drain(..).map(|n| (n.text, n.kind)).collect::<Vec<_>>();
        let commit = Ending { commit: true, connection: "Shop live".into() };
        let rollback = Ending { commit: false, connection: "Shop live".into() };
        let done = |end: &str, note: Option<&str>| Ok(json!({ "closed": 1, "active": null, "ended": end, "note": note }));
        ended(&mut app, &commit, done("commit", None));
        assert_eq!(shown(&mut app), [(Words::from("Committed on ").name("Shop live"), Kind::Success)]);
        ended(&mut app, &rollback, done("rollback", None));
        assert_eq!(shown(&mut app), [(Words::from("Rolled back on ").name("Shop live"), Kind::Success)]);
        // A lost query connection ended it with the session: rolled back all the same.
        ended(&mut app, &rollback, done("rollback", Some(dbee_engine::cmd::query::ENDED_WITH_SESSION)));
        assert_eq!(shown(&mut app), [(Words::from("Rolled back on ").name("Shop live"), Kind::Success)]);
        // Nothing was open any more, or the connection went: no toast.
        ended(&mut app, &commit, done("commit", Some(NO_TRANSACTION)));
        ended(&mut app, &commit, Ok(json!({ "ignored": true })));
        assert_eq!(shown(&mut app), []);
        // A commit the server turned into a rollback.
        ended(&mut app, &commit, done("commit", Some(FAILED_COMMIT)));
        let failed = "The commit failed: the transaction had failed, so the server rolled it back: nothing was committed.";
        assert_eq!(shown(&mut app), [(Words::from(failed), Kind::Error)]);
        // The end failed: the reason, and nothing closed.
        let server = EngineError::Db { code: "server", message: "insert violates foreign key constraint \"fk\"".into(), data: json!({}) };
        ended(&mut app, &commit, Err(server));
        assert_eq!(shown(&mut app), [(Words::from("The commit failed: insert violates foreign key constraint \"fk\"."), Kind::Error)]);
        ended(&mut app, &rollback, Err(EngineError::Busy("a COMMIT is running".into())));
        assert_eq!(shown(&mut app), [(Words::from("The rollback failed: a COMMIT is running."), Kind::Error)]);
    }

    #[test]
    fn names_in_dialogs_are_mono_as_written() {
        // The design's Dialogs (DA): the names, hosts and paths dbee puts in a dialog's text are
        // drawn in `data` as written, the rest in `body`, all in `text`; `dialog-text` still says
        // it all as plain words.
        use Text::{Body, Data};
        let rows = || -> Vec<(Dialog, Vec<(&str, Text)>)> {
            vec![
                (
                    Dialog::DeleteConnection { id: "c1".into(), name: "Shop Live".into() },
                    vec![("Delete “", Body), ("Shop Live", Data), ("”? Its saved passwords are deleted with it.", Body)],
                ),
                (
                    Dialog::HostKey {
                        host: "Db.Example:22".into(),
                        key_type: "ssh-ed25519".into(),
                        fingerprint: "SHA256:abcdef".into(),
                        rerun: rerun(),
                    },
                    vec![
                        ("dbee has not connected to ", Body),
                        ("Db.Example:22", Data),
                        (" before. Its ", Body),
                        ("ssh-ed25519", Data),
                        (" host key has this fingerprint:", Body),
                    ],
                ),
                (
                    Dialog::Secret {
                        ask: SecretAsk::Password,
                        line: Words::default().plain("Enter the password of ").name("Ann").plain(" on ").name("pg14.internal").plain("."),
                        value: String::new(),
                        keep: Keep::NotOffered,
                        save: false,
                        focused: false,
                        rerun: rerun(),
                    },
                    vec![("Enter the password of ", Body), ("Ann", Data), (" on ", Body), ("pg14.internal", Data), (".", Body)],
                ),
                (
                    Dialog::Requested { name: "export" },
                    vec![("A command asked for the “", Body), ("export", Data), ("” dialog, which this version of dbee does not have.", Body)],
                ),
            ]
        };
        for (theme, t) in both() {
            for (dialog, want) in rows() {
                let at = format!("{theme:?} {}", dialog.name());
                let h = open(theme, dialog);
                let whole: String = want.iter().map(|(w, _)| *w).collect();
                assert_eq!(h.state().tags.value("dialog-text"), Some(&json!(whole)), "{at}: dialog-text as plain words");
                let want: Vec<_> = want.into_iter().map(|(w, style)| (w.to_owned(), style.font(), t.text)).collect();
                assert_eq!(sections(&h.output().shapes, &whole), want, "{at}: the names in data, the rest in body");
            }
        }
    }

    #[test]
    fn a_secret_dialog_names_the_connection_its_command_names_by_id() {
        let home = temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            ..SessionOptions::default()
        })
        .unwrap();
        let mut app = DbeeApp::new(session, None);
        let saved = app.run("connection.save", json!({ "config": { "host": "db.example", "user": "ann", "database": "shop" } })).unwrap();
        let Outcome::Done(saved) = saved else { panic!("connection.save ran as a job") };
        let id = saved["id"].as_str().unwrap().to_string();
        let needs = |need: Need| EngineError::Needs { need, message: "asked".into() };
        let rerun = |params: Json| Rerun { command: connections::OPEN, params, origin: Origin::Menu };
        assert!(ask(&mut app, &needs(Need::Password), rerun(json!({ "id": id }))));
        let named = Words::default().plain("Enter the password of ").name("ann").plain(" on ").name("db.example").plain(".");
        assert_eq!(line_of(&app), named, "the user and the host marked as names");
        assert_eq!(line_of(&app).to_string(), "Enter the password of ann on db.example.");
        // A key's path is a name too.
        assert!(ask(&mut app, &needs(Need::Passphrase { key: "/keys/Id_Ed25519".into() }), rerun(json!({ "id": id }))));
        let key = Words::default().plain("The key ").name("/keys/Id_Ed25519").plain(" is protected by a passphrase.");
        assert_eq!(line_of(&app), key, "the key's path marked as a name");
        // An id nothing has, or a connection without a tunnel: no blanks, plain wording.
        assert!(ask(&mut app, &needs(Need::Password), rerun(json!({ "id": "nope" }))));
        assert_eq!(line_of(&app).to_string(), "Enter the database password.");
        assert!(ask(&mut app, &needs(Need::SshPassword), rerun(json!({ "id": id }))));
        assert_eq!(line_of(&app).to_string(), "Enter the SSH password.");
    }
}

//! The control channel's methods (`docs/control-protocol.md`): what a request does in the app.
//!
//! [`crate::control_server`] reads the request lines and hands each to the UI thread as a
//! [`ControlRequest`]; the app handles them in step 3 of its frame ([`DbeeApp::ui`]). A reply is
//! `{"ok": true, "result": …}` or `{"ok": false, "error": …, "code": …, "data": …}` (engine
//! errors carry `EngineError::code` and `data`); the server adds the request's `id`.
//!
//! Most replies go out at once. These wait, and are answered in step 2 of a later frame:
//! - `engine.execute` of a command that starts a job (unless `wait: false`): until the job's
//!   result is applied;
//! - `engine.wait`: until no job runs;
//! - `ui.click`, `ui.key`, `ui.text`: until the frame that received the input has run (a click
//!   on a widget scrolled out of view first waits, a few frames at most, for it to be scrolled
//!   into view);
//! - `ui.screenshot`: until the picture of the window arrives. A file goes only in the run's one
//!   screenshot folder ([`DbeeApp::with_screenshot_dir`]), under a plain file name.
//!
//! `app.quit` is answered at once and closes the window a frame later (at least
//! `QUIT_DELAY` on), so its reply is written before the app exits.
//!
//! A wait gives up after the request's `timeoutMs` ([`DEFAULT_TIMEOUT`], at most
//! [`MAX_TIMEOUT`]) with a `timeout` error; a job goes on. Requests are hostile input: a wrong
//! type is `badParams`, never a panic. Logs name the method, never params or results.

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use dbee_config::store::replace_private_file;
use dbee_engine::view::{INSPECT_TEXT_BYTES, cut};
use dbee_engine::{EngineError, JobId, Json, Outcome, allowed_file};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use serde_json::{Map, json};

use crate::DbeeApp;

/// How long a reply waits when the request gives no `timeoutMs`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest `timeoutMs` honoured.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// How long after its reply `app.quit` closes the window (at the earliest, the next frame).
const QUIT_DELAY: Duration = Duration::from_millis(100);

/// How many frames `ui.click` waits for a widget to be scrolled into view.
const IN_VIEW_FRAMES: u32 = 30;

/// Most synthetic input batches waiting for frames; past it input requests are `busy`.
const MAX_QUEUED_BATCHES: usize = 10_000;

/// The methods, for logging: anything else is caller text and is not logged.
const METHODS: &[&str] = &[
    "engine.execute",
    "engine.commands",
    "engine.journal",
    "engine.wait",
    "document.inspect",
    "ui.inspect",
    "ui.click",
    "ui.focus",
    "ui.key",
    "ui.text",
    "ui.screenshot",
    "app.quit",
];

/// One request on its way to the UI thread, with the channel its reply goes back on.
pub struct ControlRequest {
    pub method: String,
    pub params: Json,
    pub(crate) reply: Sender<Json>,
    /// When the sender stops waiting (the server then answers `timeout`): a request still queued
    /// at that instant is dropped, not run. `None` waits as long as it takes.
    pub deadline: Option<Instant>,
}

impl std::fmt::Debug for ControlRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Params can hold passwords and SQL.
        f.debug_struct("ControlRequest").field("method", &self.method).finish_non_exhaustive()
    }
}

impl ControlRequest {
    /// A request and the receiver its reply (without the `id`) arrives on.
    pub fn new(method: impl Into<String>, params: Json) -> (ControlRequest, Receiver<Json>) {
        let (reply, rx) = channel();
        (ControlRequest { method: method.into(), params, reply, deadline: None }, rx)
    }
}

/// `{"ok": true, "result": result}`.
pub fn ok(result: Json) -> Json {
    json!({ "ok": true, "result": result })
}

/// `{"ok": false, "error": message, "code": code, "data": data}`.
pub fn error(code: &str, message: impl Into<String>, data: Json) -> Json {
    json!({ "ok": false, "error": message.into(), "code": code, "data": data })
}

fn bad(message: impl Into<String>) -> Json {
    error("badParams", message, json!({}))
}

/// An engine error as a reply: its text, `code` and `data`.
pub fn engine_error(e: &EngineError) -> Json {
    error(e.code(), e.to_string(), e.data())
}

fn reply_of(r: Result<Json, EngineError>) -> Json {
    match r {
        Ok(v) => ok(v),
        Err(e) => engine_error(&e),
    }
}

/// `reply` with `id` first.
pub(crate) fn with_id(reply: Json, id: &Json) -> Json {
    let mut out = Map::new();
    out.insert("id".into(), id.clone());
    if let Json::Object(fields) = reply {
        out.extend(fields);
    }
    Json::Object(out)
}

/// The start of caller text, for echoing it in an error.
fn shown(text: &str) -> String {
    const MAX: usize = 80;
    if text.chars().count() > MAX { format!("{}…", text.chars().take(MAX).collect::<String>()) } else { text.to_string() }
}

/// A request line, parsed.
#[derive(Debug)]
pub(crate) struct Request {
    pub id: Json,
    pub method: String,
    pub params: Json,
    pub timeout: Duration,
}

/// Parses one request line; `Err` is the error reply, with the `id` when the line has one.
pub(crate) fn parse_request(line: &[u8]) -> Result<Request, Json> {
    let v: Json = serde_json::from_slice(line).map_err(|e| with_id(error("invalidJson", format!("invalid JSON: {e}"), json!({})), &Json::Null))?;
    let Json::Object(mut map) = v else {
        return Err(with_id(error("badRequest", "a request is a JSON object: {\"id\", \"method\", \"params\"}", json!({})), &Json::Null));
    };
    let id = map.remove("id").unwrap_or(Json::Null);
    let fail = |msg: &str| with_id(error("badRequest", msg, json!({})), &id);
    let method = match map.remove("method") {
        Some(Json::String(m)) => m,
        _ => return Err(fail("`method` is required (a string)")),
    };
    let params = match map.remove("params") {
        None | Some(Json::Null) => json!({}),
        Some(p @ Json::Object(_)) => p,
        Some(_) => return Err(fail("`params` must be a JSON object")),
    };
    let timeout = timeout_of(&params).map_err(|msg| with_id(bad(msg), &id))?;
    Ok(Request { id, method, params, timeout })
}

/// The request's `timeoutMs`, clamped to [`MAX_TIMEOUT`]; [`DEFAULT_TIMEOUT`] without one.
pub(crate) fn timeout_of(params: &Json) -> Result<Duration, String> {
    let max = MAX_TIMEOUT.as_millis() as f64;
    match params.get("timeoutMs") {
        None | Some(Json::Null) => Ok(DEFAULT_TIMEOUT),
        Some(v) => match v.as_f64() {
            Some(ms) if ms >= 0.0 => Ok(Duration::from_millis(ms.min(max) as u64)),
            _ => Err("`timeoutMs` must be a number of milliseconds, 0 or more".into()),
        },
    }
}

/// Handles one request line on the app, as the server and the frame would: the reply now, or
/// `None` when it waits for later frames. For tests and tools that hold the app.
pub fn handle_line(app: &mut DbeeApp, ctx: &egui::Context, line: &str) -> Option<Json> {
    let req = match parse_request(line.as_bytes()) {
        Ok(r) => r,
        Err(reply) => return Some(reply),
    };
    let (request, rx) = ControlRequest::new(req.method, req.params);
    handle(app, ctx, request);
    rx.try_recv().ok().map(|r| with_id(r, &req.id))
}

/// What a waiting reply waits for.
#[derive(Debug)]
pub(crate) enum Wait {
    Job(JobId),
    Idle,
    /// The input `ticket` was fed to a frame that has run; then `result`.
    Input {
        ticket: u64,
        result: Json,
    },
    /// The picture `token`, asked for once `asked`; saved as `file` or sent as base64.
    Shot {
        token: u64,
        file: Option<ShotFile>,
        asked: bool,
    },
    /// `ui.click` (its `params`) on widget `id`, which is being scrolled into view; `frames`
    /// counts the frames waited.
    InView {
        id: String,
        params: Json,
        frames: u32,
    },
}

/// A reply that waits ([`Wait`]) until `deadline`.
#[derive(Debug)]
pub(crate) struct Deferred {
    wait: Wait,
    reply: Sender<Json>,
    deadline: Instant,
}

enum Handled {
    Now(Json),
    Later(Wait),
}

/// Handles `req` (step 3 of the frame). A request past its deadline is not run: its sender has
/// already given up (the server answered `timeout`), and running it now, a `query.run` say,
/// could run it twice when the caller retries.
pub(crate) fn handle(app: &mut DbeeApp, ctx: &egui::Context, req: ControlRequest) {
    let started = Instant::now();
    let ControlRequest { method, params, reply, deadline } = req;
    let logged = METHODS.iter().find(|m| **m == method).copied().unwrap_or("(unknown)");
    if deadline.is_some_and(|d| started >= d) {
        log::warn!("control {logged}: dropped without running it, its reply had already timed out");
        let _ = reply.send(error("timeout", "the request waited past its deadline before the app could take it, so it was not run", json!({})));
        return;
    }
    let timeout = timeout_of(&params).unwrap_or(DEFAULT_TIMEOUT);
    let wait_until = started.checked_add(timeout).unwrap_or(started);
    let wait_until = deadline.map_or(wait_until, |d| d.min(wait_until));
    let handled = dispatch(app, ctx, &method, params);
    match handled {
        Handled::Now(r) => {
            let _ = reply.send(r);
        }
        Handled::Later(wait) => app.deferred.push(Deferred { wait, reply, deadline: wait_until }),
    }
    log::debug!("control {logged}: handled in {:?}", started.elapsed());
    ctx.request_repaint();
}

fn dispatch(app: &mut DbeeApp, ctx: &egui::Context, method: &str, params: Json) -> Handled {
    match method {
        "engine.execute" => execute(app, params),
        "engine.commands" => Handled::Now(commands(app, &params)),
        "engine.journal" => {
            let entries: Vec<Json> = app.session().journal().iter().map(|(id, p)| json!({ "command": id, "params": p })).collect();
            Handled::Now(ok(Json::Array(entries)))
        }
        "engine.wait" if app.session().busy() => Handled::Later(Wait::Idle),
        "engine.wait" => Handled::Now(ok(json!({}))),
        "document.inspect" => Handled::Now(ok(document(app))),
        "ui.inspect" => Handled::Now(ok(ui_inspect(app, ctx))),
        "ui.click" => click(app, &params),
        "ui.focus" => Handled::Now(focus(app, ctx, &params)),
        "ui.key" => key(app, &params),
        "ui.text" => match params.get("text") {
            Some(Json::String(t)) => input(app, vec![vec![Event::Text(t.clone())]], json!({})),
            _ => Handled::Now(bad("`text` is required (a string)")),
        },
        "ui.screenshot" => match params.get("path") {
            None | Some(Json::Null) => Handled::Later(screenshot(app, None)),
            Some(Json::String(name)) => match shot_file(app, name) {
                Ok(file) => Handled::Later(screenshot(app, Some(file))),
                Err(refused) => Handled::Now(refused),
            },
            Some(_) => Handled::Now(bad("`path` must be a file name (a string)")),
        },
        "app.quit" => {
            // The window closes in a later frame, so the reply is written before the app exits.
            let now = Instant::now();
            app.quit = Some((app.frames(), now.checked_add(QUIT_DELAY).unwrap_or(now)));
            Handled::Now(ok(json!({})))
        }
        other => Handled::Now(error("unknownMethod", format!("there is no method `{}`", shown(other)), json!({}))),
    }
}

fn execute(app: &mut DbeeApp, params: Json) -> Handled {
    let Json::Object(mut p) = params else { return Handled::Now(bad("params must be a JSON object")) };
    let command = match p.remove("command") {
        Some(Json::String(c)) => c,
        _ => return Handled::Now(bad("`command` is required (a string)")),
    };
    let wait = match p.get("wait") {
        None | Some(Json::Null) => true,
        Some(Json::Bool(w)) => *w,
        Some(_) => return Handled::Now(bad("`wait` must be true or false")),
    };
    match app.run(&command, p.remove("params").unwrap_or(Json::Null)) {
        Ok(Outcome::Done(v)) => Handled::Now(ok(v)),
        Ok(Outcome::Started(job)) if wait => Handled::Later(Wait::Job(job)),
        Ok(Outcome::Started(job)) => Handled::Now(ok(json!({ "job": job }))),
        Err(e) => Handled::Now(engine_error(&e)),
    }
}

/// Every command, engine and UI-only; with `search`, those whose id, label or a menu contains
/// it (ignoring case).
fn commands(app: &DbeeApp, params: &Json) -> Json {
    let search = match params.get("search") {
        None | Some(Json::Null) => None,
        Some(Json::String(s)) => Some(s.to_lowercase()),
        Some(_) => return bad("`search` must be a string"),
    };
    let has = |text: &str, q: &str| text.to_lowercase().contains(q);
    let list: Vec<Json> = app
        .commands()
        .into_iter()
        .filter(|c| search.as_deref().is_none_or(|q| has(c.id, q) || has(c.label, q) || c.menu.iter().any(|m| has(m, q))))
        .map(|c| serde_json::to_value(c).unwrap_or(Json::Null))
        .collect();
    ok(Json::Array(list))
}

/// The session summary (`Session::inspect`). When the app keeps its clipboard in memory
/// ([`crate::app::Clipboard::Memory`]) it adds `clipboard`, the first 4 KiB of the last copied
/// text (cut on a character boundary), and `clipboardBytes`, its whole length; both null before
/// any copy.
fn document(app: &DbeeApp) -> Json {
    let mut d = app.session().inspect();
    if let (Some(kept), Json::Object(fields)) = (app.kept_clipboard(), &mut d) {
        let start = kept.map(|t| Json::String(cut(t, INSPECT_TEXT_BYTES).to_string()));
        fields.insert("clipboard".into(), start.unwrap_or(Json::Null));
        fields.insert("clipboardBytes".into(), kept.map_or(Json::Null, |t| json!(t.len())));
    }
    d
}

/// `{screen, dialog, window: [width, height], frame, editor, theme: {choice, mode}, automation: [{id, rect, …}]}`;
/// `frame` counts the frames drawn so far, `editor` is the open cell editor's `{row, column}` (also
/// while it is not drawn) or null, `automation` is the last drawn frame's tags
/// ([`crate::automation::Tags::to_json`]).
fn ui_inspect(app: &DbeeApp, ctx: &egui::Context) -> Json {
    let r = ctx.content_rect();
    json!({
        "screen": app.session().state().screen,
        "dialog": app.view.dialog.as_ref().map(|d| d.name()),
        "window": [r.width(), r.height()],
        "frame": app.frames(),
        "editor": app.view.work.editor.as_ref().map(|e| e.tag_cell()),
        "theme": { "choice": app.theme_choice.as_str(), "mode": if app.mode() == egui::Theme::Dark { "dark" } else { "light" } },
        "automation": app.tags.to_json(),
    })
}

// ---- synthetic input ---------------------------------------------------------------------------

/// The input `ui.click`, `ui.key` and `ui.text` queue: one batch of events per frame, fed into
/// the frame's raw input before it runs ([`DbeeApp::feed_input`]).
#[derive(Debug, Default)]
pub(crate) struct Input {
    batches: VecDeque<(Vec<Event>, Option<u64>)>,
    /// Tickets whose last batch was fed, with the number of frames drawn before that frame.
    fed: Vec<(u64, u64)>,
    next: u64,
}

impl Input {
    /// Queues `batches` (one per frame); the ticket is fed with the last.
    fn push(&mut self, batches: Vec<Vec<Event>>) -> Option<u64> {
        if self.batches.len().saturating_add(batches.len()) > MAX_QUEUED_BATCHES {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        let ticket = self.next;
        let last = batches.len().saturating_sub(1);
        for (i, events) in batches.into_iter().enumerate() {
            self.batches.push_back((events, (i == last).then_some(ticket)));
        }
        Some(ticket)
    }

    /// Feeds the next batch into the raw input of the frame after `frames` drawn frames.
    pub(crate) fn feed(&mut self, raw: &mut egui::RawInput, frames: u64) {
        if let Some((events, ticket)) = self.batches.pop_front() {
            raw.events.extend(events);
            if let Some(t) = ticket {
                self.fed.push((t, frames));
            }
        }
    }

    /// The frame that received `ticket`'s last batch has run (`frames` drawn by now).
    fn delivered(&mut self, ticket: u64, frames: u64) -> bool {
        match self.fed.iter().position(|&(t, at)| t == ticket && at < frames) {
            Some(i) => {
                self.fed.swap_remove(i);
                true
            }
            None => false,
        }
    }

    pub(crate) fn pending(&self) -> bool {
        !self.batches.is_empty()
    }
}

fn input(app: &mut DbeeApp, batches: Vec<Vec<Event>>, result: Json) -> Handled {
    match app.input.push(batches) {
        Some(ticket) => Handled::Later(Wait::Input { ticket, result }),
        None => Handled::Now(error("busy", "too much input is waiting for frames", json!({}))),
    }
}

/// `{shift?, cmd?, alt?, ctrl?}` flags of `p` as egui modifiers: `cmd` is Command (Cmd on
/// macOS, Ctrl elsewhere), and `ctrl` is Ctrl (also Command off macOS), as the keyboard sends.
fn modifiers(p: Option<&Json>) -> Result<Modifiers, Json> {
    let Some(p) = p.filter(|p| !p.is_null()) else { return Ok(Modifiers::NONE) };
    if !p.is_object() {
        return Err(bad("`modifiers` must be an object: {shift?, cmd?, alt?, ctrl?}"));
    }
    let flag = |k: &str| match p.get(k) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(format!("`{k}` must be true or false"))),
    };
    let (shift, cmd, alt, ctrl) = (flag("shift")?, flag("cmd")?, flag("alt")?, flag("ctrl")?);
    let mac = cfg!(target_os = "macos");
    Ok(Modifiers { alt, ctrl: ctrl || (cmd && !mac), shift, mac_cmd: cmd && mac, command: cmd || (ctrl && !mac) })
}

fn with_modifiers(m: Modifiers, mut batches: Vec<Vec<Event>>) -> Vec<Vec<Event>> {
    if m != Modifiers::NONE {
        if let Some(first) = batches.first_mut() {
            first.insert(0, Event::ModifiersChanged(m));
        }
        batches.push(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    }
    batches
}

fn click(app: &mut DbeeApp, p: &Json) -> Handled {
    let button = match p.get("button") {
        None | Some(Json::Null) => PointerButton::Primary,
        Some(Json::String(b)) if b == "left" || b == "primary" => PointerButton::Primary,
        Some(Json::String(b)) if b == "right" || b == "secondary" => PointerButton::Secondary,
        Some(Json::String(b)) if b == "middle" => PointerButton::Middle,
        Some(_) => return Handled::Now(bad("`button` must be left, right or middle")),
    };
    let count = match p.get("count") {
        None | Some(Json::Null) => 1,
        Some(n) => match n.as_u64() {
            Some(c @ 1..=3) => c,
            _ => return Handled::Now(bad("`count` must be 1, 2 or 3")),
        },
    };
    let m = match modifiers(p.get("modifiers")) {
        Ok(m) => m,
        Err(reply) => return Handled::Now(reply),
    };
    let pos = match p.get("id") {
        Some(Json::String(id)) => match app.tags.rect(id) {
            Some(r) => r.center(),
            // Drawn but scrolled out of view: another widget is where it would be.
            None if app.tags.hidden(id) => {
                app.tags.scroll_to(id);
                return Handled::Later(Wait::InView { id: id.clone(), params: p.clone(), frames: 0 });
            }
            None => {
                return Handled::Now(error(
                    "notFound",
                    format!("no widget `{}` was drawn in the last frame (see ui.inspect)", shown(id)),
                    json!({ "id": shown(id) }),
                ));
            }
        },
        Some(Json::Null) | None => {
            let coord = |k: &str| p.get(k).and_then(Json::as_f64).filter(|v| v.is_finite()).map(|v| v as f32);
            match (coord("x"), coord("y")) {
                (Some(x), Some(y)) => Pos2::new(x, y),
                _ => return Handled::Now(bad("`id`, or `x` and `y` (window points), are required")),
            }
        }
        Some(_) => return Handled::Now(bad("`id` must be a string")),
    };
    let mut presses = Vec::new();
    for _ in 0..count {
        presses.push(Event::PointerButton { pos, button, pressed: true, modifiers: m });
        presses.push(Event::PointerButton { pos, button, pressed: false, modifiers: m });
    }
    // The pointer moves first, so the widget under it is hovered when the button goes down.
    let batches = with_modifiers(m, vec![vec![Event::PointerMoved(pos)], presses]);
    input(app, batches, json!({ "x": pos.x, "y": pos.y }))
}

/// `ui.focus {id}`: gives the keyboard to widget `id`, drawn in view in the last frame, as Tab
/// would, without a click (which egui would count towards a double or triple click).
fn focus(app: &DbeeApp, ctx: &egui::Context, p: &Json) -> Json {
    let id = match p.get("id") {
        Some(Json::String(id)) => id,
        _ => return bad("`id` is required (a string)"),
    };
    match app.tags.focusable(id) {
        Some(widget) => {
            ctx.memory_mut(|m| m.request_focus(widget));
            ctx.request_repaint();
            ok(json!({}))
        }
        None => error(
            "notFound",
            format!("no widget `{}` that can take the keyboard was drawn in view in the last frame (see ui.inspect)", shown(id)),
            json!({ "id": shown(id) }),
        ),
    }
}

/// A key by its egui name (`Enter`, `ArrowDown`, `A`, `F2`), ignoring case, or a common alias
/// (`esc`, `down`, `del`, `return`).
pub(crate) fn key_named(name: &str) -> Option<Key> {
    if let Some(k) = Key::from_name(name) {
        return Some(k);
    }
    let lower = name.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        "del" => "Delete",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "pgup" => "PageUp",
        "pgdn" => "PageDown",
        _ => "",
    };
    if !alias.is_empty() {
        return Key::from_name(alias);
    }
    Key::ALL.iter().copied().find(|k| k.name().eq_ignore_ascii_case(name))
}

fn key(app: &mut DbeeApp, p: &Json) -> Handled {
    let key = match p.get("key") {
        Some(Json::String(k)) => match key_named(k) {
            Some(key) => key,
            None => return Handled::Now(bad(format!("unknown key `{}`", shown(k)))),
        },
        _ => return Handled::Now(bad("`key` is required (a key name such as Enter, A or ArrowDown)")),
    };
    let m = match modifiers(Some(p)) {
        Ok(m) => m,
        Err(reply) => return Handled::Now(reply),
    };
    // As the keyboard does: Ctrl+C and Ctrl+X arrive as copy and cut, not as keys.
    let events = match key {
        Key::C if m.command => vec![Event::Copy],
        Key::X if m.command => vec![Event::Cut],
        _ => vec![
            Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m },
            Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m },
        ],
    };
    input(app, with_modifiers(m, vec![events]), json!({}))
}

// ---- screenshots -------------------------------------------------------------------------------

fn screenshot(app: &mut DbeeApp, file: Option<ShotFile>) -> Wait {
    app.shots = app.shots.wrapping_add(1);
    Wait::Shot { token: app.shots, file, asked: false }
}

/// Where `ui.screenshot {path}` writes: the file `name` in the run's one folder `dir`, which must
/// lie in the `allowed` folders of an isolated run.
#[derive(Debug, Clone)]
pub(crate) struct ShotFile {
    dir: PathBuf,
    name: String,
    allowed: Option<Vec<PathBuf>>,
}

/// `name` is one plain file name on every system: no folder, `..`, drive or stream in it.
fn plain_file_name(name: &str) -> bool {
    !name.contains(['/', '\\', ':', '\0']) && matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)])
}

/// The file `name` asks for; `Err` is the reply that refuses it.
fn shot_file(app: &DbeeApp, name: &str) -> Result<ShotFile, Json> {
    let dir = app.screenshot_dir.as_ref();
    if !plain_file_name(name) {
        let place = dir.map_or_else(|| "the run's screenshot folder".to_string(), |d| d.display().to_string());
        return Err(bad(format!(
            "`path` must be a file name such as shot.png, with no folder in it: screenshots are saved only in {place}. Leave `path` out to get the picture as base64."
        )));
    }
    let Some(dir) = dir else {
        return Err(error(
            "screenshot",
            "this run has no screenshot folder: set DBEE_SCREENSHOT_DIR, or leave `path` out to get the picture as base64",
            json!({}),
        ));
    };
    Ok(ShotFile { dir: dir.clone(), name: name.to_string(), allowed: app.session().allowed_dirs().map(<[PathBuf]>::to_vec) })
}

/// Writes `png` as `file` and returns its path; `Err` says why it did not, and then nothing was
/// written. A plain file of that name is replaced; a link never is, nor followed.
fn write_shot(file: &ShotFile, png: &[u8]) -> Result<PathBuf, String> {
    let dest = file.dir.join(&file.name);
    if let Some(allowed) = &file.allowed {
        // The folder may not be there yet: what counts is where it would be made.
        let there = file.dir.ancestors().find(|a| a.exists()).unwrap_or(&file.dir);
        if allowed_file(allowed, &there.join(&file.name)).is_err() {
            return Err(format!("this isolated run may only write files in its own folders; {} is not in them", dest.display()));
        }
    }
    std::fs::create_dir_all(&file.dir).map_err(|e| format!("{}: {e}", file.dir.display()))?;
    match std::fs::symlink_metadata(&dest) {
        Ok(found) if !found.is_file() => {
            return Err(format!("{} is a link or a folder, so it was not replaced. Choose another name.", dest.display()));
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", dest.display())),
        _ => {}
    }
    replace_private_file(&dest, png).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(dest)
}

/// The window's picture as a PNG: written as `file` (`{path, width, height}`) or returned as
/// base64 (`{base64, width, height}`).
fn save_png(image: &egui::ColorImage, file: Option<&ShotFile>) -> Json {
    let [w, h] = image.size;
    let mut rgba = Vec::with_capacity(w.saturating_mul(h).saturating_mul(4));
    for p in &image.pixels {
        rgba.extend_from_slice(&p.to_srgba_unmultiplied());
    }
    let (Ok(width), Ok(height)) = (u32::try_from(w), u32::try_from(h)) else { return error("screenshot", "the picture is too large", json!({})) };
    let Some(img) = image::RgbaImage::from_raw(width, height, rgba) else {
        return error("screenshot", "the picture has the wrong number of pixels", json!({}));
    };
    let mut png = Vec::new();
    if let Err(e) = img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
        return error("screenshot", format!("PNG encoding failed: {e}"), json!({}));
    }
    match file {
        Some(file) => match write_shot(file, &png) {
            Ok(path) => ok(json!({ "path": path.display().to_string(), "width": w, "height": h })),
            Err(why) => error("screenshot", why, json!({})),
        },
        None => ok(json!({ "base64": base64(&png), "width": w, "height": h })),
    }
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sym = |n: u32| char::from(ABC.get((n & 63) as usize).copied().unwrap_or(b'='));
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if chunk.len() > 1 { sym(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(n) } else { '=' });
    }
    out
}

/// Encodes and writes a picture off the UI thread, then replies (on the UI thread when no
/// thread can be started).
fn deliver_shot(image: Arc<egui::ColorImage>, file: Option<ShotFile>, reply: Sender<Json>) {
    let (on_thread, file_on_thread, reply_on_thread) = (Arc::clone(&image), file.clone(), reply.clone());
    let spawned = std::thread::Builder::new().name("dbee-screenshot".into()).spawn(move || {
        let _ = reply_on_thread.send(save_png(&on_thread, file_on_thread.as_ref()));
    });
    if spawned.is_err() {
        let _ = reply.send(save_png(&image, file.as_ref()));
    }
}

// ---- answering ---------------------------------------------------------------------------------

/// Answers the waiting replies whose wait is over (step 2 of the frame): the jobs in `finished`
/// (this frame's `pump`), idleness, delivered input and arrived pictures; the rest past their
/// deadline get `timeout`. Returns the finished jobs no reply waited for.
pub(crate) fn answer_deferred(
    app: &mut DbeeApp,
    ctx: &egui::Context,
    finished: Vec<(JobId, Result<Json, EngineError>)>,
) -> Vec<(JobId, Result<Json, EngineError>)> {
    let mut finished: Vec<Option<(JobId, Result<Json, EngineError>)>> = finished.into_iter().map(Some).collect();
    let pictures: Vec<(u64, Arc<egui::ColorImage>)> = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Screenshot { user_data, image, .. } => Some((user_data.data.as_ref()?.downcast_ref::<u64>().copied()?, Arc::clone(image))),
                _ => None,
            })
            .collect()
    });
    let now = Instant::now();
    let idle = !app.session().busy();
    let frames = app.frames();
    if let Some((queued, at)) = app.quit
        && frames > queued
        && now >= at
    {
        app.quit = None;
        app.close_window(ctx);
    }
    let mut waiting = Vec::new();
    for mut d in std::mem::take(&mut app.deferred) {
        if let Wait::InView { id, params, frames } = &mut d.wait {
            if app.tags.rect(id).is_some() {
                // In view now: click it as if it had been all along.
                let params = params.clone();
                match click(app, &params) {
                    Handled::Now(reply) => {
                        let _ = d.reply.send(reply);
                    }
                    Handled::Later(wait) => {
                        d.wait = wait;
                        waiting.push(d);
                    }
                }
                continue;
            }
            *frames = frames.saturating_add(1);
            if !app.tags.hidden(id) || *frames > IN_VIEW_FRAMES {
                // Gave up: a later frame must not scroll to it any more.
                app.tags.cancel_scroll(id);
                let shown_id = shown(id);
                let _ = d.reply.send(error(
                    "notFound",
                    format!("the widget `{shown_id}` could not be scrolled into view (see ui.inspect)"),
                    json!({ "id": shown_id }),
                ));
                continue;
            }
        }
        let done = match &mut d.wait {
            Wait::Job(job) => {
                finished.iter_mut().find(|f| f.as_ref().is_some_and(|(j, _)| j == job)).and_then(Option::take).map(|(_, result)| reply_of(result))
            }
            Wait::Idle => idle.then(|| ok(json!({}))),
            Wait::Input { ticket, result } => app.input.delivered(*ticket, frames).then(|| ok(std::mem::take(result))),
            Wait::Shot { token, file, asked } => {
                if !*asked {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                    *asked = true;
                    None
                } else if let Some((_, image)) = pictures.iter().find(|(t, _)| t == token) {
                    deliver_shot(Arc::clone(image), file.take(), d.reply.clone());
                    continue;
                } else {
                    None
                }
            }
            Wait::InView { .. } => None,
        };
        match done {
            Some(reply) => {
                let _ = d.reply.send(reply);
            }
            None if now >= d.deadline => {
                let data = match d.wait {
                    Wait::Job(job) => json!({ "job": job }),
                    _ => json!({}),
                };
                let _ = d.reply.send(error("timeout", "gave up waiting: the request's timeoutMs is up", data));
            }
            None => waiting.push(d),
        }
    }
    app.deferred = waiting;
    finished.into_iter().flatten().collect()
}

/// A waiting reply needs frames to come (input to feed, a picture to take), not only a job.
pub(crate) fn needs_frames(app: &DbeeApp) -> bool {
    app.input.pending()
        || app.quit.is_some()
        || app.deferred.iter().any(|d| matches!(d.wait, Wait::Input { .. } | Wait::Shot { .. } | Wait::InView { .. }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dbee_config::MemoryStore;
    use dbee_engine::{Session, SessionOptions};
    use dbee_testkit::{TempHome, temp_home};
    use serde_json::json;

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

    #[test]
    fn bad_json_and_unknown_method_are_errors() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        let r = handle_line(&mut app, &ctx, "{oops").unwrap();
        assert_eq!((r["ok"].clone(), r["id"].clone(), r["code"].clone()), (json!(false), Json::Null, json!("invalidJson")), "{r}");
        assert!(r["error"].as_str().unwrap().contains("invalid JSON"), "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"id":1,"method":"nope"}"#).unwrap();
        assert_eq!((r["ok"].clone(), r["id"].clone(), r["code"].clone()), (json!(false), json!(1), json!("unknownMethod")), "{r}");
        for bad in ["[]", "7", r#"{"id":"a"}"#, r#"{"id":2,"method":5}"#, r#"{"method":"engine.commands","params":[1]}"#, "\u{0}", ""] {
            let r = handle_line(&mut app, &ctx, bad).unwrap();
            assert_eq!(r["ok"], false, "{bad:?}: {r}");
            assert!(r["code"].is_string() && r["error"].is_string(), "{bad:?}: {r}");
        }
        // Deep nesting is an error, not a stack overflow.
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert_eq!(handle_line(&mut app, &ctx, &deep).unwrap()["code"], "invalidJson");
        let r = handle_line(&mut app, &ctx, r#"{"id":"s","method":"engine.commands","params":{"timeoutMs":"soon"}}"#).unwrap();
        assert_eq!((r["id"].clone(), r["code"].clone()), (json!("s"), json!("badParams")), "{r}");
    }

    #[test]
    fn a_request_past_its_deadline_is_not_run() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        let run = |app: &mut DbeeApp, deadline: Option<Instant>| {
            let (mut req, rx) = ControlRequest::new("engine.execute", json!({ "command": "tab.newQuery" }));
            req.deadline = deadline;
            handle(app, &ctx, req);
            rx.try_recv().unwrap()
        };
        // The server already answered `timeout`: running it now would run it behind the caller's back.
        let late = run(&mut app, Some(Instant::now()));
        assert_eq!((late["ok"].clone(), late["code"].clone()), (json!(false), json!("timeout")), "{late}");
        assert_eq!(app.session().state().tabs.len(), 0, "the late request did not run");
        let in_time = run(&mut app, Instant::now().checked_add(Duration::from_secs(60)));
        assert_eq!(in_time["ok"], true, "{in_time}");
        assert_eq!(run(&mut app, None)["ok"], true);
        assert_eq!(app.session().state().tabs.len(), 2);
    }

    #[test]
    fn execute_returns_command_result() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        let r = handle_line(&mut app, &ctx, r#"{"id":3,"method":"engine.execute","params":{"command":"tab.newQuery"}}"#).unwrap();
        assert_eq!((r["id"].clone(), r["ok"].clone()), (json!(3), json!(true)), "{r}");
        assert!(r["result"]["tab"].is_u64(), "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"id":4,"method":"document.inspect"}"#).unwrap();
        assert_eq!(r["result"]["tabs"].as_array().map(Vec::len), Some(1), "{r}");
    }

    #[test]
    fn engine_error_carries_code_and_data() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        let r =
            handle_line(&mut app, &ctx, r#"{"id":5,"method":"engine.execute","params":{"command":"query.setText","params":{"text":"x"}}}"#).unwrap();
        assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("disabled")), "{r}");
        assert_eq!(r["data"], json!({ "command": "query.setText", "why": "no query tab is active" }), "{r}");
        assert!(r["error"].as_str().unwrap().contains("cannot run now"), "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"method":"engine.execute","params":{"command":"no.such"}}"#).unwrap();
        assert_eq!((r["code"].clone(), r["data"].clone()), (json!("unknownCommand"), json!({ "command": "no.such" })), "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"method":"engine.execute","params":{}}"#).unwrap();
        assert_eq!(r["code"], "badParams", "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"method":"engine.execute","params":{"command":"tab.newQuery","params":[]}}"#).unwrap();
        assert_eq!(r["code"], "badParams", "{r}");
        let r = handle_line(&mut app, &ctx, r#"{"method":"engine.execute","params":{"command":"tab.newQuery","wait":"yes"}}"#).unwrap();
        assert_eq!(r["code"], "badParams", "{r}");
    }

    #[test]
    fn timeouts_are_clamped() {
        assert_eq!(timeout_of(&json!({})).unwrap(), DEFAULT_TIMEOUT);
        assert_eq!(timeout_of(&json!({ "timeoutMs": 5 })).unwrap(), Duration::from_millis(5));
        assert_eq!(timeout_of(&json!({ "timeoutMs": 1e300 })).unwrap(), MAX_TIMEOUT);
        assert_eq!(timeout_of(&json!({ "timeoutMs": u64::MAX })).unwrap(), MAX_TIMEOUT);
        assert!(timeout_of(&json!({ "timeoutMs": -1 })).is_err());
        assert!(timeout_of(&json!({ "timeoutMs": "1" })).is_err());
    }

    #[test]
    fn a_click_that_cannot_scroll_into_view_gives_up_and_forgets_the_scroll() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        // One frame where `far` is drawn scrolled out of view.
        ctx.run_ui(egui::RawInput::default(), |ui| {
            app.tags.begin_frame();
            egui::ScrollArea::vertical().max_height(40.0).show(ui, |ui| {
                for i in 0..30 {
                    let r = ui.button(format!("button {i}"));
                    app.tags.tag(&format!("b{i}"), &r);
                }
                let r = ui.button("far");
                app.tags.tag("far", &r);
            });
        })
        .drop_without_applying_deltas();
        assert!(app.tags.hidden("far"));
        let (req, rx) = ControlRequest::new("ui.click", json!({ "id": "far" }));
        handle(&mut app, &ctx, req);
        assert_eq!(app.tags.scroll_pending(), Some("far"));
        // No frame draws it again, so it never comes into view: the click gives up.
        for _ in 0..=IN_VIEW_FRAMES {
            answer_deferred(&mut app, &ctx, Vec::new());
        }
        let r = rx.try_recv().unwrap();
        assert_eq!(r["code"], "notFound", "{r}");
        assert_eq!(app.tags.scroll_pending(), None, "a later frame must not scroll to it");
    }

    #[test]
    fn keys_parse_like_the_keyboard_sends_them() {
        assert_eq!(key_named("Enter"), Some(egui::Key::Enter));
        assert_eq!(key_named("enter"), Some(egui::Key::Enter));
        assert_eq!(key_named("esc"), Some(egui::Key::Escape));
        assert_eq!(key_named("a"), Some(egui::Key::A));
        assert_eq!(key_named("Down"), Some(egui::Key::ArrowDown));
        assert_eq!(key_named("F2"), Some(egui::Key::F2));
        assert_eq!(key_named(""), None);
        assert_eq!(key_named("🐙"), None);
    }

    /// A folder of its own under `target/`, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let path = dbee_testkit::repo_root().join("target").join("testkit").join("shots").join(dbee_testkit::unique(name));
            std::fs::create_dir_all(&path).unwrap();
            Folder(path)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn picture() -> egui::ColorImage {
        egui::ColorImage::new([2, 1], vec![egui::Color32::RED; 2])
    }

    fn shot(dir: &Path, name: &str, allowed: Option<&[PathBuf]>) -> Json {
        save_png(&picture(), Some(&ShotFile { dir: dir.to_path_buf(), name: name.into(), allowed: allowed.map(<[PathBuf]>::to_vec) }))
    }

    fn is_png(path: &Path) -> bool {
        std::fs::read(path).unwrap().starts_with(b"\x89PNG")
    }

    #[test]
    fn a_screenshot_path_is_a_file_name_and_nothing_else() {
        let (app, _home) = app();
        let folder = Folder::new("names");
        let mut app = app.with_screenshot_dir(Some(folder.0.clone()));
        let ctx = egui::Context::default();
        let outside = std::env::temp_dir().join(dbee_testkit::unique("dbee-shot.png"));
        let absolute = outside.display().to_string();
        let above = format!("../{}", dbee_testkit::unique("above.png"));
        for path in [
            absolute.as_str(),
            above.as_str(),
            "..",
            ".",
            "",
            "a/b.png",
            "a/../b.png",
            "./x.png",
            "x.png/",
            "..\\x.png",
            "a\\b.png",
            "C:\\x.png",
            "C:x.png",
            "x.png:stream",
            "x\u{0}.png",
        ] {
            let line = json!({ "id": 1, "method": "ui.screenshot", "params": { "path": path } }).to_string();
            let r = handle_line(&mut app, &ctx, &line).unwrap_or_else(|| panic!("{path:?} waits for a picture to write, instead of being refused"));
            assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("badParams")), "{path:?}: {r}");
        }
        assert!(!outside.exists());
        assert_eq!(folder.names(), Vec::<String>::new());
        for fine in [r#"{"path":"shot.png"}"#, r#"{"path":".hidden name.png"}"#, r#"{"path":null}"#, "{}"] {
            let line = format!(r#"{{"id":1,"method":"ui.screenshot","params":{fine}}}"#);
            assert_eq!(handle_line(&mut app, &ctx, &line), None, "{fine} waits for the picture");
        }
    }

    #[test]
    fn without_a_folder_only_the_picture_comes_back() {
        let (mut app, _home) = app();
        let ctx = egui::Context::default();
        let r = handle_line(&mut app, &ctx, r#"{"id":1,"method":"ui.screenshot","params":{"path":"shot.png"}}"#).unwrap();
        assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("screenshot")), "{r}");
        assert_eq!(handle_line(&mut app, &ctx, r#"{"id":1,"method":"ui.screenshot"}"#), None);
        assert!(save_png(&picture(), None)["result"]["base64"].is_string());
    }

    #[test]
    fn a_screenshot_lands_in_its_folder_and_replaces_only_a_plain_file() {
        let folder = Folder::new("written");
        let dir = folder.0.join("made").join("for it");
        let r = shot(&dir, "shot.png", None);
        assert_eq!(r["ok"], true, "{r}");
        let dest = dir.join("shot.png");
        assert_eq!(r["result"]["path"], json!(dest), "the reply says where it went");
        assert_eq!((r["result"]["width"].clone(), r["result"]["height"].clone()), (json!(2), json!(1)));
        assert!(is_png(&dest));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o600, "a picture can show a password or data");
        }
        std::fs::write(&dest, "an older screenshot").unwrap();
        assert_eq!(shot(&dir, "shot.png", None)["ok"], true);
        assert!(is_png(&dest), "the same name again replaces it");
        std::fs::create_dir(dir.join("a folder.png")).unwrap();
        let r = shot(&dir, "a folder.png", None);
        assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("screenshot")), "{r}");
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "no temporary file stays behind: {names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_is_refused_and_what_it_names_is_untouched() {
        use std::os::unix::fs::symlink;
        let (folder, outside) = (Folder::new("links"), Folder::new("victim"));
        let victim = outside.0.join("bashrc");
        std::fs::write(&victim, "keep me").unwrap();
        let missing = outside.0.join("made-by-a-link");
        symlink(&victim, folder.0.join("to-file.png")).unwrap();
        symlink(&missing, folder.0.join("dangling.png")).unwrap();
        symlink(&outside.0, folder.0.join("to-folder.png")).unwrap();
        for name in ["to-file.png", "dangling.png", "to-folder.png"] {
            let r = shot(&folder.0, name, None);
            assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("screenshot")), "{name}: {r}");
            assert!(r["error"].as_str().unwrap().contains(name), "{r}");
            assert!(std::fs::symlink_metadata(folder.0.join(name)).unwrap().is_symlink(), "{name} stays what it was");
        }
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(!missing.exists());
        assert_eq!(outside.names(), vec!["bashrc".to_string()]);
    }

    #[test]
    fn outside_an_isolated_runs_folders_a_screenshot_is_refused_and_nothing_is_made() {
        let (allowed, outside) = (Folder::new("allowed"), Folder::new("outside"));
        let dirs = vec![allowed.0.clone()];
        let refused = |dir: &Path, list: &[PathBuf]| {
            let r = shot(dir, "shot.png", Some(list));
            assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("screenshot")), "{}: {r}", dir.display());
            assert!(r["error"].as_str().unwrap().contains("isolated run"), "{r}");
        };
        refused(&outside.0, &dirs);
        refused(&outside.0.join("new"), &dirs);
        refused(&allowed.0.join("..").join(outside.0.file_name().unwrap()), &dirs);
        refused(&allowed.0, &[]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside.0, allowed.0.join("out")).unwrap();
            refused(&allowed.0.join("out"), &dirs);
            refused(&allowed.0.join("out").join("new"), &dirs);
        }
        assert_eq!(outside.names(), Vec::<String>::new(), "nothing was written or made");
        // Inside, the folder is made as in any run.
        let inside = allowed.0.join("look").join("deep");
        assert_eq!(shot(&inside, "shot.png", Some(&dirs))["ok"], true);
        assert!(is_png(&inside.join("shot.png")));
    }

    #[test]
    fn png_base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }
}

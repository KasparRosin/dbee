//! The workspace, shown once a connection is open ([`screen`]):
//!
//! - the menu bar at the top ([`crate::menu`]);
//! - the status bar at the bottom ([`status`]): the connection's name on its colour, its address
//!   as typed in its form ([`address`]: never the password), the server
//!   version, the link's security while it is weaker than fully checked, READ-ONLY when the
//!   session is, SAVE CONNECTION while the saved list doesn't hold the connection (one opened
//!   from a file, say), and at the right one activity ([`activity`]): a reconnect, the lost
//!   connection with RECONNECT, the active tab's running job with its timer and CANCEL, or IN
//!   TRANSACTION;
//! - the schema tree on the left ([`crate::tree`]);
//! - the tabs strip, then the active tab, both from the tree's line to the window's right edge;
//!   the value panel stands inside the tab, beside its results, with its toggle in the tab's
//!   result bar or table bar ([`crate::value_panel`]).
//!
//! Tab reaches them in reading order: the menu bar, the tree, the tabs strip, the tab (its value
//! panel last), then the status bar.
//!
//! The quick search (Ctrl+P) is drawn over it by the window ([`crate::quick_search`]).
//!
//! **Tabs strip** ([`crate::components::tab`]). The tabs fill a band side by side, the active one
//! open into the content below it.
//! A tab shows its kind's icon, its title (a table tab its table's name, with the number of
//! applied filters as `orders [2]`), a dot when closing it would lose something (query text,
//! staged edits), a spinner while one of its jobs runs, and × to close it; a middle click closes
//! it too, and `+` opens a query tab. More tabs than fit scroll sideways, with `+` and the list
//! button (⌄) at the strip's right end; the list shows every tab, and choosing one makes it the
//! active tab. The active tab is always brought into view. Closing goes through `tab.close`, so a
//! tab that would lose something asks first.
//!
//! **Running jobs.** The status bar and the tabs show only live jobs: those a tab's markers name
//! (a page load, a count, an Apply, a query run) and a reconnect. A tab that closed, or whose
//! markers a reconnect cleared, shows none, even while the engine lets its job run to its end.
//! Each shows only once it has run 300 ms ([`Quiet`]): the status bar's job then with its timer,
//! RECONNECTING at once (the click's answer) and its timer from 300 ms. A page load is the table
//! bar's (LOADING, its CANCEL), not the status bar's.
//!
//! **Tab bodies.** A table tab shows its page ([`crate::table_tab`]), a query tab its editor and
//! results ([`crate::query_tab`]), across the whole width under the strip: the grid runs edge to
//! edge, and the bars round it keep their own inset. With no tab open, the work area says so and
//! offers New query (Ctrl+T) and Quick search (Ctrl+P), each running its command as its keys do.
//!
//! Automation ids: `tab-<id>` (value: the title as shown), `tab-close-<id>`, `tab-dirty-<id>`,
//! `tab-new`, `tab-list` (while the tabs don't fit; value: the number of tabs) and, while its list
//! is open, `tab-list-<n>` (`n` from 1 in strip order; value `{tab, title, active}`); with no tab
//! open, `start-tab.newQuery` and `start-ui.quickSearch` (value: the words);
//! `status-connection` (value: the name), `status-address` (value: the address, whole),
//! `status-server`, `status-tls` (value: what
//! protects the link, while it is weaker than fully checked), `status-read-only`, `status-save`,
//! `status-job` (value: what runs, or `Reconnecting…`), `status-timer` (value: its words,
//! `00:01.2`), `status-cancel`, `status-lost`, `status-reconnect`, `status-transaction`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use dbee_config::ConnectionConfig;
use dbee_engine::{JobId, Session, State, Tab, TabId, TlsState, TransactionState, Which};
use dbee_sql::filter::Condition;
use egui::{Align, Frame, Layout, Rect, Sense, UiBuilder, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::json;

use crate::app::Origin;
use crate::components::keycap::Keycap;
use crate::components::status::{self, Activity, Address};
use crate::components::tab::{self, Asked};
use crate::components::tooltip::Hint;
use crate::components::{self, Look, focus};
use crate::editors::CellEditor;
use crate::query_tab::{self, Queries};
use crate::quiet::{Quiet, Watch};
use crate::table_tab::{self, Grids};
use crate::theme::size::{KEYCAP_GAP, KEYCAP_H, NO_TAB_GAP, NO_TAB_ROW_GAP, NO_TAB_W, SIDEBAR_MAX_W, SIDEBAR_MIN_W, SIDEBAR_W, SLIM_BAR_H, TAB_H};
use crate::theme::{self, Text, Words};
use crate::tree::{self, MenuState, Refresh, Tree, TreeAct};
use crate::{DbeeApp, menu, structure_view};

/// The status bar's height.
pub(crate) const STATUS_H: f32 = SLIM_BAR_H;

/// The reconnect's command: the status bar shows it while it runs.
const RECONNECT: &str = "connection.reconnect";
/// The tree's refresh: the window times it to show `LOADING` ([`crate::quiet`]).
const REFRESH: &str = "schema.refresh";

/// What only the workspace keeps.
#[derive(Debug, Default)]
pub(crate) struct Workspace {
    pub tree: Tree,
    /// Each table tab's grid (its column widths), dropped with the tab.
    pub grids: Grids,
    /// Each table tab's structure page as worked out from its last read, dropped with the tab.
    pub structures: HashMap<TabId, structure_view::Kept>,
    /// The cell editor, while one is open ([`crate::editors`]).
    pub editor: Option<CellEditor>,
    /// Each query tab's editor text and result grids, dropped with the tab.
    pub queries: Queries,
    /// The query tab whose editor was given the keyboard when it became the active one; `None`
    /// while no query tab is active.
    pub focused_query: Option<TabId>,
    /// The connection this state belongs to; another one starts afresh.
    connection: Option<String>,
}

impl Workspace {
    /// The workspace is not shown (back on the connection screen): its state goes, so the next
    /// connection starts afresh.
    pub(crate) fn leave(&mut self) {
        if self.connection.is_some() || self.editor.is_some() {
            *self = Workspace::default();
        }
    }
}

/// A tab as the strip draws it.
struct TabView {
    id: TabId,
    query: bool,
    /// What the tab shows: `orders [2]`.
    title: String,
    hover: String,
    active: bool,
    /// Closing it would lose query text or staged edits.
    dirty: bool,
    busy: bool,
}

/// The number of applied filters, as the tab title's marker counts them: each filter row of an
/// applied condition that is part of it (a row with an empty value is left out:
/// `Filter::restricts`), or 1 for an applied raw condition.
pub fn applied_filters(applied: &Condition) -> usize {
    match applied {
        Condition::None => 0,
        Condition::Filters(f) => f.iter().filter(|f| f.restricts()).count(),
        Condition::Raw(r) if r.trim().is_empty() => 0,
        Condition::Raw(_) => 1,
    }
}

/// The title a tab shows: a table tab its table's name, with `[n]` when `n` filters are applied.
pub fn tab_title(tab: &Tab) -> String {
    match tab {
        Tab::Table(t) => match applied_filters(&t.applied) {
            0 => t.table.name.clone(),
            n => format!("{} [{n}]", t.table.name),
        },
        Tab::Query(q) => q.title.clone(),
    }
}

/// Closing `tab` would lose something.
pub(crate) fn dirty(tab: &Tab) -> bool {
    match tab {
        Tab::Table(t) => t.has_staged_edits(),
        Tab::Query(q) => !q.text.trim().is_empty(),
    }
}

/// `tab`'s live jobs (by its markers).
pub(crate) fn jobs(tab: &Tab) -> Vec<JobId> {
    match tab {
        Tab::Table(t) => [t.loading, t.counting, t.applying, t.structure.loading].into_iter().flatten().collect(),
        Tab::Query(q) => q.running.into_iter().collect(),
    }
}

/// One of `tab`'s jobs has run 300 ms as the window saw it ([`Quiet`]): the tab's running mark
/// waits as long, so a fast job never flickers it.
fn busy(quiet: &mut Quiet, ctx: &egui::Context, tab: &Tab) -> bool {
    let id = tab.id();
    // Every job's start is noted, not only the first's.
    let past: Vec<bool> = jobs(tab).into_iter().map(|job| quiet.past(ctx, Watch::Tab(id, job)).is_some()).collect();
    past.contains(&true)
}

/// A live job the status bar shows: what runs, the command that cancels it, and its marker.
#[derive(Debug, PartialEq, Eq)]
pub struct LiveJob {
    pub text: Words,
    pub cancel: Option<&'static str>,
    pub job: JobId,
}

/// The active tab's live job, by its markers: an Apply, else a count; a query tab's run. A page
/// load is none: the table bar shows it.
pub fn live_job(session: &Session) -> Option<LiveJob> {
    let job = |doing: &str, name: &str, cancel, job| LiveJob { text: Words::from(doing).name(name).plain("…"), cancel: Some(cancel), job };
    match session.state().active_tab()? {
        Tab::Table(t) => {
            let name = &t.table.name;
            match (t.applying, t.counting) {
                (Some(j), _) => Some(job("Applying the changes to ", name, "table.cancel", j)),
                (None, Some(j)) => Some(job("Counting the rows of ", name, "table.cancel", j)),
                (None, None) => None,
            }
        }
        Tab::Query(q) => q.running.map(|j| job("Running ", &q.title, "query.cancel", j)),
    }
}

/// The connection's address for the status bar, as typed in its form:
/// `user@host:port/database`, empty parts left out (an IPv6 host in brackets before its port),
/// then through a tunnel ` via ` and the SSH host; a file's path with `home` as `~`. A config
/// never holds a password. `None` when the name block already is the address: a name
/// dbee made up, or one that reads the same.
pub(crate) fn address(config: &ConnectionConfig, home: Option<&Path>) -> Option<Address> {
    if config.name.is_empty() {
        return None;
    }
    let address = if config.kind.uses_file() {
        Address::File(crate::connections::tilde_at(&config.file.as_ref()?.display().to_string(), home))
    } else {
        let user = if config.user.is_empty() { String::new() } else { format!("{}@", config.user) };
        let host = match config.port {
            0 => config.host.clone(),
            port if config.host.contains(':') => format!("[{}]:{port}", config.host),
            port => format!("{}:{port}", config.host),
        };
        let via = config.ssh.as_ref().map(|ssh| format!(" via {}", ssh.host)).unwrap_or_default();
        Address::Server(format!("{user}{host}{}{via}", config.database_suffix()))
    };
    (!address.text().is_empty() && address.text() != config.name).then_some(address)
}

/// What the status bar shows at its right, one at a time, the first that holds: a reconnect
/// running (at once, its timer once it has gone 300 ms), the connection lost, the active tab's
/// live job once it has gone 300 ms (with its timer and the command that cancels it), a
/// transaction open or failed on the connection's query session. The job comes before the
/// transaction, which lasts, so its CANCEL is never hidden.
fn activity(session: &Session, quiet: &mut Quiet, ctx: &egui::Context) -> Option<(Activity, Option<&'static str>)> {
    let c = session.state().connection(Which::Active)?;
    if let Some(job) = session.running_job(RECONNECT) {
        return Some((Activity::Reconnecting { elapsed: quiet.past(ctx, Watch::Command(RECONNECT, job)) }, None));
    }
    if c.lost {
        return Some((Activity::Disconnected, None));
    }
    let tab = session.state().active_tab().map(Tab::id);
    let shown = live_job(session).zip(tab).and_then(|(job, tab)| quiet.past(ctx, Watch::Tab(tab, job.job)).map(|e| (job, e)));
    if let Some((job, elapsed)) = shown {
        return Some((Activity::Job { text: job.text, elapsed, cancel: job.cancel.is_some() }, job.cancel));
    }
    c.transaction().filter(|t| matches!(t.state, TransactionState::Open | TransactionState::Failed)).map(|_| (Activity::InTransaction, None))
}

/// What the user did outside the tree and the menus this frame.
enum Act {
    Activate(TabId),
    Close(TabId),
    NewQuery,
    /// A way to start, chosen with no tab open: the command, and its shortcut.
    Start(&'static str, Option<&'static str>),
    Cancel(&'static str),
    Reconnect,
    SaveOpen,
}

/// Draws the workspace and acts on it.
pub(crate) fn screen(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    // A new connection starts with a fresh tree.
    let id = app.session.state().connection(Which::Active).map(|c| c.config.id.clone());
    if app.view.work.connection != id {
        app.view.work = Workspace { connection: id, ..Workspace::default() };
    }
    let tabs = &app.session.state().tabs;
    app.view.work.grids.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    app.view.work.structures.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    app.view.work.queries.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    egui::Panel::top("dbee-menubar").exact_size(SLIM_BAR_H).show_separator_line(false).frame(Frame::NONE).show(ui, |ui| menu::bar(app, ui));
    let mut acts = Vec::new();
    // The status bar's room now and its contents last, so Tab reaches it after the tab's controls.
    let footer = app.session.state().connection(Which::Active).is_some().then(|| {
        egui::Panel::bottom("dbee-status").exact_size(STATUS_H).show_separator_line(false).frame(Frame::NONE).show(ui, |ui| ui.max_rect()).inner
    });
    side(app, ui);
    let panel = egui::CentralPanel::default()
        .frame(Frame::new().fill(t.bg))
        .show(ui, |ui| {
            let top = strip(app, ui, &mut acts);
            body(app, ui, top, &mut acts)
        })
        .inner;
    // A panel not drawn keeps no selection and no text.
    if !panel {
        app.view.panel.clear();
    }
    if let Some(rect) = footer {
        status_bar(app, ui, rect, &mut acts);
    }
    for act in acts {
        match act {
            Act::Activate(tab) => {
                app.start("tab.activate", json!({ "tab": tab }), Origin::Menu);
            }
            Act::Close(tab) => {
                app.start("tab.close", json!({ "tab": tab }), Origin::Menu);
            }
            Act::NewQuery => {
                app.start("tab.newQuery", json!({}), Origin::Menu);
            }
            Act::Start(command, shortcut) => {
                app.start(command, menu::params_for(shortcut), Origin::Menu);
            }
            Act::Cancel(command) => {
                app.start(command, json!({}), Origin::Menu);
            }
            Act::Reconnect => {
                app.start(RECONNECT, json!({}), Origin::Menu);
            }
            Act::SaveOpen => {
                app.start("connection.saveOpen", json!({}), Origin::Menu);
            }
        }
    }
}

/// The status bar ([`status::bar`]) in `rect`.
fn status_bar(app: &mut DbeeApp, ui: &mut egui::Ui, rect: Rect, acts: &mut Vec<Act>) {
    let Some(c) = app.session.state().connection(Which::Active) else { return };
    let name = c.config.display_name();
    let own = !c.config.name.is_empty();
    let colour = c.config.color;
    let address = address(&c.config, dbee_config::home_dir().as_deref());
    let read_only = c.info.read_only;
    let server = dbee_engine::state::server_text(&c.info);
    // The link is named only while it is weaker than fully checked (#9).
    let link = dbee_engine::state::tls_text(&c.info, c.config.ssh.is_some()).filter(|_| c.info.tls != Some(TlsState::Checked));
    let unsaved = !app.session.state().store.is_saved(&c.config.id);
    let activity = activity(&app.session, &mut app.view.quiet, ui.ctx());
    let tags = &mut app.tags;
    let mut bar = ui.new_child(UiBuilder::new().id_salt("dbee-status").max_rect(rect).layout(Layout::top_down(Align::Min)));
    bar.set_clip_rect(rect);
    let connection = status::Connection { name: &name, colour, own };
    let facts = status::Facts { address: address.as_ref(), server: Some(&server), link, read_only, unsaved };
    let shown = status::bar(&mut bar, &connection, &facts, activity.as_ref().map(|(a, _)| a), |_| {});
    tags.tag_value("status-connection", &shown.connection, name.as_str());
    if let (Some(r), Some(a)) = (&shown.address, &address) {
        tags.tag_value("status-address", r, a.text());
    }
    if let Some(r) = &shown.server {
        tags.tag_value("status-server", r, server.as_str());
    }
    if let (Some(r), Some((words, _))) = (&shown.link, link) {
        tags.tag_value("status-tls", r, words);
    }
    if let Some(r) = &shown.read_only {
        tags.tag_value("status-read-only", r, "Read-only");
    }
    if let Some(r) = shown.save {
        let r = r.hint("Add this connection to the saved ones");
        tags.tag_value("status-save", &r, "Save connection");
    }
    if let (Some((Activity::Job { elapsed, .. } | Activity::Reconnecting { elapsed: Some(elapsed) }, _)), Some(r)) = (&activity, &shown.timer) {
        tags.tag_value("status-timer", r, status::timer(*elapsed));
    }
    let words = shown.activity.as_ref();
    match (activity.as_ref(), words) {
        (Some((Activity::Disconnected, _)), Some(r)) => tags.tag_value("status-lost", r, "Connection lost"),
        (Some((Activity::Reconnecting { .. }, _)), Some(r)) => tags.tag_value("status-job", r, "Reconnecting…"),
        (Some((Activity::Job { text, .. }, _)), Some(r)) => tags.tag_value("status-job", r, text.to_string()),
        (Some((Activity::InTransaction, _)), Some(r)) => tags.tag_value("status-transaction", r, "In transaction"),
        _ => {}
    }
    match (activity.as_ref(), &shown.button) {
        (Some((Activity::Disconnected, _)), Some(r)) => tags.tag_value("status-reconnect", r, "Reconnect"),
        (Some((Activity::Job { .. }, Some(_))), Some(r)) => tags.tag_value("status-cancel", r, "Cancel"),
        _ => {}
    }
    match shown.act {
        Some(status::Act::SaveConnection) => acts.push(Act::SaveOpen),
        Some(status::Act::Reconnect) => acts.push(Act::Reconnect),
        Some(status::Act::Cancel) => {
            if let Some((_, Some(command))) = &activity {
                acts.push(Act::Cancel(command));
            }
        }
        None => {}
    }
}

/// The tree on the left.
fn side(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let Some(catalog) = app.session.state().connection(Which::Active).map(|c| Arc::clone(&c.catalog)) else { return };
    let dialect = app.session.state().dialect(Which::Active);
    let refresh = if app.session.running(REFRESH) {
        Refresh::Running
    } else {
        match app.session.state().connection(Which::Active).map(|c| c.lost) {
            Some(true) => Refresh::Disabled("the connection is lost; reconnect first".into()),
            _ => Refresh::Ready,
        }
    };
    let loading = app
        .session
        .running_job(REFRESH)
        .filter(|_| refresh == Refresh::Running)
        .and_then(|job| app.view.quiet.past(ui.ctx(), Watch::Command(REFRESH, job)));
    app.view.work.tree.refresh = refresh;
    // The menu's reasons, read only in frames a menu may show (open, or just opened).
    let menu = if app.view.work.tree.menu_open() || ui.input(|i| i.pointer.secondary_released()) {
        let commands = app.commands();
        let why = |id: &str| commands.iter().find(|c| c.id == id).and_then(|c| (!c.enabled).then(|| c.disabled_reason.clone().unwrap_or_default()));
        MenuState { open: why("table.open"), new_query: why("tab.newQuery"), copy_name: why("clipboard.copyName") }
    } else {
        MenuState::default()
    };
    app.view.work.tree.menu = menu;
    let mut acts = Vec::new();
    {
        let DbeeApp { view, tags, .. } = app;
        let frame = Frame::new().fill(t.bg);
        let panel = egui::Panel::left("dbee-tree").resizable(true).default_size(SIDEBAR_W).size_range(SIDEBAR_MIN_W..=SIDEBAR_MAX_W);
        panel.frame(frame).show(ui, |ui| {
            acts = tree::show(ui, &catalog, dialect, &mut view.work.tree, loading, tags);
        });
    }
    for act in acts {
        match act {
            TreeAct::Open(table) => {
                app.start("table.open", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
            }
            TreeAct::NewQuery(table) => {
                app.start("tab.newQuery", json!({ "schema": table.schema }), Origin::Menu);
            }
            TreeAct::CopyName(table) => {
                app.start("clipboard.copyName", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
            }
            TreeAct::Refresh => {
                app.start(REFRESH, json!({}), Origin::Menu);
            }
        }
    }
}

/// The tabs strip ([`tab::Strip`]); where it ends below.
fn strip(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) -> f32 {
    let st: &State = app.session.state();
    let ctx = ui.ctx().clone();
    let quiet = &mut app.view.quiet;
    let tabs: Vec<TabView> = st
        .tabs
        .iter()
        .map(|tab| TabView {
            id: tab.id(),
            query: matches!(tab, Tab::Query(_)),
            title: tab_title(tab),
            hover: tab.title(),
            active: st.active == Some(tab.id()),
            dirty: dirty(tab),
            busy: busy(quiet, &ctx, tab),
        })
        .collect();
    let tags = &mut app.tags;
    let strip = tabs.iter().map(|view| {
        let one = if view.query { tab::Tab::query(&view.title) } else { tab::Tab::table(&view.title) };
        one.id_salt(view.id.0).active(view.active).unsaved(view.dirty).busy(view.busy)
    });
    let shown = tab::Strip::new(strip.collect()).show(ui, |i, shown| {
        let Some(view) = tabs.get(i) else { return };
        let r = shown.tab.clone().hint(view.hover.as_str());
        tags.tag_value(&format!("tab-{}", view.id.0), &r, view.title.as_str());
        if let Some(dot) = &shown.unsaved {
            tags.tag(&format!("tab-dirty-{}", view.id.0), dot);
        }
        let close = shown.close.clone().hint("Close (Ctrl+W)");
        tags.tag(&format!("tab-close-{}", view.id.0), &close);
    });
    match shown.asked.and_then(|(i, asked)| tabs.get(i).map(|view| (view.id, asked))) {
        Some((id, Asked::Close)) => acts.push(Act::Close(id)),
        Some((id, Asked::Activate)) => acts.push(Act::Activate(id)),
        None => {}
    }
    let bottom = shown.new.rect.bottom();
    let r = shown.new.hint("New query tab (Ctrl+T)");
    tags.tag("tab-new", &r);
    if r.clicked() {
        acts.push(Act::NewQuery);
    }
    if let Some(list) = shown.list {
        let r = list.hint("All tabs");
        tags.tag_value("tab-list", &r, tabs.len());
    }
    for (i, item) in &shown.listed {
        if let Some(view) = tabs.get(*i) {
            // A menu's first frame is a sizing pass, which reads every widget as disabled.
            let value = json!({ "tab": view.id.0, "title": view.title, "active": view.active });
            tags.tag_value_as(&format!("tab-list-{}", i.saturating_add(1)), item, value, true);
        }
    }
    bottom
}

/// The active tab's body (see the module docs) from `top`, the strip's line, or the ways to start
/// when no tab is open. Whether the tab drew the value panel.
fn body(app: &mut DbeeApp, ui: &mut egui::Ui, top: f32, acts: &mut Vec<Act>) -> bool {
    let active = app.session.state().active_tab().map(|tab| (tab.id(), matches!(tab, Tab::Query(_))));
    if active.is_none_or(|(_, query)| !query) {
        // The next query tab to become the active one takes the keyboard.
        app.view.work.focused_query = None;
        app.view.query_cursor = None;
    }
    let Some((id, query)) = active else {
        no_tab(app, ui, acts);
        return false;
    };
    // From the strip's line, with no spacing after the strip.
    let room = ui.available_rect_before_wrap();
    let content = Rect::from_min_max(egui::pos2(room.left(), top.min(room.top())), room.max);
    let mut ui = ui.new_child(egui::UiBuilder::new().id_salt("dbee-tab-body").max_rect(content).layout(Layout::top_down(Align::Min)));
    if query { query_tab::show(app, &mut ui, id) } else { table_tab::show(app, &mut ui, id) }
}

/// The ways to start with no tab open, in order: the words, and the command.
const STARTS: [(&str, &str); 2] = [("New query", "tab.newQuery"), ("Quick search", "ui.quickSearch")];

/// The work area with no tab open: "No tab open", and under it a row for each of
/// [`STARTS`] with its shortcut's keys, the block centred in the area.
fn no_tab(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = theme::tokens(ui.ctx());
    let commands = app.commands();
    // Under the strip, without the space egui leaves after it.
    let area = Rect::from_min_max(pos2(ui.max_rect().left(), ui.max_rect().top() + TAB_H), ui.max_rect().max);
    let rows = STARTS.len() as f32;
    let height = Text::Body.line() + NO_TAB_GAP + rows * KEYCAP_H + (rows - 1.0) * NO_TAB_ROW_GAP;
    let block = Rect::from_center_size(area.center(), vec2(NO_TAB_W, height));
    let mut ui = ui.new_child(UiBuilder::new().id_salt("dbee-no-tab").max_rect(block).layout(Layout::top_down(Align::Center)));
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.label(Text::Body.rich("No tab open").color(t.text_muted));
    ui.add_space(NO_TAB_GAP);
    for (i, (words, id)) in STARTS.into_iter().enumerate() {
        if i > 0 {
            ui.add_space(NO_TAB_ROW_GAP);
        }
        let Some(c) = commands.iter().find(|c| c.id == id) else { continue };
        let r = ui.add_enabled(c.enabled, Start { words, keys: c.shortcut });
        app.tags.tag_value(&format!("start-{id}"), &r, words);
        if r.clicked() {
            acts.push(Act::Start(c.id, c.shortcut));
        }
    }
}

/// A row of the work area with no tab open: its words in `body` `text_muted` (`text` while
/// hovered) on the left, its shortcut's keycaps on the right.
struct Start<'a> {
    words: &'a str,
    keys: Option<&'a str>,
}

impl egui::Widget for Start<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), KEYCAP_H), Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.words));
        if ui.is_rect_visible(rect) {
            let t = theme::tokens(ui.ctx());
            let look = Look::of(ui, &r);
            let p = components::painter(ui);
            let colour = if look.disabled {
                t.text_faint
            } else if look.hovered {
                t.text
            } else {
                t.text_muted
            };
            let words = p.layout_job(Text::Body.job_as_written(self.words, colour));
            p.galley(pos2(rect.left(), rect.center().y - words.size().y / 2.0), words, colour);
            let caps: Vec<Keycap<'_>> = self.keys.map(|k| k.split('+').map(Keycap::new).collect()).unwrap_or_default();
            let widths: Vec<f32> = caps.iter().map(|cap| cap.width(&p)).collect();
            let mut left = rect.right() - widths.iter().sum::<f32>() - KEYCAP_GAP * (caps.len().saturating_sub(1)) as f32;
            for (cap, w) in caps.iter().zip(&widths) {
                cap.paint(&p, pos2(left, rect.center().y), look.disabled);
                left += w + KEYCAP_GAP;
            }
            if look.focused {
                focus::box_around(&p, rect);
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dbee_config::{DbKind, SshConfig};
    use dbee_sql::filter::{Filter, Op};

    use super::*;

    #[test]
    fn the_address_is_the_connection_as_typed() {
        let server = |s: &str| Some(Address::Server(s.into()));
        let pg = ConnectionConfig {
            name: "Shop live".into(),
            kind: DbKind::Postgres,
            host: "pg14".into(),
            port: 5432,
            user: "postgres".into(),
            database: "dbee".into(),
            ..ConnectionConfig::default()
        };
        assert_eq!(address(&pg, None), server("postgres@pg14:5432/dbee"));
        assert_eq!(address(&ConnectionConfig { user: String::new(), ..pg.clone() }, None), server("pg14:5432/dbee"), "no user");
        assert_eq!(address(&ConnectionConfig { database: String::new(), ..pg.clone() }, None), server("postgres@pg14:5432"), "no database");
        assert_eq!(address(&ConnectionConfig { port: 0, ..pg.clone() }, None), server("postgres@pg14/dbee"), "no port");
        assert_eq!(address(&ConnectionConfig { host: "::1".into(), ..pg.clone() }, None), server("postgres@[::1]:5432/dbee"), "IPv6");
        // MySQL's and MariaDB's forms the same.
        for kind in [DbKind::MySql, DbKind::MariaDb] {
            let my = ConnectionConfig { kind, host: "mysql80".into(), port: 3306, user: "root".into(), database: String::new(), ..pg.clone() };
            assert_eq!(address(&my, None), server("root@mysql80:3306"), "{kind:?}");
            assert_eq!(address(&ConnectionConfig { database: "Shop".into(), ..my }, None), server("root@mysql80:3306/Shop"), "{kind:?}");
        }
        // Through a tunnel: the database's address as typed, then the SSH host, never its port,
        // its user or the tunnel's local port.
        let ssh = SshConfig { host: "bastion.example.com".into(), port: 2222, user: "me".into(), ..SshConfig::default() };
        let tunnel = ConnectionConfig { host: "db.internal".into(), ssh: Some(ssh), ..pg.clone() };
        assert_eq!(address(&tunnel, None), server("postgres@db.internal:5432/dbee via bastion.example.com"));
        // A file: its path with the home folder as ~.
        let file = ConnectionConfig {
            kind: DbKind::Sqlite,
            file: Some(PathBuf::from("/home/me/data/shop.sqlite")),
            host: String::new(),
            port: 0,
            user: String::new(),
            database: String::new(),
            ..pg.clone()
        };
        let home = Some(Path::new("/home/me"));
        assert_eq!(address(&file, home), Some(Address::File("~/data/shop.sqlite".into())));
        assert_eq!(address(&file, None), Some(Address::File("/home/me/data/shop.sqlite".into())));
        assert_eq!(address(&file, Some(Path::new("/home/m"))), Some(Address::File("/home/me/data/shop.sqlite".into())), "a whole folder only");
        // None when the name block already is it: a name dbee made up, or one that reads the same.
        for made in [ConnectionConfig { name: String::new(), ..pg.clone() }, ConnectionConfig { name: String::new(), ..file }] {
            assert_eq!(address(&made, home), None, "{made:?}");
        }
        assert_eq!(address(&ConnectionConfig { name: "postgres@pg14:5432/dbee".into(), ..pg }, None), None);
    }

    #[test]
    fn the_marker_counts_applied_filters() {
        let f = |n: usize| Condition::Filters((0..n).map(|i| Filter { column: format!("c{i}"), op: Op::Eq, value: "1".into() }).collect());
        assert_eq!(applied_filters(&Condition::None), 0);
        assert_eq!(applied_filters(&f(0)), 0);
        assert_eq!(applied_filters(&f(2)), 2);
        assert_eq!(applied_filters(&Condition::Raw("id > 1".into())), 1);
        assert_eq!(applied_filters(&Condition::Raw("  ".into())), 0);
    }
}

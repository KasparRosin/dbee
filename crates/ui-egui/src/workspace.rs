//! The workspace, shown once a connection is open ([`screen`]):
//!
//! - the menu bar at the top ([`crate::menu`]);
//! - the status bar at the bottom: the connection's name on its colour (with `read-only` when the
//!   session is), the server version, and the active tab's running job with a Cancel button.
//!   When the connection is lost it says so, with a Reconnect button;
//! - the schema tree on the left ([`crate::tree`]);
//! - the tabs strip, then the active tab;
//! - the quick search over everything while it is open ([`crate::quick_search`]).
//!
//! **Tabs strip.** A tab shows its kind's icon, its title (a table tab its table's name, with the
//! number of applied filters as `orders [2]`), a dot when closing it would lose something (query
//! text, staged edits), a spinner while one of its jobs runs, and × to close it; a middle click
//! closes it too, and `+` opens a query tab. Closing goes through `tab.close`, so a tab that would
//! lose something asks first.
//!
//! **Running jobs.** The status bar and the tabs show only live jobs: those a tab's markers name
//! (a page load, a count, an Apply, a query run) and a reconnect. A tab that closed, or whose
//! markers a reconnect cleared, shows none, even while the engine lets its job run to its end.
//!
//! **Tab bodies.** A table tab shows its page ([`crate::table_tab`]). Until the query tab has its
//! own screen, a query tab shows a plain editor (`query-editor`) that sends its text with
//! `query.setText` and keeps its cursor for Ctrl+Shift+Enter.
//!
//! Automation ids: `tab-<id>` (value: the title as shown), `tab-close-<id>`, `tab-dirty-<id>`,
//! `tab-new`; `status-connection` (value: the name, and `read-only`), `status-server`,
//! `status-job` (value: what runs), `status-cancel`, `status-lost`, `status-reconnect`;
//! `query-editor`.

use std::sync::Arc;

use dbee_engine::{Session, State, Tab, TabId};
use dbee_sql::filter::Condition;
use egui::{Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, pos2, vec2};
use serde_json::json;

use crate::app::Origin;
use crate::connections::line_galley;
use crate::quick_search::{self, QuickSearch};
use crate::table_tab::{self, Grids};
use crate::theme::{RADIUS, TOKENS, connection_color};
use crate::tree::{self, Refresh, Tree, TreeAct};
use crate::{DbeeApp, icons, menu};

/// The tabs strip's height.
const STRIP_H: f32 = 34.0;
/// The status bar's height.
pub(crate) const STATUS_H: f32 = 30.0;
/// The widest a tab's title gets before it is cut.
const TITLE_MAX: f32 = 200.0;

/// What only the workspace keeps.
#[derive(Debug, Default)]
pub(crate) struct Workspace {
    pub tree: Tree,
    pub quick: QuickSearch,
    /// The widget to give the keyboard back to in the next frame, once the quick search has
    /// closed (see `quick_search`).
    pub refocus: Option<egui::Id>,
    /// Each table tab's grid (its column widths), dropped with the tab.
    pub grids: Grids,
    /// The connection this state belongs to; another one starts afresh.
    connection: Option<String>,
}

impl Workspace {
    /// The workspace is not shown (back on the connection screen): its state goes, so the quick
    /// search cannot stay open (and hold the shortcuts) and the next connection starts afresh.
    pub(crate) fn leave(&mut self) {
        if self.connection.is_some() || self.quick.open {
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
/// applied condition, or 1 for an applied raw condition.
pub fn applied_filters(applied: &Condition) -> usize {
    match applied {
        Condition::None => 0,
        Condition::Filters(f) => f.len(),
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
fn dirty(tab: &Tab) -> bool {
    match tab {
        Tab::Table(t) => t.has_staged_edits(),
        Tab::Query(q) => !q.text.trim().is_empty(),
    }
}

/// One of `tab`'s jobs runs (by its markers).
fn busy(tab: &Tab) -> bool {
    match tab {
        Tab::Table(t) => t.loading.is_some() || t.counting.is_some() || t.applying.is_some(),
        Tab::Query(q) => q.running.is_some(),
    }
}

/// A live job the status bar shows: what runs, and the command that cancels it.
#[derive(Debug, PartialEq, Eq)]
pub struct LiveJob {
    pub text: String,
    pub cancel: Option<&'static str>,
}

/// The job the status bar shows: a reconnect, else the active tab's (by its markers).
pub fn live_job(session: &Session) -> Option<LiveJob> {
    if session.running("connection.reconnect") {
        return Some(LiveJob { text: "Reconnecting…".into(), cancel: None });
    }
    match session.state().active_tab()? {
        Tab::Table(t) => {
            let name = &t.table.name;
            if t.applying.is_some() {
                Some(LiveJob { text: format!("Applying the changes to {name}…"), cancel: None })
            } else if t.loading.is_some() {
                Some(LiveJob { text: format!("Loading {name}…"), cancel: Some("table.cancel") })
            } else if t.counting.is_some() {
                Some(LiveJob { text: format!("Counting the rows of {name}…"), cancel: Some("table.cancel") })
            } else {
                None
            }
        }
        Tab::Query(q) => q.running.map(|_| LiveJob { text: format!("Running {}…", q.title), cancel: Some("query.cancel") }),
    }
}

/// What the user did outside the tree and the menus this frame.
enum Act {
    Activate(TabId),
    Close(TabId),
    NewQuery,
    Cancel(&'static str),
    Reconnect,
    SetText(String),
}

/// Draws the workspace and acts on it.
pub(crate) fn screen(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = TOKENS;
    let ctx = ui.ctx().clone();
    // A new connection starts with a fresh tree and search.
    let id = app.session.state().connected.as_ref().map(|c| c.config.id.clone());
    if app.view.work.connection != id {
        app.view.work = Workspace { connection: id, ..Workspace::default() };
    }
    let tabs = &app.session.state().tabs;
    app.view.work.grids.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    egui::Panel::top("dbee-menubar").frame(Frame::new().fill(t.panel).inner_margin(Margin::symmetric(8, 3))).show(ui, |ui| menu::bar(app, ui));
    let mut acts = Vec::new();
    status_bar(app, ui, &mut acts);
    side(app, ui);
    egui::CentralPanel::default().frame(Frame::new().fill(t.bg)).show(ui, |ui| {
        strip(app, ui, &mut acts);
        body(app, ui, &mut acts);
    });
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
            Act::Cancel(command) => {
                app.start(command, json!({}), Origin::Menu);
            }
            Act::Reconnect => {
                app.start("connection.reconnect", json!({}), Origin::Menu);
            }
            Act::SetText(text) => {
                if let Err(e) = app.run("query.setText", json!({ "text": text })) {
                    app.notice(e.to_string());
                }
            }
        }
    }
    quick_search::show(app, &ctx);
}

/// The status bar.
fn status_bar(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = TOKENS;
    let Some(c) = app.session.state().connected.as_ref() else { return };
    let name = c.config.display_name();
    let label = if c.info.read_only { format!("{name} · read-only") } else { name };
    let colour = connection_color(c.config.color);
    let server = format!("PostgreSQL {}", c.info.version);
    let lost = c.lost;
    let job = live_job(&app.session);
    let reconnecting = app.session.running("connection.reconnect");
    let tags = &mut app.tags;
    let frame = Frame::new().fill(t.panel).inner_margin(Margin::symmetric(8, 3)).stroke(Stroke::new(1.0, t.border));
    egui::Panel::bottom("dbee-status").exact_size(STATUS_H).frame(frame).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            // The name on the connection's colour, so a live database is never mistaken for
            // another.
            let (fill, text) = match colour {
                Some(c) => (c, readable_on(c)),
                None => (t.border, t.text),
            };
            let galley = line_galley(ui, &label, FontId::proportional(12.5), text, 360.0);
            let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 20.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(RADIUS.saturating_mul(2)), fill);
            ui.painter().galley(pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0), galley, text);
            tags.tag_value("status-connection", &r, label.as_str());
            let r = ui.label(RichText::new(server.as_str()).size(12.0).color(t.text_muted));
            tags.tag_value("status-server", &r, server.as_str());
            if lost {
                ui.add_space(8.0);
                let r = ui.label(RichText::new("Connection lost").size(12.5).strong().color(t.danger));
                tags.tag_value("status-lost", &r, "Connection lost");
                let text = if reconnecting { "Reconnecting…" } else { "Reconnect" };
                let r = ui.add_enabled(!reconnecting, egui::Button::new(RichText::new(text).size(12.0)).min_size(vec2(0.0, 20.0)));
                tags.tag_value_as("status-reconnect", &r, text, !reconnecting);
                if r.clicked() {
                    acts.push(Act::Reconnect);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let Some(job) = &job else { return };
                if let Some(command) = job.cancel {
                    let r = ui.add(egui::Button::new(RichText::new("Cancel").size(12.0)).min_size(vec2(0.0, 20.0)));
                    tags.tag_value("status-cancel", &r, "Cancel");
                    if r.clicked() {
                        acts.push(Act::Cancel(command));
                    }
                }
                let r = ui.label(RichText::new(job.text.as_str()).size(12.0).color(t.text_muted));
                tags.tag_value("status-job", &r, job.text.as_str());
                ui.add(egui::Spinner::new().size(12.0).color(t.text_muted));
            });
        });
    });
}

/// Dark or light text, whichever reads better on `fill`.
fn readable_on(fill: Color32) -> Color32 {
    let lum = 0.2126 * f32::from(fill.r()) + 0.7152 * f32::from(fill.g()) + 0.0722 * f32::from(fill.b());
    if lum > 150.0 { TOKENS.text } else { Color32::WHITE }
}

/// The tree on the left.
fn side(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = TOKENS;
    let Some(catalog) = app.session.state().connected.as_ref().map(|c| Arc::clone(&c.catalog)) else { return };
    let refresh = if app.session.running("schema.refresh") {
        Refresh::Running
    } else {
        match app.session.state().connected.as_ref().map(|c| c.lost) {
            Some(true) => Refresh::Disabled("the connection is lost; reconnect first".into()),
            _ => Refresh::Ready,
        }
    };
    app.view.work.tree.refresh = refresh;
    let mut acts = Vec::new();
    {
        let DbeeApp { view, tags, .. } = app;
        let frame = Frame::new().fill(t.panel).inner_margin(Margin::same(8));
        egui::Panel::left("dbee-tree").resizable(true).default_size(280.0).size_range(200.0..=600.0).frame(frame).show(ui, |ui| {
            acts = tree::show(ui, &catalog, &mut view.work.tree, tags);
        });
    }
    for act in acts {
        match act {
            TreeAct::Open(table) => {
                app.start("table.open", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
            }
            TreeAct::Refresh => {
                app.start("schema.refresh", json!({}), Origin::Menu);
            }
        }
    }
}

/// The tabs strip.
fn strip(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = TOKENS;
    let st: &State = app.session.state();
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
            busy: busy(tab),
        })
        .collect();
    let tags = &mut app.tags;
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), STRIP_H), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, t.panel);
    ui.painter().line_segment([strip.left_bottom(), strip.right_bottom()], Stroke::new(1.0, t.border));
    let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(Layout::left_to_right(Align::Max)));
    ScrollArea::horizontal().id_salt("tab-strip").auto_shrink([false, true]).show(&mut strip_ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for tab in &tabs {
                draw_tab(ui, tab, tags, acts);
            }
            let (rect, r) = ui.allocate_exact_size(vec2(28.0, STRIP_H - 6.0), Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), t.hover);
            }
            icons::plus(ui.painter(), Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), t.text_muted);
            let r = r.on_hover_text("New query tab (Ctrl+T)");
            tags.tag("tab-new", &r);
            if r.clicked() {
                acts.push(Act::NewQuery);
            }
        });
    });
}

fn draw_tab(ui: &mut egui::Ui, tab: &TabView, tags: &mut crate::automation::Tags, acts: &mut Vec<Act>) {
    let t = TOKENS;
    let font = FontId::proportional(13.0);
    let colour = if tab.active { t.text } else { t.text_muted };
    let galley = line_galley(ui, &tab.title, font, colour, TITLE_MAX);
    let width = 10.0 + 16.0 + 6.0 + galley.size().x + 6.0 + if tab.dirty { 10.0 } else { 0.0 } + 20.0 + 6.0;
    let (rect, r) = ui.allocate_exact_size(vec2(width, STRIP_H - 4.0), Sense::click());
    let p = ui.painter().clone();
    let fill = if tab.active {
        t.bg
    } else if r.hovered() {
        t.hover
    } else {
        t.panel
    };
    p.rect_filled(rect, CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 }, fill);
    if tab.active {
        p.line_segment([rect.left_top() + vec2(2.0, 0.0), rect.right_top() - vec2(2.0, 0.0)], Stroke::new(2.0, t.accent));
    }
    let icon_rect = Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - 8.0), vec2(16.0, 16.0));
    if tab.busy {
        egui::Spinner::new().size(13.0).color(t.text_muted).paint_at(ui, icon_rect);
    } else if tab.query {
        icons::query(&p, icon_rect, t.icon_query);
    } else {
        icons::table(&p, icon_rect, t.icon_table);
    }
    let text_left = icon_rect.right() + 6.0;
    let galley_w = galley.size().x;
    p.galley(pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, colour);
    let r = r.on_hover_text(tab.hover.as_str());
    tags.tag_value(&format!("tab-{}", tab.id.0), &r, tab.title.as_str());
    let mut x = text_left + galley_w + 6.0;
    if tab.dirty {
        let dot = Rect::from_center_size(pos2(x + 4.0, rect.center().y), vec2(8.0, 8.0));
        p.circle_filled(dot.center(), 3.5, t.text_muted);
        let d = ui.interact(dot, egui::Id::new(("dbee-tab-dirty", tab.id.0)), Sense::hover());
        tags.tag(&format!("tab-dirty-{}", tab.id.0), &d);
        x += 10.0;
    }
    let close_rect = Rect::from_min_size(pos2(x, rect.center().y - 9.0), vec2(18.0, 18.0));
    let close = ui.interact(close_rect, egui::Id::new(("dbee-tab-close", tab.id.0)), Sense::click());
    if close.hovered() {
        p.rect_filled(close_rect, CornerRadius::same(RADIUS), t.border);
    }
    let cross = if tab.active || r.hovered() || close.hovered() { t.text_muted } else { t.border };
    icons::close(&p, close_rect.shrink(3.0), cross);
    let close = close.on_hover_text("Close (Ctrl+W)");
    tags.tag(&format!("tab-close-{}", tab.id.0), &close);
    if close.clicked() || r.middle_clicked() {
        acts.push(Act::Close(tab.id));
    } else if r.clicked() && !tab.active {
        acts.push(Act::Activate(tab.id));
    }
}

/// The active tab's body (see the module docs), or a hint when no tab is open.
fn body(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = TOKENS;
    if let Some(Tab::Table(tab)) = app.session.state().active_tab() {
        let id = tab.id;
        let content = ui.available_rect_before_wrap().shrink2(egui::vec2(8.0, 0.0));
        let mut ui = ui.new_child(egui::UiBuilder::new().id_salt("dbee-tab-body").max_rect(content).layout(Layout::top_down(Align::Min)));
        table_tab::show(app, &mut ui, id);
        return;
    }
    let content = ui.available_rect_before_wrap().shrink(16.0);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
    match app.session.state().active_tab() {
        None => {
            ui.add_space(content.height() * 0.3);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No tab is open").size(16.0).color(t.text_muted));
                ui.add_space(6.0);
                ui.label(RichText::new("Double-click a table in the tree, find one with Ctrl+P, or start a query with Ctrl+T.").color(t.text_muted));
            });
        }
        // Drawn above.
        Some(Tab::Table(_)) => {}
        Some(Tab::Query(q)) => {
            let mut text = q.text.clone();
            let out = TextEdit::multiline(&mut text)
                .id(egui::Id::new(("query-editor", q.id.0)))
                .code_editor()
                .hint_text("Write SQL here; Ctrl+Enter runs it.")
                .desired_width(f32::INFINITY)
                .desired_rows(16)
                .show(&mut ui);
            app.tags.tag("query-editor", &out.response);
            if let Some(range) = out.cursor_range {
                let chars: usize = range.primary.index.into();
                app.view.query_cursor = Some(text.char_indices().nth(chars).map_or(text.len(), |(b, _)| b));
            }
            if out.response.changed() {
                acts.push(Act::SetText(text));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dbee_sql::filter::{Filter, Op};

    use super::*;

    #[test]
    fn the_marker_counts_applied_filters() {
        let f = |n: usize| Condition::Filters((0..n).map(|i| Filter { column: format!("c{i}"), op: Op::Eq, value: "1".into() }).collect());
        assert_eq!(applied_filters(&Condition::None), 0);
        assert_eq!(applied_filters(&f(0)), 0);
        assert_eq!(applied_filters(&f(2)), 2);
        assert_eq!(applied_filters(&Condition::Raw("id > 1".into())), 1);
        assert_eq!(applied_filters(&Condition::Raw("  ".into())), 0);
    }

    #[test]
    fn text_on_a_connection_colour_stays_readable() {
        use dbee_config::ConnectionColor as C;
        for c in [C::Red, C::Orange, C::Yellow, C::Green, C::Blue, C::Purple, C::Gray] {
            let Some(fill) = connection_color(c) else { continue };
            let text = readable_on(fill);
            let lum = |c: Color32| {
                let lin = |v: u8| {
                    let v = f64::from(v) / 255.0;
                    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
                };
                0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
            };
            let (a, b) = (lum(text), lum(fill));
            let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(contrast >= 3.0, "{c:?}: {contrast}");
        }
    }
}

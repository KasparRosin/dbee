//! The workspace, shown once a connection is open ([`screen`]):
//!
//! - the menu bar at the top ([`crate::menu`]);
//! - the status bar at the bottom: the connection's name on its colour (with `read-only` when the
//!   session is), the server version, Save connection while the saved list doesn't hold the
//!   connection (one opened from a file, say), and the active tab's running job with a Cancel
//!   button. When the connection is lost it says so, with a Reconnect button;
//! - the schema tree on the left ([`crate::tree`]);
//! - the value panel on the right, while it shows ([`crate::value_panel`]), with its toggle at the
//!   right end of the status bar;
//! - the tabs strip, then the active tab.
//!
//! The quick search (Ctrl+P) is drawn over it by the window ([`crate::quick_search`]).
//!
//! **Tabs strip.** The strip is a band of its own (`tab_band`) with a line along its bottom; the
//! tabs stand on the band's bottom edge, and the active one takes the content's fill and opens
//! into the content below it: no line under it, its sides and top drawn, the accent along its top.
//! A tab shows its kind's icon, its title (a table tab its table's name, with the number of
//! applied filters as `orders [2]`), a dot when closing it would lose something (query text,
//! staged edits), a spinner while one of its jobs runs, and × to close it; a middle click closes
//! it too, and `+` opens a query tab. More tabs than fit scroll sideways. Closing goes through
//! `tab.close`, so a tab that would lose something asks first.
//!
//! **Running jobs.** The status bar and the tabs show only live jobs: those a tab's markers name
//! (a page load, a count, an Apply, a query run) and a reconnect. A tab that closed, or whose
//! markers a reconnect cleared, shows none, even while the engine lets its job run to its end.
//!
//! **Tab bodies.** A table tab shows its page ([`crate::table_tab`]), a query tab its editor and
//! results ([`crate::query_tab`]).
//!
//! Automation ids: `tab-<id>` (value: the title as shown), `tab-close-<id>`, `tab-dirty-<id>`,
//! `tab-new`; `status-connection` (value: the name, and `read-only`), `status-server`,
//! `status-tls` (value: what protects the link, for a server), `status-save`, `status-job` (value: what runs), `status-cancel`, `status-lost`,
//! `status-reconnect`, `value-toggle`.

use std::sync::Arc;

use dbee_engine::{Session, State, Tab, TabId, Which};
use dbee_sql::filter::Condition;
use egui::{Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::app::Origin;
use crate::connections::line_galley;
use crate::editors::CellEditor;
use crate::query_tab::{self, Queries};
use crate::table_tab::{self, Grids};
use crate::theme::{RADIUS, TOKENS, connection_color};
use crate::tree::{self, Refresh, Tree, TreeAct};
use crate::{DbeeApp, icons, menu, value_panel};

/// The tabs strip's height: its band.
const STRIP_H: f32 = 36.0;
/// A tab's height; it stands on the band's bottom edge.
const TAB_H: f32 = 30.0;
/// The status bar's height.
pub(crate) const STATUS_H: f32 = 30.0;
/// The height of the status bar's buttons, as of the name on its colour.
const STATUS_ITEM_H: f32 = 20.0;
/// The widest a tab's title gets before it is cut.
const TITLE_MAX: f32 = 200.0;

/// What only the workspace keeps.
#[derive(Debug, Default)]
pub(crate) struct Workspace {
    pub tree: Tree,
    /// Each table tab's grid (its column widths), dropped with the tab.
    pub grids: Grids,
    /// The cell editor, while one is open ([`crate::editors`]).
    pub editor: Option<CellEditor>,
    /// Each query tab's editor text and result grids, dropped with the tab.
    pub queries: Queries,
    /// The query tab whose editor was given the keyboard when it became the active one; `None`
    /// while no query tab is active.
    pub focused_query: Option<TabId>,
    /// The tab the strip last showed as the active one: another one (a tab just opened, say) is
    /// scrolled into view when the strip is wider than the window.
    strip_active: Option<TabId>,
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
                Some(LiveJob { text: format!("Applying the changes to {name}…"), cancel: Some("table.cancel") })
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
    SaveOpen,
}

/// Draws the workspace and acts on it.
pub(crate) fn screen(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = TOKENS;
    // A new connection starts with a fresh tree.
    let id = app.session.state().connection(Which::Active).map(|c| c.config.id.clone());
    if app.view.work.connection != id {
        app.view.work = Workspace { connection: id, ..Workspace::default() };
    }
    let tabs = &app.session.state().tabs;
    app.view.work.grids.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    app.view.work.queries.retain(|id, _| tabs.iter().any(|t| t.id() == *id));
    egui::Panel::top("dbee-menubar").frame(Frame::new().fill(t.panel).inner_margin(Margin::symmetric(8, 3))).show(ui, |ui| menu::bar(app, ui));
    let mut acts = Vec::new();
    if status_bar(app, ui, &mut acts) {
        // At once, so the panel goes or comes in this frame.
        app.start("ui.toggleValuePanel", json!({}), Origin::Menu);
    }
    side(app, ui);
    let panel = value_panel::reserve(app, ui);
    if panel.is_none() {
        app.view.panel.clear();
    }
    egui::CentralPanel::default().frame(Frame::new().fill(t.bg)).show(ui, |ui| {
        strip(app, ui, &mut acts);
        body(app, ui);
    });
    // After the tab's body, so a cell selected in this frame shows its value now.
    if let Some(rect) = panel {
        value_panel::fill(app, ui, rect);
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
            Act::Cancel(command) => {
                app.start(command, json!({}), Origin::Menu);
            }
            Act::Reconnect => {
                app.start("connection.reconnect", json!({}), Origin::Menu);
            }
            Act::SaveOpen => {
                app.start("connection.saveOpen", json!({}), Origin::Menu);
            }
        }
    }
}

/// The status bar; true when the value panel's toggle was clicked.
fn status_bar(app: &mut DbeeApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) -> bool {
    let t = TOKENS;
    let Some(c) = app.session.state().connection(Which::Active) else { return false };
    let name = c.config.display_name();
    let label = if c.info.read_only { format!("{name} · read-only") } else { name };
    let colour = connection_color(c.config.color);
    let server = dbee_engine::state::server_text(&c.info);
    let tls = dbee_engine::state::tls_text(&c.info);
    let saved = app.session.state().store.is_saved(&c.config.id);
    let lost = c.lost;
    let job = live_job(&app.session);
    let reconnecting = app.session.running("connection.reconnect");
    let panel_open = app.view.value_panel;
    let mut toggled = false;
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
            let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, STATUS_ITEM_H), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(RADIUS.saturating_mul(2)), fill);
            ui.painter().galley(pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0), galley, text);
            tags.tag_value("status-connection", &r, label.as_str());
            let r = ui.label(RichText::new(server.as_str()).size(12.0).color(t.text_muted));
            tags.tag_value("status-server", &r, server.as_str());
            if let Some((text, hover)) = tls {
                ui.add_space(6.0);
                let r = ui.label(RichText::new(text).size(12.0).color(t.text_muted)).on_hover_text(hover);
                tags.tag_value("status-tls", &r, text);
            }
            if !saved {
                ui.add_space(8.0);
                let r = status_button(ui, "Save connection", true).on_hover_text("Add this connection to the saved ones");
                tags.tag_value("status-save", &r, "Save connection");
                if r.clicked() {
                    acts.push(Act::SaveOpen);
                }
            }
            if lost {
                ui.add_space(8.0);
                let r = ui.label(RichText::new("Connection lost").size(12.5).strong().color(t.danger));
                tags.tag_value("status-lost", &r, "Connection lost");
                let text = if reconnecting { "Reconnecting…" } else { "Reconnect" };
                let r = status_button(ui, text, !reconnecting);
                tags.tag_value_as("status-reconnect", &r, text, !reconnecting);
                if r.clicked() {
                    acts.push(Act::Reconnect);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The value panel's toggle at the right end, as the panel is the window's.
                toggled = value_panel::toggle(ui, panel_open, tags);
                let Some(job) = &job else { return };
                if let Some(command) = job.cancel {
                    let r = status_button(ui, "Cancel", true);
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
    toggled
}

/// A status bar button reading `text`, [`STATUS_ITEM_H`] tall: the bar's row keeps its height
/// whether a button shows or not, so nothing in it moves when one comes or goes.
fn status_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.y = 0.0;
        ui.add_enabled(enabled, egui::Button::new(RichText::new(text).size(12.0)).min_size(vec2(0.0, STATUS_ITEM_H)))
    })
    .inner
}

/// Dark or light text, whichever reads better on `fill`.
fn readable_on(fill: Color32) -> Color32 {
    let lum = 0.2126 * f32::from(fill.r()) + 0.7152 * f32::from(fill.g()) + 0.0722 * f32::from(fill.b());
    if lum > 150.0 { TOKENS.text } else { Color32::WHITE }
}

/// The tree on the left.
fn side(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = TOKENS;
    let Some(catalog) = app.session.state().connection(Which::Active).map(|c| Arc::clone(&c.catalog)) else { return };
    let dialect = app.session.state().dialect(Which::Active);
    let refresh = if app.session.running("schema.refresh") {
        Refresh::Running
    } else {
        match app.session.state().connection(Which::Active).map(|c| c.lost) {
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
            acts = tree::show(ui, &catalog, dialect, &mut view.work.tree, tags);
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
    // A newly active tab (one just opened, or chosen elsewhere) is scrolled into view.
    let reveal = app.view.work.strip_active != st.active;
    app.view.work.strip_active = st.active;
    let tags = &mut app.tags;
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), STRIP_H), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, t.tab_band);
    let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(Layout::left_to_right(Align::Max)));
    let mut active = None;
    ScrollArea::horizontal().id_salt("tab-strip").auto_shrink([false, false]).show(&mut strip_ui, |ui| {
        // Bottom-aligned in the band's height: every tab stands on the band's bottom edge.
        ui.with_layout(Layout::left_to_right(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(4.0);
            for tab in &tabs {
                if let Some(rect) = draw_tab(ui, tab, tags, acts) {
                    if reveal {
                        ui.scroll_to_rect(rect, None);
                    }
                    active = Some(rect);
                }
            }
            let (slot, r) = ui.allocate_exact_size(vec2(28.0, TAB_H), Sense::click());
            let rect = Rect::from_center_size(slot.center(), vec2(24.0, 24.0));
            if r.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), t.tab_hover);
            }
            icons::plus(ui.painter(), Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), t.text_muted);
            let r = r.on_hover_text("New query tab (Ctrl+T)");
            tags.tag("tab-new", &r);
            if r.clicked() {
                acts.push(Act::NewQuery);
            }
        });
    });
    // The line along the band's bottom, but not under the active tab (the part of it in view),
    // which opens into the content.
    let y = strip.bottom() - 0.5;
    let line = Stroke::new(1.0, t.border);
    let open = active.map(|a| a.intersect(strip)).filter(|a| a.width() > 0.0);
    let (left, right) = open.map_or((strip.right(), strip.right()), |a| (a.left(), a.right()));
    if left > strip.left() {
        ui.painter().line_segment([pos2(strip.left(), y), pos2(left, y)], line);
    }
    if right < strip.right() {
        ui.painter().line_segment([pos2(right, y), pos2(strip.right(), y)], line);
    }
}

/// Draws `tab` on the band; returns its rect when it is the active one.
fn draw_tab(ui: &mut egui::Ui, tab: &TabView, tags: &mut crate::automation::Tags, acts: &mut Vec<Act>) -> Option<Rect> {
    let t = TOKENS;
    let font = FontId::proportional(13.0);
    let colour = if tab.active { t.text } else { t.text_muted };
    let galley = line_galley(ui, &tab.title, font, colour, TITLE_MAX);
    let width = 10.0 + 16.0 + 6.0 + galley.size().x + 6.0 + if tab.dirty { 10.0 } else { 0.0 } + 20.0 + 6.0;
    let (rect, r) = ui.allocate_exact_size(vec2(width, TAB_H), Sense::click());
    let p = ui.painter().clone();
    let corners = CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 };
    if tab.active {
        // The content's fill, its sides and top in the border colour. The shape runs past the
        // band's bottom, where the strip's clip cuts it off: no edge under the tab.
        let open = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y + 2.0));
        p.rect(open, corners, t.bg, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        p.line_segment([rect.left_top() + vec2(2.0, 1.0), rect.right_top() + vec2(-2.0, 1.0)], Stroke::new(2.0, t.accent));
    } else if r.hovered() {
        p.rect_filled(rect, corners, t.tab_hover);
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
    tab.active.then_some(rect)
}

/// The active tab's body (see the module docs), or a hint when no tab is open.
fn body(app: &mut DbeeApp, ui: &mut egui::Ui) {
    let t = TOKENS;
    let active = app.session.state().active_tab().map(|tab| (tab.id(), matches!(tab, Tab::Query(_))));
    if active.is_none_or(|(_, query)| !query) {
        // The next query tab to become the active one takes the keyboard.
        app.view.work.focused_query = None;
        app.view.query_cursor = None;
    }
    let Some((id, query)) = active else {
        let content = ui.available_rect_before_wrap().shrink(16.0);
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        ui.add_space(content.height() * 0.3);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No tab is open").size(16.0).color(t.text_muted));
            ui.add_space(6.0);
            ui.label(RichText::new("Double-click a table in the tree, find one with Ctrl+P, or start a query with Ctrl+T.").color(t.text_muted));
        });
        return;
    };
    let content = ui.available_rect_before_wrap().shrink2(egui::vec2(8.0, 0.0));
    let mut ui = ui.new_child(egui::UiBuilder::new().id_salt("dbee-tab-body").max_rect(content).layout(Layout::top_down(Align::Min)));
    if query {
        query_tab::show(app, &mut ui, id);
    } else {
        table_tab::show(app, &mut ui, id);
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

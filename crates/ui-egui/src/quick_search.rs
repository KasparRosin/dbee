//! The quick search (Ctrl+P, `ui.quickSearch`): a box near the top of the window, over the
//! `scrim`, with a query field and the results from `quickSearch.find {grouped: true}` (the
//! engine's fuzzy ranking), grouped: on the workspace the open connection's tables under its
//! heading (its colour's square mark and its name), then the saved connections but the open one
//! under OTHER CONNECTIONS, each group best first and at most [`LIMIT`] long, a group with no
//! match left out; on the connection screen the saved connections alone, with no heading. The
//! parts are drawn by [`components::quick_search`].
//!
//! The field has the keyboard while it is open: typing narrows the list, ArrowDown and ArrowUp
//! move the highlighted row (over rows only, skipping the headings; the first row is highlighted
//! as the list changes), Enter or a click chooses it, and Escape or a click outside closes the
//! search. Headings and rows are one height, so the list shows as many whole items as fit above the
//! window's bottom edge and scrolls the rest by whole items, keeping the highlighted row (with its
//! heading, when it is its group's first) in view. A table opens in a table tab (`table.open`,
//! through [`DbeeApp::start`]); a connection connects as a double-click on its card does
//! ([`connections::switch_to`]), after the open connection closes through `connection.close` with
//! its questions. It counts as a dialog for the app's other shortcuts, which do not fire while it
//! is open. Closing it gives the keyboard back to the widget that had it when the search opened,
//! so a text field keeps its own keys (Ctrl+A, Ctrl+Z, …).
//!
//! The results are asked for once per query text, catalog and saved list, not per frame; Enter
//! acts on the results for the text as it is after this frame's typing (a paste and Enter in one
//! frame open the pasted name's best match). A dialog closes the search and cannot be opened over one, so
//! the search never takes a dialog's Enter or Escape. The window draws it over the screen it
//! opened on, and it closes when the window shows another screen or another connection.
//!
//! Automation ids: `qs-input` (value: the query); `qs-item-<n>` (value: `schema.table`, or a
//! connection's name), `n` from 0 over the rows in the list's order, with `qs-item-<n>-kind`
//! (value: the database type's label) on a connection's row; `qs-group-<g>` (value: the heading's
//! words as written, before capitals), `g` from 0 top to bottom.

use std::sync::Arc;

use dbee_config::{ConnectionColor, ConnectionConfig};
use dbee_core::{Catalog, RelKind, TableRef};
use dbee_engine::{Json, Outcome, Screen, State, Which};
use egui::{Align2, Key, Modifiers, ScrollArea, Sense, vec2};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::components::quick_search::{self as parts, Field, Heading, Row};
use crate::components::tooltip::Hint;
use crate::connections;
use crate::theme::{
    self, connection_color,
    size::{QS_LIST_GAP, QS_MARGIN, QS_PAD, QS_ROW_H, QS_TOP, QS_W},
};

/// The most results shown in each group.
pub const LIMIT: usize = 20;

/// One result.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hit {
    Table {
        table: TableRef,
        kind: RelKind,
    },
    /// A saved connection: its id, display name, database type's label and colour.
    Connection {
        id: String,
        name: String,
        label: String,
        color: ConnectionColor,
    },
}

impl Hit {
    /// One of `quickSearch.find`'s hits; `None` for a kind of hit this window does not show.
    fn from_json(item: &Json) -> Option<Hit> {
        let text = |k: &str| item.get(k).and_then(Json::as_str).map(str::to_owned);
        match item.get("hit").and_then(Json::as_str)? {
            "table" => {
                let kind = item.get("kind").cloned().and_then(|k| serde_json::from_value::<RelKind>(k).ok()).unwrap_or(RelKind::Table);
                Some(Hit::Table { table: TableRef { schema: text("schema")?, name: text("table")? }, kind })
            }
            "connection" => {
                let color = item.get("color").cloned().and_then(|c| serde_json::from_value::<ConnectionColor>(c).ok()).unwrap_or_default();
                Some(Hit::Connection { id: text("id")?, name: text("name")?, label: text("kindLabel").unwrap_or_default(), color })
            }
            _ => None,
        }
    }

    /// Its automation value: `schema.table`, or the connection's name.
    fn value(&self) -> String {
        match self {
            Hit::Table { table, .. } => table.display(),
            Hit::Connection { name, .. } => name.clone(),
        }
    }
}

/// A group's heading.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Group {
    /// The open connection's tables: its display name and colour.
    Open { name: String, color: ConnectionColor },
    /// The saved connections.
    Others,
}

/// One item of the drawn list, every one [`QS_ROW_H`] tall.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Heading(Group),
    /// The hit `hits[n]`.
    Row(usize),
}

/// The list drawn for `hits`: with `headings` (the workspace), each group's heading before its
/// first hit (`open`: the open connection's name and colour, for its tables' heading); else the
/// rows alone.
fn items_of(hits: &[Hit], open: Option<(String, ConnectionColor)>, headings: bool) -> Vec<Item> {
    let mut items = Vec::with_capacity(hits.len() + 2);
    let mut last = None;
    for (n, hit) in hits.iter().enumerate() {
        let group = match hit {
            Hit::Table { .. } => open.as_ref().map(|(name, color)| Group::Open { name: name.clone(), color: *color }),
            Hit::Connection { .. } => Some(Group::Others),
        };
        if headings && group != last {
            if let Some(g) = &group {
                items.push(Item::Heading(g.clone()));
            }
            last = group;
        }
        items.push(Item::Row(n));
    }
    items
}

/// What results were found for: the catalog searched (none while not connected or lost), the
/// open connection, the saved connections and the query.
struct FoundFor {
    tables: Option<Arc<Catalog>>,
    open: Option<String>,
    saved: Vec<ConnectionConfig>,
    query: String,
}

impl FoundFor {
    fn now(state: &State, query: &str) -> FoundFor {
        let open = state.connection(Which::Active);
        FoundFor {
            tables: open.filter(|c| !c.lost).map(|c| Arc::clone(&c.catalog)),
            open: open.map(|c| c.config.id.clone()),
            saved: state.store.saved.clone(),
            query: query.to_string(),
        }
    }

    /// The results found for `self` still hold in `state` for `query`.
    fn holds(&self, state: &State, query: &str) -> bool {
        let open = state.connection(Which::Active);
        let tables = open.filter(|c| !c.lost).map(|c| &c.catalog);
        let same_tables = match (&self.tables, tables) {
            (Some(had), Some(now)) => Arc::ptr_eq(had, now),
            (None, None) => true,
            _ => false,
        };
        same_tables && self.query == query && self.open.as_deref() == open.map(|c| c.config.id.as_str()) && self.saved == state.store.saved
    }
}

/// Where the search opened: the screen and the open connection's id. It closes when either
/// changes.
type Place = (Screen, Option<String>);

/// The quick search's state.
#[derive(Default)]
pub(crate) struct QuickSearch {
    pub open: bool,
    pub query: String,
    place: Place,
    /// The highlighted result.
    selected: usize,
    /// Scroll the highlighted result into view when the list is next drawn.
    reveal: bool,
    /// The list's scroll offset, as last drawn.
    offset: f32,
    /// The field has had keyboard focus since the search opened.
    focused: bool,
    /// The widget that had keyboard focus when the search opened (`None` when none had), and
    /// whether that was looked at yet.
    restore: Option<egui::Id>,
    remembered: bool,
    hits: Vec<Hit>,
    /// The list drawn for `hits`: headings and rows.
    items: Vec<Item>,
    /// Why there are no results to show (the command failed), if it failed.
    error: Option<String>,
    /// What `hits` were found for.
    found_for: Option<FoundFor>,
    /// The open connection's tables are searched (connected, not lost), as `hits` were found.
    tables: bool,
}

impl std::fmt::Debug for QuickSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the catalog the results were found in.
        f.debug_struct("QuickSearch").field("open", &self.open).field("query", &self.query).field("hits", &self.hits.len()).finish_non_exhaustive()
    }
}

impl QuickSearch {
    /// Opens the search, empty, over what `state` shows.
    pub(crate) fn show_new(&mut self, state: &State) {
        *self = QuickSearch { open: true, place: place_of(state), reveal: true, ..QuickSearch::default() };
    }
}

/// What `state` shows, as a [`Place`].
fn place_of(state: &State) -> Place {
    (state.screen, state.connection(Which::Active).map(|c| c.config.id.clone()))
}

/// When the keyboard goes back to the widget that had it before the search.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refocus {
    /// This frame: the search was not drawn in it.
    Now,
    /// In the next frame, after the screen is drawn: egui keeps the keyboard from every widget
    /// below a modal for one frame after the modal was last drawn, so a widget given it now
    /// would give it up again.
    NextFrame,
}

/// Closes the search and gives the keyboard back to the widget that had it ([`Refocus`]).
fn close(app: &mut DbeeApp, ctx: &egui::Context, when: Refocus) {
    let qs = std::mem::take(&mut app.view.quick);
    ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(INPUT)));
    match (qs.restore, when) {
        (Some(id), Refocus::Now) => ctx.memory_mut(|m| m.request_focus(id)),
        (Some(id), Refocus::NextFrame) => app.view.quick_refocus = Some((id, qs.place)),
        (None, _) => {}
    }
}

/// The field's automation id and egui id.
const INPUT: &str = "qs-input";

/// What the user did in the search this frame.
enum Act {
    None,
    Close,
    Open(Hit),
    /// Enter: open the highlighted result for the text as it is now.
    Enter,
}

/// The scroll offset that brings items `first..=last` of the list into a view `view_h` tall,
/// scrolled to `offset`, moving it as little as it can; items are `pitch` tall, one after the
/// other. When they don't all fit, `last` alone.
fn reveal_offset(first: usize, last: usize, offset: f32, view_h: f32, pitch: f32) -> f32 {
    let bottom = (last as f32 + 1.0) * pitch;
    let top = if bottom - first as f32 * pitch > view_h { last as f32 * pitch } else { first as f32 * pitch };
    if top < offset {
        top
    } else if bottom > offset + view_h {
        bottom - view_h
    } else {
        offset
    }
}

/// Asks `quickSearch.find` for the query when it, the catalog or the saved connections changed.
fn find(app: &mut DbeeApp) {
    let qs = &app.view.quick;
    if qs.found_for.as_ref().is_some_and(|f| f.holds(app.session.state(), &qs.query)) {
        return;
    }
    let found_for = FoundFor::now(app.session.state(), &qs.query);
    let result = app.run("quickSearch.find", json!({ "query": found_for.query, "limit": LIMIT, "grouped": true }));
    let state = app.session.state();
    let headings = state.screen == Screen::Workspace;
    let open = state.connection(Which::Active).map(|c| (c.config.display_name(), c.config.color));
    let qs = &mut app.view.quick;
    qs.tables = found_for.tables.is_some();
    qs.found_for = Some(found_for);
    qs.hits.clear();
    qs.error = None;
    match result {
        Ok(Outcome::Done(Json::Array(items))) => qs.hits.extend(items.iter().filter_map(Hit::from_json)),
        Ok(_) => qs.error = Some("The search gave no list.".into()),
        Err(e) => qs.error = Some(crate::app::notice_text(&e)),
    }
    qs.items = items_of(&qs.hits, open, headings);
    qs.selected = qs.selected.min(qs.hits.len().saturating_sub(1));
}

/// Draws the search when it is open, and acts on it. Runs after the screen is drawn.
pub(crate) fn show(app: &mut DbeeApp, ctx: &egui::Context) {
    let here = place_of(app.session.state());
    if let Some((id, place)) = app.view.quick_refocus.take()
        && place == here
    {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    if !app.view.quick.open {
        return;
    }
    if app.view.quick.place != here {
        // The screen it searched is gone, and with it the widget to give the keyboard back to.
        app.view.quick = QuickSearch::default();
        return;
    }
    if app.view.dialog.is_some() {
        // The dialog has the keyboard: its Enter and Escape are its own.
        close(app, ctx, Refocus::Now);
        return;
    }
    let qs = &mut app.view.quick;
    if !qs.remembered {
        qs.restore = ctx.memory(|m| m.focused());
        qs.remembered = true;
    }
    // Escape closes the search before it is drawn, so the keyboard goes back at once.
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        close(app, ctx, Refocus::Now);
        return;
    }
    find(app);
    let act = {
        let DbeeApp { view, tags, .. } = app;
        draw(ctx, &mut view.quick, tags)
    };
    let open = match act {
        Act::None => return,
        Act::Close => None,
        Act::Open(hit) => Some(hit),
        Act::Enter => {
            // The results for the text typed this frame, not those drawn.
            find(app);
            let qs = &app.view.quick;
            match qs.hits.get(qs.selected) {
                Some(hit) => Some(hit.clone()),
                None => return,
            }
        }
    };
    close(app, ctx, Refocus::NextFrame);
    match open {
        Some(Hit::Table { table, .. }) => {
            app.start("table.open", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
        }
        Some(Hit::Connection { id, .. }) => connections::switch_to(app, connections::SwitchTo::Saved(id)),
        None => {}
    }
}

fn draw(ctx: &egui::Context, qs: &mut QuickSearch, tags: &mut Tags) -> Act {
    let t = theme::tokens(ctx);
    // The search's own keys, before the field could take them.
    let (down, up, enter) = ctx.input_mut(|i| {
        (
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    if !qs.hits.is_empty() {
        let last = qs.hits.len() - 1;
        qs.selected = qs.selected.saturating_add(down).saturating_sub(up).min(last);
    }
    if down > 0 || up > 0 {
        qs.reveal = true;
    }
    let id = egui::Id::new("dbee-quick-search");
    let screen = ctx.content_rect();
    let w = QS_W.min(screen.width() - 2.0 * QS_MARGIN).max(0.0);
    let inner = (w - 2.0 * f32::from(QS_PAD)).max(0.0);
    // A fixed corner, so the first frame puts every widget where the next ones do.
    let corner = vec2(((screen.width() - w) / 2.0).max(0.0), QS_TOP.min(screen.height() * 0.1));
    let area = egui::Modal::default_area(id).anchor(Align2::LEFT_TOP, corner).fade_in(false);
    let mut act = Act::None;
    let modal = egui::Modal::new(id).area(area).frame(parts::frame(t)).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(inner);
        ui.spacing_mut().item_spacing.y = 0.0;
        let placeholder = if qs.tables { "Find a table or connection" } else { "Find a connection" };
        let field = Field { text: &mut qs.query, placeholder, id: egui::Id::new(INPUT) }.show(ui, inner);
        tags.tag_value_as(INPUT, &field, qs.query.as_str(), true);
        if !qs.focused {
            field.request_focus();
            qs.focused = field.has_focus();
        }
        if field.changed() {
            qs.selected = 0;
            qs.reveal = true;
        }
        ui.add_space(QS_LIST_GAP);
        if let Some(e) = &qs.error {
            ui.label(egui::RichText::new(e.as_str()).color(t.danger));
        } else if qs.hits.is_empty() {
            let none = if qs.tables { "No table or connection matches." } else { "No connection matches." };
            ui.label(egui::RichText::new(none).color(t.text_muted));
        }
        let room = screen.bottom() - QS_MARGIN - f32::from(QS_PAD) - ui.cursor().top();
        // Whole items only, so moving the highlight scrolls by whole items.
        let list_h = (room / QS_ROW_H).floor().max(1.0) * QS_ROW_H;
        // The modal's room is last frame's size: the minimum lets the list grow past it.
        let mut list = ScrollArea::vertical().id_salt("qs-list").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, true]);
        if std::mem::take(&mut qs.reveal)
            && let Some(at) = qs.items.iter().position(|i| *i == Item::Row(qs.selected))
        {
            // A group's first row comes into view with its heading.
            let first = if at > 0 && matches!(qs.items.get(at - 1), Some(Item::Heading(_))) { at - 1 } else { at };
            list = list.vertical_scroll_offset(reveal_offset(first, at, qs.offset, list_h, QS_ROW_H));
        }
        let shown = list.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut group = 0;
            for item in &qs.items {
                match item {
                    Item::Heading(g) => {
                        let (rect, r) = ui.allocate_exact_size(vec2(inner, QS_ROW_H), Sense::hover());
                        let heading = match g {
                            Group::Open { name, color } => Heading::Connection { name, colour: connection_color(*color) },
                            Group::Others => Heading::Others,
                        };
                        let cut = heading.paint(ui, rect);
                        tags.tag_value_as(&format!("qs-group-{group}"), &r, heading.words(), true);
                        if let Some(whole) = cut {
                            r.hint(whole);
                        }
                        group += 1;
                    }
                    Item::Row(n) => {
                        let Some(hit) = qs.hits.get(*n) else { continue };
                        let (rect, r) = ui.allocate_exact_size(vec2(inner, QS_ROW_H), Sense::click());
                        let row = Row {
                            hit: match hit {
                                Hit::Table { table, kind } => parts::Hit::Table { schema: &table.schema, name: &table.name, kind: *kind },
                                Hit::Connection { name, label, color, .. } => {
                                    parts::Hit::Connection { name, label, colour: connection_color(*color) }
                                }
                            },
                            highlighted: *n == qs.selected,
                            hovered: r.hovered(),
                        };
                        let cut = row.paint(ui, rect);
                        r.widget_info(|| row.widget_info());
                        if let Hit::Connection { label, .. } = hit {
                            tags.tag_value_as(&format!("qs-item-{n}-kind"), &r, label.as_str(), true);
                        }
                        tags.tag_value_as(&format!("qs-item-{n}"), &r, hit.value(), true);
                        if r.clicked() {
                            act = Act::Open(hit.clone());
                        }
                        if let Some(whole) = cut {
                            r.hint(whole);
                        }
                    }
                }
            }
        });
        qs.offset = shown.state.offset.y;
    });
    if !matches!(act, Act::None) {
        return act;
    }
    if modal.backdrop_response.clicked() {
        return Act::Close;
    }
    if enter { Act::Enter } else { Act::None }
}

#[cfg(test)]
mod tests {
    use dbee_engine::{Session, SessionOptions};
    use egui::{Event, RawInput};

    use super::*;
    use crate::dialogs::{Dialog, Rerun};

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn each_group_gets_its_heading_on_the_workspace_only() {
        let table = |name: &str| Hit::Table { table: TableRef { schema: "shop".into(), name: name.into() }, kind: RelKind::Table };
        let conn = |name: &str| Hit::Connection { id: name.into(), name: name.into(), label: "MySQL".into(), color: ConnectionColor::None };
        let open = Some(("Shop live".to_owned(), ConnectionColor::Red));
        let hits = [table("orders"), table("order_items"), conn("Orders replica")];
        let shop = Item::Heading(Group::Open { name: "Shop live".into(), color: ConnectionColor::Red });
        assert_eq!(items_of(&hits, open.clone(), true), [shop, Item::Row(0), Item::Row(1), Item::Heading(Group::Others), Item::Row(2)]);
        assert_eq!(items_of(&hits[2..], open, true), [Item::Heading(Group::Others), Item::Row(0)], "a group with no match is left out");
        assert_eq!(items_of(&hits[2..], None, false), [Item::Row(0)], "no heading on the connection screen");
    }

    #[test]
    fn a_reveal_brings_the_heading_along_when_it_fits() {
        // Item 5 below a view of 3 items at the top: scrolled so it is the last in view.
        assert_eq!(reveal_offset(4, 5, 0.0, 96.0, 32.0), 96.0);
        // Item 1 above a view scrolled to item 4: its heading (item 0) comes too.
        assert_eq!(reveal_offset(0, 1, 128.0, 96.0, 32.0), 0.0);
        // A view of one item: the row alone.
        assert_eq!(reveal_offset(0, 1, 128.0, 32.0, 32.0), 32.0);
    }

    #[test]
    fn a_dialog_closes_the_search_and_keeps_its_keys() {
        let home = dbee_testkit::temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        app.view.quick.show_new(app.session.state());
        let rerun = Rerun { command: "tab.close", params: json!({}), origin: Origin::Menu };
        app.view.dialog = Some(Dialog::Confirm { text: "Close it?".into(), what: String::new(), rerun });
        let why = app.commands().into_iter().find(|c| c.id == "ui.quickSearch").and_then(|c| c.disabled_reason);
        assert!(why.as_deref().is_some_and(|w| w.contains("dialog")), "no quick search over a dialog: {why:?}");
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        // The window's order: the workspace (and the search), then the dialog.
        let frame = |app: &mut DbeeApp, events: Vec<Event>| {
            ctx.run_ui(RawInput { events, ..RawInput::default() }, |ui| {
                show(app, ui.ctx());
                crate::dialogs::show(app, ui.ctx());
            })
            .drop_without_applying_deltas();
        };
        frame(&mut app, vec![]);
        assert!(!app.view.quick.open, "a dialog closes the search");
        frame(&mut app, vec![key(Key::Escape)]);
        assert!(app.view.dialog.is_none(), "Escape reached the dialog");
    }
}

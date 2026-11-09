//! The quick search (Ctrl+P, `ui.quickSearch`): a modal near the top of the window with a query
//! field and up to [`LIMIT`] results, best first, from `quickSearch.find` (the engine's fuzzy
//! ranking): the open connection's tables and the saved connections but the open one, or only
//! the saved connections on the connection screen. A table shows its kind's icon and
//! `schema.name`; a connection its colour as a dot, its name and its database type's label.
//!
//! The field has the keyboard while it is open: typing narrows the list, ArrowDown and ArrowUp
//! move the highlighted result, Enter or a click chooses it, and Escape or a click outside closes
//! the search. The list shows as many whole rows as fit above the window's bottom edge and scrolls
//! the rest, keeping the highlighted row in view. A table opens in a table tab (`table.open`,
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
//! Automation ids: `qs-input` (value: the query) and `qs-item-<n>` (value: `schema.table`, or a
//! connection's name), `n` from 0, best first, with `qs-item-<n>-kind` (value: the database
//! type's label) on a connection's row.

use std::sync::Arc;

use dbee_config::{ConnectionColor, ConnectionConfig};
use dbee_core::{Catalog, RelKind, TableRef};
use dbee_engine::{Json, Outcome, Screen, State, Which};
use egui::{Align2, Color32, CornerRadius, FontId, Frame, Key, Margin, Modifiers, Rect, ScrollArea, Sense, Stroke, TextEdit, pos2, vec2};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::connections::{self, line_galley};
use crate::theme::{
    self, connection_color,
    size::{RADIUS_CONTROL, RADIUS_FLOAT},
};
use crate::tree::kind_icon;

/// The most results shown.
pub const LIMIT: usize = 20;
/// The modal's content width.
const WIDTH: f32 = 560.0;
/// How far below the top of the window it sits.
const TOP: f32 = 56.0;
/// A result row.
const ITEM_H: f32 = 28.0;
/// The radius of a connection's colour dot, where a table's icon is.
const DOT_R: f32 = 5.0;
/// The space inside the modal's border.
const PAD: i8 = 12;
/// The least space between the modal and the window's bottom edge.
const BOTTOM: f32 = 12.0;

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

/// The scroll offset that brings row `n` of the list into a view `view_h` tall, scrolled to
/// `offset`, moving it as little as it can; rows are `pitch` apart.
fn reveal_offset(n: usize, offset: f32, view_h: f32, pitch: f32) -> f32 {
    let top = n as f32 * pitch;
    let bottom = top + ITEM_H;
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
    let result = app.run("quickSearch.find", json!({ "query": found_for.query, "limit": LIMIT }));
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
    let outer = WIDTH + 2.0 * f32::from(PAD) + 2.0;
    // A fixed corner, so the first frame puts every widget where the next ones do.
    let offset = vec2(((screen.width() - outer) / 2.0).max(0.0), TOP.min(screen.height() * 0.1));
    let area = egui::Modal::default_area(id).anchor(Align2::LEFT_TOP, offset).fade_in(false);
    let frame = Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(1.0, t.raised_line))
        .corner_radius(CornerRadius::same(RADIUS_FLOAT))
        .inner_margin(Margin::same(PAD));
    let mut act = Act::None;
    let modal = egui::Modal::new(id).area(area).frame(frame).backdrop_color(t.text.gamma_multiply(0.25)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        let field = ui.add(
            TextEdit::singleline(&mut qs.query)
                .id(egui::Id::new(INPUT))
                .hint_text(if qs.tables { "Find a table or connection by name" } else { "Find a connection by name" })
                .desired_width(WIDTH)
                .font(FontId::proportional(16.0))
                .margin(Margin::symmetric(8, 6)),
        );
        tags.tag_value_as(INPUT, &field, qs.query.as_str(), true);
        if !qs.focused {
            field.request_focus();
            qs.focused = field.has_focus();
        }
        if field.changed() {
            qs.selected = 0;
            qs.reveal = true;
        }
        ui.add_space(6.0);
        if let Some(e) = &qs.error {
            ui.label(egui::RichText::new(e.as_str()).color(t.danger));
        } else if qs.hits.is_empty() {
            let none = if qs.tables { "No table or connection matches." } else { "No connection matches." };
            ui.label(egui::RichText::new(none).color(t.text_muted));
        }
        let gap = ui.spacing().item_spacing.y;
        let pitch = ITEM_H + gap;
        let room = screen.bottom() - ui.cursor().top() - f32::from(PAD) - 1.0 - BOTTOM;
        // Whole rows only, so moving the highlight scrolls by whole rows.
        let list_h = ((room + gap) / pitch).floor().max(1.0) * pitch - gap;
        // The modal's room is last frame's size: the minimum lets the list grow past it.
        let mut list = ScrollArea::vertical().id_salt("qs-list").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, true]);
        if std::mem::take(&mut qs.reveal) {
            list = list.vertical_scroll_offset(reveal_offset(qs.selected, qs.offset, list_h, pitch));
        }
        let shown = list.show(ui, |ui| {
            for (n, hit) in qs.hits.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(vec2(WIDTH, ITEM_H), Sense::click());
                let fill = if n == qs.selected || r.hovered() { t.raised_hover } else { Color32::TRANSPARENT };
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
                let mark = pos2(rect.left() + 16.0, rect.center().y);
                let x = rect.left() + 32.0;
                let font = FontId::proportional(14.0);
                match hit {
                    Hit::Table { table, kind } => {
                        kind_icon(*kind)(p, Rect::from_center_size(mark, vec2(16.0, 16.0)), t.text_muted);
                        let schema = line_galley(ui, &format!("{}.", table.schema), font.clone(), t.text_muted, WIDTH * 0.4);
                        let schema_w = schema.size().x;
                        p.galley(pos2(x, rect.center().y - schema.size().y / 2.0), schema, t.text_muted);
                        let name = line_galley(ui, &table.name, font, t.text, (rect.right() - 8.0 - x - schema_w).max(1.0));
                        p.galley(pos2(x + schema_w, rect.center().y - name.size().y / 2.0), name, t.text);
                    }
                    Hit::Connection { name, label, color, .. } => {
                        match connection_color(*color) {
                            Some(c) => {
                                p.circle_filled(mark, DOT_R, c);
                            }
                            // No colour: a ring, as in the form, dark enough for the highlight.
                            None => {
                                p.circle_stroke(mark, DOT_R - 0.75, Stroke::new(1.5, t.text_muted));
                            }
                        }
                        let kind = line_galley(ui, label, font.clone(), t.text_muted, WIDTH * 0.3);
                        let kind_x = rect.right() - 8.0 - kind.size().x;
                        let name = line_galley(ui, name, font, t.text, (kind_x - 12.0 - x).max(1.0));
                        p.galley(pos2(x, rect.center().y - name.size().y / 2.0), name, t.text);
                        p.galley(pos2(kind_x, rect.center().y - kind.size().y / 2.0), kind, t.text_muted);
                        tags.tag_value_as(&format!("qs-item-{n}-kind"), &r, label.as_str(), true);
                    }
                }
                tags.tag_value_as(&format!("qs-item-{n}"), &r, hit.value(), true);
                if r.clicked() {
                    act = Act::Open(hit.clone());
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
    fn a_dialog_closes_the_search_and_keeps_its_keys() {
        let home = dbee_testkit::temp_home(None, None);
        let session = Session::new(SessionOptions { config_dir: Some(home.home.join("config")), ..SessionOptions::default() }).unwrap();
        let mut app = DbeeApp::new(session, None);
        app.view.quick.show_new(app.session.state());
        let rerun = Rerun { command: "tab.close", params: json!({}), origin: Origin::Menu };
        app.view.dialog = Some(Dialog::Confirm { message: "Close it?".into(), what: String::new(), rerun });
        let why = app.commands().into_iter().find(|c| c.id == "ui.quickSearch").and_then(|c| c.disabled_reason);
        assert!(why.as_deref().is_some_and(|w| w.contains("dialog")), "no quick search over a dialog: {why:?}");
        let ctx = egui::Context::default();
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

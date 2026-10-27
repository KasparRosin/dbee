//! The quick search (Ctrl+P, `ui.quickSearch`): a modal near the top of the window with a query
//! field and up to [`LIMIT`] tables, best first, from `quickSearch.find` (the engine's fuzzy
//! ranking over the catalog).
//!
//! The field has the keyboard while it is open: typing narrows the list, ArrowDown and ArrowUp
//! move the highlighted result, Enter opens it in a table tab (`table.open`, through
//! [`DbeeApp::start`]), and Escape or a click outside closes the search. It counts as a dialog for
//! the app's other shortcuts, which do not fire while it is open. Closing it gives the keyboard
//! back to the widget that had it when the search opened, so a text field keeps its own keys
//! (Ctrl+A, Ctrl+Z, …).
//!
//! The results are asked for once per query text and catalog, not per frame; Enter acts on the
//! results for the text as it is after this frame's typing (a paste and Enter in one frame open
//! the pasted name's best match). A dialog closes the search and cannot be opened over one, so
//! the search never takes a dialog's Enter or Escape.
//!
//! Automation ids: `qs-input` (value: the query) and `qs-item-<n>` (value: `schema.table`), `n`
//! from 0, best first.

use std::sync::Arc;

use dbee_core::{Catalog, RelKind, TableRef};
use dbee_engine::{Json, Outcome};
use egui::{Align2, Color32, CornerRadius, FontId, Frame, Key, Margin, Modifiers, Rect, Sense, Stroke, TextEdit, pos2, vec2};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::connections::line_galley;
use crate::theme::{RADIUS, TOKENS};
use crate::tree::kind_icon;

/// The most results shown.
pub const LIMIT: usize = 20;
/// The modal's content width.
const WIDTH: f32 = 560.0;
/// How far below the top of the window it sits.
const TOP: f32 = 56.0;
/// A result row.
const ITEM_H: f32 = 28.0;

/// One table found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hit {
    table: TableRef,
    kind: RelKind,
}

/// The quick search's state.
#[derive(Default)]
pub(crate) struct QuickSearch {
    pub open: bool,
    pub query: String,
    /// The highlighted result.
    selected: usize,
    /// The field has had keyboard focus since the search opened.
    focused: bool,
    /// The widget that had keyboard focus when the search opened (`None` when none had), and
    /// whether that was looked at yet.
    restore: Option<egui::Id>,
    remembered: bool,
    hits: Vec<Hit>,
    /// Why there are no results to show (the command failed), if it failed.
    error: Option<String>,
    /// What `hits` were found for: the catalog and the query.
    found_for: Option<(Arc<Catalog>, String)>,
}

impl std::fmt::Debug for QuickSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the catalog the results were found in.
        f.debug_struct("QuickSearch").field("open", &self.open).field("query", &self.query).field("hits", &self.hits.len()).finish_non_exhaustive()
    }
}

impl QuickSearch {
    /// Opens the search, empty.
    pub(crate) fn show_new(&mut self) {
        *self = QuickSearch { open: true, ..QuickSearch::default() };
    }
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
    let qs = std::mem::take(&mut app.view.work.quick);
    ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(INPUT)));
    match (qs.restore, when) {
        (Some(id), Refocus::Now) => ctx.memory_mut(|m| m.request_focus(id)),
        (Some(id), Refocus::NextFrame) => app.view.work.refocus = Some(id),
        (None, _) => {}
    }
}

/// The field's automation id and egui id.
const INPUT: &str = "qs-input";

/// What the user did in the search this frame.
enum Act {
    None,
    Close,
    Open(TableRef),
    /// Enter: open the highlighted result for the text as it is now.
    Enter,
}

/// Asks `quickSearch.find` for the query when it or the catalog changed.
fn find(app: &mut DbeeApp) {
    let catalog = app.session.state().connected.as_ref().map(|c| Arc::clone(&c.catalog));
    let qs = &app.view.work.quick;
    let fresh = match (&qs.found_for, &catalog) {
        (Some((had, query)), Some(now)) => Arc::ptr_eq(had, now) && *query == qs.query,
        _ => false,
    };
    if fresh {
        return;
    }
    let query = qs.query.clone();
    let result = app.run("quickSearch.find", json!({ "query": query, "limit": LIMIT }));
    let qs = &mut app.view.work.quick;
    qs.found_for = catalog.map(|c| (c, query));
    qs.hits.clear();
    qs.error = None;
    match result {
        Ok(Outcome::Done(Json::Array(items))) => {
            for item in items {
                let text = |k: &str| item.get(k).and_then(Json::as_str).map(str::to_owned);
                let kind = item.get("kind").cloned().and_then(|k| serde_json::from_value::<RelKind>(k).ok()).unwrap_or(RelKind::Table);
                if let (Some(schema), Some(name)) = (text("schema"), text("table")) {
                    qs.hits.push(Hit { table: TableRef { schema, name }, kind });
                }
            }
        }
        Ok(_) => qs.error = Some("The search gave no list.".into()),
        Err(e) => qs.error = Some(crate::app::notice_text(&e)),
    }
    qs.selected = qs.selected.min(qs.hits.len().saturating_sub(1));
}

/// Draws the search when it is open, and acts on it. Runs after the screen is drawn.
pub(crate) fn show(app: &mut DbeeApp, ctx: &egui::Context) {
    if let Some(id) = app.view.work.refocus.take() {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    if !app.view.work.quick.open {
        return;
    }
    if app.view.dialog.is_some() {
        // The dialog has the keyboard: its Enter and Escape are its own.
        close(app, ctx, Refocus::Now);
        return;
    }
    let qs = &mut app.view.work.quick;
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
        draw(ctx, &mut view.work.quick, tags)
    };
    let open = match act {
        Act::None => return,
        Act::Close => None,
        Act::Open(table) => Some(table),
        Act::Enter => {
            // The results for the text typed this frame, not those drawn.
            find(app);
            let qs = &app.view.work.quick;
            match qs.hits.get(qs.selected) {
                Some(hit) => Some(hit.table.clone()),
                None => return,
            }
        }
    };
    close(app, ctx, Refocus::NextFrame);
    if let Some(table) = open {
        app.start("table.open", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
    }
}

fn draw(ctx: &egui::Context, qs: &mut QuickSearch, tags: &mut Tags) -> Act {
    let t = TOKENS;
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
    let id = egui::Id::new("dbee-quick-search");
    let screen = ctx.content_rect();
    let outer = WIDTH + 2.0 * 12.0 + 2.0;
    // A fixed corner, so the first frame puts every widget where the next ones do.
    let offset = vec2(((screen.width() - outer) / 2.0).max(0.0), TOP.min(screen.height() * 0.1));
    let area = egui::Modal::default_area(id).anchor(Align2::LEFT_TOP, offset).fade_in(false);
    let frame = Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(RADIUS.saturating_mul(2)))
        .inner_margin(Margin::same(12));
    let mut act = Act::None;
    let modal = egui::Modal::new(id).area(area).frame(frame).backdrop_color(t.text.gamma_multiply(0.25)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        let field = ui.add(
            TextEdit::singleline(&mut qs.query)
                .id(egui::Id::new(INPUT))
                .hint_text("Find a table by name")
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
        }
        ui.add_space(6.0);
        if let Some(e) = &qs.error {
            ui.label(egui::RichText::new(e.as_str()).color(t.danger));
        } else if qs.hits.is_empty() {
            ui.label(egui::RichText::new("No table matches.").color(t.text_muted));
        }
        for (n, hit) in qs.hits.iter().enumerate() {
            let (rect, r) = ui.allocate_exact_size(vec2(WIDTH, ITEM_H), Sense::click());
            let fill = if n == qs.selected {
                t.selection
            } else if r.hovered() {
                t.hover
            } else {
                Color32::TRANSPARENT
            };
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(RADIUS), fill);
            let (icon, color) = kind_icon(hit.kind);
            icon(p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(16.0, 16.0)), color);
            let schema = line_galley(ui, &format!("{}.", hit.table.schema), FontId::proportional(14.0), t.text_muted, WIDTH * 0.4);
            let x = rect.left() + 32.0;
            let schema_w = schema.size().x;
            p.galley(pos2(x, rect.center().y - schema.size().y / 2.0), schema, t.text_muted);
            let name = line_galley(ui, &hit.table.name, FontId::proportional(14.0), t.text, (rect.right() - 8.0 - x - schema_w).max(1.0));
            p.galley(pos2(x + schema_w, rect.center().y - name.size().y / 2.0), name, t.text);
            tags.tag_value_as(&format!("qs-item-{n}"), &r, hit.table.display(), true);
            if r.clicked() {
                act = Act::Open(hit.table.clone());
            }
        }
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
        app.view.work.quick.show_new();
        let rerun = Rerun { command: "tab.close", params: json!({}), origin: Origin::Menu };
        app.view.dialog = Some(Dialog::Confirm { message: "Close it?".into(), rerun });
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
        assert!(!app.view.work.quick.open, "a dialog closes the search");
        frame(&mut app, vec![key(Key::Escape)]);
        assert!(app.view.dialog.is_none(), "Escape reached the dialog");
    }
}

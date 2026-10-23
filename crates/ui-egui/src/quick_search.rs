//! The quick search (Ctrl+P, `ui.quickSearch`): a modal near the top of the window with a query
//! field and up to [`LIMIT`] tables, best first, from `quickSearch.find` (the engine's fuzzy
//! ranking over the catalog).
//!
//! The field has the keyboard while it is open: typing narrows the list, ArrowDown and ArrowUp
//! move the highlighted result, Enter opens it in a table tab (`table.open`, through
//! [`DbeeApp::start`]), and Escape or a click outside closes the search. It counts as a dialog for
//! the app's other shortcuts, which do not fire while it is open.
//!
//! The results are asked for once per query text and catalog, not per frame.
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

    fn close(&mut self) {
        *self = QuickSearch::default();
    }
}

/// What the user did in the search this frame.
enum Act {
    None,
    Close,
    Open(TableRef),
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
        Err(e) => qs.error = Some(e.to_string()),
    }
    qs.selected = qs.selected.min(qs.hits.len().saturating_sub(1));
}

/// Draws the search when it is open, and acts on it.
pub(crate) fn show(app: &mut DbeeApp, ctx: &egui::Context) {
    if !app.view.work.quick.open {
        return;
    }
    find(app);
    let act = {
        let DbeeApp { view, tags, .. } = app;
        draw(ctx, &mut view.work.quick, tags)
    };
    match act {
        Act::None => {}
        Act::Close => app.view.work.quick.close(),
        Act::Open(table) => {
            app.view.work.quick.close();
            app.start("table.open", json!({ "schema": table.schema, "table": table.name }), Origin::Menu);
        }
    }
}

fn draw(ctx: &egui::Context, qs: &mut QuickSearch, tags: &mut Tags) -> Act {
    let t = TOKENS;
    // The search's own keys, before the field could take them.
    let (down, up, enter, escape) = ctx.input_mut(|i| {
        (
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
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
                .id(egui::Id::new("qs-input"))
                .hint_text("Find a table by name")
                .desired_width(WIDTH)
                .font(FontId::proportional(16.0))
                .margin(Margin::symmetric(8, 6)),
        );
        tags.tag_value_as("qs-input", &field, qs.query.as_str(), true);
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
    if escape || modal.backdrop_response.clicked() {
        return Act::Close;
    }
    if enter && let Some(hit) = qs.hits.get(qs.selected) {
        return Act::Open(hit.table.clone());
    }
    Act::None
}

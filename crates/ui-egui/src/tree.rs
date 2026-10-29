//! The schema tree, on the workspace's left ([`show`]): a filter box and a refresh button, then
//! the catalog as rows.
//!
//! - User schemas first: `public` first and open, or without `public` the first user schema
//!   open, the others closed. Each lists its tables, views and materialized views, each kind
//!   with its own icon ([`crate::icons`]). A table's chevron shows its columns with their types,
//!   and a key icon on primary and foreign key columns.
//! - The system schemas (`pg_catalog`, `information_schema`, `pg_toast…`) sit in one group at the
//!   bottom, closed, and each closed inside it.
//! - The filter keeps the tables whose name or `schema.name` contains it, ignoring case; while it
//!   is typed every schema with a match shows open, the system group too.
//! - A click selects a table, a double-click opens it ([`TreeAct::Open`]); a click on a schema or
//!   the system group opens or closes it.
//!
//! **Speed.** A catalog can hold thousands of tables, and the tree must stay within a frame. The
//! rows are built once per catalog, filter and open/closed state ([`Tree`] caches them, keyed by
//! the catalog's identity, so a refresh rebuilds them), and the lowercased names the filter
//! compares once per catalog. A frame draws only the rows in view; the table and schema rows out
//! of view are listed for automation with their place ([`Tags::tag_out_of_view`]), and the tree
//! scrolls to one when a click waits for it.
//!
//! Automation ids: `tree-filter`, `tree-refresh`, `tree-system`, `tree-schema-<schema>`,
//! `tree-table-<schema>.<table>` (value: its kind), `tree-expand-<schema>.<table>` (its chevron;
//! out of view too, like the table rows) and `tree-column-<schema>.<table>.<column>` (value:
//! `{type, key}`, `key` being `primary`, `foreign` or null; drawn rows only).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dbee_core::{Catalog, RelKind, SchemaInfo, TableInfo, TableRef};
use egui::{Align, Color32, CornerRadius, FontId, Margin, Rect, ScrollArea, Sense, TextEdit, pos2, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::connections::line_galley;
use crate::icons;
use crate::theme::{RADIUS, TOKENS};

/// The height of a row.
pub const ROW_H: f32 = 22.0;
/// How far each level is indented.
const INDENT: f32 = 14.0;
/// The chevron's square, then the icon's, from a row's indented left edge.
const CHEVRON: f32 = 16.0;
const ICON: f32 = 16.0;

/// What the user asked of the tree in a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAct {
    /// A table was double-clicked: open it in a tab.
    Open(TableRef),
    /// The refresh button was clicked.
    Refresh,
}

/// What the refresh button can do now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Refresh {
    #[default]
    Ready,
    /// A refresh runs.
    Running,
    /// It cannot run, for this reason (the connection is lost).
    Disabled(String),
}

/// The tree's state: the filter, what is open, the selection, and the rows built for them.
#[derive(Default)]
pub struct Tree {
    /// The filter box's text.
    pub filter: String,
    /// Set by the workspace before each frame.
    pub refresh: Refresh,
    /// User schemas the user opened or closed, and how they are now ([`default_open`] for the
    /// others).
    user_open: HashMap<String, bool>,
    /// System schemas the user opened (they start closed).
    opened: HashSet<String>,
    system_open: bool,
    /// Tables whose columns show.
    expanded: HashSet<TableRef>,
    selected: Option<TableRef>,
    /// Bumped whenever something open or closed changes, so the rows are built again.
    shape: u64,
    rows: Option<Rows>,
    names: Option<Names>,
}

impl std::fmt::Debug for Tree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tree").field("filter", &self.filter).field("selected", &self.selected).finish_non_exhaustive()
    }
}

/// One row, by index into the catalog its [`Rows`] were built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// The system schemas' group, and how many schemas it holds.
    System {
        open: bool,
        schemas: usize,
    },
    /// A schema and how many of its tables show (all of them, or the filter's matches).
    Schema {
        s: usize,
        open: bool,
        tables: usize,
        system: bool,
    },
    Table {
        s: usize,
        t: usize,
        open: bool,
        system: bool,
    },
    Column {
        s: usize,
        t: usize,
        c: usize,
        system: bool,
    },
}

/// The rows for one catalog, filter and shape, with their automation ids.
struct Rows {
    catalog: Arc<Catalog>,
    filter: String,
    shape: u64,
    list: Vec<Row>,
    ids: Vec<String>,
    /// Each table row's expander id (`tree-expand-…`); empty for other rows.
    expanders: Vec<String>,
    /// Tables that match the filter, and all tables.
    matching: usize,
    total: usize,
}

/// The lowercased names the filter compares, per schema and table: the name and `schema.name`.
struct Names {
    catalog: Arc<Catalog>,
    lower: Vec<Vec<(String, String)>>,
}

/// A system schema: `pg_catalog`, `information_schema` (the catalog marks them) or a TOAST
/// schema.
pub fn is_system(s: &SchemaInfo) -> bool {
    s.system || s.name.starts_with("pg_toast")
}

/// The user schemas of `catalog` in the order the tree lists them: `public` first, then the
/// others in catalog order.
fn user_schemas(catalog: &Catalog) -> impl Iterator<Item = usize> + '_ {
    let users = || catalog.schemas.iter().enumerate().filter(|(_, x)| !is_system(x));
    let public = users().filter(|(_, x)| x.name == PUBLIC).map(|(s, _)| s);
    public.chain(users().filter(|(_, x)| x.name != PUBLIC).map(|(s, _)| s))
}

/// The user schema that starts open: `public`, else the first user schema listed.
fn default_open(catalog: &Catalog) -> Option<&str> {
    user_schemas(catalog).next().and_then(|s| catalog.schemas.get(s)).map(|x| x.name.as_str())
}

/// The schema Postgres creates in every database.
const PUBLIC: &str = "public";

/// The row estimate the tree shows for `t`: none for a view or a foreign table, where the
/// planner's number means nothing (it would read `~0`).
pub fn row_estimate(t: &TableInfo) -> Option<u64> {
    match t.kind {
        RelKind::Table | RelKind::PartitionedTable | RelKind::MaterializedView => t.estimated_rows,
        RelKind::View | RelKind::ForeignTable => None,
    }
}

/// `table`, `view`, … as automation values and hover text say it.
pub fn kind_name(kind: RelKind) -> &'static str {
    match kind {
        RelKind::Table => "table",
        RelKind::PartitionedTable => "partitioned table",
        RelKind::View => "view",
        RelKind::MaterializedView => "materialized view",
        RelKind::ForeignTable => "foreign table",
    }
}

/// The icon and its colour for a relation of `kind`.
pub fn kind_icon(kind: RelKind) -> (fn(&egui::Painter, Rect, Color32), Color32) {
    let t = TOKENS;
    match kind {
        RelKind::Table | RelKind::PartitionedTable => (icons::table, t.icon_table),
        RelKind::ForeignTable => (icons::table, t.icon_column),
        RelKind::View => (icons::view, t.icon_view),
        RelKind::MaterializedView => (icons::materialized_view, t.icon_matview),
    }
}

/// `1,234`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl Tree {
    /// Opens or closes schema `name` of `catalog`.
    fn toggle_schema(&mut self, catalog: &Catalog, name: &str, system: bool) {
        if system {
            if !self.opened.remove(name) {
                self.opened.insert(name.to_string());
            }
        } else {
            let open = self.user_open(catalog, name);
            self.user_open.insert(name.to_string(), !open);
        }
        self.shape = self.shape.wrapping_add(1);
    }

    /// User schema `name` is open (when no filter is typed).
    fn user_open(&self, catalog: &Catalog, name: &str) -> bool {
        self.user_open.get(name).copied().unwrap_or_else(|| default_open(catalog) == Some(name))
    }

    fn toggle_table(&mut self, t: &TableRef) {
        if !self.expanded.remove(t) {
            self.expanded.insert(t.clone());
        }
        self.shape = self.shape.wrapping_add(1);
    }

    fn toggle_system(&mut self) {
        self.system_open = !self.system_open;
        self.shape = self.shape.wrapping_add(1);
    }

    /// The lowercased names of `catalog`, built once per catalog.
    fn names(&mut self, catalog: &Arc<Catalog>) -> &Names {
        let names = match self.names.take() {
            Some(n) if Arc::ptr_eq(&n.catalog, catalog) => n,
            _ => {
                let lower = catalog
                    .schemas
                    .iter()
                    .map(|s| {
                        s.tables
                            .iter()
                            .map(|t| {
                                let name = t.table.name.to_lowercase();
                                let qualified = format!("{}.{name}", s.name.to_lowercase());
                                (name, qualified)
                            })
                            .collect()
                    })
                    .collect();
                Names { catalog: Arc::clone(catalog), lower }
            }
        };
        self.names.insert(names)
    }

    /// The rows for `catalog` and the current filter and shape, built when one of them changed.
    fn rows(&mut self, catalog: &Arc<Catalog>) -> &Rows {
        let rows = match self.rows.take() {
            Some(r) if Arc::ptr_eq(&r.catalog, catalog) && r.filter == self.filter && r.shape == self.shape => r,
            _ => self.build(catalog),
        };
        self.rows.insert(rows)
    }

    fn build(&mut self, catalog: &Arc<Catalog>) -> Rows {
        let needle = self.filter.trim().to_lowercase();
        let filtering = !needle.is_empty();
        // Which tables of each schema show.
        let shown: Vec<Vec<usize>> = self
            .names(catalog)
            .lower
            .iter()
            .map(|tables| {
                tables
                    .iter()
                    .enumerate()
                    .filter(|(_, (name, qualified))| !filtering || name.contains(&needle) || qualified.contains(&needle))
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        let mut rows = Rows {
            catalog: Arc::clone(catalog),
            filter: self.filter.clone(),
            shape: self.shape,
            list: Vec::new(),
            ids: Vec::new(),
            expanders: Vec::new(),
            matching: shown.iter().map(Vec::len).sum(),
            total: catalog.table_count(),
        };
        let visible = |s: usize| !filtering || shown.get(s).is_some_and(|t| !t.is_empty());
        for s in user_schemas(catalog) {
            let Some(schema) = catalog.schemas.get(s) else { continue };
            if visible(s) {
                let open = filtering || self.user_open(catalog, &schema.name);
                self.schema_rows(&mut rows, catalog, &shown, s, false, open);
            }
        }
        let system: Vec<usize> = catalog.schemas.iter().enumerate().filter(|(s, x)| is_system(x) && visible(*s)).map(|(s, _)| s).collect();
        if !system.is_empty() {
            let open = filtering || self.system_open;
            rows.push(Row::System { open, schemas: system.len() }, "tree-system".into());
            if open {
                for s in system {
                    let open = filtering || catalog.schemas.get(s).is_some_and(|x| self.opened.contains(&x.name));
                    self.schema_rows(&mut rows, catalog, &shown, s, true, open);
                }
            }
        }
        rows
    }

    /// Schema `s`'s row, and when it is `open` the rows of its `shown` tables (and their columns).
    fn schema_rows(&self, rows: &mut Rows, catalog: &Catalog, shown: &[Vec<usize>], s: usize, system: bool, open: bool) {
        let (Some(schema), Some(tables)) = (catalog.schemas.get(s), shown.get(s)) else { return };
        rows.push(Row::Schema { s, open, tables: tables.len(), system }, format!("tree-schema-{}", schema.name));
        if !open {
            return;
        }
        for &t in tables {
            let Some(info) = schema.tables.get(t) else { continue };
            let expanded = self.expanded.contains(&info.table);
            rows.push(Row::Table { s, t, open: expanded, system }, format!("tree-table-{}.{}", schema.name, info.table.name));
            if expanded {
                for (c, col) in info.columns.iter().enumerate() {
                    rows.push(Row::Column { s, t, c, system }, format!("tree-column-{}.{}.{}", schema.name, info.table.name, col.name));
                }
            }
        }
    }
}

impl Rows {
    fn push(&mut self, row: Row, id: String) {
        let expander = match row {
            Row::Table { .. } => format!("tree-expand-{}", id.trim_start_matches("tree-table-")),
            _ => String::new(),
        };
        self.list.push(row);
        self.ids.push(id);
        self.expanders.push(expander);
    }
}

/// What a click on a drawn row did.
enum Click {
    None,
    ToggleSystem,
    ToggleSchema(usize, bool),
    ToggleTable(usize, usize),
    Select(usize, usize),
    Open(usize, usize),
}

/// Draws the tree for `catalog` into `ui` (the filter, the refresh button, the count and the
/// rows) and returns what the user asked for.
pub fn show(ui: &mut egui::Ui, catalog: &Arc<Catalog>, tree: &mut Tree, tags: &mut Tags) -> Vec<TreeAct> {
    let t = TOKENS;
    let mut acts = Vec::new();
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 34.0).max(60.0);
        let r = ui.add(
            TextEdit::singleline(&mut tree.filter)
                .id(egui::Id::new("tree-filter"))
                .hint_text("Filter tables")
                .desired_width(width)
                .margin(Margin::symmetric(6, 4)),
        );
        tags.tag_value("tree-filter", &r, tree.filter.as_str());
        let enabled = tree.refresh == Refresh::Ready;
        let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
        let fill = if enabled && r.hovered() { t.hover } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), fill);
        let icon_rect = Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
        match &tree.refresh {
            Refresh::Running => {
                egui::Spinner::new().size(14.0).color(t.text_muted).paint_at(ui, icon_rect);
            }
            Refresh::Ready => icons::refresh(ui.painter(), icon_rect, t.text_muted),
            Refresh::Disabled(_) => icons::refresh(ui.painter(), icon_rect, t.border),
        }
        let r = match &tree.refresh {
            Refresh::Ready => r.on_hover_text("Refresh the schema"),
            Refresh::Running => r.on_hover_text("Refreshing the schema…"),
            Refresh::Disabled(why) => r.on_hover_text(format!("Cannot refresh: {why}")),
        };
        tags.tag_value_as("tree-refresh", &r, "Refresh", enabled);
        if enabled && r.clicked() {
            acts.push(TreeAct::Refresh);
        }
    });
    let filtering = !tree.filter.trim().is_empty();
    let (matching, total) = {
        let rows = tree.rows(catalog);
        (rows.matching, rows.total)
    };
    let count = if filtering {
        format!("{} of {} tables", thousands(matching as u64), thousands(total as u64))
    } else {
        format!("{} tables", thousands(total as u64))
    };
    ui.add_space(2.0);
    ui.label(egui::RichText::new(count).size(11.5).color(t.text_muted));
    ui.add_space(2.0);
    let mut click = Click::None;
    let selected = tree.selected.clone();
    let pending = tags.scroll_pending().map(str::to_owned);
    let rows = tree.rows(catalog);
    if rows.list.is_empty() {
        let text = if filtering { "No table matches the filter." } else { "This database has no schemas dbee can show." };
        ui.label(egui::RichText::new(text).color(t.text_muted));
        return acts;
    }
    ScrollArea::vertical().id_salt("tree-rows").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let frame = ui.max_rect();
        let height = ROW_H * rows.list.len() as f32;
        ui.set_min_height(height);
        let first = (viewport.min.y / ROW_H).floor().max(0.0) as usize;
        let last = ((viewport.max.y / ROW_H).ceil().max(0.0) as usize).saturating_add(1);
        for (i, ((row, id), expander)) in rows.list.iter().zip(&rows.ids).zip(&rows.expanders).enumerate() {
            let rect = Rect::from_min_size(pos2(frame.left(), frame.top() + ROW_H * i as f32), vec2(frame.width(), ROW_H));
            if i >= first && i < last {
                let c = draw_row(ui, rect, *row, id, expander, &rows.catalog, selected.as_ref(), tags);
                if !matches!(c, Click::None) {
                    click = c;
                }
            } else if !matches!(row, Row::Column { .. }) {
                tags.tag_out_of_view(id, rect);
                if !expander.is_empty() {
                    tags.tag_out_of_view(expander, Geometry::of(rect, *row).chevron);
                }
                let wanted = pending.as_deref();
                if wanted == Some(id.as_str()) || (!expander.is_empty() && wanted == Some(expander.as_str())) {
                    ui.scroll_to_rect(rect, Some(Align::Center));
                }
            }
        }
    });
    let table_ref = |s: usize, t: usize| catalog.schemas.get(s).and_then(|x| x.tables.get(t)).map(|x| x.table.clone());
    match click {
        Click::None => {}
        Click::ToggleSystem => tree.toggle_system(),
        Click::ToggleSchema(s, system) => {
            if let Some(name) = catalog.schemas.get(s).map(|x| x.name.clone()) {
                tree.toggle_schema(catalog, &name, system);
            }
        }
        Click::ToggleTable(s, t) => {
            if let Some(r) = table_ref(s, t) {
                tree.toggle_table(&r);
            }
        }
        Click::Select(s, t) => tree.selected = table_ref(s, t),
        Click::Open(s, t) => {
            if let Some(r) = table_ref(s, t) {
                tree.selected = Some(r.clone());
                acts.push(TreeAct::Open(r));
            }
        }
    }
    acts
}

/// Where a row's parts go.
struct Geometry {
    chevron: Rect,
    icon: Rect,
    text_left: f32,
}

impl Geometry {
    /// The parts of `row` drawn in `rect`: indented by its level.
    fn of(rect: Rect, row: Row) -> Geometry {
        let level = match row {
            Row::System { .. } => 0.0,
            Row::Schema { system, .. } => f32::from(u8::from(system)),
            Row::Table { system, .. } => 1.0 + f32::from(u8::from(system)),
            Row::Column { system, .. } => 2.0 + f32::from(u8::from(system)),
        };
        let left = rect.left() + 4.0 + INDENT * level;
        let chevron = Rect::from_min_size(pos2(left, rect.top() + (ROW_H - CHEVRON) / 2.0), vec2(CHEVRON, CHEVRON));
        let icon = Rect::from_min_size(pos2(left + CHEVRON + 2.0, rect.top() + (ROW_H - ICON) / 2.0), vec2(ICON, ICON));
        Geometry { chevron, icon, text_left: icon.right() + 6.0 }
    }
}

/// Draws one row in view and tags it (a table row's chevron as `expander`).
#[allow(clippy::too_many_arguments, reason = "one row's data, borrowed from the tree's cache and the frame")]
fn draw_row(
    ui: &mut egui::Ui,
    rect: Rect,
    row: Row,
    id: &str,
    expander: &str,
    catalog: &Catalog,
    selected: Option<&TableRef>,
    tags: &mut Tags,
) -> Click {
    let t = TOKENS;
    let Geometry { chevron: chevron_rect, icon: icon_rect, text_left } = Geometry::of(rect, row);
    let r = ui.interact(rect, egui::Id::new(("dbee-tree", id)), Sense::click());
    let is_selected = match row {
        Row::Table { s, t, .. } => catalog.schemas.get(s).and_then(|x| x.tables.get(t)).is_some_and(|x| Some(&x.table) == selected),
        _ => false,
    };
    let fill = if is_selected {
        t.selection
    } else if r.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter().clone();
    p.rect_filled(rect.shrink2(vec2(2.0, 0.0)), CornerRadius::same(RADIUS), fill);
    let text_w = |right: f32| (right - text_left).max(1.0);
    let font = FontId::proportional(13.0);
    let label = |text: &str, color: Color32, right: f32| {
        let g = line_galley(ui, text, font.clone(), color, text_w(right));
        p.galley(pos2(text_left, rect.center().y - g.size().y / 2.0), g, color);
    };
    let muted_right = |text: &str| -> f32 {
        let g = line_galley(ui, text, FontId::proportional(11.5), t.text_muted, (rect.width() * 0.45).max(1.0));
        let x = rect.right() - 8.0 - g.size().x;
        let w = g.size().x;
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, t.text_muted);
        rect.right() - 14.0 - w
    };
    match row {
        Row::System { open, schemas } => {
            icons::chevron(&p, chevron_rect, t.text_muted, open);
            icons::schema(&p, icon_rect, t.text_muted);
            let right = muted_right(&schemas.to_string());
            label("System schemas", t.text_muted, right);
            tags.tag_value(id, &r, json!({ "open": open }));
            if r.clicked() {
                return Click::ToggleSystem;
            }
        }
        Row::Schema { s, open, tables, system } => {
            let name = catalog.schemas.get(s).map_or("", |x| x.name.as_str());
            icons::chevron(&p, chevron_rect, t.text_muted, open);
            icons::schema(&p, icon_rect, t.icon_schema);
            let right = muted_right(&thousands(tables as u64));
            label(name, t.text, right);
            tags.tag_value(id, &r, json!({ "open": open, "tables": tables }));
            if r.clicked() {
                return Click::ToggleSchema(s, system);
            }
        }
        Row::Table { s, t: ti, open, .. } => {
            let Some(info) = catalog.schemas.get(s).and_then(|x| x.tables.get(ti)) else { return Click::None };
            icons::chevron(&p, chevron_rect, t.text_muted, open);
            let (icon, color) = kind_icon(info.kind);
            icon(&p, icon_rect, color);
            label(&info.table.name, t.text, rect.right() - 8.0);
            let mut hover = format!("{} · {}", info.table.display(), kind_name(info.kind));
            if let Some(n) = row_estimate(info) {
                hover.push_str(&format!(" · ~{} rows", thousands(n)));
            }
            let r = r.on_hover_text(hover);
            tags.tag_value(id, &r, kind_name(info.kind));
            // The chevron is its own target, above the row.
            let expand = ui.interact(chevron_rect.expand(2.0), egui::Id::new(("dbee-tree-expand", id)), Sense::click());
            tags.tag_value(expander, &expand, open);
            if expand.clicked() {
                return Click::ToggleTable(s, ti);
            }
            if r.double_clicked() {
                return Click::Open(s, ti);
            }
            if r.clicked() {
                return Click::Select(s, ti);
            }
        }
        Row::Column { s, t: ti, c, .. } => {
            let Some(info) = catalog.schemas.get(s).and_then(|x| x.tables.get(ti)) else { return Click::None };
            let Some(col) = info.columns.get(c) else { return Click::None };
            let key = if info.primary_key.contains(&col.name) {
                Some("primary")
            } else if info.fk_for(&col.name).is_some() {
                Some("foreign")
            } else {
                None
            };
            match key {
                Some("primary") => icons::key_primary(&p, icon_rect, t.key_primary),
                Some(_) => icons::key_foreign(&p, icon_rect, t.key_foreign),
                None => icons::column(&p, icon_rect, t.icon_column),
            }
            let right = muted_right(&col.type_sql);
            label(&col.name, t.text, right);
            let r = match info.fk_for(&col.name) {
                Some(fk) => r.on_hover_text(format!("{} → {}", col.name, fk.target.display())),
                None => r,
            };
            tags.tag_value(id, &r, json!({ "type": col.type_sql, "key": key }));
        }
    }
    Click::None
}

#[cfg(test)]
mod tests {
    use dbee_core::{ColumnInfo, TypeClass};

    use super::*;

    fn table(schema: &str, name: &str, kind: RelKind) -> TableInfo {
        let col = ColumnInfo {
            name: "id".into(),
            type_sql: "integer".into(),
            base_type: "int4".into(),
            class: TypeClass::Number,
            not_null: true,
            has_default: false,
            generated: Default::default(),
            hidden: false,
        };
        TableInfo {
            table: TableRef { schema: schema.into(), name: name.into() },
            kind,
            columns: vec![col],
            primary_key: vec!["id".into()],
            unique_keys: vec![],
            foreign_keys: vec![],
            estimated_rows: Some(0),
            no_edit_reason: None,
        }
    }

    fn catalog() -> Arc<Catalog> {
        let schema = |name: &str, system: bool, tables: Vec<TableInfo>| SchemaInfo { name: name.into(), system, tables };
        Arc::new(Catalog {
            schemas: vec![
                schema("shop", false, vec![table("shop", "orders", RelKind::Table), table("shop", "order_totals", RelKind::View)]),
                schema("audit", false, vec![table("audit", "log", RelKind::Table)]),
                schema("information_schema", true, vec![table("information_schema", "tables", RelKind::View)]),
                schema("pg_catalog", true, vec![table("pg_catalog", "pg_class", RelKind::Table)]),
                schema("pg_toast_temp", false, vec![]),
            ],
        })
    }

    fn ids(tree: &mut Tree, catalog: &Arc<Catalog>) -> Vec<String> {
        tree.rows(catalog).ids.clone()
    }

    #[test]
    fn system_schemas_sit_closed_in_one_group_at_the_bottom() {
        let c = catalog();
        let mut tree = Tree::default();
        // Without `public`, the first user schema starts open and the others closed.
        assert_eq!(
            ids(&mut tree, &c),
            ["tree-schema-shop", "tree-table-shop.orders", "tree-table-shop.order_totals", "tree-schema-audit", "tree-system"]
        );
        tree.toggle_system();
        let rows = ids(&mut tree, &c);
        assert_eq!(&rows[4..], ["tree-system", "tree-schema-information_schema", "tree-schema-pg_catalog", "tree-schema-pg_toast_temp"]);
        tree.toggle_schema(&c, "pg_catalog", true);
        assert!(ids(&mut tree, &c).contains(&"tree-table-pg_catalog.pg_class".to_string()));
        tree.toggle_schema(&c, "shop", false);
        assert!(!ids(&mut tree, &c).contains(&"tree-table-shop.orders".to_string()), "a closed user schema hides its tables");
        tree.toggle_schema(&c, "audit", false);
        assert!(ids(&mut tree, &c).contains(&"tree-table-audit.log".to_string()), "a closed user schema opens on a click");
    }

    #[test]
    fn public_is_listed_first_and_starts_open() {
        let mut c = (*catalog()).clone();
        c.schemas.insert(1, SchemaInfo { name: "public".into(), system: false, tables: vec![table("public", "notes", RelKind::Table)] });
        let c = Arc::new(c);
        let mut tree = Tree::default();
        assert_eq!(
            ids(&mut tree, &c),
            ["tree-schema-public", "tree-table-public.notes", "tree-schema-shop", "tree-schema-audit", "tree-system"],
            "public first and open, the other user schemas closed"
        );
        // A typed filter opens every schema with a match.
        tree.filter = "o".into();
        let rows = ids(&mut tree, &c);
        for id in ["tree-table-public.notes", "tree-table-shop.orders", "tree-table-audit.log"] {
            assert!(rows.contains(&id.to_string()), "{id}: {rows:?}");
        }
        tree.filter.clear();
        tree.toggle_schema(&c, "public", false);
        assert_eq!(ids(&mut tree, &c), ["tree-schema-public", "tree-schema-shop", "tree-schema-audit", "tree-system"]);
    }

    #[test]
    fn the_filter_keeps_matches_and_opens_what_holds_them() {
        let c = catalog();
        let mut tree = Tree { filter: "ORDER".into(), ..Tree::default() };
        assert_eq!(ids(&mut tree, &c), ["tree-schema-shop", "tree-table-shop.orders", "tree-table-shop.order_totals"]);
        assert_eq!((tree.rows(&c).matching, tree.rows(&c).total), (2, 5));
        tree.filter = "pg_cl".into();
        assert_eq!(
            ids(&mut tree, &c),
            ["tree-system", "tree-schema-pg_catalog", "tree-table-pg_catalog.pg_class"],
            "the system group opens for a match"
        );
        tree.filter = "audit.".into();
        assert_eq!(ids(&mut tree, &c), ["tree-schema-audit", "tree-table-audit.log"], "`schema.name` matches too");
        tree.filter = "zzz".into();
        assert!(ids(&mut tree, &c).is_empty());
    }

    #[test]
    fn rows_are_built_once_per_catalog_filter_and_shape() {
        let c = catalog();
        let mut tree = Tree::default();
        let first = tree.rows(&c).list.as_ptr();
        assert_eq!(tree.rows(&c).list.as_ptr(), first, "nothing changed: the same rows");
        tree.toggle_table(&TableRef { schema: "shop".into(), name: "orders".into() });
        assert!(ids(&mut tree, &c).contains(&"tree-column-shop.orders.id".to_string()), "an expanded table lists its columns");
        // Another catalog (a refresh) builds them again, even with equal contents.
        let other = Arc::new((*c).clone());
        let names = tree.names(&other).catalog.clone();
        assert!(Arc::ptr_eq(&names, &other));
        assert!(Arc::ptr_eq(&tree.rows(&other).catalog, &other));
    }

    #[test]
    fn views_never_show_an_estimate() {
        assert_eq!(row_estimate(&table("s", "v", RelKind::View)), None);
        assert_eq!(row_estimate(&table("s", "f", RelKind::ForeignTable)), None);
        assert_eq!(row_estimate(&table("s", "t", RelKind::Table)), Some(0));
        assert_eq!(row_estimate(&table("s", "m", RelKind::MaterializedView)), Some(0));
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}

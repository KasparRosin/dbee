//! The schema tree, on the workspace's left ([`show`]): a filter box and a refresh button, then
//! the catalog as rows.
//!
//! - User schemas first: the catalog's default schema (Postgres `public`, the database a MySQL
//!   connection names, SQLite `main`) first and open, or without one the first user schema open,
//!   the others closed, in the catalog's order. Each lists its tables, views and materialized
//!   views, each kind with its own icon. A table's chevron shows its visible columns with their
//!   types, and a key icon on primary and foreign key columns. Each row is drawn by
//!   [`TreeRow`].
//! - The system schemas, those the catalog marks (Postgres's `pg_catalog` and
//!   `information_schema`, MySQL's `mysql`, `sys`, …), sit in one group at the bottom, closed, and
//!   each closed inside it.
//! - The texts name schemas with the engine's word (`Dialect::schema_word`): on MySQL the group
//!   reads "System databases".
//! - The filter keeps the tables whose name or `schema.name` contains it, ignoring case; while it
//!   is typed every schema with a match shows open, the system group too. The count over the rows
//!   says how many tables match (`3 OF 516 TABLES`); with no match it reads `0 TABLES` over an
//!   empty tree. A catalog with nothing to show says so under the count.
//! - A schema's, the system group's or a column's name or type cut with `…` reads whole on hover
//!   (a foreign key's hover names its target after them); a table's hover names it whole already.
//! - A click selects a table, a double-click opens it ([`TreeAct::Open`]); a click on a schema
//!   selects it and opens or closes it; a click on the system group opens or closes it. A new
//!   query tab opens on the selection's schema ([`Tree::selected_schema`]). A selection the
//!   catalog no longer holds (after a refresh or a reconnect) is forgotten.
//!
//! **Right-click menu.** A secondary click on a table, view or materialized view row opens its
//! menu (the `menu` component): Open (`Enter`, [`TreeAct::Open`]), New query (`Ctrl+T`,
//! [`TreeAct::NewQuery`], on the row's schema), a separator, Copy name (`Ctrl+C`,
//! [`TreeAct::CopyName`]). Each act carries the row's own [`TableRef`], never an index. An item
//! the workspace says can't run ([`MenuState`]) is greyed, with `<Label>: not now, <why>` on
//! hover, and a click on it runs nothing. The right-click changes neither the selection nor what
//! is open; the row keeps the `hover` ground while its menu is open, and the menu closes when its
//! row is no longer drawn. Schema rows, the system group and column rows have no menu.
//!
//! **Keys.** Enter on a table row that has the keyboard opens it, as a double-click does (and
//! selects it); Space stays a click (select). Ctrl+C copies a table's name ([`take_keys`], before
//! the shortcuts): the open menu's row while a menu is open, else the table row that has the
//! keyboard. Anywhere else Ctrl+C is the shortcuts' (the grid's copy), and Ctrl+Shift+C always is.
//! No key opens a row's menu yet.
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
//! `{type, key}`, `key` being `primary`, `foreign` or null; drawn rows only); while a row's menu is
//! open, `tree-menu-open`, `tree-menu-new-query` and `tree-menu-copy-name` (value
//! `{label, shortcut, why?}`); `tree-count` (the count's words as written: `516 tables`,
//! `3 of 516 tables`, `0 tables`, `Loading`) and, when the catalog has nothing to show, `tree-empty` (its
//! sentence).
//!
//! **A refresh** (LC4) changes nothing before it has run 300 ms ([`crate::quiet`]; the workspace
//! asks the clock). From then the count reads `LOADING`, whatever the filter holds, and the rows
//! are drawn at [`theme::DIM`] (`tree-dimmed`, value `true`, the rows' area): a look only, they
//! scroll, select, open and take a right-click as before. With no rows to show (nothing in the
//! catalog, a filter with no match) eight rows of bars stand in their place
//! ([`skeleton::TreeRows`], `tree-skeleton`, value the rows drawn) and the empty catalog's sentence
//! waits. The refresh beside the filter keeps its plain icon.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use dbee_core::{Catalog, RelKind, SchemaInfo, TableInfo, TableRef};
use dbee_engine::Screen;
use dbee_sql::Dialect;
use egui::{Align, Label, Modifiers, Rect, Response, ScrollArea, Sense, pos2, vec2};
use serde_json::json;

use crate::DbeeApp;
use crate::automation::Tags;
use crate::components::grid_cell::Key;
use crate::components::menu::{self, Item};
use crate::components::rows::{TreeKind, TreeRow};
use crate::components::sidebar::{self, Count};
use crate::components::skeleton;
use crate::components::tooltip::Hint;
use crate::components::{Look, focus};
use crate::shortcuts;
use crate::theme;
use crate::theme::Text;
use crate::theme::size::{SIDEBAR_PAD_X, TABLE_EMPTY_PAD, TREE_ROW_H};

/// What the user asked of the tree in a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAct {
    /// A table was double-clicked, or Open chosen in its menu: open it in a tab.
    Open(TableRef),
    /// New query chosen in a table's menu: a query tab on its schema.
    NewQuery(TableRef),
    /// Copy name chosen in a table's menu: its name on the clipboard.
    CopyName(TableRef),
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

/// Why each item of a table row's menu can't run now; `None` runs. Set by the workspace while a
/// menu is open ([`Tree::menu_open`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    pub open: Option<String>,
    pub new_query: Option<String>,
    pub copy_name: Option<String>,
}

/// The tree's state: the filter, what is open, the selection, and the rows built for them.
#[derive(Default)]
pub struct Tree {
    /// The filter box's text.
    pub filter: String,
    /// Set by the workspace before each frame.
    pub refresh: Refresh,
    /// What the menu's items can do; set by the workspace while a menu is open.
    pub menu: MenuState,
    /// The table whose menu showed in the last frame.
    menu_row: Option<TableRef>,
    /// The table row that had the keyboard when the tree was last drawn, and its widget id.
    focused: Option<(egui::Id, TableRef)>,
    /// User schemas the user opened or closed, and how they are now ([`default_open`] for the
    /// others).
    user_open: HashMap<String, bool>,
    /// System schemas the user opened (they start closed).
    opened: HashSet<String>,
    system_open: bool,
    /// Tables whose columns show.
    expanded: HashSet<TableRef>,
    selected: Option<Selected>,
    /// Bumped whenever something open or closed changes, so the rows are built again.
    shape: u64,
    rows: Option<Rows>,
    names: Option<Names>,
    /// Drawn once: until then, as a new tree for another connection, it starts at its top.
    drawn: bool,
}

/// What is selected in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selected {
    Table(TableRef),
    Schema(String),
}

impl Selected {
    /// `catalog` still holds it.
    fn is_in(&self, catalog: &Catalog) -> bool {
        match self {
            Selected::Table(t) => catalog.table(t).is_some(),
            Selected::Schema(name) => catalog.schemas.iter().any(|s| s.name == *name),
        }
    }
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

/// The user schemas of `catalog` in the order the tree lists them: the default schema first,
/// then the others in catalog order.
fn user_schemas(catalog: &Catalog) -> impl Iterator<Item = usize> + '_ {
    let first = |x: &SchemaInfo| catalog.default_schema.as_deref() == Some(x.name.as_str());
    let users = move |default: bool| catalog.schemas.iter().enumerate().filter(move |(_, x)| !x.system && first(x) == default);
    users(true).chain(users(false)).map(|(s, _)| s)
}

/// The user schema that starts open: the default schema, else the first user schema listed.
fn default_open(catalog: &Catalog) -> Option<&str> {
    user_schemas(catalog).next().and_then(|s| catalog.schemas.get(s)).map(|x| x.name.as_str())
}

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
    /// A table row's menu showed in the last frame.
    pub fn menu_open(&self) -> bool {
        self.menu_row.is_some()
    }

    /// The schema of what is selected, while `catalog` holds it: the selected schema, or the
    /// selected table's.
    pub fn selected_schema(&self, catalog: &Catalog) -> Option<&str> {
        match self.selected.as_ref().filter(|s| s.is_in(catalog))? {
            Selected::Table(t) => Some(t.schema.as_str()),
            Selected::Schema(s) => Some(s.as_str()),
        }
    }

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
            Some(r) if Arc::ptr_eq(&r.catalog, catalog) => self.build(catalog),
            _ => {
                // Another catalog: a selection it no longer holds goes.
                if self.selected.as_ref().is_some_and(|s| !s.is_in(catalog)) {
                    self.selected = None;
                }
                self.build(catalog)
            }
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
        let system: Vec<usize> = catalog.schemas.iter().enumerate().filter(|(s, x)| x.system && visible(*s)).map(|(s, _)| s).collect();
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
                for (c, col) in info.visible_columns().enumerate() {
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

/// Draws the tree for `catalog`, read in `dialect`, into `ui` (the filter, the refresh button,
/// the count and the rows) and returns what the user asked for. `loading`: how long a refresh has
/// run, once it has run 300 ms.
pub fn show(
    ui: &mut egui::Ui,
    catalog: &Arc<Catalog>,
    dialect: Dialect,
    tree: &mut Tree,
    loading: Option<Duration>,
    tags: &mut Tags,
) -> Vec<TreeAct> {
    let t = theme::tokens(ui.ctx());
    let mut acts = Vec::new();
    let many = dialect.schema_word().1.to_lowercase();
    // The header (`sidebar/filter`, `sidebar/count`), the rows straight under it.
    ui.spacing_mut().item_spacing.y = 0.0;
    let refresh = match tree.refresh {
        Refresh::Ready => sidebar::Refresh::Idle,
        Refresh::Running => sidebar::Refresh::Running,
        Refresh::Disabled(_) => sidebar::Refresh::Off,
    };
    let shown = sidebar::Filter::new(&mut tree.filter, "Filter tables").id(egui::Id::new("tree-filter")).refresh(refresh).show(ui);
    tags.tag_value("tree-filter", &shown.field, tree.filter.as_str());
    if let Some(r) = shown.refresh {
        let r = match &tree.refresh {
            Refresh::Ready => r.hint(format!("Refresh the {many}")),
            Refresh::Running => r.hint(format!("Refreshing the {many}…")),
            Refresh::Disabled(why) => r.hint_disabled(format!("Cannot refresh: {why}")),
        };
        tags.tag_value_as("tree-refresh", &r, "Refresh", tree.refresh == Refresh::Ready);
    }
    if shown.asked {
        acts.push(TreeAct::Refresh);
    }
    let filtering = !tree.filter.trim().is_empty();
    let (matching, total) = {
        let rows = tree.rows(catalog);
        (rows.matching as u64, rows.total as u64)
    };
    let word = ["table", "tables"];
    // A filter with no match reads `0 TABLES` over an empty tree (E3), not `0 OF 516 TABLES`.
    let count = if loading.is_some() {
        Count::Loading
    } else if !filtering {
        Count::All(total, word)
    } else if matching == 0 {
        Count::All(0, word)
    } else {
        Count::Some(matching, total, word)
    };
    let r = count.show(ui);
    tags.tag_value("tree-count", &r, count.words());
    let mut click = Click::None;
    let selected = tree.selected.clone();
    let pending = tags.scroll_pending().map(str::to_owned);
    let fresh = !tree.drawn;
    let mut menu = Menu { state: tree.menu.clone(), last: tree.menu_row.take(), now: None, acts: Vec::new() };
    tree.focused = None;
    let mut focused = None;
    let rows = tree.rows(catalog);
    if rows.list.is_empty() && loading.is_some() {
        let (rect, drawn) = skeleton::TreeRows.show(ui);
        let r = ui.interact(rect, ui.id().with("dbee-tree-skeleton"), Sense::hover());
        tags.tag_value("tree-skeleton", &r, drawn);
        return acts;
    }
    if rows.list.is_empty() {
        // A filter with no match: the count says it, no phrase (E3). A catalog with nothing to
        // show: its sentence 12 in and 12 under the count, as the table tab's empty words sit.
        if !filtering {
            let text = format!("This connection has no {many} dbee can show.");
            let inset = egui::Margin { left: SIDEBAR_PAD_X as i8, right: SIDEBAR_PAD_X as i8, top: TABLE_EMPTY_PAD as i8, bottom: 0 };
            let shown = egui::Frame::NONE
                .inner_margin(inset)
                .show(ui, |ui| ui.add(Label::new(Text::Body.rich_as_written(text.as_str()).color(t.text_muted)).wrap()));
            tags.tag_value("tree-empty", &shown.inner, text);
        }
        return acts;
    }
    let system_label = format!("System {many}");
    if loading.is_some() {
        // Before the rows, so they stay on top for the pointer.
        let r = ui.interact(ui.available_rect_before_wrap(), ui.id().with("dbee-tree-dimmed"), Sense::hover());
        tags.tag_value("tree-dimmed", &r, true);
    }
    let mut area = ScrollArea::vertical().id_salt("tree-rows").auto_shrink([false, false]);
    if fresh {
        area = area.vertical_scroll_offset(0.0);
    }
    area.show_viewport(ui, |ui, viewport| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if loading.is_some() {
            ui.multiply_opacity(theme::DIM);
        }
        let frame = ui.max_rect();
        let height = TREE_ROW_H * rows.list.len() as f32;
        ui.set_min_height(height);
        let first = (viewport.min.y / TREE_ROW_H).floor().max(0.0) as usize;
        let last = ((viewport.max.y / TREE_ROW_H).ceil().max(0.0) as usize).saturating_add(1);
        for (i, ((row, id), expander)) in rows.list.iter().zip(&rows.ids).zip(&rows.expanders).enumerate() {
            let rect = Rect::from_min_size(pos2(frame.left(), frame.top() + TREE_ROW_H * i as f32), vec2(frame.width(), TREE_ROW_H));
            if i >= first && i < last {
                let c = draw_row(ui, rect, *row, id, expander, &rows.catalog, &system_label, selected.as_ref(), &mut menu, &mut focused, tags);
                if !matches!(c, Click::None) {
                    click = c;
                }
            } else if !matches!(row, Row::Column { .. }) {
                tags.tag_out_of_view(id, rect);
                if !expander.is_empty() {
                    tags.tag_out_of_view(expander, TreeRow::expander(rect, level(*row)));
                }
                let wanted = pending.as_deref();
                if wanted == Some(id.as_str()) || (!expander.is_empty() && wanted == Some(expander.as_str())) {
                    ui.scroll_to_rect(rect, Some(Align::Center));
                }
            }
        }
    });
    tree.drawn = true;
    tree.menu_row = menu.now;
    tree.focused = focused;
    acts.extend(menu.acts);
    let table_ref = |s: usize, t: usize| catalog.schemas.get(s).and_then(|x| x.tables.get(t)).map(|x| x.table.clone());
    match click {
        Click::None => {}
        Click::ToggleSystem => tree.toggle_system(),
        Click::ToggleSchema(s, system) => {
            if let Some(name) = catalog.schemas.get(s).map(|x| x.name.clone()) {
                tree.toggle_schema(catalog, &name, system);
                tree.selected = Some(Selected::Schema(name));
            }
        }
        Click::ToggleTable(s, t) => {
            if let Some(r) = table_ref(s, t) {
                tree.toggle_table(&r);
            }
        }
        Click::Select(s, t) => tree.selected = table_ref(s, t).map(Selected::Table),
        Click::Open(s, t) => {
            if let Some(r) = table_ref(s, t) {
                tree.selected = Some(Selected::Table(r.clone()));
                acts.push(TreeAct::Open(r));
            }
        }
    }
    acts
}

/// How deep `row` sits: a system schema and what it holds one level deeper than a user one's.
fn level(row: Row) -> usize {
    match row {
        Row::System { .. } => 0,
        Row::Schema { system, .. } => usize::from(system),
        Row::Table { system, .. } => 1 + usize::from(system),
        Row::Column { system, .. } => 2 + usize::from(system),
    }
}

/// A table row's menu across a frame's rows: what its items can do, the table whose menu showed in
/// the last frame and in this one, and what was chosen.
struct Menu {
    state: MenuState,
    last: Option<TableRef>,
    now: Option<TableRef>,
    acts: Vec<TreeAct>,
}

impl Menu {
    /// The menu of `table`'s row, attached to its response `r`, when it is open.
    fn show(&mut self, r: &Response, table: &TableRef, tags: &mut Tags) {
        let state = &self.state;
        let mut chosen = None;
        let shown = egui::Popup::context_menu(r).style(menu::style(&r.ctx)).show(|ui| {
            let mut item = |ui: &mut egui::Ui, id: &str, label: &str, keys: &str, why: Option<&String>, act: TreeAct| {
                let mut r = ui.add_enabled(why.is_none(), Item::new(label).shortcut(keys));
                let mut value = json!({ "label": label, "shortcut": keys });
                if let Some(why) = why {
                    r = r.hint_disabled(format!("{label}: not now, {why}"));
                    value["why"] = json!(why);
                }
                // A menu's first frame reads every widget as disabled: tag what the code knows.
                tags.tag_value_as(id, &r, value, why.is_none());
                if r.clicked() {
                    chosen = Some(act);
                    ui.close();
                }
            };
            item(ui, "tree-menu-open", "Open", "Enter", state.open.as_ref(), TreeAct::Open(table.clone()));
            item(ui, "tree-menu-new-query", "New query", "Ctrl+T", state.new_query.as_ref(), TreeAct::NewQuery(table.clone()));
            menu::separator(ui);
            item(ui, "tree-menu-copy-name", "Copy name", "Ctrl+C", state.copy_name.as_ref(), TreeAct::CopyName(table.clone()));
        });
        if shown.is_some() && chosen.is_none() {
            self.now = Some(table.clone());
        }
        self.acts.extend(chosen);
    }
}

/// Draws one row in view and tags it (a table row's chevron as `expander`), with a table row's
/// menu. The system group reads `system_label`. A table row with the keyboard is put in `focused`.
#[allow(clippy::too_many_arguments, reason = "one row's data, borrowed from the tree's cache and the frame")]
fn draw_row(
    ui: &mut egui::Ui,
    rect: Rect,
    row: Row,
    id: &str,
    expander: &str,
    catalog: &Catalog,
    system_label: &str,
    selected: Option<&Selected>,
    menu: &mut Menu,
    focused: &mut Option<(egui::Id, TableRef)>,
    tags: &mut Tags,
) -> Click {
    let level = level(row);
    let r = ui.interact(rect, egui::Id::new(("dbee-tree", id)), Sense::click());
    // The chevron is its own target, above the row.
    let expand = match row {
        Row::Table { .. } => Some(ui.interact(TreeRow::expander(rect, level), egui::Id::new(("dbee-tree-expand", id)), Sense::click())),
        _ => None,
    };
    let is_selected = match (row, selected) {
        (Row::Table { s, t, .. }, Some(Selected::Table(sel))) => {
            catalog.schemas.get(s).and_then(|x| x.tables.get(t)).is_some_and(|x| x.table == *sel)
        }
        (Row::Schema { s, .. }, Some(Selected::Schema(sel))) => catalog.schemas.get(s).is_some_and(|x| x.name == *sel),
        _ => false,
    };
    let look = Look::of(ui, &r);
    // The row whose menu is open keeps the `hover` ground, the pointer in the menu or not.
    let menu_row = match row {
        Row::Table { s, t, .. } => {
            menu.last.as_ref().is_some_and(|m| catalog.schemas.get(s).and_then(|x| x.tables.get(t)).is_some_and(|x| x.table == *m))
        }
        _ => false,
    };
    let hovered = look.hovered || menu_row || expand.as_ref().is_some_and(Response::hovered);
    let chevron_focused = expand.as_ref().is_some_and(|e| focus::shown(ui, e));
    let draw = |kind: TreeKind, name: &str, right: Option<&str>, open: Option<bool>| {
        let drawn = TreeRow { level, kind, name, right, open, selected: is_selected, focused: look.focused, chevron_focused, hovered };
        let cut = drawn.paint(ui, rect);
        r.widget_info(|| drawn.widget_info());
        if let Some(e) = &expand {
            e.widget_info(|| drawn.chevron_info());
        }
        cut
    };
    match row {
        Row::System { open, .. } => {
            let cut = draw(TreeKind::System, system_label, None, Some(open));
            let r = match cut {
                Some(whole) => r.hint(whole),
                None => r,
            };
            tags.tag_value(id, &r, json!({ "open": open }));
            if r.clicked() {
                return Click::ToggleSystem;
            }
        }
        Row::Schema { s, open, tables, system } => {
            let cut = draw(TreeKind::Schema, catalog.schemas.get(s).map_or("", |x| x.name.as_str()), None, Some(open));
            let r = match cut {
                Some(whole) => r.hint(whole),
                None => r,
            };
            tags.tag_value(id, &r, json!({ "open": open, "tables": tables }));
            if r.clicked() {
                return Click::ToggleSchema(s, system);
            }
        }
        Row::Table { s, t: ti, open, .. } => {
            let Some(info) = catalog.schemas.get(s).and_then(|x| x.tables.get(ti)) else { return Click::None };
            draw(TreeKind::Relation(info.kind), &info.table.name, None, Some(open));
            let mut hover = format!("{} · {}", info.table.display(), kind_name(info.kind));
            if let Some(n) = row_estimate(info) {
                hover.push_str(&format!(" · ~{} rows", thousands(n)));
            }
            let r = r.hint(hover);
            tags.tag_value(id, &r, kind_name(info.kind));
            menu.show(&r, &info.table, tags);
            // Egui's memory, not `Response::has_focus`, which is false while the window itself
            // lacks the system's focus: `take_keys` asks the memory too.
            if ui.memory(|m| m.has_focus(r.id)) {
                *focused = Some((r.id, info.table.clone()));
                // Enter opens it (egui's click from the same Enter is then not read); not while a
                // menu is open, where Enter is the menu's.
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.is_none());
                if enter && menu.last.is_none() {
                    return Click::Open(s, ti);
                }
            }
            if let Some(expand) = expand {
                tags.tag_value(expander, &expand, open);
                if expand.clicked() {
                    return Click::ToggleTable(s, ti);
                }
            }
            if r.double_clicked() {
                return Click::Open(s, ti);
            }
            if r.clicked() {
                // A click gives the row the keyboard (no focus box for a pointer's focus), so
                // Ctrl+C copies its name from then on.
                r.request_focus();
                *focused = Some((r.id, info.table.clone()));
                return Click::Select(s, ti);
            }
        }
        Row::Column { s, t: ti, c, .. } => {
            let Some(info) = catalog.schemas.get(s).and_then(|x| x.tables.get(ti)) else { return Click::None };
            let Some(col) = info.visible_columns().nth(c) else { return Click::None };
            let key = if info.primary_key.contains(&col.name) {
                Some(Key::Primary)
            } else if info.fk_for(&col.name).is_some() {
                Some(Key::Foreign)
            } else {
                None
            };
            // Cut: the whole name and type; a foreign key names its target, after the name alone
            // when nothing is cut.
            let cut = draw(TreeKind::Column(key), &col.name, Some(col.label()), None);
            let r = match (info.fk_for(&col.name), cut) {
                (Some(fk), Some(whole)) => r.hint(format!("{whole} → {}", fk.target.display())),
                (Some(fk), None) => r.hint(format!("{} → {}", col.name, fk.target.display())),
                (None, Some(whole)) => r.hint(whole),
                (None, None) => r,
            };
            tags.tag_value(id, &r, json!({ "type": col.label(), "key": key.map(Key::id) }));
        }
    }
    Click::None
}

/// Step 4 of the frame, before the shortcuts, unless a text field or a text editor has the
/// keyboard: Ctrl+C (and the keyboard's copy) copies a table's name, the open menu's row's (the
/// menu then closes), else the table row's that has the keyboard. Ctrl+Shift+C is left to the
/// shortcuts.
pub(crate) fn take_keys(app: &mut DbeeApp, ctx: &egui::Context) {
    if app.session.state().screen != Screen::Workspace {
        return;
    }
    let tree = &app.view.work.tree;
    let target = match (&tree.menu_row, &tree.focused) {
        (Some(table), _) => Some((table.clone(), true)),
        (None, Some((id, table))) if ctx.memory(|m| m.has_focus(*id)) => Some((table.clone(), false)),
        _ => None,
    };
    let Some((table, in_menu)) = target else { return };
    let copy = ctx.input_mut(|i| {
        let held = i.modifiers;
        let mut taken = false;
        // One press can come as both a key and a copy event: take them all.
        while shortcuts::take(&mut i.events, Modifiers::COMMAND, egui::Key::C, held) {
            taken = true;
        }
        taken
    });
    if !copy {
        return;
    }
    if in_menu {
        egui::Popup::close_all(ctx);
        app.view.work.tree.menu_row = None;
    }
    app.run_from_ui("clipboard.copyName", json!({ "schema": table.schema, "table": table.name }));
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
            type_label: String::new(),
            operators: dbee_core::Operators::Unnamed,
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
            default_schema: None,
        })
    }

    fn ids(tree: &mut Tree, catalog: &Arc<Catalog>) -> Vec<String> {
        tree.rows(catalog).ids.clone()
    }

    #[test]
    fn system_schemas_sit_closed_in_one_group_at_the_bottom() {
        let c = catalog();
        let mut tree = Tree::default();
        // Without a default schema, the first user schema starts open and the others closed. The
        // catalog's flag makes a schema the system's, not its name.
        assert_eq!(
            ids(&mut tree, &c),
            [
                "tree-schema-shop",
                "tree-table-shop.orders",
                "tree-table-shop.order_totals",
                "tree-schema-audit",
                "tree-schema-pg_toast_temp",
                "tree-system"
            ]
        );
        tree.toggle_system();
        let rows = ids(&mut tree, &c);
        assert_eq!(&rows[5..], ["tree-system", "tree-schema-information_schema", "tree-schema-pg_catalog"]);
        tree.toggle_schema(&c, "pg_catalog", true);
        assert!(ids(&mut tree, &c).contains(&"tree-table-pg_catalog.pg_class".to_string()));
        tree.toggle_schema(&c, "shop", false);
        assert!(!ids(&mut tree, &c).contains(&"tree-table-shop.orders".to_string()), "a closed user schema hides its tables");
        tree.toggle_schema(&c, "audit", false);
        assert!(ids(&mut tree, &c).contains(&"tree-table-audit.log".to_string()), "a closed user schema opens on a click");
    }

    #[test]
    fn the_default_schema_is_listed_first_and_starts_open() {
        let mut c = (*catalog()).clone();
        c.schemas.insert(1, SchemaInfo { name: "public".into(), system: false, tables: vec![table("public", "notes", RelKind::Table)] });
        // Without a default schema, the catalog's order: `public` is a schema like the others.
        let plain = Arc::new(c.clone());
        let mut tree = Tree::default();
        assert_eq!(
            ids(&mut tree, &plain),
            [
                "tree-schema-shop",
                "tree-table-shop.orders",
                "tree-table-shop.order_totals",
                "tree-schema-public",
                "tree-schema-audit",
                "tree-schema-pg_toast_temp",
                "tree-system"
            ]
        );
        c.default_schema = Some("audit".into());
        let c = Arc::new(c);
        let mut tree = Tree::default();
        assert_eq!(
            ids(&mut tree, &c),
            ["tree-schema-audit", "tree-table-audit.log", "tree-schema-shop", "tree-schema-public", "tree-schema-pg_toast_temp", "tree-system"],
            "the default schema first and open, the other user schemas closed"
        );
        // A typed filter opens every schema with a match.
        tree.filter = "o".into();
        let rows = ids(&mut tree, &c);
        for id in ["tree-table-public.notes", "tree-table-shop.orders", "tree-table-audit.log"] {
            assert!(rows.contains(&id.to_string()), "{id}: {rows:?}");
        }
        tree.filter.clear();
        tree.toggle_schema(&c, "audit", false);
        assert_eq!(ids(&mut tree, &c), ["tree-schema-audit", "tree-schema-shop", "tree-schema-public", "tree-schema-pg_toast_temp", "tree-system"]);
    }

    #[test]
    fn the_selection_names_a_schema() {
        let c = catalog();
        let mut tree = Tree::default();
        assert_eq!(tree.selected_schema(&c), None);
        tree.selected = Some(Selected::Table(TableRef { schema: "shop".into(), name: "orders".into() }));
        assert_eq!(tree.selected_schema(&c), Some("shop"), "a table's schema");
        tree.selected = Some(Selected::Schema("audit".into()));
        assert_eq!(tree.selected_schema(&c), Some("audit"));
    }

    #[test]
    fn a_selection_the_catalog_lost_is_forgotten() {
        let c = catalog();
        let orders = TableRef { schema: "shop".into(), name: "orders".into() };
        let mut tree = Tree { selected: Some(Selected::Table(orders.clone())), ..Tree::default() };
        ids(&mut tree, &c);
        assert_eq!(tree.selected_schema(&c), Some("shop"));
        // A refresh without the table: asked of that catalog, the selection names nothing, and
        // once the rows are built for it, it is gone.
        let mut lost = (*c).clone();
        lost.schemas.iter_mut().for_each(|s| s.tables.retain(|t| t.table != orders));
        let lost = Arc::new(lost);
        assert_eq!(tree.selected_schema(&lost), None);
        ids(&mut tree, &lost);
        assert_eq!(tree.selected, None);
        // A schema too; one still there is kept.
        tree.selected = Some(Selected::Schema("audit".into()));
        let refreshed = Arc::new((*lost).clone());
        ids(&mut tree, &refreshed);
        assert_eq!(tree.selected_schema(&refreshed), Some("audit"));
        let mut without = (*refreshed).clone();
        without.schemas.retain(|s| s.name != "audit");
        ids(&mut tree, &Arc::new(without));
        assert_eq!(tree.selected, None);
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

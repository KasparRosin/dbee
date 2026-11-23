//! The sidebar's rows: the schema tree's (`tree/row`) and the connection list's
//! (`sidebar/connection`). Each is a view over plain data that draws into the rect its list gives
//! it; the tree ([`crate::tree`]) and the connection screen keep their layout, scrolling, input,
//! menus and tags, and say what each row is: selected, under the pointer, showing keyboard focus.
//!
//! Names keep their case: a schema, table or column name, a type and a connection's name are
//! drawn as written. A selected row doesn't change on hover. Keyboard focus is a 2 px `ink` box:
//! a tree row's along the row's own edges, as far as its hover ground reaches, a connection row's
//! inside the row.

use dbee_config::DbKind;
use dbee_core::RelKind;
use egui::text::TextWrapping;
use egui::{Color32, CornerRadius, Rect, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};

use super::grid_cell::{Key, one_line};
use super::icon::Icon;
use super::logo::Logo;
use super::status::{Address, cut_middle};
use super::{focus, painter};
use crate::theme::size::{
    CONNECTION_COLOUR_W, CONNECTION_FOCUS_INSET, CONNECTION_LOGO, CONNECTION_LOGO_GAP, CONNECTION_PAD_X, RADIUS_CONTROL, TREE_CHEVRON, TREE_EXPANDER,
    TREE_GAP, TREE_ICON, TREE_INDENT, TREE_PAD_X,
};
use crate::theme::{Text, Tokens, Words, tokens};

/// What a tree row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Schema,
    /// The system schemas' group.
    System,
    /// A table, partitioned table, foreign table, view or materialized view.
    Relation(RelKind),
    /// A column, with its key when it is one.
    Column(Option<Key>),
}

impl TreeKind {
    /// The icon in its slot, in its colour; none for a column that is no key.
    fn icon(self, t: &Tokens) -> Option<(Icon, Color32)> {
        match self {
            TreeKind::Schema | TreeKind::System => Some((Icon::Schema, t.text_muted)),
            TreeKind::Relation(kind) => Some((relation_icon(kind), t.text_muted)),
            TreeKind::Column(Some(Key::Primary)) => Some((Icon::KeyPrimary, t.key)),
            TreeKind::Column(Some(Key::Foreign)) => Some((Icon::KeyForeign, t.text_muted)),
            TreeKind::Column(None) => None,
        }
    }
}

/// The icon of a relation of `kind`, wherever one is shown: a grid for every kind of table, an eye
/// for a view, an eye over a bar for a materialized view.
pub fn relation_icon(kind: RelKind) -> Icon {
    match kind {
        RelKind::Table | RelKind::PartitionedTable | RelKind::ForeignTable => Icon::Table,
        RelKind::View => Icon::View,
        RelKind::MaterializedView => Icon::MaterializedView,
    }
}

/// A row of the schema tree.
#[derive(Clone, Copy, Debug)]
pub struct TreeRow<'a> {
    /// How deep it sits: 0 for a schema and the system group.
    pub level: usize,
    pub kind: TreeKind,
    pub name: &'a str,
    /// What shows at its right end: a column's type.
    pub right: Option<&'a str>,
    /// Its chevron, down while open; `None` for a row with nothing under it.
    pub open: Option<bool>,
    pub selected: bool,
    /// Showing keyboard focus.
    pub focused: bool,
    /// Its chevron showing keyboard focus.
    pub chevron_focused: bool,
    /// Under the pointer.
    pub hovered: bool,
}

/// When a row's name and the type at its right end don't both fit, the type keeps at most this
/// share of the room and the name is cut.
const RIGHT_SHARE: f32 = 0.45;

impl TreeRow<'_> {
    /// Draws it into `rect`. When its name or the words at its right end were cut: the whole
    /// words as written (the name, then a space and the right end's), for the tree to show on
    /// hover.
    pub fn paint(&self, ui: &Ui, rect: Rect) -> Option<String> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(rect, CornerRadius::ZERO, ground(t, self.selected, self.hovered));
        let y = rect.center().y;
        let chevron = chevron_slot(rect, self.level);
        if let Some(open) = self.open {
            let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
            icon.paint(&p, chevron.center(), t.text_muted);
        }
        let slot = chevron.right() + TREE_GAP;
        if let Some((icon, colour)) = self.kind.icon(t) {
            icon.paint(&p, pos2(slot + TREE_ICON / 2.0, y), colour);
        }
        let left = slot + TREE_ICON + TREE_GAP;
        let end = rect.right() - TREE_PAD_X;
        let room = (end - left).max(0.0);
        let right = self.right.map(|right| {
            let wanted = one_line(ui, self.name, Text::Body, t.text, f32::INFINITY).size().x;
            one_line(ui, right, Text::Data, t.text_muted, (room - TREE_GAP - wanted).max(room * RIGHT_SHARE))
        });
        let name_end = right.as_ref().map_or(end, |g| end - g.size().x - TREE_GAP);
        let name = one_line(ui, self.name, Text::Body, t.text, name_end - left);
        let cut = name.elided || right.as_ref().is_some_and(|g| g.elided);
        let height = name.size().y;
        p.galley(pos2(left, y - height / 2.0), name, t.text);
        if let Some(g) = right {
            let size = g.size();
            p.galley(pos2(end - size.x, y - size.y / 2.0), g, t.text_muted);
        }
        if self.focused {
            // Along the row's own edges: the box is as large as the hover ground, square as it is.
            focus::own_edge(&p, rect, 0);
        }
        if self.chevron_focused {
            focus::box_inside(&p, TreeRow::expander(rect, self.level));
        }
        cut.then(|| match self.right {
            Some(right) => format!("{} {right}", self.name),
            None => self.name.to_owned(),
        })
    }

    /// What it reads out: its name as written, and whether it is selected.
    pub fn widget_info(&self) -> WidgetInfo {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, self.selected, self.name)
    }

    /// What a table's chevron reads out: a button showing the table's columns, pressed while
    /// open.
    pub fn chevron_info(&self) -> WidgetInfo {
        WidgetInfo::selected(WidgetType::Button, true, self.open == Some(true), format!("Columns of {}", self.name))
    }

    /// Where the chevron of a row at `level`, drawn in `rect`, answers a click.
    pub fn expander(rect: Rect, level: usize) -> Rect {
        Rect::from_center_size(chevron_slot(rect, level).center(), Vec2::splat(TREE_EXPANDER))
    }
}

/// The chevron's square in a row at `level` drawn in `rect`.
fn chevron_slot(rect: Rect, level: usize) -> Rect {
    let left = rect.left() + TREE_PAD_X + TREE_INDENT * level as f32;
    Rect::from_min_size(pos2(left, rect.center().y - TREE_CHEVRON / 2.0), Vec2::splat(TREE_CHEVRON))
}

/// A connection in the connection list (saved, or used lately and not saved).
#[derive(Clone, Debug)]
pub struct ConnectionRow<'a> {
    /// The connection's name, the user's own.
    pub name: &'a str,
    /// Its engine, shown by its logo.
    pub engine: DbKind,
    /// The line under the name, in `data`: a server's address, cut at its end when short of room,
    /// or a file's path, cut in the middle as the status bar cuts it.
    pub address: Address,
    /// The colour the user gave it.
    pub colour: Option<Color32>,
    pub selected: bool,
    /// Showing keyboard focus.
    pub focused: bool,
    /// Under the pointer.
    pub hovered: bool,
}

impl ConnectionRow<'_> {
    /// Draws it into `rect`. When its name or its line was cut: the two whole, the name over the
    /// line, for the list to show on hover.
    pub fn paint(&self, ui: &Ui, rect: Rect) -> Option<String> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(rect, CornerRadius::ZERO, ground(t, self.selected, self.hovered));
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, t.line);
        if let Some(colour) = self.colour {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(CONNECTION_COLOUR_W, rect.height())), CornerRadius::ZERO, colour);
        }
        let logo = Rect::from_min_size(pos2(rect.left() + CONNECTION_PAD_X, rect.center().y - CONNECTION_LOGO / 2.0), Vec2::splat(CONNECTION_LOGO));
        Logo::of(self.engine).paint(&p, logo, t.text_muted);
        // The name's line over a `data` line, the pair centred in the row.
        let (name_h, line_h) = (Text::Body.line(), Text::Data.line());
        let left = logo.right() + CONNECTION_LOGO_GAP;
        let width = (rect.right() - CONNECTION_PAD_X - left).max(0.0);
        let top = rect.center().y - (name_h + line_h) / 2.0;
        let name = one_line(ui, self.name, Text::Body, t.text, width);
        let name_cut = name.elided;
        p.galley(pos2(left, top), name, t.text);
        let words = |address: &str| Words::default().name(address).job(t.text_muted);
        let fits = |address: &str| ui.painter().layout_job(words(address)).size().x <= width;
        let (mut job, cut) = match &self.address {
            Address::File(path) if !fits(path) => (words(&cut_middle(path, fits).unwrap_or_else(|| path.clone())), true),
            address => (words(address.text()), false),
        };
        job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
        let line = ui.painter().layout_job(job);
        let cut = name_cut || cut || line.elided;
        let mid = top + name_h + line_h / 2.0;
        let height = line.size().y;
        p.galley(pos2(left, mid - height / 2.0), line, t.text_muted);
        if self.focused {
            focus::own_edge(&p, rect.shrink(CONNECTION_FOCUS_INSET), RADIUS_CONTROL);
        }
        cut.then(|| format!("{}\n{}", self.name, self.address.text()))
    }

    /// What it reads out: its name as written, then its engine, which only its logo shows.
    pub fn widget_info(&self) -> WidgetInfo {
        WidgetInfo::labeled(WidgetType::Button, true, format!("{}, {}", self.name, self.engine.label()))
    }
}

/// A row's ground: `selection` when selected, whatever the pointer does; `hover` under the
/// pointer; else `bg`.
fn ground(t: &Tokens, selected: bool, hovered: bool) -> Color32 {
    if selected {
        t.selection
    } else if hovered {
        t.hover
    } else {
        t.bg
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, MemoryStore};
    use dbee_core::{Catalog, ColumnInfo, ForeignKey, SchemaInfo, TableInfo, TableRef, TypeClass};
    use dbee_engine::{Screen, Session, SessionOptions};
    use dbee_sql::Dialect;
    use egui::epaint::ClippedShape;
    use egui::{Event, Key as Keys, Modifiers, PointerButton, Pos2, Sense, Shape, Theme};
    use serde_json::json;

    use super::*;
    use crate::DbeeApp;
    use crate::automation::Tags;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness_in, icon_in, images, painted, sections, texts};
    use crate::tree::{MenuState, Tree, TreeAct};

    /// The rows are drawn this wide (`tree/row`, `sidebar/connection`).
    const TREE_W: f32 = 259.0;
    const CONNECTION_W: f32 = 279.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// One frame of `paint` drawing into a rect of `size`: the shapes and the rect.
    fn drawn(theme: Theme, size: Vec2, mut paint: impl FnMut(&Ui, Rect)) -> (Vec<ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            at = rect;
            paint(ui, rect);
        });
        (shapes, at)
    }

    /// `icon` drawn alone in `colour`, centred on `centre`: its colours and the box it fills.
    fn alone(theme: Theme, icon: Icon, centre: Pos2, colour: Color32) -> (Vec<Color32>, Rect) {
        icon_in(&painted(theme, |ui| icon.paint(ui.painter(), centre, colour)), Rect::EVERYTHING)
    }

    /// The words drawn within `r`, each with where and in what colour.
    fn words_in(shapes: &[ClippedShape], r: Rect) -> Vec<(String, Rect, Color32)> {
        texts(shapes).into_iter().filter(|(w, at, _)| !w.is_empty() && r.contains_rect(*at)).collect()
    }

    fn tree_row<'a>(level: usize, kind: TreeKind, name: &'a str, right: Option<&'a str>, open: Option<bool>) -> TreeRow<'a> {
        TreeRow { level, kind, name, right, open, selected: false, focused: false, chevron_focused: false, hovered: false }
    }

    /// `row` drawn 259 by 26 is `tree/row`: on `ground`, the focus box when `focus`, the
    /// chevron and `icon` in their squares, the name and the type.
    fn check_tree_row(theme: Theme, t: &Tokens, state: &str, row: TreeRow<'_>, ground: Color32, focus: bool, icon: Option<(Icon, Color32)>) {
        let at_ = format!("{theme:?} {state}");
        let (shapes, r) = drawn(theme, vec2(TREE_W, 26.0), |ui, rect| {
            row.paint(ui, rect);
        });
        let chevron = Rect::from_min_size(pos2(r.left() + 12.0 + 16.0 * row.level as f32, r.center().y - 6.0), vec2(12.0, 12.0));
        let slot = Rect::from_min_size(pos2(chevron.right() + 6.0, r.center().y - 7.0), vec2(14.0, 14.0));
        // The table icon's header band is a fill of its own, in its square.
        let fills: Vec<_> = fills_in(&shapes, r).into_iter().filter(|(at, _)| !slot.contains_rect(*at)).collect();
        assert_eq!(fills, [(r, ground)], "{at_}: one ground, square");
        // The focus box runs along the edges of the ground, the hover ground's too: as large, as square.
        let ring = Edge { outer: r, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
        assert_eq!(edges_in(&shapes, r, 0.0), if focus { vec![ring] } else { vec![] }, "{at_}: the focus box");
        let want = match row.open {
            Some(open) => alone(theme, if open { Icon::ChevronDown } else { Icon::ChevronRight }, chevron.center(), t.text_muted),
            None => (vec![], Rect::NOTHING),
        };
        assert_eq!(icon_in(&shapes, chevron.expand(1.0)), want, "{at_}: the chevron in its 12 square");
        let want = icon.map_or((vec![], Rect::NOTHING), |(icon, colour)| alone(theme, icon, slot.center(), colour));
        assert_eq!(icon_in(&shapes, slot.expand(1.2)), want, "{at_}: the icon in its 14 square, 6 after the chevron's");
        let words = words_in(&shapes, r);
        let Some((name, n, nc)) = words.first() else { panic!("{at_}: the name: {words:?}") };
        assert_eq!((name.as_str(), *nc), (row.name, t.text), "{at_}: the name");
        assert!(close(n.left(), slot.right() + 6.0) && close(n.center().y, r.center().y), "{at_}: the name 6 after the icon, centred: {n:?}");
        assert_eq!(sections(&shapes, row.name).first().map(|s| s.1.clone()), Some(Text::Body.font()), "{at_}: the name in `body`");
        match row.right {
            Some(ty) => {
                let [_, (w, y, c)] = words.as_slice() else { panic!("{at_}: the name and type: {words:?}") };
                assert_eq!((w.as_str(), *c), (ty, t.text_muted), "{at_}: the type");
                assert!(close(y.right(), r.right() - 12.0) && close(y.center().y, r.center().y), "{at_}: the type ends 12 from the right: {y:?}");
                assert_eq!(sections(&shapes, ty).first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: the type in `data`");
            }
            None => assert_eq!(words.len(), 1, "{at_}: only the name: {words:?}"),
        }
    }

    #[test]
    fn each_tree_row_has_its_look() {
        // 26 tall on `bg`, square. From 12 in, 16 more per level: the chevron's 12
        // square (`icon/chevron-right`, down when open, `text_muted`; empty on a column), 6, the
        // icon's 14 square (`text_muted`; `icon/key-primary` in `key`, `icon/key-foreign`, nothing on
        // a plain column), 6, the name in `body` `text`; a column's type in `data` `text_muted` ends
        // 12 from the right. Hovered `hover`; selected `selection`, also when hovered; keyboard focus
        // a 2 px `ink` box along the row's edges, square, as large as the hover ground (not 2
        // inside with corners 2, which is smaller than the ground beside it).
        let rel = TreeKind::Relation;
        for (theme, t) in both() {
            let muted = |icon| Some((icon, t.text_muted));
            let kinds = [
                ("schema", tree_row(0, TreeKind::Schema, "public", None, Some(false)), muted(Icon::Schema)),
                ("schema open", tree_row(0, TreeKind::Schema, "shop", None, Some(true)), muted(Icon::Schema)),
                ("system group", tree_row(0, TreeKind::System, "System schemas", None, Some(false)), muted(Icon::Schema)),
                ("system schema", tree_row(1, TreeKind::Schema, "pg_catalog", None, Some(true)), muted(Icon::Schema)),
                ("table", tree_row(1, rel(RelKind::Table), "orders", None, Some(false)), muted(Icon::Table)),
                ("table open", tree_row(1, rel(RelKind::Table), "orders", None, Some(true)), muted(Icon::Table)),
                ("partitioned", tree_row(1, rel(RelKind::PartitionedTable), "events", None, Some(false)), muted(Icon::Table)),
                ("foreign table", tree_row(1, rel(RelKind::ForeignTable), "remote_orders", None, Some(false)), muted(Icon::Table)),
                ("view", tree_row(1, rel(RelKind::View), "orders_archive", None, Some(false)), muted(Icon::View)),
                ("materialized view", tree_row(1, rel(RelKind::MaterializedView), "order_totals", None, Some(false)), muted(Icon::MaterializedView)),
                ("system table", tree_row(2, rel(RelKind::Table), "pg_class", None, Some(false)), muted(Icon::Table)),
                ("column", tree_row(2, TreeKind::Column(None), "dealer_id", Some("int4"), None), None),
                ("primary key column", tree_row(2, TreeKind::Column(Some(Key::Primary)), "id", Some("int8"), None), Some((Icon::KeyPrimary, t.key))),
                ("foreign key column", tree_row(2, TreeKind::Column(Some(Key::Foreign)), "customer_id", Some("int8"), None), muted(Icon::KeyForeign)),
                ("system column", tree_row(3, TreeKind::Column(None), "relname", Some("name"), None), None),
            ];
            for (state, row, icon) in kinds {
                check_tree_row(theme, &t, state, row, t.bg, false, icon);
            }
            let table = tree_row(1, rel(RelKind::Table), "orders", None, Some(false));
            for (state, row, ground, focus) in [
                ("hover", TreeRow { hovered: true, ..table }, t.hover, false),
                ("selected", TreeRow { selected: true, ..table }, t.selection, false),
                ("selected and hovered", TreeRow { selected: true, hovered: true, ..table }, t.selection, false),
                ("focus", TreeRow { focused: true, ..table }, t.bg, true),
                ("selected with focus", TreeRow { selected: true, focused: true, hovered: true, ..table }, t.selection, true),
            ] {
                check_tree_row(theme, &t, state, row, ground, focus, muted(Icon::Table));
            }
        }
        // The chevron answers a click in a 20 square round its own.
        let rect = Rect::from_min_size(pos2(8.0, 100.0), vec2(TREE_W, 26.0));
        assert_eq!(TreeRow::expander(rect, 1), Rect::from_center_size(pos2(8.0 + 28.0 + 6.0, 113.0), vec2(20.0, 20.0)));
    }

    /// One frame of `paint` drawing into a rect of `size`: the shapes, the rect and the images in
    /// it (rect, tint, texture's name, size).
    fn drawn_with_images(theme: Theme, size: Vec2, paint: impl Fn(&Ui, Rect)) -> (Vec<ClippedShape>, Rect, Vec<(Rect, Color32, String, [usize; 2])>) {
        let mut ctx = None;
        let (shapes, r) = drawn(theme, size, |ui, rect| {
            ctx = Some(ui.ctx().clone());
            paint(ui, rect);
        });
        let shown = ctx.map(|ctx| images(&ctx, &shapes)).unwrap_or_default();
        (shapes, r, shown)
    }

    #[test]
    fn each_connection_row_has_its_look() {
        // `sidebar/connection` in its default, hover, selected and focus states:
        // 40 tall on `bg`, `hover` or `selection` (also when hovered), square, a 1 px
        // `line` along the bottom; the connection's colour 3 wide down the whole left edge, none
        // without one; the engine's logo (`logo/postgresql`) in its 14 square 12 in, centred
        // down the row (13 down), `text_muted` in every state; the name in `body` `text` 34 in
        // and 3 down; under it the address in `data` `text_muted`, centred 29 down; keyboard
        // focus a 2 px `ink` box 1 inside, corners 2.
        let line = || Address::Server("postgres@pg14:5432/dbee".into());
        for (theme, t) in both() {
            let row = |colour| ConnectionRow {
                name: "Shop live",
                engine: DbKind::Postgres,
                address: line(),
                colour,
                selected: false,
                focused: false,
                hovered: false,
            };
            let red = Some(t.conn_red);
            for (state, row, ground, focus) in [
                ("default", row(red), t.bg, false),
                ("hover", ConnectionRow { hovered: true, ..row(red) }, t.hover, false),
                ("selected", ConnectionRow { selected: true, ..row(red) }, t.selection, false),
                ("selected and hovered", ConnectionRow { selected: true, hovered: true, ..row(red) }, t.selection, false),
                ("focus", ConnectionRow { focused: true, ..row(red) }, t.bg, true),
                ("no colour", row(None), t.bg, false),
            ] {
                let at_ = format!("{theme:?} {state}");
                let (shapes, r, logos) = drawn_with_images(theme, vec2(CONNECTION_W, 40.0), |ui, rect| {
                    row.paint(ui, rect);
                });
                let mut want = vec![(r, ground), (Rect::from_min_max(pos2(r.left(), r.bottom() - 1.0), r.max), t.line)];
                if let Some(colour) = row.colour {
                    want.push((Rect::from_min_size(r.min, vec2(3.0, 40.0)), colour));
                }
                assert_eq!(fills_in(&shapes, r), want, "{at_}: the ground, the line along the bottom, the colour down the left edge");
                let ring = Edge { outer: r.shrink(1.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                assert_eq!(edges_in(&shapes, r, 0.0), if focus { vec![ring] } else { vec![] }, "{at_}: the focus box");
                let logo = Rect::from_min_size(pos2(r.left() + 12.0, r.top() + 13.0), Vec2::splat(14.0));
                assert_eq!(logos, [(logo, t.text_muted, "logo/postgresql 14px".to_owned(), [14, 14])], "{at_}: the engine's logo");
                let words = words_in(&shapes, r);
                let [(name, n, nc), (address, a, ac)] = words.as_slice() else { panic!("{at_}: the name and the line: {words:?}") };
                assert_eq!((name.as_str(), *nc), ("Shop live", t.text), "{at_}: the name");
                assert_eq!((address.as_str(), *ac), ("postgres@pg14:5432/dbee", t.text_muted), "{at_}: the address alone");
                assert!(close(n.left(), r.left() + 34.0) && close(n.top(), r.top() + 3.0), "{at_}: the name 34 in, 3 down: {n:?}");
                assert!(close(a.left(), r.left() + 34.0) && close(a.center().y, r.top() + 29.0), "{at_}: the line 34 in, centred 29 down: {a:?}");
                assert_eq!(sections(&shapes, "Shop live").first().map(|s| s.1.clone()), Some(Text::Body.font()), "{at_}: the name in `body`");
                let fonts: Vec<_> = sections(&shapes, "postgres@pg14:5432/dbee").into_iter().map(|s| (s.0, s.1)).collect();
                assert_eq!(fonts, [("postgres@pg14:5432/dbee".into(), Text::Data.font())], "{at_}: the address in `data`");
                assert_eq!(row.widget_info().label.as_deref(), Some("Shop live, PostgreSQL"), "{at_}: read out with its engine");
            }
            // The other engines show their own logos, and say their own names.
            for (engine, logo, label) in [
                (DbKind::MySql, "logo/mysql 14px", "Shop live, MySQL"),
                (DbKind::MariaDb, "logo/mariadb 14px", "Shop live, MariaDB"),
                (DbKind::Sqlite, "logo/sqlite 14px", "Shop live, SQLite"),
            ] {
                let row = ConnectionRow { engine, ..row(red) };
                let (_, r, logos) = drawn_with_images(theme, vec2(CONNECTION_W, 40.0), |ui, rect| {
                    row.paint(ui, rect);
                });
                let at = Rect::from_min_size(pos2(r.left() + 12.0, r.top() + 13.0), Vec2::splat(14.0));
                assert_eq!(logos, [(at, t.text_muted, logo.to_owned(), [14, 14])], "{theme:?} {engine:?}");
                assert_eq!(row.widget_info().label.as_deref(), Some(label), "{theme:?} {engine:?}");
            }
        }
    }

    /// The line `row` draws in a row `width` wide: its words, where, whether egui cut its end, the
    /// fonts of its parts, and whether `paint` said something was cut.
    fn line_of(theme: Theme, row: &ConnectionRow<'_>, width: f32) -> (String, Rect, bool, Vec<(String, egui::FontId, Color32)>, bool, Rect) {
        let mut cut = false;
        let (shapes, r) = drawn(theme, vec2(width, 40.0), |ui, rect| cut = row.paint(ui, rect).is_some());
        let line = flat(&shapes)
            .into_iter()
            .find_map(|s| match s {
                Shape::Text(t) if t.galley.text() != row.name && r.contains_rect(t.galley.rect.translate(t.pos.to_vec2())) => Some(t),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{theme:?}: the line under the name"));
        let text = line.galley.text().to_owned();
        let at = line.galley.rect.translate(line.pos.to_vec2());
        (text.clone(), at, line.galley.elided, sections(&shapes, &text), cut, r)
    }

    #[test]
    fn a_sqlite_path_is_cut_in_the_middle_as_the_status_bar_cuts_it() {
        // The status bar's path rule (`a_long_sqlite_path_is_cut_in_the_middle`), on a row: the
        // home folder as `~` (the list's), cut in the middle with `…` when short of room, as many
        // letters kept at the start as at the end, so the file's name stays; the whole path on
        // hover (the list's, through `Hint`). A server's address is still cut at its end.
        const LONG: &str = "~/Projects/clients/northwind-traders/archive/2026/exports/nightly/notes.sqlite";
        for (theme, t) in both() {
            let row = |address: Address| ConnectionRow {
                name: "Notes",
                engine: DbKind::Sqlite,
                address,
                colour: Some(t.conn_purple),
                selected: false,
                focused: false,
                hovered: false,
            };
            // Room enough: whole, as with `~/data/notes.sqlite`.
            let (text, at, elided, _, cut, r) = line_of(theme, &row(Address::File("~/data/notes.sqlite".into())), CONNECTION_W);
            assert_eq!((text.as_str(), elided, cut), ("~/data/notes.sqlite", false, false), "{theme:?}: whole");
            assert!(close(at.left(), r.left() + 34.0), "{theme:?}: 34 in: {at:?}");
            // The row at its width, and narrower.
            for width in [CONNECTION_W, 220.0] {
                let at_ = format!("{theme:?} {width}");
                let (text, at, elided, parts, cut, r) = line_of(theme, &row(Address::File(LONG.into())), width);
                assert!(cut && !elided, "{at_}: cut in the middle, not at the end: `{text}`");
                let path = text.as_str();
                let (head, tail) = path.split_once('…').unwrap_or_else(|| panic!("{at_}: a cut path: `{text}`"));
                assert!(head.starts_with("~/") && LONG.starts_with(head) && LONG.ends_with(tail), "{at_}: `{path}`");
                assert!(tail.ends_with("/notes.sqlite") && head.chars().count() >= 6, "{at_}: the file's name kept: `{path}`");
                let even = head.chars().count().abs_diff(tail.chars().count()) <= 1;
                assert!(even || tail == "/notes.sqlite", "{at_}: cut in the middle, or just before the file's name: `{path}`");
                // Inside the row's right 12, and no letter more would fit.
                let room = r.right() - 12.0 - at.left();
                assert!(at.right() <= r.right() - 12.0 + 0.5 && room - at.width() < 7.2 + 0.5, "{at_}: {at:?} in {room}");
                assert_eq!(parts, [(path.to_owned(), Text::Data.font(), t.text_muted)], "{at_}: the path in `data` `text_muted`");
            }
            // A file's name longer than the row (a long example): the folders go,
            // and the name is cut in its own middle, with one `…`.
            let long = "~/Projects/clients/acme-corp/Orders-and-Invoices-Backup.sqlite";
            let (text, at, elided, _, cut, r) = line_of(theme, &row(Address::File(long.into())), CONNECTION_W);
            assert_eq!((text.as_str(), elided, cut), ("Orders-and-Invoi…s-Backup.sqlite", false, true), "{theme:?}: the name in its middle");
            assert!(at.right() <= r.right() - 12.0 + 0.5, "{theme:?}: inside the row's right 12: {at:?}");
            // Too narrow to keep 6 letters at either end (the status bar then drops the path): cut
            // at its end, and still whole on hover.
            let (text, _, elided, _, cut, _) = line_of(theme, &row(Address::File(LONG.into())), 120.0);
            assert!(elided && cut && !text.contains('…'), "{theme:?}: a row with no room for the cut: `{text}`");
            // A server's address too long for its row is cut at its end, as before; whole on hover.
            let server = ConnectionRow {
                engine: DbKind::Postgres,
                ..row(Address::Server("postgres@analytics-replica.internal.example.com:5432/analytics".into()))
            };
            let (text, _, elided, _, cut, _) = line_of(theme, &server, 200.0);
            assert!(elided && cut && !text.contains('…'), "{theme:?}: a server's address cut at its end: `{text}`");
        }
    }

    #[test]
    fn a_cut_name_or_line_reads_whole_on_hover() {
        // When the name or the line is cut, the list shows both whole on hover, the
        // name over the line; with nothing cut, no hover. No time words before the line.
        const FAR: &str = "postgres@analytics-replica.internal.example.com:5432/analytics";
        let row = |name: &'static str, address: &str| ConnectionRow {
            name,
            engine: DbKind::Postgres,
            address: Address::Server(address.into()),
            colour: None,
            selected: false,
            focused: false,
            hovered: false,
        };
        let hover = |row: &ConnectionRow<'_>| {
            let mut said = None;
            drawn(Theme::Light, vec2(CONNECTION_W, 40.0), |ui, rect| said = row.paint(ui, rect));
            said
        };
        assert_eq!(hover(&row("Shop live", "postgres@pg14:5432/dbee")), None, "nothing cut");
        let long = "Reporting replica for the Customer Success team";
        assert_eq!(hover(&row(long, "postgres@pg14:5432/dbee")), Some(format!("{long}\npostgres@pg14:5432/dbee")), "the name cut");
        assert_eq!(hover(&row("Analytics", FAR)), Some(format!("Analytics\n{FAR}")), "the line cut");
    }

    fn column(name: &str, label: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_sql: label.into(),
            base_type: label.into(),
            class: TypeClass::Text,
            not_null: false,
            has_default: false,
            generated: Default::default(),
            hidden: false,
            type_label: label.into(),
            operators: dbee_core::Operators::Unnamed,
        }
    }

    fn table(schema: &str, name: &str, kind: RelKind, columns: Vec<ColumnInfo>) -> TableInfo {
        TableInfo {
            table: TableRef { schema: schema.into(), name: name.into() },
            kind,
            primary_key: columns.first().map(|c| c.name.clone()).into_iter().collect(),
            columns,
            unique_keys: vec![],
            foreign_keys: vec![],
            estimated_rows: Some(12),
            no_edit_reason: None,
        }
    }

    fn schema(name: &str, system: bool, tables: Vec<TableInfo>) -> SchemaInfo {
        SchemaInfo { name: name.into(), system, tables }
    }

    /// A tree drawn every frame, with what it asked for.
    struct Shown {
        catalog: Arc<Catalog>,
        dialect: Dialect,
        tree: Tree,
        tags: Tags,
        acts: Vec<TreeAct>,
    }

    fn tree_harness(size: Vec2, catalog: Catalog) -> egui_kittest::Harness<'static, Shown> {
        tree_harness_in(size, catalog, Dialect::Postgres)
    }

    /// [`tree_harness`] read in `dialect`.
    fn tree_harness_in(size: Vec2, catalog: Catalog, dialect: Dialect) -> egui_kittest::Harness<'static, Shown> {
        let shown = Shown { catalog: Arc::new(catalog), dialect, tree: Tree::default(), tags: Tags::default(), acts: Vec::new() };
        harness_in(size, Theme::Light, shown, |ui, s: &mut Shown| {
            s.tags.begin_frame();
            let acts = crate::tree::show(ui, &s.catalog, s.dialect, &mut s.tree, None, &mut s.tags);
            s.acts.extend(acts);
        })
    }

    fn press(at: Pos2, pressed: bool, button: PointerButton) -> Event {
        Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE }
    }

    /// A click (`count` 2: a double click) at `at`. The presses go in one frame, as the control
    /// channel sends them: the harness gives queued events a frame each, longer apart than a
    /// double click may be.
    fn click(h: &mut egui_kittest::Harness<'static, Shown>, at: Pos2, count: usize) {
        h.event(Event::PointerMoved(at));
        h.run();
        for _ in 0..count {
            h.input_mut().events.extend([true, false].map(|pressed| press(at, pressed, PointerButton::Primary)));
        }
        h.run();
    }

    fn rect_of(h: &egui_kittest::Harness<'static, Shown>, id: &str) -> Rect {
        h.state().tags.rect(id).unwrap_or_else(|| panic!("`{id}` drawn: {:?}", h.state().tags.ids()))
    }

    #[test]
    fn names_and_types_keep_their_case() {
        // Review Focus 2: a table `Orders`, a column's type `enum('small','Large')` and a connection
        // `Shop (Live)` at `Root@DB.Example.com` are drawn as written, never in capitals.
        for (theme, t) in both() {
            let orders = tree_row(1, TreeKind::Relation(RelKind::Table), "Orders", None, Some(false));
            let mood = tree_row(2, TreeKind::Column(None), "Mood", Some("enum('small','Large')"), None);
            for (row, want) in [(orders, vec!["Orders"]), (mood, vec!["Mood", "enum('small','Large')"])] {
                let (shapes, r) = drawn(theme, vec2(340.0, 26.0), |ui, rect| {
                    row.paint(ui, rect);
                });
                let words: Vec<String> = words_in(&shapes, r).into_iter().map(|w| w.0).collect();
                assert_eq!(words, want, "{theme:?}: as written");
            }
            let row = ConnectionRow {
                name: "Shop (Live)",
                engine: DbKind::MariaDb,
                address: Address::Server("Root@DB.Example.com:3306/Shop".into()),
                colour: Some(t.conn_blue),
                selected: false,
                focused: false,
                hovered: false,
            };
            let (shapes, r) = drawn(theme, vec2(CONNECTION_W, 40.0), |ui, rect| {
                row.paint(ui, rect);
            });
            let words: Vec<String> = words_in(&shapes, r).into_iter().map(|w| w.0).collect();
            assert_eq!(words, ["Shop (Live)", "Root@DB.Example.com:3306/Shop"], "{theme:?}: as written");
        }
        // Through the tree: its tags and what it draws keep the catalog's case.
        let mut sales = table("Sales", "Orders", RelKind::Table, vec![column("id", "int8"), column("Mood", "enum('small','Large')")]);
        sales.primary_key = vec!["id".into()];
        let catalog = Catalog { schemas: vec![schema("Sales", false, vec![sales])], default_schema: Some("Sales".into()) };
        let mut h = tree_harness(vec2(400.0, 400.0), catalog);
        h.run();
        let expand = rect_of(&h, "tree-expand-Sales.Orders").center();
        click(&mut h, expand, 1);
        let tags = &h.state().tags;
        assert_eq!(tags.value("tree-table-Sales.Orders"), Some(&json!("table")));
        assert_eq!(tags.value("tree-column-Sales.Orders.Mood"), Some(&json!({ "type": "enum('small','Large')", "key": null })));
        let drawn: Vec<String> = flat(&h.output().shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        for words in ["Sales", "Orders", "Mood", "enum('small','Large')"] {
            assert!(drawn.iter().any(|w| w == words), "`{words}` drawn as written: {drawn:?}");
        }
        assert!(!drawn.iter().any(|w| w == "ORDERS" || w == "ENUM('SMALL','LARGE')"), "never in capitals: {drawn:?}");
    }

    #[test]
    fn a_516_table_tree_lays_out_only_what_shows() {
        // Review Focus 3: a schema of 516 tables, open. A frame lays out the rows in view (26
        // tall) and no more; the others are listed out of view; scrolled to the end, the same.
        let tables = (0..516).map(|i| table("s", &format!("t_{i:03}"), RelKind::Table, vec![column("id", "int8")])).collect();
        let catalog = Catalog { schemas: vec![schema("s", false, tables)], default_schema: Some("s".into()) };
        let height = 600.0;
        let mut h = tree_harness(vec2(280.0, height), catalog);
        h.run();
        let most = (height / 26.0).ceil() as usize + 1;
        let counts = |h: &egui_kittest::Harness<'static, Shown>| {
            let tags = &h.state().tags;
            let rows: Vec<Rect> = tags.ids().into_iter().filter(|id| id.starts_with("tree-table-")).filter_map(|id| tags.rect(id)).collect();
            let listed = tags.ids().into_iter().filter(|id| id.starts_with("tree-table-")).count();
            let laid = flat(&h.output().shapes).iter().filter(|s| matches!(s, Shape::Text(t) if t.galley.text().starts_with("t_"))).count();
            (rows, listed, laid)
        };
        for at in ["at the top", "at the end"] {
            if at == "at the end" {
                h.state_mut().tags.scroll_to("tree-table-s.t_515");
                h.run();
                assert!(h.state().tags.rect("tree-table-s.t_515").is_some(), "the last table in view");
            }
            let (rows, listed, laid) = counts(&h);
            assert!(!rows.is_empty() && rows.len() <= most, "{at}: {} rows drawn, at most {most}", rows.len());
            assert!(rows.iter().all(|r| r.height() == 26.0), "{at}: rows 26 tall: {rows:?}");
            assert_eq!(listed, 516, "{at}: every table listed, out of view too");
            assert!(laid <= most, "{at}: {laid} names laid out, at most {most}");
        }
    }

    #[test]
    fn the_tables_count_says_table_for_one() {
        // The count's word follows the total: `1 TABLE`, also filtered (`1 OF 1
        // TABLE`); `2 TABLES`, `1 OF 2 TABLES`.
        use egui_kittest::kittest::Queryable;
        for (n, all, filtered) in [(1, "1 TABLE", "1 OF 1 TABLE"), (2, "2 TABLES", "1 OF 2 TABLES")] {
            let tables = (0..n).map(|i| table("shop", &format!("t{i}"), RelKind::Table, vec![])).collect();
            let catalog = Catalog { schemas: vec![schema("shop", false, tables)], default_schema: Some("shop".into()) };
            let mut h = tree_harness(vec2(300.0, 400.0), catalog);
            h.run();
            let drawn =
                |h: &egui_kittest::Harness<'static, Shown>| -> Vec<String> { texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect() };
            assert!(drawn(&h).iter().any(|w| w == all), "{n}: `{all}`: {:?}", drawn(&h));
            h.get_by_role(egui::accesskit::Role::TextInput).click();
            h.run();
            h.get_by_role(egui::accesskit::Role::TextInput).type_text("t0");
            h.run();
            h.run();
            assert!(drawn(&h).iter().any(|w| w == filtered), "{n}: `{filtered}`: {:?}", drawn(&h));
        }
    }

    #[test]
    fn a_tables_chevron_shows_focus_and_reads_out_what_it_opens() {
        // Tab goes row, chevron, next row. On the chevron the tab close's focus
        // look shows, a 2 px `ink` box 2 inside its square, and the
        // row shows none. It reads out as a button, "Columns of <table>", pressed while open.
        let tables = vec![table("shop", "orders", RelKind::Table, vec![column("id", "int8")]), table("shop", "users", RelKind::Table, vec![])];
        let catalog = Catalog { schemas: vec![schema("shop", false, tables)], default_schema: Some("shop".into()) };
        let mut h = tree_harness(vec2(300.0, 400.0), catalog);
        h.run();
        let t = crate::theme::LIGHT;
        // From the filter: Tab to the refresh, the schema, the table, then its chevron.
        let at = rect_of(&h, "tree-filter").center();
        click(&mut h, at, 1);
        for _ in 0..4 {
            h.key_press(Keys::Tab);
            h.run();
        }
        let (row, chevron) = (rect_of(&h, "tree-table-shop.orders"), rect_of(&h, "tree-expand-shop.orders"));
        let ring = |r: Rect| Edge { outer: r.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
        let drawn = edges_in(&h.output().shapes, row, 0.0);
        assert_eq!(drawn, [ring(chevron)], "the chevron's focus box, and none on the row: {chevron:?}");
        let toggled = |h: &egui_kittest::Harness<'static, Shown>| {
            use egui_kittest::kittest::{NodeT, Queryable};
            h.get_by_role_and_label(egui::accesskit::Role::Button, "Columns of orders").accesskit_node().toggled()
        };
        assert_eq!(toggled(&h), Some(egui::accesskit::Toggled::False), "closed");
        h.key_press(Keys::Enter);
        h.run();
        assert_eq!(h.state().tags.value("tree-expand-shop.orders"), Some(&json!(true)), "Enter opens it");
        assert_eq!(toggled(&h), Some(egui::accesskit::Toggled::True), "open");
        // Tab goes on to the next row.
        h.key_press(Keys::Tab);
        h.run();
        let id = rect_of(&h, "tree-column-shop.orders.id");
        let whole = Edge { outer: id, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
        assert_eq!(edges_in(&h.output().shapes, id, 0.0), [whole], "the next row's focus box, along its edges");
    }

    #[test]
    fn a_cut_tree_row_says_its_whole_words() {
        // Gap 67: a row whose name or type is cut says the whole words, as written, for the tree to
        // show on hover: a column's name and type, a schema's name; a row that fits says nothing.
        const NAME: &str = "created_at_in_the_customers_time_zone";
        const TYPE: &str = "timestamp with time zone";
        let said = |row: TreeRow<'_>, width: f32| {
            let mut said = None;
            drawn(Theme::Light, vec2(width, 26.0), |ui, rect| said = row.paint(ui, rect));
            said
        };
        let column = tree_row(2, TreeKind::Column(None), NAME, Some(TYPE), None);
        assert_eq!(said(column, 200.0), Some(format!("{NAME} {TYPE}")), "cut at 200");
        assert_eq!(said(column, 600.0), None, "whole at 600");
        const SCHEMA: &str = "a_very_long_schema_name_that_is_cut_for_sure";
        assert_eq!(said(tree_row(0, TreeKind::Schema, SCHEMA, None, Some(false)), 120.0), Some(SCHEMA.to_owned()), "a schema cut");
        let mood = tree_row(2, TreeKind::Column(None), "Mood_Of_The_Customer_At_Checkout", Some("enum('small','Large')"), None);
        assert_eq!(said(mood, 200.0), Some("Mood_Of_The_Customer_At_Checkout enum('small','Large')".to_owned()), "as written");
        assert_eq!(said(tree_row(1, TreeKind::Relation(RelKind::Table), "orders", None, Some(false)), 200.0), None, "a short table");
    }

    /// Five tables in `shop`: two hold `ord`.
    fn five_tables() -> Catalog {
        let tables = ["orders", "order_items", "customers", "products", "payments"].map(|n| table("shop", n, RelKind::Table, vec![]));
        Catalog { schemas: vec![schema("shop", false, tables.into())], default_schema: Some("shop".into()) }
    }

    fn words_of(h: &egui_kittest::Harness<'static, Shown>) -> Vec<(String, Rect, Color32)> {
        texts(&h.output().shapes)
    }

    #[test]
    fn a_filter_with_no_match_reads_0_tables() {
        // A filter with no match reads `0 TABLES` over an empty tree, no phrase under it.
        let mut h = tree_harness(vec2(260.0, 400.0), five_tables());
        h.run();
        assert_eq!(h.state().tags.value("tree-count"), Some(&json!("5 tables")), "unfiltered");
        h.state_mut().tree.filter = "zzz".into();
        h.run();
        h.run();
        assert_eq!(h.state().tags.value("tree-count"), Some(&json!("0 tables")), "no match");
        let words: Vec<String> = words_of(&h).into_iter().map(|w| w.0).collect();
        assert!(words.iter().any(|w| w == "0 TABLES"), "`0 TABLES` drawn: {words:?}");
        assert!(!words.iter().any(|w| w.contains("No table matches")), "no phrase: {words:?}");
        assert!(!h.state().tags.ids().iter().any(|id| id.starts_with("tree-table-") || id.starts_with("tree-schema-")), "an empty tree");
        assert!(h.state().tags.value("tree-empty").is_none(), "no sentence");
        h.state_mut().tree.filter = "ord".into();
        h.run();
        h.run();
        assert_eq!(h.state().tags.value("tree-count"), Some(&json!("2 of 5 tables")), "two match");
    }

    #[test]
    fn an_empty_catalog_keeps_its_sentence_restyled() {
        // Not drawn; restyled: under `0 TABLES`, the sentence in `body` `text_muted`, 12 in from
        // the sidebar's left (where the count's words start) and 12 under the count's row.
        for (dialect, word) in [(Dialect::Postgres, "schemas"), (Dialect::MySql, "databases")] {
            let catalog = Catalog { schemas: vec![], default_schema: None };
            let mut h = tree_harness_in(vec2(260.0, 400.0), catalog, dialect);
            h.run();
            h.run();
            let sentence = format!("This connection has no {word} dbee can show.");
            assert_eq!(h.state().tags.value("tree-count"), Some(&json!("0 tables")), "{dialect:?}");
            assert_eq!(h.state().tags.value("tree-empty"), Some(&json!(sentence)), "{dialect:?}");
            let count = rect_of(&h, "tree-count");
            let row_bottom = count.center().y + crate::theme::size::SIDEBAR_COUNT_H / 2.0;
            let words = words_of(&h);
            let Some((_, at, colour)) = words.iter().find(|w| w.0 == sentence) else { panic!("{dialect:?}: the sentence drawn: {words:?}") };
            assert_eq!(*colour, crate::theme::LIGHT.text_muted, "{dialect:?}: text_muted");
            assert_eq!(sections(&h.output().shapes, &sentence).first().map(|s| s.1.clone()), Some(Text::Body.font()), "{dialect:?}: body");
            assert!((at.left() - count.left()).abs() <= 1.0, "{dialect:?}: 12 in, as the count: {at:?} vs {count:?}");
            assert!((at.top() - (row_bottom + 12.0)).abs() <= 1.0, "{dialect:?}: 12 under the count's row ({row_bottom}): {at:?}");
        }
    }

    #[test]
    fn a_cut_tree_row_reads_whole_on_hover() {
        // Gap 67: in a 200 wide tree, a cut column reads whole on hover, a cut foreign key with its
        // target; a column that fits has no hover; a foreign key that fits keeps today's (`cid`: at
        // 200 `customer_id int8` is cut).
        let mut orders = table(
            "shop",
            "orders",
            RelKind::Table,
            vec![
                column("id", "int8"),
                column("created_at_in_the_customers_time_zone", "timestamp with time zone"),
                column("long_customer_reference_id", "int8"),
                column("customer_id", "int8"),
                column("cid", "int8"),
                column("note", "text"),
            ],
        );
        let fk = |col: &str| ForeignKey {
            name: format!("fk_{col}"),
            columns: vec![col.into()],
            target: TableRef { schema: "shop".into(), name: "customers".into() },
            target_columns: vec!["id".into()],
        };
        orders.foreign_keys = vec![fk("long_customer_reference_id"), fk("customer_id"), fk("cid")];
        let catalog = Catalog { schemas: vec![schema("shop", false, vec![orders])], default_schema: Some("shop".into()) };
        let mut h = tree_harness(vec2(200.0, 500.0), catalog);
        h.run();
        let at = rect_of(&h, "tree-expand-shop.orders").center();
        click(&mut h, at, 1);
        let hover = |h: &mut egui_kittest::Harness<'static, Shown>, id: &str| -> Vec<String> {
            let at = rect_of(h, id).center();
            h.event(Event::PointerMoved(at));
            h.run();
            h.run();
            let row = rect_of(h, id);
            words_of(h).into_iter().filter(|(w, r, _)| !w.is_empty() && !row.contains_rect(*r)).map(|w| w.0).collect()
        };
        let long = hover(&mut h, "tree-column-shop.orders.created_at_in_the_customers_time_zone");
        assert!(long.iter().any(|w| w == "created_at_in_the_customers_time_zone timestamp with time zone"), "the whole column: {long:?}");
        let fk_cut = hover(&mut h, "tree-column-shop.orders.long_customer_reference_id");
        assert!(fk_cut.iter().any(|w| w == "long_customer_reference_id int8 → shop.customers"), "a cut foreign key: {fk_cut:?}");
        let plain = hover(&mut h, "tree-column-shop.orders.note");
        assert!(!plain.iter().any(|w| w.contains("note")), "a short column has no hover: {plain:?}");
        // At 200, `customer_id` and `int8` don't both fit either.
        let fk_cut = hover(&mut h, "tree-column-shop.orders.customer_id");
        assert!(fk_cut.iter().any(|w| w == "customer_id int8 → shop.customers"), "customer_id cut at 200: {fk_cut:?}");
        let short_fk = hover(&mut h, "tree-column-shop.orders.cid");
        assert!(short_fk.iter().any(|w| w == "cid → shop.customers"), "a short foreign key keeps today's: {short_fk:?}");
    }

    /// A right-click at `at`, the pointer there first, and a frame for the menu to settle.
    fn right_click(h: &mut egui_kittest::Harness<'static, Shown>, at: Pos2) {
        h.event(Event::PointerMoved(at));
        h.run();
        h.input_mut().events.extend([true, false].map(|pressed| press(at, pressed, PointerButton::Secondary)));
        h.run();
        h.run();
    }

    /// `shop` (open) holds a table with a column and a view; the system group a table.
    fn menu_catalog() -> Catalog {
        Catalog {
            schemas: vec![
                schema(
                    "shop",
                    false,
                    vec![table("shop", "orders", RelKind::Table, vec![column("id", "int8")]), table("shop", "order_totals", RelKind::View, vec![])],
                ),
                schema("pg_catalog", true, vec![table("pg_catalog", "pg_class", RelKind::Table, vec![])]),
            ],
            default_schema: Some("shop".into()),
        }
    }

    const MENU_IDS: [&str; 3] = ["tree-menu-open", "tree-menu-new-query", "tree-menu-copy-name"];

    fn menu_shown(h: &egui_kittest::Harness<'static, Shown>) -> Vec<String> {
        h.state().tags.ids().into_iter().filter(|id| id.starts_with("tree-menu-")).map(str::to_owned).collect()
    }

    /// The tag `id` as `ui.inspect` lists it.
    fn listed(h: &egui_kittest::Harness<'static, Shown>, id: &str) -> serde_json::Value {
        let all = h.state().tags.to_json();
        all.as_array().and_then(|a| a.iter().find(|e| e["id"] == id).cloned()).unwrap_or_else(|| panic!("`{id}` listed: {all}"))
    }

    #[test]
    fn a_table_row_has_the_menu_and_a_schema_row_has_none() {
        let mut h = tree_harness(vec2(300.0, 500.0), menu_catalog());
        h.run();
        let at = rect_of(&h, "tree-table-shop.orders").center();
        right_click(&mut h, at);
        assert_eq!(menu_shown(&h), MENU_IDS, "the three items, in order");
        let values: Vec<_> = MENU_IDS.iter().map(|id| h.state().tags.value(id).cloned()).collect();
        assert_eq!(
            values,
            [
                Some(json!({ "label": "Open", "shortcut": "Enter" })),
                Some(json!({ "label": "New query", "shortcut": "Ctrl+T" })),
                Some(json!({ "label": "Copy name", "shortcut": "Ctrl+C" })),
            ]
        );
        let rects: Vec<Rect> = MENU_IDS.iter().map(|id| rect_of(&h, id)).collect();
        assert!(rects[0].bottom() <= rects[1].top() + 0.5, "Open over New query: {rects:?}");
        let gap = rects[2].top() - rects[1].bottom();
        let separator = crate::theme::size::MENU_SEPARATOR_PAD * 2.0 + 1.0;
        assert!((gap - separator).abs() <= 1.0, "the separator between New query and Copy name: {gap} vs {separator}");
        let words: Vec<String> = texts(&h.output().shapes).into_iter().map(|w| w.0).collect();
        for w in ["Open", "New query", "Copy name", "ENTER", "CTRL", "T", "C"] {
            assert!(words.iter().any(|x| x == w), "`{w}` drawn: {words:?}");
        }
        // A view's row has it too.
        h.key_press(Keys::Escape);
        h.run();
        assert!(menu_shown(&h).is_empty(), "Escape closes it");
        let at = rect_of(&h, "tree-table-shop.order_totals").center();
        right_click(&mut h, at);
        assert_eq!(menu_shown(&h), MENU_IDS, "a view's row has the menu");
        h.key_press(Keys::Escape);
        h.run();
        // Not on a schema, the system group or a column.
        let at = rect_of(&h, "tree-expand-shop.orders").center();
        click(&mut h, at, 1);
        for id in ["tree-schema-shop", "tree-system", "tree-column-shop.orders.id"] {
            let at = rect_of(&h, id).center();
            right_click(&mut h, at);
            assert!(menu_shown(&h).is_empty(), "{id}: no menu: {:?}", menu_shown(&h));
        }
        assert!(h.state().acts.is_empty(), "nothing asked: {:?}", h.state().acts);
    }

    #[test]
    fn the_menu_reports_what_was_chosen() {
        let orders = TableRef { schema: "shop".into(), name: "orders".into() };
        let mut h = tree_harness(vec2(300.0, 500.0), menu_catalog());
        h.run();
        let row = rect_of(&h, "tree-table-shop.orders").center();
        let wants = [TreeAct::Open(orders.clone()), TreeAct::NewQuery(orders.clone()), TreeAct::CopyName(orders.clone())];
        for (id, want) in MENU_IDS.iter().zip(wants) {
            right_click(&mut h, row);
            let at = rect_of(&h, id).center();
            click(&mut h, at, 1);
            h.run();
            assert_eq!(std::mem::take(&mut h.state_mut().acts), [want], "{id}");
            assert!(menu_shown(&h).is_empty(), "{id}: the menu closes: {:?}", menu_shown(&h));
        }
        // Off: greyed, the reason in its tag, a click runs nothing.
        let why = "the connection is lost; reconnect first";
        h.state_mut().tree.menu = MenuState { open: Some(why.into()), ..MenuState::default() };
        right_click(&mut h, row);
        let open = listed(&h, "tree-menu-open");
        assert_eq!(open["value"], json!({ "label": "Open", "shortcut": "Enter", "why": why }), "{open}");
        assert_eq!(open["enabled"], json!(false), "{open}");
        assert!(listed(&h, "tree-menu-copy-name").get("enabled").is_none(), "Copy name stays on");
        let t = crate::theme::LIGHT;
        let words = texts(&h.output().shapes);
        let colour = words.iter().find(|w| w.0 == "Open").map(|w| w.2);
        assert_eq!(colour, Some(t.text_faint), "Open in text_faint");
        let at = rect_of(&h, "tree-menu-open").center();
        click(&mut h, at, 1);
        h.run();
        assert!(h.state().acts.is_empty(), "nothing runs: {:?}", h.state().acts);
    }

    #[test]
    fn tree_and_connection_tags_keep_their_ids_and_values() {
        // The tree: `tree-schema-<schema>` {open, tables}, `tree-system` {open},
        // `tree-table-<schema>.<table>` its kind, `tree-expand-…` open, `tree-column-…` {type, key}. A
        // click on a schema opens or closes it, on the system group too, on a chevron opens the
        // table's columns, on a table selects it, a double click opens it. Tab reaches the rows and
        // shows the focus box; a click doesn't.
        let mut orders = table("shop", "orders", RelKind::Table, vec![column("id", "int8"), column("customer_id", "int8"), column("note", "text")]);
        orders.foreign_keys = vec![ForeignKey {
            name: "fk".into(),
            columns: vec!["customer_id".into()],
            target: TableRef { schema: "shop".into(), name: "customers".into() },
            target_columns: vec!["id".into()],
        }];
        let catalog = Catalog {
            schemas: vec![
                schema("shop", false, vec![orders, table("shop", "order_totals", RelKind::MaterializedView, vec![])]),
                schema("audit", false, vec![table("audit", "log", RelKind::Table, vec![])]),
                schema("pg_catalog", true, vec![table("pg_catalog", "pg_class", RelKind::Table, vec![])]),
            ],
            default_schema: Some("shop".into()),
        };
        let mut h = tree_harness(vec2(300.0, 500.0), catalog);
        h.run();
        let value = |h: &egui_kittest::Harness<'static, Shown>, id: &str| h.state().tags.value(id).cloned();
        assert_eq!(value(&h, "tree-schema-shop"), Some(json!({ "open": true, "tables": 2 })));
        assert_eq!(value(&h, "tree-table-shop.orders"), Some(json!("table")));
        assert_eq!(value(&h, "tree-table-shop.order_totals"), Some(json!("materialized view")));
        assert_eq!(value(&h, "tree-expand-shop.orders"), Some(json!(false)));
        assert_eq!(value(&h, "tree-schema-audit"), Some(json!({ "open": false, "tables": 1 })));
        assert_eq!(value(&h, "tree-system"), Some(json!({ "open": false })));
        let row = rect_of(&h, "tree-table-shop.orders");
        assert_eq!(row.height(), 26.0, "a row 26 tall");
        assert_eq!(rect_of(&h, "tree-expand-shop.orders"), TreeRow::expander(row, 1), "the chevron's target");
        // First, as egui would count a double click on a row just clicked as a triple click.
        click(&mut h, row.center(), 2);
        let open = TreeAct::Open(TableRef { schema: "shop".into(), name: "orders".into() });
        assert_eq!(h.state().acts, [open], "a double click opens it");

        let at = rect_of(&h, "tree-expand-shop.orders").center();
        click(&mut h, at, 1);
        assert_eq!(value(&h, "tree-expand-shop.orders"), Some(json!(true)));
        assert_eq!(value(&h, "tree-column-shop.orders.id"), Some(json!({ "type": "int8", "key": "primary" })));
        assert_eq!(value(&h, "tree-column-shop.orders.customer_id"), Some(json!({ "type": "int8", "key": "foreign" })));
        assert_eq!(value(&h, "tree-column-shop.orders.note"), Some(json!({ "type": "text", "key": null })));
        let at = rect_of(&h, "tree-system").center();
        click(&mut h, at, 1);
        assert_eq!(value(&h, "tree-system"), Some(json!({ "open": true })));
        assert_eq!(value(&h, "tree-schema-pg_catalog"), Some(json!({ "open": false, "tables": 1 })));
        let at = rect_of(&h, "tree-schema-audit").center();
        click(&mut h, at, 1);
        assert_eq!(value(&h, "tree-schema-audit"), Some(json!({ "open": true, "tables": 1 })));
        assert_eq!(h.state().tree.selected_schema(&h.state().catalog), Some("audit"), "a click on a schema selects it");
        let at = rect_of(&h, "tree-table-shop.orders").center();
        click(&mut h, at, 1);
        assert_eq!(h.state().tree.selected_schema(&h.state().catalog), Some("shop"), "a click on a table selects it");
        let orders = rect_of(&h, "tree-table-shop.orders");
        let t = crate::theme::LIGHT;
        let ring = Edge { outer: orders, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
        assert!(fills_in(&h.output().shapes, orders).contains(&(orders, t.selection)), "selected");
        assert!(edges_in(&h.output().shapes, orders, 0.0).contains(&ring), "a click gives the row the keyboard and its focus box");
        // From the filter, Tab goes to the refresh, then to the first row: its focus box shows.
        let at = rect_of(&h, "tree-filter").center();
        click(&mut h, at, 1);
        for _ in 0..2 {
            h.key_press(Keys::Tab);
            h.run();
        }
        let shop = rect_of(&h, "tree-schema-shop");
        let ring = Edge { outer: shop, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
        assert!(edges_in(&h.output().shapes, shop, 0.0).contains(&ring), "Tab shows the focus box: {:?}", edges_in(&h.output().shapes, shop, 0.0));

        // The connection list: `conn-saved-<id>` and `conn-recent-<n>` (a connection used and not
        // saved) show the name, their `-kind` the engine's label; rows 40 tall, one under the other. A click loads one into the
        // form; a right-click offers Connect, Edit, Duplicate and Delete.
        let home = dbee_testkit::temp_home(None, None);
        let session = Session::new(SessionOptions {
            config_dir: Some(home.home.join("config")),
            secrets: Some(Arc::new(MemoryStore::default())),
            drivers: dbee_drivers::all(),
            ..SessionOptions::default()
        })
        .unwrap_or_else(|e| panic!("{e:?}"));
        let mut app = DbeeApp::new(session, None);
        app.themed_for_test();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let frame = |app: &mut DbeeApp, events: Vec<Event>| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
            let shapes = std::mem::take(&mut out.shapes);
            out.drop_without_applying_deltas();
            shapes
        };
        frame(&mut app, vec![]);
        let file = home.home.join("Notes.sqlite");
        let notes = ConnectionConfig {
            name: "Notes (Live)".into(),
            kind: DbKind::Sqlite,
            file: Some(file.clone()),
            host: String::new(),
            port: 0,
            color: ConnectionColor::Purple,
            ..ConnectionConfig::default()
        };
        let other = ConnectionConfig { name: "Other".into(), ..notes.clone() };
        for config in [&notes, &other] {
            app.run("connection.save", json!({ "config": config })).unwrap_or_else(|e| panic!("{e:?}"));
        }
        let ids: Vec<String> = app.session.state().store.saved.iter().map(|c| c.id.clone()).collect();
        let scratch = ConnectionConfig { id: String::new(), name: "Scratch".into(), file: Some(home.home.join("Scratch.sqlite")), ..notes.clone() };
        let opened = app.session.run_blocking("connection.open", json!({ "config": scratch, "createFile": true }), Duration::from_secs(30));
        opened.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(app.session.state().screen, Screen::Workspace);
        app.session.run_blocking("connection.close", json!({ "force": true }), Duration::from_secs(30)).unwrap_or_else(|e| panic!("{e:?}"));
        frame(&mut app, vec![]);
        let shapes = frame(&mut app, vec![]);
        let tags = app.tags();
        let saved: Vec<String> = ids.iter().map(|id| format!("conn-saved-{id}")).collect();
        let names: Vec<_> = saved.iter().map(|id| tags.value(id).cloned()).collect();
        assert_eq!(names, [Some(json!("Notes (Live)")), Some(json!("Other"))], "{:?}", tags.ids());
        assert_eq!(tags.value(&format!("{}-kind", saved[0])), Some(&json!("SQLite")));
        assert_eq!(tags.value("conn-recent-0"), Some(&json!("Scratch")));
        assert_eq!(tags.value("conn-recent-0-kind"), Some(&json!("SQLite")));
        let rows: Vec<Rect> = saved.iter().map(|id| tags.rect(id).unwrap_or(Rect::NOTHING)).collect();
        assert!(rows.iter().all(|r| r.height() == 40.0), "rows 40 tall: {rows:?}");
        assert_eq!(rows[1].top(), rows[0].bottom(), "one under the other, the line between them");
        // Each row shows its engine's logo; its line is the path alone.
        let recent = tags.rect("conn-recent-0").unwrap_or(Rect::NOTHING);
        let shown = images(&ctx, &shapes);
        for r in rows.iter().chain([&recent]) {
            let logo = Rect::from_min_size(pos2(r.left() + 12.0, r.top() + 13.0), Vec2::splat(14.0));
            let here: Vec<_> = shown.iter().filter(|i| r.contains_rect(i.0)).cloned().collect();
            assert_eq!(here, [(logo, tokens(&ctx).text_muted, "logo/sqlite 14px".to_owned(), [14, 14])], "the row at {r:?}: SQLite's logo");
        }
        let lines: Vec<String> = texts(&shapes).into_iter().filter(|(_, at, _)| rows[0].contains_rect(*at)).map(|w| w.0).collect();
        let [name, path] = lines.as_slice() else { panic!("the name and the line: {lines:?}") };
        assert!(name == "Notes (Live)" && path.ends_with("Notes.sqlite") && !path.contains("SQLite"), "the name, then the path alone: {lines:?}");
        let at = rows[1].center();
        frame(&mut app, vec![Event::PointerMoved(at)]);
        frame(&mut app, vec![press(at, true, PointerButton::Primary)]);
        frame(&mut app, vec![press(at, false, PointerButton::Primary)]);
        frame(&mut app, vec![]);
        assert_eq!(app.tags().value("conn-name"), Some(&json!("Other")), "a click loads it into the form");
        frame(&mut app, vec![press(at, true, PointerButton::Secondary)]);
        frame(&mut app, vec![press(at, false, PointerButton::Secondary)]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        for (item, label) in
            [("conn-menu-connect", "Connect"), ("conn-menu-edit", "Edit"), ("conn-menu-duplicate", "Duplicate"), ("conn-menu-delete", "Delete")]
        {
            assert_eq!(app.tags().value(item), Some(&json!(label)), "{item}: {:?}", app.tags().ids());
        }
    }
}

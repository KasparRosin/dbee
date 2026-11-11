//! The sidebar's rows: the schema tree's (`tree/row`) and the connection list's
//! (`sidebar/connection`). Each is a view over plain data that draws into the rect its list gives
//! it; the tree ([`crate::tree`]) and the connection screen keep their layout, scrolling, input,
//! menus and tags, and say what each row is: selected, under the pointer, showing keyboard focus.
//!
//! Names keep their case: a schema, table or column name, a type and a connection's name are
//! drawn as written. A selected row doesn't change on hover. Keyboard focus is a 2 px `ink` box
//! inside the row.

use dbee_core::RelKind;
use egui::text::TextWrapping;
use egui::{Color32, CornerRadius, Rect, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};

use super::grid_cell::{Key, one_line};
use super::icon::Icon;
use super::{focus, painter};
use crate::theme::size::{
    CONNECTION_COLOUR_W, CONNECTION_FOCUS_INSET, CONNECTION_PAD_X, RADIUS_CONTROL, TREE_CHEVRON, TREE_EXPANDER, TREE_GAP, TREE_ICON, TREE_INDENT,
    TREE_PAD_X,
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
    /// Under the pointer.
    pub hovered: bool,
}

/// When a row's name and the type at its right end don't both fit, the type keeps at most this
/// share of the room and the name is cut.
const RIGHT_SHARE: f32 = 0.45;

impl TreeRow<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
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
        let height = name.size().y;
        p.galley(pos2(left, y - height / 2.0), name, t.text);
        if let Some(g) = right {
            let size = g.size();
            p.galley(pos2(end - size.x, y - size.y / 2.0), g, t.text_muted);
        }
        if self.focused {
            focus::box_inside(&p, rect);
        }
    }

    /// What it reads out: its name as written, and whether it is selected.
    pub fn widget_info(&self) -> WidgetInfo {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, self.selected, self.name)
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

/// A saved or recent connection in the connection list.
#[derive(Clone, Debug)]
pub struct ConnectionRow<'a> {
    /// The connection's name, the user's own.
    pub name: &'a str,
    /// The line under the name: dbee's words, with the address or path in them.
    pub line: Words,
    /// The colour the user gave it.
    pub colour: Option<Color32>,
    pub selected: bool,
    /// Showing keyboard focus.
    pub focused: bool,
    /// Under the pointer.
    pub hovered: bool,
}

impl ConnectionRow<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(rect, CornerRadius::ZERO, ground(t, self.selected, self.hovered));
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, t.line);
        if let Some(colour) = self.colour {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(CONNECTION_COLOUR_W, rect.height())), CornerRadius::ZERO, colour);
        }
        // As on the page: the name's line over a `data` line, the pair centred in the row.
        let (name_h, line_h) = (Text::Body.line(), Text::Data.line());
        let left = rect.left() + CONNECTION_PAD_X;
        let width = (rect.right() - CONNECTION_PAD_X - left).max(0.0);
        let top = rect.center().y - (name_h + line_h) / 2.0;
        p.galley(pos2(left, top), one_line(ui, self.name, Text::Body, t.text, width), t.text);
        let mut job = self.line.job(t.text_muted);
        job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
        let line = ui.painter().layout_job(job);
        let mid = top + name_h + line_h / 2.0;
        let height = line.size().y;
        p.galley(pos2(left, mid - height / 2.0), line, t.text_muted);
        if self.focused {
            focus::own_edge(&p, rect.shrink(CONNECTION_FOCUS_INSET), RADIUS_CONTROL);
        }
    }

    /// What it reads out: its name as written.
    pub fn widget_info(&self) -> WidgetInfo {
        WidgetInfo::labeled(WidgetType::Button, true, self.name)
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
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness_in, icon_in, painted, sections, texts};
    use crate::tree::{Tree, TreeAct};

    /// The page's rows are this wide (`tree/row` 167592, `sidebar/connection` 167734).
    const TREE_W: f32 = 259.0;
    const CONNECTION_W: f32 = 279.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// One frame of `paint` drawing into a rect of `size`: the shapes and the rect.
    fn drawn(theme: Theme, size: Vec2, paint: impl Fn(&Ui, Rect)) -> (Vec<ClippedShape>, Rect) {
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
        TreeRow { level, kind, name, right, open, selected: false, focused: false, hovered: false }
    }

    /// `row` drawn 259 by 26 matches node 167592: on `ground`, the focus box when `focus`, the
    /// chevron and `icon` in their squares, the name and the type.
    fn check_tree_row(theme: Theme, t: &Tokens, state: &str, row: TreeRow<'_>, ground: Color32, focus: bool, icon: Option<(Icon, Color32)>) {
        let at_ = format!("{theme:?} {state}");
        let (shapes, r) = drawn(theme, vec2(TREE_W, 26.0), |ui, rect| row.paint(ui, rect));
        let chevron = Rect::from_min_size(pos2(r.left() + 12.0 + 16.0 * row.level as f32, r.center().y - 6.0), vec2(12.0, 12.0));
        let slot = Rect::from_min_size(pos2(chevron.right() + 6.0, r.center().y - 7.0), vec2(14.0, 14.0));
        // The table icon's header band is a fill of its own, in its square.
        let fills: Vec<_> = fills_in(&shapes, r).into_iter().filter(|(at, _)| !slot.contains_rect(*at)).collect();
        assert_eq!(fills, [(r, ground)], "{at_}: one ground, square");
        let ring = Edge { outer: r.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
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
    fn each_tree_row_matches_the_page() {
        // Node 167592: 26 tall on `bg`, square. From 12 in, 16 more per level: the chevron's 12
        // square (`icon/chevron-right`, down when open, `text_muted`; empty on a column), 6, the
        // icon's 14 square (`text_muted`; `icon/key-primary` in `key`, `icon/key-foreign`, nothing on
        // a plain column), 6, the name in `body` `text`; a column's type in `data` `text_muted` ends
        // 12 from the right. Hovered `hover`; selected `selection`, also when hovered; keyboard focus
        // a 2 px `ink` box 2 inside, corners 2.
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

    #[test]
    fn each_connection_row_matches_the_page() {
        // Node 167734's variants 167726 (default), 167735 (hover), 167743 (selected), 167751
        // (focus): 40 tall on `bg`, `hover` or `selection` (also when hovered), square, a 1 px
        // `line` along the bottom; the connection's colour 3 wide down the whole left edge, none
        // without one; the name in `body` `text` 12 in and 3 down; under it the line in
        // `text_muted`, dbee's words in `body` and the address in `data`, centred 29 down; keyboard
        // focus a 2 px `ink` box 1 inside, corners 2. The engine's logo waits for its own task: the
        // line names the engine.
        let line = || Words::from("PostgreSQL · ").name("postgres@pg14:5432/dbee");
        for (theme, t) in both() {
            let row = |colour| ConnectionRow { name: "Shop live", line: line(), colour, selected: false, focused: false, hovered: false };
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
                let (shapes, r) = drawn(theme, vec2(CONNECTION_W, 40.0), |ui, rect| row.paint(ui, rect));
                let mut want = vec![(r, ground), (Rect::from_min_max(pos2(r.left(), r.bottom() - 1.0), r.max), t.line)];
                if let Some(colour) = row.colour {
                    want.push((Rect::from_min_size(r.min, vec2(3.0, 40.0)), colour));
                }
                assert_eq!(fills_in(&shapes, r), want, "{at_}: the ground, the line along the bottom, the colour down the left edge");
                let ring = Edge { outer: r.shrink(1.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                assert_eq!(edges_in(&shapes, r, 0.0), if focus { vec![ring] } else { vec![] }, "{at_}: the focus box");
                let words = words_in(&shapes, r);
                let [(name, n, nc), (said, l, lc), (address, a, ac)] = words.as_slice() else { panic!("{at_}: the name and the line: {words:?}") };
                assert_eq!((name.as_str(), *nc), ("Shop live", t.text), "{at_}: the name");
                assert_eq!(
                    (said.as_str(), *lc, address.as_str(), *ac),
                    ("PostgreSQL · ", t.text_muted, "postgres@pg14:5432/dbee", t.text_muted),
                    "{at_}"
                );
                assert_eq!(l, a, "{at_}: one line");
                assert!(close(n.left(), r.left() + 12.0) && close(n.top(), r.top() + 3.0), "{at_}: the name 12 in, 3 down: {n:?}");
                assert!(close(l.left(), r.left() + 12.0) && close(l.center().y, r.top() + 29.0), "{at_}: the line 12 in, centred 29 down: {l:?}");
                assert_eq!(sections(&shapes, "Shop live").first().map(|s| s.1.clone()), Some(Text::Body.font()), "{at_}: the name in `body`");
                let fonts: Vec<_> = sections(&shapes, "PostgreSQL · postgres@pg14:5432/dbee").into_iter().map(|s| (s.0, s.1)).collect();
                assert_eq!(fonts, [("PostgreSQL · ".into(), Text::Body.font()), ("postgres@pg14:5432/dbee".into(), Text::Data.font())], "{at_}");
            }
        }
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
        tree: Tree,
        tags: Tags,
        acts: Vec<TreeAct>,
    }

    fn tree_harness(size: Vec2, catalog: Catalog) -> egui_kittest::Harness<'static, Shown> {
        let shown = Shown { catalog: Arc::new(catalog), tree: Tree::default(), tags: Tags::default(), acts: Vec::new() };
        harness_in(size, Theme::Light, shown, |ui, s: &mut Shown| {
            s.tags.begin_frame();
            let acts = crate::tree::show(ui, &s.catalog, Dialect::Postgres, &mut s.tree, &mut s.tags);
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
                let (shapes, r) = drawn(theme, vec2(340.0, 26.0), |ui, rect| row.paint(ui, rect));
                let words: Vec<String> = words_in(&shapes, r).into_iter().map(|w| w.0).collect();
                assert_eq!(words, want, "{theme:?}: as written");
            }
            let row = ConnectionRow {
                name: "Shop (Live)",
                line: Words::from("MariaDB · ").name("Root@DB.Example.com:3306/Shop"),
                colour: Some(t.conn_blue),
                selected: false,
                focused: false,
                hovered: false,
            };
            let (shapes, r) = drawn(theme, vec2(CONNECTION_W, 40.0), |ui, rect| row.paint(ui, rect));
            let words: Vec<String> = words_in(&shapes, r).into_iter().map(|w| w.0).collect();
            assert_eq!(words, ["Shop (Live)", "MariaDB · ", "Root@DB.Example.com:3306/Shop"], "{theme:?}: as written");
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
        let ring = Edge { outer: orders.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
        assert!(fills_in(&h.output().shapes, orders).contains(&(orders, t.selection)), "selected");
        assert!(!edges_in(&h.output().shapes, orders, 0.0).contains(&ring), "a click shows no focus box");
        // From the filter, Tab goes to the refresh, then to the first row: its focus box shows.
        let at = rect_of(&h, "tree-filter").center();
        click(&mut h, at, 1);
        for _ in 0..2 {
            h.key_press(Keys::Tab);
            h.run();
        }
        let shop = rect_of(&h, "tree-schema-shop");
        let ring = Edge { outer: shop.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
        assert!(edges_in(&h.output().shapes, shop, 0.0).contains(&ring), "Tab shows the focus box: {:?}", edges_in(&h.output().shapes, shop, 0.0));

        // The connection list: `conn-saved-<id>` and `conn-recent-<n>` show the name, their
        // `-kind` the engine's label; rows 40 tall, one under the other. A click loads one into the
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
            ctx.run_ui(raw, |ui| app.ui(ui)).drop_without_applying_deltas();
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
        let opened = app.session.run_blocking("connection.open", json!({ "config": notes, "createFile": true }), Duration::from_secs(30));
        opened.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(app.session.state().screen, Screen::Workspace);
        app.session.run_blocking("connection.close", json!({ "force": true }), Duration::from_secs(30)).unwrap_or_else(|e| panic!("{e:?}"));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let tags = app.tags();
        let saved: Vec<String> = ids.iter().map(|id| format!("conn-saved-{id}")).collect();
        let names: Vec<_> = saved.iter().map(|id| tags.value(id).cloned()).collect();
        assert_eq!(names, [Some(json!("Notes (Live)")), Some(json!("Other"))], "{:?}", tags.ids());
        assert_eq!(tags.value(&format!("{}-kind", saved[0])), Some(&json!("SQLite")));
        assert_eq!(tags.value("conn-recent-0"), Some(&json!("Notes (Live)")));
        assert_eq!(tags.value("conn-recent-0-kind"), Some(&json!("SQLite")));
        let rows: Vec<Rect> = saved.iter().map(|id| tags.rect(id).unwrap_or(Rect::NOTHING)).collect();
        assert!(rows.iter().all(|r| r.height() == 40.0), "rows 40 tall: {rows:?}");
        assert_eq!(rows[1].top(), rows[0].bottom(), "one under the other, the line between them");
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

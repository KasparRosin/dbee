//! The structure view's parts (`structure/heading`, `structure/header cell`, `structure/cell`):
//! views over plain data that draw into the rect the view gives them, on the view's own ground.
//! The view (Phase 3) keeps its layout, its columns' widths and what a click does.
//!
//! A section's heading is dbee's words in `label` capitals. The header's titles are `header`
//! over a 1 px `ink` line. A cell is `data` over a 1 px `line`: a column's name in `text`, what is
//! said about it (its type, its default, its position) in `text_muted`, all as written.

use std::sync::Arc;

use egui::text::TextWrapping;
use egui::{Color32, CornerRadius, Galley, Painter, Rect, Ui, pos2};

use super::grid_cell::one_line;
use super::icon::Icon;
use super::painter;
use crate::theme::size::{STRUCTURE_CELL_PAD_X, STRUCTURE_HEADING_PAD_X, STRUCTURE_ICON_GAP, STRUCTURE_LINK_GAP, STRUCTURE_POSITION_PAD_X};
use crate::theme::{Text, tokens};

/// A section's heading over its table: COLUMNS, INDEXES, …
#[derive(Clone, Copy, Debug)]
pub struct Heading<'a> {
    pub title: &'a str,
}

impl Heading<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let colour = tokens(ui.ctx()).text_muted;
        let mut job = Text::Label.job(self.title, colour);
        job.wrap = TextWrapping::truncate_at_width((rect.width() - 2.0 * STRUCTURE_HEADING_PAD_X).max(1.0));
        let title = ui.painter().layout_job(job);
        place(&painter(ui), title, rect.left() + STRUCTURE_HEADING_PAD_X, rect.center().y, colour);
    }
}

/// A column's title in the table's header.
#[derive(Clone, Copy, Debug)]
pub struct HeaderCell<'a> {
    pub title: &'a str,
}

impl HeaderCell<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, t.ink);
        let title = one_line(ui, self.title, Text::Header, t.text, rect.width() - 2.0 * STRUCTURE_CELL_PAD_X);
        place(&p, title, rect.left() + STRUCTURE_CELL_PAD_X, rect.center().y, t.text);
    }
}

/// Draws `g` from `x`, centred on `y`; returns its width.
fn place(p: &Painter, g: Arc<Galley>, x: f32, y: f32, colour: Color32) -> f32 {
    let size = g.size();
    p.galley(pos2(x, y - size.y / 2.0), g, colour);
    size.x
}

/// What a cell of the table shows, by the page's kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell<'a> {
    /// A name, in `text`.
    Text(&'a str),
    /// What is said about a column (its type, its default), in `text_muted`; empty for nothing.
    Muted(&'a str),
    /// A column's place in its table, from 1.
    Position(usize),
    /// The column may be NULL: a check.
    Nullable,
    /// The column is in the primary key.
    PrimaryKey,
    /// The column is a foreign key to this column (`schema.table.column`), as written.
    ForeignKey(&'a str),
}

/// A primary key's words.
pub const PRIMARY_KEY: &str = "primary key";
/// What follows the column a foreign key points at.
pub const ARROW: &str = "→";

impl Cell<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, t.line);
        let (x, y, end) = (rect.left() + STRUCTURE_CELL_PAD_X, rect.center().y, rect.right() - STRUCTURE_CELL_PAD_X);
        // `s` in `data` from `x` to `end`, cut with `…`; how wide it came out.
        let words = |s: &str, x: f32, end: f32, colour: Color32| place(&p, one_line(ui, s, Text::Data, colour, end - x), x, y, colour);
        // The key's icon at the start; where its words start.
        let key = |colour: Color32| {
            Icon::Key.paint(&p, pos2(x + Icon::Key.size().x / 2.0, y), colour);
            x + Icon::Key.size().x + STRUCTURE_ICON_GAP
        };
        match *self {
            Cell::Text(s) => {
                words(s, x, end, t.text);
            }
            Cell::Muted(s) => {
                words(s, x, end, t.text_muted);
            }
            Cell::Position(n) => {
                words(&n.to_string(), rect.left() + STRUCTURE_POSITION_PAD_X, end, t.text_muted);
            }
            Cell::Nullable => Icon::Check.paint(&p, pos2(x + Icon::Check.size().x / 2.0, y), t.text_muted),
            Cell::PrimaryKey => {
                words(PRIMARY_KEY, key(t.key), end, t.text_muted);
            }
            Cell::ForeignKey(target) => {
                let x = key(t.text_muted);
                let arrow = one_line(ui, ARROW, Text::Data, t.text_muted, f32::INFINITY);
                let shown = words(target, x, end - STRUCTURE_LINK_GAP - arrow.size().x, t.text_muted);
                place(&p, arrow, x + shown + STRUCTURE_LINK_GAP, y, t.text_muted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::ClippedShape;
    use egui::{Color32, Pos2, Sense, Theme, Vec2, pos2, vec2};

    use super::*;
    use crate::components::icon::Icon;
    use crate::components::{ROOM, both, edges_in, fills_in, flat, icon_in, painted, sections, texts};
    use crate::theme::{Text, Tokens};

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

    /// The words drawn within `r`, each with where and in what colour.
    fn words_in(shapes: &[ClippedShape], r: Rect) -> Vec<(String, Rect, Color32)> {
        texts(shapes).into_iter().filter(|(w, at, _)| !w.is_empty() && r.contains_rect(*at)).collect()
    }

    /// `icon` drawn alone in `colour`, centred on `centre`: its colours and the box it fills.
    fn alone(theme: Theme, icon: Icon, centre: Pos2, colour: Color32) -> (Vec<Color32>, Rect) {
        icon_in(&painted(theme, |ui| icon.paint(ui.painter(), centre, colour)), Rect::EVERYTHING)
    }

    /// The 1 px line along the bottom of `r`.
    fn bottom(r: Rect) -> Rect {
        Rect::from_min_max(pos2(r.left(), r.bottom() - 1.0), r.max)
    }

    /// `cell` drawn `w` by 26 matches node 168953: no ground, a 1 px `line` along its bottom, no
    /// edge; `icon` (with its centre's distance from the left) and `words` (each with its left's
    /// distance from the cell's left and its colour), centred down the cell, in `data`.
    fn check_cell(theme: Theme, t: &Tokens, cell: Cell<'_>, w: f32, icon: Option<(Icon, f32, Color32)>, words: &[(&str, f32, Color32)]) {
        let at_ = format!("{theme:?} {cell:?}");
        let (shapes, r) = drawn(theme, vec2(w, 26.0), |ui, rect| cell.paint(ui, rect));
        assert_eq!(fills_in(&shapes, r), [(bottom(r), t.line)], "{at_}: no ground, the line along the bottom");
        assert_eq!(edges_in(&shapes, r, 1.0), [], "{at_}: no edge");
        let slot = Rect::from_min_max(pos2(r.left(), r.top()), pos2(r.left() + 22.0, r.bottom()));
        let want = icon.map_or((vec![], Rect::NOTHING), |(icon, x, colour)| alone(theme, icon, pos2(r.left() + x, r.center().y), colour));
        assert_eq!(icon_in(&shapes, slot), want, "{at_}: the icon");
        let drawn = words_in(&shapes, r);
        let seen: Vec<(&str, Color32)> = drawn.iter().map(|(w, _, c)| (w.as_str(), *c)).collect();
        let wanted: Vec<(&str, Color32)> = words.iter().map(|(w, _, c)| (*w, *c)).collect();
        assert_eq!(seen, wanted, "{at_}: the words, as written");
        for ((w, at, _), (_, x, _)) in drawn.iter().zip(words) {
            assert!(close(at.left(), r.left() + x) && close(at.center().y, r.center().y), "{at_}: `{w}` {x} in, centred: {at:?}");
            assert!(at.right() <= r.right() - 8.0 + 0.5, "{at_}: `{w}` inside the cell's 8: {at:?}");
            assert_eq!(sections(&shapes, w).first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: `{w}` in `data`");
        }
    }

    #[test]
    fn structure_cells_match_the_page() {
        // Nodes 168942 (heading), 168944 (header cell) and 168953 (cell: text, muted, position,
        // nullable, primary key, foreign key), in both modes. None has a ground of its own: the
        // view's table is on `card`.
        for (theme, t) in both() {
            // The heading: 36 tall, the title 12 in, centred, `label` capitals in `text_muted`.
            let (shapes, r) = drawn(theme, vec2(840.0, 36.0), |ui, rect| Heading { title: "Columns" }.paint(ui, rect));
            assert_eq!(fills_in(&shapes, r), [], "{theme:?} heading: no ground, no line");
            let words = words_in(&shapes, r);
            let [(w, at, c)] = words.as_slice() else { panic!("{theme:?} heading: one title: {words:?}") };
            assert_eq!((w.as_str(), *c), ("COLUMNS", t.text_muted), "{theme:?} heading: capitals only drawn");
            assert!(close(at.left(), r.left() + 12.0) && close(at.center().y, r.center().y), "{theme:?} heading: 12 in, centred: {at:?}");
            assert_eq!(sections(&shapes, "COLUMNS").first().map(|s| s.1.clone()), Some(Text::Label.font()), "{theme:?} heading: `label`");

            // The header cell: 30 tall, the title 8 in, centred, `header` in `text`, a 1 px `ink`
            // line along its bottom.
            let (shapes, r) = drawn(theme, vec2(150.0, 30.0), |ui, rect| HeaderCell { title: "Name" }.paint(ui, rect));
            assert_eq!(fills_in(&shapes, r), [(bottom(r), t.ink)], "{theme:?} header cell: the ink line along the bottom");
            let words = words_in(&shapes, r);
            let [(w, at, c)] = words.as_slice() else { panic!("{theme:?} header cell: one title: {words:?}") };
            assert_eq!((w.as_str(), *c), ("Name", t.text), "{theme:?} header cell");
            assert!(close(at.left(), r.left() + 8.0) && close(at.center().y, r.center().y), "{theme:?} header cell: 8 in, centred: {at:?}");
            assert_eq!(sections(&shapes, "Name").first().map(|s| s.1.clone()), Some(Text::Header.font()), "{theme:?} header cell: `header`");

            // The cells: what they hold 8 in (a position 12); a key's icon 8 in, its words 6 after
            // it; a foreign key's arrow 4 after the column it points at.
            let key = Icon::Key.size().x;
            let after_key = 8.0 + key + 6.0;
            check_cell(theme, &t, Cell::Text("customer_id"), 150.0, None, &[("customer_id", 8.0, t.text)]);
            check_cell(theme, &t, Cell::Muted("int8"), 150.0, None, &[("int8", 8.0, t.text_muted)]);
            check_cell(theme, &t, Cell::Muted("'pending'::text"), 150.0, None, &[("'pending'::text", 8.0, t.text_muted)]);
            check_cell(theme, &t, Cell::Muted(""), 76.0, None, &[]);
            check_cell(theme, &t, Cell::Position(2), 40.0, None, &[("2", 12.0, t.text_muted)]);
            check_cell(theme, &t, Cell::Nullable, 76.0, Some((Icon::Check, 8.0 + Icon::Check.size().x / 2.0, t.text_muted)), &[]);
            check_cell(theme, &t, Cell::PrimaryKey, 284.0, Some((Icon::Key, 8.0 + key / 2.0, t.key)), &[(PRIMARY_KEY, after_key, t.text_muted)]);
            let (shapes, r) = drawn(theme, vec2(284.0, 26.0), |ui, rect| Cell::ForeignKey("shop.customers.id").paint(ui, rect));
            let target = words_in(&shapes, r).first().map(|w| w.1).unwrap_or(Rect::NOTHING);
            let arrow = target.right() + 4.0 - r.left();
            check_cell(
                theme,
                &t,
                Cell::ForeignKey("shop.customers.id"),
                284.0,
                Some((Icon::Key, 8.0 + key / 2.0, t.text_muted)),
                &[("shop.customers.id", after_key, t.text_muted), (ARROW, arrow, t.text_muted)],
            );
        }
    }

    #[test]
    fn structure_cells_keep_data_as_written_and_cut_it_inside() {
        // Review Focus 2: a name and a type keep their case; a long one is cut with `…` before
        // the cell's right 8, a foreign key's arrow kept.
        for (theme, t) in both() {
            for (cell, words) in [(Cell::Text("Mood"), "Mood"), (Cell::Muted("enum('small','Large')"), "enum('small','Large')")] {
                let (shapes, r) = drawn(theme, vec2(200.0, 26.0), |ui, rect| cell.paint(ui, rect));
                let drawn: Vec<String> = words_in(&shapes, r).into_iter().map(|w| w.0).collect();
                assert_eq!(drawn, [words], "{theme:?}: as written");
            }
            // Whether the text `whole` was drawn cut.
            let cut = |shapes: &[ClippedShape], whole: &str| {
                flat(shapes).into_iter().any(|s| matches!(s, egui::Shape::Text(t) if t.galley.text() == whole && t.galley.elided))
            };
            let long = "a_column_name_far_too_long_for_its_cell";
            let (shapes, r) = drawn(theme, vec2(120.0, 26.0), |ui, rect| Cell::Text(long).paint(ui, rect));
            let words = words_in(&shapes, r);
            let [(w, at, _)] = words.as_slice() else { panic!("{theme:?}: one text: {words:?}") };
            assert!(cut(&shapes, long) && at.right() <= r.right() - 8.0 + 0.5, "{theme:?}: cut inside: {w} {at:?}");
            let target = "shop.customers_archive.customer_id";
            let (shapes, r) = drawn(theme, vec2(120.0, 26.0), |ui, rect| Cell::ForeignKey(target).paint(ui, rect));
            let words = words_in(&shapes, r);
            let [_, (arrow, at, c)] = words.as_slice() else { panic!("{theme:?}: the target and the arrow: {words:?}") };
            assert!(cut(&shapes, target), "{theme:?}: the target cut");
            assert_eq!((arrow.as_str(), *c), (ARROW, t.text_muted), "{theme:?}: the arrow kept");
            assert!(at.right() <= r.right() - 8.0 + 0.5, "{theme:?}: the arrow inside: {at:?}");
        }
    }
}

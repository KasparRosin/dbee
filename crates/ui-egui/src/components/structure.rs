//! The structure view's parts (`structure/heading`, `structure/header cell`, `structure/cell`):
//! views over plain data that draw into the rect the view gives them, on the view's own ground.
//! [`Table`] lays out one section of the page from them; the view ([`crate::structure_view`]) gives
//! its columns' widths and what each cell shows, and decides what a click does.
//!
//! A section's heading is dbee's words in `label` capitals. The header's titles are `header`
//! over a 1 px `ink` line. A cell is `data` over a 1 px `line`: a column's name in `text`, what is
//! said about it (its type, its default, its position) in `text_muted`, all as written. A link (a
//! foreign key, the table a key comes from) is `text_muted` words and an arrow, no underline:
//! `text` under the pointer with the pointing hand, `text_faint` while off; a click, or Enter or
//! Space with the keyboard, is the view's to act on, and keyboard focus draws a 2 px `ink` box
//! 4 around the words.
//!
//! A table's columns can be resized ([`Table::resize`]) as the data grid's are: the right edge of
//! a titled column, from the header down to the last row, is a handle with the grid's grab room,
//! cursor and line (the theme's hovered and dragged edges). A drag asks for the width under the
//! pointer, a double click for the width the column's content needs ([`Resized`]); the view keeps
//! the widths.

use std::sync::Arc;

use egui::text::TextWrapping;
use egui::{Color32, CornerRadius, CursorIcon, Galley, Painter, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2};

use super::grid_cell::one_line;
use super::icon::Icon;
use super::{focus, painter};
use crate::theme::size::{
    STRUCTURE_CELL_PAD_X, STRUCTURE_HEADER_H, STRUCTURE_HEADING_H, STRUCTURE_HEADING_PAD_X, STRUCTURE_ICON_GAP, STRUCTURE_LINK_GAP,
    STRUCTURE_POSITION_PAD_X, STRUCTURE_ROW_H,
};
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

/// Draws `g` from `x`, centred on `y`; returns where.
fn place(p: &Painter, g: Arc<Galley>, x: f32, y: f32, colour: Color32) -> Rect {
    let at = Rect::from_min_size(pos2(x, y - g.size().y / 2.0), g.size());
    p.galley(at.min, g, colour);
    at
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
    /// The column is a foreign key to this column (`schema.table.column`), as written: a link.
    ForeignKey(&'a str),
    /// A link to a table without the key's icon (`schema.table.column`, `schema.table (a, b)`).
    Link(&'a str),
}

/// How a cell shows: a link at rest, under the pointer, or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Rest,
    Hovered,
    Off,
}

/// A primary key's words.
pub const PRIMARY_KEY: &str = "primary key";
/// What follows the column a foreign key points at.
pub const ARROW: &str = "→";
/// A section's Name column widens for its widest name up to this many times its drawn width.
pub const WIDEN_MAX: f32 = 2.0;
/// Text past this many bytes is measured no further: far wider than a column ever grows.
const MEASURE_BYTES: usize = 512;

/// A column `drawn` wide that holds text `content` wide (its widest): as drawn, or wider to hold
/// the text and its sides' room, at most [`WIDEN_MAX`] times as drawn.
pub fn widened(drawn: f32, content: f32) -> f32 {
    drawn.max(content + 2.0 * STRUCTURE_CELL_PAD_X).min(WIDEN_MAX * drawn).max(0.0)
}

/// A cell as drawn: where its words came out ([`Rect::NOTHING`] for none), and whether they were
/// cut with `…`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Painted {
    pub words: Rect,
    pub cut: bool,
}

impl Cell<'_> {
    /// Whether it is a link.
    pub fn is_link(&self) -> bool {
        matches!(self, Cell::ForeignKey(_) | Cell::Link(_))
    }

    /// How wide what it holds draws, its sides' room left out: its words, icon and arrow.
    pub fn width(&self, ui: &Ui) -> f32 {
        let text = |s: &str| {
            let s = s.get(..s.floor_char_boundary(MEASURE_BYTES)).unwrap_or_default();
            ui.painter().layout_job(Text::Data.job_as_written(s, Color32::PLACEHOLDER)).size().x
        };
        let key = Icon::Key.size().x + STRUCTURE_ICON_GAP;
        let link = |s: &str| text(s) + STRUCTURE_LINK_GAP + text(ARROW);
        match *self {
            Cell::Text(s) | Cell::Muted(s) => text(s),
            Cell::Position(n) => text(&n.to_string()) + STRUCTURE_POSITION_PAD_X - STRUCTURE_CELL_PAD_X,
            Cell::Nullable => Icon::Check.size().x,
            Cell::PrimaryKey => key + text(PRIMARY_KEY),
            Cell::ForeignKey(target) => key + link(target),
            Cell::Link(target) => link(target),
        }
    }

    /// Draws it into `rect`, a link at rest.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        self.paint_as(ui, rect, Look::Rest);
    }

    /// Draws it into `rect`, a link as `look` says (other cells look the same whatever it says);
    /// returns where its words were drawn and whether they were cut.
    pub fn paint_as(&self, ui: &Ui, rect: Rect, look: Look) -> Painted {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, t.line);
        let (x, y, end) = (rect.left() + STRUCTURE_CELL_PAD_X, rect.center().y, rect.right() - STRUCTURE_CELL_PAD_X);
        // `s` in `data` from `x` to `end`, cut with `…`; where it came out.
        let words = |s: &str, x: f32, end: f32, colour: Color32| {
            let g = one_line(ui, s, Text::Data, colour, end - x);
            let cut = g.elided;
            Painted { words: place(&p, g, x, y, colour), cut }
        };
        // The key's icon at the start; where its words start.
        let key = |colour: Color32| {
            Icon::Key.paint(&p, pos2(x + Icon::Key.size().x / 2.0, y), colour);
            x + Icon::Key.size().x + STRUCTURE_ICON_GAP
        };
        let link_colour = match look {
            Look::Rest => t.text_muted,
            Look::Hovered => t.text,
            Look::Off => t.text_faint,
        };
        // A link's words from `x`, then its arrow; where both came out.
        let link = |target: &str, x: f32| {
            let arrow = one_line(ui, ARROW, Text::Data, link_colour, f32::INFINITY);
            let shown = words(target, x, end - STRUCTURE_LINK_GAP - arrow.size().x, link_colour);
            Painted { words: shown.words.union(place(&p, arrow, shown.words.right() + STRUCTURE_LINK_GAP, y, link_colour)), ..shown }
        };
        match *self {
            Cell::Text(s) => words(s, x, end, t.text),
            Cell::Muted(s) => words(s, x, end, t.text_muted),
            Cell::Position(n) => words(&n.to_string(), rect.left() + STRUCTURE_POSITION_PAD_X, end, t.text_muted),
            Cell::Nullable => {
                Icon::Check.paint(&p, pos2(x + Icon::Check.size().x / 2.0, y), t.text_muted);
                Painted { words: Rect::NOTHING, cut: false }
            }
            Cell::PrimaryKey => words(PRIMARY_KEY, key(t.key), end, t.text_muted),
            Cell::ForeignKey(target) => {
                let icon = if look == Look::Off { t.text_faint } else { t.text_muted };
                link(target, key(icon))
            }
            Cell::Link(target) => link(target, x),
        }
    }
}

/// A column of a section's table: its header's title and its width (the last column's is its
/// least: it takes the rest of the table's width).
#[derive(Clone, Copy, Debug)]
pub struct Column<'a> {
    pub title: &'a str,
    pub width: f32,
}

/// A section of the page: its heading, its header and `rows` rows of [`Cell`]s, in columns as
/// wide as `columns` say, the last taking what is left. No ground, no column lines: the header's
/// `ink` line and each row's `line` run the table's full width (the view's width, or `width`, or
/// the columns' when they are wider).
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    pub heading: &'a str,
    pub columns: &'a [Column<'a>],
    pub rows: usize,
    /// Whether its links are on.
    pub links: bool,
    /// The cells that are links, by row then column: they take the keyboard also while scrolled
    /// out of view, and the view scrolls to one that gets it.
    pub link_cells: &'a [(usize, usize)],
    /// The least width it runs: the page's widest table, so that every line runs as far.
    pub width: f32,
    /// Whether the right edge of each titled column is a resize handle.
    pub resize: bool,
}

/// What a resize handle asked for its column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resized {
    /// A drag: this wide (the edge under the pointer); the view keeps it within its bounds.
    To(f32),
    /// A double click: as wide as the column's content needs.
    Fit,
}

/// A section as drawn: where, its heading, its header's cells, and each cell drawn in view with
/// its row and column and whether its words were cut, then each link out of view (not drawn,
/// never cut), in row order.
pub struct TableDrawn {
    pub rect: Rect,
    pub heading: egui::Response,
    pub header: Vec<egui::Response>,
    pub cells: Vec<(usize, usize, egui::Response, bool)>,
    /// Each column's width, as drawn.
    pub widths: Vec<f32>,
    /// Each resize handle drawn, with its column.
    pub handles: Vec<(usize, egui::Response)>,
    /// What a handle asked this frame, with its column.
    pub resized: Option<(usize, Resized)>,
}

impl Table<'_> {
    /// Each column's width in a table `room` wide: as given, the last taking the rest, at least
    /// its own.
    pub fn widths(&self, room: f32) -> Vec<f32> {
        let fixed: f32 = self.columns.iter().rev().skip(1).map(|c| c.width.max(0.0)).sum();
        let last = self.columns.len().saturating_sub(1);
        self.columns.iter().enumerate().map(|(i, c)| if i == last { c.width.max(room - fixed).max(0.0) } else { c.width.max(0.0) }).collect()
    }

    /// The table's height: heading, header and rows.
    pub fn height(&self) -> f32 {
        STRUCTURE_HEADING_H + STRUCTURE_HEADER_H + STRUCTURE_ROW_H * self.rows as f32
    }

    /// Draws it from the top of `ui`'s room, as wide as the room or `width` (or the columns, when
    /// wider), and takes its room. `cell(row, col)` gives what a cell shows; it is asked only for
    /// the cells in view and the links. Every part senses hover; a link's cell, while links are on,
    /// a click too, and takes the keyboard (it reads out as a link), in view or not: the view
    /// scrolls to it when it gets the keyboard. `id` keeps their ids apart.
    pub fn show<'c>(&self, ui: &mut Ui, id: egui::Id, mut cell: impl FnMut(usize, usize) -> Cell<'c>) -> TableDrawn {
        let room = ui.available_width().max(self.width);
        let widths = self.widths(room);
        let width = widths.iter().sum::<f32>().max(room);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, self.height()), Sense::hover());
        let heading_rect = Rect::from_min_size(rect.min, egui::vec2(width, STRUCTURE_HEADING_H));
        let heading = ui.interact(heading_rect, id.with("heading"), Sense::hover());
        let clip = ui.clip_rect();
        if clip.intersects(heading_rect) {
            Heading { title: self.heading }.paint(ui, heading_rect);
        }
        // The cells of a band from `top`, `h` tall: their rects, left to right.
        let band = |top: f32, h: f32| {
            let mut x = rect.left();
            widths
                .iter()
                .map(move |w| {
                    let r = Rect::from_min_size(pos2(x, top), egui::vec2(*w, h));
                    x += w;
                    r
                })
                .collect::<Vec<Rect>>()
        };
        let header_top = heading_rect.bottom();
        let mut header = Vec::with_capacity(self.columns.len());
        for (col, (c, r)) in self.columns.iter().zip(band(header_top, STRUCTURE_HEADER_H)).enumerate() {
            if clip.intersects(r) {
                HeaderCell { title: c.title }.paint(ui, r);
            }
            header.push(ui.interact(r, id.with(("header", col)), Sense::hover()));
        }
        let rows_top = header_top + STRUCTURE_HEADER_H;
        // Only the rows in view: a table can have thousands.
        let first = ((clip.top() - rows_top) / STRUCTURE_ROW_H).floor().max(0.0);
        let last = ((clip.bottom() - rows_top) / STRUCTURE_ROW_H).ceil().max(0.0);
        let (first, last) = if first.is_finite() && last.is_finite() { (first as usize, (last as usize).min(self.rows)) } else { (0, 0) };
        let mut cells = Vec::new();
        // A link's cell senses, reads out, and is scrolled to when it gets the keyboard.
        let sense_link = |ui: &Ui, c: Cell<'_>, r: Rect, row: usize, col: usize| {
            let on = self.links;
            let response = ui.interact(r, id.with((row, col)), if on { Sense::click() } else { Sense::hover() });
            if let Cell::ForeignKey(target) | Cell::Link(target) = c {
                response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, on, format!("{target} {ARROW}")));
            }
            if on && response.gained_focus() {
                ui.scroll_to_rect(r, None);
            }
            response
        };
        let cell_rect = |row: usize, col: usize| {
            let top = rows_top + STRUCTURE_ROW_H * row as f32;
            band(top, STRUCTURE_ROW_H).get(col).copied()
        };
        // The links above the view, in order (they come before it for the keyboard).
        let mut links = self.link_cells.iter().copied().filter(|&(row, _)| row < self.rows).peekable();
        while let Some((row, col)) = links.next_if(|&(row, _)| row < first) {
            if let Some(r) = cell_rect(row, col) {
                let c = cell(row, col);
                cells.push((row, col, sense_link(ui, c, r, row, col), false));
            }
        }
        for row in first..last {
            let top = rows_top + STRUCTURE_ROW_H * row as f32;
            for (col, r) in band(top, STRUCTURE_ROW_H).into_iter().enumerate() {
                let c = cell(row, col);
                let link = c.is_link();
                if !clip.intersects(r) {
                    // Out of view sideways: a link still takes the keyboard.
                    if link {
                        cells.push((row, col, sense_link(ui, c, r, row, col), false));
                    }
                    continue;
                }
                let on = link && self.links;
                let response = if link { sense_link(ui, c, r, row, col) } else { ui.interact(r, id.with((row, col)), Sense::hover()) };
                let look = match (link, on) {
                    (true, false) => Look::Off,
                    (true, true) if response.hovered() => Look::Hovered,
                    _ => Look::Rest,
                };
                let painted = c.paint_as(ui, r, look);
                if on && response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if on && focus::shown(ui, &response) {
                    focus::box_around(&painter(ui), painted.words);
                }
                cells.push((row, col, response, painted.cut));
            }
        }
        // The links below the view.
        for (row, col) in links.filter(|&(row, _)| row >= last) {
            if let Some(r) = cell_rect(row, col) {
                let c = cell(row, col);
                cells.push((row, col, sense_link(ui, c, r, row, col), false));
            }
        }
        // The resize handles, after the cells so that they take the pointer on a column's edge.
        let mut handles = Vec::new();
        let mut resized = None;
        if self.resize {
            let grab = ui.style().interaction.resize_grab_radius_side;
            let pressed_elsewhere = ui.input(|i| i.pointer.any_down() || i.pointer.any_pressed());
            for (col, (c, r)) in self.columns.iter().zip(band(header_top, rect.bottom() - header_top)).enumerate() {
                if c.title.is_empty() {
                    continue;
                }
                let x = r.right();
                let line = Rect::from_min_max(pos2(x, r.top()), pos2(x, r.bottom()));
                // The pointer's alone: Tab goes from link to link, past the handles.
                let response = ui.interact(line.expand2(egui::vec2(grab, 0.0)), id.with(("resize", col)), Sense::CLICK | Sense::DRAG);
                if response.double_clicked() {
                    resized = Some((col, Resized::Fit));
                } else if response.dragged()
                    && let Some(pointer) = ui.ctx().pointer_latest_pos()
                {
                    resized = Some((col, Resized::To(r.width() + pointer.x - x)));
                }
                let hovered = response.hovered() && !pressed_elsewhere;
                if hovered || response.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn);
                    let widgets = &ui.style().visuals.widgets;
                    let stroke = if response.dragged() { widgets.active.bg_stroke } else { widgets.hovered.bg_stroke };
                    painter(ui).line_segment([line.min, line.max], stroke);
                }
                handles.push((col, response));
            }
        }
        TableDrawn { rect, heading, header, cells, widths, handles, resized }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::ClippedShape;
    use egui::{Color32, Pos2, Sense, Theme, Vec2, pos2, vec2};

    use super::*;
    use crate::components::icon::Icon;
    use crate::components::{ROOM, both, edges_in, fills_in, flat, icon_in, painted, painted_in, sections, texts};
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
    #[test]
    fn a_link_without_its_key_icon() {
        // Brief §3.7: FOREIGN KEYS' and REFERENCED BY's links are COLUMNS' foreign key without
        // its icon: the words and the arrow in `text_muted`, `text` while hovered, `text_faint`
        // while off.
        for (theme, t) in both() {
            let link = Cell::Link("shop.customers.id");
            let (shapes, r) = drawn(theme, vec2(284.0, 26.0), |ui, rect| link.paint(ui, rect));
            let target = words_in(&shapes, r).first().map(|w| w.1).unwrap_or(Rect::NOTHING);
            let arrow = target.right() + 4.0 - r.left();
            check_cell(theme, &t, link, 284.0, None, &[("shop.customers.id", 8.0, t.text_muted), (ARROW, arrow, t.text_muted)]);
            for (look, colour) in [(Look::Hovered, t.text), (Look::Off, t.text_faint)] {
                let at = std::cell::Cell::new(Rect::NOTHING);
                let (shapes, r) = drawn(theme, vec2(284.0, 26.0), |ui, rect| at.set(link.paint_as(ui, rect, look).words));
                let at = at.get();
                let words = words_in(&shapes, r);
                let seen: Vec<(&str, Color32)> = words.iter().map(|(w, _, c)| (w.as_str(), *c)).collect();
                assert_eq!(seen, [("shop.customers.id", colour), (ARROW, colour)], "{theme:?} {look:?}");
                assert_eq!(icon_in(&shapes, Rect::from_min_max(r.min, pos2(r.left() + 22.0, r.bottom()))).0, vec![], "{theme:?} {look:?}: no icon");
                let all = words.iter().fold(Rect::NOTHING, |a, w| a.union(w.1));
                assert!(close(at.left(), all.left()) && close(at.right(), all.right()), "{theme:?} {look:?}: the words' rect {at:?}, drawn {all:?}");
            }
        }
    }

    #[test]
    fn a_column_widens_for_its_text_up_to_twice() {
        // Brief §2.2, call 6: a Name column takes `max(drawn, widest + 16)`, at most twice as drawn.
        assert_eq!(widened(150.0, 100.0), 150.0);
        assert_eq!(widened(150.0, 144.0), 160.0);
        assert_eq!(widened(150.0, 432.0), 300.0);
        assert_eq!(widened(150.0, f32::INFINITY), 300.0);
        // A cell says whether its words were cut, and how wide they draw whole.
        let long = "a_column_name_far_too_long_for_its_cell";
        let seen = std::cell::Cell::new(None);
        let _ = painted(Theme::Light, |ui| {
            ui.add_space(ROOM);
            let (rect, _) = ui.allocate_exact_size(vec2(120.0, 26.0), Sense::hover());
            seen.set(Some((Cell::Text(long).paint_as(ui, rect, Look::Rest).cut, Cell::Text("id").paint_as(ui, rect, Look::Rest).cut)));
            let whole = ui.painter().layout_job(Text::Data.job_as_written(long, Color32::PLACEHOLDER)).size().x;
            assert!(close(Cell::Text(long).width(ui), whole), "the whole text's width");
            let arrow = ui.painter().layout_job(Text::Data.job_as_written(ARROW, Color32::PLACEHOLDER)).size().x;
            assert!(close(Cell::Link(long).width(ui), whole + 4.0 + arrow), "a link's words and arrow");
        });
        assert_eq!(seen.get(), Some((true, false)), "the long one cut, the short one whole");
    }

    #[test]
    fn a_table_draws_its_lines_the_full_width() {
        // SA: the heading 36 tall, the header 30, each row 26; the header's `ink` line and every
        // row's `line` run the table's full width, past its last column's minimum; no column lines.
        let columns = [
            Column { title: "", width: 40.0 },
            Column { title: "Name", width: 150.0 },
            Column { title: "Type", width: 140.0 },
            Column { title: "Keys", width: 226.0 },
        ];
        for (theme, t) in both() {
            let mut at = Rect::NOTHING;
            let mut cells = 0;
            let shapes = painted_in(vec2(1000.0, 400.0), theme, |ui| {
                ui.add_space(ROOM);
                let table = Table { heading: "Columns", columns: &columns, rows: 3, links: true, link_cells: &[], width: 0.0, resize: false };
                let shown = ui
                    .allocate_ui(vec2(900.0, 300.0), |ui| {
                        table.show(ui, egui::Id::new("t"), |row, col| match col {
                            0 => Cell::Position(row + 1),
                            1 => Cell::Text("id"),
                            2 => Cell::Muted("int8"),
                            _ => Cell::Muted(""),
                        })
                    })
                    .inner;
                at = shown.rect;
                cells = shown.cells.len();
            });
            assert_eq!(at.size(), vec2(900.0, 36.0 + 30.0 + 3.0 * 26.0), "{theme:?}: the table's size");
            assert_eq!(cells, 12, "{theme:?}: a response for each cell");
            let fills = fills_in(&shapes, at);
            // Each line's pieces along `y`, joined: they must run from the left to the right.
            let runs = |colour: Color32, y: f32| {
                let mut xs: Vec<(f32, f32)> = fills
                    .iter()
                    .filter(|(r, c)| *c == colour && close(r.height(), 1.0) && close(r.bottom(), y))
                    .map(|(r, _)| (r.left(), r.right()))
                    .collect();
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut joined: Vec<(f32, f32)> = Vec::new();
                for (l, r) in xs {
                    match joined.last_mut() {
                        Some(last) if l <= last.1 + 0.5 => last.1 = last.1.max(r),
                        _ => joined.push((l, r)),
                    }
                }
                joined
            };
            let top = at.top();
            assert_eq!(runs(t.ink, top + 66.0), [(at.left(), at.right())], "{theme:?}: the header's ink line, the full width");
            for row in 1..=3 {
                let y = top + 66.0 + 26.0 * row as f32;
                assert_eq!(runs(t.line, y), [(at.left(), at.right())], "{theme:?}: row {row}'s line, the full width");
            }
            let lines: Vec<f32> = fills.iter().filter(|(_, c)| *c == t.line || *c == t.ink).map(|(r, _)| r.bottom()).collect();
            let wanted = [66.0, 92.0, 118.0, 144.0].map(|y| top + y);
            assert!(lines.iter().all(|y| wanted.iter().any(|w| close(*y, *w))), "{theme:?}: lines only under the header and the rows: {lines:?}");
            assert!(fills.iter().all(|(r, _)| r.height() <= 1.0 + 0.01), "{theme:?}: no column lines and no grounds: {fills:?}");
            // The heading's words centred in its 36, the header's in its 30.
            let words = words_in(&shapes, at);
            let centre = |w: &str| words.iter().find(|(s, _, _)| s == w).map(|(_, r, _)| r.center().y).unwrap_or(f32::NAN);
            assert!(close(centre("COLUMNS"), top + 18.0), "{theme:?}: the heading 36 tall: {words:?}");
            assert!(close(centre("Name"), top + 36.0 + 15.0), "{theme:?}: the header 30 tall");
            assert!(close(centre("int8"), top + 66.0 + 13.0), "{theme:?}: a row 26 tall");
        }
    }
}

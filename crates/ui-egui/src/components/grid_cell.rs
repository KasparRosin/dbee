//! The data grid's cells (`grid/header cell`, `grid/cell`, `grid/row number`), and what fills a
//! row and the header past the last column ([`Filler`], [`header_filler`]). Each is a view over
//! plain data that draws into the rect the grid gives it; the grid ([`crate::grid`]) keeps its
//! layout, scrolling and input, and a cell's follow link is the one part that answers a click.
//!
//! One ground at a time (foundations): a marked cell (staged, new, deleted, failed, saved) keeps
//! its mark when selected or hovered, with nothing under it, and when selected takes an outline,
//! `staged_strong` on a staged cell and `ink` on the others. A selected cell doesn't change on
//! hover; the others light up with their whole row. The saved mark fades out: its ground thins
//! to nothing ([`Cell::strength`]), and the grid then draws the cell without it.
//!
//! Data keeps its case and its kind: a cell's text and a column's name and type are drawn as
//! written, and NULL ([`CellText::Null`]) is the word in `text_muted`, never the text `NULL`.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Galley, Id, Painter, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2};

pub use dbee_sql::filter::SortDir as Sort;

use super::icon::Icon;
use super::painter;
use crate::theme::size::{
    GRID_CELL_PAD_X, GRID_FAILED_MARK, GRID_HEADER_GAP, GRID_LINK_GAP, GRID_OUTLINE_W, GRID_ROW_NUMBER_MIN_W, GRID_SORT_HINT_GAP,
};
use crate::theme::{Text, Tokens, tokens};
use crate::tree::thousands;

/// The mark `display::grid_text` puts for a line break, drawn as `icon/line-break`.
pub const BREAK: char = '↵';

/// What a character of `data` takes (foundations: 7.2 at 12 px), for widths worked out before
/// anything is laid out.
pub const DATA_CHAR_W: f32 = 7.2;

/// What a cell holds besides its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A staged change.
    Staged,
    /// A row to insert.
    New,
    /// A row to delete.
    Deleted,
    /// The row the last Apply failed on.
    Failed,
    /// Just written by Apply: at full for [`SAVED_HOLD`], then fading out over [`SAVED_FADE`]
    /// ([`saved_strength`]).
    Saved,
}

/// How long, in seconds, a saved mark shows at full from the frame that first shows it.
pub const SAVED_HOLD: f64 = 1.0;

/// How long, in seconds, it then takes to fade out.
pub const SAVED_FADE: f64 = 0.5;

/// How strongly a saved mark shows `elapsed` seconds after it first showed: 1 for [`SAVED_HOLD`],
/// then down to 0 over [`SAVED_FADE`], and 0 from then on (also for a time that is no number).
pub fn saved_strength(elapsed: f64) -> f32 {
    match elapsed {
        e if e < SAVED_HOLD => 1.0,
        e if e < SAVED_HOLD + SAVED_FADE => (1.0 - (e - SAVED_HOLD) / SAVED_FADE) as f32,
        _ => 0.0,
    }
}

impl Mark {
    /// The `state` a cell's automation value names.
    pub fn id(self) -> &'static str {
        match self {
            Mark::Staged => "staged",
            Mark::New => "new",
            Mark::Deleted => "deleted",
            Mark::Failed => "failed",
            Mark::Saved => "saved",
        }
    }

    fn ground(self, t: &Tokens) -> Color32 {
        match self {
            Mark::Staged => t.staged,
            Mark::New => t.new_row,
            Mark::Deleted => t.deleted,
            Mark::Failed => t.failed,
            Mark::Saved => t.saved,
        }
    }

    /// The outline it takes when selected.
    fn outline(self, t: &Tokens) -> Color32 {
        if self == Mark::Staged { t.staged_strong } else { t.ink }
    }
}

/// A column's key, shown in its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Primary,
    Foreign,
}

impl Key {
    /// The `key` a header's automation value names.
    pub fn id(self) -> &'static str {
        match self {
            Key::Primary => "primary",
            Key::Foreign => "foreign",
        }
    }
}

/// Where a column's values sit: numbers right, the rest left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Right,
}

/// What a cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellText<'a> {
    /// The value as the grid shows it (`display::grid_text`): one line, a line break as [`BREAK`].
    Text(&'a str),
    Null,
}

/// A column's header cell: its key, name and type, and its sort.
#[derive(Clone, Copy, Debug)]
pub struct Header<'a> {
    pub name: &'a str,
    pub ty: &'a str,
    pub key: Option<Key>,
    pub sort: Option<Sort>,
    /// A number's header sits right, as its values do; a key's stays left.
    pub align: Align,
    /// A click sorts by it: hovered, it shows both arrows.
    pub sortable: bool,
    pub hovered: bool,
}

impl Header<'_> {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        head_ground(&p, rect, t);
        let (pad, gap) = (GRID_CELL_PAD_X, GRID_HEADER_GAP);
        let key_w = if self.key.is_some() { Icon::Key.size().x + gap } else { 0.0 };
        let arrow = Icon::SortAscending.size().x;
        let hint = self.sortable && self.hovered && self.sort.is_none();
        let to_right = self.align == Align::Right && self.key.is_none();
        // As on the page: a number's hint takes its room in the run, moving the name; elsewhere it
        // follows the type, into the padding if it must, and the name and type stay put.
        let sort_w = match (self.sort, hint) {
            (Some(_), _) => gap + arrow,
            (None, true) if to_right => gap + 2.0 * arrow + GRID_SORT_HINT_GAP,
            _ => 0.0,
        };
        let room = (rect.width() - 2.0 * pad).max(0.0);
        let name = one_line(ui, self.name, Text::Header, t.text, room - key_w - sort_w);
        let left = room - key_w - sort_w - name.size().x - gap;
        let ty = (left > 12.0).then(|| one_line(ui, self.ty, Text::Data, t.text_muted, left));
        let used = key_w + name.size().x + ty.as_ref().map_or(0.0, |g| gap + g.size().x) + sort_w;
        let right = to_right && used <= room;
        let mut x = if right { rect.right() - pad - used } else { rect.left() + pad };
        let y = rect.center().y;
        if let Some(key) = self.key {
            let colour = if key == Key::Primary { t.key } else { t.text_muted };
            Icon::Key.paint(&p, pos2(x + Icon::Key.size().x / 2.0, y), colour);
            x += key_w;
        }
        x += place(&p, name, x, y, t.text);
        if let Some(ty) = ty {
            x += gap;
            x += place(&p, ty, x, y, t.text_muted);
        }
        x += gap;
        match self.sort {
            Some(dir) => {
                let icon = if dir == Sort::Asc { Icon::SortAscending } else { Icon::SortDescending };
                icon.paint(&p, pos2(x + arrow / 2.0, y), t.ink);
            }
            None if hint => {
                Icon::SortAscending.paint(&p, pos2(x + arrow / 2.0, y), t.text_muted);
                Icon::SortDescending.paint(&p, pos2(x + arrow * 1.5 + GRID_SORT_HINT_GAP, y), t.text_muted);
            }
            None => {}
        }
    }
}

/// The header's corner over the row numbers.
pub fn corner(ui: &Ui, rect: Rect) {
    head_ground(&painter(ui), rect, tokens(ui.ctx()));
}

/// The header past the last column, to the grid's right edge: `card` and the 1 px `ink` line
/// along its bottom, with no line at its right.
pub fn header_filler(ui: &Ui, rect: Rect) {
    let t = tokens(ui.ctx());
    let p = painter(ui);
    p.rect_filled(rect, CornerRadius::ZERO, t.card);
    p.rect_filled(bottom_line(rect), CornerRadius::ZERO, t.ink);
}

/// A header cell's ground: `card`, a 1 px `line` at its right and a 1 px `ink` line along its
/// bottom.
fn head_ground(p: &Painter, rect: Rect, t: &Tokens) {
    p.rect_filled(rect, CornerRadius::ZERO, t.card);
    p.rect_filled(Rect::from_min_max(pos2(rect.right() - 1.0, rect.top()), rect.max), CornerRadius::ZERO, t.line);
    p.rect_filled(bottom_line(rect), CornerRadius::ZERO, t.ink);
}

/// The 1 px along `rect`'s bottom.
fn bottom_line(rect: Rect) -> Rect {
    Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max)
}

/// Draws `g` from `x`, centred on `y`; returns its width.
fn place(p: &Painter, g: Arc<Galley>, x: f32, y: f32, colour: Color32) -> f32 {
    let size = g.size();
    p.galley(pos2(x, y - size.y / 2.0), g, colour);
    size.x
}

/// The 1 px `colour` lines inside `rect`'s right and bottom: a cell's edges.
fn edges(p: &Painter, rect: Rect, colour: Color32) {
    p.rect_filled(Rect::from_min_max(pos2(rect.right() - 1.0, rect.top()), rect.max), CornerRadius::ZERO, colour);
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), CornerRadius::ZERO, colour);
}

/// `colour` at `strength` of its opacity: whole from 1 (and for no number), nothing from 0 down.
fn faded(colour: Color32, strength: f32) -> Color32 {
    if strength >= 1.0 || strength.is_nan() {
        colour
    } else if strength > 0.0 {
        colour.gamma_multiply(strength)
    } else {
        Color32::TRANSPARENT
    }
}

/// The 1 px `colour` line across `rect`'s middle: a row to delete.
fn strike(p: &Painter, rect: Rect, colour: Color32) {
    let y = rect.center().y;
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), y - 0.5), pos2(rect.right(), y + 0.5)), CornerRadius::ZERO, colour);
}

/// A data cell.
#[derive(Clone, Copy, Debug)]
pub struct Cell<'a> {
    pub text: CellText<'a>,
    pub align: Align,
    pub mark: Option<Mark>,
    /// In the selection.
    pub selected: bool,
    /// The selection's focus cell.
    pub focused: bool,
    /// Its row is to be deleted: the text in `text_muted`, a line across.
    pub deleted: bool,
    /// A foreign key that can be followed: its link shows while its row is selected.
    pub link: bool,
    /// Its row holds the selection's focus.
    pub row_selected: bool,
    /// Its row is under the pointer.
    pub hovered: bool,
    /// How strongly its mark's ground shows, 1 at full: a saved mark fades out
    /// ([`saved_strength`]). Its outline, when selected, stays whole.
    pub strength: f32,
}

impl Cell<'_> {
    /// Draws it into `rect`; the follow link's response, under `id`, when it shows.
    pub fn show(&self, ui: &Ui, rect: Rect, id: Id) -> Option<Response> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let ground = match self.mark {
            Some(mark) => faded(mark.ground(t), self.strength),
            None if self.selected || self.focused => t.selection,
            None if self.hovered => t.hover,
            None => t.card,
        };
        p.rect_filled(rect, CornerRadius::ZERO, ground);
        let pad = GRID_CELL_PAD_X;
        let y = rect.center().y;
        let link = (self.link && self.row_selected && self.text != CellText::Null).then(|| {
            let size = Icon::Follow.size();
            Rect::from_min_size(pos2(rect.right() - pad - size.x, y - size.y / 2.0), size)
        });
        let right = link.map_or(rect.right() - pad, |l| l.left() - GRID_LINK_GAP);
        let (words, colour) = match self.text {
            CellText::Null => ("NULL", t.text_muted),
            CellText::Text(s) => (s, if self.deleted { t.text_muted } else { t.text }),
        };
        if !words.is_empty() {
            let x = if self.align == Align::Right && !words.contains(BREAK) {
                // Measured first, to end at the right.
                right - one_line(ui, words, Text::Data, colour, right - rect.left() - pad).size().x
            } else {
                rect.left() + pad
            };
            paint_text(ui, &p, words, (x, y, right), colour);
        }
        if self.deleted {
            strike(&p, rect, t.text_muted);
        }
        let link = link.map(|at| {
            let r = ui.interact(at, id, Sense::click());
            Icon::Follow.paint(&p, at.center(), t.text_muted);
            r
        });
        let failed = self.mark == Some(Mark::Failed);
        if failed {
            let side = GRID_FAILED_MARK;
            let corner = vec![pos2(rect.right() - side, rect.top()), rect.right_top(), pos2(rect.right(), rect.top() + side)];
            p.add(Shape::convex_polygon(corner, t.danger, Stroke::NONE));
        }
        if self.focused || (self.selected && self.mark.is_some()) {
            let colour = self.mark.map_or(t.ink, |m| m.outline(t));
            p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(GRID_OUTLINE_W, colour), StrokeKind::Inside);
        } else if failed {
            p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.danger), StrokeKind::Inside);
        } else {
            edges(&p, rect, t.line);
        }
        link
    }
}

/// A row past its last column, to the grid's right edge: the row's ground, with its 1 px `line`
/// along the bottom and none at its right.
#[derive(Clone, Copy, Debug)]
pub struct Filler {
    /// The row is lit as selected: `selection`.
    pub selected: bool,
    /// The row is under the pointer: `hover`, unless selected.
    pub hovered: bool,
}

impl Filler {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let ground = if self.selected {
            t.selection
        } else if self.hovered {
            t.hover
        } else {
            t.card
        };
        p.rect_filled(rect, CornerRadius::ZERO, ground);
        p.rect_filled(bottom_line(rect), CornerRadius::ZERO, t.line);
    }
}

/// A row's number, at the grid's left.
#[derive(Clone, Copy, Debug)]
pub struct RowNumber {
    /// The number shown: a page's offset counted in.
    pub n: u64,
    /// A row to insert: `*` in the number's place.
    pub new: bool,
    /// A row to delete: a line across.
    pub deleted: bool,
    /// Some of its cells are selected.
    pub selected: bool,
    /// The whole row is selected (issue #17): its number in `text`.
    pub whole: bool,
    /// Its row is under the pointer.
    pub hovered: bool,
}

impl RowNumber {
    /// Draws it into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let ground = if self.new {
            t.new_row
        } else if self.deleted {
            t.deleted
        } else if self.selected || self.whole {
            t.selection
        } else if self.hovered {
            t.hover
        } else {
            t.card
        };
        p.rect_filled(rect, CornerRadius::ZERO, ground);
        edges(&p, rect, t.line);
        let words = if self.new { "*".to_owned() } else { thousands(self.n) };
        let colour = if self.whole { t.text } else { t.text_muted };
        let g = one_line(ui, &words, Text::Data, colour, rect.width() - 2.0 * GRID_CELL_PAD_X);
        let x = rect.right() - GRID_CELL_PAD_X - g.size().x;
        place(&p, g, x, rect.center().y, colour);
        if self.deleted {
            strike(&p, rect, t.text_muted);
        }
    }

    /// The row numbers' width, for numbers up to `last`.
    pub fn width(last: u64) -> f32 {
        let chars = thousands(last).chars().count() as f32;
        (chars * DATA_CHAR_W + 2.0 * GRID_CELL_PAD_X).ceil().max(GRID_ROW_NUMBER_MIN_W)
    }
}

/// Paints `text` in `data` on one line from `x`, centred on `y`, cut with `…` and clipped at
/// `right`, each [`BREAK`] drawn as `icon/line-break`. Returns how wide it came out.
fn paint_text(ui: &Ui, p: &Painter, text: &str, (x, y, right): (f32, f32, f32), colour: Color32) -> f32 {
    // A part cut to a sliver still lays out a whole `…`: nothing goes past `right`.
    let clip = p.clip_rect();
    let p = &p.with_clip_rect(clip.intersect(Rect::from_min_max(clip.min, pos2(right, clip.max.y))));
    let mark = Icon::LineBreak.size().x;
    let mut at = x;
    for (i, part) in text.split(BREAK).enumerate() {
        if i > 0 {
            if at + mark > right {
                place(p, one_line(ui, "…", Text::Data, colour, right - at), at, y, colour);
                break;
            }
            Icon::LineBreak.paint(p, pos2(at + mark / 2.0, y), colour);
            at += mark;
        }
        if part.is_empty() {
            continue;
        }
        let g = one_line(ui, part, Text::Data, colour, right - at);
        let cut = g.elided;
        at += place(p, g, at, y, colour);
        if cut {
            break;
        }
    }
    (at.min(right) - x).max(0.0)
}

/// `text` in `style` as written, on one line cut with `…` at `width`.
pub(super) fn one_line(ui: &Ui, text: &str, style: Text, colour: Color32, width: f32) -> Arc<Galley> {
    let mut job = style.job_as_written(text, colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use dbee_core::{Column as Col, ResultSet, TypeClass};
    use dbee_engine::Selection;
    use egui::epaint::ClippedShape;
    use egui::{CornerRadius, Pos2, Sense, Shape, Theme, pos2, vec2};

    use super::*;
    use crate::automation::Tags;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness_in, icon_in, painted, sections, texts};
    use crate::grid::{Grid, GridAct, GridSource, GridState, RowEdits};
    use crate::theme::Text;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// A plain cell of the page's (`grid/cell` 166898: 150 by 26).
    fn plain(text: CellText<'_>) -> Cell<'_> {
        Cell {
            text,
            align: Align::Left,
            mark: None,
            selected: false,
            focused: false,
            deleted: false,
            link: false,
            row_selected: false,
            hovered: false,
            strength: 1.0,
        }
    }

    #[test]
    fn the_saved_fade_is_full_for_one_second_then_gone_after_one_and_a_half() {
        assert_eq!((saved_strength(0.0), saved_strength(0.99)), (1.0, 1.0), "full for a second");
        let between = saved_strength(1.25);
        assert!(between > 0.0 && between < 1.0, "fading: {between}");
        assert_eq!(saved_strength(1.5), 0.0, "gone after a second and a half");
        assert!(saved_strength(1.1) > saved_strength(1.25) && saved_strength(1.25) > saved_strength(1.4), "fading out evenly");
        // A clock that went back shows it whole; long after, or on a time that is no number, none.
        assert_eq!((saved_strength(-3.0), saved_strength(60.0), saved_strength(f64::NAN)), (1.0, 0.0, 0.0));
    }

    #[test]
    fn a_fading_saved_cell_thins_its_ground_and_keeps_its_outline() {
        // Its ground alone, thinner as it fades; selected, the whole `ink` outline. At 0 nothing.
        for (theme, t) in both() {
            for strength in [0.75, 0.25, 0.0] {
                for selected in [false, true] {
                    let at_ = format!("{theme:?} {strength} selected {selected}");
                    let cell = Cell { mark: Some(Mark::Saved), selected, strength, ..plain(CellText::Text("7.25")) };
                    let (shapes, r, _) = cell_drawn(theme, cell);
                    let s = seen(&shapes, r);
                    let grounds: Vec<(Rect, Color32)> = s.fills.iter().filter(|(at, _)| *at == r).copied().collect();
                    let want: Vec<(Rect, Color32)> = if strength > 0.0 { vec![(r, t.saved.gamma_multiply(strength))] } else { vec![] };
                    assert_eq!(grounds, want, "{at_}: its ground, alone");
                    assert!(want.iter().all(|(_, c)| c.a() > 0 && c.a() < t.saved.a()), "{at_}: thinner than at full: {want:?}");
                    let outline = Edge { outer: r, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
                    assert_eq!(s.edges, if selected { vec![outline] } else { vec![] }, "{at_}: the outline");
                }
            }
        }
    }

    /// One frame of `cell` drawn into a 150 by 26 rect: the shapes, the rect and the link's rect.
    fn cell_drawn(theme: Theme, cell: Cell<'_>) -> (Vec<ClippedShape>, Rect, Option<Rect>) {
        let (mut at, mut link) = (Rect::NOTHING, None);
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            let (rect, _) = ui.allocate_exact_size(vec2(150.0, 26.0), Sense::hover());
            at = rect;
            link = cell.show(ui, rect, Id::new("test-link")).map(|r| r.rect);
        });
        (shapes, at, link)
    }

    /// The right and bottom 1 px edges of `r`, as the page's inside strokes.
    fn lines(r: Rect) -> [Rect; 2] {
        [Rect::from_min_max(pos2(r.right() - 1.0, r.top()), r.max), Rect::from_min_max(pos2(r.left(), r.bottom() - 1.0), r.max)]
    }

    /// The 1 px line across `r`'s middle.
    fn strike(r: Rect) -> Rect {
        Rect::from_min_max(pos2(r.left(), r.center().y - 0.5), pos2(r.right(), r.center().y + 0.5))
    }

    /// What a cell drew in `r`: its fills (the ground first), its edges, its texts.
    struct Seen {
        fills: Vec<(Rect, Color32)>,
        edges: Vec<Edge>,
        texts: Vec<(String, Rect, Color32)>,
    }

    fn seen(shapes: &[ClippedShape], r: Rect) -> Seen {
        let texts = texts(shapes).into_iter().filter(|(w, at, _)| !w.is_empty() && r.contains_rect(*at)).collect();
        Seen { fills: fills_in(shapes, r), edges: edges_in(shapes, r, 0.0), texts }
    }

    /// The failed mark: a `danger` triangle in `r`'s top right corner, 6 along each side.
    fn failed_mark(shapes: &[ClippedShape], r: Rect, t: &Tokens) -> bool {
        let want = [pos2(r.right() - 6.0, r.top()), r.right_top(), pos2(r.right(), r.top() + 6.0)];
        flat(shapes).iter().any(|s| matches!(s, Shape::Path(p) if p.fill == t.danger && p.points.len() == 3 && want.iter().all(|w| p.points.iter().any(|q| (*q - *w).length() < 0.01))))
    }

    #[test]
    fn each_cell_state_matches_the_page() {
        // Node 166898: 26 tall, its value in `data`, 8 in from its sides and centred. Plain on
        // `card`, hovered (its row) `hover`, selected `selection`, each with a 1 px `line` at its
        // right and bottom; the focus cell `selection` with a 2 px `ink` outline inside and no
        // line. A mark is its own ground; selected, a 2 px outline, `staged_strong` on a staged
        // cell and `ink` on the others. Failed has a 1 px `danger` edge all round and a 6 px
        // `danger` triangle in its top right corner. A row to delete: the text `text_muted` and a
        // 1 px `text_muted` line across the whole cell. NULL is the word in `text_muted`, regular;
        // an empty value shows nothing; numbers sit right. A line break is `icon/line-break`, 10
        // wide, between the parts; the follow link is `icon/follow` in `text_muted`, 14 square 8
        // from the right, the text ending 6 before it.
        for (theme, t) in both() {
            let marks =
                [(Mark::Staged, t.staged), (Mark::New, t.new_row), (Mark::Deleted, t.deleted), (Mark::Saved, t.saved), (Mark::Failed, t.failed)];
            let text = CellText::Text("Customer 191");
            let mut states: Vec<(String, Cell<'_>, Color32, Option<(f32, Color32)>)> = vec![
                ("plain".into(), plain(text), t.card, None),
                ("hover".into(), Cell { hovered: true, ..plain(text) }, t.hover, None),
                ("selected".into(), Cell { selected: true, ..plain(text) }, t.selection, None),
                ("focus".into(), Cell { selected: true, focused: true, ..plain(text) }, t.selection, Some((2.0, t.ink))),
            ];
            for (mark, ground) in marks {
                let edge = (mark == Mark::Failed).then_some((1.0, t.danger));
                states.push((format!("{mark:?}"), Cell { mark: Some(mark), ..plain(text) }, ground, edge));
                let outline = if mark == Mark::Staged { t.staged_strong } else { t.ink };
                states.push((format!("{mark:?} selected"), Cell { mark: Some(mark), selected: true, ..plain(text) }, ground, Some((2.0, outline))));
                states.push((
                    format!("{mark:?} focus"),
                    Cell { mark: Some(mark), selected: true, focused: true, ..plain(text) },
                    ground,
                    Some((2.0, outline)),
                ));
            }
            for (state, cell, ground, edge) in states {
                let at_ = format!("{theme:?} {state}");
                let (shapes, r, link) = cell_drawn(theme, cell);
                let s = seen(&shapes, r);
                let mut want = vec![(r, ground)];
                if edge.is_none() {
                    want.extend(lines(r).map(|l| (l, t.line)));
                }
                assert_eq!(s.fills, want, "{at_}: the ground, then the edges");
                let edges: Vec<Edge> = edge.map(|(width, colour)| Edge { outer: r, width, colour, radius: CornerRadius::ZERO }).into_iter().collect();
                assert_eq!(s.edges, edges, "{at_}: the outline");
                assert_eq!(failed_mark(&shapes, r, &t), cell.mark == Some(Mark::Failed), "{at_}: the failed mark");
                let [(words, at, colour)] = s.texts.as_slice() else { panic!("{at_}: one text: {:?}", s.texts) };
                assert_eq!((words.as_str(), *colour), ("Customer 191", t.text), "{at_}");
                assert!(close(at.left(), r.left() + 8.0) && close(at.center().y, r.center().y), "{at_}: {at:?} 8 in, centred in {r:?}");
                assert_eq!(sections(&shapes, "Customer 191").first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: in `data`");
                assert_eq!(link, None, "{at_}: no link");
            }

            // A row to delete.
            for selected in [false, true] {
                let at_ = format!("{theme:?} deleted, selected {selected}");
                let cell = Cell { mark: Some(Mark::Deleted), deleted: true, selected, ..plain(text) };
                let (shapes, r, _) = cell_drawn(theme, cell);
                let s = seen(&shapes, r);
                assert!(s.fills.contains(&(strike(r), t.text_muted)), "{at_}: the line across: {:?}", s.fills);
                assert_eq!(s.fills[0], (r, t.deleted), "{at_}: its ground");
                assert_eq!(s.texts.iter().map(|(w, _, c)| (w.as_str(), *c)).collect::<Vec<_>>(), [("Customer 191", t.text_muted)], "{at_}");
            }

            // NULL, empty, numbers.
            let (shapes, r, _) = cell_drawn(theme, plain(CellText::Null));
            let s = seen(&shapes, r);
            let [(w, at, c)] = s.texts.as_slice() else { panic!("{theme:?} NULL: {:?}", s.texts) };
            assert_eq!((w.as_str(), *c), ("NULL", t.text_muted), "{theme:?}: NULL");
            assert!(close(at.left(), r.left() + 8.0), "{theme:?}: NULL sits left in a text column");
            let italic = flat(&shapes).iter().any(|s| matches!(s, Shape::Text(g) if g.galley.job.sections.iter().any(|x| x.format.italics)));
            assert!(!italic, "{theme:?}: NULL is regular");
            let (shapes, r, _) = cell_drawn(theme, Cell { align: Align::Right, ..plain(CellText::Null) });
            let at = seen(&shapes, r).texts.first().map(|x| x.1);
            assert!(at.is_some_and(|a| close(a.right(), r.right() - 8.0)), "{theme:?}: NULL sits right in a number column: {at:?}");
            let (shapes, r, _) = cell_drawn(theme, plain(CellText::Text("")));
            assert!(seen(&shapes, r).texts.is_empty(), "{theme:?}: empty shows nothing");
            let (shapes, r, _) = cell_drawn(theme, Cell { align: Align::Right, ..plain(CellText::Text("221.29")) });
            let s = seen(&shapes, r);
            let [(w, at, c)] = s.texts.as_slice() else { panic!("{theme:?} number: {:?}", s.texts) };
            assert_eq!((w.as_str(), *c), ("221.29", t.text), "{theme:?}: a number");
            assert!(close(at.right(), r.right() - 8.0), "{theme:?}: a number ends 8 from the right: {at:?} in {r:?}");

            // A line break.
            let (shapes, r, _) = cell_drawn(theme, plain(CellText::Text("Leave at↵Ring twice")));
            let s = seen(&shapes, r);
            let [(w0, a0, c0), (w1, a1, c1)] = s.texts.as_slice() else { panic!("{theme:?} break: {:?}", s.texts) };
            assert_eq!((w0.as_str(), w1.as_str(), *c0, *c1), ("Leave at", "Ring twice", t.text, t.text), "{theme:?}: the parts");
            assert!(close(a0.left(), r.left() + 8.0) && close(a1.left(), a0.right() + 10.0), "{theme:?}: the mark's 10 between {a0:?} and {a1:?}");
            let slot = Rect::from_min_size(pos2(a0.right(), r.center().y - 5.0), vec2(10.0, 10.0));
            let (colours, bounds) = icon_in(&shapes, slot.expand(1.2));
            assert_eq!(colours, [t.text], "{theme:?}: the line break in the text's colour");
            let near = |a: f32, b: f32| (a - b).abs() <= 1.0;
            assert!(near(bounds.center().x, slot.center().x) && near(bounds.center().y, slot.center().y), "{theme:?}: {bounds:?} in {slot:?}");

            // The follow link: on a foreign key of the selected row, not on NULL.
            let key = |text, row_selected| Cell { align: Align::Right, link: true, row_selected, ..plain(text) };
            let (shapes, r, link) = cell_drawn(theme, key(CellText::Text("42"), true));
            let slot = Rect::from_min_size(pos2(r.right() - 8.0 - 14.0, r.center().y - 7.0), vec2(14.0, 14.0));
            assert_eq!(link, Some(slot), "{theme:?}: the link's square");
            let (colours, _) = icon_in(&shapes, slot.expand(1.2));
            assert_eq!(colours, [t.text_muted], "{theme:?}: the link in `text_muted`");
            let at = seen(&shapes, r).texts.first().map(|x| x.1);
            assert!(at.is_some_and(|a| close(a.right(), slot.left() - 6.0)), "{theme:?}: the number ends 6 before the link: {at:?}");
            for (text, row_selected) in [(CellText::Text("42"), false), (CellText::Null, true)] {
                let (shapes, r, link) = cell_drawn(theme, key(text, row_selected));
                assert_eq!(link, None, "{theme:?} {text:?} {row_selected}: no link");
                assert!(icon_in(&shapes, r).0.is_empty(), "{theme:?} {text:?} {row_selected}: no icon");
            }
        }
    }

    #[test]
    fn a_marked_cell_keeps_its_ground_when_selected_or_hovered() {
        // Foundations' one ground at a time: a marked cell keeps its own ground, nothing under or
        // over it, when selected, focused, hovered or both; selected it takes its outline. A row
        // to delete keeps its line across. An unmarked selected cell stays `selection` on hover.
        let looks = [(false, false, true), (true, false, false), (true, true, false), (true, false, true), (true, true, true)];
        for (theme, t) in both() {
            for mark in [Mark::Staged, Mark::New, Mark::Deleted, Mark::Failed, Mark::Saved] {
                for (selected, focused, hovered) in looks {
                    let at_ = format!("{theme:?} {mark:?} selected {selected} focused {focused} hovered {hovered}");
                    let deleted = mark == Mark::Deleted;
                    let cell = Cell { mark: Some(mark), selected, focused, hovered, deleted, ..plain(CellText::Text("512.00")) };
                    let (shapes, r, _) = cell_drawn(theme, cell);
                    let s = seen(&shapes, r);
                    let grounds: Vec<_> = s.fills.iter().filter(|(at, _)| *at == r).collect();
                    assert_eq!(grounds, [&(r, mark.ground(&t))], "{at_}: its own ground, alone");
                    let thin = |r: &Rect| r.width() <= 1.0 || r.height() <= 1.0;
                    assert!(s.fills.iter().all(|(at, _)| *at == r || thin(at)), "{at_}: nothing stacked, only lines: {:?}", s.fills);
                    if selected {
                        let outline = Edge { outer: r, width: 2.0, colour: mark.outline(&t), radius: CornerRadius::ZERO };
                        assert_eq!(s.edges, [outline], "{at_}: its outline");
                    }
                    assert_eq!(s.fills.contains(&(strike(r), t.text_muted)), deleted, "{at_}: the line across");
                }
            }
            let cell = Cell { selected: true, hovered: true, ..plain(CellText::Text("x")) };
            let (shapes, r, _) = cell_drawn(theme, cell);
            assert_eq!(seen(&shapes, r).fills.first(), Some(&(r, t.selection)), "{theme:?}: selected doesn't change on hover");
        }
    }

    /// One frame of `head` drawn into a `w` by 30 rect.
    fn head_drawn(theme: Theme, head: Header<'_>, w: f32) -> (Vec<ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            at = rect;
            head.paint(ui, rect);
        });
        (shapes, at)
    }

    #[test]
    fn the_header_cell_matches_the_page() {
        // Node 167321: 30 tall on `card`, a 1 px `line` at its right and a 1 px `ink` line along
        // its bottom; 8 in from its sides, its parts 6 apart: `icon/key` (`key` on a primary key,
        // `text_muted` on a foreign one), the name in `header` `text`, the type in `data`
        // `text_muted` as the database writes it, then the sort: the one arrow in `ink`, or on
        // hover, when it can sort, both arrows 1 apart in `text_muted`. A number's header sits
        // right; a key's stays left.
        let head = |name, ty| Header { name, ty, key: None, sort: None, align: Align::Left, sortable: true, hovered: false };
        let arrows = |shapes: &[ClippedShape], from: f32, r: Rect| -> Vec<(Vec<Color32>, Rect)> {
            [0.0, 7.0]
                .iter()
                .map(|dx| icon_in(shapes, Rect::from_min_size(pos2(from + dx, r.center().y - 4.0), vec2(6.0, 8.0)).expand(1.2)))
                .collect()
        };
        for (theme, t) in both() {
            for (state, h, w) in [
                ("text", head("customer", "text"), 140.0),
                ("as written", head("Mood", "enum('small','Large')"), 240.0),
                ("primary", Header { key: Some(Key::Primary), ..head("id", "int8") }, 140.0),
                // The page's own: at 150 its hint runs into the padding.
                ("foreign", Header { key: Some(Key::Foreign), ..head("customer_id", "int4") }, 150.0),
            ] {
                let at_ = format!("{theme:?} {state}");
                let (shapes, r) = head_drawn(theme, h, w);
                let s = seen(&shapes, r);
                let right = Rect::from_min_max(pos2(r.right() - 1.0, r.top()), r.max);
                let bottom = Rect::from_min_max(pos2(r.left(), r.bottom() - 1.0), r.max);
                assert_eq!(s.fills, [(r, t.card), (right, t.line), (bottom, t.ink)], "{at_}: the ground and its lines");
                assert!(s.edges.is_empty(), "{at_}: no outline");
                let [(name, n, nc), (ty, y, yc)] = s.texts.as_slice() else { panic!("{at_}: the name and type: {:?}", s.texts) };
                assert_eq!((name.as_str(), *nc, ty.as_str(), *yc), (h.name, t.text, h.ty, t.text_muted), "{at_}: as written");
                assert_eq!(sections(&shapes, h.name).first().map(|s| s.1.clone()), Some(Text::Header.font()), "{at_}: the name in `header`");
                assert_eq!(sections(&shapes, h.ty).first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: the type in `data`");
                let key_w = if h.key.is_some() { 9.5 + 6.0 } else { 0.0 };
                assert!(close(n.left(), r.left() + 8.0 + key_w) && close(y.left(), n.right() + 6.0), "{at_}: {n:?} then {y:?}");
                assert!(close(n.center().y, r.center().y) && close(y.center().y, r.center().y), "{at_}: centred");
                if let Some(key) = h.key {
                    let (colours, bounds) =
                        icon_in(&shapes, Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 4.75), vec2(9.5, 9.5)).expand(1.2));
                    assert_eq!(colours, [if key == Key::Primary { t.key } else { t.text_muted }], "{at_}: the key's colour");
                    assert!((bounds.center().y - r.center().y).abs() <= 1.0, "{at_}: the key centred");
                }
                assert!(arrows(&shapes, y.right() + 6.0, r).iter().all(|(c, _)| c.is_empty()), "{at_}: no arrows at rest");

                // Hovered: both arrows after the type, when it can sort; the name and type stay put.
                let (shapes, r) = head_drawn(theme, Header { hovered: true, ..h }, w);
                let hovered: Vec<_> = seen(&shapes, r).texts.into_iter().map(|(w, at, _)| (w, at)).collect();
                assert_eq!(hovered, s.texts.iter().map(|(w, at, _)| (w.clone(), *at)).collect::<Vec<_>>(), "{at_}: nothing moves on hover");
                let hint = arrows(&shapes, y.right() + 6.0, r);
                assert!(hint.iter().all(|(c, _)| c == &[t.text_muted]), "{at_}: on hover, both arrows: {hint:?}");
                let (shapes, r) = head_drawn(theme, Header { hovered: true, sortable: false, ..h }, w);
                assert!(arrows(&shapes, y.right() + 6.0, r).iter().all(|(c, _)| c.is_empty()), "{at_}: no hint where it can't sort");

                // Sorted.
                for sort in [Sort::Asc, Sort::Desc] {
                    let (shapes, r) = head_drawn(theme, Header { sort: Some(sort), ..h }, w);
                    let y = seen(&shapes, r).texts.get(1).map(|x| x.1).unwrap_or(Rect::NOTHING);
                    let a = arrows(&shapes, y.right() + 6.0, r);
                    assert_eq!((a[0].0.clone(), a[1].0.clone()), (vec![t.ink], vec![]), "{at_} {sort:?}: the one arrow in `ink`");
                }
            }
            // A number's header sits right: the type 8 from the right, or the hint.
            let h = Header { align: Align::Right, ..head("total", "numeric") };
            let (shapes, r) = head_drawn(theme, h, 140.0);
            let y = seen(&shapes, r).texts.get(1).map(|x| x.1).unwrap_or(Rect::NOTHING);
            assert!(close(y.right(), r.right() - 8.0), "{theme:?}: a number's type ends 8 from the right: {y:?} in {r:?}");
            let (shapes, r) = head_drawn(theme, Header { hovered: true, ..h }, 140.0);
            let y = seen(&shapes, r).texts.get(1).map(|x| x.1).unwrap_or(Rect::NOTHING);
            assert!(close(y.right(), r.right() - 8.0 - 13.0 - 6.0), "{theme:?}: hovered, the hint after it: {y:?}");
            assert!(arrows(&shapes, y.right() + 6.0, r).iter().all(|(c, _)| c == &[t.text_muted]), "{theme:?}: the hint at the right");
            let (shapes, r) = head_drawn(theme, Header { key: Some(Key::Primary), ..h }, 140.0);
            let n = seen(&shapes, r).texts.first().map(|x| x.1).unwrap_or(Rect::NOTHING);
            assert!(close(n.left(), r.left() + 8.0 + 9.5 + 6.0), "{theme:?}: a numeric key stays left: {n:?}");
            // The corner over the row numbers.
            let mut at = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                ui.add_space(ROOM);
                let (rect, _) = ui.allocate_exact_size(vec2(36.0, 30.0), Sense::hover());
                at = rect;
                corner(ui, rect);
            });
            let right = Rect::from_min_max(pos2(at.right() - 1.0, at.top()), at.max);
            let bottom = Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max);
            assert_eq!(fills_in(&shapes, at), [(at, t.card), (right, t.line), (bottom, t.ink)], "{theme:?}: the corner");
        }
    }

    #[test]
    fn the_row_number_matches_the_page() {
        // Node 167565: 26 tall, at least 36 wide, on `card` with a 1 px `line` at its right and
        // bottom; the number in `data` `text_muted`, 8 from the right. Hovered `hover`; selected
        // `selection`; the whole row selected `selection` with the number in `text`; a new row
        // `new_row` with `*`; a row to delete `deleted` with a 1 px `text_muted` line across.
        let n = RowNumber { n: 3, new: false, deleted: false, selected: false, whole: false, hovered: false };
        for (theme, t) in both() {
            for (state, row, ground, words, colour) in [
                ("plain", n, t.card, "3", t.text_muted),
                ("hover", RowNumber { hovered: true, ..n }, t.hover, "3", t.text_muted),
                ("selected", RowNumber { selected: true, ..n }, t.selection, "3", t.text_muted),
                ("selected and hovered", RowNumber { selected: true, hovered: true, ..n }, t.selection, "3", t.text_muted),
                ("row selected", RowNumber { selected: true, whole: true, ..n }, t.selection, "3", t.text),
                ("new", RowNumber { new: true, ..n }, t.new_row, "*", t.text_muted),
                ("new and selected", RowNumber { new: true, selected: true, hovered: true, ..n }, t.new_row, "*", t.text_muted),
                ("deleted", RowNumber { deleted: true, ..n }, t.deleted, "3", t.text_muted),
                ("thousands", RowNumber { n: 12_345, ..n }, t.card, "12,345", t.text_muted),
            ] {
                let at_ = format!("{theme:?} {state}");
                let mut at = Rect::NOTHING;
                let w = RowNumber::width(row.n);
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
                    at = rect;
                    row.paint(ui, rect);
                });
                let s = seen(&shapes, at);
                let mut want = vec![(at, ground)];
                want.extend(lines(at).map(|l| (l, t.line)));
                if row.deleted {
                    want.push((strike(at), t.text_muted));
                }
                assert_eq!(s.fills, want, "{at_}: the ground and its lines");
                let [(w, r, c)] = s.texts.as_slice() else { panic!("{at_}: one text: {:?}", s.texts) };
                assert_eq!((w.as_str(), *c), (words, colour), "{at_}");
                assert!(close(r.right(), at.right() - 8.0) && close(r.center().y, at.center().y), "{at_}: {r:?} 8 from the right of {at:?}");
                assert_eq!(sections(&shapes, words).first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: in `data`");
            }
        }
        assert_eq!(RowNumber::width(9), 36.0, "at least 36");
        assert_eq!(RowNumber::width(1), RowNumber::width(99));
        assert!(RowNumber::width(1_000_000) >= 9.0 * DATA_CHAR_W + 16.0, "1,000,000 fits");
    }

    /// A grid's rows as a table tab gives them: marks, rows to delete, numbers right.
    struct Rows {
        cells: Vec<Vec<Option<String>>>,
        marks: Vec<((usize, usize), Mark)>,
        deleted: Vec<usize>,
    }

    impl GridSource for Rows {
        fn columns(&self) -> usize {
            3
        }
        fn column_name(&self, col: usize) -> &str {
            ["id", "Note", "customer_id"].get(col).copied().unwrap_or("")
        }
        fn column_type(&self, col: usize) -> &str {
            ["int8", "enum('small','Large')", "int4"].get(col).copied().unwrap_or("")
        }
        fn rows(&self) -> usize {
            self.cells.len()
        }
        fn cell(&self, row: usize, col: usize) -> Option<&str> {
            self.cells.get(row)?.get(col)?.as_deref()
        }
        fn mark(&self, row: usize, col: usize) -> Option<Mark> {
            self.marks.iter().find(|(at, _)| *at == (row, col) || (at.0 == row && at.1 == usize::MAX)).map(|(_, m)| *m)
        }
        fn deleted(&self, row: usize) -> bool {
            self.deleted.contains(&row)
        }
        fn right_aligned(&self, col: usize) -> bool {
            col != 1
        }
    }

    /// A grid drawn every frame, with what it asked for.
    struct Shown {
        rows: Rows,
        state: GridState,
        tags: Tags,
        selection: Option<Selection>,
        acts: Vec<GridAct>,
        /// [`Grid::saved`].
        saved: f32,
    }

    fn grid_harness(theme: Theme, rows: Rows, selection: Option<Selection>) -> egui_kittest::Harness<'static, Shown> {
        let shown = Shown { rows, state: GridState::default(), tags: Tags::default(), selection, acts: Vec::new(), saved: 1.0 };
        harness_in(vec2(900.0, 400.0), theme, shown, |ui, s: &mut Shown| {
            s.tags.begin_frame();
            let keys = [Some(Key::Primary), None, Some(Key::Foreign)];
            let grid = Grid {
                id: Id::new("test-grid"),
                source: &s.rows,
                keys: &keys,
                selection: s.selection,
                sort: Some((0, Sort::Asc)),
                sortable: true,
                blocked: None,
                edits: RowEdits::Allowed,
                links: true,
                first_number: 1,
                editing: None,
                saved: s.saved,
            };
            let acts = grid.show(ui, &mut s.state, &mut s.tags);
            s.acts.extend(acts);
        })
    }

    #[test]
    fn a_saved_cell_is_marked_only_while_its_mark_shows() {
        // The grid draws a saved cell's mark at the strength it is given, and names it `saved`; at
        // 0 the cell is drawn and named as if unmarked. Other marks never fade.
        let some = |s: &str| Some(s.to_owned());
        let rows = Rows {
            cells: vec![vec![some("1"), some("a"), some("7")], vec![some("2"), some("b"), some("8")]],
            marks: vec![((0, 1), Mark::Saved), ((1, 1), Mark::Staged)],
            deleted: vec![],
        };
        let t = crate::theme::LIGHT;
        let mut h = grid_harness(Theme::Light, rows, None);
        for (strength, state, ground) in [(1.0, Some("saved"), t.saved), (0.5, Some("saved"), t.saved.gamma_multiply(0.5)), (0.0, None, t.card)] {
            h.state_mut().saved = strength;
            h.run();
            let tags = &h.state().tags;
            let state_of = |id: &str| tags.value(id).and_then(|v| v.get("state")).and_then(|s| s.as_str().map(str::to_owned));
            let ground_of = |id: &str| tags.rect(id).and_then(|r| fills_in(&h.output().shapes, r).first().map(|f| f.1));
            assert_eq!((state_of("grid-cell-0-1").as_deref(), ground_of("grid-cell-0-1")), (state, Some(ground)), "at {strength}");
            assert_eq!((state_of("grid-cell-1-1").as_deref(), ground_of("grid-cell-1-1")), (Some("staged"), Some(t.staged)), "at {strength}");
        }
    }

    fn click(h: &mut egui_kittest::Harness<'static, Shown>, at: Pos2) {
        h.event(egui::Event::PointerMoved(at));
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.run();
        }
    }

    #[test]
    fn grid_tags_keep_their_ids_and_values() {
        // `grid-header-<col>` {name, type, key, sort}; `grid-cell-<row>-<col>` {text, null,
        // state?, deleted?}; `fk-link-<row>-<col>` on the selected row's non-NULL foreign keys. A
        // click on a header sorts, on a cell selects, on a link follows.
        let some = |s: &str| Some(s.to_owned());
        let rows = Rows {
            cells: vec![
                vec![some("1"), some("Large"), some("7")],
                vec![some("2"), None, None],
                vec![some("3"), some("NULL"), some("9")],
                vec![some("4"), some("a↵b"), some("9")],
            ],
            marks: vec![((0, 1), Mark::Staged), ((2, usize::MAX), Mark::Failed), ((3, usize::MAX), Mark::Deleted)],
            deleted: vec![2, 3],
        };
        let mut h = grid_harness(Theme::Light, rows, Some(Selection { anchor: (0, 0), focus: (0, 0), whole: false }));
        h.run();
        let tags = &h.state().tags;
        let value = |id: &str| tags.value(id).cloned().unwrap_or_else(|| panic!("`{id}` tagged: {:?}", tags.ids()));
        use serde_json::json;
        assert_eq!(value("grid-header-0"), json!({ "name": "id", "type": "int8", "key": "primary", "sort": "asc" }));
        assert_eq!(value("grid-header-1"), json!({ "name": "Note", "type": "enum('small','Large')", "key": null, "sort": null }));
        assert_eq!(value("grid-header-2"), json!({ "name": "customer_id", "type": "int4", "key": "foreign", "sort": null }));
        assert_eq!(value("grid-cell-0-0"), json!({ "text": "1", "null": false }));
        assert_eq!(value("grid-cell-0-1"), json!({ "text": "Large", "null": false, "state": "staged" }));
        assert_eq!(value("grid-cell-1-1"), json!({ "text": "NULL", "null": true }));
        assert_eq!(value("grid-cell-2-1"), json!({ "text": "NULL", "null": false, "state": "failed", "deleted": true }));
        assert_eq!(value("grid-cell-3-1"), json!({ "text": "a↵b", "null": false, "state": "deleted", "deleted": true }));
        assert!(tags.rect("fk-link-0-2").is_some(), "the selected row's foreign key has its link");
        assert!(tags.ids().iter().all(|id| !id.starts_with("fk-link-") || *id == "fk-link-0-2"), "only there: {:?}", tags.ids());

        let at = |h: &egui_kittest::Harness<'static, Shown>, id: &str| h.state().tags.rect(id).map(|r| r.center()).unwrap_or_else(|| panic!("{id}"));
        let link = at(&h, "fk-link-0-2");
        click(&mut h, link);
        assert_eq!(h.state().acts, [GridAct::Follow { row: 0, col: 2 }], "the link follows");
        h.state_mut().acts.clear();
        let cell = at(&h, "grid-cell-2-1");
        click(&mut h, cell);
        assert_eq!(h.state().acts, [GridAct::Select { anchor: (2, 1), focus: (2, 1) }], "a click selects");
        h.state_mut().acts.clear();
        let head = at(&h, "grid-header-1");
        click(&mut h, head);
        assert_eq!(h.state().acts, [GridAct::Sort(1)], "a header click sorts");
        // The selection moves to row 1, whose foreign key is NULL: no link anywhere.
        h.state_mut().selection = Some(Selection { anchor: (1, 2), focus: (1, 2), whole: false });
        h.run();
        assert!(!h.state().tags.ids().iter().any(|id| id.starts_with("fk-link-")), "NULL has no link: {:?}", h.state().tags.ids());
    }

    #[test]
    fn text_with_line_breaks_stays_inside_its_cell() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter().clone();
            let colour = Color32::BLACK;
            let short = paint_text(ui, &p, "a↵b", (0.0, 10.0, 200.0), colour);
            let plain = paint_text(ui, &p, "ab", (0.0, 10.0, 200.0), colour);
            assert!((short - plain - 10.0).abs() < 0.5, "a break takes its own 10: {short} vs {plain}");
            let long = dbee_core::display::grid_text(&"line↵".repeat(1000)).into_owned();
            for right in [30.0, 120.0] {
                let w = paint_text(ui, &p, &long, (0.0, 10.0, right), colour);
                assert!(w <= right + 0.5, "{w} > {right}");
            }
            // No room at all: one `…` (the cell clips it), not every part.
            assert!(paint_text(ui, &p, &long, (0.0, 10.0, 1.0), colour) < 20.0);
            assert_eq!(paint_text(ui, &p, "", (0.0, 10.0, 50.0), colour), 0.0);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn null_is_never_the_text_null() {
        // A NULL and the text 'NULL' both read NULL; NULL is `text_muted`, the text `text`, and
        // the cells' values tell them apart.
        for (theme, t) in both() {
            let (shapes, r, _) = cell_drawn(theme, plain(CellText::Null));
            assert_eq!(
                seen(&shapes, r).texts.iter().map(|(w, _, c)| (w.clone(), *c)).collect::<Vec<_>>(),
                [("NULL".into(), t.text_muted)],
                "{theme:?}"
            );
            let (shapes, r, _) = cell_drawn(theme, plain(CellText::Text("NULL")));
            assert_eq!(seen(&shapes, r).texts.iter().map(|(w, _, c)| (w.clone(), *c)).collect::<Vec<_>>(), [("NULL".into(), t.text)], "{theme:?}");
        }
        let rows = Rows {
            cells: vec![vec![Some("1".into()), None, Some("2".into())], vec![Some("2".into()), Some("NULL".into()), None]],
            marks: vec![],
            deleted: vec![],
        };
        let mut h = grid_harness(Theme::Light, rows, None);
        h.run();
        let tags = &h.state().tags;
        assert_eq!(tags.value("grid-cell-0-1"), Some(&serde_json::json!({ "text": "NULL", "null": true })));
        assert_eq!(tags.value("grid-cell-1-1"), Some(&serde_json::json!({ "text": "NULL", "null": false })));
        let (a, b) = (tags.rect("grid-cell-0-1").unwrap_or(Rect::NOTHING), tags.rect("grid-cell-1-1").unwrap_or(Rect::NOTHING));
        let shapes = h.output().shapes.clone();
        let colour = |r: Rect| texts(&shapes).into_iter().find(|(w, at, _)| w == "NULL" && r.contains_rect(*at)).map(|x| x.2);
        assert_eq!((colour(a), colour(b)), (Some(crate::theme::LIGHT.text_muted), Some(crate::theme::LIGHT.text)), "drawn apart");
    }

    #[test]
    fn a_25000_row_grid_lays_out_only_what_shows() {
        // 25,000 rows of 6 columns, one value 10 MiB long: a frame lays out the rows in view and
        // no more, each cell's text cut to one line; at the far end the same.
        let huge = "x".repeat(10 * 1024 * 1024);
        let columns =
            (0..6).map(|c| Col { name: format!("c{c}"), base_type: "text".into(), type_label: "text".into(), class: TypeClass::LongText }).collect();
        let cells = (0..25_000).map(|r| (0..6).map(|c| Some(if r == 0 && c == 1 { huge.clone() } else { format!("{r}.{c}") })).collect()).collect();
        let rows = ResultSet { columns, rows: cells, rows_seen: 25_000, complete: true, ..ResultSet::default() };
        struct Big {
            rows: ResultSet,
            state: GridState,
            tags: Tags,
        }
        let height = 600.0;
        let mut h =
            harness_in(vec2(900.0, height), Theme::Light, Big { rows, state: GridState::default(), tags: Tags::default() }, |ui, s: &mut Big| {
                s.tags.begin_frame();
                let grid = Grid {
                    id: Id::new("big"),
                    source: &s.rows,
                    keys: &[],
                    selection: None,
                    sort: None,
                    sortable: false,
                    blocked: None,
                    edits: RowEdits::None,
                    links: false,
                    first_number: 1,
                    editing: None,
                    saved: 0.0,
                };
                let _ = grid.show(ui, &mut s.state, &mut s.tags);
            });
        h.run();
        // The rows that fit under the header, one cut at each end at most.
        let most = (((height - 30.0) / 26.0).ceil() as usize + 1) * 6;
        let count = |h: &egui_kittest::Harness<'static, Big>| h.state().tags.ids().iter().filter(|id| id.starts_with("grid-cell-")).count();
        let laid = count(&h);
        assert!(laid > 0 && laid <= most, "{laid} cells laid out, at most {most}");
        let longest = flat(&h.output().shapes)
            .iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some(t.galley.text().chars().count()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        assert!(longest <= 201, "the 10 MiB value lays out as one cut line: {longest} characters");
        assert!(h.state().tags.hidden("grid-cell-24999-0"), "the last row is listed out of view");
        h.state_mut().tags.scroll_to("grid-cell-24999-0");
        h.run();
        let laid = count(&h);
        assert!(laid > 0 && laid <= most, "at the end, {laid} cells laid out, at most {most}");
        assert!(h.state().tags.rect("grid-cell-24999-5").is_some(), "the last row in view: {:?}", h.state().tags.ids());
    }
}

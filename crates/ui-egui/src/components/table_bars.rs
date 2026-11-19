//! The table tab's bars: the table bar over the rows (`table bar`), the filter bar over it
//! (`filter row`), and the paging bar under the rows (`paging bar`).
//!
//! The table bar and the paging bar are views: they take what to show and say what was clicked;
//! the screen tags what they drew, says on hover why something is off, and acts. The table tab
//! ([`crate::table_tab`]) draws the table bar with STRUCTURE off and no Value cell until the
//! structure view and the query tab screen. The filter bar is a frame the screen's filter
//! rows stand in ([`filter_bar`], [`filter_row`], [`and_mark`]): [`crate::filter_bar`] draws its
//! controls there, from the shared controls.

use std::time::Duration;

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Response, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use super::button::{BarButton, BoxButton, IconButton, TextButton};
use super::icon::Icon;
use super::results::{CountShown, RowCount, Time};
use super::segmented::Segmented;
use super::status::{Piece, piece};
use super::tooltip::Hint;
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, CHANGES_GAP, FILTER_BAR_PAD_X, FILTER_BAR_PAD_Y, FILTER_GAP, FILTER_ROW_H, ICON_BUTTON, PAGING_GAP, PAGING_PAD_LEFT,
    PAGING_PAD_RIGHT, STAGED_MARK, STAGED_MARK_GAP, TABLE_BAR_GAP, TABLE_BAR_PAD_X, TABLE_CANCEL_PAD_X, TABLE_LOADING_GAP,
};
use crate::theme::{Text, tokens};

/// Why IMPORT is off, on its hover.
pub const IMPORT_LATER: &str = "Import rows: not now, import comes in a later version of dbee.";
/// Why the table bar's EXPORT is off, on its hover.
pub const EXPORT_LATER: &str = "Export these rows: not now, export comes in a later version of dbee.";

/// Which of the table's views shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Data,
    Structure,
}

/// A page on its way, for `elapsed` so far (today's spinner while the app gives no timer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loading {
    pub elapsed: Option<Duration>,
}

/// The table bar: the view switch, REFRESH (CANCEL while a page loads), + ROW, the rows' count,
/// IMPORT, EXPORT and the value panel's toggle. The structure view has REFRESH and its facts only.
#[derive(Clone, Copy, Debug)]
pub struct TableBar {
    pub view: View,
    /// STRUCTURE is on; off, it is drawn faint and a click on it asks nothing.
    pub structure: bool,
    /// A page loads: CANCEL in REFRESH's place, LOADING and its timer in the count's.
    pub loading: Option<Loading>,
    /// The rows' count, with the rows shown (`1–100`).
    pub count: RowCount,
    pub shown: Option<(u64, u64)>,
    /// The table's columns, which the structure view counts.
    pub columns: usize,
    /// REFRESH is on (nothing blocks loading the page again).
    pub can_refresh: bool,
    /// + ROW is on (the rows take edits now).
    pub can_add: bool,
    /// COUNT and COUNT ROWS are on.
    pub can_count: bool,
    /// The value panel's toggle, on while the panel shows; `None` draws no Value cell, and the bar
    /// ends at EXPORT.
    pub value: Option<bool>,
}

/// What a click in the table bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAsked {
    View(View),
    Refresh,
    Cancel,
    AddRow,
    Count,
    Value,
}

/// The table bar as drawn: each part shown, and what a click asked for.
pub struct TableShown {
    /// The view switch's segments: DATA, STRUCTURE.
    pub view: Vec<Response>,
    pub refresh: Option<Response>,
    pub cancel: Option<Response>,
    pub add: Option<Response>,
    pub count: Option<CountShown>,
    /// LOADING, or the structure's facts.
    pub facts: Option<Response>,
    pub import: Option<Response>,
    pub export: Option<Response>,
    pub value: Option<Response>,
    pub asked: Option<TableAsked>,
}

/// What stands between the table bar's cells and its right end: the count, LOADING, or the
/// structure's facts.
enum Facts {
    Count,
    Loading(Piece, Time),
    Structure(Piece),
}

impl TableBar {
    /// The structure's facts, as the page writes them: `8 columns  ~25,000 rows`.
    fn structure_words(&self) -> String {
        let columns = if self.columns == 1 { "1 column".to_string() } else { format!("{} columns", self.columns) };
        match self.count.words(None) {
            rows if rows.is_empty() => columns,
            rows => format!("{columns}  {rows}"),
        }
    }

    /// Draws it as wide as `ui`, 36 tall. Short of room the count's range goes first (`1–100
    /// of`), then IMPORT and EXPORT, then the count; the cells at the ends stay, the right ones cut
    /// at the bar's end.
    pub fn show(self, ui: &mut Ui) -> TableShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui, Line::Bottom);
        let mid = cell.center().y;
        let data = self.view == View::Data;
        let mut asked = None;
        // The left: the view switch, then REFRESH (or CANCEL's cell) and + ROW.
        let mut left = ui.new_child(UiBuilder::new().id_salt(bar.id.with("left")).max_rect(cell).layout(Layout::left_to_right(Align::Center)));
        left.spacing_mut().item_spacing.x = 0.0;
        left.add_space(TABLE_BAR_PAD_X);
        let mut view = self.view;
        let choices = [(View::Data, "Data"), (View::Structure, "Structure")];
        let off = (!self.structure).then_some(View::Structure);
        let switch = Segmented::caps(&mut view, &choices).off(off).show(&mut left);
        if switch.row.changed() {
            asked = Some(TableAsked::View(view));
        }
        left.add_space(TABLE_BAR_GAP);
        let (refresh, cancel) = match self.loading {
            Some(_) => (None, Some(cancel_cell(&mut left))),
            None => (Some(left.add_enabled(self.can_refresh, BarButton::label(Icon::Refresh, "Refresh").over_line())), None),
        };
        let add = data.then(|| left.add_enabled(self.can_add, BarButton::label(Icon::Plus, "Row").over_line()));
        let left_end = left.min_rect().right();
        // The right end: IMPORT, EXPORT and the toggle (when there is one), off in the structure
        // view.
        let import = BarButton::label(Icon::Import, "Import").over_line();
        let export = BarButton::label(Icon::Export, "Export").over_line().divider(false);
        let toggle = self.value.map(|on| BarButton::icon(Icon::Panel).on(on).over_line());
        let toggle_w = toggle.as_ref().map_or(0.0, |b| b.width(ui));
        let io_w = import.width(ui) + export.width(ui);
        // The count's room with IMPORT and EXPORT (`io_w`) or without them.
        let room_for = |io: f32| cell.right() - io - toggle_w - TABLE_BAR_GAP - left_end - TABLE_BAR_GAP;
        // The first form of the count that fits: with its range and IMPORT and EXPORT; without
        // the range; without IMPORT and EXPORT too. When none does, the count goes and IMPORT and
        // EXPORT stay.
        let (range, io) = if data && self.loading.is_none() {
            let bare = self.count.width(ui, None);
            if self.count.width(ui, self.shown) <= room_for(io_w) {
                (self.shown, true)
            } else if bare <= room_for(io_w) {
                (None, true)
            } else {
                (None, bare > room_for(0.0))
            }
        } else {
            (self.shown, true)
        };
        let right_w: f32 = if data { toggle_w + if io { io_w } else { 0.0 } } else { 0.0 };
        let right_x = (cell.right() - right_w).max(left_end);
        let (import, export, value) = if data {
            let lane = Rect::from_min_max(pos2(right_x, cell.top()), cell.max);
            let mut right = ui.new_child(UiBuilder::new().id_salt(bar.id.with("right")).max_rect(lane).layout(Layout::left_to_right(Align::Center)));
            right.spacing_mut().item_spacing.x = 0.0;
            right.set_clip_rect(right.clip_rect().intersect(cell));
            let (import, export) = if io {
                (
                    Some(right.add_enabled(false, import).hint_disabled(IMPORT_LATER)),
                    Some(right.add_enabled(false, export).hint_disabled(EXPORT_LATER)),
                )
            } else {
                (None, None)
            };
            (import, export, toggle.map(|toggle| right.add(toggle)))
        } else {
            (None, None, None)
        };
        // Between them, ending 12 before IMPORT (or the bar's end): the count, LOADING, or the
        // structure's facts; none when there is no room for it.
        let facts = match self.loading {
            Some(l) => Facts::Loading(
                Piece::new(ui, Text::Label.job("Loading", t.text_muted), "Loading"),
                l.elapsed.map_or(Time::Spinner, |e| Time::timer(ui, t, e)),
            ),
            None if data => Facts::Count,
            None => {
                let words = self.structure_words();
                Facts::Structure(Piece::new(ui, Text::Label.job(&words, t.text_muted), words))
            }
        };
        let end = if data { right_x } else { cell.right() } - TABLE_BAR_GAP;
        let room = end - left_end - TABLE_BAR_GAP;
        let mut count = None;
        let mut shown_facts = None;
        match facts {
            Facts::Count => {
                let needed = self.count.width(ui, range);
                if needed <= room {
                    let at = Rect::from_min_max(pos2(end - needed, cell.top()), pos2(end, cell.bottom()));
                    let mut c =
                        ui.new_child(UiBuilder::new().id_salt(bar.id.with("count")).max_rect(at).layout(Layout::left_to_right(Align::Center)));
                    if !self.can_count {
                        c.disable();
                    }
                    let shown = self.count.show(&mut c, range);
                    if shown.asked {
                        asked = Some(TableAsked::Count);
                    }
                    count = Some(shown);
                }
            }
            Facts::Loading(words, time) => {
                let w = words.width() + TABLE_LOADING_GAP + time.width();
                if w <= room {
                    let x = end - w;
                    shown_facts = Some(piece(ui, bar.id.with("loading"), &words, pos2(x, mid), t.text_muted));
                    time.show(ui, bar.id.with("timer"), x + words.width() + TABLE_LOADING_GAP, mid, t);
                }
            }
            Facts::Structure(words) => {
                if words.width() <= room {
                    shown_facts = Some(piece(ui, bar.id.with("facts"), &words, pos2(end - words.width(), mid), t.text_muted));
                }
            }
        }
        let clicked = |r: &Option<Response>| r.as_ref().is_some_and(Response::clicked);
        if clicked(&refresh) {
            asked = Some(TableAsked::Refresh);
        } else if clicked(&cancel) {
            asked = Some(TableAsked::Cancel);
        } else if clicked(&add) {
            asked = Some(TableAsked::AddRow);
        } else if clicked(&value) {
            asked = Some(TableAsked::Value);
        }
        TableShown { view: switch.segments, refresh, cancel, add, count, facts: shown_facts, import, export, value, asked }
    }
}

/// CANCEL in its cell, in REFRESH's place while a page loads: a divider, then `button/box`
/// primary 12 in from either side.
fn cancel_cell(ui: &mut Ui) -> Response {
    let t = tokens(ui.ctx());
    let button = BoxButton::primary("Cancel");
    let size = vec2(BAR_LINE_W + 2.0 * TABLE_CANCEL_PAD_X + button.width(ui), BAR_CELL_H);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(size);
        let cell = ui.max_rect();
        ui.painter().rect_filled(Rect::from_min_size(cell.min, vec2(BAR_LINE_W, cell.height())), CornerRadius::ZERO, t.line);
        ui.add_space(BAR_LINE_W + TABLE_CANCEL_PAD_X);
        ui.add(button)
    })
    .inner
}

/// A cell's divider: as thick as a bar's line.
const BAR_LINE_W: f32 = BAR_LINE;

/// Where a bar's line runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Top,
    Bottom,
}

/// A bar `ui`'s width wide, [`BAR_H`] tall, on `card` with its line along `line`. Returns its
/// response and the room beside the line.
fn bar_ground(ui: &mut Ui, line: Line) -> (Response, Rect) {
    let t = tokens(ui.ctx());
    let (bar, r) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::ZERO, t.card);
    let (edge, room) = match line {
        Line::Top => (Rect::from_min_size(bar.min, vec2(bar.width(), BAR_LINE)), Rect::from_min_max(pos2(bar.left(), bar.top() + BAR_LINE), bar.max)),
        Line::Bottom => (
            Rect::from_min_max(pos2(bar.left(), bar.bottom() - BAR_LINE), bar.max),
            Rect::from_min_max(bar.min, pos2(bar.right(), bar.bottom() - BAR_LINE)),
        ),
    };
    p.rect_filled(edge, CornerRadius::ZERO, t.line);
    (r, room)
}

/// The paging bar: FIRST, PREVIOUS, the page, NEXT, LAST; and with staged changes their count,
/// RESET and APPLY.
#[derive(Clone, Copy, Debug)]
pub struct PagingBar<'a> {
    /// The page's words: `Page 3`, `Page 1 of 250`.
    pub page: &'a str,
    /// FIRST and PREVIOUS are on: not the first page, and nothing blocks paging.
    pub back: bool,
    /// NEXT and LAST are on.
    pub forward: bool,
    /// Staged changes: `1 change` with the staged mark, RESET and APPLY.
    pub changes: usize,
    /// APPLY is on.
    pub can_apply: bool,
}

/// What a click in the paging bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingAsked {
    First,
    Previous,
    Next,
    Last,
    Reset,
    Apply,
}

/// The paging bar as drawn: each part shown, and what a click asked for.
pub struct PagingShown {
    pub first: Response,
    pub previous: Response,
    pub page: Response,
    pub next: Response,
    pub last: Response,
    pub changes: Option<Response>,
    pub reset: Option<Response>,
    pub apply: Option<Response>,
    pub asked: Option<PagingAsked>,
}

impl PagingBar<'_> {
    /// `1 change`, `3 changes`.
    fn changes_words(&self) -> String {
        if self.changes == 1 { "1 change".into() } else { format!("{} changes", self.changes) }
    }

    /// Draws it as wide as `ui`, 36 tall. Short of room the count of changes goes first; then the
    /// right group follows the left one, cut at the bar's end.
    pub fn show(self, ui: &mut Ui) -> PagingShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui, Line::Top);
        let mid = cell.center().y;
        let inner = Rect::from_min_max(pos2(cell.left() + PAGING_PAD_LEFT, cell.top()), pos2(cell.right() - PAGING_PAD_RIGHT, cell.bottom()));
        let mut left = ui.new_child(UiBuilder::new().id_salt(bar.id.with("left")).max_rect(inner).layout(Layout::left_to_right(Align::Center)));
        left.spacing_mut().item_spacing.x = 0.0;
        let first = left.add_enabled(self.back, IconButton::plain(Icon::First));
        let previous = left.add_enabled(self.back, IconButton::plain(Icon::Previous));
        let words = Piece::new(&left, Text::Label.job(self.page, t.text), self.page);
        let (at, _) = left.allocate_exact_size(vec2(words.width(), ICON_BUTTON), Sense::hover());
        let page = piece(&left, bar.id.with("page"), &words, pos2(at.left(), mid), t.text);
        let next = left.add_enabled(self.forward, IconButton::plain(Icon::Next));
        let last = left.add_enabled(self.forward, IconButton::plain(Icon::Last));
        let left_end = left.min_rect().right();
        let (mut changes, mut reset, mut apply) = (None, None, None);
        if self.changes > 0 {
            let reset_b = TextButton::new("Reset");
            let apply_b = BoxButton::primary("Apply");
            let buttons_w = reset_b.width(ui) + PAGING_GAP + apply_b.width(ui);
            let says = self.changes_words();
            let count = Piece::new(ui, Text::Label.job(&says, t.text), says);
            let count_w = STAGED_MARK + STAGED_MARK_GAP + count.width() + CHANGES_GAP;
            let with_count = inner.right() - buttons_w - count_w >= left_end + CHANGES_GAP;
            let w = buttons_w + if with_count { count_w } else { 0.0 };
            let x = (inner.right() - w).max(left_end + CHANGES_GAP);
            let lane = Rect::from_min_max(pos2(x, inner.top()), pos2(x + w, inner.bottom()));
            let mut right = ui.new_child(UiBuilder::new().id_salt(bar.id.with("right")).max_rect(lane).layout(Layout::left_to_right(Align::Center)));
            right.spacing_mut().item_spacing.x = 0.0;
            right.set_clip_rect(right.clip_rect().intersect(inner));
            if with_count {
                let (mark, _) = right.allocate_exact_size(vec2(STAGED_MARK, STAGED_MARK), Sense::hover());
                right.painter().rect_filled(mark, CornerRadius::ZERO, t.staged_strong);
                right.add_space(STAGED_MARK_GAP);
                let (at, _) = right.allocate_exact_size(vec2(count.width(), ICON_BUTTON), Sense::hover());
                changes = Some(piece(&right, bar.id.with("changes"), &count, pos2(at.left(), mid), t.text));
                right.add_space(CHANGES_GAP);
            }
            reset = Some(right.add(reset_b));
            right.add_space(PAGING_GAP);
            apply = Some(right.add_enabled(self.can_apply, apply_b));
        }
        let clicked = |r: &Option<Response>| r.as_ref().is_some_and(Response::clicked);
        let asked = if first.clicked() {
            Some(PagingAsked::First)
        } else if previous.clicked() {
            Some(PagingAsked::Previous)
        } else if next.clicked() {
            Some(PagingAsked::Next)
        } else if last.clicked() {
            Some(PagingAsked::Last)
        } else if clicked(&reset) {
            Some(PagingAsked::Reset)
        } else if clicked(&apply) {
            Some(PagingAsked::Apply)
        } else {
            None
        };
        PagingShown { first, previous, page, next, last, changes, reset, apply, asked }
    }
}

/// The filter bar's frame: `card` as wide as `ui`, its rows [`FILTER_BAR_PAD_Y`] from its top and
/// bottom and [`FILTER_BAR_PAD_X`] from either side, a 1 px `line` under it; `add` draws the rows
/// top-down ([`filter_row`]).
pub fn filter_bar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens(ui.ctx());
    let (x, y) = (FILTER_BAR_PAD_X as i8, FILTER_BAR_PAD_Y as i8);
    let margin = Margin { left: x, right: x, top: y, bottom: y + BAR_LINE as i8 };
    let shown = Frame::NONE.fill(t.card).inner_margin(margin).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
    });
    let rect = shown.response.rect;
    ui.painter().rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - BAR_LINE), rect.max), CornerRadius::ZERO, t.line);
    shown.inner
}

/// A row of the filter bar, [`FILTER_ROW_H`] tall and as wide as `ui`: `add` draws its controls
/// left to right, [`FILTER_GAP`] apart, centred on the row.
pub fn filter_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = vec2(ui.available_width(), FILTER_ROW_H);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(size);
        ui.spacing_mut().item_spacing.x = FILTER_GAP;
        add(ui)
    })
    .inner
}

/// AND, at the start of every filter row after the first: `label` grey, centred in a square as
/// big as the first row's toggle.
pub fn and_mark(ui: &mut Ui) -> Response {
    let t = tokens(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), Sense::hover());
    let words = Piece::new(ui, Text::Label.job("And", t.text_muted), "And");
    let _ = piece(ui, r.id.with("words"), &words, pos2(rect.center().x - words.width() / 2.0, rect.center().y), t.text_muted);
    r
}

#[cfg(test)]
mod tests {
    use dbee_engine::TabId;
    use dbee_sql::Dialect;
    use dbee_sql::filter::{Filter, Op};
    use egui::epaint::ClippedShape;
    use egui::{Color32, CornerRadius, Rect, Theme, pos2, vec2};

    use super::*;
    use crate::automation::Tags;
    use crate::components::{Edge, both, edges_in, fills_in, icon_in, painted_in, sections, texts};
    use crate::filter_bar::{self, Bar};
    use crate::theme::{Text, Tokens};

    /// The page's bars are 840 wide.
    const W: f32 = 840.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.6
    }

    /// One frame of `add` drawing a bar `W` wide at (10, 10): its shapes and where the bar lay.
    fn drawn(theme: Theme, mut add: impl FnMut(&mut Ui)) -> (Vec<ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted_in(vec2(900.0, 300.0), theme, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(W);
                    let top = ui.cursor().min;
                    add(ui);
                    at = Rect::from_min_max(top, pos2(top.x + W, ui.min_rect().bottom()));
                });
            });
        });
        (shapes, at)
    }

    /// The text `words` drawn: where, and in what colour.
    fn word(shapes: &[ClippedShape], words: &str, at_: &str) -> (Rect, Color32) {
        let all = texts(shapes);
        all.iter().find(|(w, _, _)| w == words).map(|(_, r, c)| (*r, *c)).unwrap_or_else(|| panic!("{at_}: `{words}` drawn: {all:?}"))
    }

    /// The font `words` is drawn in.
    fn font_of(shapes: &[ClippedShape], words: &str) -> Option<egui::FontId> {
        sections(shapes, words).first().map(|(_, f, _)| f.clone())
    }

    /// The 1 px `line` divider at the left end of a cell starting at `x`, `h` tall from `top`.
    fn divider(shapes: &[ClippedShape], t: &Tokens, x: f32, top: f32, h: f32) -> bool {
        fills_in(shapes, Rect::EVERYTHING)
            .iter()
            .any(|(r, c)| *c == t.line && close(r.left(), x) && close(r.width(), 1.0) && close(r.top(), top) && close(r.height(), h))
    }

    #[test]
    fn the_table_bar_matches_the_page() {
        // Node 164278: 36 tall on `card`, a 1 px `line` along its bottom; from 12 in, the view
        // switch (caps segments, 28 tall, centred above the line), 12, then REFRESH and + ROW as
        // bar cells with their dividers (90 and 63 wide); at the right end IMPORT and EXPORT (83
        // each, EXPORT without a divider) and the value panel's toggle (36); the count ends 12
        // before IMPORT. Structure: REFRESH only, then `8 COLUMNS  ~25,000 ROWS` 12 from the end.
        // Loading: CANCEL (`button/box` primary) in a 92 cell with a divider in REFRESH's place,
        // + ROW after it, LOADING 12 before its timer (`data`) in the count's place.
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let bar = |view, loading| TableBar {
                view,
                loading,
                // The page's structure bar shows the estimate.
                count: if view == View::Structure { RowCount::Estimate(25_000) } else { RowCount::Exact(25_000) },
                shown: Some((1, 100)),
                columns: 8,
                can_refresh: true,
                can_add: true,
                can_count: true,
                value: Some(false),
                structure: true,
            };
            for (state, b) in [
                ("data", bar(View::Data, None)),
                ("structure", bar(View::Structure, None)),
                ("loading", bar(View::Data, Some(Loading { elapsed: Some(s(800)) }))),
            ] {
                let at_ = format!("{theme:?} {state}");
                let (shapes, at) = drawn(theme, |ui| {
                    b.show(ui);
                });
                assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                let fills = fills_in(&shapes, at);
                assert_eq!(fills.first(), Some(&(at, t.card)), "{at_}: on card");
                let line = Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max);
                assert!(fills.contains(&(line, t.line)), "{at_}: the line along its bottom: {fills:?}");
                let mid = at.top() + 17.5;
                // The view switch.
                let switch = Rect::from_min_size(pos2(at.left() + 12.0, mid - 14.0), vec2(139.0, 28.0));
                let edges = edges_in(&shapes, switch.expand(1.5), 0.0);
                let edge = edges.iter().find(|e| e.colour == t.line_strong).unwrap_or_else(|| panic!("{at_}: the switch's edge: {edges:?}"));
                assert!(
                    close(edge.outer.left(), switch.left()) && close(edge.outer.top(), switch.top()) && (edge.outer.width() - 139.0).abs() <= 2.0,
                    "{at_}: the switch {edge:?}"
                );
                let selected = if state == "structure" { "STRUCTURE" } else { "DATA" };
                let other = if state == "structure" { "DATA" } else { "STRUCTURE" };
                assert_eq!(word(&shapes, selected, &at_).1, t.on_ink, "{at_}: {selected} chosen");
                assert_eq!(word(&shapes, other, &at_).1, t.text, "{at_}: {other} not");
                let cells_from = edge.outer.right() + 12.0;
                let first_cell = |w: &str| word(&shapes, w, &at_).0;
                match state {
                    "loading" => {
                        assert!(divider(&shapes, &t, cells_from, at.top(), 35.0), "{at_}: CANCEL's cell has a divider at {cells_from}");
                        let (cancel, c) = word(&shapes, "CANCEL", &at_);
                        assert_eq!(c, t.on_ink, "{at_}: CANCEL on ink");
                        let button = fills.iter().find(|(r, c)| *c == t.ink && r.contains_rect(cancel)).map(|(r, _)| *r).unwrap_or(Rect::NOTHING);
                        assert!(close(button.left(), cells_from + 13.0) && close(button.height(), 28.0), "{at_}: CANCEL's button {button:?}");
                        assert!((button.right() + 12.0 - (cells_from + 92.0)).abs() <= 1.5, "{at_}: its cell 92 wide");
                        assert!(texts(&shapes).iter().all(|(w, _, _)| w != "REFRESH"), "{at_}: no REFRESH while loading");
                        let row = first_cell("ROW");
                        assert!(divider(&shapes, &t, button.right() + 12.0, at.top(), 35.0), "{at_}: + ROW's divider 12 after CANCEL");
                        assert!(row.left() > button.right(), "{at_}: + ROW after CANCEL");
                        let (loading, c) = word(&shapes, "LOADING", &at_);
                        let (timer, tc) = word(&shapes, "00:00.8", &at_);
                        assert_eq!((c, tc), (t.text_muted, t.text_muted), "{at_}: LOADING and its timer grey");
                        assert_eq!(font_of(&shapes, "00:00.8"), Some(Text::Data.font()), "{at_}: the timer in data");
                        assert!(
                            close(timer.left() - loading.right(), 12.0 - Text::Label.spacing()) || close(timer.left() - loading.right(), 12.0),
                            "{at_}: 12 between: {loading:?} {timer:?}"
                        );
                    }
                    _ => {
                        let refresh = first_cell("REFRESH");
                        assert!(divider(&shapes, &t, cells_from, at.top(), 35.0), "{at_}: REFRESH's divider at {cells_from}");
                        assert!(close(refresh.left(), cells_from + 30.0), "{at_}: REFRESH's words 30 into its cell: {refresh:?}");
                        let (icons, _) = icon_in(&shapes, Rect::from_min_size(pos2(cells_from + 11.0, mid - 7.0), vec2(12.0, 14.0)));
                        assert_eq!(icons, [t.text], "{at_}: the refresh icon");
                    }
                }
                let end = at.right();
                if state == "structure" {
                    let (facts, c) = word(&shapes, "8 COLUMNS  ~25,000 ROWS", &at_);
                    assert_eq!(c, t.text_muted, "{at_}: the facts grey");
                    assert!(
                        close(end - facts.right(), 12.0 + Text::Label.spacing()) || close(end - facts.right(), 12.0),
                        "{at_}: 12 from the end: {facts:?}"
                    );
                    for w in ["ROW", "IMPORT", "EXPORT"] {
                        assert!(texts(&shapes).iter().all(|(s, _, _)| s != w), "{at_}: no `{w}`");
                    }
                    continue;
                }
                // + ROW, then the right end: IMPORT, EXPORT and the toggle.
                let row = first_cell("ROW");
                let import_x = end - 36.0 - 83.0 - 83.0;
                assert!(divider(&shapes, &t, end - 36.0, at.top(), 35.0), "{at_}: the toggle's divider");
                assert!(divider(&shapes, &t, import_x, at.top(), 35.0), "{at_}: IMPORT's divider at {import_x}");
                assert!(!divider(&shapes, &t, end - 36.0 - 83.0, at.top(), 35.0), "{at_}: none before EXPORT");
                let (import, ic) = word(&shapes, "IMPORT", &at_);
                let (export, ec) = word(&shapes, "EXPORT", &at_);
                assert!(close(import.left(), import_x + 30.0) && close(export.left(), import_x + 83.0 + 30.0), "{at_}: {import:?} {export:?}");
                assert_eq!((ic, ec), (t.text_faint, t.text_faint), "{at_}: IMPORT and EXPORT off: nothing to do yet");
                let facts = if state == "loading" { word(&shapes, "00:00.8", &at_).0 } else { word(&shapes, "1–100 OF 25,000 ROWS", &at_).0 };
                let trail = if state == "loading" { 0.0 } else { Text::Label.spacing() };
                assert!(
                    close(import_x - facts.right() + trail, 12.0) || close(import_x - facts.right(), 12.0),
                    "{at_}: the count 12 before IMPORT: {facts:?}"
                );
                assert!(row.right() < facts.left(), "{at_}: + ROW {row:?} before the count {facts:?}");
            }
        }
    }

    #[test]
    fn the_table_bar_says_what_was_asked() {
        let mut h = crate::components::harness_in(vec2(900.0, 200.0), Theme::Light, Vec::new(), |ui, seen: &mut Vec<TableAsked>| {
            let b = TableBar {
                view: View::Data,
                structure: true,
                loading: None,
                count: RowCount::Unknown,
                shown: Some((1, 100)),
                columns: 3,
                can_refresh: true,
                can_add: true,
                can_count: true,
                value: Some(true),
            };
            ui.allocate_ui(vec2(840.0, 36.0), |ui| seen.extend(b.show(ui).asked));
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["Structure", "Refresh", "Row", "Count rows", "panel", "Import", "Export"] {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(
            *h.state(),
            [TableAsked::View(View::Structure), TableAsked::Refresh, TableAsked::AddRow, TableAsked::Count, TableAsked::Value],
            "IMPORT and EXPORT ask nothing"
        );
        h.get_by_label("Import").hover();
        h.run();
        h.run();
        assert!(texts(&h.output().shapes).iter().any(|(w, _, _)| w == IMPORT_LATER), "IMPORT's hover says why");
    }

    /// The table bar as the table tab draws it until the structure view and the query tab screen:
    /// STRUCTURE off, no Value cell.
    fn screen_bar(count: RowCount) -> TableBar {
        TableBar {
            view: View::Data,
            structure: false,
            loading: None,
            count,
            shown: Some((1, 100)),
            columns: 3,
            can_refresh: true,
            can_add: true,
            can_count: true,
            value: None,
        }
    }

    #[test]
    fn structure_off_is_drawn_faint_and_asks_nothing() {
        for (theme, t) in both() {
            let at_ = format!("{theme:?}");
            let (shapes, _) = drawn(theme, |ui| {
                screen_bar(RowCount::Exact(25_000)).show(ui);
            });
            assert_eq!(word(&shapes, "STRUCTURE", &at_).1, t.text_faint, "{at_}: STRUCTURE off");
            assert_eq!(word(&shapes, "DATA", &at_).1, t.on_ink, "{at_}: DATA chosen");
        }
        let mut h = crate::components::harness_in(vec2(900.0, 200.0), Theme::Light, Vec::new(), |ui, seen: &mut Vec<TableAsked>| {
            ui.allocate_ui(vec2(840.0, 36.0), |ui| seen.extend(screen_bar(RowCount::Exact(3)).show(ui).asked));
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["Structure", "Data"] {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(*h.state(), [], "neither STRUCTURE off nor DATA chosen asks anything");
    }

    #[test]
    fn no_value_cell_ends_at_export() {
        for (theme, _) in both() {
            let at_ = format!("{theme:?}");
            for value in [None, Some(false)] {
                let mut export = Rect::NOTHING;
                let (shapes, at) = drawn(theme, |ui| {
                    let b = TableBar { value, ..screen_bar(RowCount::Exact(25_000)) };
                    export = b.show(ui).export.map_or(Rect::NOTHING, |r| r.rect);
                });
                let end = Rect::from_min_max(pos2(at.right() - 36.0, at.top()), at.max);
                let icons = icon_in(&shapes, end).0;
                match value {
                    None => {
                        assert!((export.right() - at.right()).abs() <= 0.5, "{at_}: EXPORT {export:?} ends the bar {at:?}");
                        assert!(icons.is_empty(), "{at_}: no panel icon: {icons:?}");
                    }
                    Some(_) => {
                        assert!(export.right() < at.right() - 30.0, "{at_}: the toggle after EXPORT {export:?}");
                        assert!(!icons.is_empty(), "{at_}: the panel icon");
                    }
                }
            }
        }
    }

    #[test]
    fn short_of_room_the_range_then_import_and_export_give_way() {
        // From wide to narrow: everything; the count without its range; IMPORT and EXPORT gone
        // too; the count gone, IMPORT and EXPORT back. Never two parts over each other.
        let mut seen: Vec<u8> = Vec::new();
        for width in (340..=900).rev().step_by(10) {
            let mut h = crate::components::harness_in(
                vec2(width as f32 + 20.0, 100.0),
                Theme::Light,
                None,
                move |ui, out: &mut Option<(Option<Rect>, Option<Rect>, Option<Rect>, Rect)>| {
                    ui.allocate_ui(vec2(width as f32, 36.0), |ui| {
                        let s = screen_bar(RowCount::Estimate(100_000)).show(ui);
                        let words = s.count.as_ref().and_then(|c| c.words.as_ref().map(|r| r.rect));
                        let count = s.count.as_ref().and_then(|c| c.count.as_ref().map(|r| r.rect));
                        let add = s.add.map_or(Rect::NOTHING, |r| r.rect);
                        *out = Some((words, count, s.import.map(|r| r.rect), add));
                    });
                },
            );
            h.run();
            let Some((words, count, import, add)) = *h.state() else { panic!("{width}: drawn") };
            let shapes = &h.output().shapes;
            let range = texts(shapes).iter().any(|(w, _, _)| w.starts_with("1–100"));
            let form = match (count.is_some(), range, import.is_some()) {
                (true, true, true) => 0,
                (true, false, true) => 1,
                (true, false, false) => 2,
                (false, _, true) => 3,
                other => panic!("{width}: no such form {other:?}"),
            };
            if let (Some(w), Some(i)) = (words.or(count), import) {
                assert!(w.right() <= i.left() && w.left() >= add.right(), "{width}: the count {w:?} between + ROW {add:?} and IMPORT {i:?}");
            }
            assert!(seen.last().is_none_or(|&last| form >= last), "{width}: room taken back: {seen:?} then {form}");
            seen.push(form);
        }
        for form in 0..=3 {
            assert!(seen.contains(&form), "the sweep passes through form {form}: {seen:?}");
        }
    }

    #[test]
    fn the_paging_bar_matches_the_page() {
        // Node 164453: 36 tall on `card`, a 1 px `line` along its top; from 8 in, the plain arrows
        // (28) and `PAGE 1 OF 250` in `label`, `text`, side by side; at the right end, 12 in,
        // APPLY (`button/box` primary), 4 before it RESET (`button/text`), 12 before that the
        // count of changes in `label`, `text`, 8 after an 8 px `staged_strong` square.
        for (theme, t) in both() {
            for changes in [1, 0] {
                let at_ = format!("{theme:?} changes {changes}");
                let (shapes, at) = drawn(theme, |ui| {
                    PagingBar { page: "Page 1 of 250", back: false, forward: true, changes, can_apply: true }.show(ui);
                });
                assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                let fills = fills_in(&shapes, at);
                assert_eq!(fills.first(), Some(&(at, t.card)), "{at_}: on card");
                assert!(fills.contains(&(Rect::from_min_size(at.min, vec2(W, 1.0)), t.line)), "{at_}: the line along its top: {fills:?}");
                let mid = at.top() + 1.0 + 17.5;
                let (page, c) = word(&shapes, "PAGE 1 OF 250", &at_);
                assert_eq!(c, t.text, "{at_}: the page in text");
                assert!(close(page.left(), at.left() + 8.0 + 56.0) && close(page.center().y, mid), "{at_}: the page {page:?}");
                // The arrows: first and previous off on the first page, next and last on.
                for (k, colour) in [(0.0, t.text_faint), (1.0, t.text_faint)] {
                    let cell = Rect::from_min_size(pos2(at.left() + 8.0 + 28.0 * k, mid - 14.0), vec2(28.0, 28.0));
                    assert_eq!(icon_in(&shapes, cell).0, [colour], "{at_}: arrow {k} off");
                }
                let next = Rect::from_min_size(pos2(page.right() - Text::Label.spacing(), mid - 14.0), vec2(28.0, 28.0));
                assert_eq!(icon_in(&shapes, next.expand(1.0)).0, [t.text], "{at_}: NEXT on, right after the page");
                assert_eq!(icon_in(&shapes, next.translate(vec2(28.0, 0.0)).expand(1.0)).0, [t.text], "{at_}: LAST on");
                if changes == 0 {
                    for w in ["1 CHANGE", "RESET", "APPLY"] {
                        assert!(texts(&shapes).iter().all(|(s, _, _)| s != w), "{at_}: no `{w}`");
                    }
                    continue;
                }
                let (apply, ac) = word(&shapes, "APPLY", &at_);
                let button = fills.iter().find(|(r, c)| *c == t.ink && r.contains_rect(apply)).map(|(r, _)| *r).unwrap_or(Rect::NOTHING);
                assert_eq!(ac, t.on_ink, "{at_}: APPLY on ink");
                assert!(
                    close(at.right() - button.right(), 12.0) && close(button.height(), 28.0) && close(button.center().y, mid),
                    "{at_}: APPLY {button:?}"
                );
                let (reset, rc) = word(&shapes, "RESET", &at_);
                assert_eq!(rc, t.text, "{at_}: RESET");
                assert!(
                    close(button.left() - 4.0 - 10.0, reset.right() - Text::ButtonCaps.spacing())
                        || (button.left() - 4.0 - 10.0 - reset.right()).abs() <= 1.0,
                    "{at_}: RESET 4 before APPLY: {reset:?} {button:?}"
                );
                let (count, cc) = word(&shapes, "1 CHANGE", &at_);
                assert_eq!(cc, t.text, "{at_}: the count in text");
                assert!(
                    (reset.left() - 10.0 - count.right()).abs() <= 12.0 + 1.0 && reset.left() - 10.0 - count.right() >= 11.0 - 1.0,
                    "{at_}: 12 between the count and RESET: {count:?} {reset:?}"
                );
                let mark = Rect::from_min_size(pos2(count.left() - 8.0 - 8.0, mid - 4.0), vec2(8.0, 8.0));
                assert!(
                    fills.iter().any(|(r, c)| *c == t.staged_strong && (r.min - mark.min).length() <= 0.6 && close(r.width(), 8.0)),
                    "{at_}: the staged mark {mark:?}: {fills:?}"
                );
            }
        }
    }

    #[test]
    fn the_paging_bar_says_what_was_asked() {
        let mut h = crate::components::harness(Theme::Light, Vec::new(), |ui, seen: &mut Vec<PagingAsked>| {
            ui.allocate_ui(vec2(580.0, 36.0), |ui| {
                seen.extend(PagingBar { page: "Page 2", back: true, forward: true, changes: 3, can_apply: true }.show(ui).asked)
            });
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["first", "previous", "next", "last", "Reset", "Apply"] {
            h.get_by_label(label).click();
            h.run();
        }
        let all = [PagingAsked::First, PagingAsked::Previous, PagingAsked::Next, PagingAsked::Last, PagingAsked::Reset, PagingAsked::Apply];
        assert_eq!(*h.state(), all);
    }

    /// The filter bar as the table tab draws it, `W` wide, with `filters` on `columns`: its tags.
    fn filter_bar_drawn(theme: Theme, columns: Vec<(&'static str, &'static str)>, filters: Vec<Filter>) -> (Vec<ClippedShape>, Rect, Tags) {
        let mut tags = Tags::default();
        let mut at = Rect::NOTHING;
        let shapes = painted_in(vec2(900.0, 300.0), theme, |ui| {
            tags.begin_frame();
            let bar = Bar {
                tab: TabId(1),
                columns: columns.clone(),
                filters: &filters,
                raw: "",
                raw_mode: false,
                disabled: None,
                blocked: None,
                dialect: Dialect::Postgres,
            };
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(W);
                    let top = ui.cursor().min;
                    filter_bar::show(ui, &bar, &mut tags);
                    at = Rect::from_min_max(top, pos2(top.x + W, ui.min_rect().bottom()));
                });
            });
        });
        (shapes, at, tags)
    }

    #[test]
    fn the_filter_row_matches_the_page() {
        // Node 164428 in the page's filter bar (165200): on `card`, its rows 4 from its top and
        // bottom and 12 from its left, a 1 px `line` under it. A row is 36 tall, its controls 28
        // tall, 4 from its top, 8 apart: the raw SQL toggle (`button/icon` boxed, code), the column
        // and the operator (`dropdown`), the value (`input` data), remove, add (boxed, minus and
        // plus) and APPLY (`button/box` outline). An AND row starts with AND in `label` grey,
        // centred in 28.
        let columns = vec![("dealer_id", "integer"), ("status", "text")];
        let filters = vec![
            Filter { column: "dealer_id".into(), op: Op::Eq, value: "42".into() },
            Filter { column: "status".into(), op: Op::Eq, value: "completed".into() },
        ];
        for (theme, t) in both() {
            let at_ = format!("{theme:?}");
            let (shapes, at, tags) = filter_bar_drawn(theme, columns.clone(), filters.clone());
            let rect = |id: &str| tags.rect(id).unwrap_or_else(|| panic!("{at_}: `{id}` drawn: {:?}", tags.ids()));
            assert!(close(at.height(), 4.0 + 36.0 + 36.0 + 4.0 + 1.0), "{at_}: two rows of 36, 4 above and below, the line: {at:?}");
            let fills = fills_in(&shapes, at);
            assert_eq!(fills.first(), Some(&(at, t.card)), "{at_}: on card");
            let line = Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max);
            assert!(fills.contains(&(line, t.line)), "{at_}: the line under it: {fills:?}");
            let first = ["filter-raw-toggle", "filter-0-column", "filter-0-op", "filter-0-value", "filter-0-remove", "filter-add", "filter-apply"];
            let second = ["filter-1-column", "filter-1-op", "filter-1-value", "filter-1-remove"];
            for (row, ids) in [(0.0, &first[..]), (1.0, &second[..])] {
                let top = at.top() + 4.0 + 36.0 * row + 4.0;
                let mut x = at.left() + 12.0 + if row == 0.0 { 0.0 } else { 28.0 + 8.0 };
                for id in ids {
                    let r = rect(id);
                    assert!(close(r.top(), top) && close(r.height(), 28.0), "{at_}: `{id}` 28 tall, 4 into its row: {r:?}");
                    assert!(close(r.left(), x), "{at_}: `{id}` at {x}, 8 after the one before: {r:?}");
                    x = r.right() + 8.0;
                    let edge = Edge { outer: r, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
                    assert!(edges_in(&shapes, r, 0.0).contains(&edge), "{at_}: `{id}`'s line_strong edge");
                }
            }
            // The toggle's code icon, minus and plus in `text`.
            for id in ["filter-raw-toggle", "filter-0-remove", "filter-add"] {
                assert_eq!(icon_in(&shapes, rect(id)).0, [t.text], "{at_}: `{id}`'s icon");
            }
            // Dropdowns and the value on `card`; APPLY has no ground of its own.
            for id in ["filter-0-column", "filter-0-op", "filter-0-value"] {
                assert!(fills.contains(&(rect(id), t.card)), "{at_}: `{id}` on card");
            }
            assert!(!fills.iter().any(|(r, _)| *r == rect("filter-apply")), "{at_}: APPLY outline");
            assert_eq!(word(&shapes, "APPLY", &at_).1, t.text, "{at_}: APPLY's words");
            assert_eq!(font_of(&shapes, "APPLY"), Some(Text::ButtonCaps.font()));
            for (words, font) in [("dealer_id", Text::Body), ("equals (=)", Text::Body), ("42", Text::Data), ("completed", Text::Data)] {
                assert_eq!(word(&shapes, words, &at_).1, t.text, "{at_}: `{words}` in text");
                assert_eq!(font_of(&shapes, words), Some(font.font()), "{at_}: `{words}`'s font");
            }
            let (and, c) = word(&shapes, "AND", &at_);
            assert_eq!(c, t.text_muted, "{at_}: AND grey");
            let and_box = Rect::from_min_size(pos2(at.left() + 12.0, at.top() + 4.0 + 36.0 + 4.0), vec2(28.0, 28.0));
            assert!((and.center() - and_box.center()).length() <= 1.0, "{at_}: AND {and:?} centred in {and_box:?}");
        }
    }

    #[test]
    fn a_column_name_keeps_its_case_in_the_row() {
        // A column name with capitals and quotes, drawn and tagged as written in the row.
        let filters = vec![Filter { column: "\"Mood\"".into(), op: Op::Eq, value: "Happy".into() }];
        let (shapes, _, tags) = filter_bar_drawn(Theme::Light, vec![("\"Mood\"", "text")], filters);
        let words: Vec<String> = texts(&shapes).into_iter().map(|(w, _, _)| w).collect();
        assert!(words.iter().any(|w| w == "\"Mood\"") && words.iter().any(|w| w == "Happy"), "as written: {words:?}");
        assert!(!words.iter().any(|w| w.contains("MOOD") || w.contains("HAPPY")), "nothing in capitals: {words:?}");
        assert_eq!(tags.value("filter-0-column"), Some(&serde_json::json!("\"Mood\"")));
    }
}

//! The table tab's bars: the table bar over the rows (`table bar`), the filter bar over it
//! (`filter row`), and the bottom bar under the rows or the structure's sections (`paging bar`):
//! the DATA / STRUCTURE switch at its left end, in the same place in both views, the paging in
//! its middle and the staged changes at its right end.
//!
//! The table bar and the bottom bar are views: they take what to show and say what was clicked;
//! the screen tags what they drew, says on hover why something is off, and acts. The table tab
//! ([`crate::table_tab`]) draws the table bar in its data or structure state, and
//! its Value cell (drawn last, so Tab reaches it after the count). The filter bar is a frame the screen's filter
//! rows stand in ([`filter_bar`], [`filter_row`], [`and_mark`]): [`crate::filter_bar`] draws its
//! controls there, from the shared controls.

use std::sync::Arc;
use std::time::Duration;

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rangef, Rect, Response, Sense, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2};

use super::button::{BarButton, BoxButton, IconButton, TextButton};
use super::icon::Icon;
use super::results::{CountShown, RowCount, Time};
use super::segmented::Segmented;
use super::status::{Piece, piece};
use super::tooltip::Hint;
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, CHANGES_GAP, FILTER_BAR_PAD_X, FILTER_BAR_PAD_Y, FILTER_GAP, FILTER_ROW_H, ICON_BUTTON, PAGING_GAP, PAGING_NOTE_GAP,
    PAGING_NOTE_MIN, PAGING_PAD_RIGHT, STAGED_MARK, STAGED_MARK_GAP, TABLE_BAR_GAP, TABLE_BAR_PAD_X, TABLE_CANCEL_PAD_X, TABLE_LOADING_GAP,
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

/// A page on its way, for `elapsed` so far (the tab shows it only once it has gone 300 ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loading {
    pub elapsed: Duration,
}

/// The table bar: REFRESH (CANCEL while a page loads), + ROW, the rows' count, IMPORT, EXPORT and
/// the value panel's toggle. The structure view has REFRESH and its facts only. The view switch
/// is the bottom bar's ([`PagingBar`]).
#[derive(Clone, Copy, Debug)]
pub struct TableBar {
    /// The view it is the bar of: its state.
    pub view: View,
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
    /// The toggle drawn off: the value panel has no room beside the grid.
    pub value_off: bool,
}

/// What a click in the table bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAsked {
    Refresh,
    Cancel,
    AddRow,
    Count,
    Value,
}

/// The table bar as drawn: each part shown, the form it took for its room, and what a click
/// asked for.
pub struct TableShown {
    pub refresh: Option<Response>,
    pub cancel: Option<Response>,
    pub add: Option<Response>,
    pub count: Option<CountShown>,
    /// LOADING, or the structure's facts.
    pub facts: Option<Response>,
    pub import: Option<Response>,
    pub export: Option<Response>,
    pub value: Option<Response>,
    pub fit: TableFit,
    pub asked: Option<TableAsked>,
}

/// Which of the table bar's parts show, short of room. They give way in this order (the first
/// first): the count's range (`1–100 of`), IMPORT and EXPORT, REFRESH's and + ROW's words (their
/// icons stay), the count's words (COUNT, COUNT ROWS or COUNTING stay), then the facts. The
/// structure state's facts go first, then REFRESH's word ([`TableFit::STRUCTURE_FORMS`]). Past
/// that the left group is cut at the bar's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableFit {
    pub range: bool,
    pub import_export: bool,
    pub words: bool,
    pub count_words: bool,
    pub count: bool,
}

impl TableFit {
    /// Every form, from the widest to the narrowest: each gives way one step more than the one
    /// before it.
    pub const FORMS: [TableFit; 6] = [
        TableFit { range: true, import_export: true, words: true, count_words: true, count: true },
        TableFit { range: false, import_export: true, words: true, count_words: true, count: true },
        TableFit { range: false, import_export: false, words: true, count_words: true, count: true },
        TableFit { range: false, import_export: false, words: false, count_words: true, count: true },
        TableFit { range: false, import_export: false, words: false, count_words: false, count: true },
        TableFit { range: false, import_export: false, words: false, count_words: false, count: false },
    ];

    /// The structure state's forms (it has no count, IMPORT or EXPORT), from the widest: every
    /// part; the facts gone; REFRESH's word gone too (CANCEL keeps its word).
    pub const STRUCTURE_FORMS: [TableFit; 3] = [
        TableFit { range: true, import_export: true, words: true, count_words: true, count: true },
        TableFit { range: false, import_export: false, words: true, count_words: false, count: false },
        TableFit { range: false, import_export: false, words: false, count_words: false, count: false },
    ];
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
    pub fn structure_words(&self) -> String {
        let columns = if self.columns == 1 { "1 column".to_string() } else { format!("{} columns", self.columns) };
        match self.count.words(None) {
            rows if rows.is_empty() => columns,
            rows => format!("{columns}  {rows}"),
        }
    }

    /// REFRESH and + ROW, with their words or without. REFRESH is the bar's first cell: no
    /// divider before it, its icon the cell's own 12 from the bar's left end.
    fn refresh_button(words: bool) -> BarButton<'static> {
        BarButton::label(Icon::Refresh, "Refresh").over_line().words(words).divider(false)
    }

    fn add_button(words: bool) -> BarButton<'static> {
        BarButton::label(Icon::Plus, "Row").over_line().words(words)
    }

    /// The facts between the cells: the count, LOADING with its time, or the structure's.
    fn facts(&self, ui: &Ui) -> Facts {
        let t = tokens(ui.ctx());
        match self.loading {
            Some(l) => Facts::Loading(Piece::new(ui, Text::Label.job("Loading", t.text_muted), "Loading"), Time::timer(ui, t, l.elapsed)),
            None if self.view == View::Data => Facts::Count,
            None => {
                let words = self.structure_words();
                Facts::Structure(Piece::new(ui, Text::Label.job(&words, t.text_muted), words))
            }
        }
    }

    /// The rows shown (`1–100`) in the count, when `fit` keeps them.
    fn range(&self, fit: TableFit) -> Option<(u64, u64)> {
        self.shown.filter(|_| fit.range)
    }

    /// How wide `facts` draws in form `fit`; 0 when it draws nothing.
    fn facts_width(&self, ui: &Ui, facts: &Facts, fit: TableFit) -> f32 {
        if !fit.count {
            return 0.0;
        }
        match facts {
            Facts::Count if fit.count_words => self.count.width(ui, self.range(fit)),
            Facts::Count => self.count.bare_width(ui),
            Facts::Loading(words, time) => words.width() + TABLE_LOADING_GAP + time.width(),
            Facts::Structure(words) => words.width(),
        }
    }

    /// IMPORT, EXPORT and the toggle at the bar's right end (the data view's only).
    fn right_buttons(&self) -> (BarButton<'static>, BarButton<'static>, Option<BarButton<'static>>) {
        let import = BarButton::label(Icon::Import, "Import").over_line();
        let export = BarButton::label(Icon::Export, "Export").over_line().divider(false);
        (import, export, self.value.map(|on| BarButton::icon(Icon::Panel).on(on).over_line()))
    }

    /// The first form ([`TableFit::FORMS`]; [`TableFit::STRUCTURE_FORMS`] in the structure
    /// state) that fits a bar `room` wide, each form taking room only from the steps before it;
    /// the narrowest when none does (its left group then cut at the bar's end).
    pub fn fit(&self, ui: &Ui, room: f32) -> TableFit {
        let facts = self.facts(ui);
        let data = self.view == View::Data;
        let (import, export, toggle) = self.right_buttons();
        let toggle_w = toggle.as_ref().map_or(0.0, |b| b.width(ui));
        let io_w = import.width(ui) + export.width(ui);
        let needs = |fit: TableFit| {
            let first = match self.loading {
                Some(_) => cancel_width(ui),
                None => Self::refresh_button(fit.words).width(ui),
            };
            let add = if data { Self::add_button(fit.words).width(ui) } else { 0.0 };
            let right = if data { toggle_w + if fit.import_export { io_w } else { 0.0 } } else { 0.0 };
            let facts = match self.facts_width(ui, &facts, fit) {
                w if w > 0.0 => w + 2.0 * TABLE_BAR_GAP,
                _ => 0.0,
            };
            first + add + facts + right
        };
        let forms: &[TableFit] = if data { &TableFit::FORMS } else { &TableFit::STRUCTURE_FORMS };
        let narrowest = forms.last().copied().unwrap_or(TableFit::FORMS[5]);
        forms.iter().copied().find(|&fit| needs(fit) <= room).unwrap_or(narrowest)
    }

    /// Draws it as wide as `ui`, 36 tall, in the first form that fits ([`TableBar::fit`]); short
    /// of all room the left group is cut at the bar's end. Nothing is drawn past the bar.
    pub fn show(self, ui: &mut Ui) -> TableShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui, Line::Bottom);
        let clip = ui.clip_rect().intersect(cell);
        let mid = cell.center().y;
        let data = self.view == View::Data;
        let fit = self.fit(ui, cell.width());
        let mut asked = None;
        // The left, from the bar's end: REFRESH (or CANCEL's cell) and + ROW.
        let mut left = ui.new_child(UiBuilder::new().id_salt(bar.id.with("left")).max_rect(cell).layout(Layout::left_to_right(Align::Center)));
        left.set_clip_rect(clip);
        left.spacing_mut().item_spacing.x = 0.0;
        let (refresh, cancel) = match self.loading {
            Some(_) => (None, Some(cancel_cell(&mut left))),
            None => (Some(left.add_enabled(self.can_refresh, Self::refresh_button(fit.words))), None),
        };
        let add = data.then(|| left.add_enabled(self.can_add, Self::add_button(fit.words)));
        let left_end = left.min_rect().right();
        // The right end: IMPORT, EXPORT and the toggle (when there is one), off in the structure
        // view.
        let (import, export, toggle) = self.right_buttons();
        let toggle_w = toggle.as_ref().map_or(0.0, |b| b.width(ui));
        let io = fit.import_export;
        let right_w: f32 = if data { toggle_w + if io { import.width(ui) + export.width(ui) } else { 0.0 } } else { 0.0 };
        let right_x = (cell.right() - right_w).max(left_end);
        // Between them, ending 12 before IMPORT (or the bar's end): the count, LOADING, or the
        // structure's facts; none when the form leaves it out or there is no room for it.
        let facts = self.facts(ui);
        let needed = self.facts_width(ui, &facts, fit);
        let end = if data { right_x } else { cell.right() } - TABLE_BAR_GAP;
        let room = end - left_end - TABLE_BAR_GAP;
        let mut count = None;
        let mut shown_facts = None;
        if needed > 0.0 && needed <= room {
            let lane = |ui: &mut Ui, salt: &str, at: Rect| {
                let mut c = ui.new_child(UiBuilder::new().id_salt(bar.id.with(salt)).max_rect(at).layout(Layout::left_to_right(Align::Center)));
                c.set_clip_rect(clip);
                c
            };
            let at = Rect::from_min_max(pos2(end - needed, cell.top()), pos2(end, cell.bottom()));
            match facts {
                Facts::Count => {
                    let mut c = lane(ui, "count", at);
                    if !self.can_count {
                        c.disable();
                    }
                    let shown = if fit.count_words { self.count.show(&mut c, self.range(fit)) } else { self.count.show_bare(&mut c) };
                    if shown.asked {
                        asked = Some(TableAsked::Count);
                    }
                    count = Some(shown);
                }
                Facts::Loading(words, time) => {
                    let c = lane(ui, "loading-lane", at);
                    let x = at.left();
                    shown_facts = Some(piece(&c, bar.id.with("loading"), &words, pos2(x, mid), t.text_muted));
                    let _ = time.show(&c, bar.id.with("timer"), x + words.width() + TABLE_LOADING_GAP, mid, t);
                }
                Facts::Structure(words) => {
                    let c = lane(ui, "facts-lane", at);
                    shown_facts = Some(piece(&c, bar.id.with("facts"), &words, pos2(at.left(), mid), t.text_muted));
                }
            }
        }
        // The right end last, so Tab reaches the count before the toggle.
        let (import, export, value) = if data {
            let lane = Rect::from_min_max(pos2(right_x, cell.top()), cell.max);
            let mut right = ui.new_child(UiBuilder::new().id_salt(bar.id.with("right")).max_rect(lane).layout(Layout::left_to_right(Align::Center)));
            right.spacing_mut().item_spacing.x = 0.0;
            right.set_clip_rect(clip);
            let (import, export) = if io {
                (
                    Some(right.add_enabled(false, import).hint_disabled(IMPORT_LATER)),
                    Some(right.add_enabled(false, export).hint_disabled(EXPORT_LATER)),
                )
            } else {
                (None, None)
            };
            (import, export, toggle.map(|toggle| right.add_enabled(!self.value_off, toggle)))
        } else {
            (None, None, None)
        };
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
        TableShown { refresh, cancel, add, count, facts: shown_facts, import, export, value, fit, asked }
    }
}

/// How wide CANCEL's cell draws in `ui`.
fn cancel_width(ui: &Ui) -> f32 {
    2.0 * TABLE_CANCEL_PAD_X + BoxButton::primary("Cancel").width(ui)
}

/// CANCEL in its cell, in REFRESH's place while a page loads: the bar's first cell, with no
/// divider before it, `button/box` primary 12 in from either side.
fn cancel_cell(ui: &mut Ui) -> Response {
    let button = BoxButton::primary("Cancel");
    let size = vec2(cancel_width(ui), BAR_CELL_H);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(size);
        ui.add_space(TABLE_CANCEL_PAD_X);
        ui.add(button)
    })
    .inner
}

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

/// The bottom bar of a table tab, in both of its views: the DATA / STRUCTURE switch at its left
/// end; in its middle the paging (FIRST, PREVIOUS, the page, NEXT, LAST, then a cut page's note);
/// and with staged changes, at its right end, the staged mark and their count, RESET and APPLY.
/// The page's words and the count are facts, drawn grey. Without [`PagingBar::pages`] (the
/// structure view, or no page loaded) it holds the switch alone, in the same place.
#[derive(Clone, Copy, Debug)]
pub struct PagingBar<'a> {
    /// The view that shows: the switch's chosen segment.
    pub view: View,
    /// STRUCTURE is on; off, it is drawn faint and a click on it asks nothing.
    pub structure: bool,
    /// The paging and the staged changes are drawn.
    pub pages: bool,
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
    /// The note on a page the size limit cut short, after the paging: `small` and grey, cut with
    /// `…` to the room left and whole on hover.
    pub note: Option<&'a str>,
}

/// What a click in the bottom bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingAsked {
    View(View),
    First,
    Previous,
    Next,
    Last,
    Reset,
    Apply,
}

/// The paging's parts as drawn.
pub struct PagesShown {
    pub first: Response,
    pub previous: Response,
    pub page: Response,
    pub next: Response,
    pub last: Response,
}

/// The bottom bar as drawn: each part shown, and what a click asked for.
pub struct PagingShown {
    /// The view switch's segments: DATA, STRUCTURE.
    pub view: Vec<Response>,
    /// The paging, when it is drawn and has room.
    pub pages: Option<PagesShown>,
    pub note: Option<Response>,
    pub changes: Option<Response>,
    pub reset: Option<Response>,
    pub apply: Option<Response>,
    pub asked: Option<PagingAsked>,
}

/// Which of the bottom bar's parts show, short of room. They give way in this order: the cut
/// page's note, the count of changes (APPLY's hover says it), the paging, RESET. The switch and
/// APPLY stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PagingFit {
    pages: bool,
    count: bool,
    reset: bool,
}

impl PagingBar<'_> {
    /// `1 change`, `3 changes`.
    fn changes_words(&self) -> String {
        if self.changes == 1 { "1 change".into() } else { format!("{} changes", self.changes) }
    }

    /// Draws it as wide as `ui`, 36 tall, its line along the top. The switch stands
    /// [`TABLE_BAR_PAD_X`] from the left end and the changes end [`PAGING_PAD_RIGHT`] from the
    /// right end, whatever else shows. The paging is centred on the bar's width; with too little
    /// room for that it moves off centre, kept [`CHANGES_GAP`] clear of the switch and of the
    /// changes. Short of room for all, the note goes first, then the count of changes, then the
    /// paging, then RESET; the switch and APPLY are cut only by a bar narrower than the two. No
    /// part is drawn over another, and none past the bar. Tab goes switch, paging, changes.
    pub fn show(self, ui: &mut Ui) -> PagingShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui, Line::Top);
        let mid = cell.center().y;
        let inner = Rect::from_min_max(pos2(cell.left() + TABLE_BAR_PAD_X, cell.top()), pos2(cell.right() - PAGING_PAD_RIGHT, cell.bottom()));
        let clip = ui.clip_rect().intersect(cell);
        let lane = |ui: &mut Ui, salt: &str, at: Rect| {
            let mut c = ui.new_child(UiBuilder::new().id_salt(bar.id.with(salt)).max_rect(at).layout(Layout::left_to_right(Align::Center)));
            c.spacing_mut().item_spacing.x = 0.0;
            c.set_clip_rect(clip);
            c
        };
        let mut asked = None;
        // The switch, first: Tab reaches it after what is above the bar and before the paging.
        let mut left = lane(ui, "view", inner);
        let mut view = self.view;
        let choices = [(View::Data, "Data"), (View::Structure, "Structure")];
        let switch = Segmented::caps(&mut view, &choices).off((!self.structure).then_some(View::Structure)).show(&mut left);
        if switch.row.changed() {
            asked = Some(PagingAsked::View(view));
        }
        let shown =
            |pages, note, changes, reset, apply, asked| PagingShown { view: switch.segments.clone(), pages, note, changes, reset, apply, asked };
        if !self.pages {
            return shown(None, None, None, None, None, asked);
        }
        // What the rest needs, and the form that fits between the switch and the bar's end.
        let (lo, hi) = (switch.row.rect.right() + CHANGES_GAP, inner.right());
        let words = Piece::new(ui, Text::Label.job(self.page, t.text_muted), self.page);
        let pages_w = 4.0 * ICON_BUTTON + words.width();
        let reset_b = TextButton::new("Reset");
        let apply_b = BoxButton::primary("Apply");
        let (reset_w, apply_w) = (reset_b.width(ui) + PAGING_GAP, apply_b.width(ui));
        let says = self.changes_words();
        let count = Piece::new(ui, Text::Label.job(&says, t.text_muted), says);
        let count_w = STAGED_MARK + STAGED_MARK_GAP + count.width() + CHANGES_GAP;
        let staged = self.changes > 0;
        let right_w =
            |fit: PagingFit| if staged { apply_w + if fit.reset { reset_w } else { 0.0 } + if fit.count { count_w } else { 0.0 } } else { 0.0 };
        let needs = |fit: PagingFit| right_w(fit) + if fit.pages { pages_w + if staged { CHANGES_GAP } else { 0.0 } } else { 0.0 };
        let forms = [
            PagingFit { pages: true, count: true, reset: true },
            PagingFit { pages: true, count: false, reset: true },
            PagingFit { pages: false, count: false, reset: true },
            PagingFit { pages: false, count: false, reset: false },
        ];
        let narrowest = PagingFit { pages: false, count: false, reset: false };
        let fit = forms.into_iter().find(|f| needs(*f) <= hi - lo).unwrap_or(narrowest);
        let right_x = (hi - right_w(fit)).max(lo);
        // The paging, centred on the bar, or as near to that as the switch and the changes leave.
        let pages_end = if staged { right_x - CHANGES_GAP } else { hi };
        let pages_x = (cell.center().x - pages_w / 2.0).min(pages_end - pages_w).max(lo);
        let pages = fit.pages.then(|| {
            let mut c = lane(ui, "pages", Rect::from_min_max(pos2(pages_x, inner.top()), pos2(pages_x + pages_w, inner.bottom())));
            let first = c.add_enabled(self.back, IconButton::plain(Icon::First));
            let previous = c.add_enabled(self.back, IconButton::plain(Icon::Previous));
            let (at, _) = c.allocate_exact_size(vec2(words.width(), ICON_BUTTON), Sense::hover());
            // As `piece` draws it, but only as wide as its letters: the spacing egui puts after the
            // last one runs into NEXT.
            let size = words.galley.size();
            c.painter().galley(pos2(at.left(), mid - size.y / 2.0), Arc::clone(&words.galley), t.text_muted);
            let page = c.interact(
                Rect::from_min_size(pos2(at.left(), mid - size.y / 2.0), vec2(words.width(), size.y)),
                bar.id.with("page"),
                Sense::hover(),
            );
            let says = self.page.to_owned();
            page.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, says.clone()));
            let next = c.add_enabled(self.forward, IconButton::plain(Icon::Next));
            let last = c.add_enabled(self.forward, IconButton::plain(Icon::Last));
            PagesShown { first, previous, page, next, last }
        });
        if pages.is_none() {
            // The paging's lane's place, so what follows keeps its ids.
            ui.skip_ahead_auto_ids(1);
        }
        // The note, after the paging, only while the count of changes has its room. Without it,
        // its lane's place is skipped, so what follows keeps its ids.
        let note_room = Rect::from_min_max(pos2(pages_x + pages_w + PAGING_NOTE_GAP, inner.top()), pos2(right_x - CHANGES_GAP, inner.bottom()));
        let note = self.note.filter(|_| fit.pages && (fit.count || !staged) && note_room.width() >= PAGING_NOTE_MIN);
        if note.is_none() {
            ui.skip_ahead_auto_ids(1);
        }
        let note = note.map(|note| {
            let mut n = lane(ui, "note", note_room);
            n.set_clip_rect(clip.intersect(note_room));
            n.add(egui::Label::new(Text::Small.rich(note).color(t.text_muted)).truncate())
        });
        let (mut changes, mut reset, mut apply) = (None, None, None);
        if staged {
            let mut right = lane(ui, "right", Rect::from_min_max(pos2(right_x, inner.top()), pos2(right_x + right_w(fit), inner.bottom())));
            if fit.count {
                let (mark, _) = right.allocate_exact_size(vec2(STAGED_MARK, STAGED_MARK), Sense::hover());
                right.painter().rect_filled(mark, CornerRadius::ZERO, t.staged_strong);
                right.add_space(STAGED_MARK_GAP);
                let (at, _) = right.allocate_exact_size(vec2(count.width(), ICON_BUTTON), Sense::hover());
                changes = Some(piece(&right, bar.id.with("changes"), &count, pos2(at.left(), mid), t.text_muted));
                right.add_space(CHANGES_GAP);
            } else {
                // The mark's and the count's places, so RESET and APPLY keep their ids.
                right.skip_ahead_auto_ids(2);
            }
            if fit.reset {
                reset = Some(right.add(reset_b));
            }
            // In a lane of its own, at the bar's end: its id is the same whatever stands before it.
            let end = right_x + right_w(fit);
            let mut last = lane(ui, "apply", Rect::from_min_max(pos2(end - apply_w, inner.top()), pos2(end, inner.bottom())));
            apply = Some(last.add_enabled(self.can_apply, apply_b));
        }
        let clicked = |r: &Option<Response>| r.as_ref().is_some_and(Response::clicked);
        let page = |f: fn(&PagesShown) -> &Response| pages.as_ref().is_some_and(|p| f(p).clicked());
        if page(|p| &p.first) {
            asked = Some(PagingAsked::First);
        } else if page(|p| &p.previous) {
            asked = Some(PagingAsked::Previous);
        } else if page(|p| &p.next) {
            asked = Some(PagingAsked::Next);
        } else if page(|p| &p.last) {
            asked = Some(PagingAsked::Last);
        } else if clicked(&reset) {
            asked = Some(PagingAsked::Reset);
        } else if clicked(&apply) {
            asked = Some(PagingAsked::Apply);
        }
        shown(pages, note, changes, reset, apply, asked)
    }
}

/// The filter bar's frame: `card` as wide as `ui`, its rows [`FILTER_BAR_PAD_Y`] from its top and
/// bottom and [`FILTER_BAR_PAD_X`] from either side, a 1 px `line` under it; `add` draws the rows
/// top-down ([`filter_row`]). A row wider than the bar (a narrow tab) is cut at the bar's right
/// edge: nothing is drawn past it, and the bar takes only its own width in `ui`.
pub fn filter_bar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens(ui.ctx());
    let (x, y) = (FILTER_BAR_PAD_X as i8, FILTER_BAR_PAD_Y as i8);
    let margin = Margin { left: x, right: x, top: y, bottom: y + BAR_LINE as i8 };
    let room = ui.available_rect_before_wrap();
    let mut inside = ui.new_child(UiBuilder::new().id_salt("filter-bar").max_rect(room).layout(Layout::top_down(Align::Min)));
    inside.set_clip_rect(ui.clip_rect().intersect(Rect::from_x_y_ranges(room.x_range(), Rangef::EVERYTHING)));
    let shown = Frame::NONE.fill(t.card).inner_margin(margin).show(&mut inside, |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
    });
    let (rect, _) = ui.allocate_exact_size(vec2(room.width(), shown.response.rect.height()), Sense::hover());
    inside.painter().rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - BAR_LINE), rect.max), CornerRadius::ZERO, t.line);
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
        // Node 164278, without the view switch and the divider after it (they moved to the bottom
        // bar): 36 tall on `card`, a 1 px `line` along its bottom; from its left end REFRESH, with
        // no divider before it and its icon 12 in, then + ROW with its divider; at the right end
        // IMPORT and EXPORT (83 each, EXPORT without a divider) and the value panel's toggle (36);
        // the count ends 12 before IMPORT. Structure: REFRESH only, then
        // `8 COLUMNS  ~25,000 ROWS` 12 from the end. Loading: CANCEL (`button/box` primary) 12 in,
        // in REFRESH's place, + ROW after it, LOADING 12 before its timer (`data`) in the count's
        // place.
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
                value_off: false,
            };
            for (state, b) in [
                ("data", bar(View::Data, None)),
                ("structure", bar(View::Structure, None)),
                ("loading", bar(View::Data, Some(Loading { elapsed: s(800) }))),
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
                // No view switch: the bar starts with its first cell, and nothing divides it from
                // the bar's end.
                for w in ["DATA", "STRUCTURE"] {
                    assert!(texts(&shapes).iter().all(|(s, _, _)| s != w), "{at_}: no `{w}` in the table bar");
                }
                assert_eq!(edges_in(&shapes, at, 0.0).iter().filter(|e| e.colour == t.line_strong).count(), 0, "{at_}: no switch's edge");
                let cells_from = at.left();
                assert!(!divider(&shapes, &t, cells_from, at.top(), 35.0), "{at_}: no divider at the bar's left end");
                let first_cell = |w: &str| word(&shapes, w, &at_).0;
                match state {
                    "loading" => {
                        let (cancel, c) = word(&shapes, "CANCEL", &at_);
                        assert_eq!(c, t.on_ink, "{at_}: CANCEL on ink");
                        let button = fills.iter().find(|(r, c)| *c == t.ink && r.contains_rect(cancel)).map(|(r, _)| *r).unwrap_or(Rect::NOTHING);
                        assert!(close(button.left(), cells_from + 12.0) && close(button.height(), 28.0), "{at_}: CANCEL's button {button:?}");
                        assert!((button.right() + 12.0 - (cells_from + 91.0)).abs() <= 1.5, "{at_}: its cell 91 wide");
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
                        assert!(close(refresh.left(), cells_from + 30.0), "{at_}: REFRESH's words 30 into its cell: {refresh:?}");
                        let (icons, _) = icon_in(&shapes, Rect::from_min_size(pos2(cells_from + 11.0, mid - 7.0), vec2(14.0, 14.0)));
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
                loading: None,
                count: RowCount::Unknown,
                shown: Some((1, 100)),
                columns: 3,
                can_refresh: true,
                can_add: true,
                can_count: true,
                value: Some(true),
                value_off: false,
            };
            ui.allocate_ui(vec2(840.0, 36.0), |ui| seen.extend(b.show(ui).asked));
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["Refresh", "Row", "Count rows", "panel", "Import", "Export"] {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(*h.state(), [TableAsked::Refresh, TableAsked::AddRow, TableAsked::Count, TableAsked::Value], "IMPORT and EXPORT ask nothing");
        h.get_by_label("Import").hover();
        h.run();
        h.run();
        assert!(texts(&h.output().shapes).iter().any(|(w, _, _)| w == IMPORT_LATER), "IMPORT's hover says why");
    }

    /// The table bar with no Value cell.
    fn screen_bar(count: RowCount) -> TableBar {
        TableBar {
            view: View::Data,
            loading: None,
            count,
            shown: Some((1, 100)),
            columns: 3,
            can_refresh: true,
            can_add: true,
            can_count: true,
            value: None,
            value_off: false,
        }
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

    /// What a narrow table bar drew: each part's rect as seen (cut to what shows), with its name.
    type Parts = Vec<(&'static str, Option<Rect>)>;

    #[test]
    fn the_table_bar_gives_way_in_its_order() {
        // From wide to narrow (the brief §7): everything; the count without its range; IMPORT and
        // EXPORT gone too; REFRESH and + ROW without their words; the count without its words,
        // COUNT kept; the count gone; then the left group cut at the bar's end. No two parts
        // overlap and nothing is drawn past the bar.
        let seen_rect = |r: &Response| Some(r.interact_rect).filter(|r| r.is_positive());
        let mut seen: Vec<(u32, usize)> = Vec::new();
        for width in (40..=900u32).rev().step_by(10) {
            let mut h = crate::components::harness_in(
                vec2(width as f32 + 40.0, 100.0),
                Theme::Light,
                (Parts::new(), Rect::NOTHING),
                move |ui, out: &mut (Parts, Rect)| {
                    ui.allocate_ui(vec2(width as f32, 36.0), |ui| {
                        let top = ui.cursor().min;
                        let s = screen_bar(RowCount::Estimate(100_000)).show(ui);
                        out.1 = Rect::from_min_size(top, vec2(width as f32, 36.0));
                        let words = s.count.as_ref().and_then(|c| c.words.as_ref().and_then(seen_rect));
                        let count = s.count.as_ref().and_then(|c| c.count.as_ref().and_then(seen_rect));
                        let mut parts = Parts::new();
                        parts.extend([
                            ("refresh", s.refresh.as_ref().and_then(seen_rect)),
                            ("add", s.add.as_ref().and_then(seen_rect)),
                            ("words", words),
                            ("count", count),
                            ("import", s.import.as_ref().and_then(seen_rect)),
                            ("export", s.export.as_ref().and_then(seen_rect)),
                        ]);
                        out.0 = parts;
                    });
                },
            );
            h.run();
            let (parts, bar) = h.state().clone();
            let at = format!("{width} wide");
            let part = |name: &str| parts.iter().find(|(n, _)| *n == name).and_then(|(_, r)| *r);
            let shapes = &h.output().shapes;
            let all = texts(shapes);
            let range = all.iter().any(|(w, _, _)| w.starts_with("1–100"));
            let words = all.iter().any(|(w, _, _)| w == "REFRESH");
            // Each step of the ladder, in its order: shown or given way.
            let steps = [range, part("import").is_some(), words, part("words").is_some(), part("count").is_some()];
            let form = steps.iter().take_while(|s| !**s).count();
            assert!(steps[form..].iter().all(|s| *s), "{at}: a later step gave way before an earlier one: {steps:?}");
            assert!(seen.last().is_none_or(|&(_, last)| form >= last), "{at}: room taken back: {seen:?} then {form}");
            seen.push((width, form));
            for (i, (a, ra)) in parts.iter().enumerate() {
                let Some(ra) = ra else { continue };
                assert!(bar.expand(0.5).contains_rect(*ra), "{at}: `{a}` {ra:?} inside the bar {bar:?}");
                for (b, rb) in &parts[i + 1..] {
                    let Some(rb) = rb else { continue };
                    let o = ra.intersect(*rb);
                    assert!(!o.is_positive() || o.area() <= 0.5, "{at}: `{a}` {ra:?} over `{b}` {rb:?}");
                }
            }
            for s in shapes {
                let r = s.shape.visual_bounding_rect().intersect(s.clip_rect);
                assert!(!r.is_positive() || bar.expand(0.5).contains_rect(r), "{at}: drawn past the bar {bar:?}: {r:?}");
            }
            if form >= 2 {
                // A word that left its button still names it.
                use egui_kittest::kittest::Queryable;
                assert!(h.query_by_label("Refresh").is_some(), "{at}: REFRESH still named");
            }
        }
        assert_eq!(seen.first().map(|s| s.1), Some(0), "at 900 everything shows: {seen:?}");
        println!("the table bar's form by width (0 all … 5 the count gone): {seen:?}");
        for form in 0..=5 {
            assert!(seen.iter().any(|&(_, f)| f == form), "the sweep passes through form {form}: {seen:?}");
        }
    }

    #[test]
    fn the_structure_bar_gives_way_in_its_order() {
        // The brief §7, from wide to narrow: everything; the facts gone; REFRESH's word gone too
        // (its icon stays); then the left group cut at the bar's end. While a read shows its
        // loading state, LOADING goes first and CANCEL keeps its word. No two parts overlap and
        // nothing is drawn past the bar.
        let seen_rect = |r: &Response| Some(r.interact_rect).filter(|r| r.is_positive());
        for loading in [None, Some(Loading { elapsed: Duration::from_millis(800) })] {
            let mut seen: Vec<(u32, usize)> = Vec::new();
            for width in (20..=700u32).rev().step_by(10) {
                let mut h = crate::components::harness_in(
                    vec2(width as f32 + 40.0, 100.0),
                    Theme::Light,
                    (Parts::new(), Rect::NOTHING, false, false),
                    move |ui, out: &mut (Parts, Rect, bool, bool)| {
                        ui.allocate_ui(vec2(width as f32, 36.0), |ui| {
                            let top = ui.cursor().min;
                            let bar = TableBar {
                                view: View::Structure,
                                loading,
                                count: RowCount::Estimate(25_000),
                                shown: None,
                                columns: 9,
                                can_refresh: true,
                                can_add: false,
                                can_count: false,
                                value: None,
                                value_off: false,
                            };
                            let s = bar.show(ui);
                            out.1 = Rect::from_min_size(top, vec2(width as f32, 36.0));
                            out.2 = s.facts.is_some();
                            // The first cell whole: all of it can be reached.
                            out.3 = s.refresh.iter().chain(&s.cancel).all(|r| r.interact_rect.width() >= r.rect.width() - 0.5);
                            let mut parts = Parts::new();
                            parts.extend([
                                ("refresh", s.refresh.as_ref().and_then(seen_rect)),
                                ("cancel", s.cancel.as_ref().and_then(seen_rect)),
                                ("facts", s.facts.as_ref().and_then(seen_rect)),
                            ]);
                            out.0 = parts;
                        });
                    },
                );
                h.run();
                let (parts, bar, facts, first_whole) = h.state().clone();
                let at = format!("{width} wide, loading {}", loading.is_some());
                let part = |name: &str| parts.iter().find(|(n, _)| *n == name).and_then(|(_, r)| *r);
                let shapes = &h.output().shapes;
                let all = texts(shapes);
                let refresh_word = all.iter().any(|(w, _, _)| w == "REFRESH");
                let cancel_word = all.iter().any(|(w, _, _)| w == "CANCEL");
                let first = if loading.is_some() { part("cancel") } else { part("refresh") };
                // The left group whole: its last part drawn to its full width.
                let whole = first.is_some()
                    && first_whole
                    && match loading {
                        Some(_) => cancel_word,
                        None => true,
                    };
                // Each step of the ladder, in its order: shown or given way.
                let steps = if loading.is_some() { vec![facts, whole] } else { vec![facts, refresh_word, whole] };
                let form = steps.iter().take_while(|s| !**s).count();
                assert!(steps[form..].iter().all(|s| *s), "{at}: a later step gave way before an earlier one: {steps:?}");
                assert!(seen.last().is_none_or(|&(_, last)| form >= last), "{at}: room taken back: {seen:?} then {form}");
                seen.push((width, form));
                for (i, (a, ra)) in parts.iter().enumerate() {
                    let Some(ra) = ra else { continue };
                    assert!(bar.expand(0.5).contains_rect(*ra), "{at}: `{a}` {ra:?} inside the bar {bar:?}");
                    for (b, rb) in &parts[i + 1..] {
                        let Some(rb) = rb else { continue };
                        let o = ra.intersect(*rb);
                        assert!(!o.is_positive() || o.area() <= 0.5, "{at}: `{a}` {ra:?} over `{b}` {rb:?}");
                    }
                }
                for s in shapes {
                    let r = s.shape.visual_bounding_rect().intersect(s.clip_rect);
                    assert!(!r.is_positive() || bar.expand(0.5).contains_rect(r), "{at}: drawn past the bar {bar:?}: {r:?}");
                }
            }
            let last = if loading.is_some() { 2 } else { 3 };
            println!("the structure bar's form by width (loading {}): {seen:?}", loading.is_some());
            assert_eq!(seen.first().map(|s| s.1), Some(0), "at 700 everything shows: {seen:?}");
            for form in 0..=last {
                assert!(seen.iter().any(|&(_, f)| f == form), "the sweep passes through form {form}: {seen:?}");
            }
        }
    }

    /// DATA's bottom bar: the switch, the paging and `changes` staged changes.
    fn pages<'a>(page: &'a str, changes: usize, note: Option<&'a str>) -> PagingBar<'a> {
        PagingBar { view: View::Data, structure: true, pages: true, page, back: false, forward: true, changes, can_apply: true, note }
    }

    /// A drawn bottom bar's switch (its two segments together) and, when drawn, its paging from
    /// FIRST to LAST.
    fn switch_and_pages(s: &PagingShown) -> (Rect, Option<Rect>) {
        let switch = s.view.iter().fold(Rect::NOTHING, |a, r| a.union(r.rect));
        (switch, s.pages.as_ref().map(|p| p.first.rect.union(p.last.rect)))
    }

    #[test]
    fn the_paging_bar_matches_the_page() {
        // Node 164453 with the view switch in it: 36 tall on `card`, a 1 px `line` along its top.
        // At the left end, 12 in and centred under the line, the switch (caps segments in a 28
        // box, 139 wide). In the bar's middle the plain arrows (28) and `PAGE 1 OF 250` in
        // `label`, `text_muted`, side by side, centred on the bar's width. At the right end, 12
        // in, APPLY (`button/box` primary), 4 before it RESET (`button/text`), 12 before that the
        // count of changes in `label`, `text_muted`, 8 after an 8 px `staged_strong` square.
        for (theme, t) in both() {
            for changes in [1, 0] {
                let at_ = format!("{theme:?} changes {changes}");
                let mut seen = (Rect::NOTHING, None);
                let (shapes, at) = drawn(theme, |ui| seen = switch_and_pages(&pages("Page 1 of 250", changes, None).show(ui)));
                assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                let fills = fills_in(&shapes, at);
                assert_eq!(fills.first(), Some(&(at, t.card)), "{at_}: on card");
                assert!(fills.contains(&(Rect::from_min_size(at.min, vec2(W, 1.0)), t.line)), "{at_}: the line along its top: {fills:?}");
                let mid = at.top() + 1.0 + 17.5;
                // The switch: its box 12 in, 28 tall, centred; DATA chosen.
                let (segments, group) = seen;
                let switch = segments.expand(1.0);
                assert!(close(switch.left(), at.left() + 12.0) && close(switch.center().y, mid), "{at_}: the switch 12 in, centred: {switch:?}");
                assert!(close(switch.height(), 28.0) && (switch.width() - 139.0).abs() <= 2.0, "{at_}: its box: {switch:?}");
                let edges = edges_in(&shapes, switch.expand(0.5), 0.0);
                assert!(
                    edges.iter().any(|e| e.colour == t.line_strong && close(e.outer.left(), switch.left())),
                    "{at_}: the switch's edge: {edges:?}"
                );
                assert_eq!(word(&shapes, "DATA", &at_).1, t.on_ink, "{at_}: DATA chosen");
                assert_eq!(word(&shapes, "STRUCTURE", &at_).1, t.text, "{at_}: STRUCTURE not");
                // The paging, centred on the bar.
                let group = group.unwrap_or_else(|| panic!("{at_}: the paging drawn"));
                assert!(
                    close(group.center().x, at.center().x) && close(group.center().y, mid),
                    "{at_}: the paging centred on the bar {at:?}: {group:?}"
                );
                let (page, c) = word(&shapes, "PAGE 1 OF 250", &at_);
                assert_eq!(c, t.text_muted, "{at_}: the page grey: a fact");
                assert!(close(page.left(), group.left() + 56.0) && close(page.center().y, mid), "{at_}: the page after two arrows: {page:?}");
                // The arrows: first and previous off on the first page, next and last on.
                for (k, colour) in [(0.0, t.text_faint), (1.0, t.text_faint)] {
                    let cell = Rect::from_min_size(pos2(group.left() + 28.0 * k, mid - 14.0), vec2(28.0, 28.0));
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
                assert_eq!(cc, t.text_muted, "{at_}: the count grey: a fact");
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
    fn the_switch_stands_in_the_same_place_in_both_views() {
        // STRUCTURE's bottom bar holds the switch alone, and DATA's too while no page is loaded:
        // the switch where DATA's bar with its paging and changes has it, to the pixel; the
        // chosen view on `ink`.
        for (theme, t) in both() {
            let mut data = Rect::NOTHING;
            let _ = drawn(theme, |ui| data = switch_and_pages(&pages("Page 1 of 250", 1, None).show(ui)).0);
            for (view, chosen, other) in [(View::Structure, "STRUCTURE", "DATA"), (View::Data, "DATA", "STRUCTURE")] {
                let at_ = format!("{theme:?} {view:?} alone");
                let mut seen = (Rect::NOTHING, None);
                let (shapes, at) = drawn(theme, |ui| {
                    seen = switch_and_pages(&PagingBar { view, pages: false, ..pages("", 0, None) }.show(ui));
                });
                assert_eq!(seen, (data, None), "{at_}: the switch where DATA's bar has it, and no paging");
                assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                assert_eq!(fills_in(&shapes, at).first(), Some(&(at, t.card)), "{at_}: on card");
                assert_eq!(word(&shapes, chosen, &at_).1, t.on_ink, "{at_}: {chosen} chosen");
                assert_eq!(word(&shapes, other, &at_).1, t.text, "{at_}: {other} not");
                let words: Vec<String> = texts(&shapes).into_iter().map(|(w, _, _)| w).filter(|w| !w.is_empty()).collect();
                assert_eq!(words.len(), 2, "{at_}: the switch alone: {words:?}");
            }
        }
    }

    #[test]
    fn the_paging_bar_says_what_was_asked() {
        let mut h = crate::components::harness_in(vec2(900.0, 200.0), Theme::Light, Vec::new(), |ui, seen: &mut Vec<PagingAsked>| {
            ui.allocate_ui(vec2(840.0, 36.0), |ui| seen.extend(PagingBar { back: true, ..pages("Page 2", 3, None) }.show(ui).asked));
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["Data", "Structure", "first", "previous", "next", "last", "Reset", "Apply"] {
            h.get_by_label(label).click();
            h.run();
        }
        let all = [
            PagingAsked::View(View::Structure),
            PagingAsked::First,
            PagingAsked::Previous,
            PagingAsked::Next,
            PagingAsked::Last,
            PagingAsked::Reset,
            PagingAsked::Apply,
        ];
        assert_eq!(*h.state(), all, "the chosen DATA asks nothing");
    }

    #[test]
    fn tab_goes_from_the_switch_to_the_paging_to_the_changes() {
        // Reading order: DATA, STRUCTURE, the arrows, RESET, APPLY.
        let mut h = crate::components::harness_in(vec2(900.0, 200.0), Theme::Light, (), |ui, _| {
            ui.allocate_ui(vec2(840.0, 36.0), |ui| {
                PagingBar { back: true, ..pages("Page 2", 3, None) }.show(ui);
            });
        });
        h.run();
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut order = Vec::new();
        for _ in 0..8 {
            h.key_press(egui::Key::Tab);
            h.run();
            let focused = ["Data", "Structure", "first", "previous", "next", "last", "Reset", "Apply"]
                .into_iter()
                .find(|label| h.get_by_label(label).accesskit_node().is_focused());
            order.extend(focused);
        }
        assert_eq!(order, ["Data", "Structure", "first", "previous", "next", "last", "Reset", "Apply"]);
    }

    #[test]
    fn structure_off_is_drawn_faint_and_asks_nothing() {
        let off = |page| PagingBar { structure: false, ..pages(page, 0, None) };
        for (theme, t) in both() {
            let at_ = format!("{theme:?}");
            let (shapes, _) = drawn(theme, |ui| {
                off("Page 1").show(ui);
            });
            assert_eq!(word(&shapes, "STRUCTURE", &at_).1, t.text_faint, "{at_}: STRUCTURE off");
            assert_eq!(word(&shapes, "DATA", &at_).1, t.on_ink, "{at_}: DATA chosen");
        }
        let mut h = crate::components::harness_in(vec2(900.0, 200.0), Theme::Light, Vec::new(), move |ui, seen: &mut Vec<PagingAsked>| {
            ui.allocate_ui(vec2(840.0, 36.0), |ui| seen.extend(off("Page 1").show(ui).asked));
        });
        h.run();
        use egui_kittest::kittest::Queryable;
        for label in ["Structure", "Data"] {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(*h.state(), [], "neither STRUCTURE off nor DATA chosen asks anything");
    }

    /// A cut page's note, as the screen writes it.
    const NOTE: &str = "Only the first 13 rows of this page are shown: its values passed the size limit.";

    #[test]
    fn the_paging_bars_facts_are_grey() {
        // Grey caps are facts, dark caps are buttons: the page's words and the count of changes in
        // `text_muted`; the staged mark `staged_strong`; APPLY on `ink`.
        for (theme, t) in both() {
            let at_ = format!("{theme:?}");
            let (shapes, at) = drawn(theme, |ui| {
                pages("Page 1 of 250", 1, None).show(ui);
            });
            assert_eq!(word(&shapes, "PAGE 1 OF 250", &at_).1, t.text_muted, "{at_}: the page's words grey");
            assert_eq!(word(&shapes, "1 CHANGE", &at_).1, t.text_muted, "{at_}: the count of changes grey");
            let fills = fills_in(&shapes, at);
            assert!(fills.iter().any(|(r, c)| *c == t.staged_strong && close(r.width(), 8.0) && close(r.height(), 8.0)), "{at_}: the staged mark");
            let (apply, c) = word(&shapes, "APPLY", &at_);
            assert_eq!(c, t.on_ink, "{at_}: APPLY's word");
            assert!(fills.iter().any(|(r, c)| *c == t.ink && r.contains_rect(apply)), "{at_}: APPLY on ink");
        }
    }

    /// The bottom bar `width` wide with `changes` staged and a cut page's note: where each part
    /// was drawn (the part a click can reach, `None` when not drawn or wholly cut), in the bar's
    /// order, and the bar.
    fn paging_parts(width: f32, changes: usize) -> (Vec<(&'static str, Option<Rect>)>, Rect) {
        type Out = Option<(Vec<(&'static str, Option<Rect>)>, Rect)>;
        let mut h = crate::components::harness_in(vec2(width + 20.0, 100.0), Theme::Light, None, move |ui, out: &mut Out| {
            ui.allocate_ui(vec2(width, 36.0), |ui| {
                let bar = ui.max_rect();
                let s = PagingBar { back: true, forward: false, ..pages("Page 10,000 of 10,000", changes, Some(NOTE)) }.show(ui);
                let seen = |r: &Response| Some(r.interact_rect).filter(|r| r.width() > 0.0 && r.height() > 0.0);
                let opt = |r: &Option<Response>| r.as_ref().and_then(seen);
                let page = |f: fn(&PagesShown) -> &Response| s.pages.as_ref().and_then(|p| seen(f(p)));
                let parts = vec![
                    ("data", s.view.first().and_then(seen)),
                    ("structure", s.view.get(1).and_then(seen)),
                    ("first", page(|p| &p.first)),
                    ("previous", page(|p| &p.previous)),
                    ("page", page(|p| &p.page)),
                    ("next", page(|p| &p.next)),
                    ("last", page(|p| &p.last)),
                    ("note", opt(&s.note)),
                    ("changes", opt(&s.changes)),
                    ("reset", opt(&s.reset)),
                    ("apply", opt(&s.apply)),
                ];
                *out = Some((parts, Rect::from_min_size(bar.min, vec2(width, 36.0))));
            });
        });
        h.run();
        h.state().clone().unwrap_or_else(|| panic!("{width}: drawn"))
    }

    #[test]
    fn the_bottom_bar_gives_way_in_its_order() {
        // From wide to narrow, with changes staged: everything; no note; no count of changes
        // either; no paging; no RESET. The switch and APPLY are whole at every width that holds
        // the two; the paging is centred on the bar while it has the room, then moves off centre,
        // 12 clear of the switch and of the changes. Never two parts over each other, nothing
        // past the bar.
        let mut seen: Vec<u8> = Vec::new();
        let mut centred = (0, 0);
        for width in (250..=1000).rev().step_by(10) {
            let (parts, bar) = paging_parts(width as f32, 3);
            let at = format!("{width} wide");
            let shown: Vec<(&str, Rect)> = parts.iter().filter_map(|(id, r)| r.map(|r| (*id, r))).collect();
            for (i, (a, ra)) in shown.iter().enumerate() {
                assert!(ra.left() >= bar.left() - 0.5 && ra.right() <= bar.right() + 0.5, "{at}: `{a}` {ra:?} leaves the bar {bar:?}");
                for (b, rb) in shown.iter().skip(i + 1) {
                    let o = ra.intersect(*rb);
                    assert!(o.width() <= 0.5 || o.height() <= 0.5, "{at}: `{a}` {ra:?} overlaps `{b}` {rb:?}");
                }
            }
            let get = |id: &str| parts.iter().find(|(i, _)| *i == id).and_then(|(_, r)| *r);
            // The switch whole, 12 in; APPLY whole (60 wide: 12, APPLY, 12), 12 from the end.
            let (data, structure, apply) = (get("data"), get("structure"), get("apply"));
            let switch = data.zip(structure).map(|(d, s)| d.union(s)).unwrap_or(Rect::NOTHING);
            assert!(close(switch.left(), bar.left() + 13.0) && (switch.width() - 137.0).abs() <= 2.0, "{at}: the switch whole, 12 in: {switch:?}");
            assert!(
                apply.is_some_and(|r| r.width() >= 59.5 && close(r.right(), bar.right() - 12.0)),
                "{at}: APPLY whole, 12 from the end: {apply:?}"
            );
            let (note, changes, paging, reset) = (get("note").is_some(), get("changes").is_some(), get("first").is_some(), get("reset").is_some());
            for id in ["previous", "page", "next", "last"] {
                assert_eq!(get(id).is_some(), paging, "{at}: the paging shows whole or not at all: `{id}`");
            }
            let steps = [note, changes, paging, reset];
            let form = steps.iter().take_while(|s| !**s).count();
            assert!(steps[form..].iter().all(|s| *s), "{at}: a later step gave way before an earlier one: {steps:?}");
            assert!(seen.last().is_none_or(|&last| form as u8 >= last), "{at}: room taken back while narrowing: {seen:?} then {form}");
            seen.push(form as u8);
            if let (Some(first), Some(last)) = (get("first"), get("last")) {
                let group = first.union(last);
                let right = get("changes").or(get("reset")).map_or(bar.right(), |r| r.left());
                assert!(group.left() >= switch.right() + 1.0 + 12.0 - 0.5, "{at}: the paging 12 clear of the switch: {group:?}");
                assert!(group.right() <= right - 12.0 + 0.5, "{at}: and of the changes: {group:?}, {right}");
                if close(group.center().x, bar.center().x) {
                    centred.0 += 1;
                } else {
                    centred.1 += 1;
                }
            }
        }
        assert_eq!(seen.first(), Some(&0), "1000 wide, everything shows");
        for form in 0..=4 {
            assert!(seen.contains(&form), "the sweep passes through form {form}: {seen:?}");
        }
        assert!(centred.0 > 0 && centred.1 > 0, "the paging centred while it can be, then off centre: {centred:?}");
        // Without changes the paging stays centred down to the switch, then moves off centre, then goes.
        let mut forms: Vec<u8> = Vec::new();
        for width in (150..=1000).rev().step_by(10) {
            let (parts, bar) = paging_parts(width as f32, 0);
            let get = |id: &str| parts.iter().find(|(i, _)| *i == id).and_then(|(_, r)| *r);
            let group = get("first").zip(get("last")).map(|(f, l)| f.union(l));
            let form = match group {
                Some(g) if close(g.center().x, bar.center().x) => 0,
                Some(g) => {
                    assert!(g.right() <= bar.right() - 12.0 + 0.5, "{width}: the paging inside the bar: {g:?}");
                    1
                }
                None => 2,
            };
            assert!(forms.last().is_none_or(|&last| form >= last), "{width}: room taken back: {forms:?} then {form}");
            forms.push(form);
            assert!(get("data").is_some_and(|d| close(d.left(), bar.left() + 13.0)), "{width}: the switch 12 in");
        }
        for form in 0..=2 {
            assert!(forms.contains(&form), "without changes the sweep passes through form {form}: {forms:?}");
        }
    }

    #[test]
    fn the_cut_note_is_cut_with_an_ellipsis_and_follows_the_page() {
        for (theme, t) in both() {
            let at_ = format!("{theme:?}");
            let (shapes, _) = drawn(theme, |ui| {
                pages("Page 1", 2, Some(NOTE)).show(ui);
            });
            let all = texts(&shapes);
            let (note, c) = all
                .iter()
                .find(|(w, _, _)| w.starts_with("Only the first 13 rows"))
                .map(|(w, r, c)| ((w.clone(), *r), *c))
                .unwrap_or_else(|| panic!("{at_}: the note drawn: {all:?}"));
            assert_eq!(c, t.text_muted, "{at_}: the note grey");
            assert_eq!(font_of(&shapes, &note.0), Some(Text::Small.font()), "{at_}: the note in `small`");
            assert!(note.0 == NOTE || note.0.ends_with('…'), "{at_}: whole or cut with `…`: {}", note.0);
            let (page, _) = word(&shapes, "PAGE 1", &at_);
            let (changes, _) = word(&shapes, "2 CHANGES", &at_);
            assert!(
                note.1.left() > page.right() + 28.0 * 2.0 && note.1.right() < changes.left(),
                "{at_}: after the arrows, before the changes: {note:?}"
            );
        }
    }

    #[test]
    fn reset_and_apply_keep_their_ids_when_the_count_of_changes_goes() {
        let ids = |width: f32| {
            let mut h = crate::components::harness_in(
                vec2(width + 20.0, 100.0),
                Theme::Light,
                None,
                move |ui, out: &mut Option<(bool, Option<egui::Id>, egui::Id)>| {
                    ui.allocate_ui(vec2(width, 36.0), |ui| {
                        let s = PagingBar { back: true, ..pages("Page 2", 2, None) }.show(ui);
                        if let Some(apply) = s.apply {
                            *out = Some((s.changes.is_some(), s.reset.map(|r| r.id), apply.id));
                        }
                    });
                },
            );
            h.run();
            (*h.state()).unwrap_or_else(|| panic!("{width}: Apply drawn"))
        };
        let wide = ids(840.0);
        assert!(wide.0 && wide.1.is_some(), "840 wide: the count and RESET show");
        let all: Vec<_> = (250..840).rev().step_by(5).map(|w| ids(w as f32)).collect();
        let bare = all.iter().find(|(changes, reset, _)| !changes && reset.is_some()).unwrap_or_else(|| panic!("a width without the count"));
        assert_eq!((bare.1, bare.2), (wide.1, wide.2), "Reset and Apply keep their ids");
        let alone = all.iter().find(|(_, reset, _)| reset.is_none()).unwrap_or_else(|| panic!("a width without RESET"));
        assert_eq!(alone.2, wide.2, "Apply keeps its id without RESET");
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

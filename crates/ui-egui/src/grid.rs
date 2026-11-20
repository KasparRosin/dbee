//! The data grid ([`Grid`]): rows of cells under a header that stays in place, shared by the
//! table tab and query results. It reads its rows through [`GridSource`], so it knows nothing of
//! where they come from, and it changes nothing itself: it returns what the user asked for
//! ([`GridAct`]) and the caller runs the commands.
//!
//! **Drawing.** An `egui_extras` table in a horizontal scroll area: only the rows in view are
//! laid out, so a page of 100 rows or a result of 10,000 costs the same per frame. The header,
//! the cells and the row numbers are the components' ([`crate::components::grid_cell`]): a cell
//! shows `display::grid_text` of its value (one line, cut at 200 characters), so a 20 MB value
//! lays out as 200 characters; NULL is the word in `text_muted`, an empty string an empty cell.
//! Numbers sit right. The row under the pointer, wherever it is on the row, lights up, but for its
//! selected and marked cells.
//!
//! **Edge to edge.** The grid fills its area from the left edge, row numbers first. Past the last
//! column a filler takes the rest of the width ([`grid_cell::Filler`], [`grid_cell::header_filler`];
//! none once the columns are wider than the area), so the header's `ink` line, each row's line and
//! its hover or selection ground reach the area's right edge. The table is as tall as its rows:
//! the column lines stop at the last row, and under it the area shows the tab's own ground.
//!
//! **Column widths** start from a bounded guess per column (its header, and the cut text of the
//! first rows, clamped to [`MIN_WIDTH`]..=[`MAX_WIDTH`]) worked out once; after that egui keeps
//! them, resized ones included, under the grid's id: one per tab, for the session.
//!
//! **Selection** is the engine's (`selection.set`, `selection.rows`, `selection.all`): a click
//! selects a cell, shift-click extends from the anchor, a drag selects from where it started to
//! the cell under the pointer. A click on a row's number selects the whole row (issue #17), and
//! shift-click on one the rows from the anchor's row to it. One selected cell lights its whole row
//! in `selection` (its number and the filler too) and takes the `ink` outline; a selection over
//! every column lights each of its rows the same way, with no outline, and whole rows also draw
//! their numbers in `text` ([`RowNumber::whole`]); any other rectangle lights its own cells only.
//! A marked cell keeps its mark, and takes its outline where it is in the selection (the
//! component's rule). A right-click selects the cell unless it is already selected, and opens the
//! menu: Set as NULL, Copy, Copy with header, Copy as CSV / JSON / Markdown, Delete row(s). A
//! right-click on a row's number selects the row unless the selection already takes in all of it,
//! and opens the same menu without Set as NULL. Set as NULL and Delete row are left out where rows
//! are never edited (a query's results, [`RowEdits::None`]), and listed disabled, with the reason
//! on hover, where they cannot be now ([`RowEdits::Blocked`]).
//!
//! **The keyboard.** A click, a right-click or a drag on a cell, and a click or a right-click on a
//! row's number, take the keyboard from any text field (the query editor, the filter bar's
//! inputs), so Ctrl+A, Ctrl+C and Ctrl+Shift+C act on the grid from then on
//! ([`crate::shortcuts`]: a focused text field keeps them).
//!
//! **Editing.** A double-click (or a triple one) asks to edit the cell ([`GridAct::Edit`]); the
//! caller decides whether it can. While a cell is being edited ([`Grid::editing`]), the grid
//! keeps where that cell was drawn and which part of the rows was in view
//! ([`GridState::editing_rect`]), for the editor to sit over it ([`crate::editors`]). Every
//! column of a row in view is laid out, so the cell can be drawn scrolled away to the side.
//!
//! **Marks.** A staged cell, a new row, a row to delete and the row whose change the last Apply
//! failed on each have their own ground, kept when selected ([`Mark`]); a row to delete is also
//! struck through, failed or not. A cell the last Apply wrote shows the saved mark at full for a
//! second from the frame that first shows it, then fades out over half a second ([`Grid::saved`]),
//! timed by the window's clock once per page, so once per Apply ([`GridState::saved_strength`]);
//! after that it is drawn and named as if unmarked. Only while it shows does it ask for frames.
//!
//! **Loading.** While a page loads over the rows shown ([`Grid::dimmed`], once the load has run
//! 300 ms), the rows (cells, numbers, their grounds and marks) are drawn at [`crate::theme::DIM`]
//! and the header at full: the dim is a look, the rows take input as ever. Before a first page,
//! [`skeleton`] draws the header from the catalog over rows of bars
//! ([`crate::components::skeleton`]); it keeps nothing, so the widths the rows bring win.
//!
//! **Foreign keys.** With [`Grid::links`], each non-NULL foreign key cell of the selected row
//! (the focus cell's) shows a link that follows it.
//!
//! Automation ids: `grid-header-<col>` (value `{name, type, key, sort}`), `grid-cell-<row>-<col>`
//! (value `{text, null, state?, deleted?}`: the text as shown, `NULL` for NULL, `state` one of
//! `staged`, `new`, `deleted`, `failed`, `saved` (while its mark shows), and `deleted: true` on a
//! row to delete, also when it is the failed one), `grid-row-<row>` (a row's number, value
//! `{number, selected}`: the number shown, and whether the row is selected whole),
//! `fk-link-<row>-<col>`, `grid-dimmed` (value `true`, the rows' area, while the rows are dimmed),
//! `grid-skeleton` (value `{rows, columns}`, the bars' rows under the header), and while the menu is open `grid-menu-null`, `grid-menu-copy`,
//! `grid-menu-copy-header`, `grid-menu-copy-csv`, `grid-menu-copy-json`,
//! `grid-menu-copy-markdown`, `grid-menu-delete` (value `{label, shortcut, why?}`, `why` saying
//! why a disabled entry cannot run now). Cells of rows out of view are listed with
//! `visible: false` ([`CellBlock`]); a click on one scrolls its row into view first. Row numbers
//! are tagged only while in view.

use std::sync::{Arc, Weak};
use std::time::Duration;

use dbee_core::display::grid_text;
use dbee_core::{ResultSet, TypeClass};
use dbee_engine::Selection;
use dbee_sql::filter::SortDir;
use egui::{Align, Layout, Rangef, Rect, Response, ScrollArea, Sense, Stroke, UiBuilder, pos2};
use egui_extras::{Column, TableBuilder};
use serde_json::{Map, json};

use crate::automation::{CellBlock, Tags};
use crate::components::grid_cell::{self, Cell, CellText, DATA_CHAR_W, Filler, Header, RowNumber};
pub use crate::components::grid_cell::{Key, Mark};
use crate::components::icon::Icon;
use crate::components::menu::{self, Item};
use crate::components::skeleton::GridRows;
use crate::components::tooltip::Hint;
use crate::theme::{self, size};

/// A row's height.
pub const ROW_H: f32 = size::ROW_H;
/// The header's height.
const HEADER_H: f32 = size::GRID_HEADER_H;
/// The narrowest and the widest a column starts.
pub const MIN_WIDTH: f32 = 56.0;
pub const MAX_WIDTH: f32 = 320.0;
/// Rows looked at when guessing widths.
const SAMPLE_ROWS: usize = 100;
/// The id prefixes of a cell, a row's number and a foreign key link.
pub const CELL: &str = "grid-cell-";
pub const ROW: &str = "grid-row-";
pub const LINK: &str = "fk-link-";

/// The rows a grid shows. Read every frame for the rows in view only, so each call must be
/// cheap; out-of-range rows and columns read as NULL and empty names.
pub trait GridSource {
    fn columns(&self) -> usize;
    fn column_name(&self, col: usize) -> &str;
    /// The type the header shows (`bigint`, `character varying(200)`).
    fn column_type(&self, col: usize) -> &str;
    fn rows(&self) -> usize;
    /// The text of cell `(row, col)`; `None` is NULL.
    fn cell(&self, row: usize, col: usize) -> Option<&str>;
    fn mark(&self, _row: usize, _col: usize) -> Option<Mark> {
        None
    }
    /// Row `row` is to be deleted: its text is struck through, whatever its mark.
    fn deleted(&self, _row: usize) -> bool {
        false
    }
    /// The column holds numbers, which sit right.
    fn right_aligned(&self, _col: usize) -> bool {
        false
    }
}

/// A result as it came: no marks.
impl GridSource for ResultSet {
    fn columns(&self) -> usize {
        self.columns.len()
    }
    fn column_name(&self, col: usize) -> &str {
        self.columns.get(col).map_or("", |c| c.name.as_str())
    }
    fn column_type(&self, col: usize) -> &str {
        self.columns.get(col).map_or("", |c| c.type_label.as_str())
    }
    fn rows(&self) -> usize {
        self.rows.len()
    }
    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row)?.get(col)?.as_deref()
    }
    fn right_aligned(&self, col: usize) -> bool {
        self.columns.get(col).is_some_and(|c| c.class == TypeClass::Number)
    }
}

/// What the menu offers for editing rows: Set as NULL and Delete row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowEdits<'a> {
    /// Not listed: these rows are never edited (a query's results).
    None,
    /// Offered.
    Allowed,
    /// Listed, disabled, with why on hover (a read-only table, a lost connection).
    Blocked(&'a str),
}

/// What the user asked of the grid in a frame; the caller runs the commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridAct {
    /// `selection.set`.
    Select { anchor: (usize, usize), focus: (usize, usize) },
    /// `selection.rows`: whole rows, `first` to `last`.
    SelectRows { first: usize, last: usize },
    /// A header was clicked: the column's sort moves on (asc, desc, none).
    Sort(usize),
    /// `table.followForeignKey` from this cell.
    Follow { row: usize, col: usize },
    /// `cell.setNull`.
    SetNull { row: usize, col: usize },
    /// `clipboard.copy` in this format (`tsv`, `tsvHeader`, `csv`, `json`, `markdown`).
    Copy(&'static str),
    /// `row.delete`.
    DeleteRows(Vec<usize>),
    /// The cell was double-clicked: edit it.
    Edit { row: usize, col: usize },
}

/// What the grid keeps between frames (the caller keeps one per grid: per tab).
#[derive(Debug, Default)]
pub struct GridState {
    /// The widths the columns start with, guessed once from the first rows shown.
    widths: Option<Vec<f32>>,
    /// While a drag selects: where it started and the cell it last selected to.
    drag: Option<((usize, usize), (usize, usize))>,
    /// Last frame's columns (left and right, from the left of the scrolled content), how far
    /// the content was scrolled sideways and how wide the view was.
    columns_x: Vec<(f32, f32)>,
    offset_x: f32,
    view_w: f32,
    content_w: f32,
    /// Where the cell being edited was drawn this frame (`None`: out of view), and the rows' part
    /// of the grid in view.
    edit_cell: Option<Rect>,
    rows_view: Option<Rect>,
    /// The page whose saved cells' mark is timed, by identity (a weak reference keeps its address
    /// from being reused by another page), and when, on the window's clock, it first showed.
    saved: Option<(Weak<ResultSet>, f64)>,
}

impl GridState {
    /// Where the cell being edited ([`Grid::editing`]) was drawn by the last [`Grid::show`], and
    /// the window rect of the rows in view; `None` when that cell was out of view.
    pub fn editing_rect(&self) -> Option<(Rect, Rect)> {
        self.edit_cell.zip(self.rows_view)
    }

    /// How strongly the saved mark of the cells of `page` shows in this frame, for
    /// [`Grid::saved`] ([`grid_cell::saved_strength`]). Its clock starts at the first frame that
    /// asks for `page`, and every Apply loads a new page. While the mark shows, asks for the
    /// frame that changes it.
    pub fn saved_strength(&mut self, ctx: &egui::Context, page: &Arc<ResultSet>) -> f32 {
        let now = ctx.input(|i| i.time);
        let since = match &self.saved {
            Some((of, since)) if std::ptr::eq(of.as_ptr(), Arc::as_ptr(page)) => *since,
            _ => {
                self.saved = Some((Arc::downgrade(page), now));
                now
            }
        };
        let elapsed = now - since;
        let strength = grid_cell::saved_strength(elapsed);
        if strength >= 1.0 {
            ctx.request_repaint_after(Duration::try_from_secs_f64(grid_cell::SAVED_HOLD - elapsed).unwrap_or(Duration::ZERO));
        } else if strength > 0.0 {
            ctx.request_repaint();
        }
        strength
    }
}

/// The cell `id` names when it is a cell (`grid-cell-<row>-<col>`) or a foreign key link
/// (`fk-link-<row>-<col>`) of a grid of `rows` × `cols`.
fn cell_named(id: &str, rows: usize, cols: usize) -> Option<(usize, usize)> {
    let rest = id.strip_prefix(CELL).or_else(|| id.strip_prefix(LINK))?;
    let (row, col) = rest.split_once('-')?;
    let (row, col) = (row.parse::<usize>().ok()?, col.parse::<usize>().ok()?);
    (row < rows && col < cols).then_some((row, col))
}

/// One grid to draw: what it shows and what it offers.
pub struct Grid<'a> {
    /// The id egui keeps the grid's state under (column widths, scroll): one per tab.
    pub id: egui::Id,
    pub source: &'a dyn GridSource,
    /// Each column's key (missing ones: none).
    pub keys: &'a [Option<Key>],
    pub selection: Option<Selection>,
    /// The sorted column and its direction.
    pub sort: Option<(usize, SortDir)>,
    /// A header click asks for the next sort ([`GridAct::Sort`]).
    pub sortable: bool,
    /// Why sorting cannot run now (staged changes, a lost connection), shown on a header's hover;
    /// a click still asks, and the command says why.
    pub blocked: Option<&'a str>,
    /// Whether the menu offers Set as NULL and Delete row.
    pub edits: RowEdits<'a>,
    /// Foreign key cells of the selected row show the link that follows them.
    pub links: bool,
    /// The number the gutter shows for row 0 (a page's offset plus one).
    pub first_number: u64,
    /// The cell an editor is open on, if any.
    pub editing: Option<(usize, usize)>,
    /// That editor is not over its cell (it is in the value panel): the cell draws the `editing`
    /// look itself, with its text as before the edit.
    pub edited_elsewhere: bool,
    /// How strongly the cells the source marks [`Mark::Saved`] show it, from 1 at full to 0, when
    /// they are drawn and named as if unmarked ([`GridState::saved_strength`]).
    pub saved: f32,
    /// A page loads over these rows: they are drawn at [`theme::DIM`], the header at full.
    pub dimmed: bool,
}

/// The inclusive rows and columns `sel` covers: `(rows, columns)`.
fn bounds(sel: Selection) -> ((usize, usize), (usize, usize)) {
    let (a, f) = (sel.anchor, sel.focus);
    ((a.0.min(f.0), a.0.max(f.0)), (a.1.min(f.1), a.1.max(f.1)))
}

fn covers(sel: Option<Selection>, (row, col): (usize, usize)) -> bool {
    sel.map(bounds).is_some_and(|((r0, r1), (c0, c1))| (r0..=r1).contains(&row) && (c0..=c1).contains(&col))
}

/// The widths the columns of `source` start with: room for the header (name, type, key icon and
/// sort mark) and for the cut text of the first [`SAMPLE_ROWS`] rows, clamped.
pub fn initial_widths(source: &dyn GridSource, keys: &[Option<Key>]) -> Vec<f32> {
    let chars = |s: &str| s.chars().count() as f32;
    let (pad, gap) = (size::GRID_CELL_PAD_X, size::GRID_HEADER_GAP);
    let rows = source.rows().min(SAMPLE_ROWS);
    (0..source.columns())
        .map(|col| {
            let key = if keys.get(col).copied().flatten().is_some() { Icon::Key.size().x + gap } else { 0.0 };
            let sort = gap + 2.0 * Icon::SortAscending.size().x + size::GRID_SORT_HINT_GAP;
            let head =
                2.0 * pad + key + chars(source.column_name(col)) * DATA_CHAR_W + gap + chars(source.column_type(col)) * DATA_CHAR_W * 0.85 + sort;
            let cells = (0..rows)
                .map(|row| match source.cell(row, col) {
                    None => 4.0,
                    Some(v) => grid_text(v).chars().count() as f32,
                })
                .fold(0.0, f32::max);
            head.max(cells * DATA_CHAR_W + 2.0 * pad).clamp(MIN_WIDTH, MAX_WIDTH)
        })
        .collect()
}

/// A grid whose first rows are on their way, into the rest of `ui`: the header of `columns` (with
/// each column's key and the sort) as [`Grid`] draws it, but taking no click and no hover, over
/// rows of bars ([`GridRows`]) in the widths [`initial_widths`] gives the header alone. Nothing is
/// kept: the grid works out its widths from the rows when they come. Tags `grid-skeleton`, its
/// value `{rows, columns}`, its rect the rows' area under the header.
pub fn skeleton(ui: &mut egui::Ui, columns: &ResultSet, keys: &[Option<Key>], sort: Option<(usize, SortDir)>, tags: &mut Tags) -> Response {
    let rect = ui.available_rect_before_wrap();
    let widths = initial_widths(columns, keys);
    let number_w = RowNumber::width(1);
    let right: Vec<bool> = (0..columns.columns()).map(|c| columns.right_aligned(c)).collect();
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    ui.set_clip_rect(ui.clip_rect().intersect(rect));
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    let (head, _) = ui.allocate_exact_size(egui::vec2(rect.width(), HEADER_H), Sense::hover());
    let cell = |left: f32, w: f32| Rect::from_min_max(pos2(left, head.top()), pos2(left + w, head.bottom()));
    grid_cell::corner(&ui, cell(head.left(), number_w));
    let mut x = head.left() + number_w;
    for (col, &w) in widths.iter().enumerate() {
        let align = if columns.right_aligned(col) { grid_cell::Align::Right } else { grid_cell::Align::Left };
        let header = Header {
            name: columns.column_name(col),
            ty: columns.column_type(col),
            key: keys.get(col).copied().flatten(),
            sort: sort.filter(|(c, _)| *c == col).map(|(_, dir)| dir),
            align,
            sortable: false,
            hovered: false,
        };
        header.paint(&ui, cell(x, w));
        x += w;
    }
    if x < head.right() {
        grid_cell::header_filler(&ui, Rect::from_min_max(pos2(x, head.top()), head.max));
    }
    let (rows_rect, rows) = GridRows { widths: &widths, right: &right, number_w }.show(&mut ui);
    let r = ui.interact(rows_rect, ui.id().with("dbee-grid-skeleton"), Sense::hover());
    tags.tag_value("grid-skeleton", &r, json!({ "rows": rows, "columns": widths.len() }));
    r
}

/// What drawing a cell left for the frame to act on.
struct Drawn {
    /// What the cell shows, and whether that is NULL.
    text: String,
    null: bool,
    /// The cell's own response (clicks, drags, the menu). Made while the cell is drawn, so it has
    /// this frame's rect: the response `egui_extras` gives a cell is the last frame's, which a
    /// page drawn for the first time does not have yet.
    resp: Response,
    /// The foreign key link's response, when it shows.
    link: Option<Response>,
}

impl Grid<'_> {
    /// Draws the grid into the rest of `ui` and returns what the user asked for.
    pub fn show(&self, ui: &mut egui::Ui, state: &mut GridState, tags: &mut Tags) -> Vec<GridAct> {
        let src = self.source;
        let (rows, cols) = (src.rows(), src.columns());
        if state.widths.as_ref().is_none_or(|w| w.len() != cols) {
            state.widths = Some(initial_widths(src, self.keys));
        }
        let widths = state.widths.clone().unwrap_or_default();
        let gutter = RowNumber::width(self.first_number.saturating_add(rows as u64));
        // A click waits for a cell out of view: bring its row in, and its column. (egui's own
        // scrolling to a widget stops at the table's inner, vertical scroll area, which takes the
        // sideways part too.)
        let pending = tags.scroll_pending().and_then(|id| cell_named(id, rows, cols));
        let scroll_row = pending.map(|(row, _)| row);
        let scroll_x = pending.and_then(|(_, col)| state.columns_x.get(col)).and_then(|&(left, right)| {
            let (from, to) = (state.offset_x, state.offset_x + state.view_w);
            let inside = left >= from && right <= to;
            // Centred, within what the scroll area allows (more would be undone a frame later).
            let most = (state.content_w - state.view_w).max(0.0);
            (!inside).then(|| ((left + right) / 2.0 - state.view_w / 2.0).clamp(0.0, most))
        });
        let (pointer, primary_down, shift) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.modifiers.shift));
        if !primary_down {
            state.drag = None;
        }
        let mut acts = Vec::new();
        let mut heads = vec![(0.0, 0.0); cols];
        let mut first_drawn: Option<(usize, f32)> = None;
        let mut drag_over: Option<(usize, usize)> = None;
        let mut drag_start: Option<(usize, usize)> = None;
        let mut edit_cell: Option<Rect> = None;
        let rect = ui.available_rect_before_wrap();
        let mut grid_ui = ui.new_child(UiBuilder::new().id(self.id).max_rect(rect).layout(Layout::top_down(Align::Min)));
        let mut sideways = ScrollArea::horizontal().id_salt("dbee-grid-h").auto_shrink([false, false]);
        if let Some(x) = scroll_x {
            sideways = sideways.horizontal_scroll_offset(x);
        }
        let mut content_left = 0.0;
        let mut view_x = Rangef::NOTHING;
        let out = sideways.show(&mut grid_ui, |ui| {
            content_left = ui.max_rect().left();
            view_x = ui.clip_rect().x_range();
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            // The cells draw the column lines; egui_extras' own shows only on a hovered or dragged boundary.
            ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::NONE;
            // As tall as its rows, so a boundary's resize line and handle stop at the last one.
            let mut table = TableBuilder::new(ui)
                .id_salt("dbee-grid")
                .resizable(true)
                .striped(false)
                .auto_shrink([false, true])
                .animate_scrolling(false)
                .min_scrolled_height(0.0)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(gutter).resizable(false));
            for w in &widths {
                table = table.column(Column::initial(*w).at_least(MIN_WIDTH).clip(true).resizable(true));
            }
            table = table.column(Column::remainder().clip(true).resizable(false));
            if let Some(row) = scroll_row {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            let table = table.header(HEADER_H, |mut header| {
                header.col(|ui| grid_cell::corner(ui, ui.max_rect()));
                for (col, head) in heads.iter_mut().enumerate() {
                    let mut resp = None;
                    header.col(|ui| resp = Some(self.draw_head(ui, col)));
                    let Some(r) = resp else { continue };
                    *head = (r.rect.left(), r.rect.right());
                    let r = match (self.sortable, self.blocked) {
                        (true, None) => r.hint("Sort by this column"),
                        (true, Some(why)) => r.hint(format!("Sort by this column: not now, {why}")),
                        (false, _) => r,
                    };
                    tags.tag_value(&format!("grid-header-{col}"), &r, self.head_value(col));
                    if self.sortable && r.clicked() {
                        acts.push(GridAct::Sort(col));
                    }
                }
                header.col(|ui| grid_cell::header_filler(ui, ui.max_rect()));
            });
            table.body(|body| {
                body.rows(ROW_H, rows, |mut row| {
                    let r = row.index();
                    let lit = self.lit(r);
                    let mut hovered = false;
                    let mut number = None;
                    row.col(|ui| {
                        self.dim(ui);
                        // Across the view: the number cell's own clip is the number cell.
                        let at = Rect::from_x_y_ranges(view_x, ui.clip_rect().y_range());
                        hovered = ui.ctx().rect_contains_pointer(ui.layer_id(), at);
                        number = Some(self.draw_gutter(ui, r, hovered));
                    });
                    if let Some(resp) = &number {
                        let value = json!({ "number": self.first_number.saturating_add(r as u64), "selected": self.whole_row(r) });
                        tags.tag_value(&format!("{ROW}{r}"), resp, value);
                        if resp.clicked() {
                            let first = match self.selection {
                                Some(sel) if shift => sel.anchor.0,
                                _ => r,
                            };
                            acts.push(GridAct::SelectRows { first, last: r });
                        } else if resp.secondary_clicked() && !self.spans(r) {
                            acts.push(GridAct::SelectRows { first: r, last: r });
                        }
                        if resp.clicked() || resp.secondary_clicked() {
                            resp.ctx.memory_mut(|m| m.stop_text_input());
                        }
                        egui::Popup::context_menu(resp).style(menu::style(&resp.ctx)).show(|ui| self.menu(ui, (r, None), tags, &mut acts));
                    }
                    for c in 0..cols {
                        let mut drawn = None;
                        row.col(|ui| {
                            self.dim(ui);
                            drawn = Some(self.draw_cell(ui, r, c, lit, hovered));
                        });
                        let Some(drawn) = drawn else { continue };
                        let resp = &drawn.resp;
                        if first_drawn.is_none() {
                            first_drawn = Some((r, resp.rect.top()));
                        }
                        if let Some(link) = &drawn.link {
                            let link = link.clone().hint("Open the row this key points at");
                            tags.tag(&format!("{LINK}{r}-{c}"), &link);
                            if link.clicked() {
                                acts.push(GridAct::Follow { row: r, col: c });
                            }
                        }
                        let mut value = Map::new();
                        value.insert("text".into(), json!(drawn.text));
                        value.insert("null".into(), json!(drawn.null));
                        if let Some(mark) = self.mark(r, c) {
                            value.insert("state".into(), json!(mark.id()));
                        }
                        if src.deleted(r) {
                            value.insert("deleted".into(), json!(true));
                        }
                        tags.tag_value(&format!("{CELL}{r}-{c}"), resp, serde_json::Value::Object(value));
                        let here = (r, c);
                        if self.editing == Some(here) {
                            edit_cell = Some(resp.rect);
                        }
                        // egui counts a click as a third when the click before the last came
                        // soon enough, wherever it was: a double-click on another cell just
                        // after one here can arrive as a triple click.
                        if resp.double_clicked() || resp.triple_clicked() {
                            acts.push(GridAct::Edit { row: r, col: c });
                        }
                        if resp.clicked() {
                            let anchor = match self.selection {
                                Some(sel) if shift => sel.anchor,
                                _ => here,
                            };
                            acts.push(GridAct::Select { anchor, focus: here });
                        } else if resp.secondary_clicked() && !covers(self.selection, here) {
                            acts.push(GridAct::Select { anchor: here, focus: here });
                        }
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            drag_start = Some(here);
                        }
                        // The keyboard comes to the grid: a text field that had it lets it go.
                        // egui does that itself for a click, not for a drag.
                        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
                            resp.ctx.memory_mut(|m| m.stop_text_input());
                        }
                        if primary_down && pointer.is_some_and(|p| resp.rect.contains(p)) {
                            drag_over = Some(here);
                        }
                        egui::Popup::context_menu(resp).style(menu::style(&resp.ctx)).show(|ui| self.menu(ui, (r, Some(c)), tags, &mut acts));
                    }
                    row.col(|ui| {
                        self.dim(ui);
                        Filler { selected: lit, hovered }.paint(ui, ui.max_rect());
                    });
                });
            });
        });
        // A drag selects from where it started to the cell under the pointer.
        if let Some(start) = drag_start {
            let anchor = match self.selection {
                Some(sel) if shift => sel.anchor,
                _ => start,
            };
            state.drag = Some((anchor, start));
            acts.push(GridAct::Select { anchor, focus: start });
        } else if let (Some((anchor, last)), Some(over)) = (state.drag, drag_over)
            && over != last
        {
            state.drag = Some((anchor, over));
            acts.push(GridAct::Select { anchor, focus: over });
        }
        state.columns_x = heads.iter().map(|&(l, r)| (l - content_left, r - content_left)).collect();
        state.offset_x = out.state.offset.x;
        state.view_w = out.inner_rect.width();
        state.content_w = out.content_size.x;
        state.edit_cell = edit_cell;
        let view = out.inner_rect;
        let rows_view = Rect::from_min_max(pos2(view.left(), (view.top() + HEADER_H).min(view.bottom())), view.max);
        state.rows_view = Some(rows_view);
        if self.dimmed {
            let r = ui.interact(rows_view, self.id.with("dbee-grid-dimmed"), Sense::hover());
            tags.tag_value("grid-dimmed", &r, true);
        }
        if let Some((row, top)) = first_drawn {
            let top = top - ROW_H * row as f32;
            tags.tag_cells_out_of_view(CellBlock { prefix: CELL.into(), rows, top, row_height: ROW_H, columns: heads });
        }
        acts
    }

    /// A row's part drawn into `ui` takes the dim while a page loads over the rows.
    fn dim(&self, ui: &mut egui::Ui) {
        if self.dimmed {
            ui.multiply_opacity(theme::DIM);
        }
    }

    /// Row `row` lights as selected across the grid: it holds the one selected cell, or the
    /// selection takes in every column of it.
    fn lit(&self, row: usize) -> bool {
        self.spans(row) || self.selection.is_some_and(|sel| sel.anchor == sel.focus && sel.anchor.0 == row)
    }

    /// The selection takes in every column of row `row`: whole rows, or a rectangle as wide as
    /// the grid.
    fn spans(&self, row: usize) -> bool {
        let cols = self.source.columns();
        self.selection.is_some_and(|sel| {
            let ((r0, r1), (c0, c1)) = bounds(sel);
            (r0..=r1).contains(&row) && (sel.whole || (c0 == 0 && c1.saturating_add(1) >= cols))
        })
    }

    /// Row `row` is selected whole, by its number (`selection.rows`).
    fn whole_row(&self, row: usize) -> bool {
        self.selection.filter(|sel| sel.whole).map(bounds).is_some_and(|((r0, r1), _)| (r0..=r1).contains(&row))
    }

    /// Cell `(row, col)`'s mark as drawn: a saved one only while it shows.
    fn mark(&self, row: usize, col: usize) -> Option<Mark> {
        self.source.mark(row, col).filter(|&m| m != Mark::Saved || self.saved > 0.0)
    }

    fn key(&self, col: usize) -> Option<Key> {
        self.keys.get(col).copied().flatten()
    }

    fn sort_of(&self, col: usize) -> Option<SortDir> {
        self.sort.filter(|(c, _)| *c == col).map(|(_, dir)| dir)
    }

    fn align(&self, col: usize) -> grid_cell::Align {
        if self.source.right_aligned(col) { grid_cell::Align::Right } else { grid_cell::Align::Left }
    }

    fn head_value(&self, col: usize) -> serde_json::Value {
        let sort = self.sort_of(col).map(|d| match d {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        });
        json!({ "name": self.source.column_name(col), "type": self.source.column_type(col), "key": self.key(col).map(Key::id), "sort": sort })
    }

    /// Draws column `col`'s header and returns its response.
    fn draw_head(&self, ui: &mut egui::Ui, col: usize) -> Response {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("dbee-head"), Sense::click());
        let head = Header {
            name: self.source.column_name(col),
            ty: self.source.column_type(col),
            key: self.key(col),
            sort: self.sort_of(col),
            align: self.align(col),
            sortable: self.sortable,
            hovered: resp.hovered(),
        };
        head.paint(ui, rect);
        resp
    }

    /// Draws row `row`'s number, `hovered` when the row is under the pointer, and returns its
    /// response: a click selects the row, a right-click opens the row's menu.
    fn draw_gutter(&self, ui: &mut egui::Ui, row: usize, hovered: bool) -> Response {
        let src = self.source;
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("dbee-row"), Sense::click());
        let number = RowNumber {
            n: self.first_number.saturating_add(row as u64),
            new: src.columns() > 0 && src.mark(row, 0) == Some(Mark::New),
            deleted: src.deleted(row),
            selected: self.selection.map(bounds).is_some_and(|((r0, r1), _)| (r0..=r1).contains(&row)),
            whole: self.whole_row(row),
            hovered,
        };
        number.paint(ui, rect);
        resp
    }

    /// Draws cell `(row, col)` into its `ui`; `lit` when its row lights as selected
    /// ([`Grid::lit`]), `hovered` when it is under the pointer.
    fn draw_cell(&self, ui: &mut egui::Ui, row: usize, col: usize, lit: bool, hovered: bool) -> Drawn {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("dbee-cell"), Sense::click_and_drag());
        let (text, null) = match self.source.cell(row, col) {
            None => ("NULL".to_string(), true),
            Some(v) => (grid_text(v).into_owned(), false),
        };
        let mark = self.mark(row, col);
        let focus = self.selection.map(|s| s.focus);
        let cell = Cell {
            text: if null { CellText::Null } else { CellText::Text(&text) },
            align: self.align(col),
            mark,
            // A lit row's unmarked cells take its ground; a marked one keeps its mark, and its
            // outline only where it is in the selection.
            selected: covers(self.selection, (row, col)) || (lit && mark.is_none()),
            focused: self.selection.is_some_and(|s| s.anchor == s.focus) && focus == Some((row, col)),
            deleted: self.source.deleted(row),
            link: self.links && self.key(col) == Some(Key::Foreign),
            row_selected: focus.is_some_and(|f| f.0 == row),
            hovered,
            strength: if mark == Some(Mark::Saved) { self.saved } else { 1.0 },
            editing: self.edited_elsewhere && self.editing == Some((row, col)),
        };
        let link = cell.show(ui, rect, ui.id().with(("dbee-fk-link", row, col)));
        Drawn { text, null, resp, link }
    }

    /// The right-click menu of cell `(row, col)`, or of row `row`'s number when `col` is `None`:
    /// the same without Set as NULL.
    fn menu(&self, ui: &mut egui::Ui, (row, col): (usize, Option<usize>), tags: &mut Tags, acts: &mut Vec<GridAct>) {
        // `blocked`: why the entry cannot run now, or `Some("")` when it is disabled without one.
        let mut item = |ui: &mut egui::Ui, id: &str, label: &str, keys: Option<&str>, blocked: Option<&str>, act: GridAct| {
            let destructive = matches!(act, GridAct::DeleteRows(_));
            let mut entry = Item::new(label).destructive(destructive);
            if let Some(k) = keys {
                entry = entry.shortcut(k);
            }
            let mut r = ui.add_enabled(blocked.is_none(), entry);
            let mut value = json!({ "label": label, "shortcut": keys });
            if let Some(why) = blocked.filter(|w| !w.is_empty()) {
                r = r.hint_disabled(why);
                value["why"] = json!(why);
            }
            // A menu's first frame reads every widget as disabled: tag what the code knows.
            tags.tag_value_as(id, &r, value, blocked.is_none());
            if r.clicked() {
                acts.push(act);
                ui.close();
            }
        };
        let edit = match self.edits {
            RowEdits::None => None,
            RowEdits::Allowed => Some(None),
            RowEdits::Blocked(why) => Some(Some(why)),
        };
        if let (Some(blocked), Some(col)) = (edit, col) {
            // A row to delete takes no new values.
            let null = blocked.or(self.source.deleted(row).then_some(""));
            item(ui, "grid-menu-null", "Set as NULL", None, null, GridAct::SetNull { row, col });
            menu::separator(ui);
        }
        item(ui, "grid-menu-copy", "Copy", Some("Ctrl+C"), None, GridAct::Copy("tsv"));
        item(ui, "grid-menu-copy-header", "Copy with header", Some("Ctrl+Shift+C"), None, GridAct::Copy("tsvHeader"));
        item(ui, "grid-menu-copy-csv", "Copy as CSV", None, None, GridAct::Copy("csv"));
        item(ui, "grid-menu-copy-json", "Copy as JSON", None, None, GridAct::Copy("json"));
        item(ui, "grid-menu-copy-markdown", "Copy as Markdown", None, None, GridAct::Copy("markdown"));
        if let Some(blocked) = edit {
            menu::separator(ui);
            // The selected rows, when the cell, or the row, is in the selection.
            let selected = match col {
                Some(col) => self.selection.filter(|_| covers(self.selection, (row, col))),
                None => self.selection.filter(|&sel| {
                    let ((r0, r1), _) = bounds(sel);
                    (r0..=r1).contains(&row)
                }),
            };
            let rows: Vec<usize> = match selected.map(bounds) {
                Some(((r0, r1), _)) => (r0..=r1).collect(),
                None => vec![row],
            };
            let label = if rows.len() == 1 { "Delete row".to_string() } else { format!("Delete {} rows", rows.len()) };
            item(ui, "grid-menu-delete", &label, None, blocked, GridAct::DeleteRows(rows));
        }
    }
}

#[cfg(test)]
mod tests {
    use dbee_core::Column as Col;

    use super::*;

    fn result(rows: Vec<Vec<Option<&str>>>) -> ResultSet {
        ResultSet {
            columns: vec![
                Col { name: "id".into(), base_type: "int4".into(), type_label: "integer".into(), class: TypeClass::Number },
                Col { name: "note".into(), base_type: "text".into(), type_label: "text".into(), class: TypeClass::LongText },
            ],
            rows: rows.into_iter().map(|r| r.into_iter().map(|c| c.map(str::to_owned)).collect()).collect(),
            rows_seen: 0,
            complete: true,
            rows_affected: None,
            database_after: Default::default(),
        }
    }

    #[test]
    fn widths_are_guessed_within_bounds() {
        let huge = "x".repeat(20_000_000);
        let rs = result(vec![vec![Some("1"), Some(&huge)], vec![None, Some("short")]]);
        let w = initial_widths(&rs, &[Some(Key::Primary)]);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|w| (MIN_WIDTH..=MAX_WIDTH).contains(w)), "{w:?}");
        assert_eq!(w[1], MAX_WIDTH, "a huge value asks for the most, no more");
        let empty = result(vec![]);
        assert!(initial_widths(&empty, &[]).iter().all(|w| *w >= MIN_WIDTH));
    }

    #[test]
    fn a_result_is_a_source() {
        let rs = result(vec![vec![Some("1"), None]]);
        let src: &dyn GridSource = &rs;
        assert_eq!((src.columns(), src.rows(), src.column_name(1), src.column_type(0)), (2, 1, "note", "integer"));
        assert_eq!((src.cell(0, 0), src.cell(0, 1), src.cell(5, 5)), (Some("1"), None, None));
        assert!(src.right_aligned(0) && !src.right_aligned(1) && !src.right_aligned(9));
        assert_eq!((src.column_name(9), src.mark(0, 0)), ("", None));
    }

    #[test]
    fn the_saved_mark_is_timed_once_per_page_and_asks_for_frames_only_while_it_shows() {
        let ctx = egui::Context::default();
        let mut state = GridState::default();
        // A frame at `time` (seconds on the window's clock) asking how strongly `page`'s mark
        // shows: that, and how soon the frame asked for the next one.
        let frame = |state: &mut GridState, page: &Arc<ResultSet>, time: f64| {
            let mut strength = f32::NAN;
            let out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| strength = state.saved_strength(ui.ctx(), page));
            let next = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
            out.drop_without_applying_deltas();
            (strength, next)
        };
        // egui takes a frame's time off a delay asked for, to have that frame ready by then.
        let near = |d: Duration, secs: f64| (0.0..=0.05).contains(&(secs - d.as_secs_f64()));
        // egui asks for frames of its own at the start: from the first one that asks for none, a
        // frame is only asked for by the mark.
        let idle = |time: f64| {
            let out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |_| {});
            let next = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
            out.drop_without_applying_deltas();
            next
        };
        assert!((1..=5).any(|n| idle(f64::from(n)) == Duration::MAX), "egui settles");
        let page = Arc::new(result(vec![]));
        // Its clock starts at the first frame that asks, here 10 s into the window's life.
        let (s, next) = frame(&mut state, &page, 10.0);
        assert!(s == 1.0 && near(next, 1.0), "at full, the next frame when it starts to fade: {s} {next:?}");
        let (s, next) = frame(&mut state, &page, 10.6);
        assert!(s == 1.0 && near(next, 0.4), "{s} {next:?}");
        let (s, next) = frame(&mut state, &page, 11.25);
        assert!(s > 0.0 && s < 1.0 && next == Duration::ZERO, "fading, frame after frame: {s} {next:?}");
        // egui gives an immediate repaint one more frame of its own.
        assert_eq!(frame(&mut state, &page, 11.5).0, 0.0, "gone");
        assert_eq!(frame(&mut state, &page, 11.6), (0.0, Duration::MAX), "gone: no frame asked for");
        assert_eq!(frame(&mut state, &page, 30.0), (0.0, Duration::MAX), "and it does not come back");
        // The next Apply's page has a clock of its own.
        let after = Arc::new(result(vec![]));
        assert_eq!(frame(&mut state, &after, 40.0).0, 1.0);
        assert_eq!(frame(&mut state, &after, 41.5).0, 0.0);
    }

    #[test]
    fn selections_cover_their_rectangle_either_way_round() {
        let sel = Some(Selection { anchor: (5, 3), focus: (2, 1), whole: false });
        assert!(covers(sel, (2, 1)) && covers(sel, (5, 3)) && covers(sel, (3, 2)));
        assert!(!covers(sel, (6, 2)) && !covers(sel, (3, 0)) && !covers(None, (0, 0)));
    }
}

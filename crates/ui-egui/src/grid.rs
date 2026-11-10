//! The data grid ([`Grid`]): rows of cells under a header that stays in place, shared by the
//! table tab and query results. It reads its rows through [`GridSource`], so it knows nothing of
//! where they come from, and it changes nothing itself: it returns what the user asked for
//! ([`GridAct`]) and the caller runs the commands.
//!
//! **Drawing.** An `egui_extras` table in a horizontal scroll area: only the rows in view are
//! laid out, so a page of 100 rows or a result of 10,000 costs the same per frame. A cell shows
//! `display::grid_text` of its value (one line, cut at 200 characters), so a 20 MB value lays out
//! as 200 characters; NULL is a dim italic `NULL`, an empty string an empty cell. Numbers sit
//! right. Each header shows the column's name, its type in small text and a key icon (primary or
//! foreign); the sorted column shows its direction.
//!
//! **Column widths** start from a bounded guess per column (its header, and the cut text of the
//! first rows, clamped to [`MIN_WIDTH`]..=[`MAX_WIDTH`]) worked out once; after that egui keeps
//! them, resized ones included, under the grid's id: one per tab, for the session.
//!
//! **Selection** is the engine's (`selection.set`, `selection.all`): a click selects a cell,
//! shift-click extends from the anchor, a drag selects from where it started to the cell under
//! the pointer. A right-click selects the cell unless it is already selected, and opens the menu:
//! Set as NULL, Copy, Copy with header, Copy as CSV / JSON / Markdown, Delete row(s). Set as NULL
//! and Delete row are left out where rows are never edited (a query's results, [`RowEdits::None`]),
//! and listed disabled, with the reason on hover, where they cannot be now ([`RowEdits::Blocked`]).
//!
//! **The keyboard.** A click, a right-click or a drag on a cell takes the keyboard from any text
//! field (the query editor, the filter bar's inputs), so Ctrl+A, Ctrl+C and Ctrl+Shift+C act on
//! the grid from then on ([`crate::shortcuts`]: a focused text field keeps them).
//!
//! **Editing.** A double-click (or a triple one) asks to edit the cell ([`GridAct::Edit`]); the
//! caller decides whether it can. While a cell is being edited ([`Grid::editing`]), the grid
//! keeps where that cell was drawn and which part of the rows was in view
//! ([`GridState::editing_rect`]), for the editor to sit over it ([`crate::editors`]). Every
//! column of a row in view is laid out, so the cell can be drawn scrolled away to the side.
//!
//! **Marks.** A staged cell, a new row, a row to delete and the row whose change the last Apply
//! failed on each have their own colour ([`crate::theme::Tokens`]); a row to delete is also
//! struck through, failed or not.
//!
//! **Foreign keys.** With [`Grid::links`], each non-NULL foreign key cell of the selected row
//! (the focus cell's) shows a link that follows it.
//!
//! Automation ids: `grid-header-<col>` (value `{name, type, key, sort}`), `grid-cell-<row>-<col>`
//! (value `{text, null, state?, deleted?}`: the text as shown, `NULL` for NULL, `state` one of
//! `staged`, `new`, `deleted`, `failed`, and `deleted: true` on a row to delete, also when it is
//! the failed one), `fk-link-<row>-<col>`, and while the menu is open `grid-menu-null`,
//! `grid-menu-copy`, `grid-menu-copy-header`, `grid-menu-copy-csv`, `grid-menu-copy-json`,
//! `grid-menu-copy-markdown`, `grid-menu-delete` (value `{label, shortcut, why?}`, `why` saying
//! why a disabled entry cannot run now). Cells of rows out of view are listed with
//! `visible: false` ([`CellBlock`]); a click on one scrolls its row into view first.

use dbee_core::display::grid_text;
use dbee_core::{ResultSet, TypeClass};
use dbee_engine::Selection;
use dbee_sql::filter::SortDir;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Button, Color32, CornerRadius, Layout, Rect, Response, ScrollArea, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use serde_json::{Map, json};

use crate::automation::{CellBlock, Tags};
use crate::components::icon::{self, Icon};
use crate::connections::{line_galley, line_galley_as_written};
use crate::theme::{self, Text};

/// A row's height.
pub const ROW_H: f32 = 24.0;
/// The header's height.
const HEADER_H: f32 = 30.0;
/// The narrowest and the widest a column starts.
pub const MIN_WIDTH: f32 = 56.0;
pub const MAX_WIDTH: f32 = 320.0;
/// What a character of cell text is taken to need when guessing widths.
const CHAR_W: f32 = 7.2;
/// The text's inset from a cell's sides.
const PAD: f32 = 6.0;
/// Rows looked at when guessing widths.
const SAMPLE_ROWS: usize = 100;
/// The room the foreign key link takes at a cell's right.
const LINK_W: f32 = 20.0;
/// The id prefixes of a cell and of a foreign key link.
pub const CELL: &str = "grid-cell-";
pub const LINK: &str = "fk-link-";

/// What a cell holds besides its value: a staged change, a new row, a row to delete, or the row
/// the last Apply failed on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    Plain,
    Staged,
    New,
    Deleted,
    Failed,
}

impl Mark {
    /// The `state` a cell's automation value names, if any.
    pub fn id(self) -> Option<&'static str> {
        match self {
            Mark::Plain => None,
            Mark::Staged => Some("staged"),
            Mark::New => Some("new"),
            Mark::Deleted => Some("deleted"),
            Mark::Failed => Some("failed"),
        }
    }
}

/// A column's key, for its header's icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Key {
    #[default]
    None,
    Primary,
    Foreign,
}

impl Key {
    fn id(self) -> Option<&'static str> {
        match self {
            Key::None => None,
            Key::Primary => Some("primary"),
            Key::Foreign => Some("foreign"),
        }
    }
}

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
    fn mark(&self, _row: usize, _col: usize) -> Mark {
        Mark::Plain
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
}

impl GridState {
    /// Where the cell being edited ([`Grid::editing`]) was drawn by the last [`Grid::show`], and
    /// the window rect of the rows in view; `None` when that cell was out of view.
    pub fn editing_rect(&self) -> Option<(Rect, Rect)> {
        self.edit_cell.zip(self.rows_view)
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
    pub keys: &'a [Key],
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
pub fn initial_widths(source: &dyn GridSource, keys: &[Key]) -> Vec<f32> {
    let chars = |s: &str| s.chars().count() as f32;
    let rows = source.rows().min(SAMPLE_ROWS);
    (0..source.columns())
        .map(|col| {
            let key = if keys.get(col).copied().unwrap_or_default() == Key::None { 0.0 } else { 18.0 };
            let head = 2.0 * PAD + key + chars(source.column_name(col)) * CHAR_W + 6.0 + chars(source.column_type(col)) * CHAR_W * 0.85 + 16.0;
            let cells = (0..rows)
                .map(|row| match source.cell(row, col) {
                    None => 4.0,
                    Some(v) => grid_text(v).chars().count() as f32,
                })
                .fold(0.0, f32::max);
            head.max(cells * CHAR_W + 2.0 * PAD).clamp(MIN_WIDTH, MAX_WIDTH)
        })
        .collect()
}

/// The gutter's width for row numbers up to `last`.
fn gutter_width(last: u64) -> f32 {
    let digits = last.max(1).ilog10() + 1;
    (digits as f32 * 8.0 + 2.0 * PAD).max(36.0)
}

/// The mark `display::grid_text` puts for a line break. egui's built-in fonts have no glyph for
/// it, so the grid draws it ([`icon::line_break`]).
const BREAK: char = '↵';
/// The room a drawn line break takes.
const BREAK_W: f32 = 12.0;

/// Paints `text` on one line from `x`, centred on `y`, no further right than `right`, cut with
/// `…`, with each [`BREAK`] drawn as an icon. Returns how wide it came out.
fn paint_text(ui: &egui::Ui, p: &egui::Painter, text: &str, (x, y, right): (f32, f32, f32), color: Color32, italics: bool) -> f32 {
    let mut at = x;
    for (i, part) in text.split(BREAK).enumerate() {
        if i > 0 {
            if at + BREAK_W > right {
                let g = galley(ui, "…", color, (right - at).max(1.0), false);
                p.galley(pos2(at, y - g.size().y / 2.0), g, color);
                break;
            }
            icon::line_break(p, Rect::from_center_size(pos2(at + BREAK_W / 2.0, y), vec2(10.0, 10.0)), color);
            at += BREAK_W;
        }
        if part.is_empty() {
            continue;
        }
        let g = galley(ui, part, color, (right - at).max(1.0), italics);
        let (w, h, cut) = (g.size().x, g.size().y, g.elided);
        p.galley(pos2(at, y - h / 2.0), g, color);
        at += w;
        if cut {
            break;
        }
    }
    at - x
}

/// A one-line galley of `text` cut with `…` at `width`, italic when `italics`.
fn galley(ui: &egui::Ui, text: &str, color: Color32, width: f32, italics: bool) -> std::sync::Arc<egui::Galley> {
    let format = TextFormat { italics, ..Text::Data.format(color) };
    let mut job = LayoutJob::single_section(text.to_string(), format);
    job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
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
        let gutter = gutter_width(self.first_number.saturating_add(rows as u64));
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
        let focus = self.selection.map(|s| s.focus);
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
        let out = sideways.show(&mut grid_ui, |ui| {
            content_left = ui.max_rect().left();
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            // The resize lines run down the whole table: keep them faint.
            ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::tokens(ui.ctx()).line);
            let mut table = TableBuilder::new(ui)
                .id_salt("dbee-grid")
                .resizable(true)
                .striped(false)
                .auto_shrink([false, false])
                .animate_scrolling(false)
                .min_scrolled_height(0.0)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(gutter).resizable(false));
            for w in &widths {
                table = table.column(Column::initial(*w).at_least(MIN_WIDTH).clip(true).resizable(true));
            }
            if let Some(row) = scroll_row {
                table = table.scroll_to_row(row, Some(Align::Center));
            }
            let table = table.header(HEADER_H, |mut header| {
                header.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().rect_filled(r, CornerRadius::ZERO, theme::tokens(ui.ctx()).card);
                    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, theme::tokens(ui.ctx()).line));
                });
                for (col, head) in heads.iter_mut().enumerate() {
                    let mut resp = None;
                    header.col(|ui| resp = Some(self.draw_head(ui, col)));
                    let Some(r) = resp else { continue };
                    *head = (r.rect.left(), r.rect.right());
                    let r = match (self.sortable, self.blocked) {
                        (true, None) => r.on_hover_text("Sort by this column"),
                        (true, Some(why)) => r.on_hover_text(format!("Sort by this column: not now, {why}")),
                        (false, _) => r,
                    };
                    tags.tag_value(&format!("grid-header-{col}"), &r, self.head_value(col));
                    if self.sortable && r.clicked() {
                        acts.push(GridAct::Sort(col));
                    }
                }
            });
            table.body(|body| {
                body.rows(ROW_H, rows, |mut row| {
                    let r = row.index();
                    row.col(|ui| self.draw_gutter(ui, r));
                    for c in 0..cols {
                        let mut drawn = None;
                        row.col(|ui| drawn = Some(self.draw_cell(ui, r, c, focus)));
                        let Some(drawn) = drawn else { continue };
                        let resp = &drawn.resp;
                        if first_drawn.is_none() {
                            first_drawn = Some((r, resp.rect.top()));
                        }
                        if let Some(link) = &drawn.link {
                            let link = link.clone().on_hover_text("Open the row this key points at");
                            tags.tag(&format!("{LINK}{r}-{c}"), &link);
                            if link.clicked() {
                                acts.push(GridAct::Follow { row: r, col: c });
                            }
                        }
                        let mut value = Map::new();
                        value.insert("text".into(), json!(drawn.text));
                        value.insert("null".into(), json!(drawn.null));
                        if let Some(state) = src.mark(r, c).id() {
                            value.insert("state".into(), json!(state));
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
                        egui::Popup::context_menu(resp).style(theme::popup_style(&resp.ctx)).show(|ui| self.menu(ui, here, tags, &mut acts));
                    }
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
        state.rows_view = Some(Rect::from_min_max(pos2(view.left(), (view.top() + HEADER_H).min(view.bottom())), view.max));
        if let Some((row, top)) = first_drawn {
            let top = top - ROW_H * row as f32;
            tags.tag_cells_out_of_view(CellBlock { prefix: CELL.into(), rows, top, row_height: ROW_H, columns: heads });
        }
        acts
    }

    fn key(&self, col: usize) -> Key {
        self.keys.get(col).copied().unwrap_or_default()
    }

    fn sort_of(&self, col: usize) -> Option<SortDir> {
        self.sort.filter(|(c, _)| *c == col).map(|(_, dir)| dir)
    }

    fn head_value(&self, col: usize) -> serde_json::Value {
        let sort = self.sort_of(col).map(|d| match d {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        });
        json!({ "name": self.source.column_name(col), "type": self.source.column_type(col), "key": self.key(col).id(), "sort": sort })
    }

    /// Draws column `col`'s header and returns its response.
    fn draw_head(&self, ui: &mut egui::Ui, col: usize) -> Response {
        let t = theme::tokens(ui.ctx());
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("dbee-head"), Sense::click());
        let p = ui.painter().clone();
        p.rect_filled(rect, CornerRadius::ZERO, t.card);
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.line));
        p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, t.line));
        let mut x = rect.left() + PAD;
        let slot = pos2(x + 7.0, rect.center().y);
        match self.key(col) {
            Key::Primary => Icon::Key.paint(&p, slot, t.key),
            Key::Foreign => Icon::KeyForeign.paint(&p, slot, t.text_muted),
            Key::None => {}
        }
        if self.key(col) != Key::None {
            x += 18.0;
        }
        let sort_w = if self.sort_of(col).is_some() { 16.0 } else { 0.0 };
        let right = rect.right() - PAD - sort_w;
        let name = line_galley_as_written(ui, self.source.column_name(col), Text::Header, t.text, (right - x).max(1.0));
        let name_w = name.size().x;
        p.galley(pos2(x, rect.center().y - name.size().y / 2.0), name, t.text);
        x += name_w + 6.0;
        if right - x > 12.0 {
            let ty = line_galley_as_written(ui, self.source.column_type(col), Text::Data, t.text_muted, right - x);
            p.galley(pos2(x, rect.center().y - ty.size().y / 2.0 + 1.0), ty, t.text_muted);
        }
        if let Some(dir) = self.sort_of(col) {
            let r = Rect::from_min_size(pos2(rect.right() - PAD - 12.0, rect.center().y - 6.0), vec2(12.0, 12.0));
            let arrow = if dir == SortDir::Asc { Icon::SortAscending } else { Icon::SortDescending };
            arrow.paint(&p, r.center(), t.ink);
        }
        resp
    }

    fn draw_gutter(&self, ui: &mut egui::Ui, row: usize) {
        let t = theme::tokens(ui.ctx());
        let rect = ui.max_rect();
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, t.card);
        p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, t.line));
        let label = if self.source.columns() > 0 && self.source.mark(row, 0) == Mark::New {
            "+".to_string()
        } else {
            crate::tree::thousands(self.first_number.saturating_add(row as u64))
        };
        let g = line_galley(ui, &label, Text::Data, t.text_muted, rect.width());
        p.galley(pos2(rect.right() - PAD - g.size().x, rect.center().y - g.size().y / 2.0), g, t.text_muted);
    }

    /// Draws cell `(row, col)` into its `ui`; `focus` is the selection's focus cell.
    fn draw_cell(&self, ui: &mut egui::Ui, row: usize, col: usize, focus: Option<(usize, usize)>) -> Drawn {
        let t = theme::tokens(ui.ctx());
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("dbee-cell"), Sense::click_and_drag());
        let p = ui.painter().clone();
        let mark = self.source.mark(row, col);
        let selected = covers(self.selection, (row, col));
        let fill = if selected {
            t.selection
        } else {
            match mark {
                Mark::Staged => t.staged,
                Mark::New => t.new_row,
                Mark::Deleted => t.deleted,
                Mark::Failed => t.failed,
                Mark::Plain => t.card,
            }
        };
        p.rect_filled(rect, CornerRadius::ZERO, fill);
        let line = Stroke::new(1.0, t.line);
        p.line_segment([rect.right_top(), rect.right_bottom()], line);
        p.line_segment([rect.left_bottom(), rect.right_bottom()], line);
        if focus == Some((row, col)) {
            p.rect_stroke(rect.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.5, t.ink), StrokeKind::Inside);
        }
        let value = self.source.cell(row, col);
        let link = self.links && value.is_some() && self.key(col) == Key::Foreign && focus.is_some_and(|f| f.0 == row);
        let right = rect.right() - PAD - if link { LINK_W } else { 0.0 };
        let width = (right - rect.left() - PAD).max(1.0);
        let (text, null) = match value {
            None => ("NULL".to_string(), true),
            Some(v) => (grid_text(v).into_owned(), false),
        };
        let deleted = self.source.deleted(row);
        if !text.is_empty() {
            let color = if null || deleted { t.text_muted } else { t.text };
            let y = rect.center().y;
            let x = if !null && self.source.right_aligned(col) && !text.contains(BREAK) {
                // Numbers sit right: measure first.
                right - galley(ui, &text, color, width, false).size().x
            } else {
                rect.left() + PAD
            };
            let drawn = paint_text(ui, &p, &text, (x, y, right), color, null);
            if deleted {
                p.line_segment([pos2(x, y), pos2(x + drawn, y)], Stroke::new(1.0, t.text_muted));
            }
        }
        let link = link.then(|| {
            let r = Rect::from_min_size(pos2(rect.right() - PAD - 15.0, rect.center().y - 8.0), vec2(16.0, 16.0));
            let resp = ui.interact(r, ui.id().with(("dbee-fk-link", row, col)), Sense::click());
            icon::follow(&p, r.shrink(1.0), if resp.hovered() { t.ink } else { t.text_muted });
            resp
        });
        Drawn { text, null, resp, link }
    }

    /// The right-click menu of cell `here`.
    fn menu(&self, ui: &mut egui::Ui, here: (usize, usize), tags: &mut Tags, acts: &mut Vec<GridAct>) {
        ui.set_min_width(220.0);
        let (row, col) = here;
        // `blocked`: why the entry cannot run now, or `Some("")` when it is disabled without one.
        let mut item = |ui: &mut egui::Ui, id: &str, label: &str, keys: Option<&str>, blocked: Option<&str>, act: GridAct| {
            let mut button = Button::new(label);
            if let Some(k) = keys {
                button = button.shortcut_text(k);
            }
            let mut r = ui.add_enabled(blocked.is_none(), button);
            let mut value = json!({ "label": label, "shortcut": keys });
            if let Some(why) = blocked.filter(|w| !w.is_empty()) {
                r = r.on_disabled_hover_text(why);
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
        if let Some(blocked) = edit {
            // A row to delete takes no new values.
            let null = blocked.or(self.source.deleted(row).then_some(""));
            item(ui, "grid-menu-null", "Set as NULL", None, null, GridAct::SetNull { row, col });
            ui.separator();
        }
        item(ui, "grid-menu-copy", "Copy", Some("Ctrl+C"), None, GridAct::Copy("tsv"));
        item(ui, "grid-menu-copy-header", "Copy with header", Some("Ctrl+Shift+C"), None, GridAct::Copy("tsvHeader"));
        item(ui, "grid-menu-copy-csv", "Copy as CSV", None, None, GridAct::Copy("csv"));
        item(ui, "grid-menu-copy-json", "Copy as JSON", None, None, GridAct::Copy("json"));
        item(ui, "grid-menu-copy-markdown", "Copy as Markdown", None, None, GridAct::Copy("markdown"));
        if let Some(blocked) = edit {
            ui.separator();
            let rows: Vec<usize> = match self.selection.filter(|_| covers(self.selection, here)).map(bounds) {
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
        let w = initial_widths(&rs, &[Key::Primary]);
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
        assert_eq!((src.column_name(9), src.mark(0, 0)), ("", Mark::Plain));
    }

    #[test]
    fn text_with_line_breaks_stays_inside_its_cell() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter().clone();
            let color = Color32::BLACK;
            let short = paint_text(ui, &p, "a↵b", (0.0, 10.0, 200.0), color, false);
            let plain = paint_text(ui, &p, "ab", (0.0, 10.0, 200.0), color, false);
            assert!((short - plain - BREAK_W).abs() < 0.5, "a break takes its own room: {short} vs {plain}");
            let long = grid_text(&"line↵".repeat(1000)).into_owned();
            for right in [30.0, 120.0] {
                let w = paint_text(ui, &p, &long, (0.0, 10.0, right), color, false);
                assert!(w <= right + 0.5, "{w} > {right}");
            }
            // No room at all: one `…` (the cell clips it), not every part.
            assert!(paint_text(ui, &p, &long, (0.0, 10.0, 1.0), color, false) < 20.0);
            assert_eq!(paint_text(ui, &p, "", (0.0, 10.0, 50.0), color, true), 0.0);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn selections_cover_their_rectangle_either_way_round() {
        let sel = Some(Selection { anchor: (5, 3), focus: (2, 1) });
        assert!(covers(sel, (2, 1)) && covers(sel, (5, 3)) && covers(sel, (3, 2)));
        assert!(!covers(sel, (6, 2)) && !covers(sel, (3, 0)) && !covers(None, (0, 0)));
        assert_eq!(gutter_width(9), gutter_width(1));
        assert!(gutter_width(1_000_000) > gutter_width(100));
    }
}

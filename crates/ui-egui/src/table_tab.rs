//! The table tab's body ([`show`]), from top to bottom, each area the body's full width (TA):
//!
//! - when the rows cannot be edited, a note in an area of its own saying why (`table-note`: a
//!   view, no row key, a read-only connection, or the table's own reason as the driver writes it),
//!   on one line: a longer one is cut short, and reads whole on hover;
//! - the filter bar ([`crate::filter_bar`]);
//! - the table bar ([`TableBar`]): DATA / STRUCTURE (`table.view`; STRUCTURE shows the
//!   table's structure instead, [`crate::structure_view`], and DATA's state waits unchanged), REFRESH, "+ Row" (the new row is selected and scrolled to), the rows' count with COUNT or
//!   COUNT ROWS, IMPORT and EXPORT (off until M4), and at its right end the value panel's toggle
//!   (`value-toggle`, `ui.toggleValuePanel`). A page load that has run 300 ms shows CANCEL in
//!   REFRESH's place and LOADING with its timer in the count's; a count that has run 300 ms shows
//!   COUNTING with its timer. The timers count from the frame that first saw the job
//!   ([`crate::quiet`]);
//! - the error band (`table-error`, [`crate::components::error_band`]) when the last Apply failed,
//!   with the server's message, detail and hint (the staged changes stay, and the grid marks the
//!   row it failed on), or else when the last page load failed, with the rows from before kept
//!   underneath;
//! - the grid ([`crate::grid`]) on the loaded page and its staged edits, sortable by a header
//!   click, with foreign key links on the selected row, and the cell editor over it while one is
//!   open ([`crate::editors`]; a double-click opens one). While a page loads over the rows (once
//!   it has run 300 ms) they are drawn at half, the header at full (LC2). Before a first page,
//!   once its load has run 300 ms, the header from the catalog over ten rows of bars
//!   ([`crate::grid::skeleton`], LC1; nothing before, and nothing for a table the catalog no
//!   longer has), or `No rows are loaded.` after a first load that failed or was cancelled;
//! - the paging bar ([`PagingBar`], its words and rules [`crate::paging`]): the arrows with the
//!   page's words, a cut page's note, and the staged changes with RESET and APPLY.
//!
//! While the value panel shows ([`crate::value_panel`]) it stands beside the band and the grid,
//! from the table bar's bottom to the paging bar's top; the note, the filter bar and the two bars
//! keep the body's width. It is filled after the grid, so a click on a cell shows its value in the
//! same frame.
//!
//! Every bar is cut at the body's edges. Short of room the table bar gives way in order
//! ([`TableBar::fit`]), the paging bar in its own, and a filter row wider than the bar is cut at
//! its right edge: no control is drawn over another or past the body at any width.
//!
//! It reads the tab from the engine each frame and acts only through commands, started with
//! [`DbeeApp::start`] after drawing (so a `needs…` answer opens its dialog and an error shows as a
//! notice, except a page load's or an Apply's database error, which the band shows). Whether the
//! rows take edits is the engine's rule (`cmd::edit::can_edit`). The window keeps one
//! [`GridState`] per tab (the column widths), dropped with the tab.

use std::collections::HashMap;
use std::time::Duration;

use dbee_core::{Column, ResultSet, TableInfo};
use dbee_engine::cmd::LOST;
use dbee_engine::cmd::edit::{APPLYING, can_edit};
use dbee_engine::cmd::table::{STAGED, quick_estimate};
use dbee_engine::{Edits, Json, Outcome, Showing, Tab, TabId, TableTab, Total, Which};
use dbee_sql::filter::Condition;
use dbee_sql::select::PAGE_SIZE;
use egui::{Align, CornerRadius, Layout, Rect, Sense, UiBuilder, pos2, vec2};
use serde_json::json;

use crate::app::Origin;
use crate::automation::Tags;
use crate::components::empty::{self, Phrase};
use crate::components::error_band::Band;
use crate::components::results::RowCount;
use crate::components::table_bars::{Loading, PagingAsked, PagingBar, PagingShown, TableAsked, TableBar, TableShown, View};
use crate::components::toast::Kind;
use crate::components::tooltip::Hint;
use crate::editors::{self, Place};
use crate::filter_bar::{self, Bar};
use crate::grid::{self, CELL, Grid, GridAct, GridSource, GridState, Key, Mark, RowEdits};
use crate::paging::{BAR_H, Paging, count_text, cut_note, edits_text, page_text};
use crate::quiet::Watch;
use crate::theme::size::{BAR_LINE, NOTE_BAR_H, NOTE_BAR_PAD_X};
use crate::theme::{self, Text};
use crate::{DbeeApp, structure_view, value_panel};

/// The commands that load a page (or read the structure), whose database errors the tab's band
/// shows.
const PAGE_LOADS: &[&str] = &["table.setFilters", "table.page", "table.sort", "table.refresh", "table.view"];

/// The grids' states, by tab.
pub(crate) type Grids = HashMap<TabId, GridState>;

/// A table tab's page and staged edits as the grid reads them: the loaded rows, then the new
/// ones; staged values over loaded ones.
struct Source<'a> {
    data: &'a ResultSet,
    edits: &'a Edits,
    /// The cells the last Apply wrote, in order (`TableTab::saved`).
    saved: &'a [(usize, usize)],
}

impl GridSource for Source<'_> {
    fn columns(&self) -> usize {
        self.data.columns()
    }
    fn column_name(&self, col: usize) -> &str {
        self.data.column_name(col)
    }
    fn column_type(&self, col: usize) -> &str {
        self.data.column_type(col)
    }
    fn rows(&self) -> usize {
        self.edits.grid_rows(self.data)
    }
    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        match self.edits.shown(self.data, row, col) {
            Some(staged) => staged,
            None => self.data.cell(row, col),
        }
    }
    fn mark(&self, row: usize, col: usize) -> Option<Mark> {
        let loaded = self.data.rows.len();
        if self.edits.failed.as_ref().is_some_and(|f| f.row == row) {
            Some(Mark::Failed)
        } else if row >= loaded {
            Some(Mark::New)
        } else if self.edits.deleted.contains(&row) {
            Some(Mark::Deleted)
        } else if self.edits.cells.contains_key(&(row, col)) {
            Some(Mark::Staged)
        } else if self.saved.binary_search(&(row, col)).is_ok() {
            Some(Mark::Saved)
        } else {
            None
        }
    }
    fn right_aligned(&self, col: usize) -> bool {
        self.data.right_aligned(col)
    }
    fn deleted(&self, row: usize) -> bool {
        self.edits.deleted.contains(&row)
    }
}

/// The key of each column of `data` in `info`.
fn keys(data: &ResultSet, info: Option<&TableInfo>) -> Vec<Option<Key>> {
    data.columns
        .iter()
        .map(|c| match info {
            Some(i) if i.primary_key.contains(&c.name) => Some(Key::Primary),
            Some(i) if i.fk_for(&c.name).is_some() => Some(Key::Foreign),
            _ => None,
        })
        .collect()
}

/// The columns `info` shows, with no rows: what a first load's skeleton draws its header from.
fn catalog_columns(info: &TableInfo) -> ResultSet {
    let columns = info
        .visible_columns()
        .map(|c| Column { name: c.name.clone(), base_type: c.base_type.clone(), type_label: c.label().to_owned(), class: c.class })
        .collect();
    ResultSet { columns, ..ResultSet::default() }
}

/// The sorted column of `data` and its direction, for the tab's sort.
fn sorted(data: &ResultSet, tab: &TableTab) -> Option<(usize, dbee_sql::filter::SortDir)> {
    tab.sort.as_ref().and_then(|s| data.columns.iter().position(|c| c.name == s.column).map(|col| (col, s.dir)))
}

/// The command a grid act asks for.
fn command(act: GridAct, data: Option<&ResultSet>) -> Option<(&'static str, Json)> {
    Some(match act {
        GridAct::Select { anchor, focus } => ("selection.set", json!({ "anchor": [anchor.0, anchor.1], "focus": [focus.0, focus.1] })),
        GridAct::SelectRows { first, last } => ("selection.rows", json!({ "first": first, "last": last })),
        GridAct::Sort(col) => ("table.sort", json!({ "column": data?.columns.get(col)?.name })),
        GridAct::Follow { row, col } => ("table.followForeignKey", json!({ "row": row, "column": col })),
        GridAct::SetNull { row, col } => ("cell.setNull", json!({ "row": row, "column": col })),
        GridAct::Copy(format) => ("clipboard.copy", json!({ "format": format })),
        GridAct::DeleteRows(rows) => ("row.delete", json!({ "rows": rows })),
        // The editor opens without a command (see `show`).
        GridAct::Edit { .. } => return None,
    })
}

/// Why the tab cannot load another page now (paging, sort, applying filters, refresh): a lost
/// connection or staged changes, as the engine's checks say.
fn blocked(t: &TableTab, lost: bool) -> Option<&'static str> {
    if lost {
        Some(LOST)
    } else if t.has_staged_edits() {
        Some(STAGED)
    } else {
        None
    }
}

/// What the paging bar shows for `t`.
fn paging(t: &TableTab, lost: bool) -> Paging {
    Paging {
        page: t.shown.page,
        at_end: t.view() == t.shown && !t.has_next,
        total: t.total,
        rows: t.data.as_ref().map_or(0, |d| d.rows.len()),
        complete: t.page_complete,
        edits: t.edits.count(),
        blocked: blocked(t, lost),
        apply_blocked: if lost {
            Some(LOST)
        } else if t.applying.is_some() {
            Some(APPLYING)
        } else {
            None
        },
    }
}

/// The table bar's row count for `total`, `estimate` being the catalog's for the table (when its
/// rows are not filtered), as [`count_text`] reads them: an estimate of 0 is none; a count shows
/// the estimate meanwhile, with its timer once it has run 300 ms (`counting`).
fn row_count(total: Total, estimate: Option<u64>, counting: Option<Duration>) -> RowCount {
    let estimate = estimate.filter(|n| *n > 0);
    match total {
        Total::Exact(n) => RowCount::Exact(n),
        Total::Estimate(n) if n > 0 => RowCount::Estimate(n),
        Total::Estimate(_) => RowCount::Unknown,
        Total::Unknown => estimate.map_or(RowCount::Unknown, RowCount::Estimate),
        Total::Counting { estimate: e } => RowCount::Counting { estimate: e.filter(|n| *n > 0).or(estimate), elapsed: counting },
    }
}

/// The rows a page shows, counted from 1 across pages: `(101, 200)` for page 2 of 100 rows; none
/// for an empty page.
fn shown_rows(page: u64, rows: usize) -> Option<(u64, u64)> {
    let before = page.saturating_mul(PAGE_SIZE);
    (rows > 0).then(|| (before.saturating_add(1), before.saturating_add(rows as u64)))
}

/// Tags what the table bar drew, says on hover why something is off, and gives the command its
/// click asked for.
fn table_bar_tags(
    shown: TableShown,
    tags: &mut Tags,
    words: (&str, &str),
    why: (Option<&str>, Option<&str>, Option<&str>),
    (open, narrow): (bool, bool),
) -> Option<(&'static str, Json)> {
    let (rows, count) = words;
    let (refresh_blocked, add_blocked, count_blocked) = why;
    view_tags(&shown, tags, View::Data);
    // Without its words (a narrow bar) REFRESH's hover names it; + ROW's always does.
    let words = shown.fit.words;
    if let Some(r) = shown.refresh {
        let r = match refresh_blocked {
            Some(why) => r.hint_disabled(format!("Refresh: not now, {why}")),
            None if words => r.hint("Load the page again"),
            None => r.hint("Refresh: load the page again"),
        };
        tags.tag_value_as("table-refresh", &r, "Refresh", refresh_blocked.is_none());
    }
    if let Some(r) = shown.cancel {
        let r = r.hint("Stop loading the page");
        tags.tag_value("table-cancel", &r, "Cancel");
    }
    if let Some(r) = shown.add {
        let r = match add_blocked {
            Some(why) => r.hint_disabled(format!("Cannot add a row now: {why}")),
            None => r.hint("Add a row, written with the other changes on Apply"),
        };
        tags.tag_value_as("row-add", &r, "+ Row", add_blocked.is_none());
    }
    if let Some(c) = shown.count {
        if let Some(r) = &c.words {
            tags.tag_value("table-rows", r, rows);
        }
        match (c.count, c.words.or(c.counting)) {
            (Some(r), _) => {
                let r = match count_blocked {
                    Some(why) => r.hint_disabled(format!("Can't count now: {why}.")),
                    None => r.hint("Count the rows exactly"),
                };
                tags.tag_value_as("page-count", &r, count, count_blocked.is_none());
            }
            (None, Some(r)) => tags.tag_value("page-count", &r, count),
            (None, None) => {}
        }
    }
    if let Some(r) = shown.facts {
        tags.tag_value("table-loading", &r, "Loading");
    }
    if let Some(r) = shown.import {
        tags.tag_value_as("table-import", &r, "Import", false);
    }
    if let Some(r) = shown.export {
        tags.tag_value_as("table-export", &r, "Export", false);
    }
    if let Some(r) = shown.value {
        value_panel::tag_toggle(tags, r, open, narrow);
    }
    Some(match shown.asked? {
        TableAsked::Refresh => ("table.refresh", json!({})),
        TableAsked::Cancel => ("table.cancel", json!({})),
        TableAsked::AddRow => ("row.add", json!({})),
        TableAsked::Count => ("table.count", json!({})),
        TableAsked::Value => ("ui.toggleValuePanel", json!({})),
        TableAsked::View(v) => view_command(v),
    })
}

/// Tags the view switch's segments, `chosen` the view that shows.
fn view_tags(shown: &TableShown, tags: &mut Tags, chosen: View) {
    if let [data, structure] = shown.view.as_slice() {
        tags.tag_value("table-view-data", data, chosen == View::Data);
        tags.tag_value("table-view-structure", structure, chosen == View::Structure);
    }
}

/// The command that shows view `v`.
fn view_command(v: View) -> (&'static str, Json) {
    let view = match v {
        View::Data => "data",
        View::Structure => "structure",
    };
    ("table.view", json!({ "view": view }))
}

/// Tags what the table bar drew in its structure state (as [`table_bar_tags`] does in DATA's):
/// LOADING while a read shows its loading state, else the facts as written (`structure-facts`).
fn structure_bar_tags(shown: TableShown, tags: &mut Tags, facts: &str, refresh_blocked: Option<&str>) -> Option<(&'static str, Json)> {
    view_tags(&shown, tags, View::Structure);
    if let Some(r) = shown.refresh {
        let r = match refresh_blocked {
            Some(why) => r.hint_disabled(format!("Refresh: not now, {why}")),
            None if shown.fit.words => r.hint("Read the structure again"),
            None => r.hint("Refresh: read the structure again"),
        };
        tags.tag_value_as("table-refresh", &r, "Refresh", refresh_blocked.is_none());
    }
    let loading = shown.cancel.is_some();
    if let Some(r) = shown.cancel {
        let r = r.hint("Stop reading the structure");
        tags.tag_value("table-cancel", &r, "Cancel");
    }
    if let Some(r) = shown.facts {
        if loading {
            tags.tag_value("table-loading", &r, "Loading");
        } else {
            tags.tag_value("structure-facts", &r, facts);
        }
    }
    Some(match shown.asked? {
        TableAsked::Refresh => ("table.refresh", json!({})),
        TableAsked::Cancel => ("table.cancel", json!({})),
        TableAsked::View(v) => view_command(v),
        // The structure state has none of these.
        TableAsked::AddRow | TableAsked::Count | TableAsked::Value => return None,
    })
}

/// The structure state's row count (brief §2.1): exact when the tab knows the whole table's
/// count, else the catalog's estimate above 0, else none. A count running in DATA shows as the
/// estimate.
fn structure_count(t: &TableTab, estimate: Option<u64>) -> RowCount {
    match t.total {
        Total::Exact(n) if t.shown.applied == Condition::None => RowCount::Exact(n),
        _ => estimate.filter(|n| *n > 0).map_or(RowCount::Unknown, RowCount::Estimate),
    }
}

/// Draws table tab `id` in its STRUCTURE view into `ui`: the table bar's structure state over
/// [`structure_view::show`], the body's full width (no note, filter bar, paging bar or value
/// panel), and runs what it asks for.
fn show_structure(app: &mut DbeeApp, ui: &mut egui::Ui, id: TabId) {
    let mut runs: Vec<(&'static str, Json)> = Vec::new();
    {
        let DbeeApp { session, view, tags, .. } = app;
        let Some(Tab::Table(tab)) = session.state().tab(id) else { return };
        let connected = session.state().connection(Which::Tab(id));
        let info = connected.and_then(|c| c.catalog.table(&tab.table));
        let lost = connected.is_none_or(|c| c.lost);
        let body = ui.available_rect_before_wrap();
        let clip = ui.clip_rect().intersect(body);
        // An editor open over a cell of DATA's grid left the view: it commits, as when scrolled
        // out of sight.
        if let Some(editor) = view.work.editor.as_mut().filter(|e| e.tab == id) {
            editors::left_view(editor, ui.ctx().input(|i| i.time));
        }
        let mut top = ui.new_child(UiBuilder::new().id_salt("dbee-table-tab").max_rect(body).layout(Layout::top_down(Align::Min)));
        top.set_clip_rect(clip);
        top.spacing_mut().item_spacing.y = 0.0;
        let ctx = top.ctx().clone();
        let loading = tab.structure.loading.and_then(|job| view.quiet.past(&ctx, Watch::Tab(id, job)));
        let estimate = info.and_then(|i| quick_estimate(session.state().dialect(Which::Tab(id)), i, &Condition::None));
        let columns = match (tab.structure.shown.as_deref(), info) {
            (Some(read), _) => structure_view::columns_of(read),
            (None, Some(i)) => i.visible_columns().count(),
            (None, None) => 0,
        };
        let refresh_blocked = lost.then_some(LOST);
        let table_bar = TableBar {
            view: View::Structure,
            structure: true,
            loading: loading.map(|elapsed| Loading { elapsed }),
            count: structure_count(tab, estimate),
            shown: None,
            columns,
            can_refresh: refresh_blocked.is_none(),
            can_add: false,
            can_count: false,
            value: None,
            value_off: false,
        };
        let facts = table_bar.structure_words();
        let drawn = table_bar.show(&mut top);
        runs.extend(structure_bar_tags(drawn, tags, &facts, refresh_blocked));
        let below = top.available_rect_before_wrap().top().min(body.bottom());
        let page = Rect::from_min_max(pos2(body.left(), below), body.max);
        let mut page_ui = top.new_child(UiBuilder::new().id_salt("dbee-structure").max_rect(page).layout(Layout::top_down(Align::Min)));
        page_ui.set_clip_rect(clip.intersect(page));
        let kept = view.work.structures.entry(id).or_default();
        let asked = structure_view::show(&mut page_ui, tags, id, tab, kept, loading, lost);
        // A link opens its table's tab when one is open on this connection, else a new one; a
        // link to this tab's own table (a key that references it) shows its DATA.
        runs.extend(asked.into_iter().map(|(command, params)| {
            let open = session.state().tabs.iter().find_map(|t| match t {
                Tab::Table(t) if t.connection == tab.connection && t.table.schema == params["schema"] && t.table.name == params["table"] => {
                    Some(t.id)
                }
                _ => None,
            });
            match open {
                Some(t) if command == "table.open" && t == id => ("table.view", json!({ "view": "data" })),
                Some(t) if command == "table.open" => ("tab.activate", json!({ "tab": t.0 })),
                _ => (command, params),
            }
        }));
    }
    for (command, params) in runs {
        // A read's failure shows in the page's band, not as a notice.
        let origin = if PAGE_LOADS.contains(&command) { Origin::Page } else { Origin::Menu };
        app.start(command, params, origin);
    }
}

/// Draws the paging bar for `p` into `ui`, tags what it drew, says on hover why something is off,
/// and gives the command its click asked for.
fn paging_bar(ui: &mut egui::Ui, p: &Paging, tags: &mut Tags) -> Option<(&'static str, Json)> {
    let page = page_text(p.page, p.total);
    let note = cut_note(p.rows, p.complete);
    let (back, forward) = (p.back_blocked(), p.forward_blocked());
    let bar = PagingBar {
        page: &page,
        back: back.is_none(),
        forward: forward.is_none(),
        changes: p.edits,
        can_apply: p.apply_blocked.is_none(),
        note: note.as_deref(),
    };
    let shown = bar.show(ui);
    paging_tags(shown, tags, p, (&page, note.as_deref()), (back, forward))
}

/// Tags what the paging bar drew (as [`table_bar_tags`] does the table bar's).
fn paging_tags(
    shown: PagingShown,
    tags: &mut Tags,
    p: &Paging,
    (page, note): (&str, Option<&str>),
    (back, forward): (Option<&str>, Option<&str>),
) -> Option<(&'static str, Json)> {
    let mut arrow = |id: &str, r: &egui::Response, why_not: Option<&str>, hover: &str| {
        let r = match why_not {
            Some(why) => r.clone().hint_disabled(format!("{hover}: not now, {why}")),
            None => r.clone().hint(hover),
        };
        tags.tag_value_as(id, &r, hover, why_not.is_none());
    };
    arrow("page-first", &shown.first, back, "First page");
    arrow("page-prev", &shown.previous, back, "Previous page");
    arrow("page-next", &shown.next, forward, "Next page");
    arrow("page-last", &shown.last, forward, "Last page (counts the rows)");
    tags.tag_value("page-number", &shown.page, page);
    if let (Some(r), Some(note)) = (&shown.note, note) {
        tags.tag_value("page-cut", r, note);
    }
    let edits = edits_text(p.edits);
    if let Some(r) = &shown.changes {
        tags.tag_value("edits-count", r, edits.as_str());
    }
    if let Some(r) = shown.reset {
        let r = r.hint("Drop the staged changes");
        tags.tag_value("edit-reset", &r, "Reset");
    }
    if let Some(r) = shown.apply {
        let r = match p.apply_blocked {
            Some(why) => r.hint_disabled(format!("Cannot apply {edits} now: {why}")),
            None => r.hint(format!("Write {edits} in one transaction (Ctrl+S)")),
        };
        tags.tag_value_as("edit-apply", &r, "Apply", p.apply_blocked.is_none());
    }
    Some(match shown.asked? {
        PagingAsked::First => ("table.page", json!({ "to": "first" })),
        PagingAsked::Previous => ("table.page", json!({ "to": "prev" })),
        PagingAsked::Next => ("table.page", json!({ "to": "next" })),
        PagingAsked::Last => ("table.page", json!({ "to": "last" })),
        PagingAsked::Reset => ("edit.reset", json!({})),
        PagingAsked::Apply => ("edit.apply", json!({})),
    })
}

/// The note for rows that cannot be edited, `why` being the engine's reason: an area of its own
/// across `ui`'s width, [`NOTE_BAR_H`] tall on `card` with a `line` along its bottom, the sentence
/// on one line in `small` `text_muted` (cut short, and whole on hover). The table's own reason
/// (`own`, the driver's) is a sentence and shows as written; dbee's are phrases.
fn note(ui: &mut egui::Ui, tags: &mut Tags, why: &str, own: Option<&str>) {
    let t = theme::tokens(ui.ctx());
    let text = if own == Some(why) { why.to_owned() } else { format!("These rows can't be edited: {why}.") };
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), NOTE_BAR_H), Sense::hover());
    let line = Rect::from_min_max(pos2(area.left(), area.bottom() - BAR_LINE), area.max);
    ui.painter().rect_filled(area, CornerRadius::ZERO, t.card);
    ui.painter().rect_filled(line, CornerRadius::ZERO, t.line);
    let left = area.left() + NOTE_BAR_PAD_X;
    let room = Rect::from_min_max(pos2(left, area.top()), pos2((area.right() - NOTE_BAR_PAD_X).max(left), line.top()));
    let mut inner = ui.new_child(UiBuilder::new().id_salt("dbee-table-note").max_rect(room).layout(Layout::left_to_right(Align::Center)));
    let r = inner.add(egui::Label::new(Text::Small.rich_as_written(text.as_str()).color(t.text_muted)).truncate());
    tags.tag_value("table-note", &r, text.as_str());
}

/// The band's note under a failed Apply.
pub(crate) const FAILED_NOTE: &str = "The changes are still staged, and the row the Apply failed on is marked: fix it and apply again, or reset.";

/// The band's note under an Apply cancelled before its commit.
pub(crate) const CANCELLED_NOTE: &str = "Cancelled before the commit: nothing was written, and the changes are still staged.";

/// The band's note under a page that did not load.
pub(crate) const PAGE_NOTE: &str = "The rows below are the ones from before.";

/// What the tab's error band says: a failed (or cancelled) Apply's heading, message, the
/// server's detail and hint, and note, else a failed page load's; `None` when neither failed.
/// With it, `table-error`'s value: the message as the engine keeps it, which ends with the detail
/// and hint the band shows under it as fields.
pub(crate) fn card_text(t: &TableTab) -> Option<(Band<'_>, &str)> {
    if let Some(failed) = &t.edits.failed {
        let (heading, note) = if t.edits.outcome_unknown() {
            ("The connection was lost during Apply", None)
        } else if failed.code == "cancelled" {
            ("The changes were not applied", Some(CANCELLED_NOTE))
        } else {
            ("The changes were not applied", Some(FAILED_NOTE))
        };
        let (detail, hint) = (failed.detail.as_deref(), failed.hint.as_deref());
        let message = without_fields(&failed.message, detail, hint);
        return Some((Band { heading, message, detail, hint, note, ..Band::default() }, failed.message.as_str()));
    }
    t.error.as_deref().map(|e| (Band { heading: "The page did not load", message: e, note: Some(PAGE_NOTE), ..Band::default() }, e))
}

/// `message` without the `Detail: …` and `Hint: …` lines a server error's words end with, when
/// they are `detail` and `hint` (the band shows those as fields of their own).
pub(crate) fn without_fields<'a>(message: &'a str, detail: Option<&str>, hint: Option<&str>) -> &'a str {
    let strip = |m: &'a str, field: Option<&str>, name: &str| -> &'a str {
        field.and_then(|f| m.strip_suffix(f)).and_then(|m| m.strip_suffix(name)).unwrap_or(m)
    };
    strip(strip(message, hint, "\nHint: "), detail, "\nDetail: ")
}

/// Tab `tab`'s error band across `ui`'s width, a 1 px `line` under it: something failed (see
/// [`card_text`]). Tagged `table-error`, its value `value`.
pub(crate) fn band(ui: &mut egui::Ui, tags: &mut Tags, tab: TabId, band: &Band<'_>, value: &str) {
    let r = band.show(ui, egui::Id::new(("dbee-table-error", tab.0))).response;
    tags.tag_value("table-error", &r, value);
    let (line, _) = ui.allocate_exact_size(vec2(r.rect.width(), BAR_LINE), Sense::hover());
    ui.painter().rect_filled(line, CornerRadius::ZERO, theme::tokens(ui.ctx()).line);
}

/// "+ Row": adds a row, then selects its first cell and scrolls to it.
fn add_row(app: &mut DbeeApp) {
    match app.run("row.add", json!({})) {
        Ok(Outcome::Done(v)) => {
            if let Some(row) = v["row"].as_u64() {
                app.start("selection.set", json!({ "anchor": [row, 0], "focus": [row, 0] }), Origin::Menu);
                app.tags.scroll_to(&format!("{CELL}{row}-0"));
            }
        }
        Ok(_) => {}
        Err(e) => app.notice(Kind::Error, crate::app::notice_text(&e)),
    }
}

/// Draws table tab `id` into `ui` and runs what it asks for. Whether it drew the value panel.
pub(crate) fn show(app: &mut DbeeApp, ui: &mut egui::Ui, id: TabId) -> bool {
    if matches!(app.session.state().tab(id), Some(Tab::Table(tab)) if tab.showing == Showing::Structure) {
        show_structure(app, ui, id);
        return false;
    }
    let mut runs: Vec<(&'static str, Json)> = Vec::new();
    let mut edit: Option<(usize, usize)> = None;
    let panel;
    {
        let DbeeApp { session, view, tags, .. } = app;
        let Some(Tab::Table(tab)) = session.state().tab(id) else { return false };
        let open = view.value_panel;
        let connected = session.state().connection(Which::Tab(id));
        let info = connected.and_then(|c| c.catalog.table(&tab.table));
        let lost = connected.is_none_or(|c| c.lost);
        let body = ui.available_rect_before_wrap();
        let split = (body.bottom() - BAR_H).max(body.top());
        // The panel gives way before the grid does: with no room beside it, its toggle is off.
        let narrow = open && !value_panel::has_room(body.width());
        // Whether a several-line editor opening now goes in the panel.
        view.panel.room = value_panel::has_room(body.width());
        // Every bar is cut at the body's edges: nothing is drawn past them at any width.
        let clip = ui.clip_rect().intersect(body);
        let mut top = ui.new_child(
            UiBuilder::new()
                .id_salt("dbee-table-tab")
                .max_rect(Rect::from_min_max(body.min, pos2(body.right(), split)))
                .layout(Layout::top_down(Align::Min)),
        );
        top.set_clip_rect(clip);
        top.spacing_mut().item_spacing.y = 0.0;
        let read_only = dbee_engine::cmd::table::not_editable(session, tab);
        // As the engine's edit commands allow: rows that can be edited, a connection that is not
        // lost, no page on its way and no Apply running.
        let edit_blocked = can_edit(session).err();
        if let Some(why) = read_only {
            note(&mut top, tags, &why, info.and_then(|i| i.no_edit_reason.as_deref()));
        }
        // The filter bar: the table's visible columns from the schema, else the page's.
        let columns: Vec<(&str, &str)> = match (info, tab.data.as_deref()) {
            (Some(i), _) => i.visible_columns().map(|c| (c.name.as_str(), c.label())).collect(),
            (None, Some(d)) => d.columns.iter().map(|c| (c.name.as_str(), c.type_label.as_str())).collect(),
            (None, None) => Vec::new(),
        };
        let bar = Bar {
            tab: id,
            columns,
            filters: &tab.filters,
            raw: &tab.raw,
            raw_mode: tab.raw_mode,
            disabled: lost.then_some(LOST),
            blocked: blocked(tab, lost),
            dialect: session.state().dialect(Which::Tab(id)),
        };
        runs.extend(filter_bar::show(&mut top, &bar, tags));
        // The table bar, its loading and counting states 300 ms after their jobs started.
        let ctx = top.ctx().clone();
        let loading = tab.loading.and_then(|job| view.quiet.past(&ctx, Watch::Tab(id, job)));
        let counting = tab.counting.and_then(|job| view.quiet.past(&ctx, Watch::Tab(id, job)));
        let estimate = info.and_then(|i| quick_estimate(session.state().dialect(Which::Tab(id)), i, &tab.applied));
        let count = row_count(tab.total, estimate, counting);
        let shown = shown_rows(tab.shown.page, tab.data.as_ref().map_or(0, |d| d.rows.len()));
        let refresh_blocked = blocked(tab, lost);
        let count_blocked = lost.then_some(LOST);
        let table_bar = TableBar {
            view: View::Data,
            structure: true,
            loading: loading.map(|elapsed| Loading { elapsed }),
            count,
            shown,
            columns: tab.data.as_ref().map_or(0, |d| d.columns()),
            can_refresh: refresh_blocked.is_none(),
            can_add: edit_blocked.is_none(),
            can_count: count_blocked.is_none(),
            value: Some(open && !narrow),
            value_off: narrow,
        };
        let drawn = table_bar.show(&mut top);
        let words = (count.words(shown), count_text(tab.total, estimate));
        runs.extend(table_bar_tags(drawn, tags, (&words.0, &words.1), (refresh_blocked, edit_blocked.as_deref(), count_blocked), (open, narrow)));
        // Under the table bar down to the paging bar: the band and the grid, with the value panel
        // at their right while it shows.
        let below = top.available_rect_before_wrap().top().min(split);
        panel = value_panel::reserve(open, ui, Rect::from_min_max(pos2(body.left(), below), pos2(body.right(), split)));
        // An editor in a panel that went away (hidden by an agent, or no room left) floats over
        // its cell from now on.
        if panel.is_none()
            && let Some(editor) = view.work.editor.as_mut().filter(|e| e.tab == id && e.place == Place::Panel)
        {
            editor.place = Place::Cell;
        }
        let right = panel.map_or(body.right(), |p| p.left);
        let results = Rect::from_min_max(pos2(body.left(), below), pos2(right, split));
        let mut top = ui.new_child(UiBuilder::new().id_salt("dbee-table-results").max_rect(results).layout(Layout::top_down(Align::Min)));
        top.set_clip_rect(clip.intersect(results));
        top.spacing_mut().item_spacing.y = 0.0;
        if let Some((text, value)) = card_text(tab) {
            band(&mut top, tags, id, &text, value);
        }
        match tab.data.as_ref() {
            Some(page) => {
                let data = page.as_ref();
                let source = Source { data, edits: &tab.edits, saved: &tab.saved };
                let state = view.work.grids.entry(id).or_default();
                let saved = if tab.saved.is_empty() { 0.0 } else { state.saved_strength(ui.ctx(), page) };
                let keys = keys(data, info);
                let sort = sorted(data, tab);
                let grid = Grid {
                    id: egui::Id::new(("dbee-grid", id.0)),
                    source: &source,
                    keys: &keys,
                    selection: tab.selection,
                    sort,
                    sortable: true,
                    blocked: blocked(tab, lost),
                    edits: match edit_blocked.as_deref() {
                        None => RowEdits::Allowed,
                        Some(why) => RowEdits::Blocked(why),
                    },
                    links: !lost,
                    first_number: tab.shown.page.saturating_mul(PAGE_SIZE).saturating_add(1),
                    editing: view.work.editor.as_ref().filter(|e| e.tab == id).map(|e| (e.row, e.col)),
                    edited_elsewhere: view.work.editor.as_ref().is_some_and(|e| e.tab == id && e.place == Place::Panel),
                    saved,
                    dimmed: loading.is_some(),
                };
                // The body's width, even when a row above it is wider (the filter bar in a narrow
                // window), so every column can be scrolled into view.
                let below = top.available_rect_before_wrap().top().min(split);
                let area = Rect::from_min_max(pos2(body.left(), below), pos2(right, split));
                let mut grid_ui = top.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
                for act in grid.show(&mut grid_ui, state, tags) {
                    if let GridAct::Edit { row, col } = act {
                        edit = Some((row, col));
                    }
                    runs.extend(command(act, Some(data)));
                }
                // One in the value panel is drawn there ([`value_panel::fill`]).
                if let Some(editor) = view.work.editor.as_mut().filter(|e| e.tab == id && e.place == Place::Cell) {
                    // egui lays out every column of a row in view, also those scrolled away to the
                    // side: whether the cell shows is whether it meets the rows in view.
                    match state.editing_rect().filter(|(cell, rows)| editors::in_view(*cell, *rows)) {
                        Some((cell, rows)) => editors::show(top.ctx(), editor, cell, rows, tags),
                        // Scrolled out of view, either way: it commits.
                        None => editors::left_view(editor, top.ctx().input(|i| i.time)),
                    }
                }
            }
            // A first page on its way: from 300 ms, the header over rows of bars where the grid
            // will be; before, nothing.
            None if tab.loading.is_some() => {
                if let (Some(_), Some(info)) = (loading, info) {
                    let columns = catalog_columns(info);
                    let keys = keys(&columns, Some(info));
                    let below = top.available_rect_before_wrap().top().min(split);
                    let area = Rect::from_min_max(pos2(body.left(), below), pos2(right, split));
                    let mut grid_ui = top.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
                    grid::skeleton(&mut grid_ui, &columns, &keys, sorted(&columns, tab), tags);
                }
            }
            // No page: the words stand where the grid would, 12 in and 12 down.
            None => {
                let below = top.available_rect_before_wrap().top().min(split);
                let area = Rect::from_min_max(pos2(body.left(), below), pos2(right, split));
                let words = "No rows are loaded.";
                let shown = Phrase::new(words, empty::Place::Top).show(&mut top, area);
                tags.tag_value("table-empty", &shown, words);
            }
        }
        let mut bottom = ui.new_child(UiBuilder::new().id_salt("dbee-table-paging").max_rect(Rect::from_min_max(pos2(body.left(), split), body.max)));
        bottom.set_clip_rect(clip);
        if tab.data.is_some() {
            runs.extend(paging_bar(&mut bottom, &paging(tab, lost), tags));
        }
    }
    for (command, params) in runs {
        if command == "row.add" {
            add_row(app);
            continue;
        }
        // A page that fails to load says so in the band above the grid.
        let origin = if PAGE_LOADS.contains(&command) { Origin::Page } else { Origin::Menu };
        app.start(command, params, origin);
    }
    // A double-click opens the cell's editor (it shows from the next frame), or says why not.
    if let Some((row, col)) = edit {
        let ctx = ui.ctx().clone();
        editors::open(app, &ctx, row, col);
    }
    // After the grid and what it asked for, so a cell selected in this frame shows its value now.
    if let Some(placed) = panel {
        value_panel::fill(app, ui, placed.contents);
    }
    panel.is_some()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use dbee_core::Column;
    use dbee_sql::edit::NewValue;

    use super::*;

    fn page() -> ResultSet {
        ResultSet {
            columns: vec![
                Column { name: "id".into(), base_type: "int4".into(), type_label: "integer".into(), class: dbee_core::TypeClass::Number },
                Column::untyped("note"),
            ],
            rows: vec![vec![Some("1".into()), Some("a".into())], vec![Some("2".into()), None], vec![Some("3".into()), Some("c".into())]],
            rows_seen: 3,
            complete: true,
            rows_affected: None,
            database_after: Default::default(),
        }
    }

    #[test]
    fn the_source_shows_staged_values_and_marks() {
        let data = page();
        let mut edits = Edits::default();
        edits.cells = BTreeMap::from([((0, 1), NewValue::Null)]);
        edits.new_rows = vec![BTreeMap::from([(1, NewValue::Text("new".into()))])];
        edits.deleted = BTreeSet::from([2]);
        edits.failed = Some(dbee_engine::Failed { row: 1, column: None, message: "bad".into(), code: "server", detail: None, hint: None });
        let s = Source { data: &data, edits: &edits, saved: &[] };
        assert_eq!(s.rows(), 4, "three loaded, one new");
        assert_eq!((s.cell(0, 1), s.cell(1, 1), s.cell(3, 1), s.cell(3, 0)), (None, None, Some("new"), None));
        assert_eq!(
            [s.mark(0, 1), s.mark(0, 0), s.mark(1, 0), s.mark(2, 0), s.mark(3, 0)],
            [Some(Mark::Staged), None, Some(Mark::Failed), Some(Mark::Deleted), Some(Mark::New)]
        );
        assert!(s.right_aligned(0) && !s.right_aligned(1));
        assert!(s.deleted(2) && !s.deleted(1) && !s.deleted(3));
        // A row to delete whose delete failed shows as failed, and stays struck through.
        edits.failed =
            Some(dbee_engine::Failed { row: 2, column: None, message: "still referenced".into(), code: "server", detail: None, hint: None });
        let s = Source { data: &data, edits: &edits, saved: &[] };
        assert_eq!((s.mark(2, 0), s.deleted(2)), (Some(Mark::Failed), true));
        // A saved cell is marked saved, unless another mark is on it.
        let s = Source { data: &data, edits: &edits, saved: &[(0, 0), (0, 1), (2, 1)] };
        assert_eq!([s.mark(0, 0), s.mark(0, 1), s.mark(2, 1), s.mark(1, 1)], [Some(Mark::Saved), Some(Mark::Staged), Some(Mark::Failed), None]);
    }

    #[test]
    fn a_cancelled_apply_has_a_note_of_its_own() {
        let view = dbee_engine::PageView { applied: dbee_sql::filter::Condition::None, sort: None, page: 0 };
        let mut t = TableTab::new(TabId(1), dbee_engine::ConnectionId(1), dbee_core::TableRef { schema: "s".into(), name: "t".into() }, vec![], view);
        let failed = |code| dbee_engine::Failed { row: 0, column: None, message: "m".into(), code, detail: None, hint: None };
        fn said(t: &TableTab) -> Option<(&str, &str, Option<&str>)> {
            card_text(t).map(|(b, _)| (b.heading, b.message, b.note))
        }
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed("server"));
        assert_eq!(said(&t), Some(("The changes were not applied", "m", Some(FAILED_NOTE))));
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed("cancelled"));
        assert_eq!(said(&t), Some(("The changes were not applied", "m", Some(CANCELLED_NOTE))));
        assert_eq!(CANCELLED_NOTE, "Cancelled before the commit: nothing was written, and the changes are still staged.");
    }

    /// A failed Apply's band shows the server's detail and hint; a page that did not load has none.
    #[test]
    fn a_failed_apply_carries_its_detail_and_hint() {
        let view = dbee_engine::PageView { applied: dbee_sql::filter::Condition::None, sort: None, page: 0 };
        let mut t = TableTab::new(TabId(1), dbee_engine::ConnectionId(1), dbee_core::TableRef { schema: "s".into(), name: "t".into() }, vec![], view);
        // The message the engine keeps ends with the detail and hint; the band shows them as fields,
        // and `table-error`'s value stays the whole message.
        let whole = "m (SQLSTATE 23502)\nDetail: d\nHint: h";
        let failed = |detail: Option<&str>, hint: Option<&str>| dbee_engine::Failed {
            row: 0,
            column: None,
            message: whole.into(),
            code: "server",
            detail: detail.map(str::to_owned),
            hint: hint.map(str::to_owned),
        };
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed(Some("d"), Some("h")));
        let band = card_text(&t).map(|(b, value)| (b.message, b.detail, b.hint, value));
        assert_eq!(band, Some(("m (SQLSTATE 23502)", Some("d"), Some("h"), whole)));
        // Words that don't end as the fields say stay whole.
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed(Some("other"), None));
        assert_eq!(card_text(&t).map(|(b, _)| b.message), Some(whole));
        std::sync::Arc::make_mut(&mut t.edits).failed = None;
        t.error = Some("e".into());
        let band = card_text(&t).map(|(b, value)| (b.heading, b.message, b.detail, b.hint, b.note, value));
        assert_eq!(band, Some(("The page did not load", "e", None, None, Some(PAGE_NOTE), "e")));
    }

    #[test]
    fn grid_acts_become_commands() {
        let data = page();
        let c = |act| command(act, Some(&data));
        assert_eq!(c(GridAct::Sort(1)), Some(("table.sort", json!({ "column": "note" }))));
        assert_eq!(c(GridAct::Sort(9)), None, "no such column");
        assert_eq!(c(GridAct::Select { anchor: (1, 0), focus: (2, 1) }), Some(("selection.set", json!({ "anchor": [1, 0], "focus": [2, 1] }))));
        assert_eq!(c(GridAct::SelectRows { first: 3, last: 1 }), Some(("selection.rows", json!({ "first": 3, "last": 1 }))));
        assert_eq!(c(GridAct::Copy("csv")), Some(("clipboard.copy", json!({ "format": "csv" }))));
        assert_eq!(c(GridAct::DeleteRows(vec![0, 2])), Some(("row.delete", json!({ "rows": [0, 2] }))));
        assert_eq!(c(GridAct::SetNull { row: 1, col: 0 }), Some(("cell.setNull", json!({ "row": 1, "column": 0 }))));
        assert_eq!(c(GridAct::Follow { row: 0, col: 1 }), Some(("table.followForeignKey", json!({ "row": 0, "column": 1 }))));
        assert_eq!(c(GridAct::Edit { row: 0, col: 1 }), None, "the editor opens without a command");
    }

    #[test]
    fn keys_come_from_the_catalog() {
        let data = page();
        let info = TableInfo {
            table: dbee_core::TableRef { schema: "s".into(), name: "t".into() },
            kind: dbee_core::RelKind::Table,
            columns: vec![],
            primary_key: vec!["id".into()],
            unique_keys: vec![],
            foreign_keys: vec![dbee_core::ForeignKey {
                name: "fk".into(),
                columns: vec!["note".into()],
                target: dbee_core::TableRef { schema: "s".into(), name: "u".into() },
                target_columns: vec!["id".into()],
            }],
            estimated_rows: None,
            no_edit_reason: None,
        };
        assert_eq!(keys(&data, Some(&info)), [Some(Key::Primary), Some(Key::Foreign)]);
        assert_eq!(keys(&data, None), [None, None]);
    }
}

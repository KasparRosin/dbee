//! The table tab's body ([`show`]), from top to bottom:
//!
//! - a small note when the rows cannot be edited, saying why (`table-note`: a view, no row key,
//!   a read-only connection);
//! - the filter bar ([`crate::filter_bar`]);
//! - an inline card (`table-error`) when the last Apply failed, with the server's message (the
//!   staged changes stay, and the grid marks the row it failed on), or else when the last page
//!   load failed, with the rows from before kept underneath;
//! - the grid ([`crate::grid`]) on the loaded page and its staged edits, sortable by a header
//!   click, with foreign key links on the selected row, and the cell editor over it while one is
//!   open ([`crate::editors`]; a double-click opens one);
//! - the paging bar ([`crate::paging`]), with "+ Row": the new row is selected and scrolled to.
//!
//! It reads the tab from the engine each frame and acts only through commands, started with
//! [`DbeeApp::start`] after drawing (so a `needs…` answer opens its dialog and an error shows as a
//! notice, except a page load's or an Apply's database error, which the card shows). Whether the
//! rows take edits is the engine's rule (`cmd::edit::can_edit`). The window keeps one
//! [`GridState`] per tab (the column widths), dropped with the tab.

use std::collections::HashMap;

use dbee_core::{ResultSet, TableInfo};
use dbee_engine::cmd::LOST;
use dbee_engine::cmd::edit::{APPLYING, can_edit};
use dbee_engine::cmd::table::{STAGED, quick_estimate};
use dbee_engine::{Edits, Json, Outcome, Tab, TabId, TableTab};
use dbee_sql::select::PAGE_SIZE;
use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, UiBuilder, pos2};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::editors;
use crate::filter_bar::{self, Bar};
use crate::grid::{CELL, Grid, GridAct, GridSource, GridState, Key, Mark, RowEdits};
use crate::paging::{self, BAR_H, Paging};
use crate::theme::{RADIUS, TOKENS};

/// The commands that load a page, whose database errors the tab's card shows.
const PAGE_LOADS: &[&str] = &["table.setFilters", "table.page", "table.sort", "table.refresh"];

/// The grids' states, by tab.
pub(crate) type Grids = HashMap<TabId, GridState>;

/// A table tab's page and staged edits as the grid reads them: the loaded rows, then the new
/// ones; staged values over loaded ones.
struct Source<'a> {
    data: &'a ResultSet,
    edits: &'a Edits,
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
    fn mark(&self, row: usize, col: usize) -> Mark {
        let loaded = self.data.rows.len();
        if self.edits.failed.as_ref().is_some_and(|f| f.row == row) {
            Mark::Failed
        } else if row >= loaded {
            Mark::New
        } else if self.edits.deleted.contains(&row) {
            Mark::Deleted
        } else if self.edits.cells.contains_key(&(row, col)) {
            Mark::Staged
        } else {
            Mark::Plain
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
fn keys(data: &ResultSet, info: Option<&TableInfo>) -> Vec<Key> {
    data.columns
        .iter()
        .map(|c| match info {
            Some(i) if i.primary_key.contains(&c.name) => Key::Primary,
            Some(i) if i.fk_for(&c.name).is_some() => Key::Foreign,
            _ => Key::None,
        })
        .collect()
}

/// The command a grid act asks for.
fn command(act: GridAct, data: Option<&ResultSet>) -> Option<(&'static str, Json)> {
    Some(match act {
        GridAct::Select { anchor, focus } => ("selection.set", json!({ "anchor": [anchor.0, anchor.1], "focus": [focus.0, focus.1] })),
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

/// What the paging bar shows for `t`; `edit_blocked` is why it takes no edits now.
fn paging(t: &TableTab, info: Option<&TableInfo>, lost: bool, edit_blocked: Option<String>) -> Paging {
    Paging {
        page: t.shown.page,
        at_end: t.view() == t.shown && !t.has_next,
        total: t.total,
        estimate: info.and_then(|i| quick_estimate(i, &t.applied)),
        rows: t.data.as_ref().map_or(0, |d| d.rows.len()),
        complete: t.page_complete,
        loading: t.loading.is_some(),
        edits: t.edits.count(),
        blocked: blocked(t, lost),
        count_blocked: lost.then_some(LOST),
        apply_blocked: if lost {
            Some(LOST)
        } else if t.applying.is_some() {
            Some(APPLYING)
        } else {
            None
        },
        add_blocked: edit_blocked,
    }
}

/// The note above the grid for rows that cannot be edited.
fn note(ui: &mut egui::Ui, tags: &mut Tags, why: &str) {
    let text = format!("These rows cannot be edited: {why}.");
    let r = ui.label(RichText::new(text.as_str()).size(12.0).color(TOKENS.text_muted));
    tags.tag_value("table-note", &r, text.as_str());
}

/// The card's note under a failed Apply.
const FAILED_NOTE: &str = "The changes are still staged, and the row the Apply failed on is marked: fix it and apply again, or reset.";

/// The card's note under an Apply cancelled before its commit.
const CANCELLED_NOTE: &str = "Cancelled before the commit: nothing was written, and the changes are still staged.";

/// What the tab's card says: a failed (or cancelled) Apply's heading, message and note, else a
/// failed page load's; `None` when neither failed.
fn card_text(t: &TableTab) -> Option<(&'static str, &str, &'static str)> {
    if let Some(failed) = &t.edits.failed {
        let message = failed.message.as_str();
        return Some(if t.edits.outcome_unknown() {
            ("The connection was lost during Apply", message, "")
        } else if failed.code == "cancelled" {
            ("The changes were not applied", message, CANCELLED_NOTE)
        } else {
            ("The changes were not applied", message, FAILED_NOTE)
        });
    }
    t.error.as_deref().map(|e| ("The page did not load", e, "The rows below are the ones from before."))
}

/// The tab's card: something failed (see [`card_text`]).
fn error_card(ui: &mut egui::Ui, tags: &mut Tags, (heading, error, note): (&str, &str, &str)) {
    let t = TOKENS;
    let frame =
        Frame::new().fill(t.card).stroke(Stroke::new(1.0, t.danger)).corner_radius(CornerRadius::same(RADIUS)).inner_margin(Margin::symmetric(12, 8));
    let r = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(heading).strong().color(t.danger));
            ui.add(egui::Label::new(RichText::new(error).color(t.danger)).wrap());
            if !note.is_empty() {
                ui.add(egui::Label::new(RichText::new(note).size(11.5).color(t.text_muted)).wrap());
            }
        })
        .response;
    tags.tag_value("table-error", &r, error);
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
        Err(e) => app.notice(crate::app::notice_text(&e)),
    }
}

/// Draws table tab `id` into `ui` and runs what it asks for.
pub(crate) fn show(app: &mut DbeeApp, ui: &mut egui::Ui, id: TabId) {
    let t = TOKENS;
    let mut runs: Vec<(&'static str, Json)> = Vec::new();
    let mut edit: Option<(usize, usize)> = None;
    {
        let DbeeApp { session, view, tags, .. } = app;
        let Some(Tab::Table(tab)) = session.state().tab(id) else { return };
        let connected = session.state().connected.as_ref();
        let info = connected.and_then(|c| c.catalog.table(&tab.table));
        let lost = connected.is_none_or(|c| c.lost);
        let full = ui.available_rect_before_wrap();
        let split = (full.bottom() - BAR_H).max(full.top());
        let mut top = ui.new_child(
            UiBuilder::new()
                .id_salt("dbee-table-tab")
                .max_rect(Rect::from_min_max(full.min, pos2(full.right(), split)))
                .layout(Layout::top_down(Align::Min)),
        );
        top.add_space(6.0);
        let read_only = dbee_engine::cmd::table::not_editable(session, tab);
        // As the engine's edit commands allow: rows that can be edited, a connection that is not
        // lost, no page on its way and no Apply running.
        let edit_blocked = can_edit(session).err();
        if let Some(why) = read_only {
            note(&mut top, tags, why);
            top.add_space(2.0);
        }
        // The filter bar: the table's columns from the schema, else the page's.
        let columns: Vec<(&str, &str)> = match (info, tab.data.as_deref()) {
            (Some(i), _) => i.columns.iter().map(|c| (c.name.as_str(), c.type_sql.as_str())).collect(),
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
        };
        runs.extend(filter_bar::show(&mut top, &bar, tags));
        top.add_space(6.0);
        if let Some(text) = card_text(tab) {
            error_card(&mut top, tags, text);
            top.add_space(6.0);
        }
        match tab.data.as_deref() {
            Some(data) => {
                let source = Source { data, edits: &tab.edits };
                let keys = keys(data, info);
                let sort = tab.sort.as_ref().and_then(|s| data.columns.iter().position(|c| c.name == s.column).map(|col| (col, s.dir)));
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
                };
                let state = view.work.grids.entry(id).or_default();
                // Within the tab's width, even when a row above it is wider (the filter bar in a
                // narrow window), so every column can be scrolled into view.
                let area = top.available_rect_before_wrap().intersect(Rect::from_min_max(full.min, pos2(full.right(), split)));
                let mut grid_ui = top.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
                for act in grid.show(&mut grid_ui, state, tags) {
                    if let GridAct::Edit { row, col } = act {
                        edit = Some((row, col));
                    }
                    runs.extend(command(act, Some(data)));
                }
                if let Some(editor) = view.work.editor.as_mut().filter(|e| e.tab == id) {
                    // egui lays out every column of a row in view, also those scrolled away to the
                    // side: whether the cell shows is whether it meets the rows in view.
                    match state.editing_rect().filter(|(cell, rows)| editors::in_view(*cell, *rows)) {
                        Some((cell, rows)) => editors::show(top.ctx(), editor, cell, rows, tags),
                        // Scrolled out of view, either way: it commits.
                        None => editors::left_view(editor, top.ctx().input(|i| i.time)),
                    }
                }
            }
            None if tab.loading.is_some() => {
                top.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(t.text_muted));
                    ui.label(RichText::new(format!("Loading {}…", tab.table.display())).color(t.text_muted));
                });
            }
            None => {
                top.label(RichText::new("No rows are loaded.").color(t.text_muted));
            }
        }
        let mut bottom = ui.new_child(UiBuilder::new().id_salt("dbee-table-paging").max_rect(Rect::from_min_max(pos2(full.left(), split), full.max)));
        if tab.data.is_some() {
            runs.extend(paging::show(&mut bottom, &paging(tab, info, lost, edit_blocked), tags));
        }
    }
    for (command, params) in runs {
        if command == "row.add" {
            add_row(app);
            continue;
        }
        // A page that fails to load says so in the card above the grid.
        let origin = if PAGE_LOADS.contains(&command) { Origin::Page } else { Origin::Menu };
        app.start(command, params, origin);
    }
    // A double-click opens the cell's editor (it shows from the next frame), or says why not.
    if let Some((row, col)) = edit {
        let ctx = ui.ctx().clone();
        editors::open(app, &ctx, row, col);
    }
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
        }
    }

    #[test]
    fn the_source_shows_staged_values_and_marks() {
        let data = page();
        let mut edits = Edits::default();
        edits.cells = BTreeMap::from([((0, 1), NewValue::Null)]);
        edits.new_rows = vec![BTreeMap::from([(1, NewValue::Text("new".into()))])];
        edits.deleted = BTreeSet::from([2]);
        edits.failed = Some(dbee_engine::Failed { row: 1, column: None, message: "bad".into(), code: "server" });
        let s = Source { data: &data, edits: &edits };
        assert_eq!(s.rows(), 4, "three loaded, one new");
        assert_eq!((s.cell(0, 1), s.cell(1, 1), s.cell(3, 1), s.cell(3, 0)), (None, None, Some("new"), None));
        assert_eq!(
            [s.mark(0, 1), s.mark(0, 0), s.mark(1, 0), s.mark(2, 0), s.mark(3, 0)],
            [Mark::Staged, Mark::Plain, Mark::Failed, Mark::Deleted, Mark::New]
        );
        assert!(s.right_aligned(0) && !s.right_aligned(1));
        assert!(s.deleted(2) && !s.deleted(1) && !s.deleted(3));
        // A row to delete whose delete failed shows as failed, and stays struck through.
        edits.failed = Some(dbee_engine::Failed { row: 2, column: None, message: "still referenced".into(), code: "server" });
        let s = Source { data: &data, edits: &edits };
        assert_eq!((s.mark(2, 0), s.deleted(2)), (Mark::Failed, true));
    }

    #[test]
    fn a_cancelled_apply_has_a_note_of_its_own() {
        let view = dbee_engine::PageView { applied: dbee_sql::filter::Condition::None, sort: None, page: 0 };
        let mut t = TableTab::new(TabId(1), dbee_core::TableRef { schema: "s".into(), name: "t".into() }, vec![], view);
        let failed = |code| dbee_engine::Failed { row: 0, column: None, message: "m".into(), code };
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed("server"));
        assert_eq!(card_text(&t), Some(("The changes were not applied", "m", FAILED_NOTE)));
        std::sync::Arc::make_mut(&mut t.edits).failed = Some(failed("cancelled"));
        assert_eq!(card_text(&t), Some(("The changes were not applied", "m", CANCELLED_NOTE)));
        assert_eq!(CANCELLED_NOTE, "Cancelled before the commit: nothing was written, and the changes are still staged.");
    }

    #[test]
    fn grid_acts_become_commands() {
        let data = page();
        let c = |act| command(act, Some(&data));
        assert_eq!(c(GridAct::Sort(1)), Some(("table.sort", json!({ "column": "note" }))));
        assert_eq!(c(GridAct::Sort(9)), None, "no such column");
        assert_eq!(c(GridAct::Select { anchor: (1, 0), focus: (2, 1) }), Some(("selection.set", json!({ "anchor": [1, 0], "focus": [2, 1] }))));
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
        assert_eq!(keys(&data, Some(&info)), [Key::Primary, Key::Foreign]);
        assert_eq!(keys(&data, None), [Key::None, Key::None]);
    }
}

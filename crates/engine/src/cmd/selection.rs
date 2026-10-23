//! The grid selection, copy and the value view: `selection.set`, `selection.all`,
//! `copy.selection` and `value.show`.
//!
//! They act on the active tab's grid: on a table tab the loaded page followed by its new rows, on
//! a query tab the shown result (see [`crate::view`]). A selection is the rectangle between an
//! anchor and a focus cell, `[row, column]`, 0-based. `selection.set` takes an anchor that must
//! lie in the grid (`BadParams` otherwise, and nothing changes) and clamps the focus to the edge,
//! so a drag past the last row selects to it. A selection that stops fitting (a new row undone, a
//! shorter page) is dropped when it is read.
//!
//! `copy.selection` renders the rectangle in grid order with the staged values, as text for the
//! clipboard (the UI puts it there); it needs no connection, and none of these commands is
//! journaled. The cell values never reach the log.

use dbee_core::Column;
use dbee_core::copy::{CopyFormat, render};
use serde_json::json;

use super::{CommandSpec, always, p};
use crate::state::{Selection, Tab};
use crate::view::Grid;
use crate::{EngineError, Json, Outcome, Session, TabId};

const SET: &str = "selection.set";
const ALL: &str = "selection.all";
const COPY: &str = "copy.selection";
const SHOW: &str = "value.show";

/// Why the selection commands are disabled with nothing to select in.
pub const NO_GRID: &str = "no rows are shown";
/// Why copy is disabled when nothing is selected.
pub const NOTHING_SELECTED: &str = "nothing is selected";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: SET,
        label: "Select Cells",
        menu: &[],
        shortcut: None,
        params: "{anchor: [row, column], focus?: [row, column] = anchor}",
        enabled: has_grid,
        run: set,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: ALL,
        label: "Select All",
        menu: &["Edit"],
        shortcut: Some("Ctrl+A"),
        params: "{}",
        enabled: has_cells,
        run: all,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: COPY,
        label: "Copy Selection",
        // The menu's Copy is the UI's `clipboard.copy`, which puts this text on the clipboard.
        menu: &[],
        shortcut: None,
        params: "{format: \"tsv\" | \"tsvHeader\" | \"csv\" | \"json\" | \"markdown\"} -> {text, rows, columns}",
        enabled: has_selection,
        run: copy,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SHOW,
        label: "Show Value",
        menu: &[],
        shortcut: None,
        params: "{} -> the value view of the focus cell, or null",
        enabled: always,
        run: show,
        journal: false,
        undoable: false,
    },
];

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { id: String::new(), msg: msg.into() }
}

fn disabled(why: &str) -> EngineError {
    EngineError::Disabled { id: String::new(), why: why.to_string() }
}

fn grid(s: &Session) -> Option<Grid<'_>> {
    s.state.active_tab().and_then(Grid::of)
}

fn has_grid(s: &Session) -> Result<(), String> {
    grid(s).map(|_| ()).ok_or_else(|| NO_GRID.into())
}

fn has_cells(s: &Session) -> Result<(), String> {
    match grid(s) {
        Some(g) if g.rows() > 0 && g.columns() > 0 => Ok(()),
        _ => Err(NO_GRID.into()),
    }
}

fn has_selection(s: &Session) -> Result<(), String> {
    match grid(s) {
        Some(g) if g.selection().is_some() => Ok(()),
        Some(_) => Err(NOTHING_SELECTED.into()),
        None => Err(NO_GRID.into()),
    }
}

/// A `[row, column]` param.
fn position(params: &Json, key: &str) -> Result<Option<(usize, usize)>, EngineError> {
    let Some(v) = params.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let whole = |v: &Json| v.as_u64().and_then(|n| usize::try_from(n).ok());
    match v.as_array().map(Vec::as_slice) {
        Some([row, col]) => match (whole(row), whole(col)) {
            (Some(row), Some(col)) => Ok(Some((row, col))),
            _ => Err(bad(format!("`{key}` must hold two whole numbers, 0 or more"))),
        },
        _ => Err(bad(format!("`{key}` must be [row, column]"))),
    }
}

fn store(s: &mut Session, tab: TabId, selection: Option<Selection>) {
    match s.state.tab_mut(tab) {
        Some(Tab::Table(t)) => t.selection = selection,
        Some(Tab::Query(q)) => q.selection = selection,
        None => {}
    }
}

/// `{anchor, focus?}` → `{anchor, focus}` as stored.
fn set(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let anchor = position(params, "anchor")?.ok_or_else(|| bad("`anchor` is required ([row, column])"))?;
    let focus = position(params, "focus")?.unwrap_or(anchor);
    let g = grid(s).ok_or_else(|| disabled(NO_GRID))?;
    let sel = g
        .fit(Selection { anchor, focus })
        .ok_or_else(|| bad(format!("`anchor` [{}, {}] is outside the grid ({} rows, {} columns)", anchor.0, anchor.1, g.rows(), g.columns())))?;
    let tab = g.tab;
    store(s, tab, Some(sel));
    Ok(Outcome::Done(json!({ "anchor": [sel.anchor.0, sel.anchor.1], "focus": [sel.focus.0, sel.focus.1] })))
}

/// `{}` → `{rows, columns}`: selects every cell.
fn all(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let g = grid(s).filter(|g| g.rows() > 0 && g.columns() > 0).ok_or_else(|| disabled(NO_GRID))?;
    let (rows, columns, tab) = (g.rows(), g.columns(), g.tab);
    let sel = Selection { anchor: (0, 0), focus: (rows.saturating_sub(1), columns.saturating_sub(1)) };
    store(s, tab, Some(sel));
    Ok(Outcome::Done(json!({ "rows": rows, "columns": columns })))
}

/// `{format}` → `{text, rows, columns}`.
fn copy(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = p::str(params, "format")?;
    let format = CopyFormat::from_id(id).ok_or_else(|| bad("`format` must be tsv, tsvHeader, csv, json or markdown"))?;
    let g = grid(s).ok_or_else(|| disabled(NO_GRID))?;
    let sel = g.selection().ok_or_else(|| disabled(NOTHING_SELECTED))?;
    let (r0, r1) = (sel.anchor.0.min(sel.focus.0), sel.anchor.0.max(sel.focus.0));
    let (c0, c1) = (sel.anchor.1.min(sel.focus.1), sel.anchor.1.max(sel.focus.1));
    let columns: &[Column] = g.data.columns.get(c0..=c1).unwrap_or_default();
    let rows: Vec<Vec<Option<&str>>> = (r0..=r1).map(|row| (c0..=c1).map(|col| g.cell(row, col)).collect()).collect();
    let text = render(format, columns, &rows);
    Ok(Outcome::Done(json!({ "text": text, "rows": rows.len(), "columns": columns.len() })))
}

/// `{}` → the focus cell's value view, or null.
fn show(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    Ok(Outcome::Done(match s.value_view_shared() {
        Some(v) => serde_json::to_value(&*v).unwrap_or(Json::Null),
        None => Json::Null,
    }))
}

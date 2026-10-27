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
//! journaled. The cell values never reach the log. The copied text is at most [`COPY_MAX_BYTES`],
//! everything counted (quotes, escapes, `NULL`, column names): cells holding more are refused
//! before anything is rendered (in a selection of up to a million cells, which are added up
//! first), and a text that would pass it is given up as soon as it would. Either way the command
//! is `Disabled`, saying how much is selected or how many rows fit, and asking for fewer rows.
//! The cells are read from the grid as they are written, so nothing the size of the selection is
//! built first.

use dbee_core::Column;
use dbee_core::copy::{Cells, CopyFormat, TooLarge, render_within};
use serde_json::json;

use super::{CommandSpec, always, bad, disabled, p};
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
/// The most text one copy puts on the clipboard, in bytes. Every copy measured, copied or
/// refused, stays well under a 250 ms frame at 8 MiB in a dev build; at 16 MiB the slowest (JSON
/// of a value of line breaks, and TSV of 10,000 rows of 1,664 NULLs) took up to 280 ms.
pub const COPY_MAX_BYTES: usize = 8 * 1024 * 1024;

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

/// The selected rectangle of a grid, read cell by cell.
struct Rect<'g, 'a> {
    grid: &'g Grid<'a>,
    top: usize,
    left: usize,
    rows: usize,
    columns: usize,
}

impl Cells for Rect<'_, '_> {
    fn rows(&self) -> usize {
        self.rows
    }
    fn width(&self, _: usize) -> usize {
        self.columns
    }
    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        if row >= self.rows || col >= self.columns {
            return None;
        }
        self.grid.cell(self.top.saturating_add(row), self.left.saturating_add(col))
    }
    fn each<E>(&self, row: usize, mut f: impl FnMut(Option<&str>) -> Result<(), E>) -> Result<(), E> {
        if row >= self.rows {
            return Ok(());
        }
        if self.grid.edits.is_some() {
            // A table tab's page: staged values come first, cell by cell.
            for col in 0..self.columns {
                f(self.cell(row, col))?;
            }
            return Ok(());
        }
        // A query's result: the row's cells straight from the data, NULL past a short row's end.
        let cells = self.grid.data.rows.get(self.top.saturating_add(row)).and_then(|r| r.get(self.left..)).unwrap_or_default();
        let cells = cells.get(..self.columns).unwrap_or(cells);
        for v in cells {
            f(v.as_deref())?;
        }
        for _ in cells.len()..self.columns {
            f(None)?;
        }
        Ok(())
    }
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
    let rect = Rect { grid: &g, top: r0, left: c0, rows: (r1 - r0).saturating_add(1), columns: columns.len() };
    let text = match render_within(format, columns, &rect, COPY_MAX_BYTES) {
        Ok(text) => text,
        Err(TooLarge::Cells { bytes }) => return Err(disabled(too_large(bytes))),
        Err(TooLarge::Text { rows }) => return Err(disabled(too_long(format, rows))),
    };
    let mut done = serde_json::Map::new();
    // Moved in, not copied: the text can be tens of megabytes.
    done.insert("text".into(), Json::String(text));
    done.insert("rows".into(), json!(rect.rows));
    done.insert("columns".into(), json!(columns.len()));
    Ok(Outcome::Done(Json::Object(done)))
}

const MIB: usize = 1024 * 1024;

/// Why a selection of `bytes` of cell text is not copied. The size is rounded up, so it never
/// reads as the cap itself.
fn too_large(bytes: usize) -> String {
    let tenths = bytes.saturating_mul(10).div_ceil(MIB);
    format!(
        "the selection holds {}.{} MiB of cell text, more than the {} MiB a copy can take; select fewer rows",
        tenths / 10,
        tenths % 10,
        COPY_MAX_BYTES / MIB
    )
}

/// Why a copy in `format` whose text passed the cap after `rows` whole rows is not copied.
fn too_long(format: CopyFormat, rows: usize) -> String {
    let verb = if rows == 1 { "fits" } else { "fit" };
    format!(
        "the copy as {} would pass the {} MiB a copy can take ({} of the selected rows {verb}); select fewer rows",
        format.label(),
        COPY_MAX_BYTES / MIB,
        thousands(rows)
    )
}

/// `12345` → `12,345`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `{}` → the focus cell's value view, or null.
fn show(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    Ok(Outcome::Done(match s.value_view_shared() {
        Some(v) => serde_json::to_value(&*v).unwrap_or(Json::Null),
        None => Json::Null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_too_long_says_how_many_rows_fit() {
        assert_eq!(
            too_long(CopyFormat::Json, 2_345),
            "the copy as JSON would pass the 8 MiB a copy can take (2,345 of the selected rows fit); select fewer rows"
        );
        assert!(
            too_long(CopyFormat::TsvWithHeader, 1).contains("as TSV with header would pass")
                && too_long(CopyFormat::Csv, 1).contains("(1 of the selected rows fits)")
        );
        assert_eq!(
            (thousands(0), thousands(999), thousands(1_000), thousands(1_234_567)),
            ("0".into(), "999".into(), "1,000".into(), "1,234,567".into())
        );
    }

    #[test]
    fn the_size_in_the_reason_never_reads_as_the_cap() {
        let why = too_large(COPY_MAX_BYTES + 1);
        assert_eq!(why, "the selection holds 8.1 MiB of cell text, more than the 8 MiB a copy can take; select fewer rows");
        assert!(too_large(40_000_123).contains("holds 38.2 MiB"));
        assert!(too_large(usize::MAX).contains("MiB"), "no overflow");
    }
}

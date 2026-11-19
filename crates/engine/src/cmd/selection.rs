//! The grid selection, copy and the value view: `selection.set`, `selection.rows`,
//! `selection.all`, `copy.selection` and `value.show`.
//!
//! They act on the active tab's grid: on a table tab the loaded page followed by its new rows, on
//! a query tab the shown result (see [`crate::view`]). A selection is the rectangle between an
//! anchor and a focus cell, `[row, column]`, 0-based. `selection.set` takes an anchor that must
//! lie in the grid (`BadParams` otherwise, and nothing changes) and clamps the focus to the edge,
//! so a drag past the last row selects to it. `selection.rows` selects whole rows (issue #17), as
//! a click on a row's number does: the rectangle across every column, marked `whole`, which keeps
//! it across every column the grid has ([`Selection::kept`], [`Grid::fit`]). The other commands
//! clear the mark. A selection that stops fitting (a new row undone, a shorter page) is dropped
//! when it is read.
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
use dbee_core::copy::{CopyFormat, TooLarge, render_within};
use serde_json::json;

use super::{CommandSpec, always, bad, disabled, p};
use crate::state::{Selection, Tab};
use crate::view::{Grid, Rect};
use crate::{EngineError, Json, Outcome, Session, TabId};

const SET: &str = "selection.set";
const ROWS: &str = "selection.rows";
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
        id: ROWS,
        label: "Select Rows",
        menu: &[],
        shortcut: None,
        params: "{first: row, last?: row = first} -> {anchor, focus, whole: true}",
        enabled: has_grid,
        run: rows,
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
        params: "{} -> the value view of the focus cell, or of the focus row when whole rows are selected, or null",
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

/// `{anchor, focus?}` → `{anchor, focus, whole: false}` as stored.
fn set(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let anchor = position(params, "anchor")?.ok_or_else(|| bad("`anchor` is required ([row, column])"))?;
    let focus = position(params, "focus")?.unwrap_or(anchor);
    let g = grid(s).ok_or_else(|| disabled(NO_GRID))?;
    let sel = g
        .fit(Selection { anchor, focus, whole: false })
        .ok_or_else(|| bad(format!("`anchor` [{}, {}] is outside the grid ({} rows, {} columns)", anchor.0, anchor.1, g.rows(), g.columns())))?;
    let tab = g.tab;
    store(s, tab, Some(sel));
    Ok(Outcome::Done(sel.to_json()))
}

/// `{first, last?}` → `{anchor, focus, whole: true}` as stored: rows `first` to `last` (0-based;
/// `last` past the end is the last row), every column.
fn rows(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let first = p::usize(params, "first")?;
    let last = p::opt_usize(params, "last")?.unwrap_or(first);
    let g = grid(s).ok_or_else(|| disabled(NO_GRID))?;
    let sel = g
        .fit(Selection { anchor: (first, 0), focus: (last, 0), whole: true })
        .ok_or_else(|| bad(format!("`first` {first} is outside the grid ({} rows, {} columns)", g.rows(), g.columns())))?;
    let tab = g.tab;
    store(s, tab, Some(sel));
    Ok(Outcome::Done(sel.to_json()))
}

/// `{}` → `{rows, columns}`: selects every cell.
fn all(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let g = grid(s).filter(|g| g.rows() > 0 && g.columns() > 0).ok_or_else(|| disabled(NO_GRID))?;
    let (rows, columns, tab) = (g.rows(), g.columns(), g.tab);
    let sel = Selection { anchor: (0, 0), focus: (rows.saturating_sub(1), columns.saturating_sub(1)), whole: false };
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
pub(crate) fn thousands(n: impl std::fmt::Display) -> String {
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

/// `{}` → the value view of the focus cell, or of the focus row for whole rows, or null.
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

    /// A session whose active tab is a query tab showing `results`, the last one: each of `rows`
    /// rows and `columns` text columns.
    fn showing(results: &[(usize, usize)]) -> Session {
        use std::sync::Arc;

        use dbee_core::{ResultSet, TypeClass};

        use crate::SessionOptions;
        use crate::state::{QueryTab, StatementOutcome};

        let mut s = Session::new(SessionOptions::default()).unwrap();
        let id = s.state.take_tab_id();
        let mut q = QueryTab::new(id, "q".into(), String::new());
        for &(rows, columns) in results {
            let col = |c: usize| Column { name: format!("c{c}"), base_type: "text".into(), type_label: "text".into(), class: TypeClass::LongText };
            let rs = ResultSet {
                columns: (0..columns).map(col).collect(),
                rows: (0..rows).map(|r| (0..columns).map(|c| Some(format!("{r}.{c}"))).collect()).collect(),
                rows_seen: rows as u64,
                complete: true,
                rows_affected: None,
                database_after: Default::default(),
            };
            q.results.push(StatementOutcome { range: 0..0, result: Arc::new(rs), ms: 0 });
        }
        q.shown = results.len().saturating_sub(1);
        s.state.tabs.push(Tab::Query(q));
        s.state.active = Some(id);
        s
    }

    fn done(s: &mut Session, id: &str, params: Json) -> Json {
        match s.execute(id, params) {
            Ok(Outcome::Done(v)) => v,
            other => panic!("{id}: {other:?}"),
        }
    }

    #[test]
    fn selection_rows_selects_whole_rows() {
        let mut s = showing(&[(4, 3)]);
        let one = json!({ "anchor": [1, 0], "focus": [1, 2], "whole": true });
        assert_eq!(done(&mut s, ROWS, json!({ "first": 1 })), one);
        assert_eq!(s.inspect()["selection"], one);
        // Several rows, either way round; a last row past the end is the grid's last.
        assert_eq!(done(&mut s, ROWS, json!({ "first": 2, "last": 0 })), json!({ "anchor": [2, 0], "focus": [0, 2], "whole": true }));
        assert_eq!(done(&mut s, ROWS, json!({ "first": 1, "last": 99 })), json!({ "anchor": [1, 0], "focus": [3, 2], "whole": true }));
        // The rows copy as the same rectangle selected by its cells does.
        let rows: Vec<Json> = ["tsv", "tsvHeader", "json"].into_iter().map(|f| done(&mut s, COPY, json!({ "format": f }))).collect();
        done(&mut s, SET, json!({ "anchor": [1, 0], "focus": [3, 2] }));
        let cells: Vec<Json> = ["tsv", "tsvHeader", "json"].into_iter().map(|f| done(&mut s, COPY, json!({ "format": f }))).collect();
        assert_eq!(rows, cells);
        // A first row outside the grid, or none, is refused and changes nothing.
        done(&mut s, ROWS, json!({ "first": 0 }));
        for p in [json!({ "first": 4 }), json!({}), json!({ "first": -1 }), json!({ "first": "1" }), json!({ "first": 0, "last": "x" })] {
            assert_eq!(s.execute(ROWS, p.clone()).unwrap_err().code(), "badParams", "{p}");
        }
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 0], "focus": [0, 2], "whole": true }));
    }

    #[test]
    fn selection_set_clears_whole() {
        let mut s = showing(&[(3, 3)]);
        done(&mut s, ROWS, json!({ "first": 1 }));
        // The same rectangle selected by its cells is no row selection.
        let cells = json!({ "anchor": [1, 0], "focus": [1, 2], "whole": false });
        assert_eq!(done(&mut s, SET, json!({ "anchor": [1, 0], "focus": [1, 2] })), cells);
        assert_eq!(s.inspect()["selection"], cells);
        done(&mut s, ROWS, json!({ "first": 1 }));
        done(&mut s, ALL, json!({}));
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [0, 0], "focus": [2, 2], "whole": false }));
    }

    /// A row selection is kept on another grid that still has its rows, and spans the columns that
    /// grid has; a cell selection is kept only where its cells still are.
    #[test]
    fn a_row_selection_spans_the_columns_of_the_grid_shown() {
        let mut s = showing(&[(4, 3), (3, 2)]);
        done(&mut s, ROWS, json!({ "first": 1, "last": 2 }));
        done(&mut s, "query.showResult", json!({ "index": 0 }));
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [1, 0], "focus": [2, 2], "whole": true }), "three columns");
        done(&mut s, "query.showResult", json!({ "index": 1 }));
        assert_eq!(s.inspect()["selection"], json!({ "anchor": [1, 0], "focus": [2, 1], "whole": true }), "two again");
        // Rows the other grid does not have: dropped, as a cell selection is.
        done(&mut s, ROWS, json!({ "first": 2 }));
        done(&mut s, "query.showResult", json!({ "index": 0 }));
        done(&mut s, ROWS, json!({ "first": 3 }));
        done(&mut s, "query.showResult", json!({ "index": 1 }));
        assert_eq!(s.inspect()["selection"], Json::Null);
        done(&mut s, "query.showResult", json!({ "index": 0 }));
        done(&mut s, SET, json!({ "anchor": [1, 2] }));
        done(&mut s, "query.showResult", json!({ "index": 1 }));
        assert_eq!(s.inspect()["selection"], Json::Null, "a cell in a column the grid lacks");
    }

    #[test]
    fn the_size_in_the_reason_never_reads_as_the_cap() {
        let why = too_large(COPY_MAX_BYTES + 1);
        assert_eq!(why, "the selection holds 8.1 MiB of cell text, more than the 8 MiB a copy can take; select fewer rows");
        assert!(too_large(40_000_123).contains("holds 38.2 MiB"));
        assert!(too_large(usize::MAX).contains("MiB"), "no overflow");
    }
}

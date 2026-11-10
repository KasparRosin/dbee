//! Staged edits of a table tab: `cell.edit`, `cell.setNull`, `cell.clear`, `row.add`,
//! `row.delete`, `edit.apply`, `edit.reset`, `edit.undo` and `edit.redo`.
//!
//! Every command acts on the active tab, which must be a table tab whose page has loaded
//! ([`NOT_LOADED`]). Edits wait while a page loads ([`LOADING`]) and while the tab's Apply runs
//! ([`APPLYING`]), so they always name rows of the page shown. Staging and Apply also need a
//! connection that is not lost and rows that can be edited (`table::not_editable`: not read-only,
//! a table with a row key). Reset, undo and redo do not: they change only the staged edits, so a
//! tab whose table was dropped, or that reconnected to a read-only server, can always drop them.
//! Rows are grid rows (see [`crate::edits`]); `column` is a name or a 0-based index into the
//! page's columns. A value is text or `null`; a number or a boolean is taken as its text, except
//! that a boolean in a Bool column takes the connection's own spelling (its dialect's
//! `bool_literals`: Postgres `t` and `f`).
//!
//! **Apply** builds every statement on the caller's thread (`Edits::statements`; edits it cannot
//! build are `BadParams` and change nothing), then runs one job on the browse connection: its
//! `apply` writes them all or none in one transaction, and on success the page loads
//! again in the same job. Then the edits and their history go (writes are not undone), the page
//! shows what was written, and an exact total that the writes may have changed is forgotten. When
//! the Apply fails nothing was written: every edit stays, and `failed` names the grid row of the
//! change that failed (its column when that change wrote one), the message and the error's code
//! (`inspect()` shows `{row, column, message, code}`). When the
//! connection is lost meanwhile, the outcome is unknown ([`UNKNOWN_OUTCOME`]): the edits stay,
//! the session is marked lost, and nothing is tried again. Applying those edits again could write
//! them twice, so it asks first (`Needs::Confirm { what: "applyAfterUnknownOutcome" }`) unless
//! `force` (see `Edits::outcome_unknown`). An Apply is never dropped: its job runs to its end. One
//! runs at a time (`busy`). `table.cancel` on its tab cancels it while its statements before
//! `COMMIT` run (`dbee_driver::ApplyGate`): it then fails with code `cancelled`, writes nothing and
//! keeps every edit, as a failed Apply does. Once it has decided to commit, `table.cancel` is
//! `Busy` ([`COMMITTING`]) and the Apply runs to its end.
//!
//! **Stale results** (see [`super`]): an Apply whose tab closed, or whose connection was closed or
//! replaced, is ignored. A reconnect while it runs marks its outcome unknown
//! (`TableTab::drop_dead_jobs`), and so does a job that ended without its result applied
//! (`TableTab::job_ended`).
//!
//! None of these commands is journaled: the journal never holds cell values, and a replay never
//! writes edits it could repeat only in part. Logs carry command ids and job labels, never SQL or
//! values.

use std::sync::Arc;

use dbee_core::{ResultSet, TypeClass};
use dbee_driver::{ApplyError, Connection, DbError};
use dbee_sql::Dialect;
use dbee_sql::edit::NewValue;
use dbee_sql::filter::Condition;
use serde_json::json;

use super::table::{Loaded, NO_TABLE_TAB, Reload, active_table, catalog, lookup, not_editable, table_tab_mut};
use super::{CommandSpec, bad, disabled, ignored, p};
use crate::edits::{Edits, Failed, UNKNOWN_OUTCOME};
use crate::jobs::Stop;
use crate::state::{TableTab, Total};
use crate::turns;
use crate::{Apply, ConnectionId, EngineError, Json, Need, Outcome, Session, Tab, TabId, Which};

const EDIT: &str = "cell.edit";
const SET_NULL: &str = "cell.setNull";

/// Why a value with a NUL character is refused where the dialect cannot write one.
const NUL: &str = "the value holds a NUL character (U+0000), which this database can't take";
const CLEAR: &str = "cell.clear";
const ADD_ROW: &str = "row.add";
const DELETE_ROWS: &str = "row.delete";
/// Its jobs' label too: `table.cancel` checks for one.
pub(crate) const APPLY: &str = "edit.apply";
const RESET: &str = "edit.reset";
const UNDO: &str = "edit.undo";
const REDO: &str = "edit.redo";

/// Why edits wait while the tab's Apply runs; also the `busy` text of a second Apply.
pub const APPLYING: &str = "apply in progress";

/// The `busy` text of `table.cancel` once the tab's Apply has decided to commit.
pub const COMMITTING: &str = "the changes are being committed";

/// Why edits wait while a page loads.
pub const LOADING: &str = "wait for the page to load";

/// Why a tab whose first page did not load takes no edits.
pub const NOT_LOADED: &str = "the page has not loaded";

/// Why Apply and Reset cannot run with nothing staged.
pub const NOTHING_STAGED: &str = "no staged changes";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: EDIT,
        label: "Edit Cell",
        menu: &[],
        shortcut: None,
        params: "{row, column: name | index, value: string | null}",
        enabled: can_edit,
        run: edit,
        journal: false,
        undoable: true,
    },
    CommandSpec {
        id: SET_NULL,
        label: "Set as NULL",
        menu: &[],
        shortcut: None,
        params: "{row, column: name | index}",
        enabled: can_edit,
        run: set_null,
        journal: false,
        undoable: true,
    },
    CommandSpec {
        id: CLEAR,
        label: "Clear Cell",
        menu: &[],
        shortcut: None,
        params: "{row, column: name | index}",
        enabled: can_edit,
        run: clear,
        journal: false,
        undoable: true,
    },
    CommandSpec {
        id: ADD_ROW,
        label: "Add Row",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: can_edit,
        run: add_row,
        journal: false,
        undoable: true,
    },
    CommandSpec {
        id: DELETE_ROWS,
        label: "Delete Row",
        menu: &[],
        shortcut: None,
        params: "{rows: [row, ..]}",
        enabled: can_edit,
        run: delete_rows,
        journal: false,
        undoable: true,
    },
    // The Edit menu lists them in this order: Undo, Redo, then Apply, Reset.
    CommandSpec {
        id: UNDO,
        label: "Undo",
        menu: &["Edit"],
        shortcut: Some("Ctrl+Z"),
        params: "{}",
        enabled: can_undo,
        run: undo,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: REDO,
        label: "Redo",
        menu: &["Edit"],
        shortcut: Some("Ctrl+Shift+Z"),
        params: "{}",
        enabled: can_redo,
        run: redo,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: APPLY,
        label: "Apply",
        menu: &["Edit"],
        shortcut: Some("Ctrl+S"),
        params: "{force?}",
        enabled: can_apply,
        run: apply,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: RESET,
        label: "Reset",
        menu: &["Edit"],
        shortcut: None,
        params: "{}",
        enabled: can_reset,
        run: reset,
        journal: false,
        undoable: true,
    },
];

// ---- enabled checks --------------------------------------------------------------------------

/// The active tab is a table tab on a connection that is not lost, editable, with its page loaded
/// and no page on its way.
fn editable_tab(s: &Session) -> Result<&TableTab, String> {
    active_table(s)?;
    let Some(Tab::Table(t)) = s.state.active_tab() else { return Err(NO_TABLE_TAB.into()) };
    if let Some(why) = not_editable(s, t) {
        return Err(why.into_owned());
    }
    if t.loading.is_some() {
        return Err(LOADING.into());
    }
    if t.data.is_none() {
        return Err(NOT_LOADED.into());
    }
    Ok(t)
}

/// [`editable_tab`] with no Apply running: for the commands that change the edits.
pub fn can_edit(s: &Session) -> Result<(), String> {
    let t = editable_tab(s)?;
    if t.applying.is_some() { Err(APPLYING.into()) } else { Ok(()) }
}

/// [`editable_tab`] with something staged. A running Apply is the command's `busy`.
fn can_apply(s: &Session) -> Result<(), String> {
    if editable_tab(s)?.has_staged_edits() { Ok(()) } else { Err(NOTHING_STAGED.into()) }
}

/// The active tab is a table tab with its page loaded, no page on its way and no Apply running:
/// for reset, undo and redo, which change only the staged edits. They need neither the database
/// nor rows that can be edited, so a tab is never stuck with edits it cannot drop.
fn history_tab(s: &Session) -> Result<&TableTab, String> {
    let Some(Tab::Table(t)) = s.state.active_tab() else { return Err(NO_TABLE_TAB.into()) };
    if t.loading.is_some() {
        return Err(LOADING.into());
    }
    if t.data.is_none() {
        return Err(NOT_LOADED.into());
    }
    if t.applying.is_some() {
        return Err(APPLYING.into());
    }
    Ok(t)
}

fn can_reset(s: &Session) -> Result<(), String> {
    if history_tab(s)?.has_staged_edits() { Ok(()) } else { Err(NOTHING_STAGED.into()) }
}

fn can_undo(s: &Session) -> Result<(), String> {
    if history_tab(s)?.edits.can_undo() { Ok(()) } else { Err("nothing to undo".into()) }
}

fn can_redo(s: &Session) -> Result<(), String> {
    if history_tab(s)?.edits.can_redo() { Ok(()) } else { Err("nothing to redo".into()) }
}

// ---- helpers ---------------------------------------------------------------------------------

/// The active table tab and its page.
fn target(s: &Session) -> Result<(TabId, Arc<ResultSet>), EngineError> {
    match s.state.active_tab() {
        Some(Tab::Table(t)) => t.data.clone().map(|d| (t.id, d)).ok_or_else(|| disabled(NOT_LOADED)),
        _ => Err(disabled(NO_TABLE_TAB)),
    }
}

fn edits_mut(s: &mut Session, tab: TabId) -> Result<&mut Edits, EngineError> {
    table_tab_mut(s, tab).map(|t| Arc::make_mut(&mut t.edits)).ok_or_else(|| disabled(NO_TABLE_TAB))
}

/// `column`: a name or a 0-based index into the page's columns.
fn column(params: &Json, data: &ResultSet) -> Result<usize, EngineError> {
    match params.get("column") {
        Some(Json::String(name)) => data.columns.iter().position(|c| c.name == *name).ok_or_else(|| bad("the page has no such `column`")),
        Some(Json::Number(n)) => n
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i < data.columns.len())
            .ok_or_else(|| bad("`column` is not a column of the page")),
        _ => Err(bad("`column` is required (a name or a 0-based index)")),
    }
}

/// `value` for a cell of class `class`: text, or `null` for NULL; a number or a boolean is taken
/// as its text, but a boolean for a Bool column as `dialect`'s spelling of it. Text with a NUL
/// character only where `dialect` can write one.
fn value(params: &Json, class: TypeClass, dialect: Dialect) -> Result<NewValue, EngineError> {
    match params.get("value") {
        None => Err(bad("`value` is required (text, or null for NULL)")),
        Some(Json::Null) => Ok(NewValue::Null),
        Some(Json::String(t)) if t.contains('\0') && !dialect.statement_may_hold_nul() => Err(bad(NUL)),
        Some(Json::String(t)) => Ok(NewValue::Text(t.clone())),
        Some(Json::Bool(b)) if class == TypeClass::Bool => {
            let (yes, no) = dialect.bool_literals();
            Ok(NewValue::Text(if *b { yes } else { no }.to_owned()))
        }
        Some(v @ (Json::Number(_) | Json::Bool(_))) => Ok(NewValue::Text(v.to_string())),
        Some(_) => Err(bad("`value` must be text or null")),
    }
}

/// The edits as `inspect()` shows them: `{count, cells: [{row, column, value}], newRows: [{row,
/// values: [{column, value}]}], deleted, canUndo, canRedo}`, rows in grid numbers (`loaded` is
/// the page's row count), values as text or `null`.
pub(crate) fn summary(e: &Edits, loaded: usize) -> Json {
    let value = |v: &NewValue| match v {
        NewValue::Null => Json::Null,
        NewValue::Text(t) => json!(t),
    };
    let cells: Vec<Json> = e.cells.iter().map(|(&(row, column), v)| json!({ "row": row, "column": column, "value": value(v) })).collect();
    let new_rows: Vec<Json> = e
        .new_rows
        .iter()
        .enumerate()
        .map(|(i, values)| {
            let values: Vec<Json> = values.iter().map(|(column, v)| json!({ "column": column, "value": value(v) })).collect();
            json!({ "row": loaded.saturating_add(i), "values": values })
        })
        .collect();
    json!({
        "count": e.count(),
        "cells": cells,
        "newRows": new_rows,
        "deleted": e.deleted,
        "canUndo": e.can_undo(),
        "canRedo": e.can_redo(),
    })
}

// ---- the commands ----------------------------------------------------------------------------

/// What a cell command stages.
enum Staging {
    /// The `value` param.
    Value,
    Null,
    /// `''` for a text column, NULL for every other.
    Clear,
}

/// `{row, column, ..}` → `{tab, row, column, staged, count}`: stages the cell (`staged` is false
/// when the value is the loaded one, which unstages it).
fn stage(s: &mut Session, params: &Json, what: Staging) -> Result<Outcome, EngineError> {
    let row = p::usize(params, "row")?;
    let (tab, data) = target(s)?;
    let col = column(params, &data)?;
    let catalog = catalog(s, Which::Tab(tab))?;
    let table = match s.state.tab(tab) {
        Some(Tab::Table(t)) => t.table.clone(),
        _ => return Err(disabled(NO_TABLE_TAB)),
    };
    let (name, class) = data.columns.get(col).map_or(("", TypeClass::Other), |c| (c.name.as_str(), c.class));
    // The cell's column in the catalog: what clearing stages, and whether the engine computes it.
    let info = lookup(&catalog, &table).ok().and_then(|t| t.column(name));
    let value = match what {
        Staging::Value => value(params, class, s.state.dialect(Which::Tab(tab)))?,
        Staging::Null => NewValue::Null,
        Staging::Clear => {
            let info = info.ok_or_else(|| bad("the column is no longer in the schema (refresh it)"))?;
            if info.clears_to_null() { NewValue::Null } else { NewValue::Text(String::new()) }
        }
    };
    let e = edits_mut(s, tab)?;
    e.stage(&data, row, col, value, info).map_err(bad)?;
    let staged = e.shown(&data, row, col).is_some();
    Ok(Outcome::Done(json!({ "tab": tab, "row": row, "column": col, "staged": staged, "count": e.count() })))
}

fn edit(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    stage(s, params, Staging::Value)
}

fn set_null(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    stage(s, params, Staging::Null)
}

fn clear(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    stage(s, params, Staging::Clear)
}

/// `{}` → `{tab, row, count}`: a new empty row after the others; `row` is its grid row.
fn add_row(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let (tab, data) = target(s)?;
    let e = edits_mut(s, tab)?;
    let row = e.add_row(&data);
    Ok(Outcome::Done(json!({ "tab": tab, "row": row, "count": e.count() })))
}

/// `{rows}` → `{tab, count}`: new rows go (the new rows after them move up), loaded rows are
/// marked for deletion. A row outside the grid is `BadParams`, and then nothing changes.
fn delete_rows(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let items = params.get("rows").and_then(Json::as_array).ok_or_else(|| bad("`rows` is required (a list of grid rows)"))?;
    let rows = items
        .iter()
        .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("`rows` must hold whole numbers, 0 or more")))
        .collect::<Result<Vec<usize>, EngineError>>()?;
    let (tab, data) = target(s)?;
    let e = edits_mut(s, tab)?;
    e.delete_rows(&data, &rows).map_err(bad)?;
    Ok(Outcome::Done(json!({ "tab": tab, "count": e.count() })))
}

/// `{}` → `{tab, count, canUndo, canRedo}` after `f` changed the edits.
fn history(s: &mut Session, f: fn(&mut Edits) -> bool) -> Result<Outcome, EngineError> {
    let (tab, _) = target(s)?;
    let e = edits_mut(s, tab)?;
    f(e);
    Ok(Outcome::Done(json!({ "tab": tab, "count": e.count(), "canUndo": e.can_undo(), "canRedo": e.can_redo() })))
}

/// Drops every staged change; it can be undone.
fn reset(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    history(s, Edits::reset)
}

fn undo(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    history(s, Edits::undo)
}

fn redo(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    history(s, Edits::redo)
}

/// What the Apply's result needs of what was staged.
struct Applying {
    /// Each statement's grid row and its one column, when it writes one.
    statements: Vec<(usize, Option<usize>)>,
    /// Rows were added or deleted, so an exact count no longer holds.
    reshaped: bool,
}

/// `{force?}` → a job returning `{tab, written, page, rows, hasNext, total}` (see the module
/// docs). After an unknown outcome it asks first unless `force`.
fn apply(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let force = p::bool_or(params, "force", false)?;
    if s.jobs.is_running_label(APPLY) {
        return Err(EngineError::Busy(APPLYING.into()));
    }
    let (tab, table, edits, data, hidden) = match s.state.active_tab() {
        Some(Tab::Table(t)) if t.applying.is_some() => return Err(EngineError::Busy(APPLYING.into())),
        Some(Tab::Table(t)) => {
            (t.id, t.table.clone(), Arc::clone(&t.edits), t.data.clone().ok_or_else(|| disabled(NOT_LOADED))?, Arc::clone(&t.hidden))
        }
        _ => return Err(disabled(NO_TABLE_TAB)),
    };
    let catalog = catalog(s, Which::Tab(tab))?;
    let info = lookup(&catalog, &table)?;
    let statements = edits.statements(s.state.dialect(Which::Tab(tab)), info, &data, &hidden).map_err(|e| bad(e.to_string()))?;
    if statements.is_empty() {
        return Err(disabled(NOTHING_STAGED));
    }
    if edits.outcome_unknown() && !force {
        return Err(EngineError::Needs { need: Need::Confirm { what: "applyAfterUnknownOutcome".into() }, message: UNKNOWN_OUTCOME.into() });
    }
    let loaded = data.rows.len();
    let applying = Applying {
        statements: statements.iter().map(|&(row, _)| (row, edits.only_column(loaded, row))).collect(),
        reshaped: !edits.new_rows.is_empty() || !edits.deleted.is_empty(),
    };
    let sql: Vec<String> = statements.into_iter().map(|(_, sql)| sql).collect();
    let (id, job_conn) = s.browse_in_turn(Which::Tab(tab))?;
    let conn: Arc<dyn Connection> = job_conn.clone();
    let reload = Reload::prepare(s, tab, info)?;
    if let Some(t) = table_tab_mut(s, tab)
        && (t.edits.failed.is_some() || t.edits.outcome_unknown())
    {
        Arc::make_mut(&mut t.edits).apply_started();
    }
    let gate = Arc::new(dbee_driver::ApplyGate::default());
    let job_gate = Arc::clone(&gate);
    let stop = Arc::new(Stop::default());
    let job_stop = Arc::clone(&stop);
    let job = s.spawn_job(APPLY, async move {
        let written = turns::apply_in_turn(&job_conn, &sql, &job_gate, &job_stop).await;
        let page = match written {
            Ok(_) => Some(reload.run(job_conn.as_ref()).await),
            Err(_) => None,
        };
        let apply: Apply = Box::new(move |s: &mut Session| applied(s, id, &conn, tab, applying, reload, written, page));
        apply
    });
    if let Some(t) = table_tab_mut(s, tab) {
        t.applying = Some(job);
    }
    if let Some(c) = s.state.connection_mut(Which::Id(id)) {
        c.apply_gate = Some((job, gate, stop));
    }
    Ok(Outcome::Started(job))
}

fn applied(
    s: &mut Session,
    id: ConnectionId,
    conn: &Arc<dyn Connection>,
    tab: TabId,
    applying: Applying,
    reload: Reload,
    written: Result<u64, ApplyError>,
    page: Option<Loaded>,
) -> Result<Json, EngineError> {
    let me = s.applying_job();
    if let Some(c) = s.state.connection_mut(Which::Id(id))
        && c.apply_gate.as_ref().is_some_and(|(job, _, _)| Some(*job) == me)
    {
        c.apply_gate = None;
    }
    if !s.is_current_browse(id, conn) {
        return Ok(ignored());
    }
    let Some(t) = table_tab_mut(s, tab) else { return Ok(ignored()) };
    if me.is_none() || t.applying != me {
        return Ok(ignored());
    }
    t.applying = None;
    match written {
        Ok(n) => {
            // Written: the edits and their history go, and a count the writes may have changed.
            t.edits = Arc::default();
            if matches!(t.total, Total::Exact(_)) && (applying.reshaped || t.applied != Condition::None) {
                t.total = Total::Unknown;
            }
            let shown = match page {
                Some(page) => reload.finish(s, id, conn, tab, page),
                None => Err(bad("the page was not loaded again")),
            };
            Ok(match shown {
                Ok(r) => with_written(r, n),
                Err(e) => {
                    // The changes are in the database, but the rows shown are from before them:
                    // the tab's error (its card) says both.
                    if let Some(t) = table_tab_mut(s, tab) {
                        t.error = Some(written_not_shown(n, &e));
                    }
                    json!({ "tab": tab, "written": n, "reloadError": e.to_string() })
                }
            })
        }
        Err(ApplyError { index, error }) => {
            let lost = error == DbError::ConnectionLost;
            let (row, column) = applying.statements.get(index).or(applying.statements.first()).copied().unwrap_or((0, None));
            let edits = Arc::make_mut(&mut t.edits);
            let message = if lost {
                edits.unknown_at(row);
                UNKNOWN_OUTCOME.to_owned()
            } else {
                let message = ApplyError { index, error: error.clone() }.to_string();
                edits.failed = Some(Failed { row, column, message: message.clone(), code: error.code() });
                message
            };
            let column = if lost { None } else { column };
            Err(match s.db_failed(id, conn, &error) {
                EngineError::Db { code, mut data, .. } => {
                    if let Json::Object(map) = &mut data {
                        map.insert("row".into(), json!(row));
                        map.insert("column".into(), json!(column));
                        map.insert("statement".into(), json!(index));
                    }
                    EngineError::Db { code, message, data }
                }
                other => other,
            })
        }
    }
}

/// The tab's error when the Apply wrote `n` rows and the page then did not load again with `e`.
fn written_not_shown(n: u64, e: &EngineError) -> String {
    let rows = if n == 1 { "1 row".to_owned() } else { format!("{n} rows") };
    format!("The changes were written ({rows}), but the page did not load again: {e}")
}

/// `r` with `written: n` when it is an object (the page result always is).
fn with_written(mut r: Json, n: u64) -> Json {
    if let Json::Object(map) = &mut r {
        map.insert("written".into(), json!(n));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_but_not_shown_says_both() {
        let e = EngineError::Db { code: "server", message: "division by zero (SQLSTATE 22012)".into(), data: json!({}) };
        assert_eq!(written_not_shown(1, &e), "The changes were written (1 row), but the page did not load again: division by zero (SQLSTATE 22012)");
        assert!(written_not_shown(3, &e).starts_with("The changes were written (3 rows)"));
    }

    #[test]
    fn written_goes_into_objects_only() {
        assert_eq!(with_written(json!({ "tab": 1 }), 2), json!({ "tab": 1, "written": 2 }));
        for other in [Json::Null, json!([1]), json!("x"), json!(3)] {
            assert_eq!(with_written(other.clone(), 2), other);
        }
    }
}

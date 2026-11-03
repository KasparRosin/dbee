//! Table tabs: open, the filter bar, paging, sort, the row count, cancel, refresh, read, and
//! following a foreign key.
//!
//! Every command needs a connected session that is not lost. `table.open` opens a new tab; the
//! others act on the active tab, which must be a table tab. Applying filters, paging, sort and
//! refresh wait while edits are staged ([`STAGED`], asked of `TableTab::has_staged_edits`), since
//! the edits name rows of the page shown; typing in the filter bar (`table.setFilters` with
//! `apply: false`) does not. A page never lands on staged edits: one that would is ignored.
//!
//! **Loading a page** is a job on the browse connection. The command builds the statement first,
//! in the connection's dialect (SQL that cannot be built, like an unknown column or a `;` in the
//! raw condition, is `BadParams` and changes nothing), then sets the tab's `applied`, `sort` and
//! `page` to what it asked for, bumps the tab's `seq` and starts the job. The page statement asks
//! for 101 rows: 100 are kept, and the 101st sets `has_next`. When the page's cell text passes the
//! byte cap (`PAGE_LIMITS`, 256 MiB) fewer rows are kept and `page_complete` says so
//! (`pageComplete` in `table.read` and `inspect()`). Columns are typed from the catalog; the
//! page shows the visible ones and keeps the row key's hidden ones beside them (`hidden`). A load
//! that fails sets `error` (the server's text) and puts `applied`, `sort` and `page` back to what
//! the rows still shown were loaded with; the job's result is the error. `table.page {to: "last"}`
//! counts first, in the same job. `table.read` and `inspect()` give the rows with the page they
//! are (`page`, from `shown`) and the page asked for (`requestedPage`).
//!
//! **Stale results** (see [`super`]): a page whose tab closed, whose `seq` is no longer the
//! tab's, or whose connection was closed or replaced is `{"ignored": true}` and changes nothing.
//! A count is stale when its tab closed, its connection went, or the tab's `applied` condition
//! is no longer the one counted: paging and sorting do not change a count, so a count that
//! `last` ran is kept even when a newer page load made its page stale.
//!
//! **In-flight markers.** `loading` and `counting` name the tab's jobs. A job clears the marker
//! that names it when its result arrives, stale or not (clearing the marker of a job that has
//! ended is not applying its result); when its future or its `Apply` panicked, the session
//! clears it once the job has ended (`TableTab::job_ended`), so no marker outlives its job; and a
//! reconnect clears them all, since every job started before it ran on the old connection
//! (`TableTab::drop_dead_jobs`). So a marker names a running job on the current browse
//! connection, which is what `table.cancel` relies on. A count that fails leaves the estimate it
//! showed, unless a newer count runs.
//!
//! **The total** is counted only when asked. `table.count` shows the planner's estimate at once
//! for an unfiltered table, materialized view or partitioned table (`reltuples` from the
//! catalog; views and foreign tables have none) while the exact count runs. A new condition
//! forgets the total.
//!
//! Logs carry the command ids and job labels only (the job runtime times them), never SQL,
//! filter values or cells.

use std::borrow::Cow;
use std::sync::Arc;

use dbee_core::{Catalog, Column, ColumnInfo, RelKind, ResultSet, TableInfo, TableRef};
use dbee_driver::{Connection, DbError, PAGE_LIMITS};
use dbee_sql::Dialect;
use dbee_sql::SqlError;
use dbee_sql::filter::{Condition, Filter, Op, Sort, SortDir, where_sql};
use dbee_sql::select::{PAGE_SIZE, count_sql, page_sql};
use serde_json::json;

use super::connection::cancel_statements;
use super::edit;
use super::{CommandSpec, NOT_CONNECTED, bad, disabled, ignored, needs_db, p};
use crate::state::{PageView, TableTab, Total};
use crate::{Apply, EngineError, JobId, Json, Outcome, Session, Tab, TabId};

const OPEN: &str = "table.open";
const SET_FILTERS: &str = "table.setFilters";
const PAGE: &str = "table.page";
const SORT: &str = "table.sort";
const COUNT: &str = "table.count";
const CANCEL: &str = "table.cancel";
const REFRESH: &str = "table.refresh";
const READ: &str = "table.read";
const FOLLOW: &str = "table.followForeignKey";

/// Why filters, paging, sort and refresh cannot run while edits are staged.
pub const STAGED: &str = "apply or reset your changes first";

/// Why a table command cannot run when the active tab is not a table tab.
pub const NO_TABLE_TAB: &str = "no table tab is active";

/// Rows a page keeps.
const PAGE_ROWS: usize = PAGE_SIZE as usize;

/// The last page dbee can reach: its `OFFSET` fits a signed 64-bit integer, which every engine
/// takes.
const MAX_PAGE: u64 = i64::MAX as u64 / PAGE_SIZE;

/// The rows `table.read` returns when `limit` is not given.
const READ_LIMIT: usize = 100;

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: OPEN,
        label: "Open Table",
        menu: &[],
        shortcut: None,
        params: "{schema, table, filters?: [{column, op, value}]}",
        enabled: needs_db,
        run: open,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: SET_FILTERS,
        label: "Apply Filters",
        menu: &[],
        shortcut: None,
        params: "{filters?: [{column, op, value}], raw?, rawMode?, apply? = true}",
        enabled: active_table,
        run: set_filters,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: PAGE,
        label: "Go to Page",
        menu: &[],
        shortcut: None,
        params: "{to: \"first\" | \"prev\" | \"next\" | \"last\" | <page number from 1>}",
        enabled: table_view,
        run: page,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: SORT,
        label: "Sort by Column",
        menu: &[],
        shortcut: None,
        params: "{column, dir?: \"asc\" | \"desc\" | \"none\" (default: the next of asc → desc → none)}",
        enabled: table_view,
        run: sort,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: COUNT,
        label: "Count Rows",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: active_table,
        run: count,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: CANCEL,
        label: "Cancel Loading",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: active_table,
        run: cancel,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: REFRESH,
        label: "Refresh Table",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: table_view,
        run: refresh,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: READ,
        label: "Read Table Rows",
        menu: &[],
        shortcut: None,
        params: "{offset? = 0, limit? = 100}",
        enabled: active_table,
        run: read,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: FOLLOW,
        label: "Follow Foreign Key",
        menu: &[],
        shortcut: None,
        params: "{row, column: name | index}",
        enabled: active_table,
        run: follow,
        journal: true,
        undoable: false,
    },
];

// ---- enabled checks --------------------------------------------------------------------------

/// The active tab is a table tab, on a connection that is not lost.
pub fn active_table(s: &Session) -> Result<(), String> {
    needs_db(s)?;
    match s.state.active_tab() {
        Some(Tab::Table(_)) => Ok(()),
        _ => Err(NO_TABLE_TAB.into()),
    }
}

/// [`active_table`] with no staged edits: for the commands that load another page.
pub fn table_view(s: &Session) -> Result<(), String> {
    active_table(s)?;
    match s.state.active_tab() {
        Some(Tab::Table(t)) if t.has_staged_edits() => Err(STAGED.into()),
        _ => Ok(()),
    }
}

// ---- helpers ---------------------------------------------------------------------------------

/// SQL dbee would not build from the params.
fn sql_bad(e: SqlError) -> EngineError {
    bad(e.to_string())
}

pub(crate) fn catalog(s: &Session) -> Result<Arc<Catalog>, EngineError> {
    s.state.connected.as_ref().map(|c| Arc::clone(&c.catalog)).ok_or_else(|| disabled(NOT_CONNECTED))
}

pub(crate) fn lookup<'a>(catalog: &'a Catalog, table: &TableRef) -> Result<&'a TableInfo, EngineError> {
    catalog.table(table).ok_or_else(|| bad(format!("the schema has no table `{}` (refresh the schema if it is new)", table.display())))
}

/// The active table tab.
fn active(s: &Session) -> Result<&TableTab, EngineError> {
    match s.state.active_tab() {
        Some(Tab::Table(t)) => Ok(t),
        _ => Err(disabled(NO_TABLE_TAB)),
    }
}

pub(crate) fn table_tab_mut(s: &mut Session, id: TabId) -> Option<&mut TableTab> {
    match s.state.tab_mut(id) {
        Some(Tab::Table(t)) => Some(t),
        _ => None,
    }
}

/// The condition `filters` (or `raw`) make; none when nothing is in it.
fn condition(filters: &[Filter], raw: &str, raw_mode: bool) -> Condition {
    if raw_mode {
        if raw.trim().is_empty() { Condition::None } else { Condition::Raw(raw.to_owned()) }
    } else if filters.is_empty() {
        Condition::None
    } else {
        Condition::Filters(filters.to_vec())
    }
}

/// `[{column, op, value}]`. `value` is text; a number or a boolean is taken as its text, and a
/// missing or `null` value is empty (an empty value leaves the row out of the condition).
fn parse_filters(v: &Json) -> Result<Vec<Filter>, EngineError> {
    let items = v.as_array().ok_or_else(|| bad("`filters` must be a list of {column, op, value}"))?;
    items.iter().enumerate().map(|(i, f)| parse_filter(i, f)).collect()
}

fn parse_filter(i: usize, f: &Json) -> Result<Filter, EngineError> {
    let at = |what: &str| bad(format!("`filters[{i}]` {what}"));
    let obj = f.as_object().ok_or_else(|| at("must be an object {column, op, value}"))?;
    let column = obj.get("column").and_then(Json::as_str).ok_or_else(|| at("needs `column` (a string)"))?;
    let op = obj.get("op").and_then(Json::as_str).and_then(Op::from_id).ok_or_else(|| {
        let ids: Vec<&str> = Op::ALL.iter().map(|op| op.id()).collect();
        at(&format!("needs `op`, one of {}", ids.join(", ")))
    })?;
    let value = match obj.get("value") {
        None | Some(Json::Null) => String::new(),
        Some(Json::String(v)) => v.clone(),
        Some(v @ (Json::Number(_) | Json::Bool(_))) => v.to_string(),
        Some(_) => return Err(at("has a `value` that is not text")),
    };
    Ok(Filter { column: column.to_owned(), op, value })
}

fn opt_bool(params: &Json, key: &str) -> Result<Option<bool>, EngineError> {
    match params.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| bad(format!("`{key}` must be true or false"))),
    }
}

/// The planner's estimate for `cond` on `info` in `dialect`, when there is one to show at once:
/// only when the condition restricts nothing (none, or only filter rows without a value), and
/// only for relations that keep one (not views or foreign tables). The UI shows it by the same
/// rule.
pub fn quick_estimate(dialect: Dialect, info: &TableInfo, cond: &Condition) -> Option<u64> {
    let kept = matches!(info.kind, RelKind::Table | RelKind::PartitionedTable | RelKind::MaterializedView);
    let unfiltered = matches!(where_sql(dialect, cond, info), Ok(None));
    if kept && unfiltered { info.estimated_rows } else { None }
}

/// The 0-based last page of `rows` rows.
fn last_page(rows: u64) -> u64 {
    rows.saturating_sub(1) / PAGE_SIZE
}

/// Why the tab's rows cannot be edited, or `None` when they can (`notEditableReason`): the
/// connection, the relation's kind, the table's own reason (`noEditReason`), or its key.
pub fn not_editable<'a>(s: &'a Session, t: &TableTab) -> Option<Cow<'a, str>> {
    let Some(c) = s.state.connected.as_ref() else { return Some(NOT_CONNECTED.into()) };
    if c.info.read_only || c.config.read_only {
        return Some("read-only connection".into());
    }
    let Some(info) = c.catalog.table(&t.table) else { return Some("the table is no longer in the schema".into()) };
    match info.kind {
        RelKind::Table | RelKind::PartitionedTable => {}
        RelKind::View => return Some("views are read-only".into()),
        RelKind::MaterializedView => return Some("materialized views are read-only".into()),
        RelKind::ForeignTable => return Some("foreign tables are read-only".into()),
    }
    if let Some(why) = &info.no_edit_reason {
        return Some(why.as_str().into());
    }
    if info.row_key().is_none() {
        return Some("no primary key or unique not-null key".into());
    }
    None
}

/// The table's own reason it cannot be edited, whatever the connection (`noEditReason`).
fn no_edit_reason<'a>(s: &'a Session, t: &TableTab) -> Option<&'a str> {
    s.state.connected.as_ref()?.catalog.table(&t.table)?.no_edit_reason.as_deref()
}

/// The tab as `inspect()` shows it.
pub(crate) fn summary(s: &Session, t: &TableTab) -> Json {
    let reason = not_editable(s, t);
    json!({
        "tab": t.id,
        "title": t.table.display(),
        "schema": t.table.schema,
        "name": t.table.name,
        "filters": t.filters,
        "raw": t.raw,
        "rawMode": t.raw_mode,
        "applied": t.applied,
        "sort": t.sort,
        "page": t.shown.page,
        "requestedPage": t.page,
        "rows": t.data.as_ref().map_or(0, |d| d.rows.len()),
        "hasNext": t.has_next,
        "pageComplete": t.page_complete,
        "total": t.total,
        "loading": t.loading,
        "counting": t.counting,
        "error": t.error,
        "editable": reason.is_none(),
        "notEditableReason": reason,
        "noEditReason": no_edit_reason(s, t),
        "edits": super::edit::summary(&t.edits, t.data.as_ref().map_or(0, |d| d.rows.len())),
        "failed": t.edits.failed.as_ref().map(|f| json!({ "row": f.row, "column": f.column, "message": f.message, "code": f.code })),
        "outcomeUnknown": t.edits.outcome_unknown(),
        "applying": t.applying,
    })
}

// ---- loading a page --------------------------------------------------------------------------

/// A page load, checked and ready to start.
struct Plan {
    conn: Arc<dyn Connection>,
    dialect: Dialect,
    info: TableInfo,
    view: PageView,
    /// Count first and load the last page (`view.page` is then not used).
    last: bool,
}

/// Checks that the statements for `view` can be built in `dialect`: `BadParams` when not.
fn check(dialect: Dialect, info: &TableInfo, view: &PageView, last: bool) -> Result<(), EngineError> {
    page_sql(dialect, info, &view.applied, view.sort.as_ref(), view.page).map_err(sql_bad)?;
    if last {
        count_sql(dialect, info, &view.applied).map_err(sql_bad)?;
    }
    Ok(())
}

/// [`check`], then takes the browse connection. Changes nothing but a lost mark.
fn prepare(s: &mut Session, info: &TableInfo, view: PageView, last: bool) -> Result<Plan, EngineError> {
    let dialect = s.state.dialect();
    check(dialect, info, &view, last)?;
    let conn = s.browse()?;
    Ok(Plan { conn, dialect, info: info.clone(), view, last })
}

/// Starts `plan` for tab `tab`: the tab asks for its view (a new condition forgets the total,
/// `last` shows it as counting), its `seq` moves on, and the job runs.
fn start(s: &mut Session, label: &'static str, tab: TabId, plan: Plan) -> Result<Outcome, EngineError> {
    let Plan { conn, dialect, info, view, last } = plan;
    let Some(t) = table_tab_mut(s, tab) else { return Err(bad(format!("no tab {} is open", tab.0))) };
    if t.applied != view.applied {
        t.total = Total::Unknown;
    }
    t.applied = view.applied.clone();
    t.sort = view.sort.clone();
    if last {
        t.total = Total::Counting { estimate: quick_estimate(dialect, &info, &view.applied) };
    } else {
        t.page = view.page;
    }
    t.seq = t.seq.wrapping_add(1);
    let seq = t.seq;
    let columns = PageColumns::of(&info);
    let job_conn = Arc::clone(&conn);
    let job = s.spawn_job(label, async move {
        let loaded = load(job_conn.as_ref(), dialect, &info, &view, last).await;
        let apply: Apply = Box::new(move |s: &mut Session| apply_load(s, &conn, tab, seq, view, last, columns, loaded));
        apply
    });
    if let Some(t) = table_tab_mut(s, tab) {
        t.loading = Some(job);
        if last {
            t.counting = Some(job);
        }
    }
    Ok(Outcome::Started(job))
}

/// What a page job brings back: the count `last` made, and the page number with its rows.
pub(crate) struct Loaded {
    counted: Option<u64>,
    page: Result<(u64, ResultSet), DbError>,
}

async fn load(conn: &dyn Connection, dialect: Dialect, info: &TableInfo, view: &PageView, last: bool) -> Loaded {
    let mut page = view.page;
    let mut counted = None;
    if last {
        let rows = match count_sql(dialect, info, &view.applied) {
            Ok(sql) => conn.scalar_u64(&sql).await,
            // Built once already, before the job started.
            Err(e) => Err(DbError::Other(e.to_string())),
        };
        match rows {
            Ok(n) => {
                counted = Some(n);
                page = last_page(n);
            }
            Err(e) => return Loaded { counted, page: Err(e) },
        }
    }
    let rows = match page_sql(dialect, info, &view.applied, view.sort.as_ref(), page) {
        Ok(sql) => conn.run_untyped(&sql, PAGE_LIMITS).await,
        Err(e) => Err(DbError::Other(e.to_string())),
    };
    Loaded { counted, page: rows.map(|rs| (page, rs)) }
}

/// The columns a page statement reads ([`TableInfo::page_columns`]), typed from the catalog, and
/// how many of them, from the first, are visible.
pub(crate) struct PageColumns {
    columns: Vec<Column>,
    visible: usize,
}

impl PageColumns {
    fn of(info: &TableInfo) -> PageColumns {
        PageColumns { columns: info.page_columns().into_iter().map(ColumnInfo::column).collect(), visible: info.visible_columns().count() }
    }
}

/// The page as the tab keeps it: at most 100 rows, the catalog's columns. Then whether more rows
/// follow, and whether every row of the page was kept (the byte cap can stop short of it; the
/// 101st row only says that more follow, so missing it is no loss).
fn page_data(mut rs: ResultSet, columns: Vec<Column>) -> (ResultSet, bool, bool) {
    let has_next = rs.rows_seen > PAGE_SIZE;
    let complete = rs.rows.len() as u64 >= rs.rows_seen.min(PAGE_SIZE);
    rs.rows.truncate(PAGE_ROWS);
    rs.rows_seen = rs.rows_seen.min(PAGE_SIZE);
    // The statement names the catalog's columns, so the counts match.
    if rs.columns.len() == columns.len() {
        rs.columns = columns;
    }
    (rs, has_next, complete)
}

/// `rs` cut after its first `visible` columns: the rows the grid shows, and the hidden key
/// columns beside them (every row cut the same way; a short row stays short).
fn split_hidden(mut rs: ResultSet, visible: usize) -> (ResultSet, ResultSet) {
    if visible >= rs.columns.len() {
        return (rs, ResultSet::default());
    }
    let columns = rs.columns.split_off(visible);
    let rows = rs.rows.iter_mut().map(|row| row.split_off(visible.min(row.len()))).collect();
    (rs, ResultSet { columns, rows, ..ResultSet::default() })
}

fn apply_load(
    s: &mut Session,
    conn: &Arc<dyn Connection>,
    tab: TabId,
    seq: u64,
    mut view: PageView,
    last: bool,
    columns: PageColumns,
    loaded: Loaded,
) -> Result<Json, EngineError> {
    if !s.is_current_browse(conn) {
        return Ok(ignored());
    }
    let me = s.applying_job();
    let Some(t) = table_tab_mut(s, tab) else { return Ok(ignored()) };
    if last {
        // The count holds while the condition does, even when a newer load (another page, a
        // sort) made this page stale.
        if let Some(n) = loaded.counted
            && t.applied == view.applied
        {
            t.total = Total::Exact(n);
        }
        if t.counting.is_some() && t.counting == me {
            t.counting = None;
            if loaded.counted.is_none() {
                // The count failed and no newer one runs: back to the estimate.
                t.total = t.total.settled();
            }
        }
    }
    if t.seq != seq {
        return Ok(ignored());
    }
    t.loading = None;
    if t.has_staged_edits() {
        // The edits name rows of the page shown; another page would put them on other rows.
        t.back_to_shown();
        return Ok(ignored());
    }
    match loaded.page {
        Ok((page, rs)) => {
            // Nothing is staged; what could be undone or redone named the rows of the old page.
            t.edits = Arc::default();
            view.page = page;
            let typed = columns.columns.len();
            let (data, has_next, complete) = page_data(rs, columns.columns);
            // A result of other columns than the statement named is shown as it came.
            let (data, hidden) = if data.columns.len() == typed { split_hidden(data, columns.visible) } else { (data, ResultSet::default()) };
            let (rows, width) = (data.rows.len(), data.columns.len());
            let fits = |(r, c): (usize, usize)| r < rows && c < width;
            t.selection = t.selection.filter(|sel| fits(sel.anchor) && fits(sel.focus));
            t.data = Some(Arc::new(data));
            t.hidden = Arc::new(hidden);
            t.has_next = has_next;
            t.page_complete = complete;
            t.error = None;
            t.applied = view.applied.clone();
            t.sort = view.sort.clone();
            t.page = page;
            t.shown = view;
            Ok(json!({ "tab": tab, "page": page, "rows": rows, "hasNext": has_next, "total": t.total }))
        }
        Err(e) => {
            t.error = Some(e.to_string());
            t.back_to_shown();
            Err(s.db_failed(conn, &e))
        }
    }
}

/// Loading the page a tab shows again, inside another job: `edit.apply` reloads in its own job
/// after it writes. [`Reload::prepare`] runs on the caller's thread and moves the tab's `seq` on;
/// [`Reload::run`] loads in the job; [`Reload::finish`] applies the page like any other load.
pub(crate) struct Reload {
    dialect: Dialect,
    info: TableInfo,
    view: PageView,
    columns: PageColumns,
    seq: u64,
}

impl Reload {
    pub(crate) fn prepare(s: &mut Session, tab: TabId, info: &TableInfo) -> Result<Reload, EngineError> {
        let view = table_tab_mut(s, tab).map(|t| t.shown.clone()).ok_or_else(|| bad(format!("no tab {} is open", tab.0)))?;
        let dialect = s.state.dialect();
        check(dialect, info, &view, false)?;
        let t = table_tab_mut(s, tab).ok_or_else(|| bad(format!("no tab {} is open", tab.0)))?;
        t.seq = t.seq.wrapping_add(1);
        Ok(Reload { dialect, info: info.clone(), view, columns: PageColumns::of(info), seq: t.seq })
    }

    pub(crate) async fn run(&self, conn: &dyn Connection) -> Loaded {
        load(conn, self.dialect, &self.info, &self.view, false).await
    }

    pub(crate) fn finish(self, s: &mut Session, conn: &Arc<dyn Connection>, tab: TabId, loaded: Loaded) -> Result<Json, EngineError> {
        apply_load(s, conn, tab, self.seq, self.view, false, self.columns, loaded)
    }
}

/// Opens `table` in a new active tab with `filters` applied, and loads its first page.
fn open_tab(s: &mut Session, label: &'static str, table: TableRef, filters: Vec<Filter>) -> Result<Outcome, EngineError> {
    let catalog = catalog(s)?;
    let info = lookup(&catalog, &table)?;
    let view = PageView { applied: condition(&filters, "", false), sort: None, page: 0 };
    let plan = prepare(s, info, view.clone(), false)?;
    let id = s.state.take_tab_id();
    s.state.tabs.push(Tab::Table(TableTab::new(id, table, filters, view)));
    s.state.active = Some(id);
    start(s, label, id, plan)
}

/// Loads `view` (with `last`) in the active tab.
fn reload(s: &mut Session, label: &'static str, view: PageView, last: bool) -> Result<Outcome, EngineError> {
    let (tab, table) = {
        let t = active(s)?;
        (t.id, t.table.clone())
    };
    let catalog = catalog(s)?;
    let plan = prepare(s, lookup(&catalog, &table)?, view, last)?;
    start(s, label, tab, plan)
}

// ---- the commands ----------------------------------------------------------------------------

/// `{schema, table, filters?}` → a job returning `{tab, page, rows, hasNext, total}`: a new
/// active tab on page 0, with the filters applied.
fn open(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let table = TableRef { schema: p::str(params, "schema")?.to_owned(), name: p::str(params, "table")?.to_owned() };
    let filters = match params.get("filters").filter(|v| !v.is_null()) {
        Some(v) => parse_filters(v)?,
        None => Vec::new(),
    };
    open_tab(s, OPEN, table, filters)
}

/// `{filters?, raw?, rawMode?, apply? = true}`: sets what is given in the filter bar. With
/// `apply`, a job returning `{tab, page, rows, hasNext, total}`: the bar's condition (the raw
/// one in raw mode) is applied and page 0 loads. Without, `{tab, filters, raw, rawMode}`.
fn set_filters(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let filters = params.get("filters").filter(|v| !v.is_null()).map(parse_filters).transpose()?;
    let raw = p::opt_str(params, "raw")?.map(str::to_owned);
    let raw_mode = opt_bool(params, "rawMode")?;
    let apply = p::bool_or(params, "apply", true)?;
    let t = active(s)?;
    if apply && t.has_staged_edits() {
        return Err(disabled(STAGED));
    }
    let (tab, table, sort) = (t.id, t.table.clone(), t.sort.clone());
    let filters = filters.unwrap_or_else(|| t.filters.clone());
    let raw = raw.unwrap_or_else(|| t.raw.clone());
    let raw_mode = raw_mode.unwrap_or(t.raw_mode);
    // SQL dbee will not build leaves the bar as it was; past that check the bar keeps what was
    // typed, even when the connection then turns out to be lost.
    let dialect = s.state.dialect();
    let checked = if apply {
        let catalog = catalog(s)?;
        let info = lookup(&catalog, &table)?;
        let view = PageView { applied: condition(&filters, &raw, raw_mode), sort, page: 0 };
        check(dialect, info, &view, false)?;
        Some((info.clone(), view))
    } else {
        None
    };
    let Some(t) = table_tab_mut(s, tab) else { return Err(disabled(NO_TABLE_TAB)) };
    t.filters = filters;
    t.raw = raw;
    t.raw_mode = raw_mode;
    match checked {
        Some((info, view)) => {
            let conn = s.browse()?;
            start(s, SET_FILTERS, tab, Plan { conn, dialect, info, view, last: false })
        }
        None => Ok(Outcome::Done(json!({ "tab": tab, "filters": t.filters, "raw": t.raw, "rawMode": t.raw_mode }))),
    }
}

/// Where `table.page` goes.
enum To {
    First,
    Prev,
    Next,
    Last,
    /// 0-based.
    Page(u64),
}

fn parse_to(params: &Json) -> Result<To, EngineError> {
    let what = "`to` must be \"first\", \"prev\", \"next\", \"last\" or a page number from 1";
    match params.get("to") {
        Some(Json::String(word)) => match word.as_str() {
            "first" => Ok(To::First),
            "prev" => Ok(To::Prev),
            "next" => Ok(To::Next),
            "last" => Ok(To::Last),
            _ => Err(bad(what)),
        },
        Some(n @ Json::Number(_)) => match n.as_u64().and_then(|n| n.checked_sub(1)) {
            Some(page) if page <= MAX_PAGE => Ok(To::Page(page)),
            Some(_) => Err(bad("`to` is past the last page dbee can reach")),
            None => Err(bad(what)),
        },
        _ => Err(bad(what)),
    }
}

/// `{to}` → a job returning `{tab, page, rows, hasNext, total}`. `prev` on the first page and
/// `next` on the last one stay there (and load it again): the last page is the one shown when
/// it is the one asked for and nothing follows it, or with an exact total the last one counted.
/// `last` counts the rows first (and `total` ends exact).
fn page(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let to = parse_to(params)?;
    let t = active(s)?;
    let mut view = t.view();
    // The end shows: the page asked for is the one shown, and nothing follows it. With an exact
    // count the last page is known also while another page is on its way.
    let at_end = view == t.shown && !t.has_next;
    let counted_last = match t.total {
        Total::Exact(n) => last_page(n),
        _ => MAX_PAGE,
    };
    let last = matches!(to, To::Last);
    view.page = match to {
        To::First => 0,
        To::Prev => view.page.saturating_sub(1),
        To::Next if at_end => view.page,
        To::Next => view.page.saturating_add(1).min(counted_last).min(MAX_PAGE),
        To::Last => view.page,
        To::Page(n) => n,
    };
    reload(s, PAGE, view, last)
}

/// `{column, dir?}` → a job returning `{tab, page, rows, hasNext, total}`: sorts by the column
/// (`none` drops the sort) and goes to page 0. Without `dir` the column's sort moves on like a
/// header click: asc, desc, none.
fn sort(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let column = p::str(params, "column")?;
    let dir = p::opt_str(params, "dir")?;
    let t = active(s)?;
    let catalog = catalog(s)?;
    let info = lookup(&catalog, &t.table)?;
    if info.visible_column(column).is_none() {
        return Err(bad("the table has no such `column`"));
    }
    let current = t.sort.as_ref().filter(|sort| sort.column == column).map(|sort| sort.dir);
    let dir = match dir {
        Some("asc") => Some(SortDir::Asc),
        Some("desc") => Some(SortDir::Desc),
        Some("none") => None,
        Some(_) => return Err(bad("`dir` must be \"asc\", \"desc\" or \"none\"")),
        None => match current {
            None => Some(SortDir::Asc),
            Some(SortDir::Asc) => Some(SortDir::Desc),
            Some(SortDir::Desc) => None,
        },
    };
    let view = PageView { applied: t.applied.clone(), sort: dir.map(|dir| Sort { column: column.to_owned(), dir }), page: 0 };
    reload(s, SORT, view, false)
}

/// `{}` → a job returning `{tab, total}` with the exact count of the rows the applied condition
/// lets through. Meanwhile the total is `Counting`, with the estimate when there is one.
fn count(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let t = active(s)?;
    let (tab, condition) = (t.id, t.applied.clone());
    let catalog = catalog(s)?;
    let info = lookup(&catalog, &t.table)?;
    let dialect = s.state.dialect();
    let sql = count_sql(dialect, info, &condition).map_err(sql_bad)?;
    let estimate = quick_estimate(dialect, info, &condition);
    let conn = s.browse()?;
    let job_conn = Arc::clone(&conn);
    let job = s.spawn_job(COUNT, async move {
        let rows = job_conn.scalar_u64(&sql).await;
        let apply: Apply = Box::new(move |s: &mut Session| apply_count(s, &conn, tab, &condition, rows));
        apply
    });
    if let Some(t) = table_tab_mut(s, tab) {
        t.total = Total::Counting { estimate };
        t.counting = Some(job);
    }
    Ok(Outcome::Started(job))
}

fn apply_count(
    s: &mut Session,
    conn: &Arc<dyn Connection>,
    tab: TabId,
    condition: &Condition,
    rows: Result<u64, DbError>,
) -> Result<Json, EngineError> {
    if !s.is_current_browse(conn) {
        return Ok(ignored());
    }
    let me = s.applying_job();
    let Some(t) = table_tab_mut(s, tab) else { return Ok(ignored()) };
    let newest = t.counting.is_some() && t.counting == me;
    if newest {
        t.counting = None;
    }
    if t.applied != *condition {
        return Ok(ignored());
    }
    match rows {
        Ok(n) => {
            t.total = Total::Exact(n);
            Ok(json!({ "tab": tab, "total": t.total }))
        }
        Err(e) => {
            if newest {
                // No newer count runs: back to the estimate.
                t.total = t.total.settled();
            }
            Err(s.db_failed(conn, &e))
        }
    }
}

/// `{}` → `{sent}`: while the tab loads a page or counts, sends a cancel request for the
/// statement running on the browse connection (the job then ends with `cancelled`, and the rows
/// from before stay). `sent: false` when the tab runs nothing. The tab's markers name only
/// running jobs on the current browse connection (see the module docs); a marker whose job is
/// not running does not count, all the same.
///
/// While the tab's own Apply runs, `{sent: true, apply: true}`: the Apply is cancelled (it writes
/// nothing, and keeps its edits) and a cancel request stops the statement it waits on, as long as
/// its statements before `COMMIT` run. Once the Apply has decided to commit, nothing is sent and
/// the result is `Busy` ([`edit::COMMITTING`]): a cancel never meets the `COMMIT`, so the outcome
/// stays known.
///
/// A cancel request cancels whatever statement runs on the connection at that moment, and every
/// tab shares the browse connection. So while another tab's `edit.apply` job runs (or one of a tab
/// closed meanwhile) nothing is sent and the result is `Busy`: the tab's load may still be
/// waiting behind the Apply, and the request would roll the Apply back instead.
fn cancel(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let t = active(s)?;
    let alive = |job: Option<JobId>| job.is_some_and(|job| s.jobs.is_running(job));
    if let Some(job) = t.applying.filter(|&job| s.jobs.is_running(job)) {
        let gate = s.apply_gate.as_ref().filter(|(of, _)| *of == job).map(|(_, gate)| Arc::clone(gate));
        let Some(gate) = gate else { return Err(EngineError::Busy(edit::APPLYING.into())) };
        let conn = s.browse()?;
        if !gate.cancel() {
            return Err(EngineError::Busy(edit::COMMITTING.into()));
        }
        cancel_statements(s, std::iter::once(conn));
        return Ok(Outcome::Done(json!({ "sent": true, "apply": true })));
    }
    let running = alive(t.loading) || alive(t.counting);
    if !running {
        return Ok(Outcome::Done(json!({ "sent": false })));
    }
    if s.jobs.is_running_label(edit::APPLY) {
        return Err(EngineError::Busy(edit::APPLYING.into()));
    }
    let conn = s.browse()?;
    cancel_statements(s, std::iter::once(conn));
    Ok(Outcome::Done(json!({ "sent": true })))
}

/// `{}` → a job returning `{tab, page, rows, hasNext, total}`: loads the page again. The total
/// stays (count again for a new one).
fn refresh(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let view = active(s)?.view();
    reload(s, REFRESH, view, false)
}

/// `{offset? = 0, limit? = 100}` → `{tab, columns, rows, offset, page, requestedPage, hasNext,
/// pageComplete, total, editable, notEditableReason, outcomeUnknown, loading, error}`: the loaded
/// rows from `offset`, at most `limit`, each a list of text or `null`, and the 0-based page they
/// are (`requestedPage` is the one asked for, which differs while a load runs). `pageComplete` is
/// false when the byte cap kept fewer rows than the page has. The columns are the visible ones;
/// before the first page arrives, the catalog's, and no rows.
fn read(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let offset = p::opt_usize(params, "offset")?.unwrap_or(0);
    let limit = p::opt_usize(params, "limit")?.unwrap_or(READ_LIMIT);
    let t = active(s)?;
    let (columns, rows) = match &t.data {
        Some(d) => (json!(d.columns), json!(d.rows.iter().skip(offset).take(limit).collect::<Vec<_>>())),
        None => {
            let catalog = catalog(s)?;
            let columns: Vec<Column> = catalog.table(&t.table).map(|i| i.visible_columns().map(ColumnInfo::column).collect()).unwrap_or_default();
            (json!(columns), json!([]))
        }
    };
    let reason = not_editable(s, t);
    Ok(Outcome::Done(json!({
        "tab": t.id,
        "columns": columns,
        "rows": rows,
        "offset": offset,
        "page": t.shown.page,
        "requestedPage": t.page,
        "hasNext": t.has_next,
        "pageComplete": t.page_complete,
        "total": t.total,
        "editable": reason.is_none(),
        "notEditableReason": reason,
        "outcomeUnknown": t.edits.outcome_unknown(),
        "loading": t.loading,
        "error": t.error,
    })))
}

/// `{row, column}` → a job returning `{tab, page, rows, hasNext, total}`: the foreign key that
/// holds the column (a name, or a 0-based index into the page's columns), followed from that
/// loaded row: its table opens in a new tab with one `eq` filter per key column.
fn follow(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let row = p::usize(params, "row")?;
    let t = active(s)?;
    let data = t.data.as_ref().ok_or_else(|| bad("the page has not loaded yet"))?;
    let name = match params.get("column") {
        Some(Json::String(name)) => name.as_str(),
        Some(Json::Number(n)) => n
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| data.columns.get(i))
            .map(|c| c.name.as_str())
            .ok_or_else(|| bad("`column` is not a column of the page"))?,
        _ => return Err(bad("`column` is required (a name or a 0-based index)")),
    };
    let catalog = catalog(s)?;
    let info = lookup(&catalog, &t.table)?;
    let fk = info.fk_for(name).ok_or_else(|| bad("`column` is not part of a foreign key"))?;
    let values = data.rows.get(row).ok_or_else(|| bad("`row` is not a loaded row"))?;
    let mut filters = Vec::with_capacity(fk.columns.len());
    for (column, target) in fk.columns.iter().zip(&fk.target_columns) {
        let at = data.columns.iter().position(|c| c.name == *column).ok_or_else(|| bad("the page lacks a column of the foreign key; refresh it"))?;
        match values.get(at) {
            Some(Some(value)) => filters.push(Filter { column: target.clone(), op: Op::Eq, value: value.clone() }),
            Some(None) => return Err(bad("no reference: the value is NULL")),
            None => return Err(bad("the row is shorter than the page's columns")),
        }
    }
    let target = fk.target.clone();
    open_tab(s, FOLLOW, target, filters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbee_core::TypeClass;

    #[test]
    fn totals_serialize_and_settle() {
        assert_eq!(json!(Total::Unknown), json!("unknown"));
        assert_eq!(json!(Total::Estimate(5)), json!({ "estimate": 5 }));
        assert_eq!(json!(Total::Counting { estimate: None }), json!({ "counting": { "estimate": null } }));
        assert_eq!(json!(Total::Exact(7)), json!({ "exact": 7 }));
        assert_eq!(Total::Counting { estimate: Some(3) }.settled(), Total::Estimate(3));
        assert_eq!(Total::Counting { estimate: None }.settled(), Total::Unknown);
        assert_eq!(Total::Exact(2).settled(), Total::Exact(2));
    }

    #[test]
    fn last_pages() {
        assert_eq!([0, 1, 100, 101, 1_000_000].map(last_page), [0, 0, 0, 1, 9999]);
        assert_eq!(last_page(u64::MAX), u64::MAX / PAGE_SIZE);
    }

    #[test]
    fn page_targets() {
        let to = |v: Json| parse_to(&json!({ "to": v }));
        assert!(matches!(to(json!(1)), Ok(To::Page(0))));
        assert!(matches!(to(json!(MAX_PAGE + 1)), Ok(To::Page(MAX_PAGE))));
        assert!(matches!(to(json!("last")), Ok(To::Last)));
        for bad in [json!(0), json!(-3), json!(2.5), json!(MAX_PAGE + 2), json!(u64::MAX), json!("Next"), json!(null), json!([1])] {
            assert!(to(bad.clone()).is_err(), "{bad}");
        }
        assert!(parse_to(&json!({})).is_err());
    }

    #[test]
    fn filters_parse_leniently_and_refuse_garbage() {
        let f = parse_filters(&json!([
            { "column": "a", "op": "eq", "value": "x" },
            { "column": "b", "op": "gt", "value": 42 },
            { "column": "c", "op": "isNull" },
            { "column": "d", "op": "eq", "value": true },
        ]))
        .unwrap();
        assert_eq!(f.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["x", "42", "", "true"]);
        assert_eq!(f[2].op, Op::IsNull);
        for bad in [
            json!({}),
            json!([1]),
            json!([{ "op": "eq" }]),
            json!([{ "column": "a" }]),
            json!([{ "column": "a", "op": "between" }]),
            json!([{ "column": "a", "op": "eq", "value": [1] }]),
        ] {
            assert!(parse_filters(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn conditions_from_the_bar() {
        let f = vec![Filter { column: "a".into(), op: Op::Eq, value: "1".into() }];
        assert_eq!(condition(&f, "x > 1", false), Condition::Filters(f.clone()));
        assert_eq!(condition(&f, "x > 1", true), Condition::Raw("x > 1".into()));
        assert_eq!(condition(&f, "  ", true), Condition::None);
        assert_eq!(condition(&[], "", false), Condition::None);
    }

    #[test]
    fn page_data_keeps_100_rows_and_typed_columns() {
        let rs = ResultSet {
            columns: vec![Column::untyped("id")],
            rows: (0..101).map(|i| vec![Some(i.to_string())]).collect(),
            rows_seen: 101,
            complete: true,
            rows_affected: Some(101),
        };
        let typed = vec![Column { name: "id".into(), base_type: "int4".into(), type_label: "integer".into(), class: TypeClass::Number }];
        let (page, more, complete) = page_data(rs.clone(), typed.clone());
        assert!(more && complete);
        assert_eq!((page.rows.len(), page.rows_seen, &page.columns), (100, 100, &typed));
        let short = ResultSet { rows: rs.rows[..3].to_vec(), rows_seen: 3, ..rs.clone() };
        let (page, more, complete) = page_data(short, vec![]);
        assert!(!more && complete);
        assert_eq!((page.rows.len(), page.columns[0].base_type.as_str()), (3, ""), "a column count that differs keeps the result's");
        // The byte cap kept 40 of 101 rows: cut. Kept all 100 but not the 101st: whole.
        let cut = ResultSet { rows: rs.rows[..40].to_vec(), complete: false, ..rs.clone() };
        let (page, more, complete) = page_data(cut, vec![]);
        assert!(more && !complete, "40 of 100 rows kept, more rows follow");
        assert_eq!(page.rows.len(), 40);
        let probe_lost = ResultSet { rows: rs.rows[..100].to_vec(), complete: false, ..rs };
        assert!(page_data(probe_lost, vec![]).2, "only the 101st row was not kept");
    }

    #[test]
    fn a_page_keeps_its_hidden_key_beside_the_rows_it_shows() {
        let cell = |v: &str| Some(v.to_owned());
        let rs = ResultSet {
            columns: vec![Column::untyped("body"), Column::untyped("rowid")],
            rows: vec![vec![cell("a"), cell("1")], vec![cell("b")], vec![]],
            rows_seen: 3,
            complete: true,
            rows_affected: None,
        };
        let (shown, hidden) = split_hidden(rs.clone(), 1);
        assert_eq!((shown.columns, shown.rows), (vec![Column::untyped("body")], vec![vec![cell("a")], vec![cell("b")], vec![]]));
        assert_eq!((shown.rows_seen, shown.complete), (3, true));
        assert_eq!((hidden.columns, hidden.rows), (vec![Column::untyped("rowid")], vec![vec![cell("1")], vec![], vec![]]), "short rows stay short");
        // Every column shows: nothing beside it.
        let (shown, hidden) = split_hidden(rs.clone(), 2);
        assert_eq!((shown, hidden), (rs, ResultSet::default()));
    }
}

//! Query tabs: `query.run`, `query.cancel`, `query.showResult` and `query.setDatabase`.
//!
//! **Running** is a job on the query connection, which is not the browse connection: a `BEGIN`
//! left open in a query tab never reaches table pages or an Apply. The job opens the connection
//! when the session has none (the first run, after a reconnect, or after it was lost), with the
//! driver, settings, password and tunnel of the open connection, so the query connection of a
//! read-only connection is read-only too. It runs the statements `dbee_sql::lexer` splits from the
//! tab text in the connection's dialect (all of them, or the one under the cursor) one by one,
//! each in auto-commit, and stops at the first error. Every result is kept
//! ([`QueryTab::results`]) and the last one is shown; all the
//! results of one run together keep at most [`RUN_LIMITS`] (10,000 rows each, 256 MiB held
//! for the whole run: each statement gets what the ones before left). One run goes at a time on a
//! connection (`Busy`): its query connection runs one statement at a time, and a cancel request hits
//! whatever statement runs on it.
//!
//! **A lost connection.** When the browse connection or the tunnel has closed, the run is refused
//! as every command that needs the database is: the session is marked lost, and the error is
//! `connectionLost` naming `connection.reconnect` in `data.action`. When only the query connection
//! has closed, the session goes on: the connection is let go and the run fails with
//! [`QUERY_SESSION_LOST`]. Found before the run, it fails at its first statement, before sending
//! it; failing a statement, the outcome of that statement is unknown (it may have been the
//! `COMMIT` or an auto-commit write), and the statements after it are not sent. The error has
//! code `connectionLost` and no `action`. Its session state is gone, an open transaction, `SET`s
//! and temporary tables, so dbee never runs the rest on a new connection as if nothing happened
//! (a `COMMIT` there would succeed with nothing to commit). The next run opens a new one.
//!
//! **Stopping.** `query.cancel`, closing the running tab, `connection.close` and a reconnect stop
//! the run through its `Stop`. The job then sends the cancel request itself, through the query
//! connection's `Cancel`, again while the statement has not ended (a request that reaches the
//! server before the statement is lost), and waits for the statement to end and for each request
//! to arrive (so none can reach a later run, [`crate::jobs::stoppable`]); it starts no further
//! statement. A stopped run ends with code `cancelled`. The job is never dropped or aborted.
//!
//! **The database a run starts in.** On a dialect with `USE`, a tab with a database of its own
//! makes it current first, unless the query connection is known to be there already (a `USE`
//! also clears MySQL's warnings, which a `SHOW WARNINGS` of its own run would then miss). The
//! engine keeps the database the query connection is in ([`CurrentDatabase`]): the settings'
//! when the connection opens, each `USE` it sends, and where the driver says each statement that
//! succeeded left it (`ResultSet::database_after`; MySQL's server reports it in its OK packets).
//! It sends no statement of its own to find out. A statement the server answered with an error
//! moves nothing; one that ended any other way (cancelled, or lost) may have, so the database is
//! no longer known, as when the driver cannot tell: it is never guessed. After a run
//! that got past its `USE`, the tab's database becomes that one, so a `USE` in the text sticks to
//! the tab; the next run's `USE` keeps every tab in its own. A tab with none starts where the
//! connection is ([`crate::Connected::starts_in`]).
//!
//! **Errors** keep the server's message, code, meaning, detail and hint ([`QueryError`]); the
//! position the server gives (characters into the statement) becomes a byte offset into the tab
//! text.
//!
//! **Stale results** (see [`super`]): a run whose connection was closed or replaced is
//! `{"ignored": true}` and changes nothing. A run whose tab closed is ignored too, but the engine
//! keeps where it left the query connection, and the connection the run opened unless it was
//! lost. A query connection the session had is kept even when the run lost it, so the next run
//! says it was lost.
//!
//! None of these commands is journaled: a replay would run the SQL again, writes included. Logs
//! carry command ids and job labels, never SQL text.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use dbee_config::ConnectionConfig;
use dbee_core::ResultSet;
use dbee_driver::{Connection, DbError, Driver, Limits, Purpose, QUERY_LIMITS, Target, TlsState};
use dbee_sql::{Dialect, lexer};
use dbee_ssh::Tunnel;
use serde_json::json;

use super::connection::{db_error, driver_for};
use super::{CommandSpec, NO_QUERY_TAB, NOT_CONNECTED, active_query, bad, disabled, ignored, needs_db, p};
use crate::jobs::{self, Stop};
use crate::state::{Connected, CurrentDatabase, QueryError, QueryRun, QueryTab, Selection, StatementOutcome};
use crate::turns::InTurn;
use crate::{Apply, ConnectionId, EngineError, JobId, Json, Outcome, Session, Tab, TabId, Which};

const RUN: &str = "query.run";
const CANCEL: &str = "query.cancel";
const SHOW_RESULT: &str = "query.showResult";
const SET_DATABASE: &str = "query.setDatabase";

/// Why `query.run` does not start: the text holds no statement.
pub const NOTHING_TO_RUN: &str = "there is no statement to run";

/// What one run keeps at most: `max_rows` per result, and `max_bytes` held by all its results
/// together, as [`dbee_driver::result_cost`] counts it.
pub const RUN_LIMITS: Limits = QUERY_LIMITS;

/// The error of a run whose query connection was lost while the session was not (see the module
/// docs).
pub const QUERY_SESSION_LOST: &str = "the query connection was lost, and its session with it: the statement that was running may or may not have taken effect (its outcome is unknown), any other uncommitted work in the transaction is gone, and SET values and temporary tables are gone. Run again to open a new one";

/// The error of a statement holding a NUL character where the dialect says the server cannot
/// take one.
const NUL: &str = "the statement holds a NUL character (U+0000), which this database can't take";

/// Why a run fails whose new query connection has less TLS than the browse connection.
pub const QUERY_TLS_LOST: &str = "The query connection could not set up TLS, though the connection did. dbee did not send the statement. \
                                  Reconnect; if this happens again, something on the network may be stopping TLS.";

/// Why `query.setDatabase` is off: the dialect has no `USE`.
pub const NO_DATABASE_SWITCH: &str = "a query tab's database can't be set on this connection";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: RUN,
        label: "Run",
        menu: &["Query"],
        shortcut: Some("Ctrl+Enter"),
        params: "{current? = false, cursor?: byte offset into the text, needed with current}",
        enabled: can_run,
        run,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: CANCEL,
        label: "Cancel Query",
        menu: &["Query"],
        shortcut: None,
        params: "{}",
        enabled: active_query,
        run: cancel,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SHOW_RESULT,
        label: "Show Result",
        menu: &[],
        shortcut: None,
        params: "{index}",
        enabled: active_query,
        run: show_result,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SET_DATABASE,
        label: "Set Query Database",
        menu: &[],
        shortcut: None,
        params: "{tab?, database: a name, or null for none}",
        enabled: can_set_database,
        run: set_database,
        journal: true,
        undoable: false,
    },
];

/// `query.run`'s enabled check: a query tab is active, on a connection that is not lost. The UI's
/// Run buttons follow it; what the run itself refuses (no statement, a run going) it says when
/// asked to run.
pub fn can_run(s: &Session) -> Result<(), String> {
    needs_db(s)?;
    active_query(s)
}

/// `query.setDatabase`'s enabled check: a connection is open whose dialect has `USE`.
pub fn can_set_database(s: &Session) -> Result<(), String> {
    let c = s.state.connection(Which::Active).ok_or(NOT_CONNECTED)?;
    if c.dialect.has_use_database() { Ok(()) } else { Err(NO_DATABASE_SWITCH.into()) }
}

// ---- stopping a run --------------------------------------------------------------------------

/// Stops the query run of connection `connection` when it is `job`.
pub(crate) fn stop_run(s: &Session, connection: ConnectionId, job: JobId) {
    if let Some(run) = s.state.connection(Which::Id(connection)).and_then(|c| c.query_run.as_ref()).filter(|run| run.job == job) {
        run.stop.stop();
    }
}

/// Why another run cannot start now on connection `connection`: one is in progress.
fn busy(s: &Session, connection: ConnectionId) -> Option<String> {
    let run = s.state.connection(Which::Id(connection))?.query_run.as_ref()?;
    if !s.jobs.is_running(run.job) {
        return None;
    }
    let title = s.state.tabs.iter().find_map(|t| match t {
        Tab::Query(q) if q.running == Some(run.job) => Some(q.title.as_str()),
        _ => None,
    });
    Some(match title {
        Some(title) => running_in(title),
        None => "a cancelled query is still stopping".into(),
    })
}

/// Why a command waits for the run of the tab titled `title`.
fn running_in(title: &str) -> String {
    format!("a query is running in {title}")
}

// ---- the job ---------------------------------------------------------------------------------

/// The connection a run uses.
enum Via {
    /// The session's query connection.
    Open(Arc<dyn Connection>),
    /// None yet: the job opens one as the browse connection was opened.
    Connect {
        driver: Arc<dyn Driver>,
        config: Box<ConnectionConfig>,
        password: Option<String>,
        tunnel: Option<Arc<Tunnel>>,
        allowed_dirs: Option<Vec<PathBuf>>,
        /// What protects the browse connection's link: the query connection may have no less.
        browse_tls: Option<TlsState>,
    },
}

/// Why a run stopped early: at statement `statement` (0-based), with the byte offset in the tab
/// text the error points at.
struct Failure {
    statement: usize,
    error: DbError,
    position: Option<usize>,
}

/// What a run brings back.
struct Ran {
    /// The connection the statements ran on and whether the job opened it; `None` when it could
    /// not be opened, or the run stopped first.
    conn: Option<(Arc<dyn Connection>, bool)>,
    outcomes: Vec<StatementOutcome>,
    failure: Option<Failure>,
    /// The database the connection is in after the run.
    current: CurrentDatabase,
    /// The tab's `USE` went through, or it needed none: its statements were reached.
    past_use: bool,
}

/// The byte offset in `text` of 1-based character `chars` of the statement at `range`, as the
/// server counts error positions; at the end of the input and past it, the statement's end.
/// `None` for 0, or a range that is not in the text.
fn byte_position(text: &str, range: &Range<usize>, chars: u32) -> Option<usize> {
    let statement = text.get(range.clone())?;
    let n = usize::try_from(chars.checked_sub(1)?).ok()?;
    let within = statement.char_indices().nth(n).map_or(statement.len(), |(at, _)| at);
    range.start.checked_add(within)
}

/// What a run sends besides the statements, from the tab and the connection's dialect.
struct RunSetup {
    /// Sent before the first statement: the tab's database, and its `USE`.
    use_database: Option<(String, String)>,
    /// What the statements are checked with before they are sent (a NUL the server cannot take);
    /// whether a statement can move the connection to another database.
    dialect: Dialect,
    /// The database the connection the run uses is in when it starts.
    current: CurrentDatabase,
}

/// Runs `ranges` of `text` on the connection `via` gives, until one fails or `stop` is set; the
/// `USE` of `setup` first, a failure of which is the first statement's. The results together keep
/// at most `budget` bytes held: each statement gets what the ones before left, and once it
/// is spent, later results keep no rows (and say they are incomplete).
async fn run_statements(via: Via, text: String, ranges: Vec<Range<usize>>, setup: RunSetup, stop: Arc<Stop>, budget: usize) -> Ran {
    let mut ran = Ran { conn: None, outcomes: Vec::with_capacity(ranges.len()), failure: None, current: setup.current.clone(), past_use: false };
    let stopped = |statement| Some(Failure { statement, error: DbError::Cancelled, position: None });
    if stop.is_set() {
        ran.failure = stopped(0);
        return ran;
    }
    let (conn, opened) = match via {
        Via::Open(conn) => (conn, false),
        Via::Connect { driver, config, password, tunnel, allowed_dirs, browse_tls } => {
            let target = Target {
                config: &config,
                password: password.as_deref(),
                tunnel,
                purpose: Purpose::Query,
                create_file: false,
                allowed_dirs: allowed_dirs.as_deref(),
            };
            // A connect that is stopped is dropped: nothing ran on it yet.
            tokio::select! {
                connected = driver.connect(target) => match connected {
                    // The same settings gave the browse connection more: someone on the network
                    // took TLS away. The connection closes as it is dropped, unused.
                    Ok(conn) if conn.info().tls < browse_tls => {
                        log::info!("the query connection has less TLS than the browse connection; nothing was sent");
                        ran.failure = Some(Failure { statement: 0, error: DbError::Other(QUERY_TLS_LOST.into()), position: None });
                        return ran;
                    }
                    Ok(conn) => (conn, true),
                    Err(error) => {
                        ran.failure = Some(Failure { statement: 0, error, position: None });
                        return ran;
                    }
                },
                () = stop.stopped() => {
                    ran.failure = stopped(0);
                    return ran;
                }
            }
        }
    };
    let moves = setup.dialect.has_use_database();
    if stop.is_set() {
        ran.failure = stopped(0);
    } else if let Some((name, sql)) = setup.use_database {
        match run_one(conn.as_ref(), &sql, Limits { max_rows: 0, max_bytes: 0 }, &stop).await {
            Ok(_) => ran.current = CurrentDatabase::known(Some(name)),
            Err(error) => {
                ran.current = ran.current.after_failure(&error, moves);
                let error = match error {
                    DbError::Server(mut e) => {
                        e.message = format!("the tab's database `{name}`: {}", e.message);
                        DbError::Server(e)
                    }
                    other => other,
                };
                ran.failure = Some(Failure { statement: 0, error, position: None });
            }
        }
    }
    if ran.failure.is_none() {
        ran.past_use = true;
        let mut left = budget;
        for (i, range) in ranges.into_iter().enumerate() {
            if stop.is_set() {
                ran.failure = stopped(i);
                break;
            }
            let Some(sql) = text.get(range.clone()) else {
                ran.failure = Some(Failure { statement: i, error: DbError::Other("the statement is not in the text".into()), position: None });
                break;
            };
            if let Some(at) = setup.dialect.misplaced_nul(sql) {
                ran.failure = Some(Failure { statement: i, error: DbError::Other(NUL.into()), position: range.start.checked_add(at) });
                break;
            }
            let limits = Limits { max_rows: if left == 0 { 0 } else { RUN_LIMITS.max_rows }, max_bytes: left };
            match run_one(conn.as_ref(), sql, limits, &stop).await {
                Ok(result) => {
                    ran.current = ran.current.after(&result.database_after, moves);
                    left = left.saturating_sub(dbee_driver::result_cost(&result));
                    ran.outcomes.push(StatementOutcome { range, result: Arc::new(result) });
                }
                Err(error) => {
                    ran.current = ran.current.after_failure(&error, moves);
                    let position = match &error {
                        DbError::Server(s) => s.position.and_then(|chars| byte_position(&text, &range, chars)),
                        _ => None,
                    };
                    ran.failure = Some(Failure { statement: i, error, position });
                    break;
                }
            }
        }
    }
    ran.conn = Some((conn, opened));
    ran
}

/// Runs one statement. When the run is stopped meanwhile, the cancel request goes out until the
/// statement has ended ([`jobs::stoppable`]); it then usually ends with `Cancelled`.
async fn run_one(conn: &dyn Connection, sql: &str, limits: Limits, stop: &Stop) -> Result<ResultSet, DbError> {
    jobs::stoppable(conn.canceller(), conn.run(sql, limits), stop).await
}

// ---- applying a run --------------------------------------------------------------------------

/// The result as `query.run`, `query.showResult` and `inspect()` show it: `{statement, start,
/// end, columns, rows, rowsSeen, complete, rowsAffected}`, `rows` being the rows kept.
fn outcome_json(statement: usize, o: &StatementOutcome) -> Json {
    let r = &o.result;
    json!({
        "statement": statement,
        "start": o.range.start,
        "end": o.range.end,
        "columns": r.columns,
        "rows": r.rows.len(),
        "rowsSeen": r.rows_seen,
        "complete": r.complete,
        "rowsAffected": r.rows_affected,
    })
}

fn results_json(q: &QueryTab) -> Vec<Json> {
    q.results.iter().enumerate().map(|(i, o)| outcome_json(i, o)).collect()
}

/// The tab as `inspect()` shows it: `{tab, title, text, database, startsIn, running, results,
/// shown, error}`, `startsIn` being `{database, known}`, where its next run starts
/// ([`Connected::starts_in`]; null while no connection is open).
pub(crate) fn summary(q: &QueryTab, connected: Option<&Connected>) -> Json {
    json!({
        "tab": q.id,
        "title": q.title,
        "text": q.text,
        "database": q.database,
        "startsIn": connected.map(|c| c.starts_in(q).summary()),
        "running": q.running,
        "results": results_json(q),
        "shown": q.shown,
        "error": q.error,
    })
}

/// `selection` when it lies in `result`'s grid.
fn fitting(selection: Option<Selection>, result: Option<&ResultSet>) -> Option<Selection> {
    let result = result?;
    let fits = |(row, column): (usize, usize)| row < result.rows.len() && column < result.columns.len();
    selection.filter(|sel| fits(sel.anchor) && fits(sel.focus))
}

/// The error a run ended with, its `position` a byte offset into the tab text and `statement`
/// added; a `Db` error's `data` gets the same two.
fn run_error(e: EngineError, f: &Failure) -> EngineError {
    match e {
        EngineError::Db { code, message, mut data } => {
            if let Json::Object(map) = &mut data {
                map.insert("position".into(), json!(f.position));
                map.insert("statement".into(), json!(f.statement));
            }
            EngineError::Db { code, message, data }
        }
        other => other,
    }
}

/// The error of a run whose query connection was lost while the session was not.
fn session_lost() -> EngineError {
    EngineError::Db { code: DbError::ConnectionLost.code(), message: QUERY_SESSION_LOST.into(), data: json!({}) }
}

/// The error `f` gives, on the query connection `conn` of open connection `id` (`None` when it
/// could not be opened). A lost query connection is let go: alone, it is [`session_lost`]; with
/// the browse connection or the tunnel, which an unopened query connection can also have failed
/// on, the connection is lost (see the module docs).
fn failed(s: &mut Session, id: ConnectionId, conn: Option<&Arc<dyn Connection>>, f: &Failure) -> EngineError {
    let Some(c) = s.state.connection_mut(Which::Id(id)) else { return db_error(&f.error) };
    let query_lost = f.error == DbError::ConnectionLost || conn.is_some_and(|q| q.is_closed());
    if query_lost && c.query.as_ref().zip(conn).is_some_and(|(mine, used)| Arc::ptr_eq(mine, used)) {
        c.query = None;
    }
    let gone = c.browse.is_closed() || c.tunnel.as_ref().is_some_and(|t| t.is_closed());
    if gone && (query_lost || conn.is_none()) {
        if !c.lost {
            c.lost = true;
            log::info!("query: the browse connection or the tunnel has closed; the session is marked lost");
        }
        return db_error(&DbError::ConnectionLost);
    }
    if query_lost {
        log::info!("query: the query connection was lost; the next run opens a new one");
        return session_lost();
    }
    db_error(&f.error)
}

/// What the tab shows of `f`, which the run failed with as `e`: a server error's own message with
/// its code, the code, its meaning, the detail and the hint, else the engine error's text.
fn query_error(e: &EngineError, f: &Failure) -> QueryError {
    let (message, server_code, meaning, detail, hint) = match &f.error {
        DbError::Server(s) => (s.message_with_code(), Some(s.code.clone()), s.meaning, s.detail.clone(), s.hint.clone()),
        _ => (e.to_string(), None, None, None, None),
    };
    QueryError { code: e.code(), message, server_code, meaning, detail, hint, position: f.position, statement: f.statement }
}

/// `conn` is the connection `generation` was taken from.
fn same_connection(generation: &Weak<InTurn>, conn: &Arc<InTurn>) -> bool {
    std::ptr::eq(Weak::as_ptr(generation), Arc::as_ptr(conn))
}

/// Applies a run on open connection `id`: first to its query connection, also when the run's tab
/// has closed (where the run left it, and the connection the job opened), then to the tab, when
/// it is still open and running this job (its results, and its error, which lets a lost query
/// connection go).
fn apply_run(s: &mut Session, tab: TabId, id: ConnectionId, generation: &Weak<InTurn>, ran: Ran) -> Result<Json, EngineError> {
    let me = s.applying_job();
    let for_tab = matches!(s.state.tab(tab), Some(Tab::Query(q)) if me.is_some() && q.running == me);
    // The connection is still the one the run started on: not closed, not replaced.
    let Some(c) = s.state.connection_mut(Which::Id(id)).filter(|c| same_connection(generation, &c.browse)) else { return Ok(ignored()) };
    // The run is over (clearing the marker of a job that ended is not applying its result).
    if me.is_some() && c.query_run.as_ref().map(|run| run.job) == me {
        c.query_run = None;
    }
    let Ran { conn, outcomes, failure, current, past_use } = ran;
    match &conn {
        // Opened by the job: it becomes the session's query connection. Without the tab, a lost
        // one is dropped: no open tab ever ran on it.
        Some((used, true)) if c.query.is_none() && (for_tab || !used.is_closed()) => c.query = Some(Arc::clone(used)),
        Some((used, false)) if !c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, used)) => return Ok(ignored()),
        _ => {}
    }
    if let Some((used, _)) = &conn
        && c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, used))
    {
        c.query_database = current.clone();
    }
    if !for_tab {
        return Ok(ignored());
    }
    let error = failure.map(|f| {
        let e = failed(s, id, conn.as_ref().map(|(conn, _)| conn), &f);
        (query_error(&e, &f), run_error(e, &f))
    });
    let has_use = s.state.dialect(Which::Id(id)).has_use_database();
    let Some(Tab::Query(q)) = s.state.tab_mut(tab) else { return Ok(ignored()) };
    if has_use && past_use && current.known {
        q.database = current.database;
    }
    q.running = None;
    q.results = outcomes;
    q.shown = q.results.len().saturating_sub(1);
    q.selection = fitting(q.selection, q.shown_result().map(|o| o.result.as_ref()));
    match error {
        Some((shown, e)) => {
            q.error = Some(shown);
            Err(e)
        }
        None => {
            q.error = None;
            Ok(json!({ "tab": tab, "results": results_json(q), "shown": q.shown }))
        }
    }
}

// ---- the commands ----------------------------------------------------------------------------

/// `{current? = false, cursor?}` → a job returning `{tab, results, shown}` (see the module docs).
/// `current` runs only the statement under `cursor`, a byte offset into the text: `cursor == len`
/// gives the last statement, a cursor past the end is `BadParams`. A run that stops early fails with the error, and the results before it
/// are kept.
fn run(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let current = p::bool_or(params, "current", false)?;
    let cursor = p::opt_usize(params, "cursor")?;
    let (tab, text, database) = match s.state.active_tab() {
        Some(Tab::Query(q)) => (q.id, q.text.clone(), q.database.clone()),
        _ => return Err(disabled(NO_QUERY_TAB)),
    };
    if let Some(at) = cursor
        && !text.is_char_boundary(at)
    {
        return Err(bad("`cursor` must be a byte offset into the text, from 0 to its length, on a character boundary"));
    }
    let id = s.state.connection(Which::Tab(tab)).map(|c| c.id).ok_or_else(|| disabled(NOT_CONNECTED))?;
    if let Some(why) = busy(s, id) {
        return Err(EngineError::Busy(why));
    }
    let dialect = s.state.dialect(Which::Id(id));
    let ranges = if current {
        let cursor = cursor.ok_or_else(|| bad("`cursor` is required with `current` (a byte offset into the text)"))?;
        lexer::statement_at(dialect, &text, cursor).into_iter().collect()
    } else {
        lexer::statements(dialect, &text)
    };
    if ranges.is_empty() {
        return Err(disabled(NOTHING_TO_RUN));
    }
    // A closed browse connection or tunnel marks the connection lost.
    s.browse(Which::Id(id))?;
    let c = s.state.connection_mut(Which::Id(id)).ok_or_else(|| disabled(NOT_CONNECTED))?;
    let current = if c.query.is_some() { c.query_database.clone() } else { CurrentDatabase::of_settings(&c.config) };
    let use_database = match database.filter(|name| !current.is(name)) {
        Some(name) => dialect.use_database(&name).map_err(|e| bad(format!("the tab's database: {e}")))?.map(|sql| (name, sql)),
        None => None,
    };
    let setup = RunSetup { use_database, dialect, current };
    if c.query.as_ref().is_some_and(|q| q.is_closed()) {
        log::info!("query: the query connection has closed; this run fails, and the next opens a new one");
        c.query = None;
        let f = Failure { statement: 0, error: DbError::ConnectionLost, position: None };
        let e = session_lost();
        if let Some(Tab::Query(q)) = s.state.tab_mut(tab) {
            q.results = Vec::new();
            q.shown = 0;
            q.selection = None;
            q.error = Some(query_error(&e, &f));
        }
        return Err(run_error(e, &f));
    }
    let via = match &c.query {
        Some(conn) => Via::Open(Arc::clone(conn)),
        None => Via::Connect {
            driver: driver_for(&s.drivers, c.config.kind)?,
            config: Box::new(c.config.clone()),
            password: c.credentials.password.clone(),
            tunnel: c.tunnel.clone(),
            allowed_dirs: s.allowed_dirs.clone(),
            browse_tls: c.info.tls,
        },
    };
    let generation = Arc::downgrade(&c.browse);
    let stop = Arc::new(Stop::default());
    let job_stop = Arc::clone(&stop);
    let job = s.spawn_stoppable_job(RUN, Arc::clone(&stop), async move {
        let ran = run_statements(via, text, ranges, setup, job_stop, RUN_LIMITS.max_bytes).await;
        let apply: Apply = Box::new(move |s: &mut Session| apply_run(s, tab, id, &generation, ran));
        apply
    });
    if let Some(c) = s.state.connection_mut(Which::Id(id)) {
        c.query_run = Some(QueryRun { job, stop });
    }
    if let Some(Tab::Query(q)) = s.state.tab_mut(tab) {
        q.running = Some(job);
    }
    Ok(Outcome::Started(job))
}

/// `{}` → `{tab, sent, stopping?}`: stops the active tab's run (see the module docs); `sent: false`
/// when the tab runs nothing. Called again while the run stops, it sends nothing itself (the job
/// sends the request again while the statement runs): `{tab, sent: false, stopping: true}`.
fn cancel(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let (tab, running) = match s.state.active_tab() {
        Some(Tab::Query(q)) => (q.id, q.running),
        _ => return Err(disabled(NO_QUERY_TAB)),
    };
    let run = s.state.connection(Which::Tab(tab)).and_then(|c| c.query_run.as_ref());
    let stopped = match (running, run) {
        (Some(job), Some(run)) if run.job == job && s.jobs.is_running(job) => Some(run.stop.stop()),
        _ => None,
    };
    Ok(Outcome::Done(match stopped {
        Some(false) => json!({ "tab": tab, "sent": false, "stopping": true }),
        sent => json!({ "tab": tab, "sent": sent.is_some() }),
    }))
}

/// `{tab?, database}` → `{tab, database}`: the database the query tab's runs start in (default:
/// the active tab); `null` for none, which starts where the query connection is. A name is any
/// the dialect can write in its `USE`. A run that gets past it sets it to where it ends, so it is
/// `Busy` while the tab runs.
fn set_database(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let tab = match p::opt_tab_id(params, "tab")? {
        Some(id) => match s.state.tab(id) {
            Some(Tab::Query(q)) => q.id,
            Some(Tab::Table(_)) => return Err(bad(format!("tab {} is not a query tab", id.0))),
            None => return Err(bad(format!("no tab {} is open", id.0))),
        },
        None => match s.state.active_tab() {
            Some(Tab::Query(q)) => q.id,
            _ => return Err(disabled(NO_QUERY_TAB)),
        },
    };
    let dialect = s.state.dialect(Which::Tab(tab));
    let database = match params.get("database") {
        Some(Json::Null) => None,
        Some(Json::String(name)) if !name.is_empty() => {
            dialect.use_database(name).map_err(|e| bad(format!("`database`: {e}")))?;
            Some(name.clone())
        }
        _ => return Err(bad("`database` is required: a database's name, or null for none")),
    };
    let Some(Tab::Query(q)) = s.state.tab_mut(tab) else { return Err(disabled(NO_QUERY_TAB)) };
    if q.running.is_some() {
        return Err(EngineError::Busy(running_in(&q.title)));
    }
    q.database = database;
    Ok(Outcome::Done(json!({ "tab": tab, "database": q.database })))
}

/// `{index}` → `{tab, shown, result}`: the grid shows result `index` of the last run.
fn show_result(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let index = p::usize(params, "index")?;
    let tab = match s.state.active_tab() {
        Some(Tab::Query(q)) => q.id,
        _ => return Err(disabled(NO_QUERY_TAB)),
    };
    let Some(Tab::Query(q)) = s.state.tab_mut(tab) else { return Err(disabled(NO_QUERY_TAB)) };
    let Some(outcome) = q.results.get(index) else {
        return Err(bad(match q.results.len() {
            0 => "`index`: the tab has no results".to_owned(),
            n => format!("`index` must be below {n}, the number of results"),
        }));
    };
    let result = outcome_json(index, outcome);
    let selection = fitting(q.selection, Some(outcome.result.as_ref()));
    q.shown = index;
    q.selection = selection;
    Ok(Outcome::Done(json!({ "tab": tab, "shown": index, "result": result })))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dbee_config::{SshAuth, SshConfig};
    use dbee_sql::Dialect;
    use dbee_ssh::{AgentSource, SshEnv};

    use super::*;
    use crate::SessionOptions;
    use crate::cmd::LOST;

    #[test]
    fn server_positions_become_byte_offsets() {
        let text = "select 1;\nselect 'é🐝', nope";
        let range = 10..text.len();
        let at = |p: u32| byte_position(text, &range, p);
        // 1-based characters of the statement, counted after the multibyte ones.
        assert_eq!(at(1), Some(10));
        assert_eq!(at(14), text.find("nope"));
        // At the end of the input, and past it, is the statement's end.
        let chars = text[range.clone()].chars().count() as u32;
        assert_eq!(at(chars + 1), Some(text.len()));
        assert_eq!(at(u32::MAX), Some(text.len()));
        // 0 is no position; a range that is not in the text has none.
        assert_eq!(at(0), None);
        assert_eq!(byte_position(text, &(5..999), 1), None);
        assert_eq!(byte_position(text, &(19..24), 1), None, "not on a character boundary");
    }

    const T: Duration = Duration::from_secs(30);

    #[test]
    fn a_dead_tunnel_marks_the_session_lost() {
        let db = dbee_testkit::require_testdb!();
        let home = dbee_testkit::temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
        let ssh = SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None };
        let options = SessionOptions { ssh, drivers: dbee_drivers::all(), ..SessionOptions::default() };
        let mut s = Session::new(options).unwrap();
        let pg14 = db.server("pg14").unwrap();
        let config = ConnectionConfig {
            host: pg14.name.clone(),
            port: pg14.service_port,
            user: "postgres".into(),
            database: "dbee".into(),
            ssh: Some(SshConfig {
                host: "127.0.0.1".into(),
                port: db.sshd.port,
                user: db.sshd.user.clone(),
                auth: SshAuth::Key { path: None },
                ..SshConfig::default()
            }),
            ..ConnectionConfig::default()
        };
        s.run_blocking("connection.open", json!({ "config": config, "password": db.password }), T).unwrap();
        s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
        s.run_blocking(RUN, json!({}), T).unwrap();
        // The SSH connection ends: the test closes the session's own tunnel.
        let tunnel = s.state.connection(Which::Active).and_then(|c| c.tunnel.clone()).unwrap();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tunnel.close());
        assert!(tunnel.is_closed());
        let e = s.run_blocking(RUN, json!({}), T).unwrap_err();
        assert_eq!((e.code(), e.data()["action"].as_str()), ("connectionLost", Some("connection.reconnect")), "{e}");
        assert_eq!(s.inspect()["connection"]["lost"], true);
        assert_eq!(s.execute(RUN, json!({})).unwrap_err().data()["why"], LOST);
        // A reconnect opens a new tunnel, and the next run a query connection through it.
        s.run_blocking("connection.reconnect", json!({}), T).unwrap();
        s.run_blocking(RUN, json!({}), T).unwrap();
    }

    #[tokio::test]
    async fn one_byte_budget_for_the_whole_run() {
        let db = dbee_testkit::require_testdb!();
        let server = db.server("pg14").unwrap();
        let config = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: server.port,
            user: "postgres".into(),
            database: "dbee".into(),
            ..ConnectionConfig::default()
        };
        let driver = driver_for(&dbee_drivers::all(), config.kind).unwrap();
        let via = || Via::Connect {
            driver: Arc::clone(&driver),
            config: Box::new(config.clone()),
            password: Some(db.password.clone()),
            tunnel: None,
            allowed_dirs: None,
            browse_tls: None,
        };
        let text = "select repeat('x', 60) as a; select repeat('y', 600) as b; select 'z' as c".to_owned();
        let ranges = lexer::statements(Dialect::Postgres, &text);
        let kept = |ran: &Ran| ran.outcomes.iter().map(|o| (o.result.rows.len(), o.result.rows_seen, o.result.complete)).collect::<Vec<_>>();
        let setup = || RunSetup { use_database: None, dialect: Dialect::Postgres, current: CurrentDatabase::default() };
        let whole = run_statements(via(), text.clone(), ranges.clone(), setup(), Arc::new(Stop::default()), RUN_LIMITS.max_bytes).await;
        assert_eq!(kept(&whole), [(1, 1, true), (1, 1, true), (1, 1, true)]);
        let held = |i: usize| dbee_driver::result_cost(&whole.outcomes[i].result);
        // The first result, the second's column and the third: the second's row does not fit in
        // what the first left, and the third does in what the second's column left.
        let second_column = dbee_driver::column_cost(&whole.outcomes[1].result.columns[0]);
        let ran = run_statements(via(), text.clone(), ranges.clone(), setup(), Arc::new(Stop::default()), held(0) + second_column + held(2)).await;
        assert!(ran.failure.is_none());
        assert_eq!(kept(&ran), [(1, 1, true), (0, 1, false), (1, 1, true)]);
        // Spent by the first result: the later ones keep nothing.
        let ran = run_statements(via(), text, ranges, setup(), Arc::new(Stop::default()), held(0)).await;
        assert_eq!(kept(&ran), [(1, 1, true), (0, 1, false), (0, 1, false)]);
        assert_eq!(RUN_LIMITS, QUERY_LIMITS, "a whole run gets the query editor's caps");
    }
}

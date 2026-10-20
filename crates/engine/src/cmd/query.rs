//! Query tabs: `query.run`, `query.cancel` and `query.showResult`.
//!
//! **Running** is a job on the query connection, which is not the browse connection: a `BEGIN`
//! left open in a query tab never reaches table pages or an Apply. The job opens the connection
//! when the session has none (the first run, after a reconnect, or after it was lost), with the
//! settings, password and tunnel of the open connection, so the query connection of a read-only
//! connection is read-only too. It runs the statements `dbee_sql::lexer` splits from the tab text
//! (all of them, or the one under the cursor) one by one, each in auto-commit, and stops at the
//! first error. Every result is kept ([`QueryTab::results`]) and the last one is shown. One run
//! goes at a time in a session (`Busy`): the query connection runs one statement at a time, and
//! a cancel request hits whatever statement runs on it.
//!
//! **Stopping.** `query.cancel`, closing the running tab, `connection.close` and a reconnect stop
//! the run through its [`Stop`]. The job then sends the cancel request itself, through the query
//! connection's `Canceller`, and waits for the statement to end (so the request cannot reach a
//! later run), and it starts no further statement. A stopped run ends with code `cancelled`. The
//! job is never dropped or aborted.
//!
//! **Errors** keep the server's message, detail and hint ([`QueryError`]); the position the
//! server gives (characters into the statement) becomes a byte offset into the tab text.
//!
//! **Stale results** (see [`super`]): a run whose tab closed, or whose connection was closed or
//! replaced, is `{"ignored": true}` and changes nothing.
//!
//! None of these commands is journaled: a replay would run the SQL again, writes included. Logs
//! carry command ids and job labels, never SQL text.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use dbee_config::ConnectionConfig;
use dbee_core::ResultSet;
use dbee_pg::{PgConnection, PgError, QUERY_LIMITS, Target};
use dbee_sql::lexer;
use dbee_ssh::Tunnel;
use serde_json::json;
use tokio::sync::Notify;

use super::connection::db_error;
use super::{CommandSpec, LOST, NO_QUERY_TAB, NOT_CONNECTED, active_query, ignored, needs_db, p};
use crate::state::{QueryError, QueryTab, Selection, StatementOutcome};
use crate::{Apply, EngineError, JobId, Json, Outcome, Session, Tab, TabId};

const RUN: &str = "query.run";
const CANCEL: &str = "query.cancel";
const SHOW_RESULT: &str = "query.showResult";

/// Why `query.run` does not start: the text holds no statement.
pub const NOTHING_TO_RUN: &str = "there is no statement to run";

/// The error of a statement holding a NUL character, which the protocol cannot carry.
const NUL: &str = "the statement holds a NUL character (U+0000), which Postgres cannot take";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: RUN,
        label: "Run",
        menu: &[],
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
        menu: &[],
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
];

/// A query tab is active, on a connection that is not lost.
fn can_run(s: &Session) -> Result<(), String> {
    needs_db(s)?;
    active_query(s)
}

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { id: String::new(), msg: msg.into() }
}

fn disabled(why: &str) -> EngineError {
    EngineError::Disabled { id: String::new(), why: why.to_string() }
}

// ---- stopping a run --------------------------------------------------------------------------

/// Asks a query run to stop. Set once and for good; the job looks before each statement and
/// waits on it while one runs.
#[derive(Default)]
pub(crate) struct Stop {
    set: AtomicBool,
    notify: Notify,
}

impl Stop {
    pub(crate) fn stop(&self) {
        self.set.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_set(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }

    /// Returns once [`Stop::stop`] was called, also when that was before.
    pub(crate) async fn stopped(&self) {
        loop {
            let notified = self.notify.notified();
            let mut notified = std::pin::pin!(notified);
            // Waiting from here on, so a stop after the check below wakes it.
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop").field("set", &self.is_set()).finish()
    }
}

/// The session's query run in progress (`Connected::query_run`).
#[derive(Clone, Debug)]
pub(crate) struct QueryRun {
    pub(crate) job: JobId,
    pub(crate) stop: Arc<Stop>,
}

/// Stops the session's query run when it is `job`.
pub(crate) fn stop_run(s: &Session, job: JobId) {
    if let Some(run) = s.state.connected.as_ref().and_then(|c| c.query_run.as_ref()).filter(|run| run.job == job) {
        run.stop.stop();
    }
}

/// Why another run cannot start now: one is in progress.
fn busy(s: &Session) -> Option<String> {
    let run = s.state.connected.as_ref()?.query_run.as_ref()?;
    if !s.jobs.is_running(run.job) {
        return None;
    }
    let title = s.state.tabs.iter().find_map(|t| match t {
        Tab::Query(q) if q.running == Some(run.job) => Some(q.title.as_str()),
        _ => None,
    });
    Some(match title {
        Some(title) => format!("a query is running in {title}"),
        None => "a cancelled query is still stopping".into(),
    })
}

// ---- the job ---------------------------------------------------------------------------------

/// The connection a run uses.
enum Via {
    /// The session's query connection.
    Open(Arc<PgConnection>),
    /// None yet: the job opens one as the browse connection was opened.
    Connect { config: Box<ConnectionConfig>, password: Option<String>, tunnel: Option<Arc<Tunnel>> },
}

/// Why a run stopped early: at statement `statement` (0-based), with the byte offset in the tab
/// text the error points at.
struct Failure {
    statement: usize,
    error: PgError,
    position: Option<usize>,
}

/// What a run brings back.
struct Ran {
    /// The connection the statements ran on and whether the job opened it; `None` when it could
    /// not be opened, or the run stopped first.
    conn: Option<(Arc<PgConnection>, bool)>,
    outcomes: Vec<StatementOutcome>,
    failure: Option<Failure>,
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

async fn run_statements(via: Via, text: String, ranges: Vec<Range<usize>>, stop: Arc<Stop>) -> Ran {
    let mut ran = Ran { conn: None, outcomes: Vec::with_capacity(ranges.len()), failure: None };
    let stopped = |statement| Some(Failure { statement, error: PgError::Cancelled, position: None });
    if stop.is_set() {
        ran.failure = stopped(0);
        return ran;
    }
    let (conn, opened) = match via {
        Via::Open(conn) => (conn, false),
        Via::Connect { config, password, tunnel } => {
            let target = Target { config: &config, password: password.as_deref(), tunnel };
            // A connect that is stopped is dropped: nothing ran on it yet.
            tokio::select! {
                connected = dbee_pg::connect(target) => match connected {
                    Ok(conn) => (Arc::new(conn), true),
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
    for (i, range) in ranges.into_iter().enumerate() {
        if stop.is_set() {
            ran.failure = stopped(i);
            break;
        }
        let Some(sql) = text.get(range.clone()) else {
            ran.failure = Some(Failure { statement: i, error: PgError::Other("the statement is not in the text".into()), position: None });
            break;
        };
        if let Some(at) = sql.find('\0') {
            ran.failure = Some(Failure { statement: i, error: PgError::Other(NUL.into()), position: range.start.checked_add(at) });
            break;
        }
        match run_one(&conn, sql, &stop).await {
            Ok(result) => ran.outcomes.push(StatementOutcome { range, result: Arc::new(result) }),
            Err(error) => {
                let position = match &error {
                    PgError::Server { position: Some(chars), .. } => byte_position(&text, &range, *chars),
                    _ => None,
                };
                ran.failure = Some(Failure { statement: i, error, position });
                break;
            }
        }
    }
    ran.conn = Some((conn, opened));
    ran
}

/// Runs one statement. When the run is stopped meanwhile, the cancel request goes out and the
/// statement is waited for (it then usually ends with `Cancelled`).
async fn run_one(conn: &PgConnection, sql: &str, stop: &Stop) -> Result<ResultSet, PgError> {
    let mut running = std::pin::pin!(dbee_pg::query::run(conn, sql, QUERY_LIMITS));
    tokio::select! {
        result = &mut running => result,
        () = stop.stopped() => {
            let canceller = conn.canceller();
            let (sent, result) = futures_util::future::join(canceller.cancel(), &mut running).await;
            if sent.is_err() {
                log::info!("query: the cancel request was not delivered");
            }
            result
        }
    }
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

/// The tab as `inspect()` shows it: `{tab, title, text, running, results, shown, error}`.
pub(crate) fn summary(q: &QueryTab) -> Json {
    json!({
        "tab": q.id,
        "title": q.title,
        "text": q.text,
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

fn query_error(code: &'static str, f: &Failure) -> QueryError {
    let (message, detail, hint) = match &f.error {
        PgError::Server { code, message, detail, hint, .. } => (format!("{message} (SQLSTATE {code})"), detail.clone(), hint.clone()),
        other => (other.to_string(), None, None),
    };
    QueryError { code, message, detail, hint, position: f.position, statement: f.statement }
}

fn apply_run(s: &mut Session, tab: TabId, generation: &Weak<PgConnection>, ran: Ran) -> Result<Json, EngineError> {
    let me = s.applying_job();
    // The session's connection is still the one the run started on: not closed, not replaced.
    let Some(c) = s.state.connected.as_mut().filter(|c| Weak::as_ptr(generation) == Arc::as_ptr(&c.browse)) else { return Ok(ignored()) };
    // The run is over (clearing the marker of a job that ended is not applying its result).
    if me.is_some() && c.query_run.as_ref().map(|run| run.job) == me {
        c.query_run = None;
    }
    let fresh = match &ran.conn {
        // Opened by the job: it becomes the session's query connection.
        Some((conn, true)) if c.query.is_none() => Some(Arc::clone(conn)),
        Some((conn, false)) if !c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, conn)) => return Ok(ignored()),
        _ => None,
    };
    if !matches!(s.state.tab(tab), Some(Tab::Query(q)) if me.is_some() && q.running == me) {
        return Ok(ignored());
    }
    if let Some(conn) = fresh
        && let Some(c) = s.state.connected.as_mut()
    {
        c.query = Some(conn);
    }
    let Ran { conn, outcomes, failure } = ran;
    let error = failure.map(|f| {
        let e = match &conn {
            // A lost query connection is let go; the next run opens another.
            Some((conn, _)) => s.db_failed(conn, &f.error),
            None => db_error(&f.error),
        };
        (query_error(e.code(), &f), run_error(e, &f))
    });
    let Some(Tab::Query(q)) = s.state.tab_mut(tab) else { return Ok(ignored()) };
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
/// `current` runs only the statement under `cursor`, a byte offset into the text (past the end:
/// the last statement). A run that stops early fails with the error, and the results before it
/// are kept.
fn run(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let current = p::bool_or(params, "current", false)?;
    let cursor = p::opt_usize(params, "cursor")?;
    let (tab, text) = match s.state.active_tab() {
        Some(Tab::Query(q)) => (q.id, q.text.clone()),
        _ => return Err(disabled(NO_QUERY_TAB)),
    };
    if let Some(why) = busy(s) {
        return Err(EngineError::Busy(why));
    }
    let ranges = if current {
        let cursor = cursor.ok_or_else(|| bad("`cursor` is required with `current` (a byte offset into the text)"))?;
        lexer::statement_at(&text, cursor).into_iter().collect()
    } else {
        lexer::statements(&text)
    };
    if ranges.is_empty() {
        return Err(disabled(NOTHING_TO_RUN));
    }
    let c = s.state.connected.as_mut().ok_or_else(|| disabled(NOT_CONNECTED))?;
    if c.lost {
        return Err(disabled(LOST));
    }
    if c.query.as_ref().is_some_and(|q| q.is_closed()) {
        log::info!("query: the query connection has closed; the run opens a new one");
        c.query = None;
    }
    let via = match &c.query {
        Some(conn) => Via::Open(Arc::clone(conn)),
        None => Via::Connect { config: Box::new(c.config.clone()), password: c.credentials.password.clone(), tunnel: c.tunnel.clone() },
    };
    let generation = Arc::downgrade(&c.browse);
    let stop = Arc::new(Stop::default());
    let job_stop = Arc::clone(&stop);
    let job = s.spawn_job(RUN, async move {
        let ran = run_statements(via, text, ranges, job_stop).await;
        let apply: Apply = Box::new(move |s: &mut Session| apply_run(s, tab, &generation, ran));
        apply
    });
    if let Some(c) = s.state.connected.as_mut() {
        c.query_run = Some(QueryRun { job, stop });
    }
    if let Some(Tab::Query(q)) = s.state.tab_mut(tab) {
        q.running = Some(job);
    }
    Ok(Outcome::Started(job))
}

/// `{}` → `{tab, sent}`: stops the active tab's run (see the module docs); `sent: false` when the
/// tab runs nothing.
fn cancel(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let (tab, running) = match s.state.active_tab() {
        Some(Tab::Query(q)) => (q.id, q.running),
        _ => return Err(disabled(NO_QUERY_TAB)),
    };
    let run = s.state.connected.as_ref().and_then(|c| c.query_run.as_ref());
    let sent = match (running, run) {
        (Some(job), Some(run)) if run.job == job && s.jobs.is_running(job) => {
            run.stop.stop();
            true
        }
        _ => false,
    };
    Ok(Outcome::Done(json!({ "tab": tab, "sent": sent })))
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

    use super::*;

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

    #[tokio::test]
    async fn stop_is_seen_before_and_while_waiting() {
        let stop = Arc::new(Stop::default());
        assert!(!stop.is_set());
        let waiting = tokio::spawn({
            let stop = Arc::clone(&stop);
            async move { stop.stopped().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.stop();
        tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
        // Once set, it stays set, and waiting returns at once.
        assert!(stop.is_set());
        tokio::time::timeout(Duration::from_secs(5), stop.stopped()).await.unwrap();
    }
}

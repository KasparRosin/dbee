//! Query tabs: `query.run`, `query.cancel`, `query.showResult`, `query.setDatabase`, `query.commit`
//! and `query.rollback`.
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
//! for the whole run: each statement gets what the ones before left, so a result whose rows do not
//! fit keeps those that do and says it is cut, and once what is left does not hold a result's
//! columns, that statement ends the run in a `tooLarge` error and the connection goes on). One run goes at a time on a
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
//! statement. A stopped run ends with code `cancelled`. The job is never dropped or aborted. Only
//! dbee's own cancel reads so: a statement the server stopped while dbee had sent no cancel for it
//! (a timeout, another session's cancel) fails with the server's code and words, its `meaning`
//! `interrupted`, and one that ended before the cancel reached it keeps its result.
//!
//! **The database a run starts in.** On a dialect with `USE`, a tab with a database of its own
//! makes it current first, unless the query connection is known to be there already (a `USE`
//! also clears MySQL's warnings, which a `SHOW WARNINGS` of its own run would then miss). The
//! engine keeps the database the query connection is in ([`CurrentDatabase`]): the settings'
//! when the connection opens, each `USE` it sends, and where the driver says each statement that
//! succeeded left it (`ResultSet::database_after`; MySQL's server reports it in its OK packets).
//! It sends no statement of its own to find out. A statement the server answered with an error
//! moves nothing; one that ended any other way (cancelled or interrupted, or lost) may have, so the database is
//! no longer known, as when the driver cannot tell: it is never guessed. After a run
//! that got past its `USE`, the tab's database becomes that one, so a `USE` in the text sticks to
//! the tab; the next run's `USE` keeps every tab in its own. A tab with none starts where the
//! connection is ([`crate::Connected::starts_in`]).
//!
//! **The transaction.** All of a connection's query tabs share the query session, so its
//! transaction belongs to the connection ([`Connected::transaction`]). After each run, commit and
//! rollback the job asks the driver whether the session is in a transaction, as the server reported
//! it ([`Connection::transaction`]): never from the SQL text, so a `BEGIN` in a script, MySQL's
//! `SET autocommit = 0` and a statement that commits on its own all read as the server says. A run
//! that finds the session idle and leaves it in a transaction makes its tab the owner; a run of
//! another tab keeps the owner, and closing the owner with `force` leaves the transaction without
//! one. A run whose result was not applied leaves it unknown; a lost or replaced query connection
//! ends it. `query.commit` and `query.rollback` send `COMMIT` or `ROLLBACK` on the query session
//! from any of its tabs, as a job that counts as the connection's run; with no transaction open
//! they send nothing and say so. Table tabs run on the browse connection and never touch it.
//!
//! **Closing over a transaction.** Closing the owner tab while the transaction is open or failed,
//! or the connection while it has one in any state, asks first ([`end_question`]). Run again with
//! `end`, the close sends the same `COMMIT` or `ROLLBACK` as a job and closes once it went through
//! ([`end_then_close`]). The question is asked from the state the last finished run left; a run
//! still going is inside the transaction, so only a rollback ends it then: the close stops the
//! run, waits for its result to come in first, and looks at the transaction it left before it
//! ends it.
//!
//! **Errors** keep the server's message, code, meaning, detail and hint ([`QueryError`]); the
//! position the server gives (characters into the statement) becomes a byte offset into the tab
//! text.
//!
//! **Time.** Each result and the error say how long their statement took (`ms`): whole
//! milliseconds from sending the statement to its result, its failure or its stop, not the run's
//! time. A run that stopped before its statement was sent (connecting, the tab's `USE`, a NUL in
//! the text, a stop between statements) gives the error no time (`null`).
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
use std::time::Instant;

use dbee_config::ConnectionConfig;
use dbee_core::ResultSet;
use dbee_driver::{Connection, DbError, Driver, Limits, Purpose, QUERY_LIMITS, Target, TlsState, TransactionState};
use dbee_sql::{Dialect, lexer};
use dbee_ssh::Tunnel;
use serde_json::json;

use super::connection::{db_error, driver_for};
use super::{CommandSpec, NO_QUERY_TAB, NOT_CONNECTED, active_query, bad, disabled, ignored, needs_db, p};
use crate::jobs::{self, Sent, Stop};
use crate::state::{Connected, CurrentDatabase, QueryError, QueryRun, QueryTab, Selection, StatementOutcome, Transaction};
use crate::turns::InTurn;
use crate::{Apply, ConnectionId, EndAbout, EngineError, JobId, Json, Need, Outcome, Running, Session, Tab, TabId, Which};

const RUN: &str = "query.run";
const CANCEL: &str = "query.cancel";
const SHOW_RESULT: &str = "query.showResult";
const SET_DATABASE: &str = "query.setDatabase";
const COMMIT: &str = "query.commit";
const ROLLBACK: &str = "query.rollback";

/// What `query.commit` and `query.rollback` say when the query session has no transaction open.
pub const NO_TRANSACTION: &str = "no transaction is open";

/// What `query.commit` says of a Postgres transaction that had failed: the server ends it with a
/// rollback.
pub const FAILED_COMMIT: &str = "the transaction had failed, so the server rolled it back: nothing was committed";

/// What `query.rollback` says when the query connection was lost: its transaction ended with it.
pub const ENDED_WITH_SESSION: &str = "the query connection was lost, and the server rolled its transaction back";

/// Why `query.commit` fails when the query connection was lost.
pub const COMMIT_SESSION_LOST: &str =
    "the query connection was lost, and the server rolled its transaction back: nothing was committed. Run again to open a new one";

/// Why a close refuses `end: "commit"` while a query run goes on its connection.
pub const ONLY_ROLLBACK: &str = "a statement is still running in the transaction; it can only be rolled back";

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
        id: COMMIT,
        label: "Commit",
        menu: &[],
        shortcut: None,
        params: "{tab?}",
        enabled: needs_db,
        run: commit,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: ROLLBACK,
        label: "Rollback",
        menu: &[],
        shortcut: None,
        params: "{tab?}",
        enabled: needs_db,
        run: rollback,
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
    let label = s.jobs.running().find(|(job, _)| *job == run.job).map(|(_, label)| label);
    Some(match (title, label) {
        (Some(title), _) => running_in(title),
        (None, Some(COMMIT)) => "a COMMIT is running".into(),
        (None, Some(ROLLBACK)) => "a ROLLBACK is running".into(),
        (None, Some("tab.close" | "connection.close")) => "a COMMIT or ROLLBACK is running before a close".into(),
        (None, _) => "a cancelled query is still stopping".into(),
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
    /// How long the statement ran before it failed or stopped; `None` when it was not sent.
    ms: Option<u64>,
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
    /// Whether the connection's session is in a transaction after the run, as the driver says;
    /// `None` when no connection was reached.
    transaction: Option<TransactionState>,
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
/// at most `budget` bytes held: each statement gets what the ones before left. Once a statement's
/// rows do not fit its result is cut (and says so); once its columns do not fit it fails, and the
/// run ends there ([`DbError::TooLarge`]). A statement without columns still runs.
async fn run_statements(via: Via, text: String, ranges: Vec<Range<usize>>, setup: RunSetup, stop: Arc<Stop>, budget: usize) -> Ran {
    let mut ran = Ran {
        conn: None,
        outcomes: Vec::with_capacity(ranges.len()),
        failure: None,
        current: setup.current.clone(),
        past_use: false,
        transaction: None,
    };
    let stopped = |statement| Some(Failure { statement, error: DbError::Cancelled, position: None, ms: None });
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
                        ran.failure = Some(Failure { statement: 0, error: DbError::Other(QUERY_TLS_LOST.into()), position: None, ms: None });
                        return ran;
                    }
                    Ok(conn) => (conn, true),
                    Err(error) => {
                        ran.failure = Some(Failure { statement: 0, error, position: None, ms: None });
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
                ran.failure = Some(Failure { statement: 0, error, position: None, ms: None });
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
                ran.failure =
                    Some(Failure { statement: i, error: DbError::Other("the statement is not in the text".into()), position: None, ms: None });
                break;
            };
            if let Some(at) = setup.dialect.misplaced_nul(sql) {
                ran.failure = Some(Failure { statement: i, error: DbError::Other(NUL.into()), position: range.start.checked_add(at), ms: None });
                break;
            }
            let limits = Limits { max_rows: if left == 0 { 0 } else { RUN_LIMITS.max_rows }, max_bytes: left };
            let sent = Instant::now();
            let ran_one = run_one(conn.as_ref(), sql, limits, &stop).await;
            let ms = ms_since(sent);
            match ran_one {
                Ok(result) => {
                    ran.current = ran.current.after(&result.database_after, moves);
                    left = left.saturating_sub(dbee_driver::result_cost(&result));
                    ran.outcomes.push(StatementOutcome { range, result: Arc::new(result), ms });
                }
                Err(error) => {
                    ran.current = ran.current.after_failure(&error, moves);
                    let position = match &error {
                        DbError::Server(s) => s.position.and_then(|chars| byte_position(&text, &range, chars)),
                        _ => None,
                    };
                    ran.failure = Some(Failure { statement: i, error, position, ms: Some(ms) });
                    break;
                }
            }
        }
    }
    ran.transaction = Some(conn.transaction().await);
    ran.conn = Some((conn, opened));
    ran
}

/// The whole milliseconds since `start`, at most `u64::MAX`.
fn ms_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Runs one statement. When the run is stopped meanwhile, the cancel request goes out until the
/// statement has ended ([`jobs::stoppable`]); it then usually ends with `Cancelled`, unless it
/// ended first.
async fn run_one(conn: &dyn Connection, sql: &str, limits: Limits, stop: &Stop) -> Result<ResultSet, DbError> {
    jobs::stoppable(conn.canceller(), conn.run(sql, limits), stop).await
}

// ---- applying a run --------------------------------------------------------------------------

/// The result as `query.run`, `query.showResult` and `inspect()` show it: `{statement, start,
/// end, columns, rows, rowsSeen, complete, rowsAffected, ms}`, `rows` being the rows kept and
/// `ms` how long the statement took.
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
        "ms": o.ms,
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

/// `selection` as far as it is kept on `result`'s grid ([`Selection::kept`]).
fn fitting(selection: Option<Selection>, result: Option<&ResultSet>) -> Option<Selection> {
    let result = result?;
    selection.and_then(|sel| sel.kept(result.rows.len(), result.columns.len()))
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
    QueryError { code: e.code(), message, server_code, meaning, detail, hint, position: f.position, statement: f.statement, ms: f.ms }
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
    let tab_open = s.state.tab(tab).is_some();
    // The connection is still the one the run started on: not closed, not replaced.
    let Some(c) = s.state.connection_mut(Which::Id(id)).filter(|c| same_connection(generation, &c.browse)) else { return Ok(ignored()) };
    // The run is over (clearing the marker of a job that ended is not applying its result).
    if me.is_some() && c.query_run.as_ref().map(|run| run.job) == me {
        c.query_run = None;
    }
    let Ran { conn, outcomes, failure, current, past_use, transaction } = ran;
    match &conn {
        // Opened by the job: it becomes the session's query connection, with a new session. Without
        // the tab, a lost one is dropped: no open tab ever ran on it.
        Some((used, true)) if c.query.is_none() && (for_tab || !used.is_closed()) => {
            c.query = Some(Arc::clone(used));
            c.transaction = None;
        }
        Some((used, false)) if !c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, used)) => return Ok(ignored()),
        _ => {}
    }
    if let Some((used, _)) = &conn
        && c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, used))
    {
        c.query_database = current.clone();
        if let Some(state) = transaction {
            c.transaction = Transaction::after(c.transaction, state, tab_open.then_some(tab));
        }
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
        let f = Failure { statement: 0, error: DbError::ConnectionLost, position: None, ms: None };
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
    let tab = query_tab(s, params)?;
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

/// The query tab `params` names in `tab`, else the active one.
fn query_tab(s: &Session, params: &Json) -> Result<TabId, EngineError> {
    match p::opt_tab_id(params, "tab")? {
        Some(id) => match s.state.tab(id) {
            Some(Tab::Query(q)) => Ok(q.id),
            Some(Tab::Table(_)) => Err(bad(format!("tab {} is not a query tab", id.0))),
            None => Err(bad(format!("no tab {} is open", id.0))),
        },
        None => match s.state.active_tab() {
            Some(Tab::Query(q)) => Ok(q.id),
            _ => Err(disabled(NO_QUERY_TAB)),
        },
    }
}

/// What `query.commit` and `query.rollback` send, and what a close with `end` sends before it
/// closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    Commit,
    Rollback,
}

impl End {
    /// A close's `end` param: `"commit"` or `"rollback"`; `None` when not given.
    pub(crate) fn param(params: &Json) -> Result<Option<End>, EngineError> {
        match p::opt_str(params, "end")? {
            None => Ok(None),
            Some("commit") => Ok(Some(End::Commit)),
            Some("rollback") => Ok(Some(End::Rollback)),
            Some(_) => Err(bad("`end` must be \"commit\" or \"rollback\"")),
        }
    }

    /// As `end` names it, and a close's `ended` says it.
    fn name(self) -> &'static str {
        match self {
            End::Commit => "commit",
            End::Rollback => "rollback",
        }
    }

    fn id(self) -> &'static str {
        match self {
            End::Commit => COMMIT,
            End::Rollback => ROLLBACK,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            End::Commit => "COMMIT",
            End::Rollback => "ROLLBACK",
        }
    }
}

fn commit(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    end(s, params, End::Commit)
}

fn rollback(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    end(s, params, End::Rollback)
}

/// `{tab?}` → `{tab, sent, transaction, note?}`: ends the query session's transaction (see the
/// module docs), asked from query tab `tab` (default: the active one), any of the connection's.
/// `transaction` is the state after it. With none open it sends nothing: `sent: false` and
/// `note` [`NO_TRANSACTION`], at once. A commit of a Postgres transaction that had failed is a
/// rollback, which `note` says ([`FAILED_COMMIT`]). On a query connection found lost, a commit
/// fails with `connectionLost` and a rollback has nothing left to do ([`ENDED_WITH_SESSION`]).
fn end(s: &mut Session, params: &Json, end: End) -> Result<Outcome, EngineError> {
    let tab = query_tab(s, params)?;
    let id = s.state.connection(Which::Tab(tab)).map(|c| c.id).ok_or_else(|| disabled(NOT_CONNECTED))?;
    if let Some(why) = busy(s, id) {
        return Err(EngineError::Busy(why));
    }
    s.browse(Which::Id(id))?;
    let c = s.state.connection_mut(Which::Id(id)).ok_or_else(|| disabled(NOT_CONNECTED))?;
    let nothing = |note: &str| Ok(Outcome::Done(json!({ "tab": tab, "sent": false, "transaction": null, "note": note })));
    let ending = match ending(c, Vec::new()) {
        Ending::Lost { conn, .. } => {
            let_go(c, &conn);
            return match end {
                End::Commit => Err(commit_lost()),
                End::Rollback => nothing(ENDED_WITH_SESSION),
            };
        }
        Ending::Nothing(note) => return nothing(note),
        send => send,
    };
    let job = spawn_end(s, end.id(), end, ending, move |_, done| {
        let Some(done) = done? else { return Ok(ignored()) };
        let mut result = json!({ "tab": tab, "sent": done.sent, "transaction": done.transaction });
        if let Some(note) = done.note {
            result["note"] = json!(note);
        }
        Ok(result)
    });
    Ok(Outcome::Started(job))
}

/// `end` of a close of connection `id` (`None` for a query tab that has none): a job that ends the
/// query session's transaction as [`end`] does, then runs `close` on the caller's thread. Its
/// result is `close`'s plus `ended` (`"commit"` or `"rollback"`) and `note?` ([`NO_TRANSACTION`]
/// when there was none, [`FAILED_COMMIT`], [`ENDED_WITH_SESSION`]). When the end fails, the job
/// fails with its error and nothing closes. `{"ignored": true}` when the connection was closed or
/// replaced meanwhile, or `close` finds nothing left to close (`None`).
///
/// Query runs going on the connection are inside the transaction: a commit is refused
/// ([`ONLY_ROLLBACK`]), and a rollback stops them first, then waits for their results before it
/// looks at the transaction they left and ends it. Another commit or rollback going is `Busy`.
pub(crate) fn end_then_close(
    s: &mut Session,
    id: Option<ConnectionId>,
    end: End,
    label: &'static str,
    close: impl FnOnce(&mut Session) -> Option<Json> + Send + 'static,
) -> Result<Outcome, EngineError> {
    let runs = id.map(|id| runs_going(s, id)).unwrap_or_default();
    if let Some(id) = id
        && let Some(why) = busy(s, id)
        && !s.state.connection(Which::Id(id)).and_then(|c| c.query_run.as_ref()).is_some_and(|run| runs.iter().any(|(job, _)| *job == run.job))
    {
        return Err(EngineError::Busy(why));
    }
    if !runs.is_empty() && end == End::Commit {
        return Err(bad(ONLY_ROLLBACK));
    }
    let mut after = Vec::new();
    for (job, _) in &runs {
        s.jobs.stop(*job);
        after.extend(s.jobs.sent(*job));
    }
    let ending = match id.and_then(|id| s.state.connection(Which::Id(id))) {
        Some(c) => ending(c, after),
        None => Ending::Nothing(NO_TRANSACTION),
    };
    let job = spawn_end(s, label, end, ending, move |s, done| {
        let Some(done) = done? else { return Ok(ignored()) };
        let Some(mut result) = close(s) else { return Ok(ignored()) };
        result["ended"] = json!(end.name());
        if let Some(note) = done.note {
            result["note"] = json!(note);
        }
        Ok(result)
    });
    Ok(Outcome::Started(job))
}

/// The question a close asks before it drops connection `id`'s transaction, in `state`
/// ([`Need::EndTransaction`]): `question` is the close's own (`Close Query 1?`), `name` what it
/// closes, `lost` what it loses besides, as its `needsConfirm` says it after the question.
pub(crate) fn end_question(
    s: &Session,
    id: ConnectionId,
    state: TransactionState,
    about: EndAbout,
    name: String,
    question: &str,
    lost: Option<String>,
) -> EngineError {
    let runs = runs_going(s, id);
    let running = runs.first().map(|(_, title)| Running { title: title.clone(), count: runs.len() });
    let why = match (&running, state, about) {
        (Some(r), _, _) => format!("{} is still running in the transaction, so it can only be rolled back.", r.title),
        (None, TransactionState::Failed, _) => "Its transaction failed, so it can only be rolled back.".to_owned(),
        (None, TransactionState::Idle | TransactionState::Unknown, _) => "dbee can't tell whether it still has a transaction open.".to_owned(),
        (None, TransactionState::Open, EndAbout::Tab) => "It opened a transaction that isn't committed yet.".to_owned(),
        (None, TransactionState::Open, EndAbout::Connection) => "It has a transaction that isn't committed yet.".to_owned(),
    };
    let message = match &lost {
        Some(lost) => format!("{question} {why} {lost}"),
        None => format!("{question} {why}"),
    };
    EngineError::Needs { need: Need::EndTransaction { state, about, name, lost, running }, message }
}

/// The query runs going on connection `id`, in tab order: each one's job and its tab's title.
fn runs_going(s: &Session, id: ConnectionId) -> Vec<(JobId, String)> {
    s.state
        .tabs
        .iter()
        .filter_map(|t| match t {
            Tab::Query(q) if q.connection == Some(id) => q.running.filter(|job| s.jobs.is_running(*job)).map(|job| (job, q.title.clone())),
            _ => None,
        })
        .collect()
}

/// Where a commit or rollback goes: open connection `id`, while its browse connection is still
/// `generation` (not closed, not replaced by a reconnect).
struct At {
    id: ConnectionId,
    generation: Weak<InTurn>,
}

/// How a commit or rollback goes, decided on the caller's thread.
enum Ending {
    /// Nothing to send, for the reason the note gives.
    Nothing(&'static str),
    /// The query connection `conn` was found lost, and its transaction with it.
    Lost { at: At, conn: Arc<dyn Connection> },
    /// Send it on the query connection `conn`, where the transaction was `before`, once the
    /// stopped runs `after` have sent their results.
    Send { at: At, conn: Arc<dyn Connection>, before: Option<TransactionState>, after: Vec<Sent> },
}

/// How a commit or rollback goes on `c`'s query session. `after` are the runs a close stopped
/// first: the transaction they leave is looked at once they have ended, so the query connection
/// is reached even when the last finished run left none.
fn ending(c: &Connected, after: Vec<Sent>) -> Ending {
    let at = At { id: c.id, generation: Arc::downgrade(&c.browse) };
    match (&c.query, c.transaction) {
        (Some(q), _) if q.is_closed() => Ending::Lost { at, conn: Arc::clone(q) },
        (Some(q), Some(t)) => Ending::Send { at, conn: Arc::clone(q), before: Some(t.state), after },
        (Some(q), None) if !after.is_empty() => Ending::Send { at, conn: Arc::clone(q), before: None, after },
        _ => Ending::Nothing(NO_TRANSACTION),
    }
}

/// Lets go of `c`'s query connection `conn`, found lost, and of its transaction; the next run opens
/// a new one.
fn let_go(c: &mut Connected, conn: &Arc<dyn Connection>) {
    if c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, conn)) {
        log::info!("query: the query connection has closed, and its transaction with it; the next run opens a new one");
        c.query = None;
        c.transaction = None;
    }
}

fn commit_lost() -> EngineError {
    EngineError::Db { code: DbError::ConnectionLost.code(), message: COMMIT_SESSION_LOST.into(), data: json!({}) }
}

/// Starts the job that ends the query session's transaction as `ending` says, labelled `label`;
/// `then` makes the job's result of what it did, on the caller's thread. A job that sends counts
/// as the connection's run.
fn spawn_end(
    s: &mut Session,
    label: &'static str,
    end: End,
    ending: Ending,
    then: impl FnOnce(&mut Session, Result<Option<EndDone>, EngineError>) -> Result<Json, EngineError> + Send + 'static,
) -> JobId {
    let sends = match &ending {
        Ending::Send { at, .. } => Some(at.id),
        Ending::Nothing(_) | Ending::Lost { .. } => None,
    };
    let stop = Arc::new(Stop::default());
    let job_stop = Arc::clone(&stop);
    let job = s.spawn_stoppable_job(label, Arc::clone(&stop), async move {
        let ran = run_end(ending, end, &job_stop).await;
        let apply: Apply = Box::new(move |s: &mut Session| {
            let done = apply_end(s, end, ran);
            then(s, done)
        });
        apply
    });
    if let Some(c) = sends.and_then(|id| s.state.connection_mut(Which::Id(id))) {
        c.query_run = Some(QueryRun { job, stop });
    }
    job
}

/// What a commit or rollback job did.
enum EndRan {
    Nothing(&'static str),
    Lost {
        at: At,
        conn: Arc<dyn Connection>,
    },
    /// Sent on `conn`: the state before it, whether it went through, and the state the driver
    /// reported after it.
    Sent {
        at: At,
        conn: Arc<dyn Connection>,
        before: Option<TransactionState>,
        sent: Result<(), DbError>,
        state: TransactionState,
    },
}

/// The job's part of `ending`. After the stopped runs, the query connection is looked at again:
/// they may have lost it, or left no transaction.
async fn run_end(ending: Ending, end: End, stop: &Stop) -> EndRan {
    let (at, conn, mut before, after) = match ending {
        Ending::Nothing(note) => return EndRan::Nothing(note),
        Ending::Lost { at, conn } => return EndRan::Lost { at, conn },
        Ending::Send { at, conn, before, after } => (at, conn, before, after),
    };
    if !after.is_empty() {
        for run in &after {
            run.wait().await;
        }
        if conn.is_closed() {
            return EndRan::Lost { at, conn };
        }
        match conn.transaction().await {
            TransactionState::Idle => return EndRan::Nothing(NO_TRANSACTION),
            state => before = Some(state),
        }
    }
    let sent = jobs::stoppable(conn.canceller(), conn.run(end.sql(), Limits { max_rows: 0, max_bytes: 0 }), stop).await.map(drop);
    let state = conn.transaction().await;
    EndRan::Sent { at, conn, before, sent, state }
}

/// What a commit or rollback did: whether it was sent, the state after it (null once it has
/// ended), and a note.
struct EndDone {
    sent: bool,
    transaction: Json,
    note: Option<&'static str>,
}

/// Applies a commit or rollback job's `ran`: the new state, then its error, if any. `None` when
/// the connection it went to was closed or replaced meanwhile, or the query connection is no
/// longer the session's: nothing changes.
fn apply_end(s: &mut Session, end: End, ran: EndRan) -> Result<Option<EndDone>, EngineError> {
    let (at, conn, sent) = match ran {
        EndRan::Nothing(note) => return Ok(Some(EndDone { sent: false, transaction: Json::Null, note: Some(note) })),
        EndRan::Lost { at, conn } => (at, conn, None),
        EndRan::Sent { at, conn, before, sent, state } => (at, conn, Some((before, sent, state))),
    };
    let me = s.applying_job();
    let Some(c) = s.state.connection_mut(Which::Id(at.id)).filter(|c| same_connection(&at.generation, &c.browse)) else { return Ok(None) };
    if me.is_some() && c.query_run.as_ref().map(|run| run.job) == me {
        c.query_run = None;
    }
    let Some((before, sent, state)) = sent else {
        let_go(c, &conn);
        return match end {
            End::Commit => Err(commit_lost()),
            End::Rollback => Ok(Some(EndDone { sent: false, transaction: Json::Null, note: Some(ENDED_WITH_SESSION) })),
        };
    };
    if !c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, &conn)) {
        return Ok(None);
    }
    c.transaction = Transaction::after(c.transaction, state, None);
    let transaction = c.transaction().map_or(Json::Null, |t| t.summary());
    if let Err(error) = sent {
        return Err(s.db_failed(at.id, &conn, &error));
    }
    let failed_commit = end == End::Commit && before == Some(TransactionState::Failed) && state == TransactionState::Idle;
    Ok(Some(EndDone { sent: true, transaction, note: failed_commit.then_some(FAILED_COMMIT) }))
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
        let text = "select 1;\nselect 'é🐙', nope";
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
        let (pg, mysql) = (db.server("pg14").unwrap(), db.server("mysql80").unwrap());
        let servers = [
            (pg.port, dbee_config::DbKind::Postgres, "postgres", "dbee", Dialect::Postgres),
            (mysql.port, dbee_config::DbKind::MySql, "root", "", Dialect::MySql),
        ];
        for (port, kind, user, database, dialect) in servers {
            let config = ConnectionConfig {
                kind,
                host: "127.0.0.1".into(),
                port,
                user: user.into(),
                database: database.into(),
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
            let ranges = lexer::statements(dialect, &text);
            let kept = |ran: &Ran| ran.outcomes.iter().map(|o| (o.result.rows.len(), o.result.rows_seen, o.result.complete)).collect::<Vec<_>>();
            let setup = || RunSetup { use_database: None, dialect, current: CurrentDatabase::default() };
            let run = |budget: usize| run_statements(via(), text.clone(), ranges.clone(), setup(), Arc::new(Stop::default()), budget);
            let whole = run(RUN_LIMITS.max_bytes).await;
            assert_eq!(kept(&whole), [(1, 1, true), (1, 1, true), (1, 1, true)], "{dialect:?}");
            let held = |i: usize| dbee_driver::result_cost(&whole.outcomes[i].result);
            // The first result, the second's column and the third: the second's row does not fit in
            // what the first left, and the third does in what the second's column left.
            let second_column = dbee_driver::column_cost(&whole.outcomes[1].result.columns[0]);
            let ran = run(held(0) + second_column + held(2)).await;
            assert!(ran.failure.is_none(), "{dialect:?}");
            assert_eq!(kept(&ran), [(1, 1, true), (0, 1, false), (1, 1, true)], "{dialect:?}");
            // Spent by the first result: the second's columns do not fit, so it fails and the run
            // ends there, with the connection still good.
            let ran = run(held(0)).await;
            assert_eq!(kept(&ran), [(1, 1, true)], "{dialect:?}");
            let failed = ran.failure.as_ref().map(|f| (f.statement, f.error.clone()));
            assert_eq!(failed, Some((1, DbError::TooLarge { left: 0 })), "{dialect:?}");
            let (conn, _) = ran.conn.as_ref().unwrap();
            assert!(!conn.is_closed(), "{dialect:?}");
            assert_eq!(conn.scalar_u64("select 7").await, Ok(7), "{dialect:?}");
        }
        assert_eq!(RUN_LIMITS, QUERY_LIMITS, "a whole run gets the query editor's caps");
    }
}

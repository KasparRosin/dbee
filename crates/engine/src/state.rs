//! The session state: screen, saved connections, the open connections and the tabs.
//!
//! [`State`] is cloned before every command (the panic guard's snapshot), so large data sits
//! behind `Arc`.

use std::ops::Range;
use std::sync::Arc;

use dbee_config::{ConnectionConfig, ConnectionStore};
use dbee_core::{Catalog, Cell, DatabaseAfter, ForeignKey, ResultSet, TableInfo, TableRef, TableStructure};
use dbee_driver::{ApplyGate, Connection, DbError, Meaning, ServerInfo, TlsState, TransactionState};
use dbee_sql::Dialect;
use dbee_sql::filter::{Condition, Filter, Sort, SortDir};
use dbee_ssh::{SshSecrets, Tunnel};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use crate::edits::Edits;
use crate::jobs::Stop;
use crate::turns::InTurn;
use crate::{ConnectionId, JobId, Json, TabId};

/// The dialect SQL is read with while no connection is open (the query editor's colours then).
pub const NO_CONNECTION_DIALECT: Dialect = Dialect::Postgres;

/// What the result texts and the status bar call the server: `PostgreSQL 14.12`.
pub fn server_text(info: &ServerInfo) -> String {
    format!("{} {}", info.product, info.version)
}

/// What the status bar says of the link to the server, and its longer text, for a connection
/// through the SSH tunnel or not: `None` for a database without a link, a file.
pub fn tls_text(info: &ServerInfo, tunnel: bool) -> Option<(&'static str, &'static str)> {
    const NOT_ENCRYPTED: &str = "Not encrypted";
    Some(match info.tls? {
        TlsState::Off if tunnel => {
            (NOT_ENCRYPTED, "The SSH tunnel encrypts the connection as far as the SSH server; from there to the database server it is not encrypted.")
        }
        TlsState::Off if info.local => (NOT_ENCRYPTED, "The connection is not encrypted, but it does not leave this machine."),
        TlsState::Off => {
            (NOT_ENCRYPTED, "The connection to the server is not encrypted: anyone on the network between them can read and change what is sent.")
        }
        TlsState::Unchecked => (
            "Encrypted, server not checked",
            "The connection is encrypted, but dbee did not check the server's certificate, so it cannot tell who answered. SSL mode verify-full checks it.",
        ),
        TlsState::CertificateChecked => (
            "Encrypted, certificate checked",
            "The connection is encrypted, and the certificate was checked against the CA file, but not the server's name. SSL mode verify-full checks the name too.",
        ),
        TlsState::Checked => ("Encrypted, server checked", "The connection is encrypted, and the server's certificate and its name were checked."),
    })
}

/// Which screen the window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    #[default]
    Connections,
    Workspace,
}

/// An open connection, with what belongs to it alone.
///
/// The connections are crate-private: commands reach them through the session, and a job that
/// used one checks that it is still the session's before applying its result. `Debug` shows no
/// secret.
#[derive(Clone)]
pub struct Connected {
    pub id: ConnectionId,
    pub config: ConnectionConfig,
    pub info: ServerInfo,
    /// The server's SQL rules, from the browse connection: every statement the engine builds or
    /// splits for this connection is in this dialect.
    pub dialect: Dialect,
    pub catalog: Arc<Catalog>,
    /// The connection broke; commands that need the database wait for a reconnect.
    pub lost: bool,
    /// Table pages, counts, the catalog and applies, each in its turn.
    pub(crate) browse: Arc<InTurn>,
    /// Query tabs' statements; opened on the first query run (and again after a reconnect, or
    /// once it was lost).
    pub(crate) query: Option<Arc<dyn Connection>>,
    /// The database the query connection is in, while it is open: the settings' when it opens,
    /// each `USE` the engine sends, then where the driver says each statement left it. MySQL's
    /// driver takes that from the server's session tracking, and stops trusting it on a connection
    /// after a run that names `session_track`.
    pub(crate) query_database: CurrentDatabase,
    /// The query run in progress: one at a time, since the query connection runs one statement at
    /// a time and a cancel request hits whatever runs on it. A `query.commit` or `query.rollback`
    /// counts as one.
    pub(crate) query_run: Option<QueryRun>,
    /// The query session's transaction, as the driver reported it after the last run, commit or
    /// rollback on the query connection; read it through [`Connected::transaction`].
    pub(crate) transaction: Option<Transaction>,
    /// The SSH tunnel both connections run through, when the config has one.
    pub(crate) tunnel: Option<Arc<Tunnel>>,
    /// What the connect used, kept in memory for reconnects (and the query connection).
    pub(crate) credentials: Credentials,
    /// The running `edit.apply` job and its gate: `table.cancel` on its tab cancels it until it
    /// decides to commit (one Apply runs at a time).
    pub(crate) apply_gate: Option<(JobId, Arc<ApplyGate>, Arc<Stop>)>,
}

impl Connected {
    /// `conn` is this connection's browse connection.
    pub(crate) fn is_browse(&self, conn: &Arc<dyn Connection>) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.browse), Arc::as_ptr(conn))
    }
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected")
            .field("id", &self.id)
            .field("config", &self.config)
            .field("info", &self.info)
            .field("dialect", &self.dialect)
            .field("tables", &self.catalog.table_count())
            .field("lost", &self.lost)
            .field("browse_closed", &self.browse.is_closed())
            .field("query_closed", &self.query.as_ref().map(|q| q.is_closed()))
            .field("query_database", &self.query_database)
            .field("query_run", &self.query_run)
            .field("transaction", &self.transaction)
            .field("tunnel", &self.tunnel)
            .field("credentials", &self.credentials)
            .field("apply_gate", &self.apply_gate.as_ref().map(|(job, _, _)| job))
            .finish()
    }
}

/// What the engine knows of the database a connection is in, or none. It is never guessed: once
/// a statement may have moved it unseen, `known` is false and `database` the last one known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurrentDatabase {
    pub database: Option<String>,
    pub known: bool,
}

impl CurrentDatabase {
    /// Known to be `database`, or none.
    pub fn known(database: Option<String>) -> CurrentDatabase {
        CurrentDatabase { database, known: true }
    }

    /// The settings' database, as a connection opened with `config` starts in it.
    pub fn of_settings(config: &ConnectionConfig) -> CurrentDatabase {
        CurrentDatabase::known(Some(config.database.clone()).filter(|d| !d.is_empty()))
    }

    /// Known to be `name`.
    pub fn is(&self, name: &str) -> bool {
        self.known && self.database.as_deref() == Some(name)
    }

    /// Where a statement that succeeded leaves it, by the driver's `report`, on a dialect where a
    /// statement can move it (`moves`): where the driver says; where it was, when the driver says
    /// it stayed; not known any more, when the driver cannot tell.
    pub(crate) fn after(self, report: &DatabaseAfter, moves: bool) -> CurrentDatabase {
        match report {
            _ if !moves => self,
            DatabaseAfter::Changed(database) => CurrentDatabase::known(database.clone()),
            DatabaseAfter::Unchanged => self,
            DatabaseAfter::Untold => CurrentDatabase { known: false, ..self },
        }
    }

    /// Where a statement that failed with `error` leaves it, on a dialect where a statement can
    /// move it (`moves`): where it was, after an error the server answered with; not known any
    /// more after any other end, an interrupt among them, dbee's or not (its kill can land just
    /// after the statement took effect, and the server's error then hides the change).
    pub(crate) fn after_failure(self, error: &DbError, moves: bool) -> CurrentDatabase {
        match error {
            _ if !moves => self,
            e if e.is_interrupted() => CurrentDatabase { known: false, ..self },
            DbError::Server(_) | DbError::ReadOnly { .. } => self,
            _ => CurrentDatabase { known: false, ..self },
        }
    }

    /// `{database, known}`: the name or null, and whether the engine knows it.
    pub fn summary(&self) -> Json {
        json!({ "database": self.database, "known": self.known })
    }
}

/// The query session's open transaction: its state and the tab whose run opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// `Open`, `Failed` or `Unknown`; never `Idle`, which is no transaction.
    pub state: TransactionState,
    /// The query tab whose run found the session idle and left it in a transaction; `None` once
    /// that tab has closed, or when the state is unknown.
    pub tab: Option<TabId>,
}

impl Transaction {
    /// The transaction after a run of `tab` (`None` once it has closed), or a commit or rollback
    /// (`None`), left the session in `state`, where it was `before`: none when idle; else the
    /// owner it had, or the run's tab when it found the session idle.
    pub(crate) fn after(before: Option<Transaction>, state: TransactionState, tab: Option<TabId>) -> Option<Transaction> {
        match (state, before) {
            (TransactionState::Idle, _) => None,
            (TransactionState::Unknown, _) => Some(Transaction { state, tab: None }),
            (_, Some(before)) => Some(Transaction { state, tab: before.tab }),
            (_, None) => Some(Transaction { state, tab }),
        }
    }

    /// `{state, tab}`: `state` is `open`, `failed` or `unknown` ([`transaction_state`]).
    pub fn summary(&self) -> Json {
        json!({ "state": transaction_state(self.state), "tab": self.tab })
    }
}

/// A transaction's state as the engine names it: `open`, `failed` or `unknown` (`Idle` is no
/// transaction, so it never names one).
pub fn transaction_state(state: TransactionState) -> &'static str {
    match state {
        TransactionState::Open => "open",
        TransactionState::Failed => "failed",
        TransactionState::Idle | TransactionState::Unknown => "unknown",
    }
}

impl Connected {
    /// The query session's transaction; none while the query connection is not open, or has
    /// closed, which ends its session.
    pub fn transaction(&self) -> Option<Transaction> {
        self.query.as_ref().filter(|q| !q.is_closed()).and(self.transaction)
    }

    /// Query tab `tab` has closed: a transaction it opened has no tab any more.
    pub(crate) fn tab_closed(&mut self, tab: TabId) {
        if let Some(t) = &mut self.transaction
            && t.tab == Some(tab)
        {
            t.tab = None;
        }
    }

    /// The database a run of `q` starts in: the tab's own, which the run makes current first;
    /// else the query connection's while it is open, or the settings' before it opens.
    pub fn starts_in(&self, q: &QueryTab) -> CurrentDatabase {
        match (&q.database, &self.query) {
            (Some(name), _) => CurrentDatabase::known(Some(name.clone())),
            (None, Some(_)) => self.query_database.clone(),
            (None, None) => CurrentDatabase::of_settings(&self.config),
        }
    }

    /// `{connection, id, name, color, kind, host, port, database, user, ssh, readOnly, server, tls,
    /// lost, saved, transaction}`, as `inspect()` and `connection.open` show it: `connection` is its
    /// [`ConnectionId`], `id` the saved connection's, `kind` the database type's id, `server` the
    /// product and its version (`PostgreSQL 14.12`), `tls` the link's [`TlsState`] (`off`,
    /// `unchecked`, `certificateChecked`, `checked`; `null` for a file), `saved` whether the saved list holds it
    /// (`ConnectionStore::is_saved`, which the caller asks), `transaction` the query session's
    /// ([`Transaction::summary`], or null). Never a secret.
    pub fn summary(&self, saved: bool) -> Json {
        let c = &self.config;
        json!({
            "connection": self.id,
            "id": c.id,
            "name": c.display_name(),
            "color": c.color,
            "kind": c.kind,
            "host": c.host,
            "port": c.port,
            "database": c.database,
            "user": c.user,
            "ssh": c.ssh.is_some(),
            "readOnly": self.info.read_only,
            "server": server_text(&self.info),
            "tls": self.info.tls.map(TlsState::id),
            "lost": self.lost,
            "saved": saved,
            "transaction": self.transaction().map(|t| t.summary()),
        })
    }
}

/// The session's query run in progress ([`Connected::query_run`]): its job, and how to stop it.
#[derive(Clone, Debug)]
pub(crate) struct QueryRun {
    pub job: JobId,
    pub stop: Arc<Stop>,
}

/// The secrets of one connect and the SSH host key fingerprint the user accepted. Kept only in
/// crate-private fields; `Debug` hides the secrets.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Credentials {
    /// The database password.
    pub password: Option<String>,
    /// The SSH password and key passphrase.
    pub ssh: SshSecrets,
    /// The fingerprint from `trustHostKey`, so a key accepted for this connection only (no
    /// `known_hosts` to learn it into) is accepted again on a reconnect.
    pub trust: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("ssh", &self.ssh)
            .field("trust", &self.trust)
            .finish()
    }
}

/// An inclusive rectangle of grid cells, `(row, column)`: where the selection started and
/// where it ends now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub focus: (usize, usize),
    /// Whole rows, selected by their numbers (`selection.rows`, issue #17): the rectangle spans
    /// every column the grid has, also when those change.
    #[serde(default)]
    pub whole: bool,
}

impl Selection {
    /// The selection on a grid of `rows` × `columns` that replaces the one it was made on (a
    /// refresh, another page or result): kept while its cells are there; whole rows while their
    /// rows are, across the new grid's columns.
    pub fn kept(self, rows: usize, columns: usize) -> Option<Selection> {
        if self.whole {
            let last = columns.checked_sub(1)?;
            let (a, f) = (self.anchor.0, self.focus.0);
            return (a < rows && f < rows).then_some(Selection { anchor: (a, 0), focus: (f, last), whole: true });
        }
        let fits = |(row, column): (usize, usize)| row < rows && column < columns;
        (fits(self.anchor) && fits(self.focus)).then_some(self)
    }

    /// `{anchor: [row, column], focus: [row, column], whole}`, as the selection commands and
    /// `inspect()` give it.
    pub fn to_json(self) -> Json {
        json!({ "anchor": [self.anchor.0, self.anchor.1], "focus": [self.focus.0, self.focus.1], "whole": self.whole })
    }
}

/// A query editor tab.
///
/// `results`, `shown` and `error` are the last run's: they change when a run ends (not when it
/// starts), all together. Byte offsets (`StatementOutcome::range`, `QueryError::position`) are
/// into the tab text as it was when that run started; the text may have been edited since.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryTab {
    pub id: TabId,
    /// The connection it runs on; `None` for a tab opened while no connection was open, until one
    /// opens.
    pub connection: Option<ConnectionId>,
    pub title: String,
    pub text: String,
    /// The job running this tab's statements.
    pub running: Option<JobId>,
    /// In the shown result's grid.
    pub selection: Option<Selection>,
    /// One per statement that ran without an error, in order.
    pub results: Vec<StatementOutcome>,
    /// The index of the result the grid shows: the last one after a run (0 when there is none).
    pub shown: usize,
    /// Why the last run stopped early: an error, or a cancel.
    pub error: Option<QueryError>,
    /// The database each run starts in, made current first with the dialect's `USE` when the query
    /// connection is not known to be there already. After a run that got past it, it becomes the
    /// database the driver says the run left the connection in, so a `USE` in the text sticks to
    /// the tab. `None` runs in the query connection's current database ([`Connected::starts_in`]).
    pub database: Option<String>,
}

impl QueryTab {
    /// A tab with `text` that has not run, on no connection, with no database of its own.
    pub fn new(id: TabId, title: String, text: String) -> QueryTab {
        QueryTab { id, connection: None, title, text, running: None, selection: None, results: Vec::new(), shown: 0, error: None, database: None }
    }

    /// The result the grid shows, when there is one.
    pub fn shown_result(&self) -> Option<&StatementOutcome> {
        self.results.get(self.shown)
    }
}

/// One statement's result in a query tab, and the order its rows are shown in.
///
/// The rows stay as the server sent them: a sort ([`ResultSort`]) is a list of row indices beside
/// them (`order`), so sorting copies no cell and dropping the sort gives the received order back.
/// Whoever reads a row for the grid, a copy or the value view reads it through [`Self::row`] or
/// [`Self::cell`] (the engine's own readers through `view::Grid`), never from `result.rows` by
/// its position in the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementOutcome {
    /// Where the statement is in the tab text: bytes, without the `;` and the comments around it.
    pub range: Range<usize>,
    /// Behind `Arc`: up to 10,000 rows or 256 MiB of text, and the state is cloned per command.
    pub result: Arc<ResultSet>,
    /// How long the statement took, in milliseconds from sending it to its result.
    pub ms: u64,
    /// The column the rows are sorted by in memory (`query.sortResult`); `None` as received.
    pub(crate) sort: Option<ResultSort>,
    /// With a sort: grid row `i` is `result.rows[order[i]]`. Behind `Arc`, as the result is; a new
    /// sort is a new list, which is how the value view knows its row may have changed.
    pub(crate) order: Option<Arc<[u32]>>,
}

impl StatementOutcome {
    /// A statement's result as received, unsorted.
    pub fn new(range: Range<usize>, result: Arc<ResultSet>, ms: u64) -> StatementOutcome {
        StatementOutcome { range, result, ms, sort: None, order: None }
    }

    /// The column the rows are sorted by, when they are.
    pub fn sort(&self) -> Option<ResultSort> {
        self.sort
    }

    /// The order the rows are shown in, when they are sorted.
    pub(crate) fn order(&self) -> Option<&Arc<[u32]>> {
        self.order.as_ref()
    }

    /// The cells of grid row `row`: the row the sort puts there. `None` past the last row.
    pub fn row(&self, row: usize) -> Option<&[Cell]> {
        let at = received_row(self.order.as_deref(), row)?;
        self.result.rows.get(at).map(Vec::as_slice)
    }

    /// The text of grid cell `(row, col)`; `None` is NULL, also for a cell the row does not have.
    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.row(row)?.get(col)?.as_deref()
    }
}

/// The index in a result's rows of grid row `row` under `order` (`None`: as received). `None`
/// for a row the order does not hold.
pub(crate) fn received_row(order: Option<&[u32]>, row: usize) -> Option<usize> {
    match order {
        None => Some(row),
        Some(order) => order.get(row).and_then(|at| usize::try_from(*at).ok()),
    }
}

/// A query result's sort: one column, by its index (result columns can share a name, or have
/// none), in memory over the rows held ([`dbee_core::sort`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSort {
    pub column: usize,
    pub dir: SortDir,
}

/// Why a query run stopped before its last statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryError {
    /// The error's `EngineError` code: `server`, `cancelled`, `readOnly`, `connectionLost`, ….
    pub code: &'static str,
    /// The server's message with its code (`column "x" does not exist (SQLSTATE 42703)`), or
    /// dbee's text for errors that are not the server's.
    pub message: String,
    /// The server's own code, for a server error (Postgres's SQLSTATE `42703`, MySQL's errno
    /// `1054`).
    pub server_code: Option<String>,
    /// What the server error means, when the driver knows: `transactionAborted` (a statement
    /// failed earlier in the transaction, which refuses every statement until a ROLLBACK) or
    /// `interrupted` (the server stopped the statement, and dbee had sent no cancel for it).
    #[serde(serialize_with = "meaning_id")]
    pub meaning: Option<Meaning>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// Where the server says the error is, as a byte offset into the tab text.
    pub position: Option<usize>,
    /// The 0-based index of the statement that failed, among those the run ran.
    pub statement: usize,
    /// How long the failing or stopped statement ran, in milliseconds from sending it until it
    /// failed; `None` when the run stopped before it was sent (connecting, the tab's `USE`, a NUL
    /// in the text, a stop between statements).
    pub ms: Option<u64>,
}

fn meaning_id<S: Serializer>(meaning: &Option<Meaning>, s: S) -> Result<S::Ok, S::Error> {
    match meaning {
        None => s.serialize_none(),
        Some(Meaning::TransactionAborted) => s.serialize_str("transactionAborted"),
        Some(Meaning::Interrupted) => s.serialize_str("interrupted"),
    }
}

/// A table tab's row count.
///
/// As JSON: `"unknown"`, `{"estimate": n}`, `{"counting": {"estimate": n | null}}` or
/// `{"exact": n}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Total {
    /// Not counted.
    #[default]
    Unknown,
    /// The planner's estimate (`reltuples`), shown as `~n`; a count that failed or was
    /// cancelled leaves it.
    Estimate(u64),
    /// An exact count is running; the estimate shows meanwhile when there is one.
    Counting { estimate: Option<u64> },
    /// Counted.
    Exact(u64),
}

impl Total {
    /// What is left when a count ends without a number: the estimate it showed, else unknown.
    pub fn settled(self) -> Total {
        match self {
            Total::Counting { estimate: Some(n) } => Total::Estimate(n),
            Total::Counting { estimate: None } => Total::Unknown,
            other => other,
        }
    }
}

/// What a table page was loaded with: the condition, the sort and the 0-based page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageView {
    pub applied: Condition,
    pub sort: Option<Sort>,
    pub page: u64,
}

/// What a table tab shows: its rows (DATA) or its structure (STRUCTURE). A tab opens in DATA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Showing {
    #[default]
    Data,
    Structure,
}

/// A table's structure as a tab read it: the catalog's table (columns and keys) as it was when
/// the read started, the foreign keys of every table that reference it, and what the read
/// brought (indexes, triggers, defaults, the foreign keys' actions).
#[derive(Clone, Debug, PartialEq)]
pub struct StructureRead {
    pub info: TableInfo,
    /// Each with the table it belongs to.
    pub referenced_by: Vec<(TableRef, ForeignKey)>,
    pub structure: TableStructure,
}

/// A table tab's STRUCTURE view: the read running (`loading`), the last one that landed
/// (`shown`), and why the last read failed (`error`; `shown` stays). Only the read `loading`
/// names lands: a newer read, a cancel or a reconnect leaves an older one ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabStructure {
    pub loading: Option<JobId>,
    pub shown: Option<Arc<StructureRead>>,
    pub error: Option<String>,
    /// The server's DETAIL and HINT of the read that failed, when it gave them (`error` ends with
    /// them too, as the server error's text does).
    pub detail: Option<String>,
    pub hint: Option<String>,
}

impl TabStructure {
    /// The last read failed with `error`, and the server's `detail` and `hint`; or, with `None`,
    /// nothing failed.
    pub(crate) fn failed(&mut self, error: Option<String>, detail: Option<String>, hint: Option<String>) {
        self.error = error;
        self.detail = detail;
        self.hint = hint;
    }
}

/// A table data tab.
///
/// `applied`, `sort` and `page` are what the tab asks for: what the rows were loaded with, or are
/// being loaded with while `loading` runs (so a second "next" goes on from the page asked for).
/// `shown` is what `data` was loaded with; number the rows from `shown.page`. A load that fails
/// puts `applied`, `sort` and `page` back to `shown` and sets `error`; `data` stays.
///
/// `loading`, `counting` and `applying` name the jobs in flight for the tab, always on its
/// connection's current browse connection: a job clears its own when its result arrives (stale or
/// not), the session clears any left once the job has ended (its future or its `Apply`
/// panicked, [`TableTab::job_ended`]), and a reconnect clears them all
/// ([`TableTab::drop_dead_jobs`]).
///
/// `edits` holds the staged edits, on the rows of `data`: while any are staged, the tab loads no
/// other page ([`TableTab::has_staged_edits`]), and they are not changed while a page loads or
/// an Apply runs. A new page starts a new undo history.
///
/// `saved` lists the cells of `data` the last successful Apply wrote, found on the page it loaded
/// by their rows' keys (`Edits::written`). Only that page sets it. An Apply starting, any other
/// page load, a reconnect, and a reset, undo or redo empty it; staging an edit on a cell takes
/// that cell out.
#[derive(Clone, Debug, PartialEq)]
pub struct TableTab {
    pub id: TabId,
    /// The connection whose table it shows.
    pub connection: ConnectionId,
    pub table: TableRef,
    pub selection: Option<Selection>,
    /// The filter bar's rows, as edited (applied or not).
    pub filters: Vec<Filter>,
    /// The filter bar's raw `WHERE` condition, as edited.
    pub raw: String,
    /// The filter bar shows the raw condition instead of the rows.
    pub raw_mode: bool,
    pub applied: Condition,
    pub sort: Option<Sort>,
    /// 0-based.
    pub page: u64,
    /// The loaded page: at most 100 rows, the table's visible columns typed from the catalog.
    /// `None` until the first page arrives.
    pub data: Option<Arc<ResultSet>>,
    /// The page's row key columns that are not visible (`TableInfo::page_columns`), row for row
    /// with `data`: what the edits find their rows by, never shown. No columns when every key
    /// column is visible.
    pub hidden: Arc<ResultSet>,
    /// More rows follow this page (the page statement got 101).
    pub has_next: bool,
    /// `data` holds every row of its page: false when the page's cell text passed the byte cap
    /// (256 MiB), so fewer rows were kept than the page has (the rest are not shown, and the
    /// next page starts after them all the same).
    pub page_complete: bool,
    pub total: Total,
    /// The job loading a page.
    pub loading: Option<JobId>,
    /// The job counting the rows (`table.count`, or `table.page` to the last page).
    pub counting: Option<JobId>,
    /// Why the last load failed, the server's text; the rows from before stay.
    pub error: Option<String>,
    /// Bumped by every page load; a result whose `seq` is not the tab's is stale and ignored.
    pub seq: u64,
    /// What `data` was loaded with.
    pub shown: PageView,
    /// The staged edits (behind `Arc`: the state is cloned before every command).
    pub edits: Arc<Edits>,
    /// The job writing the edits (`edit.apply`).
    pub applying: Option<JobId>,
    /// The cells the last successful Apply wrote, `(row, column)` of `data`, in order (see above).
    pub saved: Vec<(usize, usize)>,
    /// What the tab shows (`table.view`); DATA's state above is kept while STRUCTURE shows.
    pub showing: Showing,
    /// The STRUCTURE view's read.
    pub structure: TabStructure,
}

impl TableTab {
    /// A tab for `table` of connection `connection` about to load `view`; nothing loaded yet.
    pub fn new(id: TabId, connection: ConnectionId, table: TableRef, filters: Vec<Filter>, view: PageView) -> TableTab {
        TableTab {
            id,
            connection,
            table,
            selection: None,
            filters,
            raw: String::new(),
            raw_mode: false,
            applied: view.applied.clone(),
            sort: view.sort.clone(),
            page: view.page,
            data: None,
            hidden: Arc::default(),
            has_next: false,
            page_complete: true,
            total: Total::Unknown,
            loading: None,
            counting: None,
            error: None,
            seq: 0,
            shown: view,
            edits: Arc::default(),
            applying: None,
            saved: Vec::new(),
            showing: Showing::Data,
            structure: TabStructure::default(),
        }
    }

    /// What `applied`, `sort` and `page` ask for.
    pub fn view(&self) -> PageView {
        PageView { applied: self.applied.clone(), sort: self.sort.clone(), page: self.page }
    }

    /// The connection the tab's jobs ran on was replaced (a reconnect): they are dead, and their
    /// results will be ignored. A load in flight counts as failed without a message (the view
    /// goes back to `shown`), a count in flight leaves the estimate it showed, and an Apply in
    /// flight ends with an unknown outcome (the edits stay, `failed` says so). The saved cells
    /// lose their mark, and a structure read in flight is forgotten (STRUCTURE keeps what it showed).
    pub(crate) fn drop_dead_jobs(&mut self) {
        self.saved.clear();
        self.structure.loading = None;
        if self.loading.take().is_some() {
            self.back_to_shown();
        }
        self.counting = None;
        self.total = self.total.settled();
        if self.applying.take().is_some() {
            self.apply_outcome_unknown();
        }
    }

    /// Job `job` has ended and its result was applied, or dropped because it panicked: a marker
    /// that still names it is cleared as [`TableTab::drop_dead_jobs`] would clear it, so no
    /// marker outlives its job.
    pub(crate) fn job_ended(&mut self, job: JobId) {
        if self.structure.loading == Some(job) {
            self.structure.loading = None;
        }
        if self.loading == Some(job) {
            self.loading = None;
            self.back_to_shown();
        }
        if self.counting == Some(job) {
            self.counting = None;
            self.total = self.total.settled();
        }
        if self.applying == Some(job) {
            self.applying = None;
            self.apply_outcome_unknown();
        }
    }

    fn apply_outcome_unknown(&mut self) {
        let loaded = self.data.as_ref().map_or(0, |d| d.rows.len());
        Arc::make_mut(&mut self.edits).mark_unknown(loaded);
    }

    /// A load ended without rows: `applied`, `sort` and `page` go back to what the rows shown were
    /// loaded with. A total about the condition that did not load is forgotten.
    pub(crate) fn back_to_shown(&mut self) {
        let shown = self.shown.clone();
        if self.applied != shown.applied {
            self.total = Total::Unknown;
        }
        self.applied = shown.applied;
        self.sort = shown.sort;
        self.page = shown.page;
    }

    /// Edits are staged: filters, paging, sort and refresh wait until they are applied or reset
    /// (they would throw the staged rows away). The one place that answers it.
    pub fn has_staged_edits(&self) -> bool {
        !self.edits.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant, reason = "a window holds a few dozen tabs at most, and every command matches `Tab::Table(TableTab)`")]
pub enum Tab {
    Table(TableTab),
    Query(QueryTab),
}

impl Tab {
    pub fn id(&self) -> TabId {
        match self {
            Tab::Table(t) => t.id,
            Tab::Query(q) => q.id,
        }
    }

    /// The connection the tab belongs to; `None` for a query tab opened while none was open.
    pub fn connection(&self) -> Option<ConnectionId> {
        match self {
            Tab::Table(t) => Some(t.connection),
            Tab::Query(q) => q.connection,
        }
    }

    /// `query` or `table`.
    pub fn kind(&self) -> &'static str {
        match self {
            Tab::Table(_) => "table",
            Tab::Query(_) => "query",
        }
    }

    /// What the tab strip shows.
    pub fn title(&self) -> String {
        match self {
            Tab::Table(t) => t.table.display(),
            Tab::Query(q) => q.title.clone(),
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        match self {
            Tab::Table(t) => t.selection,
            Tab::Query(q) => q.selection,
        }
    }

    /// `{id, kind, title, connection}`, as `tab.list` and `inspect` show it.
    pub fn summary(&self) -> Json {
        json!({ "id": self.id(), "kind": self.kind(), "title": self.title(), "connection": self.connection() })
    }
}

/// Which open connection: read them all through [`State::connection`] with one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// The one tab `TabId` belongs to.
    Tab(TabId),
    /// The one with this id.
    Id(ConnectionId),
    /// The active tab's; with no tab active, the one opened last.
    Active,
}

#[derive(Clone, Debug)]
pub struct State {
    pub screen: Screen,
    pub store: ConnectionStore,
    /// The open connections, in the order they opened: at most one for now (`connection.open`
    /// refuses another).
    pub connections: Vec<Connected>,
    /// In tab-strip order.
    pub tabs: Vec<Tab>,
    pub active: Option<TabId>,
    /// The id the next tab gets; ids are never reused.
    pub next_tab: u64,
    /// The id the next connection gets; ids are never reused.
    pub next_connection: u64,
    /// Why the connections file could not be used, at the start or at a later change (then the
    /// store may be empty, or memory only), for the UI to show.
    pub store_warning: Option<String>,
    /// Where the update stands: a newer release, its install, or nothing.
    pub update: crate::update::UpdateState,
}

impl State {
    pub(crate) fn new(store: ConnectionStore, store_warning: Option<String>) -> State {
        State {
            screen: Screen::Connections,
            store,
            connections: Vec::new(),
            tabs: Vec::new(),
            active: None,
            next_tab: 1,
            next_connection: 1,
            store_warning,
            update: crate::update::UpdateState::None,
        }
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id() == id)
    }

    pub(crate) fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id() == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tab(id))
    }

    /// The open connection `which` names; `None` when it names none that is open.
    pub fn connection(&self, which: Which) -> Option<&Connected> {
        let id = self.connection_id(which)?;
        self.connections.iter().find(|c| c.id == id)
    }

    /// [`State::connection`], to change.
    pub(crate) fn connection_mut(&mut self, which: Which) -> Option<&mut Connected> {
        let id = self.connection_id(which)?;
        self.connections.iter_mut().find(|c| c.id == id)
    }

    fn connection_id(&self, which: Which) -> Option<ConnectionId> {
        match which {
            Which::Tab(tab) => self.tab(tab)?.connection(),
            Which::Id(id) => Some(id),
            Which::Active => self.active_tab().and_then(Tab::connection).or_else(|| self.connections.last().map(|c| c.id)),
        }
    }

    /// The SQL rules of the connection `which` names; [`NO_CONNECTION_DIALECT`] when it names none.
    pub fn dialect(&self, which: Which) -> Dialect {
        self.connection(which).map_or(NO_CONNECTION_DIALECT, |c| c.dialect)
    }

    /// A fresh tab id.
    pub(crate) fn take_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab = self.next_tab.saturating_add(1);
        id
    }

    /// A fresh connection id.
    pub(crate) fn take_connection_id(&mut self) -> ConnectionId {
        let id = ConnectionId(self.next_connection);
        self.next_connection = self.next_connection.saturating_add(1);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Not encrypted" says what that means for the link: on the network anyone can read it, past
    /// the SSH tunnel only from the SSH server on, and on this machine it goes nowhere.
    #[test]
    fn the_plain_links_text_says_where_it_runs() {
        let info = |local| ServerInfo {
            product: "PostgreSQL".into(),
            version: "16".into(),
            version_num: 160000,
            read_only: false,
            warnings: Vec::new(),
            tls: Some(TlsState::Off),
            local,
        };
        let hover = |local, tunnel| tls_text(&info(local), tunnel).map(|(label, hover)| (label.to_owned(), hover.to_owned())).unwrap();
        assert_eq!(hover(false, false).0, "Not encrypted");
        assert!(hover(false, false).1.contains("anyone on the network"), "{:?}", hover(false, false));
        assert!(hover(true, false).1.contains("does not leave this machine"), "{:?}", hover(true, false));
        assert!(hover(false, true).1.contains("as far as the SSH server"), "{:?}", hover(false, true));
        assert_eq!(tls_text(&ServerInfo { tls: None, ..info(true) }, false), None);
    }

    #[test]
    fn the_database_follows_what_the_driver_tells() {
        let shop = || CurrentDatabase::known(Some("shop".into()));
        assert_eq!(shop().after(&DatabaseAfter::Changed(Some("many".into())), true), CurrentDatabase::known(Some("many".into())));
        assert_eq!(shop().after(&DatabaseAfter::Changed(None), true), CurrentDatabase::known(None));
        assert_eq!(shop().after(&DatabaseAfter::Unchanged, true), shop());
        // A driver that cannot tell: no longer known, the last one kept for what it is.
        let unknown = shop().after(&DatabaseAfter::Untold, true);
        assert_eq!(unknown, CurrentDatabase { database: Some("shop".into()), known: false });
        assert!(!unknown.is("shop") && shop().is("shop"));
        assert_eq!(unknown.summary(), json!({ "database": "shop", "known": false }));
        // Where no statement can move it, nothing does.
        assert_eq!(shop().after(&DatabaseAfter::Untold, false), shop());
        assert_eq!(shop().after(&DatabaseAfter::Changed(Some("many".into())), false), shop());
    }

    #[test]
    fn a_failed_statement_moves_it_only_as_far_as_not_known() {
        let shop = || CurrentDatabase::known(Some("shop".into()));
        let unknown = CurrentDatabase { database: Some("shop".into()), known: false };
        let refused = DbError::Server(Box::new(dbee_driver::ServerError {
            code: "1049".into(),
            code_label: None,
            message: "Unknown database 'gone'".into(),
            detail: None,
            hint: None,
            position: None,
            meaning: None,
        }));
        // The server answered with an error: the statement did nothing.
        assert_eq!(shop().after_failure(&refused, true), shop());
        assert_eq!(shop().after_failure(&DbError::ReadOnly { message: "read-only".into() }, true), shop());
        // A cancel, dbee's or the server's interrupt whoever asked for it: a kill that lands as a
        // `USE` finishes is answered with an error after the change. A lost connection, or an
        // answer that broke: the outcome is unknown too.
        let DbError::Server(s) = &refused else { panic!("a server error") };
        let interrupted =
            DbError::Server(Box::new(dbee_driver::ServerError { code: "1317".into(), meaning: Some(Meaning::Interrupted), ..(**s).clone() }));
        for error in [DbError::Cancelled, interrupted, DbError::ConnectionLost, DbError::Other("a broken answer".into())] {
            assert_eq!(shop().after_failure(&error, true), unknown, "{error:?}");
            assert_eq!(shop().after_failure(&error, false), shop(), "{error:?}: where no statement can move it");
        }
    }
}

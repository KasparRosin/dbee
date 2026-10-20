//! The session state: screen, saved connections, the open connection and the tabs.
//!
//! [`State`] is cloned before every command (the panic guard's snapshot), so large data sits
//! behind `Arc`.

use std::sync::Arc;

use dbee_config::{ConnectionConfig, ConnectionStore};
use dbee_core::{Catalog, ResultSet, TableRef};
use dbee_pg::{PgConnection, ServerInfo};
use dbee_sql::filter::{Condition, Filter, Sort};
use dbee_ssh::{SshSecrets, Tunnel};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::edits::Edits;
use crate::{JobId, Json, TabId};

/// Which screen the window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    #[default]
    Connections,
    Workspace,
}

/// The open connection.
///
/// The connections are crate-private: commands reach them through the session, and a job that
/// used one checks that it is still the session's before applying its result. `Debug` shows no
/// secret.
#[derive(Clone)]
pub struct Connected {
    pub config: ConnectionConfig,
    pub info: ServerInfo,
    pub catalog: Arc<Catalog>,
    /// The connection broke; commands that need the database wait for a reconnect.
    pub lost: bool,
    /// Table pages, counts, the catalog and applies.
    pub(crate) browse: Arc<PgConnection>,
    /// Query tabs' statements; opened on the first query run (and again after a reconnect).
    pub(crate) query: Option<Arc<PgConnection>>,
    /// The SSH tunnel both connections run through, when the config has one.
    pub(crate) tunnel: Option<Arc<Tunnel>>,
    /// What the connect used, kept in memory for reconnects (and the query connection).
    pub(crate) credentials: Credentials,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected")
            .field("config", &self.config)
            .field("info", &self.info)
            .field("tables", &self.catalog.table_count())
            .field("lost", &self.lost)
            .field("browse", &self.browse)
            .field("query", &self.query)
            .field("tunnel", &self.tunnel)
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl Connected {
    /// `{id, name, color, host, port, database, user, ssh, readOnly, server, lost}`, as
    /// `inspect()` and `connection.open` show it. Never a secret.
    pub fn summary(&self) -> Json {
        let c = &self.config;
        json!({
            "id": c.id,
            "name": c.display_name(),
            "color": c.color,
            "host": c.host,
            "port": c.port,
            "database": c.database,
            "user": c.user,
            "ssh": c.ssh.is_some(),
            "readOnly": self.info.read_only,
            "server": self.info.version,
            "lost": self.lost,
        })
    }
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
}

/// A query editor tab.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryTab {
    pub id: TabId,
    pub title: String,
    pub text: String,
    /// The job running this tab's statements.
    pub running: Option<JobId>,
    pub selection: Option<Selection>,
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

/// A table data tab.
///
/// `applied`, `sort` and `page` are what the tab asks for: what the rows were loaded with, or are
/// being loaded with while `loading` runs (so a second "next" goes on from the page asked for).
/// `shown` is what `data` was loaded with; number the rows from `shown.page`. A load that fails
/// puts `applied`, `sort` and `page` back to `shown` and sets `error`; `data` stays.
///
/// `loading`, `counting` and `applying` name the jobs in flight for the tab, always on the
/// session's current browse connection: a job clears its own when its result arrives (stale or
/// not), the session clears any left once the job has ended (its future or its `Apply`
/// panicked, [`TableTab::job_ended`]), and a reconnect clears them all
/// ([`TableTab::drop_dead_jobs`]).
///
/// `edits` holds the staged edits, on the rows of `data`: while any are staged, the tab loads no
/// other page ([`TableTab::has_staged_edits`]), and they are not changed while a page loads or
/// an Apply runs. A new page starts a new undo history.
#[derive(Clone, Debug, PartialEq)]
pub struct TableTab {
    pub id: TabId,
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
    /// The loaded page: at most 100 rows, columns typed from the catalog. `None` until the first
    /// page arrives.
    pub data: Option<Arc<ResultSet>>,
    /// More rows follow this page (the page statement got 101).
    pub has_next: bool,
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
}

impl TableTab {
    /// A tab for `table` about to load `view`; nothing loaded yet.
    pub fn new(id: TabId, table: TableRef, filters: Vec<Filter>, view: PageView) -> TableTab {
        TableTab {
            id,
            table,
            selection: None,
            filters,
            raw: String::new(),
            raw_mode: false,
            applied: view.applied.clone(),
            sort: view.sort.clone(),
            page: view.page,
            data: None,
            has_next: false,
            total: Total::Unknown,
            loading: None,
            counting: None,
            error: None,
            seq: 0,
            shown: view,
            edits: Arc::default(),
            applying: None,
        }
    }

    /// What `applied`, `sort` and `page` ask for.
    pub fn view(&self) -> PageView {
        PageView { applied: self.applied.clone(), sort: self.sort.clone(), page: self.page }
    }

    /// The connection the tab's jobs ran on was replaced (a reconnect): they are dead, and their
    /// results will be ignored. A load in flight counts as failed without a message (the view
    /// goes back to `shown`), a count in flight leaves the estimate it showed, and an Apply in
    /// flight ends with an unknown outcome (the edits stay, `failed` says so).
    pub(crate) fn drop_dead_jobs(&mut self) {
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

    /// `{id, kind, title}`, as `tab.list` and `inspect` show it.
    pub fn summary(&self) -> Json {
        json!({ "id": self.id(), "kind": self.kind(), "title": self.title() })
    }
}

#[derive(Clone, Debug)]
pub struct State {
    pub screen: Screen,
    pub store: ConnectionStore,
    pub connected: Option<Connected>,
    /// In tab-strip order.
    pub tabs: Vec<Tab>,
    pub active: Option<TabId>,
    /// The id the next tab gets; ids are never reused.
    pub next_tab: u64,
    /// Why the connections file could not be used (then the store is empty, and may be memory
    /// only), for the UI to show.
    pub store_warning: Option<String>,
}

impl State {
    pub(crate) fn new(store: ConnectionStore, store_warning: Option<String>) -> State {
        State { screen: Screen::Connections, store, connected: None, tabs: Vec::new(), active: None, next_tab: 1, store_warning }
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

    /// A fresh tab id.
    pub(crate) fn take_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab = self.next_tab.saturating_add(1);
        id
    }
}

//! The session state: screen, saved connections, the open connection and the tabs.
//!
//! [`State`] is cloned before every command (the panic guard's snapshot), so large data sits
//! behind `Arc`.

use std::sync::Arc;

use dbee_config::{ConnectionConfig, ConnectionStore};
use dbee_core::{Catalog, TableRef};
use dbee_pg::{PgConnection, ServerInfo};
use dbee_ssh::{SshSecrets, Tunnel};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

/// A table data tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TableTab {
    pub id: TabId,
    pub table: TableRef,
    pub selection: Option<Selection>,
}

#[derive(Clone, Debug, PartialEq)]
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

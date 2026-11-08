//! The interface every database driver implements, and that the engine holds without naming a
//! driver: [`Driver`] opens a [`Connection`], whose [`Cancel`] stops what runs on it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind};
use dbee_core::{Catalog, Cell, Column, ResultSet};
use dbee_sql::Dialect;
use dbee_ssh::Tunnel;
pub use futures_util::future::BoxFuture;

use crate::{ApplyError, ApplyGate, DbError};

/// What every driver call returns: a future that fails with a [`DbError`].
pub type DbFuture<'a, T> = BoxFuture<'a, Result<T, DbError>>;

/// How long opening a connection (TCP or the tunnel channel, TLS, login and the session setup)
/// and delivering a cancel may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Caps on what one run keeps: rows, and bytes of memory its columns and kept rows hold, as
/// [`column_cost`], [`ROW_COST`] and [`cell_cost`] count them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_rows: usize,
    pub max_bytes: usize,
}

/// The smallest heap block a cell's text is counted at: an allocator hands out no smaller one.
const MIN_TEXT_BLOCK: usize = 16;

/// What a kept cell holds, as the byte caps count it: its slot, and its text of `len` bytes
/// (`None` for NULL) in at least [`MIN_TEXT_BLOCK`]. An empty text takes no heap block.
pub fn cell_cost(len: Option<usize>) -> usize {
    let text = match len {
        None | Some(0) => 0,
        Some(len) => len.max(MIN_TEXT_BLOCK),
    };
    size_of::<Cell>().saturating_add(text)
}

/// What a kept row holds beside its cells.
pub const ROW_COST: usize = size_of::<Vec<Cell>>();

/// What a result column holds, as the byte caps count it: the column and its names.
pub fn column_cost(c: &Column) -> usize {
    [size_of::<Column>(), c.name.len(), c.base_type.len(), c.type_label.len()].into_iter().fold(0, usize::saturating_add)
}

/// What `r`'s columns and rows hold, as the byte caps count it.
pub fn result_cost(r: &ResultSet) -> usize {
    let columns = r.columns.iter().map(column_cost).fold(0, usize::saturating_add);
    let rows = r.rows.iter().map(|row| row.iter().map(|c| cell_cost(c.as_ref().map(String::len))).fold(ROW_COST, usize::saturating_add));
    rows.fold(columns, usize::saturating_add)
}

/// The query editor's caps: 10,000 rows or 256 MiB held.
pub const QUERY_LIMITS: Limits = Limits { max_rows: 10_000, max_bytes: 256 * 1024 * 1024 };

/// A table page: 100 rows plus one, so the caller can tell whether another page exists.
pub const PAGE_LIMITS: Limits = Limits { max_rows: 101, max_bytes: 256 * 1024 * 1024 };

/// Catalog queries: far above any real catalog (5 million rows a query, 512 MiB held by all the
/// queries of one read together, [`crate::catalog::CatalogBudget`]), since a catalog is never
/// cut. A result that still hits them is an error, never a partial catalog.
pub const CATALOG_LIMITS: Limits = Limits { max_rows: 5_000_000, max_bytes: 512 * 1024 * 1024 };

/// What protects the link to the server, as the connection found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsState {
    /// No TLS: what is sent can be read and changed on the way.
    Off,
    /// TLS, with a certificate dbee did not check: encrypted, but whoever answered could be anyone.
    Unchecked,
    /// TLS, with the server's certificate checked: its chain, and under `verify-full` its name.
    Checked,
}

impl TlsState {
    /// The id the control channel shows: `off`, `unchecked` or `checked`.
    pub fn id(self) -> &'static str {
        match self {
            TlsState::Off => "off",
            TlsState::Unchecked => "unchecked",
            TlsState::Checked => "checked",
        }
    }
}

/// What the server said about itself when the connection opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// The product as the server reports itself: `PostgreSQL`, `MySQL`, `MariaDB` or `SQLite`.
    pub product: String,
    /// The version as the driver shows it beside the product: Postgres's as it prints it (`14.12`,
    /// `18.0 (Debian 18.0-1)`), MySQL's and MariaDB's numbers alone (`8.0.46`, `13.0.2`).
    pub version: String,
    /// The version as one number that grows with it, e.g. Postgres's `server_version_num`
    /// `140012`.
    pub version_num: u32,
    /// New transactions are read-only: set by dbee for a read-only connection, or by the server.
    pub read_only: bool,
    /// Things the user should know, e.g. that the server is older than dbee supports.
    pub warnings: Vec<String>,
    /// TLS on the link to the server; `None` for a database without one, a file.
    pub tls: Option<TlsState>,
}

/// What a connection is for. A driver may set its session up differently for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The catalog, table pages, counts and Apply.
    Browse,
    /// A query tab's statements.
    Query,
}

/// Where and how to connect. `password` is the database password (never stored in `config`);
/// `tunnel` is the open SSH tunnel when `config.ssh` is set; `create_file` lets a file database
/// make its file when it does not exist yet. `allowed_dirs`, in an isolated run, are the only
/// folders a database file may lie in: a file driver checks its file against them inside its
/// open ([`crate::allowed_file`]) and opens the path that check resolved, and then keeps the
/// connection's SQL from opening or making any other file. A server database's driver ignores it.
#[derive(Clone)]
pub struct Target<'a> {
    pub config: &'a ConnectionConfig,
    pub password: Option<&'a str>,
    pub tunnel: Option<Arc<Tunnel>>,
    pub purpose: Purpose,
    pub create_file: bool,
    pub allowed_dirs: Option<&'a [PathBuf]>,
}

impl std::fmt::Debug for Target<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("config", &self.config)
            .field("password", &self.password.map(|_| "<redacted>"))
            .field("tunnel", &self.tunnel)
            .field("purpose", &self.purpose)
            .field("create_file", &self.create_file)
            .field("allowed_dirs", &self.allowed_dirs)
            .finish()
    }
}

/// A database driver: it opens connections for the database types it serves (a family, such as
/// MySQL and MariaDB).
pub trait Driver: Send + Sync + 'static {
    /// The connection types this driver opens.
    fn kinds(&self) -> &'static [DbKind];
    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>>;

    /// A connection to read the server's databases through ([`Connection::list_databases`]),
    /// opened as the driver's list needs, whatever database `target` names. `None` for a driver
    /// whose connection is one database, which has no list.
    fn list_connection<'a>(&'a self, _target: Target<'a>) -> DbFuture<'a, Option<Arc<dyn Connection>>> {
        Box::pin(async { Ok(None) })
    }
}

/// An open connection. Operations on it never interleave; dropping the last handle closes it.
pub trait Connection: Send + Sync + 'static {
    fn info(&self) -> &ServerInfo;
    /// The SQL rules of the server this connection reached.
    fn dialect(&self) -> Dialect;
    /// The connection has ended; every further operation fails with [`DbError::ConnectionLost`].
    fn is_closed(&self) -> bool;
    /// Cancels whatever runs on this connection at the time of the call.
    fn canceller(&self) -> Arc<dyn Cancel>;
    /// Typed columns; caps as `limits`. The last row-returning statement's rows are kept.
    fn run<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet>;
    /// Columns untyped (table pages take them from the catalog).
    fn run_untyped<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet>;
    /// The first cell of the first row as an unsigned integer (a count).
    fn scalar_u64<'a>(&'a self, sql: &'a str) -> DbFuture<'a, u64>;
    fn load_catalog(&self) -> DbFuture<'_, Catalog>;
    /// The databases of the server this connection reached, by name, to choose one from. A
    /// connection that is one database lists none.
    fn list_databases(&self) -> DbFuture<'_, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// All or nothing; each statement must change exactly one row. Stops before COMMIT if `gate` is cancelled.
    fn apply<'a>(&'a self, statements: &'a [String], gate: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>>;
}

/// The error of a cancel request that was not delivered within [`CONNECT_TIMEOUT`].
pub fn cancel_not_delivered() -> DbError {
    DbError::Other(format!("the cancel request was not delivered within {} s", CONNECT_TIMEOUT.as_secs()))
}

/// Stops the statement running on one connection, from outside it. Holds no lock.
pub trait Cancel: Send + Sync + 'static {
    /// `Ok` means the cancel was delivered; the statement then fails with
    /// [`DbError::Cancelled`], unless it finished first.
    fn cancel(&self) -> DbFuture<'_, ()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_that_times_out_says_so_and_is_no_connect_error() {
        let e = cancel_not_delivered();
        assert_eq!(e.to_string(), "the cancel request was not delivered within 15 s");
        assert_eq!(e.code(), "other");
    }

    #[test]
    fn a_kept_cell_costs_its_slot_and_its_block() {
        // A NULL or empty cell holds its 24-byte slot, though it has no text.
        assert_eq!(cell_cost(None), 24);
        assert_eq!(cell_cost(Some(0)), 24);
        // A short text takes a whole heap block.
        assert_eq!(cell_cost(Some(1)), 24 + 16);
        assert_eq!(cell_cost(Some(100)), 24 + 100);
        assert_eq!(cell_cost(Some(usize::MAX)), usize::MAX);
        let c = Column { name: "id".into(), base_type: "int4".into(), type_label: "integer".into(), class: dbee_core::TypeClass::Number };
        assert_eq!(column_cost(&c), size_of::<Column>() + 2 + 4 + 7);
        let r = ResultSet { columns: vec![c.clone()], rows: vec![vec![None], vec![Some("x".into())]], ..ResultSet::default() };
        assert_eq!(result_cost(&r), column_cost(&c) + 2 * ROW_COST + 24 + 40);
    }

    #[test]
    fn m1_limits_and_timeout() {
        assert_eq!(QUERY_LIMITS, Limits { max_rows: 10_000, max_bytes: 256 * 1024 * 1024 });
        assert_eq!(PAGE_LIMITS, Limits { max_rows: 101, max_bytes: 256 * 1024 * 1024 });
        assert_eq!(CATALOG_LIMITS, Limits { max_rows: 5_000_000, max_bytes: 512 * 1024 * 1024 });
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(15));
    }

    /// A driver that opens nothing and says nothing about its databases.
    struct Silent;

    impl Driver for Silent {
        fn kinds(&self) -> &'static [DbKind] {
            &[DbKind::Sqlite]
        }

        fn connect<'a>(&'a self, _target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
            Box::pin(async { Err(DbError::Other("opens nothing".into())) })
        }
    }

    #[test]
    fn a_driver_lists_no_databases_unless_it_says_how() {
        use futures_util::FutureExt as _;
        let config = ConnectionConfig::default();
        let target = Target { config: &config, password: None, tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
        assert!(matches!(Silent.list_connection(target).now_or_never(), Some(Ok(None))));
    }

    #[test]
    fn target_debug_hides_the_password() {
        let config = ConnectionConfig::default();
        let dirs = [PathBuf::from("/run/allowed")];
        let t = Target {
            config: &config,
            password: Some("hunter2"),
            tunnel: None,
            purpose: Purpose::Query,
            create_file: false,
            allowed_dirs: Some(&dirs),
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("hunter2") && shown.contains("<redacted>") && shown.contains("Query"), "{shown}");
        assert!(shown.contains("/run/allowed"), "{shown}");
    }
}

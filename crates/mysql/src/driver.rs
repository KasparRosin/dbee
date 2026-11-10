//! MySQL and MariaDB behind the driver interface: [`MySqlDriver`], and the `Connection` trait on
//! [`MySqlConnection`], each method forwarding to the connection's own.

use std::sync::Arc;

use dbee_config::DbKind;
use dbee_core::{Catalog, ResultSet};
use dbee_driver::{ApplyError, ApplyGate, BoxFuture, Cancel, Connection, DbFuture, Driver, Limits, ServerInfo, Target, TransactionState};
use dbee_sql::Dialect;

use crate::catalog;
use crate::connect::{self, MySqlConnection};

/// The driver for the MySQL family: MySQL and MariaDB.
#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlDriver;

impl Driver for MySqlDriver {
    fn kinds(&self) -> &'static [DbKind] {
        &[DbKind::MySql, DbKind::MariaDb]
    }

    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        Box::pin(async move {
            let conn: Arc<dyn Connection> = Arc::new(connect::connect(target).await?);
            Ok(conn)
        })
    }

    fn list_connection<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Option<Arc<dyn Connection>>> {
        Box::pin(async move {
            let conn: Arc<dyn Connection> = Arc::new(catalog::list_connection(target).await?);
            Ok(Some(conn))
        })
    }
}

impl Connection for MySqlConnection {
    fn info(&self) -> &ServerInfo {
        MySqlConnection::info(self)
    }

    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }

    fn is_closed(&self) -> bool {
        MySqlConnection::is_closed(self)
    }

    fn canceller(&self) -> Arc<dyn Cancel> {
        Arc::new(MySqlConnection::canceller(self))
    }

    fn run<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(MySqlConnection::run(self, sql, limits))
    }

    fn run_untyped<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(MySqlConnection::run_untyped(self, sql, limits))
    }

    fn scalar_u64<'a>(&'a self, sql: &'a str) -> DbFuture<'a, u64> {
        Box::pin(MySqlConnection::scalar_u64(self, sql))
    }

    fn load_catalog(&self) -> DbFuture<'_, Catalog> {
        Box::pin(MySqlConnection::load_catalog(self))
    }

    fn list_databases(&self) -> DbFuture<'_, Vec<String>> {
        Box::pin(MySqlConnection::list_databases(self))
    }

    fn apply<'a>(&'a self, statements: &'a [String], gate: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>> {
        Box::pin(MySqlConnection::apply(self, statements, gate))
    }

    fn transaction(&self) -> BoxFuture<'_, TransactionState> {
        Box::pin(MySqlConnection::transaction(self))
    }
}

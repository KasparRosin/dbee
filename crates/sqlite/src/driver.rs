//! SQLite behind the driver interface: [`SqliteDriver`], and the traits on [`SqliteConnection`]
//! and [`Canceller`], each method forwarding to the connection's own.

use std::sync::Arc;

use dbee_config::DbKind;
use dbee_core::{Catalog, ResultSet, TableRef, TableStructure};
use dbee_driver::{ApplyError, ApplyGate, BoxFuture, Cancel, Connection, DbFuture, Driver, Limits, ServerInfo, Target, TransactionState};
use dbee_sql::Dialect;

use crate::connect::{self, Canceller, SqliteConnection};

/// The SQLite driver: a database file on this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDriver;

impl Driver for SqliteDriver {
    fn kinds(&self) -> &'static [DbKind] {
        &[DbKind::Sqlite]
    }

    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        Box::pin(async move {
            let conn: Arc<dyn Connection> = Arc::new(connect::connect(target).await?);
            Ok(conn)
        })
    }
}

impl Connection for SqliteConnection {
    fn info(&self) -> &ServerInfo {
        SqliteConnection::info(self)
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    fn is_closed(&self) -> bool {
        SqliteConnection::is_closed(self)
    }

    fn canceller(&self) -> Arc<dyn Cancel> {
        Arc::new(SqliteConnection::canceller(self))
    }

    fn run<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(SqliteConnection::run(self, sql, limits))
    }

    fn run_untyped<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(SqliteConnection::run_untyped(self, sql, limits))
    }

    fn scalar_u64<'a>(&'a self, sql: &'a str) -> DbFuture<'a, u64> {
        Box::pin(SqliteConnection::scalar_u64(self, sql))
    }

    fn load_catalog(&self) -> DbFuture<'_, Catalog> {
        Box::pin(SqliteConnection::load_catalog(self))
    }

    fn table_structure<'a>(&'a self, table: &'a TableRef) -> DbFuture<'a, TableStructure> {
        Box::pin(SqliteConnection::table_structure(self, table))
    }

    fn apply<'a>(&'a self, statements: &'a [String], gate: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>> {
        Box::pin(SqliteConnection::apply(self, statements, gate))
    }

    fn transaction(&self) -> BoxFuture<'_, TransactionState> {
        Box::pin(SqliteConnection::transaction(self))
    }
}

impl Cancel for Canceller {
    fn cancel(&self) -> DbFuture<'_, ()> {
        Box::pin(Canceller::cancel(self))
    }
}

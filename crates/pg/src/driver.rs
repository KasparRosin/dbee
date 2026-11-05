//! Postgres behind the driver interface: [`PgDriver`], and the traits on [`PgConnection`] and
//! [`Canceller`], each delegating to this crate's functions.

use std::sync::Arc;

use dbee_config::DbKind;
use dbee_core::{Catalog, ResultSet};
use dbee_driver::{ApplyError, ApplyGate, BoxFuture, Cancel, Connection, DbFuture, Driver, Limits, ServerInfo, Target};
use dbee_sql::Dialect;

use crate::{Canceller, PgConnection, apply, catalog, connect, query};

/// The Postgres driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgDriver;

impl Driver for PgDriver {
    fn kinds(&self) -> &'static [DbKind] {
        &[DbKind::Postgres]
    }

    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        Box::pin(async move {
            let conn: Arc<dyn Connection> = Arc::new(connect::connect(target).await?);
            Ok(conn)
        })
    }

    fn list_databases<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Vec<String>> {
        Box::pin(catalog::list_databases(target))
    }
}

impl Connection for PgConnection {
    fn info(&self) -> &ServerInfo {
        PgConnection::info(self)
    }

    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn is_closed(&self) -> bool {
        PgConnection::is_closed(self)
    }

    fn canceller(&self) -> Arc<dyn Cancel> {
        Arc::new(PgConnection::canceller(self))
    }

    fn run<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(query::run(self, sql, limits))
    }

    fn run_untyped<'a>(&'a self, sql: &'a str, limits: Limits) -> DbFuture<'a, ResultSet> {
        Box::pin(query::run_untyped(self, sql, limits))
    }

    fn scalar_u64<'a>(&'a self, sql: &'a str) -> DbFuture<'a, u64> {
        Box::pin(query::scalar_u64(self, sql))
    }

    fn load_catalog(&self) -> DbFuture<'_, Catalog> {
        Box::pin(catalog::load_catalog(self))
    }

    fn apply<'a>(&'a self, statements: &'a [String], gate: &'a ApplyGate) -> BoxFuture<'a, Result<u64, ApplyError>> {
        Box::pin(apply::apply_gated(self, statements, gate))
    }
}

impl Cancel for Canceller {
    fn cancel(&self) -> DbFuture<'_, ()> {
        Box::pin(Canceller::cancel(self))
    }
}

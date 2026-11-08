//! dbee Postgres driver (layer L4), behind `dbee-driver`'s interface ([`PgDriver`]): connecting
//! over TCP or an SSH tunnel, TLS, read-only mode, cancel, running SQL text over the
//! simple-query protocol, the catalog and the all-or-nothing apply.
//!
//! Values are text exactly as Postgres prints them; no prepared statements carry data. Every
//! session runs with `standard_conforming_strings = on` and a UTF-8 `client_encoding`, which the
//! quoting in `dbee-sql` relies on. Logs name hosts, ports and error codes only: never SQL text,
//! passwords, server messages or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod apply;
pub mod catalog;
pub mod connect;
pub mod driver;
mod error;
mod guard;
pub mod query;
pub mod tls;
mod watch;

pub use apply::{apply, apply_gated};
pub use connect::{Canceller, MIN_VERSION_NUM, PgConnection, connect, version_warning};
pub use driver::PgDriver;

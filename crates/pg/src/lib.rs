//! dbee Postgres connection (layer L3): connecting over TCP or an SSH tunnel, TLS, read-only
//! mode, cancel, and running SQL text over the simple-query protocol.
//!
//! Values are text exactly as Postgres prints them; no prepared statements carry data. Every
//! session runs with `standard_conforming_strings = on` and a UTF-8 `client_encoding`, which the
//! quoting in `dbee-sql` relies on. Logs name hosts, ports and error codes only: never SQL text,
//! passwords, server messages or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod connect;
pub mod error;
pub mod query;
pub mod tls;

pub use connect::{CONNECT_TIMEOUT, Canceller, MIN_VERSION_NUM, PgConnection, ServerInfo, Target, connect, version_warning};
pub use error::PgError;
pub use query::{Limits, QUERY_LIMITS};

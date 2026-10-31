//! dbee MySQL and MariaDB driver (layer L4): dbee's own client for MySQL's protocol. `mysql_common`
//! gives the packets, the login scrambles and RSA; the I/O is dbee's: TCP or a channel through
//! the SSH tunnel, upgraded to TLS with `dbee-driver`'s setup.
//!
//! - [`connect`] opens a [`MySqlConnection`]: TLS, the login, the session setup, read-only mode.
//! - [`Canceller`] stops the running statement with `KILL QUERY` over a second connection.
//! - [`Flavor`] tells MySQL from MariaDB by the server's version; both speak `Dialect::MySql`.
//!
//! Every length, count and offset a server sends is checked before it is used, and no buffer
//! grows past the bytes that arrived. Logs name hosts, ports, login methods and error codes
//! only: never SQL text, passwords, scrambles, server messages or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod auth;
pub mod connect;
mod errors;
pub mod wire;

pub use connect::{Canceller, Flavor, MySqlConnection, connect, version_num, version_warning};
pub use wire::TextResult;

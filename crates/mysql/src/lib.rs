//! dbee MySQL and MariaDB driver (layer L4): dbee's own client for MySQL's protocol. `mysql_common`
//! gives most packets, the login scrambles and RSA; dbee reads the column definitions and rows
//! itself, and the I/O is dbee's: TCP or a channel through the SSH tunnel, upgraded to TLS with
//! `dbee-driver`'s setup.
//!
//! - [`connect`] opens a [`MySqlConnection`]: TLS, the login, the session setup, read-only mode.
//! - [`MySqlConnection::run`] runs SQL text as one `COM_QUERY`, within the row and byte caps,
//!   with values as text as the server sends them (binary ones as `0x` and hex) and each column
//!   typed from its definition; [`MySqlConnection::apply`] writes staged changes all or nothing.
//! - [`Canceller`] stops the running statement with `KILL QUERY` over a second connection.
//! - [`Flavor`] tells MySQL from MariaDB by the server's version; both speak `Dialect::MySql`.
//!
//! Every length, count and offset a server sends is checked before it is used, and no buffer
//! grows past the bytes that arrived. Logs name hosts, ports, login methods and error codes
//! only: never SQL text, passwords, scrambles, server messages or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod apply;
mod auth;
pub mod cancel;
pub mod connect;
mod errors;
pub mod flavor;
mod query;
mod rsa;
mod values;
pub mod wire;

pub use cancel::Canceller;
pub use connect::{MySqlConnection, connect};
pub use flavor::{Flavor, version_num, version_warning};

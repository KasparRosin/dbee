//! dbee SQLite driver (layer L4), on `rusqlite` with the SQLite library compiled in.
//!
//! - [`connect`] opens a database file as a [`SqliteConnection`]: read-write, or read-only, with
//!   foreign keys on, a 5 s busy timeout and a check that the file is a database. A missing file is
//!   made only when the caller asks.
//! - [`SqliteConnection::run`] prepares and steps each statement of the text in turn, on a
//!   blocking thread, within the row and byte caps; values become text by fixed rules (reals as
//!   SQLite's own `CAST(x AS TEXT)` prints them, blobs as `X'…'`), and each column's class comes
//!   from its declared type by SQLite's affinity rules. Text holding a NUL never reaches SQLite,
//!   which would end the statement there. [`SqliteConnection::apply`] writes staged changes all or
//!   nothing.
//! - [`SqliteConnection::load_catalog`] reads `main`'s tables and views from `sqlite_schema` and
//!   the pragmas.
//! - [`Canceller`] interrupts the call running on a connection, never a later one.
//! - [`SqliteDriver`] puts all of it behind the driver interface.
//!
//! Logs carry error codes only: never file names, SQL text or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod apply;
mod catalog;
pub mod connect;
pub mod driver;
mod errors;
mod query;
mod structure;
mod values;

pub use connect::{BUSY_TIMEOUT, Canceller, SqliteConnection, connect};
pub use driver::SqliteDriver;

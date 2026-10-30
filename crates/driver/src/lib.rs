//! dbee driver interface (layer L3): what every database driver provides, so the engine holds
//! connections without naming a driver.
//!
//! - [`Driver`] opens a [`Connection`] for the database types it serves; [`Cancel`] stops what
//!   runs on one. [`Drivers`] finds the driver for a type.
//! - [`DbError`] is every driver's error, with [`Meaning`]s for the server errors dbee acts on.
//! - Shared by the drivers: the [`ApplyGate`], the TLS setup ([`tls`]) and the catalog building
//!   ([`catalog`]).
//!
//! Values are text exactly as the server prints them. Drivers log hosts, ports and error codes
//! only: never SQL text, passwords, server messages or cell values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod api;
pub mod apply;
pub mod catalog;
pub mod error;
pub mod registry;
pub mod tls;

pub use api::{
    BoxFuture, CATALOG_LIMITS, CONNECT_TIMEOUT, Cancel, Connection, DbFuture, Driver, Limits, PAGE_LIMITS, Purpose, QUERY_LIMITS, ServerInfo, Target,
};
pub use apply::{ApplyError, ApplyGate};
pub use catalog::CatalogBuilder;
pub use error::{DbError, Meaning, ServerError};
pub use registry::Drivers;

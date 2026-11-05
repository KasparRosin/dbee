//! dbee core (layer L0): cells, result sets, the schema model, grid text and copy formats.
//!
//! Plain values with serde shapes (camelCase); no I/O, no database, no UI.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cell;
pub mod copy;
pub mod display;
pub mod json;
pub mod schema;

pub use cell::{Cell, Column, DatabaseAfter, ResultSet, TypeClass};
pub use copy::CopyFormat;
pub use schema::{Catalog, ColumnInfo, ForeignKey, Generated, RelKind, SchemaInfo, TableInfo, TableRef};

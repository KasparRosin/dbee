//! dbee SQL (layer L1): each engine's dialect (quoting, literals and its `Rules`), a SQL lexer and
//! statement splitting, and the statements dbee builds: the filter's `WHERE`, the page and count
//! `SELECT`s and the edit statements.
//!
//! Pure text in, text out: no database, no I/O, no UI. Every value dbee writes or filters on is
//! quoted here, so the functions are total and never panic on hostile input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod dialect;
pub mod edit;
pub mod error;
pub mod filter;
pub mod lexer;
pub mod select;

pub use dialect::Dialect;
pub use error::SqlError;
pub use lexer::{Token, TokenKind};

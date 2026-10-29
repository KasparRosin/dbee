//! Errors from building SQL text.

use std::fmt;

/// Why a piece of SQL could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlError {
    /// A value or name contains NUL, which no supported database can store or parse.
    Nul,
    /// An `IN` filter with no values.
    EmptyIn,
    /// A column name that is not in the table.
    UnknownColumn(String),
    /// A statement that must be a single statement contains a top-level `;`.
    Semicolon,
    /// An edit with nothing to write.
    NoChanges,
    /// A row edit on a table with no usable key.
    NoKey,
    /// A binary value that is not hex text.
    BadBinary,
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlError::Nul => f.write_str("value contains a NUL character"),
            SqlError::EmptyIn => f.write_str("an IN filter needs at least one value"),
            SqlError::UnknownColumn(c) => write!(f, "unknown column \"{c}\""),
            SqlError::Semicolon => f.write_str("the statement contains a semicolon"),
            SqlError::NoChanges => f.write_str("there are no changes to apply"),
            SqlError::NoKey => f.write_str("the table has no primary key or unique key to identify the row"),
            SqlError::BadBinary => f.write_str("the binary value is not valid hex"),
        }
    }
}

impl std::error::Error for SqlError {}

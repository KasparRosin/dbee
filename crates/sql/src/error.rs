//! Errors from building SQL text.

use std::fmt;

/// Why a piece of SQL could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlError {
    /// A name contains NUL, which no engine allows, or a value does where the statement cannot
    /// hold one ([`crate::Dialect::statement_may_hold_nul`]).
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
    /// A filter or key compares a type by an operator its operator class lacks.
    NoComparison { type_name: String, op: &'static str },
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
            SqlError::NoComparison { type_name, op } => write!(f, "{type_name} values can't be compared with {op}; choose another comparison"),
        }
    }
}

impl std::error::Error for SqlError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        assert_eq!(SqlError::Nul.to_string(), "value contains a NUL character");
        assert!(SqlError::UnknownColumn("x".into()).to_string().contains('x'));
    }
}

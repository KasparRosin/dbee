//! Postgres casts of a value; the quoting itself is [`crate::dialect::Dialect`].

use crate::SqlError;
use crate::dialect::Dialect;

/// `CAST('value' AS type_sql)`. `type_sql` comes from the server's `format_type` (or is a
/// type without its length, from the Postgres dialect) and is used verbatim.
pub fn typed(value: &str, type_sql: &str) -> Result<String, SqlError> {
    Ok(format!("CAST({} AS {type_sql})", Dialect::Postgres.literal(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqlError;

    #[test]
    fn typed_casts_a_literal() {
        assert_eq!(typed("42", "integer").unwrap(), "CAST('42' AS integer)");
        assert_eq!(typed("a\0", "text"), Err(SqlError::Nul));
    }

    #[test]
    fn error_display() {
        assert_eq!(SqlError::Nul.to_string(), "value contains a NUL character");
        assert!(SqlError::UnknownColumn("x".into()).to_string().contains('x'));
    }
}

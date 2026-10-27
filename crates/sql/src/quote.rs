//! Identifier and literal quoting. Every value dbee writes or filters on goes through here.
//!
//! Literals rely on `standard_conforming_strings = on` (dbee sets it on every connection), so a
//! backslash is an ordinary character and only `'` needs doubling.

use crate::SqlError;
use dbee_core::TableRef;

/// `"name"` with inner `"` doubled. NUL is an error.
pub fn ident(name: &str) -> Result<String, SqlError> {
    if name.contains('\0') {
        return Err(SqlError::Nul);
    }
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

/// `"schema"."name"`.
pub fn table(t: &TableRef) -> Result<String, SqlError> {
    Ok(format!("{}.{}", ident(&t.schema)?, ident(&t.name)?))
}

/// `'value'` with `'` doubled; backslashes stay literal. NUL is an error.
pub fn literal(value: &str) -> Result<String, SqlError> {
    if value.contains('\0') {
        return Err(SqlError::Nul);
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    Ok(out)
}

/// `CAST('value' AS type_sql)`. `type_sql` comes from the server's `format_type` (or is a
/// type without its length, `ColumnInfo::write_type_sql`) and is used verbatim.
pub fn typed(value: &str, type_sql: &str) -> Result<String, SqlError> {
    Ok(format!("CAST({} AS {type_sql})", literal(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqlError;

    #[test]
    fn quoting() {
        assert_eq!(ident("a\"b").unwrap(), "\"a\"\"b\"");
        assert_eq!(ident("Odd \"Name\" 🐝").unwrap(), "\"Odd \"\"Name\"\" 🐝\"");
        assert_eq!(literal("it's").unwrap(), "'it''s'");
        assert_eq!(literal(r"a\b").unwrap(), r"'a\b'");
        assert_eq!(literal("a\0b"), Err(SqlError::Nul));
        assert_eq!(ident("a\0b"), Err(SqlError::Nul));
        assert_eq!(typed("42", "integer").unwrap(), "CAST('42' AS integer)");
        assert_eq!(typed("a\0", "text"), Err(SqlError::Nul));
        let t = dbee_core::TableRef { schema: "My \"S\"".into(), name: "t".into() };
        assert_eq!(table(&t).unwrap(), "\"My \"\"S\"\"\".\"t\"");
    }

    #[test]
    fn error_display() {
        assert_eq!(SqlError::Nul.to_string(), "value contains a NUL character");
        assert!(SqlError::UnknownColumn("x".into()).to_string().contains('x'));
    }
}

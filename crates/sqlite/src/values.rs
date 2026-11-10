//! What a SQLite column holds, from its declared type by SQLite's affinity rules, and its values
//! as text: integers in decimal, reals as `CAST(x AS TEXT)` prints them, text as stored, blobs as
//! `X'…'` with uppercase hex.

use dbee_core::{Column, TypeClass};

/// A column's type affinity, from its declared type (SQLite's "Datatypes In SQLite", 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affinity {
    Integer,
    Text,
    Blob,
    Real,
    Numeric,
}

/// The affinity of `declared`, by SQLite's rules in their order: a type holding `INT` is
/// INTEGER; `CHAR`, `CLOB` or `TEXT`, TEXT; `BLOB` or no type, BLOB; `REAL`, `FLOA` or `DOUB`,
/// REAL; any other, NUMERIC. Case does not count.
fn affinity(declared: &str) -> Affinity {
    let t = declared.to_ascii_uppercase();
    let has = |parts: &[&str]| parts.iter().any(|p| t.contains(p));
    if has(&["INT"]) {
        Affinity::Integer
    } else if has(&["CHAR", "CLOB", "TEXT"]) {
        Affinity::Text
    } else if has(&["BLOB"]) || t.trim().is_empty() {
        Affinity::Blob
    } else if has(&["REAL", "FLOA", "DOUB"]) {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

/// The declared type's name: what comes before its size, as written (`VARCHAR` of
/// `VARCHAR(36)`).
pub(crate) fn base_type(declared: &str) -> &str {
    declared.split('(').next().unwrap_or_default().trim()
}

/// The class of a column declared `declared` (`""` for no type, as an expression has none).
///
/// By the type's affinity: INTEGER, NUMERIC and REAL are numbers, TEXT is long text, BLOB is
/// bytes. A type of NUMERIC or INTEGER affinity named `BOOL` or `BOOLEAN` is true/false, `JSON`
/// is JSON, and `DATE`, `DATETIME` or `TIMESTAMP` a date and time; one of TEXT affinity named
/// `CHAR`, `VARCHAR`, `NCHAR`, `NVARCHAR` or `CHARACTER` is one line of text. No type at all
/// holds anything, shown as it comes.
pub(crate) fn class_of(declared: &str) -> TypeClass {
    if declared.trim().is_empty() {
        return TypeClass::Other;
    }
    let name = base_type(declared).split_ascii_whitespace().next().unwrap_or_default().to_ascii_uppercase();
    match affinity(declared) {
        Affinity::Integer | Affinity::Numeric => match name.as_str() {
            "BOOL" | "BOOLEAN" => TypeClass::Bool,
            "JSON" => TypeClass::Json,
            "DATE" | "DATETIME" | "TIMESTAMP" => TypeClass::DateTime,
            _ => TypeClass::Number,
        },
        Affinity::Real => TypeClass::Number,
        Affinity::Text => match name.as_str() {
            "CHAR" | "VARCHAR" | "NCHAR" | "NVARCHAR" | "CHARACTER" => TypeClass::Text,
            _ => TypeClass::LongText,
        },
        Affinity::Blob => TypeClass::Binary,
    }
}

/// The result column `name` declared `declared` (`None` for an expression).
pub(crate) fn column(name: &str, declared: Option<&str>) -> Column {
    let declared = declared.unwrap_or_default();
    Column { name: name.to_owned(), base_type: base_type(declared).to_owned(), type_label: declared.to_owned(), class: class_of(declared) }
}

/// A value, before its text is built: [`len`](CellText::len) is the length that text will have,
/// counted without building it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CellText<'a> {
    /// An integer or a real, already as SQLite prints it.
    Number(String),
    /// Bytes shown as UTF-8, each invalid sequence replaced by U+FFFD (SQLite keeps a TEXT's
    /// bytes unchecked).
    Text(&'a [u8]),
    /// Bytes, written `X'…'` in uppercase hex.
    Blob(&'a [u8]),
}

/// The length of U+FFFD in UTF-8, which stands for each invalid sequence.
const REPLACEMENT_LEN: usize = '\u{FFFD}'.len_utf8();

impl CellText<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            CellText::Number(text) => text.len(),
            CellText::Text(bytes) => bytes.utf8_chunks().fold(0usize, |sum, chunk| {
                let replaced = if chunk.invalid().is_empty() { 0 } else { REPLACEMENT_LEN };
                sum.saturating_add(chunk.valid().len()).saturating_add(replaced)
            }),
            CellText::Blob(bytes) => bytes.len().saturating_mul(2).saturating_add(3),
        }
    }

    pub(crate) fn into_string(self) -> String {
        match self {
            CellText::Number(text) => text,
            CellText::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            CellText::Blob(bytes) => {
                let mut text = String::with_capacity(bytes.len().saturating_mul(2).saturating_add(3));
                text.push_str("X'");
                for &b in bytes {
                    text.push(hex_digit(b >> 4));
                    text.push(hex_digit(b & 0x0F));
                }
                text.push('\'');
                text
            }
        }
    }
}

/// The uppercase hex digit of `n`, which is below 16.
fn hex_digit(n: u8) -> char {
    char::from(if n < 10 { b'0' + n } else { b'A' + (n - 10) })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn affinity_by_sqlites_rules() {
        use Affinity::*;
        for (declared, expected) in [
            ("INTEGER", Integer),
            ("int", Integer),
            ("TINYINT(1)", Integer),
            ("UNSIGNED BIG INT", Integer),
            ("INT8", Integer),
            // The rules' order: INT first, so a POINT is an integer and a CHARINT too.
            ("POINT", Integer),
            ("CHARINT", Integer),
            ("FLOATING POINT", Integer),
            ("VARCHAR(255)", Text),
            ("NCHAR(55)", Text),
            ("CLOB", Text),
            ("text", Text),
            ("BLOB", Blob),
            ("", Blob),
            ("  ", Blob),
            ("REAL", Real),
            ("DOUBLE PRECISION", Real),
            ("FLOAT", Real),
            ("NUMERIC", Numeric),
            ("DECIMAL(10,5)", Numeric),
            ("BOOLEAN", Numeric),
            ("DATE", Numeric),
            ("DATETIME", Numeric),
            ("JSON", Numeric),
            ("STRING", Numeric),
            ("quoted type", Numeric),
        ] {
            assert_eq!(affinity(declared), expected, "{declared:?}");
        }
    }

    #[test]
    fn classes() {
        use TypeClass::*;
        for (declared, expected) in [
            ("INTEGER", Number),
            ("NUMERIC(12,2)", Number),
            ("DECIMAL(30,10)", Number),
            ("REAL", Number),
            ("DOUBLE PRECISION", Number),
            ("TINYINT(1)", Number),
            ("BOOL", Bool),
            ("boolean", Bool),
            ("JSON", Json),
            ("json", Json),
            ("DATE", DateTime),
            ("DATETIME", DateTime),
            ("datetime(3)", DateTime),
            ("TIMESTAMP", DateTime),
            ("timestamp with time zone", DateTime),
            ("TEXT", LongText),
            ("CLOB", LongText),
            ("VARCHAR(36)", Text),
            ("varchar", Text),
            ("CHAR(2)", Text),
            ("NCHAR(55)", Text),
            ("NVARCHAR(100)", Text),
            ("CHARACTER(20)", Text),
            ("character varying(10)", Text),
            // A name is a class only where the affinity is NUMERIC or INTEGER.
            ("JSON TEXT", LongText),
            ("DATE TEXT", LongText),
            ("BLOB", Binary),
            ("", Other),
            ("STRING", Number),
        ] {
            assert_eq!(class_of(declared), expected, "{declared:?}");
        }
    }

    #[test]
    fn columns_name_their_types() {
        let c = column("total", Some("NUMERIC(12,2)"));
        assert_eq!((c.name.as_str(), c.base_type.as_str(), c.type_label.as_str(), c.class), ("total", "NUMERIC", "NUMERIC(12,2)", TypeClass::Number));
        let e = column("x + 1", None);
        assert_eq!((e.base_type.as_str(), e.type_label.as_str(), e.class), ("", "", TypeClass::Other));
        assert_eq!(base_type("VARCHAR (20)"), "VARCHAR");
        assert_eq!(base_type("timestamp with time zone"), "timestamp with time zone");
    }

    #[test]
    fn values_as_text() {
        assert_eq!(CellText::Blob(&[0x00, 0xFF, 0x0A, 0xDE]).into_string(), "X'00FF0ADE'");
        assert_eq!(CellText::Blob(&[]).into_string(), "X''");
        assert_eq!(CellText::Text(b"plain").into_string(), "plain");
        assert_eq!(CellText::Text(&[b'1', 0xE9, b'2']).into_string(), "1\u{FFFD}2");
        assert_eq!(CellText::Text(&[0xE9]).len(), 3, "the replacement counts as its bytes");
        assert_eq!(CellText::Number("1.0e+300".into()).len(), 8);
    }

    /// Bytes that are mostly UTF-8 with some bytes changed, or any bytes.
    fn mostly_text() -> impl Strategy<Value = Vec<u8>> {
        let changed = (any::<String>(), prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..6)).prop_map(|(text, edits)| {
            let mut bytes = text.into_bytes();
            for (at, byte) in edits {
                if !bytes.is_empty() {
                    let i = at.index(bytes.len());
                    bytes[i] = byte;
                }
            }
            bytes
        });
        prop_oneof![changed, prop::collection::vec(any::<u8>(), 0..200)]
    }

    proptest! {
        #[test]
        fn a_cells_length_is_its_texts(bytes in mostly_text(), blob in any::<bool>()) {
            let cell = if blob { CellText::Blob(&bytes) } else { CellText::Text(&bytes) };
            let len = cell.len();
            let text = cell.into_string();
            prop_assert_eq!(len, text.len());
            if blob {
                prop_assert!(text.starts_with("X'") && text.ends_with('\''));
                prop_assert!(text[2..text.len() - 1].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()));
            } else {
                prop_assert_eq!(text, String::from_utf8_lossy(&bytes).into_owned());
            }
        }

        #[test]
        fn every_declared_type_gets_a_class(declared in "\\PC{0,40}") {
            let c = column("c", Some(&declared));
            prop_assert_eq!(c.class, class_of(&declared));
            prop_assert!(declared.contains(c.base_type.as_str()));
            prop_assert_eq!(c.type_label, declared);
        }
    }
}

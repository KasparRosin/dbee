//! What a MySQL or MariaDB column holds, from the facts its column definition gives (which the
//! catalog can give too), and its values as text: as the server sends them, binary ones as `0x`
//! and hex.

use std::borrow::Cow;

use dbee_core::{Column, TypeClass};
use mysql_common::collations::{Collation, CollationId};
use mysql_common::constants::{ColumnFlags, ColumnType};

/// The character set number of `binary`: its strings hold bytes, not text.
const BINARY: u16 = 63;

/// What a column definition says about a column's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypeFacts {
    pub(crate) column_type: ColumnType,
    pub(crate) flags: ColumnFlags,
    /// The values' character set, as a collation number; [`BINARY`] for bytes.
    pub(crate) charset: u16,
    /// The longest value: for strings, in bytes of `charset`.
    pub(crate) length: u32,
    /// The values are JSON: MariaDB, which has no JSON type, says so in its extended metadata.
    pub(crate) json: bool,
}

impl TypeFacts {
    fn binary(&self) -> bool {
        self.charset == BINARY
    }

    /// The longest value in characters, when the character set is known.
    fn chars(&self) -> Option<u32> {
        let id = CollationId::from(self.charset);
        let per_char = match id {
            _ if self.binary() => 1,
            CollationId::UNKNOWN_COLLATION_ID => 0,
            known => Collation::from(known).max_len(),
        };
        self.length.checked_div(u32::from(per_char))
    }
}

/// The class of a column with type facts `t`. `TINYINT(1)` (`BOOL`) is a true/false column;
/// strings and blobs in the `binary` character set hold bytes, as do `BIT` and the other types
/// MySQL sends as bytes (geometries, vectors).
pub(crate) fn class_of(t: &TypeFacts) -> TypeClass {
    use ColumnType::*;
    if t.json {
        return TypeClass::Json;
    }
    if t.flags.intersects(ColumnFlags::ENUM_FLAG | ColumnFlags::SET_FLAG) {
        return TypeClass::Text;
    }
    match t.column_type {
        MYSQL_TYPE_JSON => TypeClass::Json,
        MYSQL_TYPE_TINY if t.length == 1 => TypeClass::Bool,
        MYSQL_TYPE_TINY
        | MYSQL_TYPE_SHORT
        | MYSQL_TYPE_INT24
        | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_DECIMAL
        | MYSQL_TYPE_NEWDECIMAL
        | MYSQL_TYPE_FLOAT
        | MYSQL_TYPE_DOUBLE
        | MYSQL_TYPE_YEAR => TypeClass::Number,
        MYSQL_TYPE_DATE
        | MYSQL_TYPE_NEWDATE
        | MYSQL_TYPE_TIME
        | MYSQL_TYPE_TIME2
        | MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => TypeClass::DateTime,
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => TypeClass::Text,
        MYSQL_TYPE_BIT => TypeClass::Binary,
        MYSQL_TYPE_STRING | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR if !t.binary() => TypeClass::Text,
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB if !t.binary() => TypeClass::LongText,
        MYSQL_TYPE_NULL => TypeClass::Other,
        _ if t.binary() => TypeClass::Binary,
        _ => TypeClass::Other,
    }
}

/// The type's name as MySQL writes it in a column's type (`varchar`, `int`, `json`), or `""`
/// when it has none.
pub(crate) fn type_name(t: &TypeFacts) -> &'static str {
    use ColumnType::*;
    if t.json {
        return "json";
    }
    if t.flags.contains(ColumnFlags::ENUM_FLAG) {
        return "enum";
    }
    if t.flags.contains(ColumnFlags::SET_FLAG) {
        return "set";
    }
    let binary = t.binary();
    match t.column_type {
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_INT24 => "mediumint",
        MYSQL_TYPE_LONG => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_NULL => "null",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime",
        MYSQL_TYPE_YEAR => "year",
        MYSQL_TYPE_BIT => "bit",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_ENUM => "enum",
        MYSQL_TYPE_SET => "set",
        MYSQL_TYPE_GEOMETRY => "geometry",
        MYSQL_TYPE_VECTOR => "vector",
        MYSQL_TYPE_STRING if binary => "binary",
        MYSQL_TYPE_STRING => "char",
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR if binary => "varbinary",
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR => "varchar",
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB => blob_name(t),
        MYSQL_TYPE_TYPED_ARRAY | MYSQL_TYPE_UNKNOWN => "",
    }
}

/// A blob's or text's name by the most it holds; the server sends every one as a blob.
fn blob_name(t: &TypeFacts) -> &'static str {
    let names = if t.binary() { ["tinyblob", "blob", "mediumblob", "longblob"] } else { ["tinytext", "text", "mediumtext", "longtext"] };
    let [tiny, plain, medium, long] = names;
    match t.chars().unwrap_or(t.length) {
        0..=0xFF => tiny,
        0x100..=0xFFFF => plain,
        0x1_0000..=0xFF_FFFF => medium,
        _ => long,
    }
}

/// The result column `name` with type facts `t`: its type's name, the name with the length in
/// characters for strings (`varchar(45)`), and its class.
pub(crate) fn column(name: String, t: &TypeFacts) -> Column {
    let base = type_name(t);
    let sized = matches!(t.column_type, ColumnType::MYSQL_TYPE_STRING | ColumnType::MYSQL_TYPE_VAR_STRING | ColumnType::MYSQL_TYPE_VARCHAR)
        && !t.flags.intersects(ColumnFlags::ENUM_FLAG | ColumnFlags::SET_FLAG);
    let type_label = match t.chars() {
        Some(n) if sized => format!("{base}({n})"),
        _ => base.to_owned(),
    };
    Column { name, base_type: base.to_owned(), type_label, class: class_of(t) }
}

/// A value of a column, before it is made text; its [`len`](Cell::len) is the length of that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell<'a> {
    /// The bytes as UTF-8, invalid sequences replaced.
    Text(Cow<'a, str>),
    /// Bytes, written `0x` and uppercase hex.
    Hex(&'a [u8]),
}

impl Cell<'_> {
    /// A value of a column of class `class`.
    pub(crate) fn of(class: TypeClass, bytes: &[u8]) -> Cell<'_> {
        match class {
            TypeClass::Binary => Cell::Hex(bytes),
            _ => Cell::Text(String::from_utf8_lossy(bytes)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Cell::Text(text) => text.len(),
            Cell::Hex(bytes) => bytes.len().saturating_mul(2).saturating_add(2),
        }
    }

    pub(crate) fn into_string(self) -> String {
        match self {
            Cell::Text(text) => text.into_owned(),
            Cell::Hex(bytes) => {
                let mut text = String::with_capacity(bytes.len().saturating_mul(2).saturating_add(2));
                text.push_str("0x");
                for &b in bytes {
                    text.push(hex_digit(b >> 4));
                    text.push(hex_digit(b & 0x0F));
                }
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

    const UTF8MB4: u16 = 45;
    const LATIN1: u16 = 8;

    fn facts(column_type: ColumnType, flags: ColumnFlags, charset: u16, length: u32) -> TypeFacts {
        TypeFacts { column_type, flags, charset, length, json: false }
    }

    fn of(column_type: ColumnType, charset: u16, length: u32) -> TypeFacts {
        facts(column_type, ColumnFlags::empty(), charset, length)
    }

    #[test]
    fn classes() {
        use ColumnType::*;
        let class = |t: TypeFacts| class_of(&t);
        assert_eq!(class(of(MYSQL_TYPE_TINY, BINARY, 1)), TypeClass::Bool);
        for length in [2, 3, 4] {
            assert_eq!(class(of(MYSQL_TYPE_TINY, BINARY, length)), TypeClass::Number, "{length}");
        }
        for t in [MYSQL_TYPE_SHORT, MYSQL_TYPE_INT24, MYSQL_TYPE_LONG, MYSQL_TYPE_LONGLONG, MYSQL_TYPE_NEWDECIMAL, MYSQL_TYPE_DOUBLE, MYSQL_TYPE_YEAR]
        {
            assert_eq!(class(of(t, BINARY, 1)), TypeClass::Number, "{t:?}");
        }
        for t in [MYSQL_TYPE_DATE, MYSQL_TYPE_TIME, MYSQL_TYPE_DATETIME, MYSQL_TYPE_TIMESTAMP] {
            assert_eq!(class(of(t, BINARY, 19)), TypeClass::DateTime, "{t:?}");
        }
        assert_eq!(class(of(MYSQL_TYPE_JSON, BINARY, u32::MAX)), TypeClass::Json);
        assert_eq!(class(of(MYSQL_TYPE_JSON, UTF8MB4, u32::MAX)), TypeClass::Json);
        // MariaDB's JSON is a text that its extended metadata names.
        let mariadb_json = TypeFacts { json: true, ..facts(MYSQL_TYPE_BLOB, ColumnFlags::BLOB_FLAG | ColumnFlags::BINARY_FLAG, UTF8MB4, u32::MAX) };
        assert_eq!((class(mariadb_json), type_name(&mariadb_json)), (TypeClass::Json, "json"));
        assert_eq!(class(TypeFacts { json: false, ..mariadb_json }), TypeClass::LongText);
        // Strings and blobs: text in any character set but binary.
        assert_eq!(class(of(MYSQL_TYPE_VAR_STRING, UTF8MB4, 180)), TypeClass::Text);
        assert_eq!(class(of(MYSQL_TYPE_STRING, LATIN1, 3)), TypeClass::Text);
        assert_eq!(class(of(MYSQL_TYPE_VAR_STRING, BINARY, 16)), TypeClass::Binary);
        assert_eq!(class(of(MYSQL_TYPE_BLOB, UTF8MB4, 262_140)), TypeClass::LongText);
        assert_eq!(class(of(MYSQL_TYPE_MEDIUM_BLOB, BINARY, 1)), TypeClass::Binary);
        // A binary collation of a text set (utf8mb4_bin, with the BINARY flag) is still text.
        assert_eq!(class(facts(MYSQL_TYPE_VAR_STRING, ColumnFlags::BINARY_FLAG, 46, 80)), TypeClass::Text);
        for flag in [ColumnFlags::ENUM_FLAG, ColumnFlags::SET_FLAG] {
            assert_eq!(class(facts(MYSQL_TYPE_STRING, flag, UTF8MB4, 36)), TypeClass::Text);
        }
        assert_eq!(class(of(MYSQL_TYPE_BIT, BINARY, 8)), TypeClass::Binary);
        // Geometries and vectors come as bytes; NULL and unknown text have no class.
        assert_eq!(class(of(MYSQL_TYPE_GEOMETRY, BINARY, u32::MAX)), TypeClass::Binary);
        assert_eq!(class(of(MYSQL_TYPE_VECTOR, BINARY, 64)), TypeClass::Binary);
        assert_eq!(class(of(MYSQL_TYPE_NULL, BINARY, 0)), TypeClass::Other);
        assert_eq!(class(of(MYSQL_TYPE_UNKNOWN, UTF8MB4, 0)), TypeClass::Other);
    }

    #[test]
    fn names_and_labels() {
        use ColumnType::*;
        let named = |t: TypeFacts| {
            let c = column("c".into(), &t);
            (c.base_type, c.type_label)
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(named(of(MYSQL_TYPE_VAR_STRING, UTF8MB4, 180)), pair("varchar", "varchar(45)"));
        assert_eq!(named(of(MYSQL_TYPE_VAR_STRING, 255, 180)), pair("varchar", "varchar(45)"));
        assert_eq!(named(of(MYSQL_TYPE_STRING, LATIN1, 3)), pair("char", "char(3)"));
        assert_eq!(named(of(MYSQL_TYPE_STRING, BINARY, 16)), pair("binary", "binary(16)"));
        assert_eq!(named(of(MYSQL_TYPE_VAR_STRING, BINARY, 10)), pair("varbinary", "varbinary(10)"));
        // A character set dbee does not know: no length rather than a wrong one.
        assert_eq!(named(of(MYSQL_TYPE_VAR_STRING, 4000, 180)), pair("varchar", "varchar"));
        assert_eq!(named(facts(MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG, UTF8MB4, 36)), pair("enum", "enum"));
        assert_eq!(named(of(MYSQL_TYPE_LONG, BINARY, 11)), pair("int", "int"));
        // Every blob comes as a blob: its name by the most it holds, in characters.
        for (length, text, blob) in [(1020, "tinytext", "blob"), (262_140, "text", "mediumblob"), (67_108_860, "mediumtext", "longblob")] {
            assert_eq!(named(of(MYSQL_TYPE_BLOB, UTF8MB4, length)).0, text, "{length}");
            assert_eq!(named(of(MYSQL_TYPE_BLOB, BINARY, length)).0, blob, "{length}");
        }
        assert_eq!(named(of(MYSQL_TYPE_BLOB, BINARY, 255)).0, "tinyblob");
        assert_eq!(named(of(MYSQL_TYPE_BLOB, BINARY, 65_535)).0, "blob");
        assert_eq!(named(of(MYSQL_TYPE_LONG_BLOB, UTF8MB4, u32::MAX)).0, "longtext");
    }

    #[test]
    fn values_as_text() {
        let text = |class, bytes: &[u8]| Cell::of(class, bytes).into_string();
        assert_eq!(text(TypeClass::Binary, &[0x00, 0xFF, 0x0A, 0xDE]), "0x00FF0ADE");
        assert_eq!(text(TypeClass::Binary, &[]), "0x");
        assert_eq!(text(TypeClass::Text, b"plain"), "plain");
        assert_eq!(text(TypeClass::Number, &[b'1', 0xE9, b'2']), "1\u{FFFD}2");
        assert_eq!(Cell::of(TypeClass::Text, &[0xE9]).len(), 3, "the replacement counts as its bytes");
    }

    proptest! {
        #[test]
        fn a_cells_length_is_its_texts(bytes in prop::collection::vec(any::<u8>(), 0..200), binary in any::<bool>()) {
            let class = if binary { TypeClass::Binary } else { TypeClass::LongText };
            let cell = Cell::of(class, &bytes);
            let len = cell.len();
            let text = cell.into_string();
            prop_assert_eq!(len, text.len());
            if binary {
                prop_assert!(text.starts_with("0x") && text[2..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()));
            }
        }
    }
}

//! The SQL text differences between Postgres, MySQL (and MariaDB) and SQLite, in one place.

use crate::filter::Op;
use crate::{SqlError, quote};
use dbee_core::{ColumnInfo, TableRef, TypeClass};
use serde::{Deserialize, Serialize};

mod mysql;
mod postgres;
mod sqlite;

/// The length of the longest word of a keyword list.
const fn longest(words: &[&str]) -> usize {
    let (mut most, mut k) = (0, 0);
    while k < words.len() {
        if words[k].len() > most {
            most = words[k].len();
        }
        k += 1;
    }
    most
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    Postgres,
    MySql,
    Sqlite,
}

fn no_nul(s: &str) -> Result<(), SqlError> {
    if s.contains('\0') { Err(SqlError::Nul) } else { Ok(()) }
}

/// `open` + `s` with `close` doubled + `close`.
fn quoted(s: &str, close: char) -> Result<String, SqlError> {
    no_nul(s)?;
    let mut out = String::with_capacity(s.len() + 2);
    out.push(close);
    for c in s.chars() {
        if c == close {
            out.push(close);
        }
        out.push(c);
    }
    out.push(close);
    Ok(out)
}

/// The hex digits of `0x…` (MySQL) or `X'…'` (SQLite), as an even number of them.
fn hex_digits(text: &str, sqlite: bool) -> Result<&str, SqlError> {
    let digits = if sqlite {
        text.strip_prefix(['X', 'x']).and_then(|t| t.strip_prefix('\'')).and_then(|t| t.strip_suffix('\''))
    } else {
        text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
    };
    digits.filter(|d| d.len() % 2 == 0 && d.bytes().all(|b| b.is_ascii_hexdigit())).ok_or(SqlError::BadBinary)
}

fn is_plain_decimal(text: &str) -> bool {
    let t = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = t.split_once('.').map_or((t, None), |(i, f)| (i, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

impl Dialect {
    /// The engine's keywords for highlighting: lowercase, sorted, unique.
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Dialect::Postgres => postgres::KEYWORDS,
            Dialect::MySql => mysql::KEYWORDS,
            Dialect::Sqlite => sqlite::KEYWORDS,
        }
    }

    /// True when `word` is one of the engine's keywords, in any case.
    pub fn is_keyword(self, word: &str) -> bool {
        let longest = match self {
            Dialect::Postgres => postgres::LONGEST,
            Dialect::MySql => mysql::LONGEST,
            Dialect::Sqlite => sqlite::LONGEST,
        };
        if word.len() > longest || !word.is_ascii() {
            return false;
        }
        self.keywords().binary_search(&word.to_ascii_lowercase().as_str()).is_ok()
    }

    /// `"name"` (Postgres, SQLite) or `` `name` `` (MySQL), the quote character doubled. NUL is an error.
    pub fn ident(self, name: &str) -> Result<String, SqlError> {
        quoted(name, if self == Dialect::MySql { '`' } else { '"' })
    }

    /// `schema.name`, both quoted (SQLite's schema is `main`).
    pub fn table(self, t: &TableRef) -> Result<String, SqlError> {
        Ok(format!("{}.{}", self.ident(&t.schema)?, self.ident(&t.name)?))
    }

    /// `'value'` with `'` doubled; backslashes stay literal, which Postgres gets from
    /// `standard_conforming_strings`. MySQL may or may not escape them, so a value with one is
    /// hex. NUL is an error.
    pub fn literal(self, value: &str) -> Result<String, SqlError> {
        if self == Dialect::MySql && value.contains('\\') {
            no_nul(value)?;
            let hex: String = value.bytes().map(|b| format!("{b:02X}")).collect();
            return Ok(format!("_utf8mb4 X'{hex}'"));
        }
        quoted(value, '\'')
    }

    /// A value to write into the column.
    pub fn value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        match self {
            Dialect::Postgres => quote::typed(text, pg_write_type(column)),
            Dialect::MySql if column.class == TypeClass::Binary => {
                let digits = hex_digits(text, false)?;
                Ok(if digits.is_empty() { "X''".to_string() } else { format!("0x{digits}") })
            }
            Dialect::Sqlite if column.class == TypeClass::Binary => Ok(format!("X'{}'", hex_digits(text, true)?)),
            Dialect::Sqlite if column.class == TypeClass::Number && is_plain_decimal(text) => Ok(text.to_string()),
            Dialect::MySql | Dialect::Sqlite => self.literal(text),
        }
    }

    /// A value of the row's key, which the server sent.
    pub fn key_value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        match self {
            Dialect::Postgres => quote::typed(text, &column.type_sql),
            _ => self.value(column, text),
        }
    }

    /// A value a filter compares the column with.
    pub fn filter_value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        match self {
            Dialect::Postgres => quote::typed(text, pg_filter_type(column)),
            _ => self.value(column, text),
        }
    }

    /// `expr` as text.
    pub fn as_text(self, expr: &str) -> String {
        let ty = match self {
            Dialect::Postgres => "text",
            Dialect::MySql => "CHAR",
            Dialect::Sqlite => "TEXT",
        };
        format!("CAST({expr} AS {ty})")
    }

    /// A pattern filter on the column's text: `like`, `not like`, `ilike` or `not ilike`.
    pub fn like(self, op: Op, column_sql: &str, pattern_sql: &str) -> String {
        let text = self.as_text(column_sql);
        match (self, op) {
            (Dialect::MySql | Dialect::Sqlite, Op::ILike | Op::NotILike) => {
                let not = if op == Op::NotILike { "NOT " } else { "" };
                format!("LOWER({text}) {not}LIKE LOWER({pattern_sql})")
            }
            _ => format!("{text} {} {pattern_sql}", op.symbol()),
        }
    }

    /// An insert that gives every column its default.
    pub fn insert_defaults(self, table_sql: &str) -> String {
        match self {
            Dialect::MySql => format!("INSERT INTO {table_sql} () VALUES ()"),
            _ => format!("INSERT INTO {table_sql} DEFAULT VALUES"),
        }
    }

    /// The statement that makes `name` the current database, on engines that have one.
    pub fn use_database(self, name: &str) -> Result<Option<String>, SqlError> {
        match self {
            Dialect::MySql => Ok(Some(format!("USE {}", self.ident(name)?))),
            _ => Ok(None),
        }
    }

    /// The texts a Bool column writes for true and false.
    pub fn bool_literals(self) -> (&'static str, &'static str) {
        match self {
            Dialect::Postgres => ("t", "f"),
            _ => ("1", "0"),
        }
    }

    /// What a filter operator does on this engine that the label does not say.
    pub fn op_note(self, op: Op) -> Option<&'static str> {
        match (self, op) {
            (Dialect::MySql, Op::Eq | Op::Ne | Op::Like | Op::NotLike) => {
                Some("Compares by the column's collation; most ignore upper and lower case.")
            }
            (Dialect::Sqlite, Op::Like | Op::NotLike) => Some("Ignores upper and lower case for A–Z only."),
            _ => None,
        }
    }
}

/// The type a value to write is cast to. Postgres defines an explicit cast to
/// `varchar(n)`, `char(n)` and `bit varying(n)` as cutting the value to the length, and to
/// `bit(n)` as cutting or padding it, without an error; only the assignment to the column (or
/// to a domain over such a type) refuses a value that does not fit. So these, their arrays
/// and domains over them are cast to the type without its length: `text`, `bit varying`,
/// `text[]` or `bit varying[]`. Every other type is cast to `type_sql` itself. A key value the
/// server sent fits its column, so key comparisons keep `type_sql`.
fn pg_write_type(column: &ColumnInfo) -> &str {
    match column.base_type.as_str() {
        "varchar" | "bpchar" => "text",
        "bit" | "varbit" => "bit varying",
        "_varchar" | "_bpchar" => "text[]",
        "_bit" | "_varbit" => "bit varying[]",
        _ => &column.type_sql,
    }
}

/// The type a filter value is cast to: the column's type without its length, so that a value
/// longer than the length matches nothing instead of the rows its cut prefix would match, chosen
/// to keep the column's own operators, and so its indexes: `char(n)` compares as `bpchar`
/// (trailing spaces do not count, as for the column), `bit(n)` as `"bit"` (quoted: a bare `bit`
/// means `bit(1)`), and arrays of `varchar(n)`, `char(n)` and `bit(n)` as `character varying[]`,
/// `bpchar[]` and `"bit"[]` (no operator compares them with `text[]` or `bit varying[]`). The
/// others are [`pg_write_type`]: `varchar(n)` as `text`, whose indexes it uses.
fn pg_filter_type(column: &ColumnInfo) -> &str {
    match column.base_type.as_str() {
        "bpchar" => "bpchar",
        "bit" => "\"bit\"",
        "_varchar" => "character varying[]",
        "_bpchar" => "bpchar[]",
        "_bit" => "\"bit\"[]",
        _ => pg_write_type(column),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_lists_are_lowercase_sorted_and_unique() {
        for d in ALL {
            let list = d.keywords();
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{d:?} must be sorted and unique");
            assert!(list.iter().all(|k| k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')), "{d:?}");
            for k in ["select", "from", "where", "join", "insert", "update", "delete"] {
                assert!(list.binary_search(&k).is_ok(), "{d:?} {k}");
            }
        }
    }

    #[test]
    fn the_longest_keyword_of_each_engine_is_found_and_longer_words_are_not() {
        for d in ALL {
            let longest = d.keywords().iter().copied().max_by_key(|k| k.len()).unwrap_or_default();
            assert!(d.is_keyword(longest) && d.is_keyword(&longest.to_ascii_uppercase()), "{d:?} {longest}");
            assert!(!d.is_keyword(&format!("{longest}x")), "{d:?}");
        }
        assert!(Dialect::MySql.is_keyword("master_ssl_verify_server_cert"));
        assert!(!Dialect::Postgres.is_keyword("master_ssl_verify_server_cert"));
        assert!(!Dialect::Sqlite.is_keyword("master_ssl_verify_server_cert"));
        assert!(Dialect::MySql.is_keyword("accessible") && !Dialect::Postgres.is_keyword("accessible"));
    }
    use crate::edit::{NewValue, delete_sql, insert_sql, update_sql};
    use crate::filter::{Condition, Filter, where_sql};
    use crate::select::{count_sql, page_sql};
    use dbee_core::{RelKind, TableInfo};
    use proptest::prelude::*;

    fn tr(schema: &str, name: &str) -> TableRef {
        TableRef { schema: schema.into(), name: name.into() }
    }

    fn of_class(name: &str, type_sql: &str, base: &str, class: TypeClass) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_sql: type_sql.into(),
            base_type: base.into(),
            class,
            not_null: false,
            has_default: false,
            generated: Default::default(),
            hidden: false,
        }
    }

    fn bin(name: &str) -> ColumnInfo {
        of_class(name, "blob", "blob", TypeClass::Binary)
    }

    fn num(name: &str) -> ColumnInfo {
        of_class(name, "integer", "int4", TypeClass::Number)
    }

    fn txt(name: &str) -> ColumnInfo {
        of_class(name, "text", "text", TypeClass::Text)
    }

    fn table_of(cols: Vec<ColumnInfo>) -> TableInfo {
        TableInfo {
            table: tr("shop", "orders"),
            kind: RelKind::Table,
            columns: cols,
            primary_key: vec!["id".into()],
            unique_keys: vec![],
            foreign_keys: vec![],
            estimated_rows: None,
            no_edit_reason: None,
        }
    }

    const ALL: [Dialect; 3] = [Dialect::Postgres, Dialect::MySql, Dialect::Sqlite];

    #[test]
    fn postgres_output_is_unchanged() {
        let pg = Dialect::Postgres;
        let mut total = of_class("total", "numeric(12,2)", "numeric", TypeClass::Number);
        total.not_null = true;
        let t = table_of(vec![
            of_class("id", "bigint", "int8", TypeClass::Number),
            total.clone(),
            of_class("note", "character varying(10)", "varchar", TypeClass::Text),
        ]);
        let f = |column: &str, op, value: &str| Condition::Filters(vec![Filter { column: column.into(), op, value: value.into() }]);
        assert_eq!(where_sql(pg, &f("note", Op::ILike, "%a'b%"), &t).unwrap().unwrap(), "CAST(\"note\" AS text) ILIKE '%a''b%'");
        assert_eq!(where_sql(pg, &f("note", Op::Eq, "x"), &t).unwrap().unwrap(), "\"note\" = CAST('x' AS text)");
        assert_eq!(where_sql(pg, &f("id", Op::In, "1, 2"), &t).unwrap().unwrap(), "\"id\" IN (CAST('1' AS bigint), CAST('2' AS bigint))");
        assert_eq!(
            page_sql(pg, &t, &Condition::None, None, 2).unwrap(),
            "SELECT \"id\", \"total\", \"note\" FROM \"shop\".\"orders\" ORDER BY \"id\" LIMIT 101 OFFSET 200"
        );
        assert_eq!(count_sql(pg, &t, &f("id", Op::Gt, "5")).unwrap(), "SELECT count(*) FROM \"shop\".\"orders\" WHERE \"id\" > CAST('5' AS bigint)");
        let id = t.column("id").unwrap();
        let note = t.column("note").unwrap();
        assert_eq!(
            update_sql(pg, &t, &[(id, "7")], &[(note, &NewValue::Text("Ann".into())), (&total, &NewValue::Null)]).unwrap(),
            "UPDATE \"shop\".\"orders\" SET \"note\" = CAST('Ann' AS text), \"total\" = NULL WHERE \"id\" = CAST('7' AS bigint)"
        );
        assert_eq!(insert_sql(pg, &t, &[]).unwrap(), "INSERT INTO \"shop\".\"orders\" DEFAULT VALUES");
        assert_eq!(delete_sql(pg, &t, &[(id, "7")]).unwrap(), "DELETE FROM \"shop\".\"orders\" WHERE \"id\" = CAST('7' AS bigint)");
    }

    #[test]
    fn mysql_quotes_with_backticks() {
        assert_eq!(Dialect::MySql.ident("a`b").unwrap(), "`a``b`");
        assert_eq!(Dialect::MySql.table(&tr("shop", "orders")).unwrap(), "`shop`.`orders`");
        assert_eq!(Dialect::MySql.ident("a\"b").unwrap(), "`a\"b`");
        assert_eq!(Dialect::Sqlite.ident("a\"b").unwrap(), "\"a\"\"b\"");
        assert_eq!(Dialect::Sqlite.table(&tr("main", "t")).unwrap(), "\"main\".\"t\"");
    }

    #[test]
    fn nul_is_refused_everywhere() {
        for d in ALL {
            assert_eq!(d.ident("a\0"), Err(SqlError::Nul));
            assert_eq!(d.literal("a\0"), Err(SqlError::Nul));
            assert_eq!(d.value(&txt("n"), "a\0"), Err(SqlError::Nul));
            assert_eq!(d.use_database("a\0"), if d == Dialect::MySql { Err(SqlError::Nul) } else { Ok(None) });
        }
    }

    #[test]
    fn literals_double_quotes_and_keep_backslashes() {
        for d in ALL {
            assert_eq!(d.literal("it's").unwrap(), "'it''s'");
            if d != Dialect::MySql {
                assert_eq!(d.literal(r"a\b").unwrap(), r"'a\b'");
            }
        }
    }

    #[test]
    fn mysql_binary_values() {
        assert_eq!(Dialect::MySql.value(&bin("k"), "0x00FF").unwrap(), "0x00FF");
        assert_eq!(Dialect::MySql.value(&bin("k"), "0xZZ"), Err(SqlError::BadBinary));
        assert_eq!(Dialect::MySql.value(&bin("k"), "0x0"), Err(SqlError::BadBinary));
        assert_eq!(Dialect::MySql.value(&bin("k"), "00FF"), Err(SqlError::BadBinary));
        assert_eq!(Dialect::MySql.value(&bin("k"), "0x").unwrap(), "X''");
        assert_eq!(Dialect::MySql.value(&txt("k"), "0xZZ").unwrap(), "'0xZZ'");
        assert_eq!(Dialect::MySql.key_value(&bin("k"), "0x0A").unwrap(), "0x0A");
        assert_eq!(Dialect::MySql.filter_value(&bin("k"), "0x0A").unwrap(), "0x0A");
    }

    #[test]
    fn sqlite_numbers_go_in_bare() {
        assert_eq!(Dialect::Sqlite.value(&num("id"), "42").unwrap(), "42");
        assert_eq!(Dialect::Sqlite.value(&num("id"), "-1.5").unwrap(), "-1.5");
        assert_eq!(Dialect::Sqlite.value(&num("id"), "4 2").unwrap(), "'4 2'");
        for odd in ["1e5", "", "-", ".5", "5.", "0x10", "1; DROP", "NaN"] {
            assert_eq!(Dialect::Sqlite.value(&num("id"), odd).unwrap(), format!("'{odd}'"), "{odd}");
        }
        assert_eq!(Dialect::Sqlite.value(&txt("n"), "42").unwrap(), "'42'");
        assert_eq!(Dialect::Sqlite.value(&bin("b"), "X'0A'").unwrap(), "X'0A'");
        assert_eq!(Dialect::Sqlite.value(&bin("b"), "x'0a'").unwrap(), "X'0a'");
        assert_eq!(Dialect::Sqlite.value(&bin("b"), "X''").unwrap(), "X''");
        for bad in ["0x0A", "X'0'", "X'ZZ'", "X'0A", "0A"] {
            assert_eq!(Dialect::Sqlite.value(&bin("b"), bad), Err(SqlError::BadBinary), "{bad}");
        }
    }

    #[test]
    fn postgres_values_and_keys() {
        let v = of_class("v", "character varying(10)", "varchar", TypeClass::Text);
        assert_eq!(Dialect::Postgres.value(&v, "a").unwrap(), "CAST('a' AS text)");
        assert_eq!(Dialect::Postgres.key_value(&v, "a").unwrap(), "CAST('a' AS character varying(10))");
        let c = of_class("c", "character(2)", "bpchar", TypeClass::Text);
        assert_eq!(Dialect::Postgres.filter_value(&c, "a").unwrap(), "CAST('a' AS bpchar)");
        assert_eq!(Dialect::Postgres.filter_value(&v, "a").unwrap(), "CAST('a' AS text)");
    }

    #[test]
    fn write_type_drops_the_lengths_an_explicit_cast_cuts_to() {
        for (base, type_sql, write) in [
            ("varchar", "character varying(10)", "text"),
            ("varchar", "character varying", "text"),
            ("bpchar", "character(2)", "text"),
            ("bit", "bit(2)", "bit varying"),
            ("varbit", "bit varying(2)", "bit varying"),
            ("_varchar", "character varying(3)[]", "text[]"),
            ("_bpchar", "character(2)[]", "text[]"),
            ("_bit", "bit(2)[]", "bit varying[]"),
            ("_varbit", "bit varying(2)[]", "bit varying[]"),
            // A domain over varchar(5): the base type decides, the assignment to the column checks the domain.
            ("varchar", "scratch.d5", "text"),
            // Everything else is written as its own type.
            ("text", "text", "text"),
            ("int4", "integer", "integer"),
            ("numeric", "numeric(12,2)", "numeric(12,2)"),
            ("timestamp", "timestamp(3) without time zone", "timestamp(3) without time zone"),
            ("char", "\"char\"", "\"char\""),
            ("_text", "text[]", "text[]"),
            ("order_status", "shop.order_status", "shop.order_status"),
        ] {
            let c = of_class("x", type_sql, base, TypeClass::Other);
            assert_eq!(pg_write_type(&c), write, "{base} {type_sql}");
        }
    }

    #[test]
    fn ilike_is_lower_like_off_postgres() {
        assert_eq!(Dialect::MySql.like(Op::ILike, "`n`", "'%a%'"), "LOWER(CAST(`n` AS CHAR)) LIKE LOWER('%a%')");
        assert_eq!(Dialect::MySql.like(Op::NotILike, "`n`", "'%a%'"), "LOWER(CAST(`n` AS CHAR)) NOT LIKE LOWER('%a%')");
        assert_eq!(Dialect::Sqlite.like(Op::ILike, "\"n\"", "'%a%'"), "LOWER(CAST(\"n\" AS TEXT)) LIKE LOWER('%a%')");
        assert_eq!(Dialect::Sqlite.like(Op::Like, "\"n\"", "'a'"), "CAST(\"n\" AS TEXT) LIKE 'a'");
        assert_eq!(Dialect::MySql.like(Op::NotLike, "`n`", "'a'"), "CAST(`n` AS CHAR) NOT LIKE 'a'");
        assert_eq!(Dialect::Postgres.like(Op::ILike, "\"n\"", "'a'"), "CAST(\"n\" AS text) ILIKE 'a'");
        assert_eq!(Dialect::Postgres.like(Op::NotILike, "\"n\"", "'a'"), "CAST(\"n\" AS text) NOT ILIKE 'a'");
        assert_eq!(
            [Dialect::Postgres, Dialect::MySql, Dialect::Sqlite].map(|d| d.as_text("x")),
            ["CAST(x AS text)", "CAST(x AS CHAR)", "CAST(x AS TEXT)"]
        );
    }

    #[test]
    fn insert_of_all_defaults() {
        assert_eq!(Dialect::MySql.insert_defaults("`s`.`t`"), "INSERT INTO `s`.`t` () VALUES ()");
        assert_eq!(Dialect::Postgres.insert_defaults("\"s\".\"t\""), "INSERT INTO \"s\".\"t\" DEFAULT VALUES");
        assert_eq!(Dialect::Sqlite.insert_defaults("\"main\".\"t\""), "INSERT INTO \"main\".\"t\" DEFAULT VALUES");
    }

    #[test]
    fn use_database_only_on_mysql() {
        assert_eq!(Dialect::MySql.use_database("shop").unwrap(), Some("USE `shop`".to_string()));
        assert_eq!(Dialect::MySql.use_database("a`b").unwrap(), Some("USE `a``b`".to_string()));
        assert_eq!(Dialect::Postgres.use_database("shop").unwrap(), None);
        assert_eq!(Dialect::Sqlite.use_database("shop").unwrap(), None);
    }

    #[test]
    fn bool_literals_and_op_notes() {
        assert_eq!(Dialect::Postgres.bool_literals(), ("t", "f"));
        assert_eq!(Dialect::MySql.bool_literals(), ("1", "0"));
        assert_eq!(Dialect::Sqlite.bool_literals(), ("1", "0"));
        let collation = "Compares by the column's collation; most ignore upper and lower case.";
        for op in Op::ALL {
            let mysql = matches!(op, Op::Eq | Op::Ne | Op::Like | Op::NotLike).then_some(collation);
            let sqlite = matches!(op, Op::Like | Op::NotLike).then_some("Ignores upper and lower case for A–Z only.");
            assert_eq!(Dialect::MySql.op_note(op), mysql, "{op:?}");
            assert_eq!(Dialect::Sqlite.op_note(op), sqlite, "{op:?}");
            assert_eq!(Dialect::Postgres.op_note(op), None, "{op:?}");
        }
    }

    #[test]
    fn serde_names() {
        assert_eq!(serde_json::to_value(Dialect::MySql).unwrap(), "MySql");
        assert_eq!(serde_json::from_value::<Dialect>(serde_json::json!("Sqlite")).unwrap(), Dialect::Sqlite);
    }

    /// `s` is one literal: it opens and closes with `'` and every inner `'` is doubled.
    fn is_one_literal(s: &str) -> bool {
        if let Some(hex) = s.strip_prefix("_utf8mb4 X'").and_then(|s| s.strip_suffix('\'')) {
            return hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
        }
        let Some(inner) = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) else { return false };
        inner.replace("''", "").find('\'').is_none()
    }

    #[test]
    fn mysql_literals_with_a_backslash_are_hex() {
        let col = txt("n");
        for (text, hex) in [
            (r"a\b", "615C62"),
            (r"\'; DROP TABLE t; -- ", "5C273B2044524F50205441424C4520743B202D2D20"),
            (r"it's a\b", "6974277320615C62"),
            ("é\\", "C3A95C"),
        ] {
            let lit = format!("_utf8mb4 X'{hex}'");
            assert_eq!(Dialect::MySql.literal(text).unwrap(), lit, "{text}");
            assert_eq!(Dialect::MySql.value(&col, text).unwrap(), lit);
            assert_eq!(Dialect::MySql.key_value(&col, text).unwrap(), lit);
            assert_eq!(Dialect::MySql.filter_value(&col, text).unwrap(), lit);
            assert_eq!(Dialect::Postgres.literal(text).unwrap().replace("''", "'"), format!("'{text}'"));
        }
        assert_eq!(Dialect::MySql.literal("it's").unwrap(), "'it''s'");
        assert_eq!(Dialect::MySql.literal("").unwrap(), "''");
        assert_eq!(Dialect::MySql.literal("a\\\0"), Err(SqlError::Nul));
        assert_eq!(Dialect::Sqlite.literal(r"a\b").unwrap(), r"'a\b'");
    }

    proptest! {
        #[test]
        fn hostile_values_stay_literals(s in ".*") {
            for d in ALL {
                match d.literal(&s) {
                    Err(e) => prop_assert!(s.contains('\0') && e == SqlError::Nul),
                    Ok(lit) => {
                        prop_assert!(!s.contains('\0') && is_one_literal(&lit), "{lit}");
                        prop_assert!(!lit.contains('\0'));
                        if d == Dialect::MySql {
                            prop_assert!(!lit.contains('\\'), "{lit}");
                            if let Some(hex) = lit.strip_prefix("_utf8mb4 X'").and_then(|l| l.strip_suffix('\'')) {
                                let bytes: Vec<u8> = (0..hex.len()).step_by(2).filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
                                prop_assert_eq!(bytes, s.as_bytes());
                            } else {
                                prop_assert!(!s.contains('\\'));
                            }
                        }
                    }
                }
                let column = txt("n");
                for out in [d.value(&column, &s), d.key_value(&column, &s), d.filter_value(&column, &s)] {
                    match out {
                        Err(e) => prop_assert!(s.contains('\0') && e == SqlError::Nul),
                        Ok(v) => {
                            prop_assert!(!s.contains('\0') && !v.contains('\0'));
                            prop_assert!(d != Dialect::MySql || !v.contains('\\'), "{v}");
                            let literal = v.strip_prefix("CAST(").and_then(|v| v.strip_suffix(" AS text)")).unwrap_or(&v);
                            prop_assert!(is_one_literal(literal), "{v}");
                        }
                    }
                }
                match d.ident(&s) {
                    Err(e) => prop_assert!(s.contains('\0') && e == SqlError::Nul),
                    Ok(id) => {
                        let q = if d == Dialect::MySql { '`' } else { '"' };
                        let inner = &id[1..id.len() - 1];
                        prop_assert!(id.starts_with(q) && id.ends_with(q) && !inner.replace(&format!("{q}{q}"), "").contains(q), "{id}");
                    }
                }
            }
        }
    }
}

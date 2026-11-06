//! The SQL text differences between engines. Each engine's file holds its [`Rules`], and
//! [`Dialect::rules`] is the one place a dialect is matched on.

use crate::SqlError;
use crate::filter::Op;
use crate::lexer::{Token, TokenKind, tokenize};
use dbee_core::{ColumnInfo, TableRef, TypeClass};
use serde::{Deserialize, Serialize};

mod mysql;
mod postgres;
mod sqlite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    Postgres,
    MySql,
    Sqlite,
}

impl Dialect {
    pub const ALL: [Dialect; 3] = [Dialect::Postgres, Dialect::MySql, Dialect::Sqlite];

    /// The engine's rules: the one `match` on a dialect.
    pub(crate) fn rules(self) -> &'static Rules {
        match self {
            Dialect::Postgres => &postgres::RULES,
            Dialect::MySql => &mysql::RULES,
            Dialect::Sqlite => &sqlite::RULES,
        }
    }
}

/// How one engine writes and reads SQL. Each engine's `RULES` names every field.
pub(crate) struct Rules {
    /// The quote around an identifier; one inside it is doubled.
    pub ident_quote: char,
    /// The type [`Dialect::as_text`] casts to.
    pub text_type: &'static str,
    pub ilike: ILike,
    /// What follows a `LIKE` pattern so that `\` escapes `%`, `_` and itself, as it does by
    /// default on Postgres: an `ESCAPE` clause, or nothing.
    pub like_escape: &'static str,
    /// How an insert gives every column its default.
    pub insert_defaults: InsertDefaults,
    /// The statement that makes a database current, `{}` standing for its quoted name.
    pub use_database: Option<&'static str>,
    /// Statement text may hold a NUL character inside a string literal; outside one no engine
    /// takes it ([`Dialect::misplaced_nul`]). The literals dbee writes still never carry one as a
    /// raw byte: [`StringLiteral::HexForBackslash`] writes such a value as hex.
    pub text_holds_nul: bool,
    /// The texts a Bool column writes for true and false.
    pub bool_literals: (&'static str, &'static str),
    /// What filter operators do on the engine that their labels do not say.
    pub op_notes: &'static [(&'static [Op], &'static str)],
    /// What the engine calls a schema: one, in a sentence, and many, as a heading.
    pub schema_word: (&'static str, &'static str),
    pub string_literal: StringLiteral,
    /// How a binary value is written where values are literals; `None` writes it as text.
    pub binary_literal: Option<BinaryLiteral>,
    /// How a value for a column is written.
    pub values: Values,
    /// How a page of rows is asked for.
    pub paging: Paging,
    pub lex: LexRules,
    /// The keywords for highlighting: lowercase, sorted, unique.
    pub keywords: &'static [&'static str],
}

/// What the lexer reads: features, never the engine's name.
pub(crate) struct LexRules {
    /// What `"…"` quotes.
    pub double_quote: DoubleQuote,
    /// `` `…` `` quotes an identifier.
    pub backtick_idents: bool,
    /// `[…]` quotes an identifier, up to the first `]`.
    pub bracket_idents: bool,
    /// A backslash in a string escapes the character after it.
    pub backslash_escapes: bool,
    /// `E'…'` strings, whose backslashes escape.
    pub escape_strings: bool,
    /// `U&'…'` strings and `U&"…"` identifiers.
    pub unicode_strings: bool,
    /// `X'…'` hex strings.
    pub hex_strings: bool,
    /// `B'…'` bit strings.
    pub bit_strings: bool,
    /// `0x…` hex numbers.
    pub hex_numbers: bool,
    /// `$tag$…$tag$` strings.
    pub dollar_quotes: bool,
    /// `$1` parameters.
    pub numbered_params: bool,
    /// `$` may start an identifier.
    pub dollar_idents: bool,
    /// `#` starts a line comment, so it is no operator.
    pub hash_comment: bool,
    /// `--` starts a comment only before a space, a control character or the end of the text.
    pub dash_comment_needs_space: bool,
    /// `/* … */` comments nest.
    pub nested_block_comments: bool,
    pub separator: Separator,
    pub bodies: Bodies,
    /// How far past a token's end the lexer may read to decide it, with one byte to spare: a
    /// number reads three bytes (`1e+` and no digit), an operator two (three when `--` needs a
    /// space), a `U` or a `0` two (`U&'`, `0x` and a hex digit), all else one. A lone `$` reads
    /// further where dollar quotes exist ([`crate::lexer::reach`]).
    pub lookahead: usize,
}

/// How `ilike` and `not ilike` are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ILike {
    /// `ILIKE`, `NOT ILIKE`.
    Native,
    /// `LOWER(…) LIKE LOWER(…)`.
    LowerLike,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertDefaults {
    /// `INSERT INTO t DEFAULT VALUES`.
    DefaultValues,
    /// `INSERT INTO t () VALUES ()`.
    EmptyLists,
}

/// How a string literal is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StringLiteral {
    /// `'…'` with `'` doubled; a backslash is a plain character.
    Quoted,
    /// As `Quoted`, but hex when the value holds a backslash, which may or may not escape, or a NUL.
    HexForBackslash,
}

/// The hex form of a binary literal; a binary value to write comes as text in the same form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryLiteral {
    /// `0x…`, and `X''` for no bytes.
    ZeroX,
    /// `X'…'`.
    XQuoted,
}

/// How a value for a column is written; the logic for each is in its engine's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Values {
    /// `CAST('…' AS type)`, the type picked from the column (Postgres).
    Cast,
    /// A literal.
    Literal,
    /// A literal, but a plain decimal for a number column bare (SQLite).
    BareNumbers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Paging {
    /// `LIMIT n OFFSET m`.
    LimitOffset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoubleQuote {
    Ident,
    String,
}

/// What ends a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Separator {
    /// `;`.
    Semicolon,
    /// `;`, or what a `DELIMITER` line sets.
    DelimiterCommand,
}

/// Which statements have a body whose `BEGIN … END` keeps its `;`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bodies {
    None,
    /// Procedures, functions, triggers and events, their bodies compound statements (MySQL).
    Compound,
    /// Triggers, their bodies plain statements (SQLite).
    TriggerOnly,
}

/// The body of the statement being split, counting its blocks.
pub(crate) enum Body {
    /// No body, or one whose `;`s end the statement.
    Plain,
    Compound(mysql::Compound),
    Trigger(sqlite::Trigger),
}

impl Bodies {
    /// The body of the statement whose first token is `toks[k]`.
    pub(crate) fn body(self, sql: &str, toks: &[Token], k: usize) -> Body {
        match self {
            Bodies::Compound if mysql::routine_header(sql, toks, k) => Body::Compound(mysql::Compound::default()),
            Bodies::TriggerOnly if sqlite::trigger_header(sql, toks, k) => Body::Trigger(sqlite::Trigger::default()),
            Bodies::None | Bodies::Compound | Bodies::TriggerOnly => Body::Plain,
        }
    }
}

impl Body {
    /// The blocks open: a `;` ends the statement only when there are none.
    pub(crate) fn depth(&self) -> usize {
        match self {
            Body::Plain => 0,
            Body::Compound(c) => c.depth,
            Body::Trigger(t) => t.depth,
        }
    }

    /// Reads `toks[k]`, the statement's next significant token.
    pub(crate) fn step(&mut self, sql: &str, toks: &[Token], k: usize) {
        match self {
            Body::Plain => {}
            Body::Compound(c) => c.step(sql, toks, k),
            Body::Trigger(t) => t.step(sql, toks, k),
        }
    }
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

impl BinaryLiteral {
    /// The literal of `text`, a value written in this form with an even number of hex digits.
    fn of(self, text: &str) -> Result<String, SqlError> {
        let digits = match self {
            BinaryLiteral::ZeroX => text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")),
            BinaryLiteral::XQuoted => text.strip_prefix(['X', 'x']).and_then(|t| t.strip_prefix('\'')).and_then(|t| t.strip_suffix('\'')),
        };
        let digits = digits.filter(|d| d.len() % 2 == 0 && d.bytes().all(|b| b.is_ascii_hexdigit())).ok_or(SqlError::BadBinary)?;
        Ok(match self {
            BinaryLiteral::ZeroX if digits.is_empty() => "X''".to_string(),
            BinaryLiteral::ZeroX => format!("0x{digits}"),
            BinaryLiteral::XQuoted => format!("X'{digits}'"),
        })
    }
}

impl Dialect {
    /// The engine's keywords for highlighting: lowercase, sorted, unique.
    pub fn keywords(self) -> &'static [&'static str] {
        self.rules().keywords
    }

    /// True when `word` is one of the engine's keywords, in any case.
    pub fn is_keyword(self, word: &str) -> bool {
        self.rules().keywords.binary_search_by(|k| k.bytes().cmp(word.bytes().map(|b| b.to_ascii_lowercase()))).is_ok()
    }

    /// `"name"` or `` `name` ``, the engine's quote doubled inside it. NUL is an error.
    pub fn ident(self, name: &str) -> Result<String, SqlError> {
        quoted(name, self.rules().ident_quote)
    }

    /// `schema.name`, both quoted (SQLite's schema is `main`).
    pub fn table(self, t: &TableRef) -> Result<String, SqlError> {
        Ok(format!("{}.{}", self.ident(&t.schema)?, self.ident(&t.name)?))
    }

    /// `'value'` with `'` doubled; backslashes stay literal, which Postgres gets from
    /// `standard_conforming_strings`. MySQL may or may not escape them, so a value with one is
    /// hex, as is one with a NUL. NUL is an error where the statement cannot hold one.
    pub fn literal(self, value: &str) -> Result<String, SqlError> {
        match self.rules().string_literal {
            StringLiteral::Quoted => quoted(value, '\''),
            StringLiteral::HexForBackslash => mysql::literal(value),
        }
    }

    /// A value to write into the column.
    pub fn value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        let rules = self.rules();
        match (rules.values, rules.binary_literal) {
            (Values::Cast, _) => postgres::value(column, text),
            (_, Some(form)) if column.class == TypeClass::Binary => form.of(text),
            (Values::BareNumbers, _) if sqlite::is_bare_number(column, text) => Ok(text.to_string()),
            (Values::Literal | Values::BareNumbers, _) => self.literal(text),
        }
    }

    /// A value of the row's key, which the server sent.
    pub fn key_value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        match self.rules().values {
            Values::Cast => postgres::key_value(column, text),
            Values::Literal | Values::BareNumbers => self.value(column, text),
        }
    }

    /// A value a filter compares the column with.
    pub fn filter_value(self, column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
        match self.rules().values {
            Values::Cast => postgres::filter_value(column, text),
            Values::Literal | Values::BareNumbers => self.value(column, text),
        }
    }

    /// `expr` as text.
    pub fn as_text(self, expr: &str) -> String {
        format!("CAST({expr} AS {})", self.rules().text_type)
    }

    /// A pattern filter on the column's text: `like`, `not like`, `ilike` or `not ilike`. In the
    /// pattern `\` escapes `%`, `_` and `\` on every engine.
    pub fn like(self, op: Op, column_sql: &str, pattern_sql: &str) -> String {
        let text = self.as_text(column_sql);
        let escape = self.rules().like_escape;
        match (self.rules().ilike, op) {
            (ILike::LowerLike, Op::ILike | Op::NotILike) => {
                let not = if op == Op::NotILike { "NOT " } else { "" };
                format!("LOWER({text}) {not}LIKE LOWER({pattern_sql}){escape}")
            }
            (ILike::Native | ILike::LowerLike, _) => format!("{text} {} {pattern_sql}{escape}", op.symbol()),
        }
    }

    /// An insert that gives every column its default.
    pub fn insert_defaults(self, table_sql: &str) -> String {
        match self.rules().insert_defaults {
            InsertDefaults::DefaultValues => format!("INSERT INTO {table_sql} DEFAULT VALUES"),
            InsertDefaults::EmptyLists => format!("INSERT INTO {table_sql} () VALUES ()"),
        }
    }

    /// The statement that makes `name` the current database, on engines that have one.
    pub fn use_database(self, name: &str) -> Result<Option<String>, SqlError> {
        self.rules().use_database.map(|template| Ok(template.replacen("{}", &self.ident(name)?, 1))).transpose()
    }

    /// The engine has a statement that makes a database current ([`Dialect::use_database`]).
    pub fn has_use_database(self) -> bool {
        self.rules().use_database.is_some()
    }

    /// Statement text may hold a NUL character inside a string literal ([`Rules::text_holds_nul`]),
    /// so a value with one can be written.
    pub fn statement_may_hold_nul(self) -> bool {
        self.rules().text_holds_nul
    }

    /// The byte offset of the first NUL in `sql` the engine cannot take, which must not be sent:
    /// any NUL where statements cannot hold one, else one outside a string literal.
    pub fn misplaced_nul(self, sql: &str) -> Option<usize> {
        let first = sql.find('\0')?;
        if !self.rules().text_holds_nul {
            return Some(first);
        }
        let tokens = tokenize(self, sql);
        let in_literal = |at: usize| {
            let k = tokens.partition_point(|t| t.range.end <= at);
            tokens.get(k).is_some_and(|t| t.kind == TokenKind::String && t.range.contains(&at))
        };
        sql.match_indices('\0').map(|(at, _)| at).find(|&at| !in_literal(at))
    }

    /// The texts a Bool column writes for true and false.
    pub fn bool_literals(self) -> (&'static str, &'static str) {
        self.rules().bool_literals
    }

    /// What a filter operator does on this engine that the label does not say.
    pub fn op_note(self, op: Op) -> Option<&'static str> {
        self.rules().op_notes.iter().find(|(ops, _)| ops.contains(&op)).map(|&(_, note)| note)
    }

    /// What the engine calls a schema: one, in a sentence (`schema`), and many, as a heading
    /// (`Schemas`). MySQL's are its databases.
    pub fn schema_word(self) -> (&'static str, &'static str) {
        self.rules().schema_word
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{NewValue, delete_sql, insert_sql, update_sql};
    use crate::filter::{Condition, Filter, where_sql};
    use crate::select::{count_sql, page_sql};
    use dbee_core::{RelKind, TableInfo};
    use proptest::prelude::*;

    #[test]
    fn all_holds_every_dialect_once() {
        // Each dialect and the next, by an exhaustive match: a new variant joins the chain here.
        let after = |d: Dialect| match d {
            Dialect::Postgres => Some(Dialect::MySql),
            Dialect::MySql => Some(Dialect::Sqlite),
            Dialect::Sqlite => None,
        };
        let every: Vec<Dialect> = std::iter::successors(Some(Dialect::Postgres), |d| after(*d)).collect();
        assert_eq!(Dialect::ALL.to_vec(), every);
    }

    #[test]
    fn rules_are_consts_per_engine() {
        for d in Dialect::ALL {
            let list = d.rules().keywords;
            assert_eq!(d.keywords(), list, "{d:?}");
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{d:?} must be sorted and unique");
            assert!(list.iter().all(|k| k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')), "{d:?}");
            for k in ["select", "from", "where", "join", "insert", "update", "delete", "limit"] {
                assert!(list.binary_search(&k).is_ok(), "{d:?} {k}");
            }
        }
    }

    #[test]
    fn is_keyword_ignores_case_without_a_cap() {
        let long = "x".repeat(10_000);
        for d in Dialect::ALL {
            assert!(d.is_keyword("SELECT") && d.is_keyword("select") && d.is_keyword("SeLeCt"), "{d:?}");
            assert_eq!(d.is_keyword("master_ssl_verify_server_cert"), d == Dialect::MySql, "{d:?}");
            assert_eq!(d.is_keyword("MASTER_SSL_VERIFY_SERVER_CERT"), d == Dialect::MySql, "{d:?}");
            assert!(!d.is_keyword(&long) && !d.is_keyword(&format!("select{long}")), "{d:?}");
            assert!(!d.is_keyword("") && !d.is_keyword("sélect") && !d.is_keyword("selec"), "{d:?}");
        }
    }

    #[test]
    fn the_longest_keyword_of_each_engine_is_found_and_longer_words_are_not() {
        for d in Dialect::ALL {
            let longest = d.keywords().iter().copied().max_by_key(|k| k.len()).unwrap_or_default();
            assert!(d.is_keyword(longest) && d.is_keyword(&longest.to_ascii_uppercase()), "{d:?} {longest}");
            assert!(!d.is_keyword(&format!("{longest}x")), "{d:?}");
        }
        assert!(Dialect::MySql.is_keyword("master_ssl_verify_server_cert"));
        assert!(!Dialect::Postgres.is_keyword("master_ssl_verify_server_cert"));
        assert!(!Dialect::Sqlite.is_keyword("master_ssl_verify_server_cert"));
        assert!(Dialect::MySql.is_keyword("accessible") && !Dialect::Postgres.is_keyword("accessible"));
    }

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
    fn a_nul_outside_a_string_literal_is_misplaced_everywhere() {
        let refused_everywhere = [
            ("select 1\0 + 1", 8),
            ("select 1 -- c\0", 13),
            ("select 1 # c\0\n", 12),
            ("/* \0 */ select 1", 3),
            ("select `a\0`", 9),
            ("delete from t \0   ", 14),
        ];
        for d in Dialect::ALL {
            for (sql, at) in refused_everywhere {
                assert_eq!(d.misplaced_nul(sql), Some(at), "{d:?}: {sql:?}");
            }
            assert_eq!(d.misplaced_nul("select 'no nul'"), None, "{d:?}");
            // Inside a string literal only where statements may hold one.
            let in_literal = if d.statement_may_hold_nul() { None } else { Some(9) };
            assert_eq!(d.misplaced_nul("select 'a\0b'"), in_literal, "{d:?}");
            // The first misplaced one, after a literal that holds one.
            let after = d.misplaced_nul("select 'a\0', 1\0");
            assert_eq!(after, if d.statement_may_hold_nul() { Some(14) } else { Some(9) }, "{d:?}");
        }
        assert_eq!(Dialect::MySql.misplaced_nul("select \"a\0b\""), None, "a double-quoted string is a literal on MySQL");
    }

    #[test]
    fn nul_follows_each_dialects_rule() {
        assert_eq!(Dialect::ALL.map(Dialect::statement_may_hold_nul), [false, true, false]);
        for d in Dialect::ALL {
            // No engine takes NUL in a name.
            assert_eq!(d.ident("a\0"), Err(SqlError::Nul));
            assert_eq!(d.use_database("a\0"), if d.has_use_database() { Err(SqlError::Nul) } else { Ok(None) });
            if d.statement_may_hold_nul() {
                // Never as a raw byte in the statement: hex, as for a backslash.
                for out in [d.literal("a\0b"), d.value(&txt("n"), "a\0b"), d.key_value(&txt("n"), "a\0b"), d.filter_value(&txt("n"), "a\0b")] {
                    assert_eq!(out.as_deref(), Ok("_utf8mb4 X'610062'"), "{d:?}");
                }
            } else {
                assert_eq!(d.literal("a\0"), Err(SqlError::Nul));
                assert_eq!(d.value(&txt("n"), "a\0"), Err(SqlError::Nul));
            }
        }
    }

    #[test]
    fn literals_double_quotes_and_keep_backslashes() {
        for d in Dialect::ALL {
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
    fn ilike_is_lower_like_off_postgres() {
        let my = " ESCAPE _utf8mb4 X'5C'";
        assert_eq!(Dialect::MySql.like(Op::ILike, "`n`", "'%a%'"), format!("LOWER(CAST(`n` AS CHAR)) LIKE LOWER('%a%'){my}"));
        assert_eq!(Dialect::MySql.like(Op::NotILike, "`n`", "'%a%'"), format!("LOWER(CAST(`n` AS CHAR)) NOT LIKE LOWER('%a%'){my}"));
        assert_eq!(Dialect::Sqlite.like(Op::ILike, "\"n\"", "'%a%'"), "LOWER(CAST(\"n\" AS TEXT)) LIKE LOWER('%a%') ESCAPE '\\'");
        assert_eq!(Dialect::Sqlite.like(Op::Like, "\"n\"", "'a'"), "CAST(\"n\" AS TEXT) LIKE 'a' ESCAPE '\\'");
        assert_eq!(Dialect::MySql.like(Op::NotLike, "`n`", "'a'"), format!("CAST(`n` AS CHAR) NOT LIKE 'a'{my}"));
        assert_eq!(Dialect::Postgres.like(Op::ILike, "\"n\"", "'a'"), "CAST(\"n\" AS text) ILIKE 'a'");
        assert_eq!(Dialect::Postgres.like(Op::NotILike, "\"n\"", "'a'"), "CAST(\"n\" AS text) NOT ILIKE 'a'");
        assert_eq!(Dialect::ALL.map(|d| d.as_text("x")), ["CAST(x AS text)", "CAST(x AS CHAR)", "CAST(x AS TEXT)"]);
    }

    #[test]
    fn insert_of_all_defaults() {
        assert_eq!(Dialect::MySql.insert_defaults("`s`.`t`"), "INSERT INTO `s`.`t` () VALUES ()");
        assert_eq!(Dialect::Postgres.insert_defaults("\"s\".\"t\""), "INSERT INTO \"s\".\"t\" DEFAULT VALUES");
        assert_eq!(Dialect::Sqlite.insert_defaults("\"main\".\"t\""), "INSERT INTO \"main\".\"t\" DEFAULT VALUES");
    }

    #[test]
    fn use_database_only_on_mysql() {
        assert_eq!(Dialect::ALL.map(Dialect::has_use_database), [false, true, false]);
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
            let sqlite = matches!(op, Op::Like | Op::NotLike | Op::ILike | Op::NotILike).then_some("Ignores upper and lower case for A–Z only.");
            assert_eq!(Dialect::MySql.op_note(op), mysql, "{op:?}");
            assert_eq!(Dialect::Sqlite.op_note(op), sqlite, "{op:?}");
            assert_eq!(Dialect::Postgres.op_note(op), None, "{op:?}");
        }
    }

    #[test]
    fn each_engine_names_its_schemas() {
        assert_eq!(Dialect::Postgres.schema_word(), ("schema", "Schemas"));
        assert_eq!(Dialect::MySql.schema_word(), ("database", "Databases"));
        assert_eq!(Dialect::Sqlite.schema_word(), ("schema", "Schemas"));
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
        assert_eq!(Dialect::MySql.literal("a\\\0").unwrap(), "_utf8mb4 X'615C00'");
        assert_eq!(Dialect::MySql.literal("\0").unwrap(), "_utf8mb4 X'00'");
        assert_eq!(Dialect::Sqlite.literal(r"a\b").unwrap(), r"'a\b'");
    }

    proptest! {
        #[test]
        fn hostile_values_stay_literals(s in ".*") {
            for d in Dialect::ALL {
                match d.literal(&s) {
                    Err(e) => prop_assert!(s.contains('\0') && !d.statement_may_hold_nul() && e == SqlError::Nul),
                    Ok(lit) => {
                        prop_assert!((d.statement_may_hold_nul() || !s.contains('\0')) && is_one_literal(&lit), "{lit}");
                        prop_assert!(!lit.contains('\0'));
                        if d == Dialect::MySql {
                            prop_assert!(!lit.contains('\\'), "{lit}");
                            if let Some(hex) = lit.strip_prefix("_utf8mb4 X'").and_then(|l| l.strip_suffix('\'')) {
                                let bytes: Vec<u8> = (0..hex.len()).step_by(2).filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
                                prop_assert_eq!(bytes, s.as_bytes());
                            } else {
                                prop_assert!(!s.contains('\\') && !s.contains('\0'));
                            }
                        }
                    }
                }
                let column = txt("n");
                for out in [d.value(&column, &s), d.key_value(&column, &s), d.filter_value(&column, &s)] {
                    match out {
                        Err(e) => prop_assert!(s.contains('\0') && !d.statement_may_hold_nul() && e == SqlError::Nul),
                        Ok(v) => {
                            prop_assert!((d.statement_may_hold_nul() || !s.contains('\0')) && !v.contains('\0'));
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

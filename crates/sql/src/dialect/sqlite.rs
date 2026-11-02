//! SQLite.

use super::{BinaryLiteral, Bodies, DoubleQuote, ILike, InsertDefaults, LexRules, Paging, Rules, Separator, StringLiteral, Values};
use crate::filter::Op;
use crate::lexer::{Token, is_word, significant, word};
use dbee_core::{ColumnInfo, TypeClass};

pub(crate) const RULES: Rules = Rules {
    ident_quote: '"',
    text_type: "TEXT",
    ilike: ILike::LowerLike,
    insert_defaults: InsertDefaults::DefaultValues,
    use_database: None,
    // Its prepare stops at a NUL, so text after one would be cut off unseen.
    text_holds_nul: false,
    bool_literals: ("1", "0"),
    op_notes: &[(&[Op::Like, Op::NotLike], "Ignores upper and lower case for A–Z only.")],
    string_literal: StringLiteral::Quoted,
    binary_literal: Some(BinaryLiteral::XQuoted),
    values: Values::BareNumbers,
    paging: Paging::LimitOffset,
    lex: LexRules {
        double_quote: DoubleQuote::Ident,
        backtick_idents: true,
        bracket_idents: true,
        backslash_escapes: false,
        escape_strings: false,
        unicode_strings: false,
        hex_strings: true,
        bit_strings: false,
        hex_numbers: true,
        dollar_quotes: false,
        numbered_params: false,
        dollar_idents: false,
        hash_comment: false,
        dash_comment_needs_space: false,
        nested_block_comments: false,
        separator: Separator::Semicolon,
        bodies: Bodies::TriggerOnly,
        lookahead: 4,
    },
    keywords: KEYWORDS,
};

/// A number column's value that goes in bare: a plain decimal, with no exponent or hex.
pub(super) fn is_bare_number(column: &ColumnInfo, text: &str) -> bool {
    let t = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = t.split_once('.').map_or((t, None), |(i, f)| (i, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    column.class == TypeClass::Number && digits(int) && frac.is_none_or(digits)
}

/// True when the statement whose first token is `toks[k]` creates a trigger, whose body may hold `;`.
pub(super) fn trigger_header(sql: &str, toks: &[Token], k: usize) -> bool {
    let mut sig = toks.get(k..).unwrap_or_default().iter().filter(|t| significant(t)).take(16);
    if !sig.next().is_some_and(|t| is_word(sql, t, "create")) {
        return false;
    }
    let mut rest = sig.skip_while(|t| is_word(sql, t, "temp") || is_word(sql, t, "temporary"));
    rest.next().is_some_and(|t| is_word(sql, t, "trigger"))
}

/// Counts the `BEGIN … END` blocks of a trigger body; a `CASE … END` nests too.
#[derive(Default)]
pub(crate) struct Trigger {
    pub(super) depth: usize,
}

impl Trigger {
    pub(super) fn step(&mut self, sql: &str, toks: &[Token], k: usize) {
        let Some(w) = toks.get(k).and_then(|t| word(sql, t)) else { return };
        if w.eq_ignore_ascii_case("begin") || w.eq_ignore_ascii_case("case") {
            self.depth += 1;
        } else if w.eq_ignore_ascii_case("end") {
            self.depth = self.depth.saturating_sub(1);
        }
    }
}

/// SQLite's keywords, from the SQLite Keywords page of sqlite.org, lowercase and sorted.
static KEYWORDS: &[&str] = &[
    "abort",
    "action",
    "add",
    "after",
    "all",
    "alter",
    "always",
    "analyze",
    "and",
    "as",
    "asc",
    "attach",
    "autoincrement",
    "before",
    "begin",
    "between",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "conflict",
    "constraint",
    "create",
    "cross",
    "current",
    "current_date",
    "current_time",
    "current_timestamp",
    "database",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "desc",
    "detach",
    "distinct",
    "do",
    "drop",
    "each",
    "else",
    "end",
    "escape",
    "except",
    "exclude",
    "exclusive",
    "exists",
    "explain",
    "fail",
    "filter",
    "first",
    "following",
    "for",
    "foreign",
    "from",
    "full",
    "generated",
    "glob",
    "group",
    "groups",
    "having",
    "if",
    "ignore",
    "immediate",
    "in",
    "index",
    "indexed",
    "initially",
    "inner",
    "insert",
    "instead",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "last",
    "left",
    "like",
    "limit",
    "match",
    "materialized",
    "natural",
    "no",
    "not",
    "nothing",
    "notnull",
    "null",
    "nulls",
    "of",
    "offset",
    "on",
    "or",
    "order",
    "others",
    "outer",
    "over",
    "partition",
    "plan",
    "pragma",
    "preceding",
    "primary",
    "query",
    "raise",
    "range",
    "recursive",
    "references",
    "regexp",
    "reindex",
    "release",
    "rename",
    "replace",
    "restrict",
    "returning",
    "right",
    "rollback",
    "row",
    "rows",
    "savepoint",
    "select",
    "set",
    "table",
    "temp",
    "temporary",
    "then",
    "ties",
    "to",
    "transaction",
    "trigger",
    "unbounded",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "values",
    "view",
    "virtual",
    "when",
    "where",
    "window",
    "with",
    "without",
];

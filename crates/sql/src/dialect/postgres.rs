//! PostgreSQL.

use super::{Bodies, DoubleQuote, ILike, InsertDefaults, LexRules, LikeOperators, Paging, Rules, Separator, StringLiteral, Values, quoted};
use crate::SqlError;
use crate::refusal::{CHAIN, CHANGES_MODE, Code, READ_WRITE, Refusal};
use dbee_core::ColumnInfo;

pub(crate) const RULES: Rules = Rules {
    ident_quote: '"',
    text_type: "pg_catalog.text",
    // Named in `pg_catalog`, so no `count` made in a schema the search path lists first is used.
    count_all: "pg_catalog.count(*)",
    operator_schema: Some("pg_catalog"),
    like_operators: Some(LikeOperators { like: "~~", not_like: "!~~", ilike: "~~*", not_ilike: "!~~*" }),
    ilike: ILike::Native,
    // Its LIKE escapes with `\` already.
    like_escape: "",
    insert_defaults: InsertDefaults::DefaultValues,
    use_database: None,
    // The protocol carries statements as C strings.
    text_holds_nul: false,
    bool_literals: ("t", "f"),
    op_notes: &[],
    schema_word: ("schema", "Schemas"),
    string_literal: StringLiteral::EscapeForBackslash,
    binary_literal: None,
    values: Values::Cast,
    paging: Paging::LimitOffset,
    lex: LexRules {
        double_quote: DoubleQuote::Ident,
        backtick_idents: false,
        bracket_idents: false,
        backslash_escapes: false,
        escape_strings: true,
        unicode_strings: true,
        hex_strings: false,
        bit_strings: false,
        hex_numbers: false,
        dollar_quotes: true,
        numbered_params: true,
        dollar_idents: false,
        hash_comment: false,
        dash_comment_needs_space: false,
        nested_block_comments: true,
        executable_comments: false,
        separator: Separator::Semicolon,
        bodies: Bodies::None,
        lookahead: 4,
    },
    keywords: KEYWORDS,
    refuse,
};

/// `'value'`, but a dollar quote when it holds a backslash, whose tag occurs neither in the value
/// nor across its end: no escape reaches the server then, so the value reads the same whatever
/// `standard_conforming_strings`, `client_encoding` or `backslash_quote` say, which a function the
/// session runs can change.
pub(super) fn literal(value: &str) -> Result<String, SqlError> {
    if !value.contains('\\') {
        return quoted(value, '\'');
    }
    if value.contains('\0') {
        return Err(SqlError::Nul);
    }
    let fits = |tag: &String| format!("{value}{tag}").find(tag.as_str()) == Some(value.len());
    let tag = ["$v$".to_owned(), "$dbee$".to_owned()].into_iter().chain((1..=8).map(|n| format!("$v{n}$"))).find(fits).unwrap_or_else(|| {
        // One digit more than the longest run of digits in the value: no `$v` and that many digits
        // occur in it, and the tag's own `$` ends a run that reaches the value's end.
        let run = value.split(|c: char| !c.is_ascii_digit()).map(str::len).max().unwrap_or(0);
        format!("$v{}$", "1".repeat(run.saturating_add(1)))
    });
    Ok(format!("{tag}{value}{tag}"))
}

/// `CAST(<literal> AS type_sql)`. `type_sql` comes from the server's `format_type` (or is a
/// type without its length, from [`write_type`] or [`filter_type`]) and is used verbatim.
fn typed(value: &str, type_sql: &str) -> Result<String, SqlError> {
    Ok(format!("CAST({} AS {type_sql})", literal(value)?))
}

/// A value to write, cast to [`write_type`].
pub(super) fn value(column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
    typed(text, write_type(column))
}

/// A value of the row's key, cast to the column's own type: the server sent it, so it fits.
pub(super) fn key_value(column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
    typed(text, &column.type_sql)
}

/// A value a filter compares the column with, cast to [`filter_type`].
pub(super) fn filter_value(column: &ColumnInfo, text: &str) -> Result<String, SqlError> {
    typed(text, filter_type(column))
}

/// The type a value to write is cast to. Postgres defines an explicit cast to
/// `varchar(n)`, `char(n)` and `bit varying(n)` as cutting the value to the length, and to
/// `bit(n)` as cutting or padding it, without an error; only the assignment to the column (or
/// to a domain over such a type) refuses a value that does not fit. So these, their arrays
/// and domains over them are cast to the type without its length: `text`, `bit varying`,
/// `text[]` or `bit varying[]`. Every other type is cast to `type_sql` itself. A key value the
/// server sent fits its column, so key comparisons keep `type_sql`.
fn write_type(column: &ColumnInfo) -> &str {
    match column.base_type.as_str() {
        "varchar" | "bpchar" => "pg_catalog.text",
        "bit" | "varbit" => "bit varying",
        "_varchar" | "_bpchar" => "pg_catalog.text[]",
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
/// others are [`write_type`]: `varchar(n)` as `text`, whose indexes it uses.
fn filter_type(column: &ColumnInfo) -> &str {
    match column.base_type.as_str() {
        "bpchar" => "pg_catalog.bpchar",
        "bit" => "pg_catalog.\"bit\"",
        "_varchar" => "character varying[]",
        "_bpchar" => "pg_catalog.bpchar[]",
        "_bit" => "pg_catalog.\"bit\"[]",
        _ => write_type(column),
    }
}

/// The settings that hold read-only mode: the transaction's, the session's default, and
/// `SESSION CHARACTERISTICS`, which sets that default.
const READ_ONLY_SETTINGS: &[&str] = &["default_transaction_read_only", "transaction_read_only", "characteristics"];

/// The large-object functions that write; Postgres 14 runs them in a read-only transaction too
/// (measured; 18 refuses them).
const LARGE_OBJECT_WRITERS: &[&str] =
    &["lo_create", "lo_creat", "lo_from_bytea", "lo_import", "lo_put", "lowrite", "lo_truncate", "lo_truncate64", "lo_unlink"];

const CONFORMING: &str = "standard_conforming_strings";

/// The values that turn a setting on.
const ON: &[&str] = &["on", "true", "yes", "1"];

const CONFORMING_OFF: &str = "dbee keeps standard_conforming_strings on: with it off the server reads backslashes in strings differently \
                              from how dbee shows and splits statements. This statement turns it off, so dbee did not run it.";

/// Why dbee does not send a statement: on any connection, one that turns
/// `standard_conforming_strings` off, which dbee's lexer and literals read as on; on a read-only
/// one, those that can turn read-only off or write in spite of it.
fn refuse(s: &Code<'_>, read_only: bool) -> Option<Refusal> {
    let turns_conforming_off = (s.starts(&["set"]) && s.names(&[CONFORMING]) && !s.value_of(CONFORMING).is_some_and(|v| ON.contains(&v.as_str())))
        || (s.starts(&["reset"]) && s.names(&[CONFORMING]))
        || (s.names(&["set_config"]) && s.string_holds(CONFORMING));
    if turns_conforming_off {
        return Some(Refusal::Misread(CONFORMING_OFF));
    }
    if !read_only {
        return None;
    }
    let why = if s.starts(&["do"]) {
        "a DO block can commit and turn read-only off, so dbee does not run one here. To run it, turn Read-only off in the connection's settings."
    } else if s.starts(&["call"]) {
        "a procedure can commit and turn read-only off, so dbee does not call one here. To call it, turn Read-only off in the connection's settings."
    } else if s.starts(&["reset", "discard"]) && s.names(&["all"]) {
        "this statement resets the session's settings, read-only among them. Reset each setting by its name instead."
    } else if s.starts(&["set", "reset"]) && (s.names(READ_ONLY_SETTINGS) || s.pair("read", "write"))
        || s.names(&["set_config"]) && s.string_holds("read_only")
    {
        CHANGES_MODE
    } else if s.starts(&["begin", "start"]) && s.pair("read", "write") {
        READ_WRITE
    } else if s.starts(&["commit", "rollback", "end", "abort"]) && s.names(&["chain"]) && !s.pair("no", "chain") {
        CHAIN
    } else if s.names(LARGE_OBJECT_WRITERS) {
        "large-object functions can write in a read-only transaction on some Postgres versions, so dbee does not run them here. To run them, \
         turn Read-only off in the connection's settings."
    } else {
        return None;
    };
    Some(Refusal::ReadOnly(why))
}

/// Common PostgreSQL keywords (reserved and non-reserved, from the SQL Key Words appendix of the
/// PostgreSQL documentation), lowercase and sorted so `binary_search` works.
static KEYWORDS: &[&str] = &[
    "abort",
    "absolute",
    "access",
    "action",
    "add",
    "admin",
    "after",
    "aggregate",
    "all",
    "alter",
    "always",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "assertion",
    "assignment",
    "asymmetric",
    "at",
    "attach",
    "attribute",
    "authorization",
    "backward",
    "before",
    "begin",
    "between",
    "bigint",
    "binary",
    "bit",
    "boolean",
    "both",
    "by",
    "cache",
    "call",
    "called",
    "cascade",
    "cascaded",
    "case",
    "cast",
    "catalog",
    "chain",
    "char",
    "character",
    "characteristics",
    "check",
    "checkpoint",
    "class",
    "close",
    "cluster",
    "coalesce",
    "collate",
    "collation",
    "column",
    "columns",
    "comment",
    "comments",
    "commit",
    "committed",
    "concurrently",
    "configuration",
    "conflict",
    "connection",
    "constraint",
    "constraints",
    "content",
    "continue",
    "conversion",
    "copy",
    "cost",
    "create",
    "cross",
    "csv",
    "cube",
    "current",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "cycle",
    "data",
    "database",
    "day",
    "deallocate",
    "dec",
    "decimal",
    "declare",
    "default",
    "defaults",
    "deferrable",
    "deferred",
    "definer",
    "delete",
    "delimiter",
    "delimiters",
    "depends",
    "desc",
    "detach",
    "dictionary",
    "disable",
    "discard",
    "distinct",
    "do",
    "document",
    "domain",
    "double",
    "drop",
    "each",
    "else",
    "enable",
    "encoding",
    "encrypted",
    "end",
    "enum",
    "escape",
    "event",
    "except",
    "exclude",
    "excluding",
    "exclusive",
    "execute",
    "exists",
    "explain",
    "expression",
    "extension",
    "external",
    "extract",
    "false",
    "family",
    "fetch",
    "filter",
    "first",
    "float",
    "following",
    "for",
    "force",
    "foreign",
    "forward",
    "freeze",
    "from",
    "full",
    "function",
    "functions",
    "generated",
    "global",
    "grant",
    "granted",
    "greatest",
    "group",
    "grouping",
    "groups",
    "handler",
    "having",
    "header",
    "hold",
    "hour",
    "identity",
    "if",
    "ilike",
    "immediate",
    "immutable",
    "implicit",
    "import",
    "in",
    "include",
    "including",
    "increment",
    "index",
    "indexes",
    "inherit",
    "inherits",
    "initially",
    "inline",
    "inner",
    "inout",
    "input",
    "insensitive",
    "insert",
    "instead",
    "int",
    "integer",
    "intersect",
    "interval",
    "into",
    "invoker",
    "is",
    "isnull",
    "isolation",
    "join",
    "key",
    "label",
    "language",
    "large",
    "last",
    "lateral",
    "leading",
    "leakproof",
    "least",
    "left",
    "level",
    "like",
    "limit",
    "listen",
    "load",
    "local",
    "localtime",
    "localtimestamp",
    "location",
    "lock",
    "locked",
    "logged",
    "mapping",
    "match",
    "materialized",
    "maxvalue",
    "method",
    "minute",
    "minvalue",
    "mode",
    "month",
    "move",
    "name",
    "names",
    "national",
    "natural",
    "nchar",
    "new",
    "next",
    "no",
    "none",
    "not",
    "nothing",
    "notify",
    "notnull",
    "nowait",
    "null",
    "nullif",
    "nulls",
    "numeric",
    "object",
    "of",
    "off",
    "offset",
    "oids",
    "old",
    "on",
    "only",
    "operator",
    "option",
    "options",
    "or",
    "order",
    "ordinality",
    "others",
    "out",
    "outer",
    "over",
    "overlaps",
    "overlay",
    "overriding",
    "owned",
    "owner",
    "parallel",
    "parser",
    "partial",
    "partition",
    "passing",
    "password",
    "placing",
    "plans",
    "policy",
    "position",
    "preceding",
    "precision",
    "prepare",
    "prepared",
    "preserve",
    "primary",
    "prior",
    "privileges",
    "procedural",
    "procedure",
    "procedures",
    "program",
    "publication",
    "quote",
    "range",
    "read",
    "real",
    "reassign",
    "recheck",
    "recursive",
    "ref",
    "references",
    "referencing",
    "refresh",
    "reindex",
    "relative",
    "release",
    "rename",
    "repeatable",
    "replace",
    "replica",
    "reset",
    "restart",
    "restrict",
    "return",
    "returning",
    "returns",
    "revoke",
    "right",
    "role",
    "rollback",
    "rollup",
    "routine",
    "routines",
    "row",
    "rows",
    "rule",
    "savepoint",
    "schema",
    "schemas",
    "scroll",
    "search",
    "second",
    "security",
    "select",
    "sequence",
    "sequences",
    "serializable",
    "server",
    "session",
    "session_user",
    "set",
    "setof",
    "sets",
    "share",
    "show",
    "similar",
    "simple",
    "skip",
    "smallint",
    "snapshot",
    "some",
    "sql",
    "stable",
    "standalone",
    "start",
    "statement",
    "statistics",
    "stdin",
    "stdout",
    "storage",
    "stored",
    "strict",
    "strip",
    "subscription",
    "substring",
    "support",
    "symmetric",
    "sysid",
    "system",
    "table",
    "tables",
    "tablesample",
    "tablespace",
    "temp",
    "template",
    "temporary",
    "text",
    "then",
    "ties",
    "time",
    "timestamp",
    "to",
    "trailing",
    "transaction",
    "transform",
    "treat",
    "trigger",
    "trim",
    "true",
    "truncate",
    "trusted",
    "type",
    "types",
    "unbounded",
    "uncommitted",
    "unencrypted",
    "union",
    "unique",
    "unknown",
    "unlisten",
    "unlogged",
    "until",
    "update",
    "user",
    "using",
    "vacuum",
    "valid",
    "validate",
    "validator",
    "value",
    "values",
    "varchar",
    "variadic",
    "varying",
    "verbose",
    "version",
    "view",
    "views",
    "volatile",
    "when",
    "where",
    "whitespace",
    "window",
    "with",
    "within",
    "without",
    "work",
    "wrapper",
    "write",
    "xml",
    "year",
    "yes",
    "zone",
];

#[cfg(test)]
mod tests {
    use super::*;
    use dbee_core::TypeClass;

    fn column(type_sql: &str, base: &str) -> ColumnInfo {
        ColumnInfo {
            name: "x".into(),
            type_sql: type_sql.into(),
            base_type: base.into(),
            class: TypeClass::Other,
            not_null: false,
            has_default: false,
            generated: Default::default(),
            hidden: false,
            type_label: String::new(),
            builtin_operators: false,
        }
    }

    #[test]
    fn typed_casts_a_literal() {
        assert_eq!(typed("42", "integer").unwrap(), "CAST('42' AS integer)");
        assert_eq!(typed("a\0", "text"), Err(SqlError::Nul));
    }

    #[test]
    fn write_type_drops_the_lengths_an_explicit_cast_cuts_to() {
        for (base, type_sql, write) in [
            ("varchar", "character varying(10)", "pg_catalog.text"),
            ("varchar", "character varying", "pg_catalog.text"),
            ("bpchar", "character(2)", "pg_catalog.text"),
            ("bit", "bit(2)", "bit varying"),
            ("varbit", "bit varying(2)", "bit varying"),
            ("_varchar", "character varying(3)[]", "pg_catalog.text[]"),
            ("_bpchar", "character(2)[]", "pg_catalog.text[]"),
            ("_bit", "bit(2)[]", "bit varying[]"),
            ("_varbit", "bit varying(2)[]", "bit varying[]"),
            // A domain over varchar(5): the base type decides, the assignment to the column checks the domain.
            ("varchar", "scratch.d5", "pg_catalog.text"),
            // Everything else is written as its own type.
            ("text", "text", "text"),
            ("int4", "integer", "integer"),
            ("numeric", "numeric(12,2)", "numeric(12,2)"),
            ("timestamp", "timestamp(3) without time zone", "timestamp(3) without time zone"),
            ("char", "\"char\"", "\"char\""),
            ("_text", "text[]", "text[]"),
            ("order_status", "shop.order_status", "shop.order_status"),
        ] {
            assert_eq!(write_type(&column(type_sql, base)), write, "{base} {type_sql}");
        }
    }
}

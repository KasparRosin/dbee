//! MySQL and MariaDB.

use super::{BinaryLiteral, Bodies, DoubleQuote, ILike, InsertDefaults, LexRules, Paging, Rules, Separator, StringLiteral, Values, quoted};
use crate::SqlError;
use crate::filter::Op;
use crate::lexer::{Token, TokenKind, code, is_word, word};
use crate::refusal::{CHAIN, CHANGES_MODE, Code, READ_WRITE, Refusal};

pub(crate) const RULES: Rules = Rules {
    ident_quote: '`',
    text_type: "CHAR",
    count_all: "count(*)",
    operator_schema: None,
    like_operators: None,
    ilike: ILike::LowerLike,
    // 8.0 and later have no LIKE escape under NO_BACKSLASH_ESCAPES (measured); hex reads alike in every mode.
    like_escape: " ESCAPE _utf8mb4 X'5C'",
    insert_defaults: InsertDefaults::EmptyLists,
    use_database: Some("USE {}"),
    // Measured on MySQL 5.7, 8.0 and 26.10 and MariaDB 10.11 and 13.0: a NUL inside a string
    // literal is a character. Outside one, a NUL followed by more text (a comment too) is a syntax
    // error, one in a /* */ comment is ignored, and one followed only by spaces ends the
    // statement there, so the engine refuses every NUL outside a string literal.
    text_holds_nul: true,
    bool_literals: ("1", "0"),
    op_notes: &[(&[Op::Eq, Op::Ne, Op::Like, Op::NotLike], "Compares by the column's collation; most ignore upper and lower case.")],
    schema_word: ("database", "Databases"),
    string_literal: StringLiteral::HexForBackslash,
    binary_literal: Some(BinaryLiteral::ZeroX),
    values: Values::Literal,
    paging: Paging::LimitOffset,
    lex: LexRules {
        double_quote: DoubleQuote::String,
        backtick_idents: true,
        bracket_idents: false,
        backslash_escapes: true,
        escape_strings: false,
        unicode_strings: false,
        hex_strings: true,
        bit_strings: true,
        hex_numbers: true,
        dollar_quotes: false,
        numbered_params: false,
        dollar_idents: true,
        hash_comment: true,
        dash_comment_needs_space: true,
        nested_block_comments: false,
        executable_comments: true,
        separator: Separator::DelimiterCommand,
        bodies: Bodies::Compound,
        lookahead: 4,
    },
    keywords: KEYWORDS,
    refuse,
};

/// `'value'`, but `_utf8mb4 X'…'` when it holds a backslash, whose escaping depends on the
/// session's `sql_mode`, or a NUL, so that none reaches the statement as a raw byte.
pub(super) fn literal(value: &str) -> Result<String, SqlError> {
    if !value.contains(['\\', '\0']) {
        return quoted(value, '\'');
    }
    let hex: String = value.bytes().map(|b| format!("{b:02X}")).collect();
    Ok(format!("_utf8mb4 X'{hex}'"))
}

/// True when the statement whose first token is `toks[k]` creates a procedure, function, trigger
/// or event: one whose body may hold `;`.
pub(super) fn routine_header(sql: &str, toks: &[Token], k: usize) -> bool {
    let mut sig = toks.get(k..).unwrap_or_default().iter().filter(|t| code(t)).take(16);
    if !sig.next().is_some_and(|t| is_word(sql, t, "create")) {
        return false;
    }
    // A bare word after `=` or `@` is the account in DEFINER = user@host.
    let mut account = false;
    for t in sig {
        if account {
            account = false;
            if word(sql, t).is_some() {
                continue;
            }
        }
        match t.kind {
            TokenKind::Keyword | TokenKind::Ident => {
                if ["procedure", "function", "trigger", "event"].iter().any(|w| is_word(sql, t, w)) {
                    return true;
                }
                if !["or", "replace", "aggregate", "definer"].iter().any(|w| is_word(sql, t, w)) {
                    return false;
                }
            }
            TokenKind::String | TokenKind::QuotedIdent => {}
            TokenKind::Operator | TokenKind::Other => match sql.get(t.range.clone()) {
                Some("=" | "@") => account = true,
                Some("(" | ")") => {}
                _ => return false,
            },
            _ => return false,
        }
    }
    false
}

/// Counts the `BEGIN … END` blocks of a routine body, token by token.
#[derive(Default)]
pub(crate) struct Compound {
    pub(super) depth: usize,
    /// A token here starts a statement of the body, where `IF`, `LOOP`, `REPEAT` and `WHILE` open blocks.
    fresh: bool,
    /// The token `END` took along (`END IF`).
    taken: Option<usize>,
    /// A `CREATE PROCEDURE`: the token after its parameter list starts a statement.
    procedure: bool,
    parens: usize,
    params_done: bool,
}

impl Compound {
    pub(super) fn step(&mut self, sql: &str, toks: &[Token], k: usize) {
        let Some(t) = toks.get(k).filter(|t| code(t)) else { return };
        if let Some(w) = word(sql, t).filter(|_| self.taken != Some(k)) {
            let is = |x: &str| w.eq_ignore_ascii_case(x);
            let next = toks.get(k + 1..).unwrap_or_default().iter().position(code).map(|p| k + 1 + p);
            if is("begin") || is("case") {
                self.depth += 1;
            } else if is("end") {
                self.depth = self.depth.saturating_sub(1);
                if let Some(n) =
                    next.filter(|&n| toks.get(n).is_some_and(|t| ["if", "case", "loop", "repeat", "while"].iter().any(|x| is_word(sql, t, x))))
                {
                    self.taken = Some(n);
                }
            } else if self.fresh {
                let call = sql.as_bytes().get(t.range.end) == Some(&b'(');
                let exists = next.and_then(|n| toks.get(n)).is_some_and(|t| is_word(sql, t, "exists"));
                if is("loop") || is("while") || (is("if") && !call && !exists) || (is("repeat") && !call) {
                    self.depth += 1;
                }
            }
            self.fresh = ["begin", "then", "else", "do", "row", "loop", "repeat"].iter().any(|x| is(x));
            self.procedure |= is("procedure") && !self.params_done;
        } else {
            let text = sql.get(t.range.clone());
            self.fresh = t.kind == TokenKind::Semicolon || (t.kind == TokenKind::Other && text == Some(":"));
            if t.kind == TokenKind::Other && self.procedure && !self.params_done {
                match text {
                    Some("(") => self.parens += 1,
                    Some(")") => {
                        self.parens = self.parens.saturating_sub(1);
                        self.params_done = self.parens == 0;
                        self.fresh = self.params_done;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The session variables that hold read-only mode: MySQL's and MariaDB's spellings.
const READ_ONLY_VARIABLES: &[&str] = &["transaction_read_only", "tx_read_only"];

/// The client character sets under which the server reads dbee's UTF-8 text as dbee does.
const UTF8: &[&str] = &["utf8mb4", "utf8mb3", "utf8"];

/// A `SET` of the character set the server reads statements in to anything but one of [`UTF8`]
/// alone: `NAMES`, `CHARACTER SET` (or `CHAR SET`, which the servers read as one) or `CHARSET` (a
/// `COLLATE` may follow), or `character_set_client` with `=` or `:=`.
fn sets_character_set(s: &Code<'_>) -> bool {
    let utf8 = |at: usize, collate: bool| {
        s.literal(at).is_some_and(|v| UTF8.contains(&v.as_str())) && (s.text(at + 1).is_none_or(|t| t == ",") || (collate && s.is(at + 1, "collate")))
    };
    let names = s.assignments().into_iter().any(|i| {
        let value = if s.is(i, "names") || s.is(i, "charset") {
            i + 1
        } else if (s.is(i, "character") || s.is(i, "char")) && s.is(i + 1, "set") {
            i + 2
        } else {
            return false;
        };
        !utf8(value, true)
    });
    // `=` takes the operator characters after it along (`=@x`), and `:=` is two tokens.
    let client = s.starts(&["set"])
        && s.naming("character_set_client").into_iter().any(|i| match (s.text(i + 1), s.text(i + 2)) {
            (Some("="), _) => !utf8(i + 2, false),
            (Some(":"), Some("=")) => !utf8(i + 3, false),
            (Some(sign), _) => sign.starts_with('=') || sign == ":",
            (None, _) => false,
        });
    names || client
}

/// Why dbee does not send a statement on a read-only connection: those that can turn read-only
/// off, or hide what they run. Nothing is refused on another connection.
fn refuse(s: &Code<'_>, read_only: bool) -> Option<Refusal> {
    if !read_only {
        return None;
    }
    let why = if s.starts(&["call"]) {
        "a procedure can start a read-write transaction, so dbee does not call one here. To call it, turn Read-only off in the connection's settings."
    } else if s.starts(&["prepare", "execute"]) {
        "dbee cannot see what a prepared statement runs, so it does not prepare or execute one here. To run it, turn Read-only off in the \
         connection's settings."
    } else if s.starts(&["set"]) && s.is(1, "statement") {
        "SET STATEMENT can turn read-only off for the statement it runs, so dbee does not run it here. Run the statement without it."
    } else if s.starts(&["set"]) && (s.sets_off(READ_ONLY_VARIABLES) || s.pair("read", "write")) {
        CHANGES_MODE
    } else if s.starts(&["set"]) && s.sets_scope(&["global", "persist", "persist_only"]) {
        "SET GLOBAL and SET PERSIST change the server's settings for every session, so dbee does not run them here. To run them, turn \
         Read-only off in the connection's settings."
    } else if sets_character_set(s) {
        "on a read-only connection dbee keeps the character set at utf8mb4, so it does not run this statement: under another character \
         set, the server can read dbee's text differently from how dbee reads it. To change it, turn Read-only off in the connection's \
         settings."
    } else if s.pair("into", "outfile") || s.pair("into", "dumpfile") {
        "INTO OUTFILE and INTO DUMPFILE write a file on the server, so dbee does not run them here. To run one, turn Read-only off in the \
         connection's settings."
    } else if s.starts(&["start"]) && s.pair("read", "write") {
        READ_WRITE
    } else if s.starts(&["commit", "rollback"]) && s.names(&["chain"]) && !s.pair("no", "chain") {
        CHAIN
    } else {
        return None;
    };
    Some(Refusal::ReadOnly(why))
}

/// MySQL's reserved words, from the Keywords and Reserved Words page of the MySQL Reference Manual
/// (the entries marked (R)), lowercase and sorted.
static KEYWORDS: &[&str] = &[
    "accessible",
    "add",
    "all",
    "alter",
    "analyze",
    "and",
    "as",
    "asc",
    "asensitive",
    "before",
    "between",
    "bigint",
    "binary",
    "blob",
    "both",
    "by",
    "call",
    "cascade",
    "case",
    "change",
    "char",
    "character",
    "check",
    "collate",
    "column",
    "condition",
    "constraint",
    "continue",
    "convert",
    "create",
    "cross",
    "cube",
    "cume_dist",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "database",
    "databases",
    "day_hour",
    "day_microsecond",
    "day_minute",
    "day_second",
    "dec",
    "decimal",
    "declare",
    "default",
    "delayed",
    "delete",
    "dense_rank",
    "desc",
    "describe",
    "deterministic",
    "distinct",
    "distinctrow",
    "div",
    "double",
    "drop",
    "dual",
    "each",
    "else",
    "elseif",
    "empty",
    "enclosed",
    "escaped",
    "except",
    "exists",
    "exit",
    "explain",
    "false",
    "fetch",
    "first_value",
    "float",
    "float4",
    "float8",
    "for",
    "force",
    "foreign",
    "from",
    "fulltext",
    "function",
    "generated",
    "get",
    "grant",
    "group",
    "grouping",
    "groups",
    "having",
    "high_priority",
    "hour_microsecond",
    "hour_minute",
    "hour_second",
    "if",
    "ignore",
    "in",
    "index",
    "infile",
    "inner",
    "inout",
    "insensitive",
    "insert",
    "int",
    "int1",
    "int2",
    "int3",
    "int4",
    "int8",
    "integer",
    "intersect",
    "interval",
    "into",
    "io_after_gtids",
    "io_before_gtids",
    "is",
    "iterate",
    "join",
    "json_table",
    "key",
    "keys",
    "kill",
    "lag",
    "last_value",
    "lateral",
    "lead",
    "leading",
    "leave",
    "left",
    "like",
    "limit",
    "linear",
    "lines",
    "load",
    "localtime",
    "localtimestamp",
    "lock",
    "long",
    "longblob",
    "longtext",
    "loop",
    "low_priority",
    "master_bind",
    "master_ssl_verify_server_cert",
    "match",
    "maxvalue",
    "mediumblob",
    "mediumint",
    "mediumtext",
    "middleint",
    "minute_microsecond",
    "minute_second",
    "mod",
    "modifies",
    "natural",
    "no_write_to_binlog",
    "not",
    "nth_value",
    "ntile",
    "null",
    "numeric",
    "of",
    "on",
    "optimize",
    "optimizer_costs",
    "option",
    "optionally",
    "or",
    "order",
    "out",
    "outer",
    "outfile",
    "over",
    "partition",
    "percent_rank",
    "precision",
    "primary",
    "procedure",
    "purge",
    "range",
    "rank",
    "read",
    "read_write",
    "reads",
    "real",
    "recursive",
    "references",
    "regexp",
    "release",
    "rename",
    "repeat",
    "replace",
    "require",
    "resignal",
    "restrict",
    "return",
    "revoke",
    "right",
    "rlike",
    "row",
    "row_number",
    "rows",
    "schema",
    "schemas",
    "second_microsecond",
    "select",
    "sensitive",
    "separator",
    "set",
    "show",
    "signal",
    "smallint",
    "spatial",
    "specific",
    "sql",
    "sql_big_result",
    "sql_calc_found_rows",
    "sql_small_result",
    "sqlexception",
    "sqlstate",
    "sqlwarning",
    "ssl",
    "starting",
    "stored",
    "straight_join",
    "system",
    "table",
    "terminated",
    "then",
    "tinyblob",
    "tinyint",
    "tinytext",
    "to",
    "trailing",
    "trigger",
    "true",
    "undo",
    "union",
    "unique",
    "unlock",
    "unsigned",
    "update",
    "usage",
    "use",
    "using",
    "utc_date",
    "utc_time",
    "utc_timestamp",
    "values",
    "varbinary",
    "varchar",
    "varcharacter",
    "varying",
    "virtual",
    "when",
    "where",
    "while",
    "window",
    "with",
    "write",
    "xor",
    "year_month",
    "zerofill",
];

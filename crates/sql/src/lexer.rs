//! A small SQL lexer for colouring and statement splitting. One linear pass; never panics, and
//! the tokens are contiguous and cover the whole input.

use std::ops::Range;

/// Common PostgreSQL keywords (reserved and non-reserved, from the SQL Key Words appendix of the
/// PostgreSQL documentation), lowercase and sorted so `binary_search` works.
pub static KEYWORDS: &[&str] = &[
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Ident,
    QuotedIdent,
    String,
    Number,
    Comment,
    Operator,
    Semicolon,
    Whitespace,
    Param,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: Range<usize>,
}

fn is_keyword(word: &str) -> bool {
    if word.len() > 24 || !word.is_ascii() {
        return false;
    }
    let lower = word.to_ascii_lowercase();
    KEYWORDS.binary_search(&lower.as_str()).is_ok()
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_cont(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit() || b == b'$'
}

fn is_op_byte(b: u8) -> bool {
    b"+-*/<>=~!@#%^&|?".contains(&b)
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// End of a `'…'` string starting after the opening quote at `i` (`''` escapes; with `backslash`,
/// `\x` skips one byte). Runs to the end of input if unterminated.
fn scan_quoted(b: &[u8], mut i: usize, quote: u8, backslash: bool) -> usize {
    while i < b.len() {
        let c = b[i];
        if backslash && c == b'\\' {
            i += 2;
        } else if c == quote {
            if at(b, i + 1) == quote {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += 1;
        }
    }
    b.len()
}

/// Dollar-quote opener at `i` (`$tag$`): returns the end of the opening delimiter.
fn dollar_open(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if at(b, j) == b'$' {
        return Some(j + 1);
    }
    if !is_ident_start(at(b, j)) {
        return None;
    }
    while j < b.len() && (is_ident_start(b[j]) || b[j].is_ascii_digit()) {
        j += 1;
    }
    (at(b, j) == b'$').then_some(j + 1)
}

fn find_sub(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Splits `sql` into tokens. Ranges are contiguous and cover the input. An unterminated string,
/// quoted identifier, dollar quote or comment runs to the end.
pub fn tokenize(sql: &str) -> Vec<Token> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let c = b[i];
        let kind;
        if c.is_ascii_whitespace() || c == 0x0b {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == 0x0b) {
                i += 1;
            }
            kind = TokenKind::Whitespace;
        } else if c == b'-' && at(b, i + 1) == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            kind = TokenKind::Comment;
        } else if c == b'/' && at(b, i + 1) == b'*' {
            let mut depth = 1usize;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'/' && at(b, i + 1) == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && at(b, i + 1) == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i = i.min(b.len());
            kind = TokenKind::Comment;
        } else if c == b'\'' {
            i = scan_quoted(b, i + 1, b'\'', false);
            kind = TokenKind::String;
        } else if (c == b'E' || c == b'e') && at(b, i + 1) == b'\'' {
            i = scan_quoted(b, i + 2, b'\'', true).min(b.len());
            kind = TokenKind::String;
        } else if (c == b'U' || c == b'u') && at(b, i + 1) == b'&' && (at(b, i + 2) == b'\'' || at(b, i + 2) == b'"') {
            let q = b[i + 2];
            i = scan_quoted(b, i + 3, q, false);
            kind = if q == b'"' { TokenKind::QuotedIdent } else { TokenKind::String };
        } else if c == b'"' {
            i = scan_quoted(b, i + 1, b'"', false);
            kind = TokenKind::QuotedIdent;
        } else if c == b'$' {
            if at(b, i + 1).is_ascii_digit() {
                i += 1;
                while at(b, i).is_ascii_digit() {
                    i += 1;
                }
                kind = TokenKind::Param;
            } else if let Some(open_end) = dollar_open(b, i) {
                let delim = &b[i..open_end];
                i = find_sub(b, open_end, delim).map_or(b.len(), |p| p + delim.len());
                kind = TokenKind::String;
            } else {
                i += 1;
                kind = TokenKind::Other;
            }
        } else if c.is_ascii_digit() || (c == b'.' && at(b, i + 1).is_ascii_digit()) {
            while at(b, i).is_ascii_digit() {
                i += 1;
            }
            if at(b, i) == b'.' {
                i += 1;
                while at(b, i).is_ascii_digit() {
                    i += 1;
                }
            }
            if matches!(at(b, i), b'e' | b'E') {
                let sign = usize::from(matches!(at(b, i + 1), b'+' | b'-'));
                if at(b, i + 1 + sign).is_ascii_digit() {
                    i += 1 + sign;
                    while at(b, i).is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            kind = TokenKind::Number;
        } else if is_ident_start(c) {
            while i < b.len() && is_ident_cont(b[i]) {
                i += 1;
            }
            kind = if is_keyword(&sql[start..i]) { TokenKind::Keyword } else { TokenKind::Ident };
        } else if c == b';' {
            i += 1;
            kind = TokenKind::Semicolon;
        } else if is_op_byte(c) {
            i += 1;
            while i < b.len() && is_op_byte(b[i]) && !(b[i] == b'-' && at(b, i + 1) == b'-') && !(b[i] == b'/' && at(b, i + 1) == b'*') {
                i += 1;
            }
            kind = TokenKind::Operator;
        } else {
            // One whole character (punctuation, or anything unexpected).
            i += 1;
            while i < b.len() && (b[i] & 0xC0) == 0x80 {
                i += 1;
            }
            kind = TokenKind::Other;
        }
        let end = i.min(b.len());
        out.push(Token { kind, range: start..end });
        i = end;
    }
    out
}

struct Stmt {
    range: Range<usize>,
    /// End of the span: through the `;` and the whitespace after it.
    span_end: usize,
}

fn scan_statements(sql: &str) -> Vec<Stmt> {
    let toks = tokenize(sql);
    let mut out = Vec::new();
    let mut cur: Option<Range<usize>> = None;
    for (k, t) in toks.iter().enumerate() {
        match t.kind {
            TokenKind::Whitespace | TokenKind::Comment => {}
            TokenKind::Semicolon => {
                if let Some(r) = cur.take() {
                    let mut span_end = t.range.end;
                    for w in &toks[k + 1..] {
                        if w.kind != TokenKind::Whitespace {
                            break;
                        }
                        span_end = w.range.end;
                    }
                    out.push(Stmt { range: r, span_end });
                }
            }
            _ => match &mut cur {
                Some(r) => r.end = t.range.end,
                None => cur = Some(t.range.clone()),
            },
        }
    }
    if let Some(range) = cur {
        out.push(Stmt { range, span_end: sql.len() });
    }
    out
}

/// Statements split on top-level `;`. Leading whitespace and comments, trailing comments and the
/// `;` are excluded; empty statements are dropped.
pub fn statements(sql: &str) -> Vec<Range<usize>> {
    scan_statements(sql).into_iter().map(|s| s.range).collect()
}

/// The statement whose span (its start through its `;` and the whitespace after it) holds `cursor`.
/// Past the end it is the last statement; `None` when there are no statements.
pub fn statement_at(sql: &str, cursor: usize) -> Option<Range<usize>> {
    let all = scan_statements(sql);
    let pick = all.iter().position(|s| cursor < s.span_end).or(all.len().checked_sub(1))?;
    all.into_iter().nth(pick).map(|s| s.range)
}

/// True when `sql` has a `;` outside strings, comments and quoted identifiers.
pub fn has_top_level_semicolon(sql: &str) -> bool {
    tokenize(sql).iter().any(|t| t.kind == TokenKind::Semicolon)
}
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn kinds(sql: &str) -> Vec<(TokenKind, &str)> {
        tokenize(sql).into_iter().filter(|t| t.kind != TokenKind::Whitespace).map(|t| (t.kind, &sql[t.range])).collect()
    }

    #[test]
    fn splits_statements() {
        let sql = "select 1; select 'a;b'; -- c;\n select $x$ ; $x$; /* only */ ;";
        let got: Vec<&str> = statements(sql).into_iter().map(|r| &sql[r]).collect();
        assert_eq!(got, ["select 1", "select 'a;b'", "select $x$ ; $x$"]);
    }

    #[test]
    fn statement_under_cursor() {
        let sql = "select 1;\nselect 2;\n\nselect 3";
        assert_eq!(&sql[statement_at(sql, 0).unwrap()], "select 1");
        assert_eq!(&sql[statement_at(sql, 12).unwrap()], "select 2");
        assert_eq!(&sql[statement_at(sql, sql.len()).unwrap()], "select 3");
        assert_eq!(statement_at("", 0), None);
        assert_eq!(statement_at(" ; -- x", 3), None);
        assert_eq!(&sql[statement_at(sql, 9).unwrap()], "select 1");
        assert_eq!(&sql[statement_at(sql, 10).unwrap()], "select 2");
        assert_eq!(&sql[statement_at(sql, 99999).unwrap()], "select 3");
    }

    #[test]
    fn semicolons_inside_strings_do_not_count() {
        assert!(!has_top_level_semicolon("status = 'a;b'"));
        assert!(has_top_level_semicolon("x > 5; drop table t"));
        assert!(!has_top_level_semicolon("x /* ; */ -- ;"));
        assert!(!has_top_level_semicolon("select \"a;b\", $$;$$, E'\\';'"));
    }

    #[test]
    fn token_kinds() {
        use TokenKind::*;
        assert_eq!(kinds("SELECT foo"), [(Keyword, "SELECT"), (Ident, "foo")]);
        assert_eq!(kinds("select \"A\"\"b\""), [(Keyword, "select"), (QuotedIdent, "\"A\"\"b\"")]);
        assert_eq!(kinds("$1 + $23"), [(Param, "$1"), (Operator, "+"), (Param, "$23")]);
        assert_eq!(kinds("1.5e-3 .5 42"), [(Number, "1.5e-3"), (Number, ".5"), (Number, "42")]);
        assert_eq!(kinds("a -- x\nb"), [(Ident, "a"), (Comment, "-- x"), (Ident, "b")]);
        assert_eq!(kinds("/* a /* b */ c */ x"), [(Comment, "/* a /* b */ c */"), (Ident, "x")]);
        assert_eq!(kinds("'it''s'"), [(String, "'it''s'")]);
        assert_eq!(kinds(r"E'a\'b;'"), [(String, r"E'a\'b;'")]);
        assert_eq!(kinds("U&'d\\0061'"), [(String, "U&'d\\0061'")]);
        assert_eq!(kinds("$a$ x $b$ $a$"), [(String, "$a$ x $b$ $a$")]);
        assert_eq!(kinds("$$x$$"), [(String, "$$x$$")]);
        assert_eq!(kinds("a;"), [(Ident, "a"), (Semicolon, ";")]);
        assert_eq!(kinds("a<=b"), [(Ident, "a"), (Operator, "<="), (Ident, "b")]);
    }

    #[test]
    fn unterminated_runs_to_end() {
        use TokenKind::*;
        assert_eq!(kinds("'abc"), [(String, "'abc")]);
        assert_eq!(kinds("/* abc"), [(Comment, "/* abc")]);
        assert_eq!(kinds("$t$ abc $"), [(String, "$t$ abc $")]);
        assert_eq!(kinds("\"abc"), [(QuotedIdent, "\"abc")]);
        assert_eq!(kinds("E'abc\\"), [(String, "E'abc\\")]);
        assert_eq!(statements("select 'a;b"), vec![0..11]);
    }

    #[test]
    fn odd_dollars_and_chars_do_not_panic() {
        for s in [
            "$",
            "$$",
            "$ ",
            "$é$ x $é$",
            "$1a",
            "a$b",
            "$-",
            "🐝$🐝",
            "$t$",
            "é",
            "\u{0}",
            "U&",
            "U&\"x\"",
            "e'",
            "1e",
            "1e+",
            "--",
            "/*/",
            "/*/ */",
        ] {
            let t = tokenize(s);
            assert_eq!(t.last().map(|k| k.range.end).unwrap_or(0), s.len(), "{s:?}");
            let _ = statements(s);
            let _ = statement_at(s, 1);
        }
    }

    #[test]
    fn keywords_are_sorted_and_lowercase() {
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]), "KEYWORDS must be sorted and unique");
        assert!(KEYWORDS.iter().all(|k| k.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')));
        for k in ["select", "from", "where", "join", "insert", "update", "delete", "returning", "limit"] {
            assert!(KEYWORDS.binary_search(&k).is_ok(), "{k}");
        }
    }

    proptest! {
        #[test]
        fn tokens_cover_input(s in "\\PC{0,300}") {
            let t = tokenize(&s);
            let mut at = 0;
            for k in &t { prop_assert_eq!(k.range.start, at); prop_assert!(k.range.end > k.range.start); at = k.range.end; }
            prop_assert_eq!(at, s.len());
        }
        #[test]
        fn statements_are_in_bounds(s in "\\PC{0,300}") {
            for r in statements(&s) { prop_assert!(s.get(r).is_some()); }
        }
        #[test]
        fn sqlish_input_covers_and_splits(s in "[a-z0-9 ;'\"$E&U\\\\/*\\n-]{0,200}") {
            let t = tokenize(&s);
            prop_assert_eq!(t.last().map(|k| k.range.end).unwrap_or(0), s.len());
            let st = statements(&s);
            for w in st.windows(2) { prop_assert!(w[0].end <= w[1].start); }
            for r in &st { prop_assert!(s.get(r.clone()).is_some()); }
            for c in 0..=s.len() { prop_assert_eq!(statement_at(&s, c).is_some(), !st.is_empty()); }
        }
    }
}

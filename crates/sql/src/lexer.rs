//! A small SQL lexer for colouring and statement splitting. One linear pass; never panics, and
//! the tokens are contiguous and cover the whole input.

use std::ops::Range;

use crate::Dialect;

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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_cont(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit() || b == b'$'
}

/// `#` starts a comment in MySQL, so it is no operator there.
fn is_op_byte(d: Dialect, b: u8) -> bool {
    b"+-*/<>=~!@%^&|?".contains(&b) || (b == b'#' && d != Dialect::MySql)
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// A `--` at `i` starts a comment: always, except in MySQL, where a space or a control character
/// (or the end of the text) must follow.
fn dash_comment(d: Dialect, b: &[u8], i: usize) -> bool {
    at(b, i) == b'-' && at(b, i + 1) == b'-' && (d != Dialect::MySql || at(b, i + 2) <= b' ' || at(b, i + 2) == 0x7f)
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
pub fn tokenize(d: Dialect, sql: &str) -> Vec<Token> {
    let (pg, my, lite) = (d == Dialect::Postgres, d == Dialect::MySql, d == Dialect::Sqlite);
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
        } else if dash_comment(d, b, i) || (my && c == b'#') {
            while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                i += 1;
            }
            kind = TokenKind::Comment;
        } else if c == b'/' && at(b, i + 1) == b'*' {
            let mut depth = 1usize;
            i += 2;
            while i < b.len() && depth > 0 {
                if pg && b[i] == b'/' && at(b, i + 1) == b'*' {
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
            i = scan_quoted(b, i + 1, b'\'', my);
            kind = TokenKind::String;
        } else if pg && (c == b'E' || c == b'e') && at(b, i + 1) == b'\'' {
            i = scan_quoted(b, i + 2, b'\'', true).min(b.len());
            kind = TokenKind::String;
        } else if pg && (c == b'U' || c == b'u') && at(b, i + 1) == b'&' && (at(b, i + 2) == b'\'' || at(b, i + 2) == b'"') {
            let q = b[i + 2];
            i = scan_quoted(b, i + 3, q, false);
            kind = if q == b'"' { TokenKind::QuotedIdent } else { TokenKind::String };
        } else if at(b, i + 1) == b'\'' && ((my || lite) && matches!(c, b'X' | b'x') || my && matches!(c, b'B' | b'b')) {
            i = scan_quoted(b, i + 2, b'\'', false);
            kind = TokenKind::String;
        } else if c == b'"' {
            i = scan_quoted(b, i + 1, b'"', my);
            kind = if my { TokenKind::String } else { TokenKind::QuotedIdent };
        } else if (my || lite) && c == b'`' {
            i = scan_quoted(b, i + 1, b'`', false);
            kind = TokenKind::QuotedIdent;
        } else if lite && c == b'[' {
            i = b[i..].iter().position(|&x| x == b']').map_or(b.len(), |p| i + p + 1);
            kind = TokenKind::QuotedIdent;
        } else if pg && c == b'$' {
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
        } else if (my || lite) && c == b'0' && matches!(at(b, i + 1), b'x' | b'X') && at(b, i + 2).is_ascii_hexdigit() {
            i += 2;
            while at(b, i).is_ascii_hexdigit() {
                i += 1;
            }
            kind = TokenKind::Number;
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
        } else if is_ident_start(c) || (my && c == b'$') {
            while i < b.len() && is_ident_cont(b[i]) {
                i += 1;
            }
            kind = if d.is_keyword(&sql[start..i]) { TokenKind::Keyword } else { TokenKind::Ident };
        } else if c == b';' {
            i += 1;
            kind = TokenKind::Semicolon;
        } else if is_op_byte(d, c) {
            i += 1;
            while i < b.len() && is_op_byte(d, b[i]) && !dash_comment(d, b, i) && !(b[i] == b'/' && at(b, i + 1) == b'*') {
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

fn word<'a>(sql: &'a str, t: &Token) -> Option<&'a str> {
    if matches!(t.kind, TokenKind::Keyword | TokenKind::Ident) { sql.get(t.range.clone()) } else { None }
}

fn is_word(sql: &str, t: &Token, w: &str) -> bool {
    word(sql, t).is_some_and(|x| x.eq_ignore_ascii_case(w))
}

fn significant(t: &Token) -> bool {
    !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment)
}

/// True when the statement whose first token is `toks[k]` creates a procedure, function, trigger
/// or event (MySQL), or a trigger (SQLite): one whose body may hold `;`.
fn routine_header(d: Dialect, sql: &str, toks: &[Token], k: usize) -> bool {
    let mut sig = toks.get(k..).unwrap_or_default().iter().filter(|t| significant(t)).take(16);
    if !sig.next().is_some_and(|t| is_word(sql, t, "create")) {
        return false;
    }
    if d == Dialect::Sqlite {
        let mut rest = sig.skip_while(|t| is_word(sql, t, "temp") || is_word(sql, t, "temporary"));
        return rest.next().is_some_and(|t| is_word(sql, t, "trigger"));
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
struct Blocks {
    on: bool,
    depth: usize,
    /// A token here starts a statement of the body, where `IF`, `LOOP`, `REPEAT` and `WHILE` open blocks.
    fresh: bool,
    /// The token `END` took along (`END IF`).
    taken: Option<usize>,
    /// A `CREATE PROCEDURE`: the token after its parameter list starts a statement.
    procedure: bool,
    parens: usize,
    params_done: bool,
}

impl Blocks {
    fn step(&mut self, d: Dialect, sql: &str, toks: &[Token], k: usize) {
        let Some(t) = toks.get(k) else { return };
        let me = d == Dialect::MySql;
        if let Some(w) = word(sql, t).filter(|_| self.taken != Some(k)) {
            let is = |x: &str| w.eq_ignore_ascii_case(x);
            let next = toks.get(k + 1..).unwrap_or_default().iter().position(significant).map(|p| k + 1 + p);
            if is("begin") || is("case") {
                self.depth += 1;
            } else if is("end") {
                self.depth = self.depth.saturating_sub(1);
                if me
                    && let Some(n) =
                        next.filter(|&n| toks.get(n).is_some_and(|t| ["if", "case", "loop", "repeat", "while"].iter().any(|x| is_word(sql, t, x))))
                {
                    self.taken = Some(n);
                }
            } else if me && self.fresh {
                let call = sql.as_bytes().get(t.range.end) == Some(&b'(');
                let exists = next.and_then(|n| toks.get(n)).is_some_and(|t| is_word(sql, t, "exists"));
                if is("loop") || is("while") || (is("if") && !call && !exists) || (is("repeat") && !call) {
                    self.depth += 1;
                }
            }
            self.fresh = ["begin", "then", "else", "do", "row", "loop", "repeat"].iter().any(|x| is(x));
            self.procedure |= me && is("procedure") && !self.params_done;
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

fn ws_end(b: &[u8], mut at: usize) -> usize {
    while b.get(at).is_some_and(|&c| c.is_ascii_whitespace() || c == 0x0b) {
        at += 1;
    }
    at
}

fn scan_statements(d: Dialect, sql: &str) -> Vec<Stmt> {
    let toks = tokenize(d, sql);
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut cur: Option<Range<usize>> = None;
    let grow = |cur: &mut Option<Range<usize>>, from: usize, to: usize| match cur {
        Some(r) => r.end = to,
        None => *cur = Some(from..to),
    };
    // MySQL's `DELIMITER` command: the statement end, when it is not `;`.
    let mut delimiter: Option<&str> = None;
    // Bytes before `pos` are done with (a `DELIMITER` line, a delimiter inside a word).
    let mut pos = 0;
    let mut blocks = Blocks::default();
    for (k, t) in toks.iter().enumerate() {
        if t.range.end <= pos || !significant(t) {
            continue;
        }
        let opaque = matches!(t.kind, TokenKind::String | TokenKind::QuotedIdent);
        if opaque && t.range.start < pos {
            continue;
        }
        let from = t.range.start.max(pos);
        if d == Dialect::MySql && cur.is_none() && is_word(sql, t, "delimiter") {
            let line_end = b.get(t.range.end..).and_then(|r| r.iter().position(|&c| c == b'\n' || c == b'\r')).map_or(b.len(), |p| t.range.end + p);
            let line = sql.get(t.range.end..line_end).unwrap_or_default();
            if let Some(new) = line.split_ascii_whitespace().next() {
                delimiter = (new != ";").then_some(new);
            }
            pos = line_end;
            continue;
        }
        if let Some(delim) = delimiter {
            if opaque {
                grow(&mut cur, from, t.range.end);
                continue;
            }
            let (mut p, mut piece) = (from, from);
            while p < t.range.end {
                if b.get(p..).is_some_and(|rest| rest.starts_with(delim.as_bytes())) {
                    if p > piece {
                        grow(&mut cur, piece, p);
                    }
                    if let Some(range) = cur.take() {
                        out.push(Stmt { range, span_end: ws_end(b, p + delim.len()) });
                    }
                    p += delim.len();
                    piece = p;
                } else {
                    p += 1;
                }
            }
            if piece < t.range.end {
                grow(&mut cur, piece, t.range.end);
            }
            pos = piece.max(t.range.end);
            continue;
        }
        if cur.is_none() {
            blocks = Blocks { on: matches!(d, Dialect::MySql | Dialect::Sqlite) && routine_header(d, sql, &toks, k), ..Blocks::default() };
        }
        if t.kind == TokenKind::Semicolon && blocks.depth == 0 {
            if let Some(range) = cur.take() {
                out.push(Stmt { range, span_end: ws_end(b, t.range.end) });
            }
            continue;
        }
        if blocks.on {
            blocks.step(d, sql, &toks, k);
        }
        grow(&mut cur, from, t.range.end);
    }
    if let Some(range) = cur {
        out.push(Stmt { range, span_end: sql.len() });
    }
    out
}

/// Statements split on top-level `;`. Leading whitespace and comments, trailing comments and the
/// `;` are excluded; empty statements are dropped.
pub fn statements(d: Dialect, sql: &str) -> Vec<Range<usize>> {
    scan_statements(d, sql).into_iter().map(|s| s.range).collect()
}

/// The statement whose span (its start through its `;` and the whitespace after it) holds `cursor`.
/// Past the end it is the last statement; `None` when there are no statements.
pub fn statement_at(d: Dialect, sql: &str, cursor: usize) -> Option<Range<usize>> {
    let all = scan_statements(d, sql);
    let pick = all.iter().position(|s| cursor < s.span_end).or(all.len().checked_sub(1))?;
    all.into_iter().nth(pick).map(|s| s.range)
}

/// True when `sql` has a `;` outside strings, comments and quoted identifiers.
pub fn has_top_level_semicolon(d: Dialect, sql: &str) -> bool {
    tokenize(d, sql).iter().any(|t| t.kind == TokenKind::Semicolon)
}
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn kinds(sql: &str) -> Vec<(TokenKind, &str)> {
        kinds_in(Dialect::Postgres, sql)
    }

    fn kinds_in(d: Dialect, sql: &str) -> Vec<(TokenKind, &str)> {
        tokenize(d, sql).into_iter().filter(|t| t.kind != TokenKind::Whitespace).map(|t| (t.kind, &sql[t.range])).collect()
    }

    #[test]
    fn splits_statements() {
        let sql = "select 1; select 'a;b'; -- c;\n select $x$ ; $x$; /* only */ ;";
        let got: Vec<&str> = statements(Dialect::Postgres, sql).into_iter().map(|r| &sql[r]).collect();
        assert_eq!(got, ["select 1", "select 'a;b'", "select $x$ ; $x$"]);
    }

    #[test]
    fn statement_under_cursor() {
        let sql = "select 1;\nselect 2;\n\nselect 3";
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, 0).unwrap()], "select 1");
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, 12).unwrap()], "select 2");
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, sql.len()).unwrap()], "select 3");
        assert_eq!(statement_at(Dialect::Postgres, "", 0), None);
        assert_eq!(statement_at(Dialect::Postgres, " ; -- x", 3), None);
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, 9).unwrap()], "select 1");
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, 10).unwrap()], "select 2");
        assert_eq!(&sql[statement_at(Dialect::Postgres, sql, 99999).unwrap()], "select 3");
    }

    #[test]
    fn semicolons_inside_strings_do_not_count() {
        assert!(!has_top_level_semicolon(Dialect::Postgres, "status = 'a;b'"));
        assert!(has_top_level_semicolon(Dialect::Postgres, "x > 5; drop table t"));
        assert!(!has_top_level_semicolon(Dialect::Postgres, "x /* ; */ -- ;"));
        assert!(!has_top_level_semicolon(Dialect::Postgres, "select \"a;b\", $$;$$, E'\\';'"));
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
    fn line_comment_ends_at_bare_cr() {
        use TokenKind::*;
        let sql = "x -- a\r; drop table t";
        assert_eq!(kinds(sql)[1], (Comment, "-- a"));
        assert!(has_top_level_semicolon(Dialect::Postgres, sql));
        let got: Vec<&str> = statements(Dialect::Postgres, sql).into_iter().map(|r| &sql[r]).collect();
        assert_eq!(got, ["x", "drop table t"]);
    }

    #[test]
    fn unterminated_runs_to_end() {
        use TokenKind::*;
        assert_eq!(kinds("'abc"), [(String, "'abc")]);
        assert_eq!(kinds("/* abc"), [(Comment, "/* abc")]);
        assert_eq!(kinds("$t$ abc $"), [(String, "$t$ abc $")]);
        assert_eq!(kinds("\"abc"), [(QuotedIdent, "\"abc")]);
        assert_eq!(kinds("E'abc\\"), [(String, "E'abc\\")]);
        assert_eq!(statements(Dialect::Postgres, "select 'a;b"), vec![0..11]);
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
            let t = tokenize(Dialect::Postgres, s);
            assert_eq!(t.last().map(|k| k.range.end).unwrap_or(0), s.len(), "{s:?}");
            let _ = statements(Dialect::Postgres, s);
            let _ = statement_at(Dialect::Postgres, s, 1);
        }
    }

    #[test]
    fn keywords_are_sorted_and_lowercase() {
        assert!(Dialect::Postgres.keywords().windows(2).all(|w| w[0] < w[1]), "KEYWORDS must be sorted and unique");
        assert!(Dialect::Postgres.keywords().iter().all(|k| k.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')));
        for k in ["select", "from", "where", "join", "insert", "update", "delete", "returning", "limit"] {
            assert!(Dialect::Postgres.keywords().binary_search(&k).is_ok(), "{k}");
        }
    }

    proptest! {
        #[test]
        fn tokens_cover_input(s in "\\PC{0,300}") {
            let t = tokenize(Dialect::Postgres, &s);
            let mut at = 0;
            for k in &t { prop_assert_eq!(k.range.start, at); prop_assert!(k.range.end > k.range.start); at = k.range.end; }
            prop_assert_eq!(at, s.len());
        }
        #[test]
        fn statements_are_in_bounds(s in "\\PC{0,300}") {
            for r in statements(Dialect::Postgres, &s) { prop_assert!(s.get(r).is_some()); }
        }
        #[test]
        fn sqlish_input_covers_and_splits(s in "[a-z0-9 ;'\"$E&U\\\\/*\\n\\r-]{0,200}") {
            let t = tokenize(Dialect::Postgres, &s);
            prop_assert_eq!(t.last().map(|k| k.range.end).unwrap_or(0), s.len());
            let st = statements(Dialect::Postgres, &s);
            for w in st.windows(2) { prop_assert!(w[0].end <= w[1].start); }
            for r in &st { prop_assert!(s.get(r.clone()).is_some()); }
            for c in 0..=s.len() { prop_assert_eq!(statement_at(Dialect::Postgres, &s, c).is_some(), !st.is_empty()); }
        }
    }

    fn pieces(d: Dialect, sql: &str) -> Vec<&str> {
        statements(d, sql).into_iter().map(|r| sql[r].trim()).collect()
    }

    fn dialects() -> impl Strategy<Value = Dialect> {
        prop_oneof![Just(Dialect::Postgres), Just(Dialect::MySql), Just(Dialect::Sqlite)]
    }

    #[test]
    fn mysql_delimiter_blocks() {
        let sql = "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END$$\nDELIMITER ;\nSELECT 3;";
        assert_eq!(pieces(Dialect::MySql, sql), ["CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END", "SELECT 3"]);
    }

    #[test]
    fn mysql_delimiter_rules() {
        let my = Dialect::MySql;
        assert_eq!(
            pieces(my, "delimiter //\nselect 1; select 2//\nselect 3// delimiter ;\nselect 4;"),
            ["select 1; select 2", "select 3", "select 4"]
        );
        assert_eq!(pieces(my, "DELIMITER\nselect 1; select 2"), ["select 1", "select 2"]);
        assert_eq!(pieces(my, "DELIMITER $$\nselect '$$', `a$$`, \"$$\" /* $$ */ $$ select 2;"), ["select '$$', `a$$`, \"$$\"", "select 2;"]);
        assert_eq!(pieces(my, "DELIMITER ;;\nselect 1;; select 2;;"), ["select 1", "select 2"]);
        assert_eq!(
            pieces(my, "DELIMITER $$\nCREATE PROCEDURE p() BEGIN IF a THEN SELECT 1; END IF; END $$"),
            ["CREATE PROCEDURE p() BEGIN IF a THEN SELECT 1; END IF; END"]
        );
        let sql = "select 1;\nDELIMITER $$\nselect 2$$";
        assert_eq!(pieces(my, sql), ["select 1", "select 2"]);
        assert_eq!(&sql[statement_at(my, sql, 12).unwrap()], "select 2");
        assert_eq!(pieces(Dialect::Postgres, "DELIMITER $$\nselect 1"), ["DELIMITER $$\nselect 1"]);
    }

    #[test]
    fn mysql_begin_end_without_delimiter() {
        let my = Dialect::MySql;
        let sql = "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN SET NEW.a = 1; END; SELECT 1;";
        assert_eq!(pieces(my, sql), ["CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN SET NEW.a = 1; END", "SELECT 1"]);
        let nested = "CREATE DEFINER=`root`@`localhost` PROCEDURE p(a INT) BEGIN \
            DECLARE x INT DEFAULT 0; \
            IF a > 1 THEN SET x = CASE WHEN a = 2 THEN 1 ELSE 2 END; ELSEIF a THEN SET x = IF(a, 1, 2); ELSE SET x = 0; END IF; \
            l: LOOP SET x = x + 1; IF x > 3 THEN LEAVE l; END IF; END LOOP l; \
            WHILE x < 9 DO SET x = x + 1; END WHILE; \
            REPEAT SET x = x + 1; UNTIL x > 20 END REPEAT; \
            CASE x WHEN 1 THEN SELECT 1; ELSE SELECT 2; END CASE; \
            BEGIN SELECT 3; END; \
            END; SELECT 9;";
        assert_eq!(pieces(my, nested).len(), 2, "{:?}", pieces(my, nested));
        assert_eq!(
            pieces(my, "CREATE OR REPLACE AGGREGATE FUNCTION f() BEGIN SELECT 1; END; SELECT 2"),
            ["CREATE OR REPLACE AGGREGATE FUNCTION f() BEGIN SELECT 1; END", "SELECT 2"]
        );
        assert_eq!(
            pieces(my, "CREATE DEFINER = CURRENT_USER() EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN SELECT 1; END; SELECT 2"),
            ["CREATE DEFINER = CURRENT_USER() EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN SELECT 1; END", "SELECT 2"]
        );
        assert_eq!(pieces(my, "CREATE PROCEDURE p() SELECT 1; SELECT 2"), ["CREATE PROCEDURE p() SELECT 1", "SELECT 2"]);
        assert_eq!(pieces(my, "DROP TABLE IF EXISTS t; SELECT IF(1, 2, 3); SELECT 2"), ["DROP TABLE IF EXISTS t", "SELECT IF(1, 2, 3)", "SELECT 2"]);
        assert_eq!(pieces(my, "CREATE TABLE t (a int); BEGIN; SELECT 1; COMMIT;"), ["CREATE TABLE t (a int)", "BEGIN", "SELECT 1", "COMMIT"]);
        assert_eq!(pieces(my, "CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2;"), ["CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2;"]);
        assert_eq!(pieces(my, "CREATE PROCEDURE p() BEGIN END END END; SELECT 1"), ["CREATE PROCEDURE p() BEGIN END END END", "SELECT 1"]);
        assert_eq!(
            pieces(Dialect::Postgres, "CREATE TRIGGER t BEGIN SELECT 1; END; SELECT 2"),
            ["CREATE TRIGGER t BEGIN SELECT 1", "END", "SELECT 2"]
        );
    }

    #[test]
    fn mysql_blocks_inside_loop_and_repeat() {
        let my = Dialect::MySql;
        for sql in [
            "CREATE PROCEDURE p() BEGIN l: LOOP\nIF x THEN LEAVE l; END IF; END LOOP l; SELECT 2; END; SELECT 1",
            "CREATE PROCEDURE p() BEGIN REPEAT IF x THEN SELECT 3; END IF; UNTIL y END REPEAT; SELECT 2; END; SELECT 1",
            "CREATE PROCEDURE p() BEGIN REPEAT\nIF x THEN SELECT 3; END IF; UNTIL y END REPEAT; SELECT 2; END; SELECT 1",
        ] {
            assert_eq!(pieces(my, sql).len(), 2, "{sql}");
        }
    }

    #[test]
    fn mysql_procedure_body_without_begin() {
        let my = Dialect::MySql;
        assert_eq!(
            pieces(my, "CREATE PROCEDURE p(a DECIMAL(10, 2)) IF a THEN SELECT 1; END IF; SELECT 2"),
            ["CREATE PROCEDURE p(a DECIMAL(10, 2)) IF a THEN SELECT 1; END IF", "SELECT 2"]
        );
        assert_eq!(pieces(my, "CREATE DEFINER = CURRENT_USER() PROCEDURE p() WHILE x DO SELECT 1; END WHILE; SELECT 2").len(), 2);
    }

    #[test]
    fn mysql_hash_comment_and_backslash_string() {
        let sql = "SELECT 'a\\'b;' # c;\n; SELECT 2";
        assert_eq!(pieces(Dialect::MySql, sql), ["SELECT 'a\\'b;'", "SELECT 2"]);
        use TokenKind::*;
        assert_eq!(kinds_in(Dialect::MySql, "a # x\nb"), [(Ident, "a"), (Comment, "# x"), (Ident, "b")]);
        assert_eq!(kinds_in(Dialect::Postgres, "a # x"), [(Ident, "a"), (Operator, "#"), (Ident, "x")]);
        assert_eq!(kinds_in(Dialect::MySql, "\"a\\\"b;\" 'c''d'"), [(String, "\"a\\\"b;\""), (String, "'c''d'")]);
        assert_eq!(kinds_in(Dialect::Postgres, "'a\\' b"), [(String, "'a\\'"), (Ident, "b")]);
        assert_eq!(kinds_in(Dialect::Sqlite, "'a\\' b"), [(String, "'a\\'"), (Ident, "b")]);
    }

    #[test]
    fn mysql_dash_needs_space() {
        use TokenKind::*;
        let sql = "SELECT 1--1;";
        assert!(kinds_in(Dialect::MySql, sql).iter().all(|(k, _)| *k != Comment));
        assert_eq!(pieces(Dialect::MySql, sql), ["SELECT 1--1"]);
        assert_eq!(kinds_in(Dialect::MySql, "a -- x\nb"), [(Ident, "a"), (Comment, "-- x"), (Ident, "b")]);
        assert_eq!(kinds_in(Dialect::MySql, "a --\tx"), [(Ident, "a"), (Comment, "--\tx")]);
        assert_eq!(kinds_in(Dialect::MySql, "a --"), [(Ident, "a"), (Comment, "--")]);
        assert_eq!(kinds_in(Dialect::MySql, "a --x"), [(Ident, "a"), (Operator, "--"), (Ident, "x")]);
        assert_eq!(kinds_in(Dialect::MySql, "a +-- b"), [(Ident, "a"), (Operator, "+"), (Comment, "-- b")]);
        assert_eq!(kinds_in(Dialect::Sqlite, "a --x"), [(Ident, "a"), (Comment, "--x")]);
    }

    #[test]
    fn mysql_literals_and_identifiers() {
        use TokenKind::*;
        let my = Dialect::MySql;
        assert_eq!(kinds_in(my, "`a``;b` x"), [(QuotedIdent, "`a``;b`"), (Ident, "x")]);
        assert_eq!(kinds_in(my, "0xCAFE x'ab;' b'01' 0x"), [(Number, "0xCAFE"), (String, "x'ab;'"), (String, "b'01'"), (Number, "0"), (Ident, "x")]);
        assert_eq!(kinds_in(my, "END$$"), [(Ident, "END$$")]);
        assert_eq!(kinds_in(my, "/* a /* b */ c"), [(Comment, "/* a /* b */"), (Ident, "c")]);
        assert_eq!(kinds_in(Dialect::Postgres, "/* a /* b */ c */"), [(Comment, "/* a /* b */ c */")]);
        assert!(!has_top_level_semicolon(my, "select `;`, '\\';', \"\\\";\" # ;"));
    }

    #[test]
    fn sqlite_trigger_body() {
        let lite = Dialect::Sqlite;
        let sql = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET n = n + 1; END; SELECT 1;";
        assert_eq!(pieces(lite, sql), ["CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET n = n + 1; END", "SELECT 1"]);
        let case = "CREATE TEMP TRIGGER IF NOT EXISTS t AFTER INSERT ON a WHEN CASE WHEN 1 THEN 1 END BEGIN UPDATE b SET n = CASE WHEN x THEN 1 ELSE 2 END; DELETE FROM c; END; SELECT 1";
        assert_eq!(pieces(lite, case).len(), 2);
        assert_eq!(pieces(lite, "BEGIN; SELECT 1; END; SELECT 2"), ["BEGIN", "SELECT 1", "END", "SELECT 2"]);
        assert_eq!(
            pieces(lite, "CREATE TABLE t (a); CREATE TRIGGER x AFTER DELETE ON t BEGIN SELECT 1; END"),
            ["CREATE TABLE t (a)", "CREATE TRIGGER x AFTER DELETE ON t BEGIN SELECT 1; END"]
        );
        assert_eq!(
            pieces(lite, "CREATE TRIGGER t AFTER INSERT ON a BEGIN SELECT 1; SELECT 2;"),
            ["CREATE TRIGGER t AFTER INSERT ON a BEGIN SELECT 1; SELECT 2;"]
        );
    }

    #[test]
    fn sqlite_bracket_identifiers() {
        use TokenKind::*;
        let lite = Dialect::Sqlite;
        assert_eq!(kinds_in(lite, "[a;b]"), [(QuotedIdent, "[a;b]")]);
        assert_eq!(kinds_in(lite, "[a\"]]"), [(QuotedIdent, "[a\"]"), (Other, "]")]);
        assert_eq!(kinds_in(lite, "[abc"), [(QuotedIdent, "[abc")]);
        assert_eq!(kinds_in(lite, "`a``;b` X'0A;' x"), [(QuotedIdent, "`a``;b`"), (String, "X'0A;'"), (Ident, "x")]);
        assert_eq!(
            kinds_in(lite, "\"a;b\" 0xFF /* a /* b */ c"),
            [(QuotedIdent, "\"a;b\""), (Number, "0xFF"), (Comment, "/* a /* b */"), (Ident, "c")]
        );
        assert_eq!(kinds_in(lite, "a # b"), [(Ident, "a"), (Operator, "#"), (Ident, "b")]);
        assert_eq!(kinds_in(Dialect::Postgres, "a[1]"), [(Ident, "a"), (Other, "["), (Number, "1"), (Other, "]")]);
        assert_eq!(pieces(lite, "select [a;b]; select 2"), ["select [a;b]", "select 2"]);
    }

    #[test]
    fn keywords_per_dialect() {
        use TokenKind::*;
        assert_eq!(kinds_in(Dialect::MySql, "select accessible pragma"), [(Keyword, "select"), (Keyword, "accessible"), (Ident, "pragma")]);
        assert_eq!(kinds_in(Dialect::Sqlite, "select pragma accessible"), [(Keyword, "select"), (Keyword, "pragma"), (Ident, "accessible")]);
        assert_eq!(kinds_in(Dialect::MySql, "MASTER_SSL_VERIFY_SERVER_CERT"), [(Keyword, "MASTER_SSL_VERIFY_SERVER_CERT")]);
        assert_eq!(kinds_in(Dialect::Postgres, "accessible"), [(Ident, "accessible")]);
    }

    proptest! {
        #[test]
        fn never_panics_and_ranges_are_in_bounds(d in dialects(), s in ".{0,400}") {
            for r in statements(d, &s) { prop_assert!(r.end <= s.len() && s.is_char_boundary(r.start) && s.is_char_boundary(r.end)); }
            let mut at = 0;
            for k in tokenize(d, &s) { prop_assert_eq!(k.range.start, at); at = k.range.end; prop_assert!(s.is_char_boundary(at)); }
            prop_assert_eq!(at, s.len());
        }
        #[test]
        fn sqlish_input_never_panics_in_any_dialect(d in dialects(), s in "([a-zA-Z0-9 ;'\"`\\[\\]#$:=()@\\\\/*\\n\\r-]|DELIMITER |BEGIN |END |CASE |IF |LOOP |CREATE |TRIGGER |PROCEDURE |\\$\\$){0,80}") {
            let st = statements(d, &s);
            for w in st.windows(2) { prop_assert!(w[0].end <= w[1].start); }
            for r in &st { prop_assert!(s.get(r.clone()).is_some()); }
            for c in 0..=s.len() { prop_assert_eq!(statement_at(d, &s, c).is_some(), !st.is_empty()); }
        }
    }
}

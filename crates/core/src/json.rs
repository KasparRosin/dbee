//! JSON helpers for the value panel: classify, validate, pretty-print and highlight.
//!
//! One iterative tokenizer with an explicit container stack serves all of them, so hostile nesting depth never
//! recurses. Number text and key order are never touched.

use std::ops::Range;

use crate::cell::{Column, TypeClass};

/// Largest input or output `pretty` handles.
pub const MAX_PRETTY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonKind {
    Json,
    TextContainingJson,
    JsonStringContainingJson,
    NotJson,
}

impl JsonKind {
    pub fn label(self) -> &'static str {
        match self {
            JsonKind::Json => "json",
            JsonKind::TextContainingJson => "text containing JSON",
            JsonKind::JsonStringContainingJson => "JSON string containing JSON",
            JsonKind::NotJson => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonToken {
    Key,
    String,
    Number,
    Literal,
    Punct,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Open,
    Close,
    Colon,
    Comma,
    Key,
    Str,
    Num,
    Lit,
}

#[derive(Clone, Copy)]
enum St {
    Value,
    ArrFirst,
    ObjFirst,
    ObjKey,
    Colon,
    After,
    Done,
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// End index of the string starting at `b[i] == '"'`, or None when malformed.
fn scan_string(b: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'"' => return Some(i + 1),
            b'\\' => match b.get(i + 1)? {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 2,
                b'u' => {
                    let hex = b.get(i + 2..i + 6)?;
                    if !hex.iter().all(u8::is_ascii_hexdigit) {
                        return None;
                    }
                    i += 6;
                }
                _ => return None,
            },
            0..=0x1f => return None,
            _ => i += 1,
        }
    }
    None
}

fn digits(b: &[u8], mut i: usize) -> Option<usize> {
    let start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    (i > start).then_some(i)
}

fn scan_number(b: &[u8], mut i: usize) -> Option<usize> {
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    match b.get(i)? {
        b'0' => i += 1,
        b'1'..=b'9' => i = digits(b, i)?,
        _ => return None,
    }
    if b.get(i) == Some(&b'.') {
        i = digits(b, i + 1)?;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        i = digits(b, i)?;
    }
    Some(i)
}

/// A string, number or literal starting at `i`: its end and token.
fn scan_scalar(b: &[u8], i: usize) -> Option<(usize, Tok)> {
    match b.get(i)? {
        b'"' => scan_string(b, i).map(|e| (e, Tok::Str)),
        b'-' | b'0'..=b'9' => scan_number(b, i).map(|e| (e, Tok::Num)),
        b't' => b[i..].starts_with(b"true").then_some((i + 4, Tok::Lit)),
        b'f' => b[i..].starts_with(b"false").then_some((i + 5, Tok::Lit)),
        b'n' => b[i..].starts_with(b"null").then_some((i + 4, Tok::Lit)),
        _ => None,
    }
}

/// Tokenizes `s` (RFC 8259, one top-level value). The sink gets each token in order and returns false to stop;
/// the result is true only when the whole text is valid and the sink never stopped it.
fn scan(s: &str, sink: &mut dyn FnMut(Tok, Range<usize>) -> bool) -> bool {
    let b = s.as_bytes();
    let mut stack: Vec<bool> = Vec::new(); // true = object
    let mut st = St::Value;
    let mut i = 0;
    loop {
        while b.get(i).copied().is_some_and(is_ws) {
            i += 1;
        }
        let Some(&c) = b.get(i) else { break };
        let after_value = |stack: &Vec<bool>| if stack.is_empty() { St::Done } else { St::After };
        match st {
            St::Done => return false,
            St::ObjKey | St::ObjFirst if c == b'}' && matches!(st, St::ObjFirst) => {
                stack.pop();
                if !sink(Tok::Close, i..i + 1) {
                    return false;
                }
                i += 1;
                st = after_value(&stack);
            }
            St::ObjKey | St::ObjFirst => {
                if c != b'"' {
                    return false;
                }
                let Some(e) = scan_string(b, i) else { return false };
                if !sink(Tok::Key, i..e) {
                    return false;
                }
                i = e;
                st = St::Colon;
            }
            St::Colon => {
                if c != b':' || !sink(Tok::Colon, i..i + 1) {
                    return false;
                }
                i += 1;
                st = St::Value;
            }
            St::ArrFirst if c == b']' => {
                stack.pop();
                if !sink(Tok::Close, i..i + 1) {
                    return false;
                }
                i += 1;
                st = after_value(&stack);
            }
            St::Value | St::ArrFirst => match c {
                b'{' | b'[' => {
                    stack.push(c == b'{');
                    if !sink(Tok::Open, i..i + 1) {
                        return false;
                    }
                    i += 1;
                    st = if c == b'{' { St::ObjFirst } else { St::ArrFirst };
                }
                _ => {
                    let Some((e, t)) = scan_scalar(b, i) else { return false };
                    if !sink(t, i..e) {
                        return false;
                    }
                    i = e;
                    st = after_value(&stack);
                }
            },
            St::After => {
                let in_object = stack.last().copied().unwrap_or(false);
                match c {
                    b',' => {
                        if !sink(Tok::Comma, i..i + 1) {
                            return false;
                        }
                        st = if in_object { St::ObjKey } else { St::Value };
                    }
                    b'}' if in_object => {
                        stack.pop();
                        if !sink(Tok::Close, i..i + 1) {
                            return false;
                        }
                        st = after_value(&stack);
                    }
                    b']' if !in_object => {
                        stack.pop();
                        if !sink(Tok::Close, i..i + 1) {
                            return false;
                        }
                        st = after_value(&stack);
                    }
                    _ => return false,
                }
                i += 1;
            }
        }
    }
    matches!(st, St::Done)
}

/// True when `text` is one valid JSON value (RFC 8259). Iterative.
pub fn is_valid(text: &str) -> bool {
    scan(text, &mut |_, _| true)
}

/// Pretty-prints with a 2-space indent, keeping key order and number text. `None` when invalid or when the input or
/// output would pass `MAX_PRETTY_BYTES`.
pub fn pretty(text: &str) -> Option<String> {
    if text.len() > MAX_PRETTY_BYTES {
        return None;
    }
    let mut out = String::with_capacity(text.len().saturating_add(text.len() / 4));
    let mut depth = 0usize;
    let mut pending_open = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', depth.saturating_mul(2)));
    };
    let ok = scan(text, &mut |tok, r| {
        if pending_open && tok != Tok::Close {
            newline(&mut out, depth);
            pending_open = false;
        }
        match tok {
            Tok::Open => {
                out.push_str(&text[r]);
                depth += 1;
                pending_open = true;
            }
            Tok::Close => {
                depth = depth.saturating_sub(1);
                if pending_open {
                    pending_open = false;
                } else {
                    newline(&mut out, depth);
                }
                out.push_str(&text[r]);
            }
            Tok::Comma => {
                out.push(',');
                newline(&mut out, depth);
            }
            Tok::Colon => out.push_str(": "),
            Tok::Key | Tok::Str | Tok::Num | Tok::Lit => out.push_str(&text[r]),
        }
        out.len() <= MAX_PRETTY_BYTES
    });
    ok.then_some(out)
}

/// For a JSON string value, its decoded content. `None` for other values, invalid text, or a lone surrogate escape.
pub fn decoded_inner(text: &str) -> Option<String> {
    let mut range = None;
    if !scan(text, &mut |tok, r| {
        if tok == Tok::Str {
            range = Some(r);
        }
        true
    }) {
        return None;
    }
    let r = range?;
    // A top-level string is the only token when it is the whole value.
    if text.trim_matches(is_json_ws) != &text[r.clone()] {
        return None;
    }
    let inner = text.get(r.start + 1..r.end - 1)?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let hi = hex4(&mut chars)?;
                let cp = match hi {
                    0xD800..=0xDBFF => {
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let lo = hex4(&mut chars)?;
                        if !(0xDC00..=0xDFFF).contains(&lo) {
                            return None;
                        }
                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                    }
                    0xDC00..=0xDFFF => return None,
                    _ => hi,
                };
                out.push(char::from_u32(cp)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn is_json_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn hex4(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        v = v * 16 + chars.next()?.to_digit(16)?;
    }
    Some(v)
}

/// The kind of a column before its value is looked at: a `Json`-class column is JSON, whatever
/// the engine calls its type. Any other class is `NotJson`.
pub fn column_kind(column: &Column) -> JsonKind {
    match column.class {
        TypeClass::Json => JsonKind::Json,
        _ => JsonKind::NotJson,
    }
}

/// What kind of JSON `text` is, for `column`.
pub fn classify(column: &Column, text: &str) -> JsonKind {
    let looks_like_container = |t: &str| t.trim_matches(is_json_ws).starts_with(['{', '[']);
    match column.class {
        TypeClass::Json => {
            let nested = decoded_inner(text).is_some_and(|inner| looks_like_container(&inner) && is_valid(&inner));
            if nested { JsonKind::JsonStringContainingJson } else { column_kind(column) }
        }
        TypeClass::Text | TypeClass::LongText if looks_like_container(text) && is_valid(text) => JsonKind::TextContainingJson,
        _ => JsonKind::NotJson,
    }
}

/// Token ranges (byte offsets into `pretty`) for syntax colouring. Stops at the first invalid spot.
pub fn highlight(pretty: &str) -> Vec<(Range<usize>, JsonToken)> {
    let mut out = Vec::new();
    scan(pretty, &mut |tok, r| {
        let kind = match tok {
            Tok::Key => JsonToken::Key,
            Tok::Str => JsonToken::String,
            Tok::Num => JsonToken::Number,
            Tok::Lit => JsonToken::Literal,
            Tok::Open | Tok::Close | Tok::Colon | Tok::Comma => JsonToken::Punct,
        };
        out.push((r, kind));
        true
    });
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn pretty_keeps_order_and_number_text() {
        let src = r#"{"b":1,"a":[1,2,{"c":null}],"e":{},"n":12345678901234567890.5,"s":"x\"y"}"#;
        assert_eq!(
            pretty(src).unwrap(),
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2,\n    {\n      \"c\": null\n    }\n  ],\n  \"e\": {},\n  \"n\": 12345678901234567890.5,\n  \"s\": \"x\\\"y\"\n}"
        );
    }

    fn typed(base: &str, class: TypeClass) -> Column {
        Column { name: "c".into(), base_type: base.into(), type_label: base.into(), class }
    }

    #[test]
    fn classify_labels() {
        let (jsonb, json, text, varchar, int4) = (
            typed("jsonb", TypeClass::Json),
            typed("json", TypeClass::Json),
            typed("text", TypeClass::LongText),
            typed("varchar", TypeClass::Text),
            typed("int4", TypeClass::Number),
        );
        assert_eq!(classify(&jsonb, r#"{"a":1}"#), JsonKind::Json);
        assert_eq!(classify(&jsonb, r#""{\"a\":1}""#), JsonKind::JsonStringContainingJson);
        assert_eq!(classify(&json, r#""just a string""#), JsonKind::Json);
        assert_eq!(classify(&text, r#"  {"a":1} "#), JsonKind::TextContainingJson);
        assert_eq!(classify(&varchar, "[1,2]"), JsonKind::TextContainingJson);
        assert_eq!(classify(&text, "{not json"), JsonKind::NotJson);
        assert_eq!(classify(&text, "123"), JsonKind::NotJson);
        assert_eq!(classify(&int4, "1"), JsonKind::NotJson);
        assert_eq!(column_kind(&jsonb), JsonKind::Json, "whatever the engine calls the type");
        assert_eq!(column_kind(&json), JsonKind::Json);
        assert_eq!(column_kind(&text), JsonKind::NotJson);
        // Another engine's JSON column is plain JSON.
        assert_eq!(column_kind(&typed("", TypeClass::Json)), JsonKind::Json);
    }

    #[test]
    fn labels() {
        assert_eq!(JsonKind::TextContainingJson.label(), "text containing JSON");
        assert_eq!(JsonKind::NotJson.label(), "");
    }

    #[test]
    fn validity_edge_cases() {
        for ok in ["0", "-0.5e+10", "\"\"", " [ ] ", "{\"a\" : [ true , false , null ]}", "\"\\ud83d\\ude00\""] {
            assert!(is_valid(ok), "{ok}");
        }
        for bad in [
            "",
            "01",
            "1.",
            ".5",
            "+1",
            "[1,]",
            "{\"a\":1,}",
            "{a:1}",
            "[1 2]",
            "1 2",
            "\"a\nb\"",
            "\"\\x\"",
            "nul",
            "[",
            "]",
            "{\"a\"}",
            "\u{feff}1",
        ] {
            assert!(!is_valid(bad), "{bad:?}");
        }
    }

    #[test]
    fn decoded_inner_decodes() {
        assert_eq!(decoded_inner(r#" "a\n\u00e9\ud83d\ude00" "#).unwrap(), "a\n\u{e9}\u{1F600}");
        assert_eq!(decoded_inner(r#"{"a":"b"}"#), None);
        assert_eq!(decoded_inner(r#""\ud800""#), None);
        assert_eq!(decoded_inner("1"), None);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let d = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
        assert!(is_valid(&d));
        let _ = pretty(&d);
        let _ = highlight(&d);
        let _ = classify(&typed("jsonb", TypeClass::Json), &d);
    }

    #[test]
    fn pretty_gives_up_above_the_cap() {
        let big = format!("[\"{}\"]", "x".repeat(MAX_PRETTY_BYTES + 1));
        assert!(pretty(&big).is_none());
    }

    #[test]
    fn highlight_marks_keys_and_values() {
        let p = pretty(r#"{"k":"v","n":1,"t":true}"#).unwrap();
        let kinds: Vec<_> = highlight(&p).into_iter().map(|(_, k)| k).filter(|k| *k != JsonToken::Punct).collect();
        assert_eq!(kinds, [JsonToken::Key, JsonToken::String, JsonToken::Key, JsonToken::Number, JsonToken::Key, JsonToken::Literal]);
    }

    fn arb_json() -> impl Strategy<Value = String> {
        let quoted = |s: String| serde_json::to_string(&s).unwrap_or_default();
        let leaf = prop_oneof![
            Just("null".to_string()),
            Just("true".to_string()),
            Just("false".to_string()),
            any::<i64>().prop_map(|n| n.to_string()),
            (any::<i32>(), 0u8..99).prop_map(|(a, b)| format!("{a}.{b:02}e-3")),
            ".{0,8}".prop_map(quoted),
        ];
        leaf.prop_recursive(4, 48, 4, move |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(|v| format!("[{}]", v.join(","))),
                prop::collection::vec((".{0,6}".prop_map(quoted), inner), 0..4).prop_map(|v| {
                    let items: Vec<_> = v.into_iter().map(|(k, x)| format!("{k}:{x}")).collect();
                    format!("{{{}}}", items.join(","))
                }),
            ]
        })
    }

    proptest! {
        #[test]
        fn never_panics(s in ".{0,200}") {
            let _ = classify(&typed("text", TypeClass::LongText), &s);
            let _ = classify(&typed("jsonb", TypeClass::Json), &s);
            let _ = pretty(&s);
            let _ = decoded_inner(&s);
            let _ = highlight(&s);
        }

        #[test]
        fn pretty_is_idempotent_and_valid(v in arb_json()) {
            let p = pretty(&v).unwrap();
            prop_assert!(is_valid(&p));
            prop_assert_eq!(pretty(&p).unwrap(), p);
        }
    }
}

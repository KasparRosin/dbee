//! Clipboard formats: TSV, CSV, JSON, Markdown.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cell::Column;
use crate::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CopyFormat {
    Tsv,
    TsvWithHeader,
    Csv,
    Json,
    Markdown,
}

impl CopyFormat {
    pub fn from_id(id: &str) -> Option<CopyFormat> {
        Some(match id {
            "tsv" => CopyFormat::Tsv,
            "tsvHeader" => CopyFormat::TsvWithHeader,
            "csv" => CopyFormat::Csv,
            "json" => CopyFormat::Json,
            "markdown" => CopyFormat::Markdown,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "tsv",
            CopyFormat::TsvWithHeader => "tsvHeader",
            CopyFormat::Csv => "csv",
            CopyFormat::Json => "json",
            CopyFormat::Markdown => "markdown",
        }
    }
}

/// Renders rows in `format`. Lines are joined with `\n`, with no trailing newline.
///
/// Everything is written into one string, sized up front from the cells, so a large selection is
/// copied about once: each field is searched for the characters that need quoting or escaping and
/// appended, never built as a string of its own.
pub fn render(format: CopyFormat, columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
    let mut out = String::with_capacity(capacity(columns, rows));
    match format {
        CopyFormat::Tsv => delimited(&mut out, columns, rows, false, push_tsv, '\t'),
        CopyFormat::TsvWithHeader => delimited(&mut out, columns, rows, true, push_tsv, '\t'),
        CopyFormat::Csv => delimited(&mut out, columns, rows, true, push_csv, ','),
        CopyFormat::Json => json(&mut out, columns, rows),
        CopyFormat::Markdown => markdown(&mut out, columns, rows),
    }
    out
}

/// The most [`render`] reserves up front; a larger copy grows as it goes.
const MAX_RESERVE: usize = 64 * 1024 * 1024;

/// A guess at the rendered length: the cells, and room for separators and quotes.
fn capacity(columns: &[Column], rows: &[Vec<Option<&str>>]) -> usize {
    let names = columns.iter().fold(0usize, |n, c| n.saturating_add(c.name.len()).saturating_add(3));
    let cells = rows.iter().flatten().fold(0usize, |n, v| n.saturating_add(v.map_or(4, str::len)).saturating_add(3));
    names.saturating_add(cells).min(MAX_RESERVE)
}

fn delimited(out: &mut String, columns: &[Column], rows: &[Vec<Option<&str>>], header: bool, field: fn(&mut String, &str), sep: char) {
    let mut first = true;
    let mut line = |out: &mut String, fields: &mut dyn Iterator<Item = Option<&str>>| {
        if !std::mem::take(&mut first) {
            out.push('\n');
        }
        for (i, v) in fields.enumerate() {
            if i > 0 {
                out.push(sep);
            }
            if let Some(v) = v {
                field(out, v);
            }
        }
    };
    if header {
        line(out, &mut columns.iter().map(|c| Some(c.name.as_str())));
    }
    for row in rows {
        line(out, &mut row.iter().copied());
    }
}

/// Whether `s` holds any of `chars`. One fast search per character beats one pass that compares
/// each character with them all.
fn has_any(s: &str, chars: &[char]) -> bool {
    chars.iter().any(|&c| s.contains(c))
}

fn push_tsv(out: &mut String, s: &str) {
    if s.starts_with('"') || has_any(s, &['\t', '\n', '\r']) { push_quoted(out, s) } else { out.push_str(s) }
}

fn push_csv(out: &mut String, s: &str) {
    if has_any(s, &[',', '"', '\n', '\r']) { push_quoted(out, s) } else { out.push_str(s) }
}

/// `s` in double quotes, each `"` in it doubled.
fn push_quoted(out: &mut String, s: &str) {
    out.push('"');
    for (i, part) in s.split('"').enumerate() {
        if i > 0 {
            out.push_str("\"\"");
        }
        out.push_str(part);
    }
    out.push('"');
}

fn json(out: &mut String, columns: &[Column], rows: &[Vec<Option<&str>>]) {
    if rows.is_empty() {
        out.push_str("[]");
        return;
    }
    // Each name as a JSON string, once.
    let names: Vec<String> = columns.iter().map(|c| json_string(&c.name)).collect();
    out.push_str("[\n");
    for (r, row) in rows.iter().enumerate() {
        if r > 0 {
            out.push_str(",\n");
        }
        out.push_str("  {");
        for (i, (c, name)) in columns.iter().zip(&names).enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(name);
            out.push_str(": ");
            push_json_value(out, &c.base_type, row.get(i).copied().flatten());
        }
        out.push('}');
    }
    out.push_str("\n]");
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn push_json_value(out: &mut String, base_type: &str, v: Option<&str>) {
    let Some(text) = v else {
        out.push_str("null");
        return;
    };
    match base_type {
        "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" if is_json_number(text) => out.push_str(text),
        "bool" if text == "t" => out.push_str("true"),
        "bool" if text == "f" => out.push_str("false"),
        "json" | "jsonb" if json::is_valid(text) => out.push_str(text),
        _ => out.push_str(&json_string(text)),
    }
}

fn is_json_number(s: &str) -> bool {
    s.trim() == s && matches!(serde_json::from_str::<Value>(s), Ok(Value::Number(_)))
}

fn markdown(out: &mut String, columns: &[Column], rows: &[Vec<Option<&str>>]) {
    let line = |out: &mut String, cells: &mut dyn Iterator<Item = Option<&str>>| {
        out.push_str("| ");
        for (i, v) in cells.enumerate() {
            if i > 0 {
                out.push_str(" | ");
            }
            match v {
                Some(v) => push_md(out, v),
                None => out.push_str("NULL"),
            }
        }
        out.push_str(" |");
    };
    line(out, &mut columns.iter().map(|c| Some(c.name.as_str())));
    out.push_str("\n|");
    for _ in columns {
        out.push_str("---|");
    }
    for row in rows {
        out.push('\n');
        line(out, &mut row.iter().copied());
    }
}

/// `s` for a Markdown table cell: `|` escaped, and each line break (`\r\n`, `\n` or `\r`) as
/// `<br>`.
fn push_md(out: &mut String, s: &str) {
    if !has_any(s, &['|', '\n', '\r']) {
        out.push_str(s);
        return;
    }
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let (with, len) = match b {
            b'|' => ("\\|", 1),
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => ("<br>", 2),
            b'\r' | b'\n' => ("<br>", 1),
            _ => {
                i += 1;
                continue;
            }
        };
        // `start` and `i` sit on ASCII bytes: character boundaries.
        out.push_str(s.get(start..i).unwrap_or_default());
        out.push_str(with);
        i += len;
        start = i;
    }
    out.push_str(s.get(start..).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<Column> {
        ["id:int4", "name:text", "billing:jsonb", "note:text"]
            .iter()
            .map(|s| {
                let (n, t) = s.split_once(':').unwrap();
                Column { name: n.into(), base_type: t.into(), type_label: t.into() }
            })
            .collect()
    }
    fn rows() -> Vec<Vec<Option<&'static str>>> {
        vec![vec![Some("1"), Some("Ann\tB"), Some(r#"{"a":1}"#), None], vec![Some("2"), Some(""), None, Some(r#"say "hi""#)]]
    }

    #[test]
    fn tsv_values_only() {
        assert_eq!(render(CopyFormat::Tsv, &cols(), &rows()), "1\t\"Ann\tB\"\t{\"a\":1}\t\n2\t\t\tsay \"hi\"");
    }
    #[test]
    fn tsv_with_header() {
        assert!(render(CopyFormat::TsvWithHeader, &cols(), &rows()).starts_with("id\tname\tbilling\tnote\n1\t"));
    }
    #[test]
    fn csv_with_header() {
        assert_eq!(render(CopyFormat::Csv, &cols(), &rows()), "id,name,billing,note\n1,Ann\tB,\"{\"\"a\"\":1}\",\n2,,,\"say \"\"hi\"\"\"");
    }
    #[test]
    fn json_types_and_nulls() {
        assert_eq!(
            render(CopyFormat::Json, &cols(), &rows()),
            "[\n  {\"id\": 1, \"name\": \"Ann\\tB\", \"billing\": {\"a\":1}, \"note\": null},\n  {\"id\": 2, \"name\": \"\", \"billing\": null, \"note\": \"say \\\"hi\\\"\"}\n]"
        );
    }
    #[test]
    fn markdown_nulls_and_pipes() {
        let r = render(CopyFormat::Markdown, &cols(), &[vec![Some("1"), Some("a|b\nc"), None, Some("")]]);
        assert_eq!(r, "| id | name | billing | note |\n|---|---|---|---|\n| 1 | a\\|b<br>c | NULL |  |");
    }

    #[test]
    fn tsv_quotes_leading_quote_and_newlines() {
        let c = [Column::untyped("a")];
        assert_eq!(render(CopyFormat::Tsv, &c, &[vec![Some("\"x")], vec![Some("a\nb")]]), "\"\"\"x\"\n\"a\nb\"");
    }

    #[test]
    fn json_scalars_fall_back_to_strings() {
        let c: Vec<Column> = ["int4", "numeric", "bool", "bool", "jsonb", "float8", "text"]
            .iter()
            .map(|t| Column { name: (*t).into(), base_type: (*t).into(), type_label: String::new() })
            .collect();
        let row = vec![Some("x"), Some("NaN"), Some("t"), Some("f"), Some("{bad"), Some("1.5e3"), Some("7")];
        assert_eq!(
            render(CopyFormat::Json, &c, &[row]),
            "[\n  {\"int4\": \"x\", \"numeric\": \"NaN\", \"bool\": true, \"bool\": false, \"jsonb\": \"{bad\", \"float8\": 1.5e3, \"text\": \"7\"}\n]"
        );
    }

    #[test]
    fn deeply_nested_json_does_not_overflow() {
        let c = [Column { name: "j".into(), base_type: "jsonb".into(), type_label: String::new() }];
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let out = render(CopyFormat::Json, &c, &[vec![Some(deep.as_str())]]);
        assert!(out.starts_with("[\n  {\"j\": ["), "{}", &out[..40]);
    }

    #[test]
    fn short_rows_and_empty_results() {
        assert_eq!(render(CopyFormat::Json, &cols(), &[]), "[]");
        assert_eq!(render(CopyFormat::Csv, &cols(), &[]), "id,name,billing,note");
        assert_eq!(render(CopyFormat::Tsv, &cols(), &[]), "");
        assert!(render(CopyFormat::Json, &cols(), &[vec![Some("1")]]).contains("\"name\": null"));
    }

    /// The renderer as first written (a string per field, joined), kept to check the one-buffer
    /// version against.
    mod reference {
        use super::super::{is_json_number, json_string};
        use crate::cell::Column;
        use crate::json;

        pub fn delimited(columns: &[Column], rows: &[Vec<Option<&str>>], header: bool, csv: bool) -> String {
            let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            let field = |s: &str| {
                let q = if csv { s.contains([',', '"', '\n', '\r']) } else { s.contains(['\t', '\n', '\r']) || s.starts_with('"') };
                if q { quote(s) } else { s.to_string() }
            };
            let sep = if csv { "," } else { "\t" };
            let mut lines: Vec<String> = Vec::new();
            if header {
                lines.push(columns.iter().map(|c| field(&c.name)).collect::<Vec<_>>().join(sep));
            }
            for row in rows {
                lines.push(row.iter().map(|v| v.map(field).unwrap_or_default()).collect::<Vec<_>>().join(sep));
            }
            lines.join("\n")
        }

        pub fn json_text(columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
            if rows.is_empty() {
                return "[]".to_string();
            }
            let value = |t: &str, v: Option<&str>| {
                let Some(text) = v else { return "null".to_string() };
                match t {
                    "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" if is_json_number(text) => text.to_string(),
                    "bool" if text == "t" => "true".to_string(),
                    "bool" if text == "f" => "false".to_string(),
                    "json" | "jsonb" if json::is_valid(text) => text.to_string(),
                    _ => json_string(text),
                }
            };
            let lines: Vec<String> = rows
                .iter()
                .map(|row| {
                    let fields: Vec<String> = columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| format!("{}: {}", json_string(&c.name), value(&c.base_type, row.get(i).copied().flatten())))
                        .collect();
                    format!("  {{{}}}", fields.join(", "))
                })
                .collect();
            format!("[\n{}\n]", lines.join(",\n"))
        }

        pub fn markdown(columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
            let esc = |s: &str| s.replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>");
            let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
            let mut lines = vec![line(columns.iter().map(|c| esc(&c.name)).collect())];
            lines.push(format!("|{}", "---|".repeat(columns.len())));
            for row in rows {
                lines.push(line(row.iter().map(|v| v.map(esc).unwrap_or_else(|| "NULL".to_string())).collect()));
            }
            lines.join("\n")
        }
    }

    #[test]
    fn one_buffer_rendering_matches_the_reference() {
        // Pieces that need quoting, escaping or a line break, mixed at random.
        let pieces = ["a", "\t", "\n", "\r", "\r\n", "\"", ",", "|", "é", "🐝", "1", "-2.5", "t", "f", "{\"k\": [1]}", "{bad", " ", "\\", "NaN"];
        let types = ["text", "int4", "numeric", "bool", "jsonb", "json", "float8", ""];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for round in 0..2_000 {
            let word = |next: &mut dyn FnMut(usize) -> usize| (0..next(4)).map(|_| pieces[next(pieces.len())]).collect::<String>();
            let ncols = next(4);
            let columns: Vec<Column> = (0..ncols)
                .map(|_| Column { name: word(&mut next), base_type: types[next(types.len())].to_string(), type_label: String::new() })
                .collect();
            let owned: Vec<Vec<Option<String>>> = (0..next(4))
                .map(|_| (0..if next(5) == 0 { next(5) } else { ncols }).map(|_| (next(4) != 0).then(|| word(&mut next))).collect())
                .collect();
            let rows: Vec<Vec<Option<&str>>> = owned.iter().map(|r| r.iter().map(|v| v.as_deref()).collect()).collect();
            let cases = [
                (CopyFormat::Tsv, reference::delimited(&columns, &rows, false, false)),
                (CopyFormat::TsvWithHeader, reference::delimited(&columns, &rows, true, false)),
                (CopyFormat::Csv, reference::delimited(&columns, &rows, true, true)),
                (CopyFormat::Json, reference::json_text(&columns, &rows)),
                (CopyFormat::Markdown, reference::markdown(&columns, &rows)),
            ];
            for (format, expected) in cases {
                assert_eq!(render(format, &columns, &rows), expected, "round {round}, {format:?}: {columns:?} {rows:?}");
            }
        }
    }

    #[test]
    fn format_ids_round_trip() {
        for f in [CopyFormat::Tsv, CopyFormat::TsvWithHeader, CopyFormat::Csv, CopyFormat::Json, CopyFormat::Markdown] {
            assert_eq!(CopyFormat::from_id(f.id()), Some(f));
        }
        assert_eq!(CopyFormat::from_id("nope"), None);
    }
}

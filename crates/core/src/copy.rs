//! Clipboard formats: TSV, CSV, JSON, Markdown.
//!
//! [`render_within`] renders cells it reads one at a time ([`Cells`]) into at most a budget of
//! bytes, counting everything it writes (separators, quotes, escapes, `NULL`, column names), and
//! gives up as soon as the next write would pass it.

use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cell::{Column, TypeClass};
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

    /// The format's name for people: `TSV`, `TSV with header`, `CSV`, `JSON`, `Markdown`.
    pub fn label(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "TSV",
            CopyFormat::TsvWithHeader => "TSV with header",
            CopyFormat::Csv => "CSV",
            CopyFormat::Json => "JSON",
            CopyFormat::Markdown => "Markdown",
        }
    }
}

/// The cells [`render_within`] copies, read one at a time, so a large selection is never gathered
/// first.
pub trait Cells {
    fn rows(&self) -> usize;
    /// How many cells row `row` has: the columns, for a grid. A shorter row renders shorter in
    /// TSV, CSV and Markdown, and with nulls in JSON.
    fn width(&self, row: usize) -> usize;
    /// Cell `(row, col)`; `None` is NULL, as is a cell past the row's end.
    fn cell(&self, row: usize, col: usize) -> Option<&str>;

    /// Calls `f` with each of row `row`'s cells in turn (`width(row)` of them), stopping at its
    /// first error. Reading a whole row at once can cost far less than [`Cells::cell`] for each.
    fn each<E>(&self, row: usize, mut f: impl FnMut(Option<&str>) -> Result<(), E>) -> Result<(), E> {
        for col in 0..self.width(row) {
            f(self.cell(row, col))?;
        }
        Ok(())
    }
}

impl Cells for [Vec<Option<&str>>] {
    fn rows(&self) -> usize {
        self.len()
    }
    fn width(&self, row: usize) -> usize {
        self.get(row).map_or(0, Vec::len)
    }
    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.get(row)?.get(col).copied().flatten()
    }
    fn each<E>(&self, row: usize, mut f: impl FnMut(Option<&str>) -> Result<(), E>) -> Result<(), E> {
        if let Some(cells) = self.get(row) {
            for &v in cells {
                f(v)?;
            }
        }
        Ok(())
    }
}

/// Why [`render_within`] rendered nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooLarge {
    /// The cells alone hold `bytes`, more than the budget: rendering never started.
    Cells { bytes: usize },
    /// The text would have passed the budget, after `rows` whole rows.
    Text { rows: usize },
}

/// Renders rows in `format`. Lines are joined with `\n`, with no trailing newline.
pub fn render(format: CopyFormat, columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
    // No budget: nothing to give up on.
    render_within(format, columns, rows, usize::MAX).unwrap_or_default()
}

/// [`render`] of `cells`, in at most `budget` bytes.
///
/// First, for a selection of up to [`PRESUM_MAX_CELLS`] cells, their bytes are added up: more than
/// the budget is refused before anything is written ([`TooLarge::Cells`]), since every format
/// writes each cell's text at least once. A larger selection skips this: reading its cells twice
/// would cost more than writing up to the budget, where the text stops anyway.
///
/// Then the text is written into one string, sized up front from the cells and never past the
/// budget: each field is searched for the characters that need quoting or escaping and appended,
/// never built as a string of its own. A write that would pass the budget ends it
/// ([`TooLarge::Text`]).
pub fn render_within<C: Cells + ?Sized>(format: CopyFormat, columns: &[Column], cells: &C, budget: usize) -> Result<String, TooLarge> {
    let count = (0..cells.rows()).fold(0usize, |n, row| n.saturating_add(cells.width(row)));
    let mut bytes = 0usize;
    if count <= PRESUM_MAX_CELLS {
        for row in 0..cells.rows() {
            let Ok(()) = cells.each::<Infallible>(row, |v| {
                bytes = bytes.saturating_add(v.map_or(0, str::len));
                Ok(())
            });
        }
        if bytes > budget {
            return Err(TooLarge::Cells { bytes });
        }
    }
    let names = columns.iter().fold(0usize, |n, c| n.saturating_add(c.name.len()).saturating_add(3));
    let guess = bytes.saturating_add(count.saturating_mul(3)).saturating_add(names);
    let mut out = Out { text: String::with_capacity(guess.min(budget).min(MAX_RESERVE)), budget, rows: 0 };
    let done = match format {
        CopyFormat::Tsv => delimited(&mut out, columns, cells, false, push_tsv, b'\t'),
        CopyFormat::TsvWithHeader => delimited(&mut out, columns, cells, true, push_tsv, b'\t'),
        CopyFormat::Csv => delimited(&mut out, columns, cells, true, push_csv, b','),
        CopyFormat::Json => json(&mut out, columns, cells),
        CopyFormat::Markdown => markdown(&mut out, columns, cells),
    };
    match done {
        Ok(()) => Ok(out.text),
        Err(Over) => Err(TooLarge::Text { rows: out.rows }),
    }
}

/// The most cells [`render_within`] adds up before rendering.
pub const PRESUM_MAX_CELLS: usize = 1_000_000;

/// The most [`render_within`] reserves up front; a larger copy grows as it goes.
const MAX_RESERVE: usize = 64 * 1024 * 1024;

/// The text being rendered, which never grows past its budget.
struct Out {
    text: String,
    budget: usize,
    /// The rows written whole so far, and with room left for the text's end.
    rows: usize,
}

/// A write would have passed the budget; nothing of it was written.
struct Over;

impl Out {
    fn push_str(&mut self, s: &str) -> Result<(), Over> {
        if s.len() > self.budget.saturating_sub(self.text.len()) {
            return Err(Over);
        }
        self.text.push_str(s);
        Ok(())
    }

    /// An ASCII byte.
    fn push_byte(&mut self, b: u8) -> Result<(), Over> {
        if self.text.len() >= self.budget {
            return Err(Over);
        }
        self.text.push(char::from(b));
        Ok(())
    }

    /// The ASCII byte `b`, `n` times.
    fn push_repeated(&mut self, b: u8, n: usize) -> Result<(), Over> {
        if n > self.budget.saturating_sub(self.text.len()) {
            return Err(Over);
        }
        // In pieces of a run of 64, which costs far less than a character at a time.
        let run = [b; 64];
        match std::str::from_utf8(&run) {
            Ok(run) => {
                let mut left = n;
                while left > 0 {
                    let k = left.min(run.len());
                    self.text.push_str(run.get(..k).unwrap_or_default());
                    left -= k;
                }
            }
            Err(_) => self.text.extend(std::iter::repeat_n(char::from(b), n)),
        }
        Ok(())
    }

    /// A row was written whole: it counts as fitting when the text's last `trailer` bytes (JSON's
    /// closing `\n]`) would still fit after it.
    fn row_done(&mut self, trailer: usize) {
        if self.text.len().saturating_add(trailer) <= self.budget {
            self.rows += 1;
        }
    }
}

type Field = fn(&mut Out, &str) -> Result<(), Over>;

fn delimited<C: Cells + ?Sized>(out: &mut Out, columns: &[Column], cells: &C, header: bool, field: Field, sep: u8) -> Result<(), Over> {
    if header {
        for (i, c) in columns.iter().enumerate() {
            if i > 0 {
                out.push_byte(sep)?;
            }
            field(out, &c.name)?;
        }
    }
    for row in 0..cells.rows() {
        if header || row > 0 {
            out.push_byte(b'\n')?;
        }
        // A NULL is no text: the separators before a value are written together, so a row of
        // many NULLs costs little more than counting them.
        let (mut first, mut owed) = (true, 0usize);
        cells.each(row, |v| {
            if !std::mem::take(&mut first) {
                owed += 1;
            }
            match v {
                Some(v) => {
                    out.push_repeated(sep, std::mem::take(&mut owed))?;
                    field(out, v)
                }
                None => Ok(()),
            }
        })?;
        out.push_repeated(sep, owed)?;
        out.row_done(0);
    }
    Ok(())
}

/// Whether `s` holds any of `chars`. One fast search per character beats one pass that compares
/// each character with them all.
fn has_any(s: &str, chars: &[char]) -> bool {
    chars.iter().any(|&c| s.contains(c))
}

fn push_tsv(out: &mut Out, s: &str) -> Result<(), Over> {
    if s.starts_with('"') || has_any(s, &['\t', '\n', '\r']) { push_quoted(out, s) } else { out.push_str(s) }
}

fn push_csv(out: &mut Out, s: &str) -> Result<(), Over> {
    if has_any(s, &[',', '"', '\n', '\r']) { push_quoted(out, s) } else { out.push_str(s) }
}

/// `s` in double quotes, each `"` in it doubled.
fn push_quoted(out: &mut Out, s: &str) -> Result<(), Over> {
    out.push_byte(b'"')?;
    for (i, part) in s.split('"').enumerate() {
        if i > 0 {
            out.push_str("\"\"")?;
        }
        out.push_str(part)?;
    }
    out.push_byte(b'"')
}

fn json<C: Cells + ?Sized>(out: &mut Out, columns: &[Column], cells: &C) -> Result<(), Over> {
    if cells.rows() == 0 {
        return out.push_str("[]");
    }
    // Each name as a JSON string, once.
    let names: Vec<String> = columns.iter().map(|c| json_string(&c.name)).collect();
    out.push_str("[\n")?;
    for row in 0..cells.rows() {
        if row > 0 {
            out.push_str(",\n")?;
        }
        out.push_str("  {")?;
        // Every column: the row's cells, then NULL for those a short row lacks.
        let mut fields = columns.iter().zip(&names);
        let field = |out: &mut Out, first: bool, (c, name): (&Column, &String), v: Option<&str>| {
            if !first {
                out.push_str(", ")?;
            }
            out.push_str(name)?;
            out.push_str(": ")?;
            push_json_value(out, c.class, v)
        };
        let mut i = 0usize;
        cells.each(row, |v| {
            if let Some(f) = fields.next() {
                field(out, i == 0, f, v)?;
                i += 1;
            }
            Ok(())
        })?;
        for f in fields {
            field(out, i == 0, f, None)?;
            i += 1;
        }
        out.push_byte(b'}')?;
        out.row_done(2);
    }
    out.push_str("\n]")
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn push_json_value(out: &mut Out, class: TypeClass, v: Option<&str>) -> Result<(), Over> {
    let Some(text) = v else { return out.push_str("null") };
    match class {
        TypeClass::Number if is_json_number(text) => out.push_str(text),
        TypeClass::Bool => match json_bool(text) {
            Some(b) => out.push_str(if b { "true" } else { "false" }),
            None => push_json_string(out, text),
        },
        TypeClass::Json if json::is_valid(text) => out.push_str(text),
        _ => push_json_string(out, text),
    }
}

/// A boolean as the engines print it: Postgres `t`/`f`, MySQL and SQLite `1`/`0`, or the words.
fn json_bool(text: &str) -> Option<bool> {
    match text {
        "t" | "true" | "1" => Some(true),
        "f" | "false" | "0" => Some(false),
        _ => None,
    }
}

/// The most of a string escaped at once.
const ESCAPE_CHUNK: usize = 64 * 1024;

/// `s` as a JSON string, escaped a piece at a time (escaping is the same piece by piece), so a long
/// string grows by at most one piece's escapes past what the budget takes.
fn push_json_string(out: &mut Out, s: &str) -> Result<(), Over> {
    out.push_byte(b'"')?;
    let mut rest = s;
    while !rest.is_empty() {
        let mut end = rest.len().min(ESCAPE_CHUNK);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, tail) = rest.split_at_checked(end).filter(|(p, _)| !p.is_empty()).unwrap_or((rest, ""));
        let escaped = json_string(piece);
        // Without its quotes.
        out.push_str(escaped.get(1..escaped.len().saturating_sub(1)).unwrap_or_default())?;
        rest = tail;
    }
    out.push_byte(b'"')
}

fn is_json_number(s: &str) -> bool {
    s.trim() == s && matches!(serde_json::from_str::<Value>(s), Ok(Value::Number(_)))
}

fn markdown<C: Cells + ?Sized>(out: &mut Out, columns: &[Column], cells: &C) -> Result<(), Over> {
    out.push_str("| ")?;
    for (i, c) in columns.iter().enumerate() {
        if i > 0 {
            out.push_str(" | ")?;
        }
        push_md(out, &c.name)?;
    }
    out.push_str(" |\n|")?;
    for _ in columns {
        out.push_str("---|")?;
    }
    for row in 0..cells.rows() {
        out.push_str("\n| ")?;
        let mut first = true;
        cells.each(row, |v| {
            if !std::mem::take(&mut first) {
                out.push_str(" | ")?;
            }
            match v {
                Some(v) => push_md(out, v),
                None => out.push_str("NULL"),
            }
        })?;
        out.push_str(" |")?;
        out.row_done(0);
    }
    Ok(())
}

/// `s` for a Markdown table cell: `|` escaped, and each line break (`\r\n`, `\n` or `\r`) as
/// `<br>`.
fn push_md(out: &mut Out, s: &str) -> Result<(), Over> {
    if !has_any(s, &['|', '\n', '\r']) {
        return out.push_str(s);
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
        if start < i {
            out.push_str(s.get(start..i).unwrap_or_default())?;
        }
        out.push_str(with)?;
        i += len;
        start = i;
    }
    out.push_str(s.get(start..).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The class a Postgres type name has in these tests.
    fn class_of(t: &str) -> TypeClass {
        match t {
            "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" => TypeClass::Number,
            "bool" => TypeClass::Bool,
            "json" | "jsonb" => TypeClass::Json,
            "text" => TypeClass::LongText,
            _ => TypeClass::Other,
        }
    }

    fn cols() -> Vec<Column> {
        ["id:int4", "name:text", "billing:jsonb", "note:text"]
            .iter()
            .map(|s| {
                let (n, t) = s.split_once(':').unwrap();
                Column { name: n.into(), base_type: t.into(), type_label: t.into(), class: class_of(t) }
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
            .map(|t| Column { name: (*t).into(), base_type: (*t).into(), type_label: String::new(), class: class_of(t) })
            .collect();
        let row = vec![Some("x"), Some("NaN"), Some("t"), Some("f"), Some("{bad"), Some("1.5e3"), Some("7")];
        assert_eq!(
            render(CopyFormat::Json, &c, &[row]),
            "[\n  {\"int4\": \"x\", \"numeric\": \"NaN\", \"bool\": true, \"bool\": false, \"jsonb\": \"{bad\", \"float8\": 1.5e3, \"text\": \"7\"}\n]"
        );
    }

    #[test]
    fn json_copy_by_class() {
        let col = |class| Column { name: "c".into(), base_type: String::new(), type_label: String::new(), class };
        let one = |class, value: &str| render(CopyFormat::Json, &[col(class)], &[vec![Some(value)]]);
        let field = |v: &str| format!("[\n  {{\"c\": {v}}}\n]");
        // Bool: the engines' spellings of true and false, anything else a string.
        for (text, out) in
            [("1", "true"), ("t", "true"), ("true", "true"), ("0", "false"), ("f", "false"), ("false", "false"), ("2", "\"2\""), ("yes", "\"yes\"")]
        {
            assert_eq!(one(TypeClass::Bool, text), field(out), "bool {text}");
        }
        // Number: bare when it is a JSON number.
        assert_eq!(one(TypeClass::Number, "1.50"), field("1.50"));
        assert_eq!(one(TypeClass::Number, "NaN"), field("\"NaN\""));
        // Text keeps a number-looking value a string; JSON goes in bare when valid.
        assert_eq!(one(TypeClass::Text, "1"), field("\"1\""));
        assert_eq!(one(TypeClass::LongText, "true"), field("\"true\""));
        assert_eq!(one(TypeClass::Json, r#"{"a":1}"#), field(r#"{"a":1}"#));
        assert_eq!(one(TypeClass::Json, "{bad"), field("\"{bad\""));
        assert_eq!(one(TypeClass::Other, "1"), field("\"1\""));
    }

    #[test]
    fn deeply_nested_json_does_not_overflow() {
        let c = [Column { name: "j".into(), base_type: "jsonb".into(), type_label: String::new(), class: TypeClass::Json }];
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
        use crate::cell::{Column, TypeClass};
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
            let value = |class: TypeClass, v: Option<&str>| {
                let Some(text) = v else { return "null".to_string() };
                match class {
                    TypeClass::Number if is_json_number(text) => text.to_string(),
                    TypeClass::Bool if ["t", "true", "1"].contains(&text) => "true".to_string(),
                    TypeClass::Bool if ["f", "false", "0"].contains(&text) => "false".to_string(),
                    TypeClass::Json if json::is_valid(text) => text.to_string(),
                    _ => json_string(text),
                }
            };
            let lines: Vec<String> = rows
                .iter()
                .map(|row| {
                    let fields: Vec<String> = columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| format!("{}: {}", json_string(&c.name), value(c.class, row.get(i).copied().flatten())))
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
        let pieces = ["a", "\t", "\n", "\r", "\r\n", "\"", ",", "|", "é", "🐙", "1", "-2.5", "t", "f", "{\"k\": [1]}", "{bad", " ", "\\", "NaN"];
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
                .map(|_| {
                    let name = word(&mut next);
                    let t = types[next(types.len())];
                    Column { name, base_type: t.to_string(), type_label: String::new(), class: class_of(t) }
                })
                .collect();
            let owned: Vec<Vec<Option<String>>> = (0..next(4))
                .map(|_| (0..if next(5) == 0 { next(5) } else { ncols }).map(|_| (next(4) != 0).then(|| word(&mut next))).collect())
                .collect();
            let rows: Vec<Vec<Option<&str>>> = owned.iter().map(|r| r.iter().map(|v| v.as_deref()).collect()).collect();
            let reference = |format: CopyFormat, rows: &[Vec<Option<&str>>]| match format {
                CopyFormat::Tsv => reference::delimited(&columns, rows, false, false),
                CopyFormat::TsvWithHeader => reference::delimited(&columns, rows, true, false),
                CopyFormat::Csv => reference::delimited(&columns, rows, true, true),
                CopyFormat::Json => reference::json_text(&columns, rows),
                CopyFormat::Markdown => reference::markdown(&columns, rows),
            };
            let formats = [CopyFormat::Tsv, CopyFormat::TsvWithHeader, CopyFormat::Csv, CopyFormat::Json, CopyFormat::Markdown];
            let cell_bytes: usize = rows.iter().flatten().map(|v| v.map_or(0, str::len)).sum();
            for format in formats {
                let expected = reference(format, &rows);
                assert_eq!(render(format, &columns, &rows), expected, "round {round}, {format:?}: {columns:?} {rows:?}");
                // Within a budget: the whole text when it fits, else nothing, and never past it.
                let budget = next(expected.len() + 4);
                let what = format!("round {round}, {format:?}, budget {budget} for {}: {columns:?} {rows:?}", expected.len());
                match render_within(format, &columns, rows.as_slice(), budget) {
                    Ok(text) => assert!(text.len() <= budget && text == expected, "{what}"),
                    Err(TooLarge::Cells { bytes }) => assert!(bytes == cell_bytes && bytes > budget, "{what}"),
                    Err(TooLarge::Text { rows: fit }) => {
                        // With no rows, only the header can be too long.
                        assert!(expected.len() > budget && cell_bytes <= budget && (fit < rows.len() || rows.is_empty()), "{what}: {fit} rows fit");
                        // Exactly that many: the first `fit` rows fit, one more does not.
                        let first = rows.get(..fit).unwrap_or_default();
                        assert!(reference(format, first).len() <= budget || fit == 0, "{what}: {fit} rows fit");
                        if let Some(one_more) = rows.get(..fit + 1) {
                            assert!(reference(format, one_more).len() > budget, "{what}: {fit} rows fit, and one more too");
                        }
                    }
                }
            }
        }
    }

    /// Cells read one at a time: a grid too large to gather, all NULL.
    struct Nulls {
        rows: usize,
        columns: usize,
    }

    impl Cells for Nulls {
        fn rows(&self) -> usize {
            self.rows
        }
        fn width(&self, _: usize) -> usize {
            self.columns
        }
        fn cell(&self, _: usize, _: usize) -> Option<&str> {
            None
        }
    }

    #[test]
    fn nulls_and_names_count_toward_the_budget() {
        let columns: Vec<Column> = (0..1_664).map(|i| Column::untyped(&format!("column_{i}"))).collect();
        let nulls = Nulls { rows: 10_000, columns: 1_664 };
        let budget = 1024 * 1024;
        // No cell holds a byte, yet every format but TSV renders far more than the budget.
        for format in [CopyFormat::TsvWithHeader, CopyFormat::Csv, CopyFormat::Json, CopyFormat::Markdown] {
            let r = render_within(format, &columns, &nulls, budget);
            assert!(matches!(r, Err(TooLarge::Text { rows }) if rows < 1_000), "{format:?}: {:?}", r.map(|t| t.len()));
        }
        // TSV of one row is its 1,663 tabs.
        let one = Nulls { rows: 1, columns: 1_664 };
        assert_eq!(render_within(CopyFormat::Tsv, &columns, &one, budget).map(|t| t.len()), Ok(1_663));
        // Escaping counts: 1,000 control characters are 6,000 bytes of JSON.
        let control = "\u{1}".repeat(1_000);
        let row = [vec![Some(control.as_str())]];
        let a = [Column::untyped("a")];
        assert!(matches!(render_within(CopyFormat::Json, &a, row.as_slice(), 6_000), Err(TooLarge::Text { rows: 0 })));
        assert_eq!(render_within(CopyFormat::Json, &a, row.as_slice(), 6_015).map(|t| t.len()), Ok(6_015));
    }

    /// `rows` × `columns` cells holding `x`.
    struct Xs {
        rows: usize,
        columns: usize,
    }

    impl Cells for Xs {
        fn rows(&self) -> usize {
            self.rows
        }
        fn width(&self, _: usize) -> usize {
            self.columns
        }
        fn cell(&self, _: usize, _: usize) -> Option<&str> {
            Some("x")
        }
    }

    #[test]
    fn only_a_small_selection_is_added_up_first() {
        let columns: Vec<Column> = (0..1_000).map(|i| Column::untyped(&format!("c{i}"))).collect();
        // A million cells holding a byte each, against a budget of one byte less: refused before
        // any rendering, with the cells' size.
        let small = Xs { rows: PRESUM_MAX_CELLS / 1_000, columns: 1_000 };
        assert_eq!(render_within(CopyFormat::Tsv, &columns, &small, PRESUM_MAX_CELLS - 1), Err(TooLarge::Cells { bytes: PRESUM_MAX_CELLS }));
        // One row more: rendered up to the budget instead, and refused there.
        let large = Xs { rows: PRESUM_MAX_CELLS / 1_000 + 1, columns: 1_000 };
        let r = render_within(CopyFormat::Tsv, &columns, &large, PRESUM_MAX_CELLS - 1);
        // n rows of TSV are 1,999 bytes each and n - 1 line breaks: 2,000 n - 1 bytes.
        assert_eq!(r, Err(TooLarge::Text { rows: PRESUM_MAX_CELLS / 2_000 }));
    }

    #[test]
    fn long_strings_escape_in_pieces_as_a_whole() {
        // Pieces end where a character does, wherever a two-byte one falls.
        for text in [format!("a{}", "é".repeat(40_000)), format!("{}\u{1}\"x\n", "é\\".repeat(30_000)), "\u{1}".repeat(ESCAPE_CHUNK + 3)] {
            let mut out = Out { text: String::new(), budget: usize::MAX, rows: 0 };
            assert!(push_json_string(&mut out, &text).is_ok());
            assert_eq!(out.text, json_string(&text));
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

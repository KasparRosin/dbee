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
pub fn render(format: CopyFormat, columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
    match format {
        CopyFormat::Tsv => delimited(columns, rows, false, tsv_field, '\t'),
        CopyFormat::TsvWithHeader => delimited(columns, rows, true, tsv_field, '\t'),
        CopyFormat::Csv => delimited(columns, rows, true, csv_field, ','),
        CopyFormat::Json => json(columns, rows),
        CopyFormat::Markdown => markdown(columns, rows),
    }
}

fn delimited(columns: &[Column], rows: &[Vec<Option<&str>>], header: bool, field: fn(&str) -> String, sep: char) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(rows.len().saturating_add(1));
    let join = |items: Vec<String>| items.join(&sep.to_string());
    if header {
        lines.push(join(columns.iter().map(|c| field(&c.name)).collect()));
    }
    for row in rows {
        lines.push(join(row.iter().map(|v| v.map(field).unwrap_or_default()).collect()));
    }
    lines.join("\n")
}

fn tsv_field(s: &str) -> String {
    if s.contains(['\t', '\n', '\r']) || s.starts_with('"') { quote(s) } else { s.to_string() }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { quote(s) } else { s.to_string() }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn json(columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
    if rows.is_empty() {
        return "[]".to_string();
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let v = row.get(i).copied().flatten();
                    format!("{}: {}", json_string(&c.name), json_value(&c.base_type, v))
                })
                .collect();
            format!("  {{{}}}", fields.join(", "))
        })
        .collect();
    format!("[\n{}\n]", lines.join(",\n"))
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn json_value(base_type: &str, v: Option<&str>) -> String {
    let Some(text) = v else { return "null".to_string() };
    match base_type {
        "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" if is_json_number(text) => text.to_string(),
        "bool" if text == "t" => "true".to_string(),
        "bool" if text == "f" => "false".to_string(),
        "json" | "jsonb" if json::is_valid(text) => text.to_string(),
        _ => json_string(text),
    }
}

fn is_json_number(s: &str) -> bool {
    s.trim() == s && matches!(serde_json::from_str::<Value>(s), Ok(Value::Number(_)))
}

fn markdown(columns: &[Column], rows: &[Vec<Option<&str>>]) -> String {
    let mut lines = Vec::with_capacity(rows.len().saturating_add(2));
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    lines.push(line(columns.iter().map(|c| md_escape(&c.name)).collect()));
    lines.push(format!("|{}", "---|".repeat(columns.len())));
    for row in rows {
        lines.push(line(row.iter().map(|v| v.map(md_escape).unwrap_or_else(|| "NULL".to_string())).collect()));
    }
    lines.join("\n")
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
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

    #[test]
    fn format_ids_round_trip() {
        for f in [CopyFormat::Tsv, CopyFormat::TsvWithHeader, CopyFormat::Csv, CopyFormat::Json, CopyFormat::Markdown] {
            assert_eq!(CopyFormat::from_id(f.id()), Some(f));
        }
        assert_eq!(CopyFormat::from_id("nope"), None);
    }
}

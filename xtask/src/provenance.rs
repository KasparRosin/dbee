//! `cargo xtask provenance`: no name, brand or spreadsheet term from the project dbee started
//! from may appear in a tracked file's name or text.
//!
//! Scans every git-tracked (and untracked, not ignored) file, so the local `plan/` folder is
//! never checked. Two kinds of term:
//! - `BRANDS` match anywhere, as a case-insensitive substring: `AcmeSheetApp`, `libacmesheet`
//!   and `acmesheet_engine` all match.
//! - `WORDS` match as words, plurals included, case-insensitively. A word may start or end at a
//!   camelCase or digit boundary, so `ExcelFile`, `FormulasPanel` and `xlsx2csv` match, while
//!   `excellent`, `methods` and `HTTPMethods` don't.
//!
//! A line that legitimately needs a term (say, an XLSX export format) carries
//! `provenance-ok: <reason>`, like `brand-ok:` comments elsewhere. Binary files are
//! checked by name only.

use std::path::Path;
use std::process::Command;

/// Lowercase brand and project names. They match anywhere, as substrings.
pub const BRANDS: &[&str] = &[
    "acmesheet",
    "acmestory",
    "ai.acmeteller",
    "acmeart",
    "getacmeart",
    "acmerules",
    "acme-fonts",
    "acme_fonts",
    "acme-libs",
    "example labs",
    "acmephoto",
    "acmevector",
    "acmedesign",
    "acmepdf",
    "acmefilm",
    "acmelight",
    "acmeeffect",
    "acmecad",
    "acmefig",
    "acmeword",
    "acmedeck",
    "acmesound",
];

/// Lowercase spreadsheet terms. They match as words, so they may sit inside identifiers.
pub const WORDS: &[&str] = &["excel", "spreadsheet", "workbook", "worksheet", "xlsx", "ods", "formula"];

/// A line containing this marker is not checked.
pub const OPT_OUT: &str = "provenance-ok:";

/// This file names every term, so it is the one file not scanned.
pub const EXEMPT: &str = "xtask/src/provenance.rs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    /// 1-based line number; 0 for the file name.
    pub line: usize,
    pub term: &'static str,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{} (file name): `{}`", self.path, self.term)
        } else {
            write!(f, "{}:{}: `{}`", self.path, self.line, self.term)
        }
    }
}

/// The banned terms in `line`: brand hits first, then word hits, each in const order.
pub fn terms_in(line: &str) -> Vec<&'static str> {
    if line.contains(OPT_OUT) {
        return Vec::new();
    }
    let lower = line.to_ascii_lowercase();
    let mut found: Vec<&'static str> = BRANDS.iter().copied().filter(|t| lower.contains(*t)).collect();
    found.extend(WORDS.iter().copied().filter(|t| has_word(line.as_bytes(), lower.as_bytes(), t.as_bytes())));
    found
}

/// Whether the lowercase term `t` occurs in `lower` as a word. Boundaries are read from `orig`,
/// the same line in its original case. `to_ascii_lowercase` keeps byte offsets, so the two line up.
fn has_word(orig: &[u8], lower: &[u8], t: &[u8]) -> bool {
    let mut from = 0;
    while let Some(pos) = lower.get(from..).and_then(|rest| rest.windows(t.len()).position(|w| w == t)) {
        let start = from + pos;
        let end = start + t.len();
        let plural = (lower.get(end) == Some(&b's') && ends_word(orig, end + 1))
            || (lower.get(end..end + 2) == Some(b"es".as_slice()) && ends_word(orig, end + 2));
        if starts_word(orig, start) && (ends_word(orig, end) || plural) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// A word can start at `i`: at the line start, after a non-alphanumeric byte, at a camelCase
/// step (uppercase after lowercase or a digit), or a letter after a digit.
fn starts_word(orig: &[u8], i: usize) -> bool {
    let Some(&first) = orig.get(i) else { return false };
    match i.checked_sub(1).and_then(|j| orig.get(j)).copied() {
        None => true,
        Some(p) => {
            !p.is_ascii_alphanumeric()
                || (first.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
                || (first.is_ascii_alphabetic() && p.is_ascii_digit())
        }
    }
}

/// A word can end before `i`: at the line end, before a non-alphanumeric byte, at a camelCase
/// step (uppercase after lowercase), or a digit after a letter.
fn ends_word(orig: &[u8], i: usize) -> bool {
    let Some(&next) = orig.get(i) else { return true };
    let prev = i.checked_sub(1).and_then(|j| orig.get(j)).copied();
    !next.is_ascii_alphanumeric()
        || (next.is_ascii_uppercase() && prev.is_some_and(|p| p.is_ascii_lowercase()))
        || (next.is_ascii_digit() && prev.is_some_and(|p| p.is_ascii_alphabetic()))
}

/// Findings for one file: its name, then its text (`None` for binary files).
pub fn scan(path: &str, text: Option<&str>) -> Vec<Finding> {
    if path == EXEMPT {
        return Vec::new();
    }
    let mut out: Vec<Finding> = terms_in(path).into_iter().map(|term| Finding { path: path.into(), line: 0, term }).collect();
    for (i, line) in text.unwrap_or_default().lines().enumerate() {
        out.extend(terms_in(line).into_iter().map(|term| Finding { path: path.into(), line: i + 1, term }));
    }
    out
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut findings = Vec::new();
    let mut files = 0usize;
    for path in String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()) {
        let bytes = match std::fs::read(root.join(path)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // deleted, not yet staged
            Err(e) => return Err(format!("{path}: {e}")),
        };
        files += 1;
        findings.extend(scan(path, std::str::from_utf8(&bytes).ok()));
    }
    if findings.is_empty() {
        println!("provenance: {files} files clean");
        return Ok(());
    }
    for f in &findings {
        println!("  {f}");
    }
    Err(format!("{} provenance finding(s); remove them or mark a legitimate line with `{OPT_OUT} <reason>`", findings.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_names_in_any_case_and_with_separators() {
        assert_eq!(terms_in("AcmeSheet"), vec!["acmesheet"]);
        assert_eq!(terms_in("use acmesheet_engine::Session;"), vec!["acmesheet"]);
        assert_eq!(terms_in("ai.acmeteller.acmesheet"), vec!["acmesheet", "ai.acmeteller"]);
        assert_eq!(terms_in("vendor: Example Labs LLC"), vec!["example labs"]);
        assert_eq!(terms_in("https://discord.gg/acmeart"), vec!["acmeart"]);
        assert_eq!(terms_in("ACME_FONTS_DIR"), vec!["acme_fonts"]);
        assert_eq!(terms_in("book.XLSX"), vec!["xlsx"]);
    }

    #[test]
    fn finds_plurals() {
        assert_eq!(terms_in("Two Formulas"), vec!["formula"]);
        assert_eq!(terms_in("the workbooks"), vec!["workbook"]);
        assert_eq!(terms_in("spreadsheets?"), vec!["spreadsheet"]);
        assert!(terms_in("formulaic").is_empty(), "a longer word is not a plural");
    }

    #[test]
    fn ignores_words_that_only_contain_a_term() {
        for line in ["excellent work", "HTTP methods", "periods", "reformulated", "Microsoft Windows", "handcrafted", "goods", "odds"] {
            assert!(terms_in(line).is_empty(), "{line}: {:?}", terms_in(line));
        }
    }

    #[test]
    fn opt_out_marker_skips_the_line() {
        assert!(terms_in("| XLSX | export | <!-- provenance-ok: Beekeeper exports it --> |").is_empty());
        assert_eq!(terms_in("| XLSX | export |"), vec!["xlsx"]);
    }

    #[test]
    fn non_ascii_text_does_not_panic() {
        assert!(terms_in("Ünïcødé — 数据库 🦀").is_empty());
        assert_eq!(terms_in("数据库 acmesheet 🦀"), vec!["acmesheet"]);
    }

    #[test]
    fn scan_reports_file_names_and_line_numbers() {
        let f = scan("docs/xlsx-notes.md", Some("fine\nAcmeSheet here\n"));
        assert_eq!(
            f,
            vec![
                Finding { path: "docs/xlsx-notes.md".into(), line: 0, term: "xlsx" },
                Finding { path: "docs/xlsx-notes.md".into(), line: 2, term: "acmesheet" },
            ]
        );
        assert_eq!(f[1].to_string(), "docs/xlsx-notes.md:2: `acmesheet`");
    }

    #[test]
    fn binary_files_are_checked_by_name_only() {
        assert!(scan("assets/app-icon/dbee.png", None).is_empty());
        assert_eq!(scan("assets/acmesheet.png", None).len(), 1);
    }

    #[test]
    fn the_checker_itself_is_exempt() {
        assert!(scan(EXEMPT, Some("acmesheet acmestory")).is_empty());
    }

    #[test]
    fn finds_terms_inside_identifiers() {
        assert_eq!(terms_in("struct AcmeSheetApp;"), vec!["acmesheet"]);
        assert_eq!(terms_in("libacmesheet.so"), vec!["acmesheet"]);
        for (line, term) in [
            ("ExcelFile", "excel"),
            ("XlsxExport", "xlsx"),
            ("WorkbookView", "workbook"),
            ("FormulaBar", "formula"),
            ("FormulasPanel", "formula"),
            ("xlsx2csv", "xlsx"),
            ("open_xlsx_file", "xlsx"),
        ] {
            assert_eq!(terms_in(line), vec![term], "{line}");
        }
    }

    #[test]
    fn word_edges() {
        assert!(terms_in("").is_empty());
        assert_eq!(terms_in("two formulas"), vec!["formula"], "plural at the end of the line");
        assert_eq!(terms_in("xformula formula"), vec!["formula"], "the search continues past a non-match");
        assert!(terms_in("PODS and HTTPMethods").is_empty());
    }
}

//! `cargo xtask provenance`: keeps terms that don't belong in dbee out of the names and text of
//! the repository's files.
//!
//! Scans every git-tracked (and untracked, not ignored) file, so the local `plan/` folder is
//! never checked. Two kinds of term:
//! - Brand names, listed one per line in the local, gitignored [`TERMS_FILE`] (blank lines and
//!   `#` comments are skipped). They match anywhere, as a case-insensitive substring: with
//!   `acmeware` listed, `AcmewareApp`, `libacmeware` and `acmeware_engine` all match. Without
//!   the file (CI, a fresh clone) the brand check is skipped, and the summary says so.
//! - `WORDS`, spreadsheet terms, match as words, plurals included, case-insensitively. A word
//!   may start or end at a camelCase or digit boundary, so `ExcelFile`, `FormulasPanel` and
//!   `xlsx2csv` match, while `excellent`, `methods` and `HTTPMethods` don't.
//!
//! A line that legitimately needs a term (say, an XLSX export format) carries
//! `provenance-ok: <reason>`. Binary files are checked by name only.

use std::path::Path;
use std::process::Command;

/// The local brand list, relative to the repository root. `plan/` is gitignored, so the list
/// itself is never published.
pub const TERMS_FILE: &str = "plan/provenance-terms.txt";

/// Lowercase spreadsheet terms. They match as words, so they may sit inside identifiers.
pub const WORDS: &[&str] = &["excel", "spreadsheet", "workbook", "worksheet", "xlsx", "ods", "formula"];

/// A line containing this marker is not checked.
pub const OPT_OUT: &str = "provenance-ok:";

/// This file names every word, so it is checked for brands only.
pub const EXEMPT: &str = "xtask/src/provenance.rs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    /// 1-based line number; 0 for the file name.
    pub line: usize,
    pub term: String,
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

/// The brand terms in a terms file: one per line, trimmed and lowercased. Blank lines and lines
/// starting with `#` are skipped.
pub fn parse_terms(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_ascii_lowercase).collect()
}

/// The brand terms from [`TERMS_FILE`] under `root`, or `None` when there is no such file.
pub fn load_terms(root: &Path) -> Result<Option<Vec<String>>, String> {
    match std::fs::read_to_string(root.join(TERMS_FILE)) {
        Ok(text) => Ok(Some(parse_terms(&text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{TERMS_FILE}: {e}")),
    }
}

/// The banned terms in `line`: brand hits first, in `brands` order, then word hits in `WORDS`
/// order (word hits only when `words` is set).
fn find<'a>(line: &str, brands: &'a [String], words: bool) -> Vec<&'a str> {
    if line.contains(OPT_OUT) {
        return Vec::new();
    }
    let lower = line.to_ascii_lowercase();
    let mut found: Vec<&'a str> = brands.iter().map(String::as_str).filter(|t| lower.contains(*t)).collect();
    if words {
        found.extend(WORDS.iter().copied().filter(|t| has_word(line.as_bytes(), lower.as_bytes(), t.as_bytes())));
    }
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

/// Findings for one file: its name, then its text (`None` for binary files). [`EXEMPT`] is
/// checked for brands only.
pub fn scan(path: &str, text: Option<&str>, brands: &[String]) -> Vec<Finding> {
    let words = path != EXEMPT;
    let finding = |line: usize| move |term: &str| Finding { path: path.into(), line, term: term.into() };
    let mut out: Vec<Finding> = find(path, brands, words).into_iter().map(finding(0)).collect();
    for (i, line) in text.unwrap_or_default().lines().enumerate() {
        out.extend(find(line, brands, words).into_iter().map(finding(i + 1)));
    }
    out
}

pub fn run(root: &Path) -> Result<(), String> {
    let brands = load_terms(root)?;
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
        findings.extend(scan(path, std::str::from_utf8(&bytes).ok(), brands.as_deref().unwrap_or_default()));
    }
    match &brands {
        Some(list) => println!("provenance: brand check on, {} terms from {TERMS_FILE}", list.len()),
        None => println!("provenance: brand check skipped ({TERMS_FILE} not found); spreadsheet terms checked"),
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

    /// The banned terms in `line`, as in any file but this one.
    fn terms_in<'a>(line: &str, brands: &'a [String]) -> Vec<&'a str> {
        find(line, brands, true)
    }

    /// A made-up brand list, as a terms file would hold it.
    fn brands() -> Vec<String> {
        parse_terms("# made-up brands\nacmeware\n  Initech.Labs  \n\nglobex widgets\nhooli-libs\nhooli_libs\n")
    }

    #[test]
    fn parse_terms_trims_lowercases_and_skips_blanks_and_comments() {
        assert_eq!(brands(), vec!["acmeware", "initech.labs", "globex widgets", "hooli-libs", "hooli_libs"]);
        assert!(parse_terms("").is_empty());
        assert!(parse_terms("\n   \n# only a comment\n").is_empty());
    }

    #[test]
    fn load_terms_is_none_without_the_file() {
        let root = std::env::temp_dir().join(format!("dbee-provenance-{}", std::process::id()));
        std::fs::create_dir_all(root.join("plan")).unwrap();
        assert_eq!(load_terms(&root), Ok(None));
        std::fs::write(root.join(TERMS_FILE), "Acmeware\n").unwrap();
        assert_eq!(load_terms(&root), Ok(Some(vec!["acmeware".to_string()])));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn finds_names_in_any_case_and_with_separators() {
        let b = brands();
        assert_eq!(terms_in("Acmeware", &b), vec!["acmeware"]);
        assert_eq!(terms_in("use acmeware_engine::Session;", &b), vec!["acmeware"]);
        assert_eq!(terms_in("initech.labs.acmeware", &b), vec!["acmeware", "initech.labs"]);
        assert_eq!(terms_in("vendor: Globex Widgets LLC", &b), vec!["globex widgets"]);
        assert_eq!(terms_in("https://discord.gg/acmeware", &b), vec!["acmeware"]);
        assert_eq!(terms_in("HOOLI_LIBS_DIR", &b), vec!["hooli_libs"]);
        assert_eq!(terms_in("book.XLSX", &b), vec!["xlsx"]);
    }

    #[test]
    fn without_a_brand_list_only_words_are_found() {
        assert!(terms_in("Acmeware", &[]).is_empty());
        assert_eq!(terms_in("acmeware.xlsx", &[]), vec!["xlsx"]);
    }

    #[test]
    fn finds_plurals() {
        assert_eq!(terms_in("Two Formulas", &[]), vec!["formula"]);
        assert_eq!(terms_in("the workbooks", &[]), vec!["workbook"]);
        assert_eq!(terms_in("spreadsheets?", &[]), vec!["spreadsheet"]);
        assert!(terms_in("formulaic", &[]).is_empty(), "a longer word is not a plural");
    }

    #[test]
    fn ignores_words_that_only_contain_a_term() {
        let b = brands();
        for line in ["excellent work", "HTTP methods", "periods", "reformulated", "Microsoft Windows", "handcrafted", "goods", "odds"] {
            assert!(terms_in(line, &b).is_empty(), "{line}: {:?}", terms_in(line, &b));
        }
    }

    #[test]
    fn opt_out_marker_skips_the_line() {
        let b = brands();
        assert!(terms_in("| XLSX | export | <!-- provenance-ok: Beekeeper exports it --> |", &b).is_empty());
        assert!(terms_in("acmeware <!-- provenance-ok: a test fixture -->", &b).is_empty());
        assert_eq!(terms_in("| XLSX | export |", &b), vec!["xlsx"]);
    }

    #[test]
    fn non_ascii_text_does_not_panic() {
        let b = brands();
        assert!(terms_in("Ünïcødé — 数据库 🦀", &b).is_empty());
        assert_eq!(terms_in("数据库 acmeware 🦀", &b), vec!["acmeware"]);
    }

    #[test]
    fn scan_reports_file_names_and_line_numbers() {
        let f = scan("docs/xlsx-notes.md", Some("fine\nAcmeware here\n"), &brands());
        assert_eq!(
            f,
            vec![
                Finding { path: "docs/xlsx-notes.md".into(), line: 0, term: "xlsx".into() },
                Finding { path: "docs/xlsx-notes.md".into(), line: 2, term: "acmeware".into() },
            ]
        );
        assert_eq!(f[1].to_string(), "docs/xlsx-notes.md:2: `acmeware`");
    }

    #[test]
    fn binary_files_are_checked_by_name_only() {
        let b = brands();
        assert!(scan("assets/app-icon/dbee.png", None, &b).is_empty());
        assert_eq!(scan("assets/acmeware.png", None, &b).len(), 1);
    }

    #[test]
    fn the_checker_itself_is_checked_for_brands_only() {
        let f = scan(EXEMPT, Some("excel xlsx formula\nacmeware\n"), &brands());
        assert_eq!(f, vec![Finding { path: EXEMPT.into(), line: 2, term: "acmeware".into() }]);
    }

    #[test]
    fn finds_terms_inside_identifiers() {
        let b = brands();
        assert_eq!(terms_in("struct AcmewareApp;", &b), vec!["acmeware"]);
        assert_eq!(terms_in("libacmeware.so", &b), vec!["acmeware"]);
        for (line, term) in [
            ("ExcelFile", "excel"),
            ("XlsxExport", "xlsx"),
            ("WorkbookView", "workbook"),
            ("FormulaBar", "formula"),
            ("FormulasPanel", "formula"),
            ("xlsx2csv", "xlsx"),
            ("open_xlsx_file", "xlsx"),
        ] {
            assert_eq!(terms_in(line, &b), vec![term], "{line}");
        }
    }

    #[test]
    fn word_edges() {
        assert!(terms_in("", &[]).is_empty());
        assert_eq!(terms_in("two formulas", &[]), vec!["formula"], "plural at the end of the line");
        assert_eq!(terms_in("xformula formula", &[]), vec!["formula"], "the search continues past a non-match");
        assert!(terms_in("PODS and HTTPMethods", &[]).is_empty());
    }
}

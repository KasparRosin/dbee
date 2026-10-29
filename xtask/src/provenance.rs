//! `cargo xtask provenance`: keeps brand names that don't belong in dbee out of the names and text
//! of the repository's files.
//!
//! Scans every git-tracked (and untracked, not ignored) file, so the local `plan/` folder is
//! never checked. Brand names are listed one per line in the local, gitignored [`TERMS_FILE`]
//! (blank lines and `#` comments are skipped). They match anywhere, as a case-insensitive
//! substring: with `acmeware` listed, `AcmewareApp`, `libacmeware` and `acmeware_engine` all
//! match. Without the file (CI, a fresh clone) the check is skipped, and the summary says so.
//!
//! A line that legitimately needs a term carries `provenance-ok: <reason>`. Binary files are
//! checked by name only.

use std::path::Path;
use std::process::Command;

/// The local brand list, relative to the repository root. `plan/` is gitignored, so the list
/// itself is never published.
pub const TERMS_FILE: &str = "plan/provenance-terms.txt";

/// A line containing this marker is not checked.
pub const OPT_OUT: &str = "provenance-ok:";

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

/// The brand terms in `line`, in `brands` order.
fn find<'a>(line: &str, brands: &'a [String]) -> Vec<&'a str> {
    if line.contains(OPT_OUT) {
        return Vec::new();
    }
    let lower = line.to_ascii_lowercase();
    brands.iter().map(String::as_str).filter(|t| lower.contains(*t)).collect()
}

/// Findings for one file: its name, then its text (`None` for binary files).
pub fn scan(path: &str, text: Option<&str>, brands: &[String]) -> Vec<Finding> {
    let finding = |line: usize| move |term: &str| Finding { path: path.into(), line, term: term.into() };
    let mut out: Vec<Finding> = find(path, brands).into_iter().map(finding(0)).collect();
    for (i, line) in text.unwrap_or_default().lines().enumerate() {
        out.extend(find(line, brands).into_iter().map(finding(i + 1)));
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
        None => println!("provenance: brand check skipped ({TERMS_FILE} not found)"),
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

    fn terms_in<'a>(line: &str, brands: &'a [String]) -> Vec<&'a str> {
        find(line, brands)
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
    }

    #[test]
    fn without_a_brand_list_nothing_is_found() {
        assert!(terms_in("Acmeware", &[]).is_empty());
    }

    #[test]
    fn opt_out_marker_skips_the_line() {
        let b = brands();
        assert!(terms_in("acmeware <!-- provenance-ok: a test fixture -->", &b).is_empty());
        assert_eq!(terms_in("| acmeware | export |", &b), vec!["acmeware"]);
    }

    #[test]
    fn non_ascii_text_does_not_panic() {
        let b = brands();
        assert!(terms_in("Ünïcødé — 数据库 🦀", &b).is_empty());
        assert_eq!(terms_in("数据库 acmeware 🦀", &b), vec!["acmeware"]);
    }

    #[test]
    fn scan_reports_file_names_and_line_numbers() {
        let f = scan("docs/hooli-libs-notes.md", Some("fine\nAcmeware here\n"), &brands());
        assert_eq!(
            f,
            vec![
                Finding { path: "docs/hooli-libs-notes.md".into(), line: 0, term: "hooli-libs".into() },
                Finding { path: "docs/hooli-libs-notes.md".into(), line: 2, term: "acmeware".into() },
            ]
        );
        assert_eq!(f[1].to_string(), "docs/hooli-libs-notes.md:2: `acmeware`");
    }

    #[test]
    fn binary_files_are_checked_by_name_only() {
        let b = brands();
        assert!(scan("assets/app-icon/dbee.png", None, &b).is_empty());
        assert_eq!(scan("assets/acmeware.png", None, &b).len(), 1);
    }

    #[test]
    fn finds_terms_inside_identifiers() {
        let b = brands();
        assert_eq!(terms_in("struct AcmewareApp;", &b), vec!["acmeware"]);
        assert_eq!(terms_in("libacmeware.so", &b), vec!["acmeware"]);
    }
}

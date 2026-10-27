//! `cargo xtask licenses`: list every shipped dependency whose licence is outside the allowlist
//! in `docs/standards/licensing.md`.
//!
//! For each target in `TARGETS` it reads `cargo metadata --filter-platform <target>`, walks the
//! resolve graph from the workspace members over normal and build edges (dev edges are not
//! shipped), and fails on every package whose `license` expression is not allowed, or that has
//! only a `license_file`, unless the package is in `EXCEPTIONS`. A by-hand command, not a ci step.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

/// Platforms whose dependency trees are checked.
pub const TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-freebsd"];

/// Packages approved by the owner despite a licence outside the allowlist: (crate name, reason).
pub const EXCEPTIONS: &[(&str, &str)] =
    &[("epaint_default_fonts", "egui's built-in fonts (OFL-1.1, Ubuntu-font-1.0); AGENTS.md: the app uses egui's default fonts")];

/// Licence ids the project may depend on.
const ALLOWED_IDS: &[&str] = &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "CC0-1.0", "BSL-1.0", "MPL-2.0"];

/// Nesting limit for parentheses; deeper expressions count as malformed.
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Open,
    Close,
    Or,
    And,
    With,
    Id(String),
}

fn tokenize(expr: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, toks: &mut Vec<Tok>| {
        if !word.is_empty() {
            toks.push(match word.as_str() {
                "OR" => Tok::Or,
                "AND" => Tok::And,
                "WITH" => Tok::With,
                w => Tok::Id(w.to_string()),
            });
            word.clear();
        }
    };
    for c in expr.chars() {
        match c {
            '(' => {
                flush(&mut word, &mut toks);
                toks.push(Tok::Open);
            }
            ')' => {
                flush(&mut word, &mut toks);
                toks.push(Tok::Close);
            }
            '/' => {
                flush(&mut word, &mut toks);
                toks.push(Tok::Or);
            }
            c if c.is_whitespace() => flush(&mut word, &mut toks),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut toks);
    toks
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    /// or-expr := and-expr (OR and-expr)*; `None` means malformed, `Some(b)` is "allowed".
    fn or_expr(&mut self, depth: usize) -> Option<bool> {
        let mut ok = self.and_expr(depth)?;
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            let rhs = self.and_expr(depth)?;
            ok = ok || rhs;
        }
        Some(ok)
    }

    fn and_expr(&mut self, depth: usize) -> Option<bool> {
        let mut ok = self.term(depth)?;
        while self.peek() == Some(&Tok::And) {
            self.pos += 1;
            let rhs = self.term(depth)?;
            ok = ok && rhs;
        }
        Some(ok)
    }

    fn term(&mut self, depth: usize) -> Option<bool> {
        match self.toks.get(self.pos)?.clone() {
            Tok::Open => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                self.pos += 1;
                let ok = self.or_expr(depth + 1)?;
                if self.peek() != Some(&Tok::Close) {
                    return None;
                }
                self.pos += 1;
                Some(ok)
            }
            Tok::Id(id) => {
                self.pos += 1;
                if self.peek() == Some(&Tok::With) {
                    self.pos += 1;
                    match self.toks.get(self.pos) {
                        Some(Tok::Id(_)) => self.pos += 1,
                        _ => return None,
                    }
                }
                Some(ALLOWED_IDS.contains(&id.as_str()))
            }
            _ => None,
        }
    }
}

/// Whether an SPDX licence expression may be used: `OR` needs one allowed side, `AND` needs both,
/// `X WITH e` is allowed when `X` is, and `/` means `OR`. A malformed expression is not allowed.
pub fn allowed(expr: &str) -> bool {
    let mut p = Parser { toks: tokenize(expr), pos: 0 };
    match p.or_expr(0) {
        Some(ok) => ok && p.pos == p.toks.len(),
        None => false,
    }
}

struct Offender {
    name: String,
    version: String,
    license: String,
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Packages shipped on one target that are outside the allowlist and not excepted, plus the
/// number of packages walked.
fn check_target(target: &str) -> Result<(Vec<Offender>, usize), String> {
    let out = crate::cargo()
        .args(["metadata", "--format-version", "1", "--filter-platform", target])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata ({target}) failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))?;
    analyze(&meta)
}

fn analyze(meta: &Value) -> Result<(Vec<Offender>, usize), String> {
    let empty = Vec::new();
    let packages: HashMap<&str, &Value> =
        meta.get("packages").and_then(Value::as_array).unwrap_or(&empty).iter().map(|p| (str_field(p, "id"), p)).collect();
    let resolve = meta.get("resolve").ok_or("cargo metadata: no resolve section")?;
    let nodes: HashMap<&str, &Value> =
        resolve.get("nodes").and_then(Value::as_array).unwrap_or(&empty).iter().map(|n| (str_field(n, "id"), n)).collect();

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = meta.get("workspace_members").and_then(Value::as_array).unwrap_or(&empty).iter().filter_map(Value::as_str).collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node.get("deps").and_then(Value::as_array).unwrap_or(&empty) {
            let kinds = dep.get("dep_kinds").and_then(Value::as_array).unwrap_or(&empty);
            // `kind` is null for a normal dependency, "build" or "dev" otherwise.
            let shipped = kinds.is_empty() || kinds.iter().any(|k| k.get("kind").and_then(Value::as_str) != Some("dev"));
            if shipped {
                stack.push(str_field(dep, "pkg"));
            }
        }
    }

    let mut offenders = Vec::new();
    for id in &seen {
        let Some(pkg) = packages.get(id) else { continue };
        let name = str_field(pkg, "name");
        let license = pkg.get("license").and_then(Value::as_str).unwrap_or("");
        if allowed(license) || EXCEPTIONS.iter().any(|(k, _)| *k == name) {
            continue;
        }
        let shown = if license.is_empty() {
            if pkg.get("license_file").is_some_and(|f| !f.is_null()) { "(license_file only)" } else { "(none)" }
        } else {
            license
        };
        offenders.push(Offender { name: name.to_string(), version: str_field(pkg, "version").to_string(), license: shown.to_string() });
    }
    Ok((offenders, seen.len()))
}

pub fn run(_root: &std::path::Path) -> Result<(), String> {
    let mut failures: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut most = 0;
    for target in TARGETS {
        let (offenders, n) = check_target(target)?;
        most = most.max(n);
        for o in offenders {
            failures.entry(format!("{} {} {}", o.name, o.version, o.license)).or_default().push(target);
        }
    }
    if failures.is_empty() {
        println!("OK: {most} packages on {} targets", TARGETS.len());
        return Ok(());
    }
    for (line, targets) in &failures {
        for t in targets {
            println!("{line} {t}");
        }
    }
    Err(format!("{} package(s) outside the licence allowlist (docs/standards/licensing.md)", failures.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spdx_expressions() {
        for ok in [
            "MIT",
            "MIT OR Apache-2.0",
            "MIT/Apache-2.0",
            "MIT / Apache-2.0",
            "(MIT OR Apache-2.0) AND Unicode-3.0",
            "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
            "Apache-2.0 AND ISC",
            "Unlicense OR MIT",
            "0BSD OR MIT OR Apache-2.0",
            "Apache-2.0 OR GPL-2.0-only",
            "MIT OR Apache-2.0 OR LGPL-2.1-or-later",
            "BSL-1.0",
            "MPL-2.0",
            "Zlib",
            "CC0-1.0",
            "BSD-2-Clause OR Apache-2.0 OR MIT",
            "BSD-3-Clause",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "",
            "GPL-3.0-only",
            "LGPL-2.1-or-later",
            "AGPL-3.0-only",
            "MIT AND GPL-2.0-only",
            "CDLA-Permissive-2.0",
            "ISC AND OpenSSL",
            "Unicode-DFS-2016",
            "(MIT",
            "MIT OR",
            "MIT AND",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }

    #[test]
    fn exceptions_give_reasons() {
        for (krate, why) in EXCEPTIONS {
            assert!(!krate.is_empty() && why.len() > 20, "{krate}");
        }
    }

    #[test]
    fn deep_nesting_is_malformed_not_a_crash() {
        let deep = format!("{}MIT{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(!allowed(&deep));
    }
}

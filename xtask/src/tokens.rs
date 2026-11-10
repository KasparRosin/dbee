//! `cargo xtask tokens`: colours and font sizes live in `crates/ui-egui/src/theme/` only.

use std::path::Path;

const FORBIDDEN: &[&str] = &["FontId::", "TextStyle::", ".small()", ".heading()", ".monospace()"];

fn bad_colour(line: &str) -> bool {
    line.match_indices("Color32::").any(|(i, _)| {
        let rest = &line[i + "Color32::".len()..];
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        name != "TRANSPARENT" && name != "PLACEHOLDER"
    })
}

fn bad_size(line: &str) -> bool {
    line.match_indices(".size(").any(|(i, _)| {
        let rest = &line[i + ".size(".len()..];
        !rest.starts_with(')') && !rest.starts_with("size::")
    })
}

/// Line numbers (1-based) and text of lines that name a colour or font size outside the theme.
/// The reading ends at the test module (`#[cfg(test)]` on a `mod`); any other `#[cfg(test)]`
/// skips only the item it marks.
pub fn violations(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    let mut n = 0;
    while let Some(line) = lines.get(n) {
        if let Some(rest) = line.trim_start().strip_prefix("#[cfg(test)]") {
            if marks_a_module(rest, lines.get(n + 1..).unwrap_or_default()) {
                break;
            }
            n = item_end(rest, &lines, n) + 1;
            continue;
        }
        if !line.trim_start().starts_with("//") && (bad_colour(line) || bad_size(line) || FORBIDDEN.iter().any(|f| line.contains(f))) {
            found.push((n + 1, line.trim().to_owned()));
        }
        n += 1;
    }
    found
}

/// `line` without the attributes it starts with; empty when one goes on to the next line.
fn past_attributes(mut line: &str) -> &str {
    loop {
        line = line.trim_start();
        let Some(inner) = line.strip_prefix("#[") else { return line };
        let mut depth = 1;
        let close = inner.char_indices().find(|&(_, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            depth == 0
        });
        match close {
            Some((i, _)) => line = inner.get(i + 1..).unwrap_or_default(),
            None => return "",
        }
    }
}

/// Whether the item after `#[cfg(test)]` (the `rest` of its line, then the lines `after`) is a
/// module.
fn marks_a_module(rest: &str, after: &[&str]) -> bool {
    let item = std::iter::once(rest).chain(after.iter().copied()).map(past_attributes).find(|l| !l.is_empty() && !l.starts_with("//"));
    item.is_some_and(|l| {
        let mut words = l.split_whitespace();
        let first = words.next();
        let word = if first.is_some_and(|w| w.starts_with("pub")) { words.next() } else { first };
        word == Some("mod")
    })
}

/// The index of the line where the item starting at `rest` (the end of line `start`) ends: at a
/// `;` or `,` outside brackets, at the `}` that closes its body, or at a bracket that closes the
/// block it is in. Brackets in strings, characters and comments don't count.
fn item_end(rest: &str, lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    // In a string: `Some(None)` for "…", `Some(Some(n))` for a raw string with `n` hashes.
    let mut string: Option<Option<usize>> = None;
    let text = std::iter::once(rest).chain(lines.iter().skip(start + 1).copied());
    for (i, line) in text.enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let at = |k: usize| chars.get(k).copied();
        let mut k = 0;
        while let Some(c) = at(k) {
            k += 1;
            match string {
                Some(None) => match c {
                    '\\' => k += 1,
                    '"' => string = None,
                    _ => {}
                },
                Some(Some(hashes)) => {
                    if c == '"' && (k..k + hashes).all(|j| at(j) == Some('#')) {
                        k += hashes;
                        string = None;
                    }
                }
                None => match c {
                    '/' if at(k) == Some('/') => break,
                    '"' => string = Some(None),
                    'r' if !k.checked_sub(2).and_then(at).is_some_and(|p| p.is_alphanumeric() || p == '_') => {
                        let hashes = (k..).take_while(|&j| at(j) == Some('#')).count();
                        if at(k + hashes) == Some('"') {
                            k += hashes + 1;
                            string = Some(Some(hashes));
                        }
                    }
                    '\'' if at(k) == Some('\\') => {
                        k = (k + 2..chars.len()).find(|&j| at(j) == Some('\'')).map_or(chars.len(), |j| j + 1);
                    }
                    '\'' if at(k + 1) == Some('\'') => k += 2,
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        depth -= 1;
                        if depth < 0 || (depth == 0 && c == '}') {
                            return start + i;
                        }
                    }
                    ';' | ',' if depth == 0 => return start + i,
                    _ => {}
                },
            }
        }
    }
    lines.len().saturating_sub(1)
}

fn collect(dir: &Path, skip: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for e in entries {
        let p = e.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if p == skip {
            continue;
        }
        if p.is_dir() {
            collect(&p, skip, out)?;
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let skip = root.join("crates/ui-egui/src/theme");
    let mut files = Vec::new();
    collect(&root.join("crates/ui-egui/src"), &skip, &mut files)?;
    collect(&root.join("apps/dbee/src"), &skip, &mut files)?;
    files.sort();
    let mut report = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let rel = f.strip_prefix(root).unwrap_or(f).display().to_string();
        for (n, text) in violations(&src) {
            report.push(format!("{rel}:{n}: {text}"));
        }
    }
    if report.is_empty() {
        println!("tokens: {} files, no colours or font sizes outside the theme", files.len());
        Ok(())
    } else {
        Err(format!(
            "Colours and font sizes belong in crates/ui-egui/src/theme/. Use a token, a Text style or a named size:\n  {}",
            report.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_font_sizes_are_flagged() {
        for bad in [
            "let c = Color32::from_rgb(1, 2, 3);",
            "let c = Color32::WHITE;",
            "let c = egui::Color32::BLACK;",
            "FontId::proportional(12.0)",
            "FontId::monospace(s)",
            "RichText::new(s).size(12.0)",
            "RichText::new(s).size(FONT)",
            "label.small()",
            "label.heading()",
            "label.monospace()",
            "TextStyle::Body",
        ] {
            assert_eq!(violations(bad).len(), 1, "{bad}");
        }
    }

    #[test]
    fn what_is_allowed_passes() {
        for good in [
            "Color32::TRANSPARENT",
            "Color32::PLACEHOLDER",
            "galley.size()",
            "// Color32::WHITE in a comment",
            "Spinner::new().size(size::SPINNER)",
            "Text::Label.rich(s)",
            "#[cfg(test)]\nmod tests { const C: Color32 = Color32::BLACK; }",
        ] {
            assert!(violations(good).is_empty(), "{good}");
        }
    }

    #[test]
    fn only_the_item_after_a_test_attribute_is_skipped() {
        let colour = "let c = Color32::from_rgb(1, 2, 3);";
        for (src, line) in [
            // A test helper mid-file, then drawing code.
            (
                format!(
                    "#[cfg(test)]\nfn list(&self) -> &[Token] {{\n    let _ = Color32::BLACK;\n    &self.list\n}}\n\nfn draw() {{\n    {colour}\n}}"
                ),
                8,
            ),
            (format!("#[cfg(test)]\n#[allow(dead_code)]\npub(crate) fn mark(&mut self) {{ self.c = Color32::RED; }}\n{colour}"), 4),
            (format!("#[cfg(test)] fn mark() {{ Color32::RED; }}\n{colour}"), 2),
            (format!("#[cfg(test)]\nconst C: Color32 = Color32::BLACK;\n{colour}"), 3),
            (format!("#[cfg(test)]\nuse egui::Color32;\n{colour}"), 3),
            (format!("struct S {{\n    #[cfg(test)]\n    pub c: [u8; 2],\n}}\n{colour}"), 5),
            (format!("struct S {{\n    #[cfg(test)]\n    c: u8\n}}\n{colour}"), 5),
            // Braces in strings and characters don't end the item early.
            (format!("#[cfg(test)]\nfn f() -> (&'static str, char) {{\n    (\"}}\", '{{')\n}}\n{colour}"), 5),
            (format!("#[cfg(test)]\nfn f() -> &'static str {{\n    r#\"}} \"quoted\" }}\"#\n}}\n{colour}"), 5),
            (format!("#[cfg(test)]\nfn f() -> &'static str {{\n    \"a }}\n    b\"\n}}\n{colour}"), 6),
        ] {
            assert_eq!(violations(&src), [(line, colour.to_owned())], "{src}");
        }
    }

    #[test]
    fn the_test_module_ends_the_reading() {
        for src in [
            "#[cfg(test)]\nmod tests {\n    const C: Color32 = Color32::BLACK;\n}\nconst D: Color32 = Color32::WHITE;",
            "#[cfg(test)]\n#[allow(clippy::unwrap_used)]\nmod tests {\n    const C: Color32 = Color32::BLACK;\n}",
            "#[cfg(test)]\npub(crate) mod testing {\n    const C: Color32 = Color32::BLACK;\n}",
        ] {
            assert!(violations(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn the_tree_passes() {
        assert_eq!(run(&crate::root()), Ok(()));
    }
}

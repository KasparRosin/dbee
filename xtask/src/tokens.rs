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
pub fn violations(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (n, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            break;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        if bad_colour(line) || bad_size(line) || FORBIDDEN.iter().any(|f| line.contains(f)) {
            found.push((n + 1, line.trim().to_owned()));
        }
    }
    found
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
    fn the_tree_passes() {
        assert_eq!(run(&crate::root()), Ok(()));
    }
}

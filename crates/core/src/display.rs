//! Grid text: one line, bounded length.

use std::borrow::Cow;

/// Longest grid text, in chars, before the `…`.
pub const GRID_MAX_CHARS: usize = 200;

/// The text a grid cell shows: newlines become `↵`, tabs a space, cut to [`GRID_MAX_CHARS`] plus `…`.
/// Stops reading after the cut, so a huge value costs a bounded amount of work.
pub fn grid_text(value: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut changed = false;
    let mut count = 0usize;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if count == GRID_MAX_CHARS {
            out.push('…');
            return Cow::Owned(out);
        }
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('↵');
                changed = true;
            }
            '\n' => {
                out.push('↵');
                changed = true;
            }
            '\t' => {
                out.push(' ');
                changed = true;
            }
            other => out.push(other),
        }
        count += 1;
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_text_cuts_and_flattens() {
        assert_eq!(grid_text("a\nb\tc"), "a↵b c");
        let long = "é".repeat(300);
        let g = grid_text(&long);
        assert_eq!(g.chars().count(), 201);
        assert!(g.ends_with('…'));
    }

    #[test]
    fn crlf_is_one_mark_and_short_text_is_borrowed() {
        assert_eq!(grid_text("a\r\nb\rc"), "a↵b↵c");
        assert!(matches!(grid_text("plain"), Cow::Borrowed(_)));
        let exact = "x".repeat(GRID_MAX_CHARS);
        assert_eq!(grid_text(&exact).chars().count(), GRID_MAX_CHARS);
        assert!(matches!(grid_text(&exact), Cow::Borrowed(_)));
        assert_eq!(grid_text(""), "");
    }
}

//! Grid text: one line, bounded length; and a type's short label for tight places.

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

/// PostgreSQL's long type spellings (as `format_type` writes them) that have a documented alias at
/// least as well known: the words before a precision or length, the words after it, and the alias.
const SHORT_TYPES: [(&str, &str, &str); 6] = [
    ("timestamp", " without time zone", "timestamp"),
    ("timestamp", " with time zone", "timestamptz"),
    ("time", " without time zone", "time"),
    ("time", " with time zone", "timetz"),
    ("character varying", "", "varchar"),
    ("character", "", "char"),
];

/// A type's label for a place with little room (a grid header, the sidebar's column list): the
/// long spellings in [`SHORT_TYPES`] as their alias, `timestamp(3) without time zone[]` as
/// `timestamp(3)[]` and `character varying(255)` as `varchar(255)`, with the precision or length
/// and any array marks kept. Every other label is returned as written (`double precision`,
/// `integer`, a quoted or schema-qualified name), and nothing changes case. The whole type stays
/// where there is room for it (the structure view).
pub fn short_type(label: &str) -> Cow<'_, str> {
    for (head, tail, short) in SHORT_TYPES {
        let Some(rest) = label.strip_prefix(head) else { continue };
        // The precision or length: `(3)`, `(255)`.
        let (modifier, rest) = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
            Some((inside, after)) if !inside.is_empty() && inside.bytes().all(|b| b.is_ascii_digit()) => {
                (rest.strip_suffix(after).unwrap_or_default(), after)
            }
            Some(_) => continue,
            None => ("", rest),
        };
        let Some(arrays) = rest.strip_prefix(tail) else { continue };
        if !arrays.is_empty() && !(arrays.len().is_multiple_of(2) && arrays.as_bytes().chunks(2).all(|c| c == b"[]")) {
            continue;
        }
        return Cow::Owned(format!("{short}{modifier}{arrays}"));
    }
    Cow::Borrowed(label)
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

    #[test]
    fn long_type_spellings_take_their_alias_and_keep_their_suffixes() {
        for (long, short) in [
            ("timestamp without time zone", "timestamp"),
            ("timestamp with time zone", "timestamptz"),
            ("timestamp(3) without time zone", "timestamp(3)"),
            ("timestamp(6) with time zone", "timestamptz(6)"),
            ("timestamp with time zone[]", "timestamptz[]"),
            ("timestamp(3) without time zone[][]", "timestamp(3)[][]"),
            ("time without time zone", "time"),
            ("time(0) with time zone", "timetz(0)"),
            ("time with time zone", "timetz"),
            ("character varying", "varchar"),
            ("character varying(255)", "varchar(255)"),
            ("character varying(255)[]", "varchar(255)[]"),
            ("character(3)", "char(3)"),
            ("character", "char"),
        ] {
            assert_eq!(short_type(long), short, "{long}");
        }
    }

    #[test]
    fn every_other_type_label_stays_as_written() {
        for label in [
            "",
            "integer",
            "boolean",
            "double precision",
            "numeric(12,2)",
            "bit varying(8)",
            "interval day to second",
            "\"char\"",
            "text[]",
            "timestamp",
            "timestamptz",
            "varchar(255)",
            // Another case is another engine's or a user's own spelling: data, left alone.
            "CHARACTER VARYING(20)",
            "TIMESTAMP WITHOUT TIME ZONE",
            // Not these types, whatever they start with.
            "character_set",
            "characters(3)",
            "timestamp without time zone array",
            "timestamp without time zone[",
            "timestamp() without time zone",
            "timestamp(3",
            "timestamp(x) with time zone",
            "character varying(255) [] ",
            "public.\"character varying\"",
            "time with time zones",
            "timé",
        ] {
            assert!(matches!(short_type(label), Cow::Borrowed(l) if l == label), "{label}");
        }
    }
}

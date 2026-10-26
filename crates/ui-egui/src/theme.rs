//! dbee's colours and spacing: original tokens, a light theme only in M1 (a dark theme is a
//! non-goal). Screens take colours from [`TOKENS`], never from literals, so a theme stays one
//! table. The value panel's JSON colours ([`Syntax`]) and the query editor's SQL colours
//! ([`SqlSyntax`]) come in a light and a dark set already; [`syntax`] and [`sql_syntax`] pick the
//! one for the visuals in use.

use egui::{Color32, CornerRadius, Stroke};

/// The colours every screen uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    /// The window behind everything.
    pub bg: Color32,
    /// Side bars and the status bar.
    pub panel: Color32,
    /// Cards, fields and the grid.
    pub card: Color32,
    pub border: Color32,
    pub text: Color32,
    /// Secondary text: hints, counts, the version line.
    pub text_muted: Color32,
    /// Buttons that do the main thing, the selection, focus rings.
    pub accent: Color32,
    /// Text on [`Tokens::accent`].
    pub accent_text: Color32,
    /// Selected rows and cells.
    pub selection: Color32,
    /// Hovered rows.
    pub hover: Color32,
    /// Errors.
    pub danger: Color32,
    /// Things that went well (a connection test that passed).
    pub success: Color32,
    /// The schema tree's icons, one colour per kind, and the query tab's.
    pub icon_schema: Color32,
    pub icon_table: Color32,
    pub icon_view: Color32,
    pub icon_matview: Color32,
    pub icon_column: Color32,
    pub icon_query: Color32,
    /// Primary and foreign key icons.
    pub key_primary: Color32,
    pub key_foreign: Color32,
    /// The grid's header row, and the fill of every other row.
    pub grid_header: Color32,
    pub stripe: Color32,
    /// Grid cells with staged changes: a changed cell (amber), a new row (green), a row to delete
    /// (lavender grey, its text also struck through), and the row whose change the last Apply
    /// failed on (rose). Clearly apart from each other and from the selection.
    pub staged: Color32,
    pub new_row: Color32,
    pub deleted: Color32,
    pub failed: Color32,
}

/// The light theme: a warm grey ground with a slate-teal accent.
pub const TOKENS: Tokens = Tokens {
    bg: Color32::from_rgb(0xFA, 0xFA, 0xF8),
    panel: Color32::from_rgb(0xEF, 0xF0, 0xEC),
    card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    border: Color32::from_rgb(0xD5, 0xD8, 0xD1),
    text: Color32::from_rgb(0x1D, 0x23, 0x20),
    text_muted: Color32::from_rgb(0x56, 0x5E, 0x59),
    accent: Color32::from_rgb(0x2B, 0x63, 0x5A),
    accent_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    selection: Color32::from_rgb(0xCD, 0xE2, 0xDC),
    hover: Color32::from_rgb(0xE4, 0xE7, 0xE1),
    danger: Color32::from_rgb(0xA8, 0x23, 0x1C),
    success: Color32::from_rgb(0x2D, 0x6A, 0x3A),
    icon_schema: Color32::from_rgb(0x8A, 0x7A, 0x5C),
    icon_table: Color32::from_rgb(0x2B, 0x7A, 0x6E),
    icon_view: Color32::from_rgb(0x3A, 0x6E, 0xB0),
    icon_matview: Color32::from_rgb(0x7A, 0x52, 0xA6),
    icon_column: Color32::from_rgb(0x7B, 0x83, 0x7E),
    icon_query: Color32::from_rgb(0xB0, 0x5A, 0x2E),
    key_primary: Color32::from_rgb(0xB8, 0x86, 0x14),
    key_foreign: Color32::from_rgb(0x4A, 0x7F, 0xA8),
    grid_header: Color32::from_rgb(0xF2, 0xF3, 0xEF),
    stripe: Color32::from_rgb(0xF8, 0xF9, 0xF6),
    staged: Color32::from_rgb(0xF8, 0xE4, 0xA8),
    new_row: Color32::from_rgb(0xE0, 0xF2, 0xC8),
    deleted: Color32::from_rgb(0xE6, 0xDF, 0xF2),
    failed: Color32::from_rgb(0xF7, 0xD0, 0xCA),
};

/// Syntax colours for JSON in the value panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// Object keys.
    pub key: Color32,
    /// String values.
    pub string: Color32,
    pub number: Color32,
    /// `true`, `false` and `null`.
    pub literal: Color32,
    /// Brackets, braces, colons and commas.
    pub punct: Color32,
}

impl Syntax {
    /// Each colour with its name.
    pub fn parts(&self) -> [(&'static str, Color32); 5] {
        [("key", self.key), ("string", self.string), ("number", self.number), ("literal", self.literal), ("punct", self.punct)]
    }
}

/// JSON on a light ground (a card, the selection): steel blue keys, moss green strings, rust
/// numbers, plum literals and slate punctuation.
pub const SYNTAX_LIGHT: Syntax = Syntax {
    key: Color32::from_rgb(0x1C, 0x5A, 0x85),
    string: Color32::from_rgb(0x2F, 0x6B, 0x1F),
    number: Color32::from_rgb(0x96, 0x44, 0x0E),
    literal: Color32::from_rgb(0x7A, 0x3B, 0xA0),
    punct: Color32::from_rgb(0x54, 0x5D, 0x58),
};

/// The same hues lightened for a dark ground.
pub const SYNTAX_DARK: Syntax = Syntax {
    key: Color32::from_rgb(0x8C, 0xC2, 0xEE),
    string: Color32::from_rgb(0xA9, 0xD6, 0x8A),
    number: Color32::from_rgb(0xED, 0xB0, 0x7A),
    literal: Color32::from_rgb(0xD1, 0xA6, 0xF0),
    punct: Color32::from_rgb(0xA3, 0xAB, 0xA6),
};

/// The JSON colours for a dark ground when `dark`, else for a light one.
pub fn syntax(dark: bool) -> Syntax {
    if dark { SYNTAX_DARK } else { SYNTAX_LIGHT }
}

/// Syntax colours for SQL in the query editor; other tokens (names, operators, punctuation) take
/// the text colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqlSyntax {
    pub keyword: Color32,
    /// String literals, dollar-quoted ones included.
    pub string: Color32,
    /// Numbers and `$1` parameters.
    pub number: Color32,
    /// Comments, shown in italics too.
    pub comment: Color32,
    /// Quoted identifiers (`"Odd Name"`).
    pub quoted: Color32,
}

impl SqlSyntax {
    /// Each colour with its name.
    pub fn parts(&self) -> [(&'static str, Color32); 5] {
        [("keyword", self.keyword), ("string", self.string), ("number", self.number), ("comment", self.comment), ("quoted", self.quoted)]
    }
}

/// SQL on a light ground (the editor's card, the selection): steel blue keywords, moss green
/// strings, rust numbers, slate grey comments and plum quoted names.
pub const SQL_LIGHT: SqlSyntax = SqlSyntax {
    keyword: Color32::from_rgb(0x1C, 0x5A, 0x85),
    string: Color32::from_rgb(0x2F, 0x6B, 0x1F),
    number: Color32::from_rgb(0x96, 0x44, 0x0E),
    comment: Color32::from_rgb(0x59, 0x61, 0x5C),
    quoted: Color32::from_rgb(0x7A, 0x3B, 0xA0),
};

/// The same hues lightened for a dark ground.
pub const SQL_DARK: SqlSyntax = SqlSyntax {
    keyword: Color32::from_rgb(0x8C, 0xC2, 0xEE),
    string: Color32::from_rgb(0xA9, 0xD6, 0x8A),
    number: Color32::from_rgb(0xED, 0xB0, 0x7A),
    comment: Color32::from_rgb(0x8F, 0xB0, 0x9B),
    quoted: Color32::from_rgb(0xD1, 0xA6, 0xF0),
};

/// The SQL colours for a dark ground when `dark`, else for a light one.
pub fn sql_syntax(dark: bool) -> SqlSyntax {
    if dark { SQL_DARK } else { SQL_LIGHT }
}

/// The colour a connection is tagged with (its card, the form's strip); `None` for no colour.
pub fn connection_color(c: dbee_config::ConnectionColor) -> Option<Color32> {
    use dbee_config::ConnectionColor as C;
    match c {
        C::None => None,
        C::Red => Some(Color32::from_rgb(0xC4, 0x3B, 0x31)),
        C::Orange => Some(Color32::from_rgb(0xD4, 0x7A, 0x24)),
        C::Yellow => Some(Color32::from_rgb(0xC7, 0xA3, 0x1E)),
        C::Green => Some(Color32::from_rgb(0x3B, 0x8C, 0x4F)),
        C::Blue => Some(Color32::from_rgb(0x2F, 0x6D, 0xB5)),
        C::Purple => Some(Color32::from_rgb(0x7C, 0x4D, 0xA8)),
        C::Gray => Some(Color32::from_rgb(0x7B, 0x80, 0x7C)),
    }
}

/// Corner radius of buttons, fields and cards.
pub const RADIUS: u8 = 4;

/// Applies the theme to `ctx`: light visuals from [`TOKENS`], egui's default fonts.
pub fn apply(ctx: &egui::Context) {
    let t = TOKENS;
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let v = &mut style.visuals;
        v.dark_mode = false;
        v.override_text_color = Some(t.text);
        v.panel_fill = t.bg;
        v.window_fill = t.card;
        v.window_stroke = Stroke::new(1.0, t.border);
        v.extreme_bg_color = t.card;
        v.faint_bg_color = t.panel;
        v.code_bg_color = t.panel;
        v.error_fg_color = t.danger;
        v.hyperlink_color = t.accent;
        v.selection.bg_fill = t.selection;
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.window_corner_radius = CornerRadius::same(RADIUS.saturating_mul(2));
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = t.bg;
        w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
        for (state, fill) in [(&mut w.inactive, t.card), (&mut w.hovered, t.hover), (&mut w.active, t.selection), (&mut w.open, t.hover)] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = CornerRadius::same(RADIUS);
            state.fg_stroke = Stroke::new(1.0, t.text);
        }
        w.inactive.bg_stroke = Stroke::new(1.0, t.border);
        w.hovered.bg_stroke = Stroke::new(1.0, t.accent);
        w.active.bg_stroke = Stroke::new(1.0, t.accent);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: egui::Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_is_readable_on_every_surface() {
        let t = TOKENS;
        for (name, bg) in [("bg", t.bg), ("panel", t.panel), ("card", t.card)] {
            assert!(contrast(t.text, bg) >= 7.0, "text on {name}");
            assert!(contrast(t.text_muted, bg) >= 4.5, "muted text on {name}");
        }
        assert!(contrast(t.accent_text, t.accent) >= 4.5, "button text on the accent");
        assert!(contrast(t.danger, t.bg) >= 4.5, "errors on the background");
        assert!(contrast(t.success, t.card) >= 4.5, "success text on a card");
        assert!(contrast(t.danger, t.card) >= 4.5, "errors on a card");
    }

    #[test]
    fn grid_marks_differ_and_keep_text_readable() {
        let t = TOKENS;
        let marks = [("staged", t.staged), ("new row", t.new_row), ("deleted", t.deleted), ("failed", t.failed), ("selection", t.selection)];
        for (name, fill) in marks.iter().copied().chain([("header", t.grid_header), ("stripe", t.stripe)]) {
            assert!(contrast(t.text, fill) >= 7.0, "text on {name}");
            assert!(contrast(t.text_muted, fill) >= 4.5, "muted text (NULL) on {name}");
        }
        // Clearly apart: a CIE76 colour difference of 15 or more between any two.
        for (i, (a, x)) in marks.iter().enumerate() {
            for (b, y) in &marks[i + 1..] {
                let d = delta_e(*x, *y);
                assert!(d >= 15.0, "{a} and {b} look alike (ΔE {d:.1})");
            }
        }
    }

    #[test]
    fn json_colours_read_on_their_ground_and_differ() {
        let dark = egui::Visuals::dark();
        for (theme, s, grounds) in [
            ("light", SYNTAX_LIGHT, [("card", TOKENS.card), ("selection", TOKENS.selection)]),
            ("dark", SYNTAX_DARK, [("field", dark.extreme_bg_color), ("panel", dark.panel_fill)]),
        ] {
            let parts = s.parts();
            for (part, colour) in parts {
                for (ground, fill) in grounds {
                    let c = contrast(colour, fill);
                    assert!(c >= 4.5, "{theme} {part} on {ground}: {c:.2}");
                }
            }
            for (i, (a, x)) in parts.iter().enumerate() {
                for (b, y) in &parts[i + 1..] {
                    let d = delta_e(*x, *y);
                    assert!(d >= 15.0, "{theme}: {a} and {b} look alike (ΔE {d:.1})");
                }
            }
        }
        assert_eq!((syntax(false), syntax(true)), (SYNTAX_LIGHT, SYNTAX_DARK));
    }

    #[test]
    fn sql_colours_read_on_their_ground_and_differ_from_each_other_and_the_text() {
        let dark = egui::Visuals::dark();
        for (theme, s, text, grounds) in [
            ("light", SQL_LIGHT, TOKENS.text, [("card", TOKENS.card), ("selection", TOKENS.selection)]),
            ("dark", SQL_DARK, dark.text_color(), [("field", dark.extreme_bg_color), ("panel", dark.panel_fill)]),
        ] {
            let parts = s.parts();
            for (part, colour) in parts {
                for (ground, fill) in grounds {
                    let c = contrast(colour, fill);
                    assert!(c >= 4.5, "{theme} {part} on {ground}: {c:.2}");
                }
            }
            let with_text: Vec<_> = parts.iter().copied().chain([("text", text)]).collect();
            for (i, (a, x)) in with_text.iter().enumerate() {
                for (b, y) in &with_text[i + 1..] {
                    let d = delta_e(*x, *y);
                    assert!(d >= 15.0, "{theme}: {a} and {b} look alike (ΔE {d:.1})");
                }
            }
        }
        assert_eq!((sql_syntax(false), sql_syntax(true)), (SQL_LIGHT, SQL_DARK));
    }

    /// CIE76 ΔE between two sRGB colours (D65).
    fn delta_e(a: egui::Color32, b: egui::Color32) -> f64 {
        let lab = |c: egui::Color32| {
            let lin = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
            let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
            let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
            (116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z)))
        };
        let ((l1, a1, b1), (l2, a2, b2)) = (lab(a), lab(b));
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }
}

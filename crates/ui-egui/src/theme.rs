//! dbee's colours and spacing: original tokens, a light theme only in M1 (a dark theme is a
//! non-goal). Screens take colours from [`TOKENS`], never from literals, so a theme stays one
//! table.

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
};

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
    }
}

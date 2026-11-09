//! dbee's colours and sizes: the 44 tokens in light and dark ([`Tokens`], [`LIGHT`], [`DARK`]).
//! Screens read the mode they draw in with [`tokens`], never literals.

mod fonts;
pub mod size;
mod text;
mod tokens;
pub use fonts::{INTER_MEDIUM, MONO_MEDIUM};
pub use text::Text;
pub use tokens::{DARK, LIGHT, Tokens, connection_color, on_connection_color, tokens};

use egui::{Color32, CornerRadius, Shadow, Stroke};
use size::{RADIUS_CONTROL, RADIUS_FLOAT};

fn style_for(style: &mut egui::Style, t: &Tokens, dark: bool) {
    let v = &mut style.visuals;
    v.dark_mode = dark;
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_muted);
    v.panel_fill = t.bg;
    v.window_fill = t.raised;
    v.window_stroke = Stroke::new(1.0, t.raised_line);
    v.extreme_bg_color = t.card;
    v.faint_bg_color = t.card;
    v.code_bg_color = t.card;
    v.error_fg_color = t.danger;
    v.warn_fg_color = t.staged_strong;
    v.hyperlink_color = t.ink;
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.ink);
    v.text_cursor.stroke = Stroke::new(v.text_cursor.stroke.width, t.ink);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow };
    v.window_corner_radius = CornerRadius::same(RADIUS_FLOAT);
    v.menu_corner_radius = CornerRadius::same(RADIUS_FLOAT);
    let w = &mut v.widgets;
    w.noninteractive.weak_bg_fill = t.bg;
    w.noninteractive.bg_fill = t.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.noninteractive.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    let states = [
        (&mut w.inactive, t.card, t.line_strong, t.fill),
        (&mut w.hovered, t.hover, t.ink, t.fill),
        (&mut w.active, t.selection, t.ink, t.fill),
        (&mut w.open, t.hover, t.line_strong, Color32::TRANSPARENT),
    ];
    for (state, ground, edge, bar) in states {
        state.weak_bg_fill = ground;
        state.bg_fill = if bar == Color32::TRANSPARENT { ground } else { bar };
        state.bg_stroke = Stroke::new(1.0, edge);
        state.corner_radius = CornerRadius::same(RADIUS_CONTROL);
        state.fg_stroke = Stroke::new(1.0, t.text);
    }
    style.text_styles = [
        (egui::TextStyle::Heading, Text::Title),
        (egui::TextStyle::Body, Text::Body),
        (egui::TextStyle::Button, Text::Button),
        (egui::TextStyle::Small, Text::Small),
        (egui::TextStyle::Monospace, Text::Data),
    ]
    .into_iter()
    .map(|(style, text)| (style, text.font()))
    .collect();
    style.spacing.scroll.foreground_color = false;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
}

/// Applies the theme to `ctx`: fonts, and both modes' styles from [`LIGHT`] and [`DARK`].
/// It draws in light until the mode follows the system.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts::definitions());
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| style_for(style, &LIGHT, false));
    ctx.style_mut_of(egui::Theme::Dark, |style| style_for(style, &DARK, true));
}

/// For a test harness whose first frame runs before it can set fonts: applies the theme once and
/// says whether it had been applied already.
#[cfg(test)]
pub(crate) fn applied(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("dbee-test-theme-applied");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return true;
    }
    apply(ctx);
    ctx.data_mut(|d| d.insert_temp(id, true));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_both_modes() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
            let style = ctx.style_of(theme);
            assert_eq!(style.visuals.panel_fill, t.bg);
            assert!(!style.spacing.scroll.foreground_color);
            assert_eq!(style.visuals.widgets.inactive.bg_fill, t.fill);
            assert_eq!(style.visuals.widgets.inactive.weak_bg_fill, t.card);
        }
    }
}

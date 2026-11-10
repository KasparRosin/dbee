//! dbee's colours and sizes: the 44 tokens in light and dark ([`Tokens`], [`LIGHT`], [`DARK`]).
//! Screens read the mode they draw in with [`tokens`], never literals.

mod fonts;
pub mod size;
mod system;
mod text;
mod tokens;
pub use fonts::{INTER_MEDIUM, MONO_MEDIUM};
pub use system::desktop_mode;
pub use text::Text;
pub use tokens::{DARK, LIGHT, Tokens, connection_color, on_connection_color, tokens};

use egui::{Color32, CornerRadius, Shadow, Stroke};
use size::{RADIUS_CONTROL, RADIUS_FLOAT};

/// The design's one shadow, under every popup and dialog.
pub fn float_shadow(t: &Tokens) -> Shadow {
    Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow }
}

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
    // egui thickens light text on a dark ground; the design's text weighs the same in both modes.
    v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Off;
    v.text_options.font_hinting = false;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = float_shadow(t);
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

/// The style of every popup on `raised` (menus, context menus, lists) in the mode `ctx` draws in:
/// egui's menu look, with a hovered, pressed, open or chosen item on `raised_hover` and lines in
/// `raised_line`, since `hover`, `selection` and `line` don't show on `raised` in dark.
pub fn popup_style(ctx: &egui::Context) -> egui::style::StyleModifier {
    let t = tokens(ctx);
    egui::style::StyleModifier::new(move |style| {
        egui::containers::menu::menu_style(style);
        let v = &mut style.visuals;
        for state in [&mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            state.weak_bg_fill = t.raised_hover;
        }
        v.widgets.noninteractive.bg_stroke.color = t.raised_line;
        v.selection.bg_fill = t.raised_hover;
    })
}

/// Applies the theme to `ctx`: fonts, and both modes' styles from [`LIGHT`] and [`DARK`].
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts::definitions());
    ctx.style_mut_of(egui::Theme::Light, |style| style_for(style, &LIGHT, false));
    ctx.style_mut_of(egui::Theme::Dark, |style| style_for(style, &DARK, true));
}

/// The mode `choice` draws in: Light and Dark as chosen, System as `system` says, Light when
/// it says nothing.
pub fn mode_for(choice: dbee_config::ThemeChoice, system: Option<egui::Theme>) -> egui::Theme {
    use dbee_config::ThemeChoice;
    match choice {
        ThemeChoice::Light => egui::Theme::Light,
        ThemeChoice::Dark => egui::Theme::Dark,
        ThemeChoice::System => system.unwrap_or(egui::Theme::Light),
    }
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
    fn mode_for_follows_the_choice() {
        use dbee_config::ThemeChoice::{Dark, Light, System};
        use egui::Theme;
        assert_eq!(mode_for(Light, Some(Theme::Dark)), Theme::Light);
        assert_eq!(mode_for(Dark, Some(Theme::Light)), Theme::Dark);
        assert_eq!(mode_for(Dark, None), Theme::Dark);
        assert_eq!(mode_for(System, Some(Theme::Dark)), Theme::Dark);
        assert_eq!(mode_for(System, Some(Theme::Light)), Theme::Light);
        assert_eq!(mode_for(System, None), Theme::Light);
    }

    #[test]
    fn text_draws_unthickened_and_unhinted_in_both_modes() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let o = ctx.style_of(theme).visuals.text_options;
            assert_eq!(o.color_transfer_function, egui::epaint::FontColorTransferFunction::Off, "{theme:?}: no thickening");
            assert!(!o.font_hinting, "{theme:?}: no hinting");
        }
    }

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

//! dbee's colours and sizes: the 44 tokens in light and dark ([`Tokens`], [`LIGHT`], [`DARK`]).
//! Screens read the mode they draw in with [`tokens`], never literals.

mod fonts;
pub mod size;
mod system;
mod text;
mod tokens;
pub use fonts::{INTER_MEDIUM, MONO_MEDIUM};
pub use system::desktop_mode;
pub use text::{Text, Words};
pub use tokens::{DARK, LIGHT, Tokens, connection_color, on_connection_color, tokens};

use egui::{Color32, CornerRadius, Shadow, Stroke};
use size::{CARET_W, RADIUS_CONTROL, RADIUS_FLOAT};

/// The one strength of the dim: what waits on a job is drawn at this opacity, the connection
/// form's fields while a test or connect runs and the old rows of a
/// reload.
pub const DIM: f32 = 0.5;

/// The one shadow, under every popup and dialog.
pub fn float_shadow(t: &Tokens) -> Shadow {
    // The shadow's blur 18 is a radius: its fade reaches 18 past the edge. egui's blur is the
    // fade's whole width, centred on the edge, so it takes twice that.
    Shadow { offset: [0, 6], blur: 36, spread: 0, color: t.shadow }
}

/// A mask's texel: white, as opaque as `coverage`. A texture of them has no colour of its own and
/// takes the tint it is drawn with, a token (the engine logos).
pub fn mask(coverage: u8) -> Color32 {
    Color32::from_white_alpha(coverage)
}

fn style_for(style: &mut egui::Style, t: &Tokens, dark: bool) {
    let v = &mut style.visuals;
    v.dark_mode = dark;
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_muted);
    v.panel_fill = t.bg;
    v.extreme_bg_color = t.card;
    v.faint_bg_color = t.card;
    v.code_bg_color = t.card;
    v.error_fg_color = t.danger;
    v.warn_fg_color = t.staged_strong;
    v.hyperlink_color = t.ink;
    // egui paints this behind selected text, in every text field and selectable label; nothing
    // of dbee's draws an egui widget in its `selected` look, which would take it too.
    v.selection.bg_fill = t.text_selection;
    v.selection.stroke = Stroke::new(1.0, t.ink);
    v.text_cursor.stroke = Stroke::new(CARET_W, t.ink);
    v.window_shadow = Shadow::NONE;
    v.window_corner_radius = CornerRadius::same(RADIUS_FLOAT);
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
    // egui's floating bars: the handle in the widgets' `bg_fill` (`fill`), with no track.
    style.spacing.scroll.foreground_color = false;
    style.spacing.scroll.dormant_background_opacity = 0.0;
    style.spacing.scroll.active_background_opacity = 0.0;
    style.spacing.scroll.interact_background_opacity = 0.0;
    // egui fades the ends of a scroll area that has more to scroll to; dbee's are flat.
    style.spacing.scroll.fade.strength = 0.0;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    crate::components::tooltip::apply(style, t);
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
    fn text_draws_with_eguis_own_options_in_each_mode() {
        use egui::epaint::FontColorTransferFunction::{Off, TwoCoverageMinusCoverageSq};
        let ctx = egui::Context::default();
        apply(&ctx);
        for (theme, weight) in [(egui::Theme::Light, Off), (egui::Theme::Dark, TwoCoverageMinusCoverageSq)] {
            let o = ctx.style_of(theme).visuals.text_options;
            assert_eq!(o.color_transfer_function, weight, "{theme:?}: egui's weight for the mode");
            assert!(o.font_hinting, "{theme:?}: hinted");
            assert!(o.subpixel_binning, "{theme:?}: sub-pixel binning");
            assert_eq!(o, theme.default_visuals().text_options, "{theme:?}: egui's own text options");
        }
        let fonts = fonts::definitions();
        for (name, data) in &fonts.font_data {
            assert_eq!(data.tweak.hinting, None, "{name}: the hinting egui's options say");
            assert_eq!(data.tweak.hinting_target, egui::epaint::text::HintingTarget::default(), "{name}: egui's hinting target");
        }
    }

    #[test]
    fn apply_sets_both_modes() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
            let style = ctx.style_of(theme);
            assert_eq!(style.visuals.panel_fill, t.bg);
            let scroll = style.spacing.scroll;
            assert!(scroll.floating && !scroll.foreground_color, "egui's floating bars");
            let w = &style.visuals.widgets;
            assert_eq!([w.inactive.bg_fill, w.hovered.bg_fill, w.active.bg_fill], [t.fill; 3], "the handle in `fill`, hovered and dragged too");
            let track = [scroll.dormant_background_opacity, scroll.active_background_opacity, scroll.interact_background_opacity];
            assert_eq!(track, [0.0; 3], "no track");
            assert_eq!(scroll.fade.strength, 0.0, "no fade at a scroll area's ends");
            assert_eq!(style.visuals.widgets.inactive.bg_fill, t.fill);
            assert_eq!(style.visuals.widgets.inactive.weak_bg_fill, t.card);
        }
    }
}

//! The showcase of the design's Components page (`components::showcase`), in both modes.

mod common;

use common::{flat, text_colours};
use dbee_ui_egui::components::showcase;
use dbee_ui_egui::theme::{DARK, LIGHT, Tokens};
use egui_kittest::Harness;

/// Every colour of a mode. Listed field by field, so a new token can't be left out.
fn every(t: &Tokens) -> Vec<egui::Color32> {
    let Tokens {
        bg,
        card,
        ink,
        on_ink,
        text,
        text_muted,
        text_faint,
        line,
        line_strong,
        fill,
        hover,
        selection,
        text_selection,
        raised,
        raised_hover,
        raised_line,
        scrim,
        shadow,
        danger,
        on_danger,
        success,
        staged,
        staged_strong,
        new_row,
        deleted,
        saved,
        failed,
        key,
        brand,
        syn_keyword,
        syn_string,
        syn_number,
        syn_literal,
        match_other,
        match_current,
        conn_red,
        conn_orange,
        conn_yellow,
        conn_green,
        conn_blue,
        conn_purple,
        conn_grey,
        on_colour,
        on_colour_dark,
    } = *t;
    vec![
        bg,
        card,
        ink,
        on_ink,
        text,
        text_muted,
        text_faint,
        line,
        line_strong,
        fill,
        hover,
        selection,
        text_selection,
        raised,
        raised_hover,
        raised_line,
        scrim,
        shadow,
        danger,
        on_danger,
        success,
        staged,
        staged_strong,
        new_row,
        deleted,
        saved,
        failed,
        key,
        brand,
        syn_keyword,
        syn_string,
        syn_number,
        syn_literal,
        match_other,
        match_current,
        conn_red,
        conn_orange,
        conn_yellow,
        conn_green,
        conn_blue,
        conn_purple,
        conn_grey,
        on_colour,
        on_colour_dark,
    ]
}

#[test]
fn the_showcase_draws_in_both_modes() {
    for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        let mut ready = false;
        let h = Harness::builder().with_size(egui::vec2(1240.0, 2400.0)).build_ui(|ui| {
            if !ready {
                showcase::prepare(ui.ctx(), theme);
                ready = true;
                ui.ctx().request_repaint();
                return;
            }
            showcase::show(ui);
        });
        assert_eq!(h.ctx.theme(), theme);
        let shapes = flat(&h.output().shapes);
        let words: Vec<String> = shapes
            .iter()
            .filter_map(|(_, s)| match s {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        for caption in ["ICON/, TODAY'S DRAWINGS IN THEIR USUAL COLOUR", "KEYCAP: DEFAULT, DISABLED", "CHIP: NEXT, AVAILABLE"] {
            assert!(words.iter().any(|w| w == caption), "{theme:?}: the section `{caption}`: {words:?}");
        }
        let tokens = every(&t);
        for c in text_colours(&shapes) {
            assert!(tokens.contains(&c), "{theme:?}: a text in {c:?}, which is no token of this mode");
        }
    }
}

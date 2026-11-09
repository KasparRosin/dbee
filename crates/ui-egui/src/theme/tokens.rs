//! The 44 colour tokens, in light and dark. Screens read them through [`tokens`], never from
//! literals, so a mode stays one table.

use egui::Color32;

/// The colours every screen uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub bg: Color32,
    pub card: Color32,
    pub ink: Color32,
    pub on_ink: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    pub fill: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub text_selection: Color32,
    pub raised: Color32,
    pub raised_hover: Color32,
    pub raised_line: Color32,
    pub scrim: Color32,
    pub shadow: Color32,
    pub danger: Color32,
    pub on_danger: Color32,
    pub success: Color32,
    pub staged: Color32,
    pub staged_strong: Color32,
    pub new_row: Color32,
    pub deleted: Color32,
    pub saved: Color32,
    pub failed: Color32,
    pub key: Color32,
    pub brand: Color32,
    pub syn_keyword: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_literal: Color32,
    pub match_other: Color32,
    pub match_current: Color32,
    pub conn_red: Color32,
    pub conn_orange: Color32,
    pub conn_yellow: Color32,
    pub conn_green: Color32,
    pub conn_blue: Color32,
    pub conn_purple: Color32,
    pub conn_grey: Color32,
    pub on_colour: Color32,
    pub on_colour_dark: Color32,
}

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgba_unmultiplied_const((h >> 16) as u8, (h >> 8) as u8, h as u8, 255)
}

const fn rgba(h: u32) -> Color32 {
    Color32::from_rgba_unmultiplied_const((h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub const LIGHT: Tokens = Tokens {
    bg: rgb(0xFFFFFF),
    card: rgb(0xFFFFFF),
    ink: rgb(0x111111),
    on_ink: rgb(0xFFFFFF),
    text: rgb(0x111111),
    text_muted: rgb(0x5A5A5A),
    text_faint: rgba(0x5A5A5A80),
    line: rgb(0xE8E8E8),
    line_strong: rgb(0x5A5A5A),
    fill: rgb(0xE8E8E8),
    hover: rgba(0xE8E8E880),
    selection: rgb(0xE8E8E8),
    text_selection: rgb(0xE8E8E8),
    raised: rgb(0xFFFFFF),
    raised_hover: rgb(0xE8E8E8),
    raised_line: rgb(0xE8E8E8),
    scrim: rgba(0x11111133),
    shadow: rgba(0x00000029),
    danger: rgb(0xC4271D),
    on_danger: rgb(0xFFFFFF),
    success: rgb(0x1E7F3A),
    staged: rgba(0xF2B30059),
    staged_strong: rgb(0x9A6B00),
    new_row: rgba(0x1E7F3A38),
    deleted: rgba(0xC4271D1F),
    saved: rgba(0x1E7F3A33),
    failed: rgba(0xC4271D40),
    key: rgb(0xAB7C12),
    brand: rgb(0x4CAF1E),
    syn_keyword: rgb(0x1C5A85),
    syn_string: rgb(0x2F6B1F),
    syn_number: rgb(0x96440E),
    syn_literal: rgb(0x7A3BA0),
    match_other: rgba(0xF2B3004D),
    match_current: rgb(0xF2B300),
    conn_red: rgb(0xC43B31),
    conn_orange: rgb(0xD47A24),
    conn_yellow: rgb(0xC7A31E),
    conn_green: rgb(0x3B8C4F),
    conn_blue: rgb(0x2F6DB5),
    conn_purple: rgb(0x7C4DA8),
    conn_grey: rgb(0x7B807C),
    on_colour: rgb(0xFFFFFF),
    on_colour_dark: rgb(0x111111),
};

pub const DARK: Tokens = Tokens {
    bg: rgb(0x111111),
    card: rgb(0x111111),
    ink: rgb(0xF2F2F2),
    on_ink: rgb(0x111111),
    text: rgb(0xF2F2F2),
    text_muted: rgb(0xA6A6A6),
    text_faint: rgba(0xA6A6A680),
    line: rgb(0x2A2A2A),
    line_strong: rgb(0xA6A6A6),
    fill: rgb(0x2A2A2A),
    hover: rgba(0x2A2A2A80),
    selection: rgb(0x2A2A2A),
    text_selection: rgb(0x2A2A2A),
    raised: rgb(0x2A2A2A),
    raised_hover: rgba(0xF2F2F214),
    raised_line: rgb(0x111111),
    scrim: rgba(0x00000080),
    shadow: rgba(0x00000029),
    danger: rgb(0xFF7B6F),
    on_danger: rgb(0x111111),
    success: rgb(0x4CC46B),
    staged: rgba(0xF2B30038),
    staged_strong: rgb(0xF2B300),
    new_row: rgba(0x4CC46B33),
    deleted: rgba(0xFF7B6F2E),
    saved: rgba(0x4CC46B33),
    failed: rgba(0xFF7B6F40),
    key: rgb(0xE2B53E),
    brand: rgb(0x4CAF1E),
    syn_keyword: rgb(0x8CC2EE),
    syn_string: rgb(0xA9D68A),
    syn_number: rgb(0xEDB07A),
    syn_literal: rgb(0xD1A6F0),
    match_other: rgba(0xF2B30040),
    match_current: rgb(0xF2B300),
    conn_red: rgb(0xC43B31),
    conn_orange: rgb(0xD47A24),
    conn_yellow: rgb(0xC7A31E),
    conn_green: rgb(0x3B8C4F),
    conn_blue: rgb(0x2F6DB5),
    conn_purple: rgb(0x7C4DA8),
    conn_grey: rgb(0x7B807C),
    on_colour: rgb(0xFFFFFF),
    on_colour_dark: rgb(0x111111),
};

/// The tokens of the mode `ctx` draws in.
pub fn tokens(ctx: &egui::Context) -> &'static Tokens {
    match ctx.theme() {
        egui::Theme::Light => &LIGHT,
        egui::Theme::Dark => &DARK,
    }
}

/// The colour a connection is tagged with (its card, the form's strip); `None` for no colour.
pub fn connection_color(c: dbee_config::ConnectionColor) -> Option<Color32> {
    use dbee_config::ConnectionColor as C;
    let t = LIGHT;
    match c {
        C::None => None,
        C::Red => Some(t.conn_red),
        C::Orange => Some(t.conn_orange),
        C::Yellow => Some(t.conn_yellow),
        C::Green => Some(t.conn_green),
        C::Blue => Some(t.conn_blue),
        C::Purple => Some(t.conn_purple),
        C::Gray => Some(t.conn_grey),
    }
}

/// The text colour that reads on [`connection_color`]: white on red, blue and purple, near-black on the rest.
pub fn on_connection_color(c: dbee_config::ConnectionColor) -> Option<Color32> {
    use dbee_config::ConnectionColor as C;
    let t = LIGHT;
    match c {
        C::None => None,
        C::Red | C::Blue | C::Purple => Some(t.on_colour),
        C::Orange | C::Yellow | C::Green | C::Gray => Some(t.on_colour_dark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `top` with its own opacity laid over an opaque `ground`.
    fn over(top: Color32, ground: Color32) -> Color32 {
        let [r, g, b, a] = top.to_srgba_unmultiplied();
        let a = f64::from(a) / 255.0;
        let mix = |t: u8, g: u8| (f64::from(t) * a + f64::from(g) * (1.0 - a)).round() as u8;
        Color32::from_rgb(mix(r, ground.r()), mix(g, ground.g()), mix(b, ground.b()))
    }

    fn both() -> [(&'static str, Tokens); 2] {
        [("light", LIGHT), ("dark", DARK)]
    }

    /// One ground at a time, as foundations says: nothing here is stacked on another mark.
    fn grounds(t: &Tokens) -> Vec<(&'static str, Color32)> {
        vec![
            ("bg", t.bg),
            ("card", t.card),
            ("fill", t.fill),
            ("selection", t.selection),
            ("raised", t.raised),
            ("hover on card", over(t.hover, t.card)),
            ("raised_hover on raised", over(t.raised_hover, t.raised)),
            ("staged on card", over(t.staged, t.card)),
            ("new_row on card", over(t.new_row, t.card)),
            ("deleted on card", over(t.deleted, t.card)),
            ("saved on card", over(t.saved, t.card)),
            ("failed on card", over(t.failed, t.card)),
            ("match_other on card", over(t.match_other, t.card)),
        ]
    }

    #[test]
    fn text_and_muted_text_read_on_every_ground() {
        for (mode, t) in both() {
            for (name, g) in grounds(&t) {
                assert!(contrast(t.text, g) >= 4.5, "{mode}: text on {name}: {:.2}", contrast(t.text, g));
                assert!(contrast(t.text_muted, g) >= 4.5, "{mode}: muted text on {name}: {:.2}", contrast(t.text_muted, g));
            }
        }
    }

    #[test]
    fn field_edges_reach_three_to_one() {
        for (mode, t) in both() {
            for (name, g) in [("card", t.card), ("raised", t.raised)] {
                assert!(contrast(t.line_strong, g) >= 3.0, "{mode}: line_strong on {name}");
            }
        }
    }

    #[test]
    fn the_key_icon_reaches_three_to_one() {
        for (mode, t) in both() {
            for (name, g) in [("card", t.card), ("fill", t.fill), ("selection", t.selection)] {
                assert!(contrast(t.key, g) >= 3.0, "{mode}: key on {name}: {:.2}", contrast(t.key, g));
            }
        }
    }

    #[test]
    fn disabled_text_stays_faint() {
        for (mode, t) in both() {
            for (name, g) in [("bg", t.bg), ("card", t.card), ("fill", t.fill), ("raised", t.raised)] {
                let c = contrast(over(t.text_faint, g), g);
                assert!((1.9..=3.0).contains(&c), "{mode}: faint text on {name}: {c:.2}");
            }
        }
    }

    #[test]
    fn text_on_a_colour_is_readable() {
        for (mode, t) in both() {
            assert!(contrast(t.on_ink, t.ink) >= 4.5, "{mode}: on_ink");
            assert!(contrast(t.on_danger, t.danger) >= 4.5, "{mode}: on_danger");
            for (name, c) in [("danger", t.danger), ("success", t.success)] {
                for (gn, g) in [("bg", t.bg), ("card", t.card)] {
                    assert!(contrast(c, g) >= 4.5, "{mode}: {name} on {gn}: {:.2}", contrast(c, g));
                }
            }
        }
    }

    #[test]
    fn syntax_reads_on_card_and_selection() {
        for (mode, t) in both() {
            for (name, c) in [("keyword", t.syn_keyword), ("string", t.syn_string), ("number", t.syn_number), ("literal", t.syn_literal)] {
                for (gn, g) in [("card", t.card), ("selection", t.selection)] {
                    assert!(contrast(c, g) >= 4.5, "{mode}: {name} on {gn}: {:.2}", contrast(c, g));
                }
            }
        }
    }

    #[test]
    fn every_connection_colour_has_a_readable_text() {
        use dbee_config::ConnectionColor as C;
        let all = [C::Red, C::Orange, C::Yellow, C::Green, C::Blue, C::Purple, C::Gray];
        for c in all {
            let (fill, text) = (connection_color(c).unwrap(), on_connection_color(c).unwrap());
            assert!(contrast(text, fill) >= 4.5, "{c:?}: {:.2}", contrast(text, fill));
        }
        for c in [C::Red, C::Blue, C::Purple] {
            assert_eq!(on_connection_color(c), Some(LIGHT.on_colour), "{c:?}");
        }
        for c in [C::Orange, C::Yellow, C::Green, C::Gray] {
            assert_eq!(on_connection_color(c), Some(LIGHT.on_colour_dark), "{c:?}");
        }
        assert_eq!((connection_color(C::None), on_connection_color(C::None)), (None, None));
    }

    #[test]
    fn four_neutrals_per_mode() {
        for (mode, t) in both() {
            assert!(t.bg == t.card && t.card == t.on_ink, "{mode}: white family");
            assert_eq!(t.ink, t.text, "{mode}: black family");
            assert_eq!(t.text_muted, t.line_strong, "{mode}: grey");
            assert!(t.line == t.fill && t.fill == t.selection && t.selection == t.text_selection, "{mode}: tint");
        }
    }

    #[test]
    fn a_state_tint_is_its_meaning_colour() {
        // egui stores a colour premultiplied, so a faint tint loses precision when read back
        // unmultiplied: compare what is stored with the meaning colour at the tint's opacity.
        let close = |tint: Color32, meaning: Color32| {
            let a = f32::from(tint.a()) / 255.0;
            let (t, m) = (tint.to_array(), meaning.to_array());
            (0..3).all(|i| (f32::from(t[i]) - f32::from(m[i]) * a).abs() <= 1.0)
        };
        for (mode, t) in both() {
            assert!(close(t.failed, t.danger) && close(t.deleted, t.danger), "{mode}: danger tints");
            assert!(close(t.new_row, t.success) && close(t.saved, t.success), "{mode}: success tints");
        }
    }
}

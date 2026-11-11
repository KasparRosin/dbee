//! Tooltips (`tooltip`): every hover text in the app (`on_hover_text`, `on_disabled_hover_text`)
//! takes this look from the theme's style ([`apply`], called where the theme is applied), so a
//! call names only its words.
//!
//! What egui decides, and the page draws otherwise: a tooltip sits 4 under the widget it belongs
//! to, its left edge at the widget's (the page: 4 under the pointer, at the pointer), and its
//! words are in the theme's `body` (the page: `small`). egui places and words a hover text
//! itself; matching the page would mean a change at every call.

use egui::{CornerRadius, Frame, Label, Margin, Response, Stroke, Style, Ui};

use crate::theme::size::{RADIUS_FLOAT, TOOLTIP_PAD_X, TOOLTIP_PAD_Y, TOOLTIP_W};
use crate::theme::{Tokens, float_shadow};

/// The tooltip's look in `style`: `raised`, a 1 px `raised_line` edge, corners 4, the one shadow,
/// its words 8 and 4 in from the outside edge, wrapping to keep it at most 320 wide. Its words
/// take the theme's text colour (`text`). Menus and popup lists set their own frame
/// (`menu::style`), so nothing else floats in this one.
pub fn apply(style: &mut Style, t: &Tokens) {
    const EDGE: i8 = 1;
    let v = &mut style.visuals;
    v.window_fill = t.raised;
    v.window_stroke = Stroke::new(f32::from(EDGE), t.raised_line);
    v.menu_corner_radius = CornerRadius::same(RADIUS_FLOAT);
    v.popup_shadow = float_shadow(t);
    // egui puts the edge outside the margin; the page measures its padding from the outside.
    style.spacing.menu_margin = Margin::symmetric(TOOLTIP_PAD_X - EDGE, TOOLTIP_PAD_Y - EDGE);
    style.spacing.tooltip_width = TOOLTIP_W - 2.0 * f32::from(TOOLTIP_PAD_X);
}

/// A tooltip saying `words`, drawn where it stands (the showcase), as egui draws a hover text.
pub fn in_place(ui: &mut Ui, words: &str) -> Response {
    Frame::popup(ui.style())
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.spacing().tooltip_width);
                ui.add(Label::new(words).selectable(false));
            });
        })
        .response
}

#[cfg(test)]
mod tests {
    use egui::{Align, Button, CornerRadius, FontFamily, Layout, Rect, Sense, vec2};

    use super::*;
    use crate::components::{ROOM, both, flat, harness};
    use crate::theme::Text;

    const ONE: &str = "New query tab (Ctrl+T)";
    const WRAPPED: &str = "Next page: not now, changes are staged. Apply or reset them first, then go to the next page.";

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// The tooltip egui shows for a button hovered in `theme`: the button, the tooltip's frame,
    /// its shadow and its words (where, the font, the colour).
    struct Seen {
        button: Rect,
        frame: egui::epaint::RectShape,
        shadow: Option<egui::epaint::RectShape>,
        words: Vec<(Rect, egui::FontId, egui::Color32)>,
    }

    fn hovered(theme: egui::Theme, t: &Tokens, words: &'static str) -> Seen {
        let mut h = harness(theme, Rect::NOTHING, move |ui, at| {
            ui.add_space(ROOM);
            ui.horizontal(|ui| {
                ui.add_space(ROOM);
                *at = ui.add(Button::new("Hover me").sense(Sense::click())).on_hover_text(words).rect;
            });
        });
        h.run();
        let button = *h.state();
        h.hover_at(button.center());
        h.run();
        h.run();
        let shapes = flat(&h.output().shapes);
        let mut words_drawn = Vec::new();
        for s in &shapes {
            if let egui::Shape::Text(text) = s
                && text.galley.text() == words
            {
                for row in &text.galley.rows {
                    let rect = row.rect().translate(text.pos.to_vec2());
                    let format = &text.galley.job.sections[0].format;
                    let colour = if format.color == egui::Color32::PLACEHOLDER { text.fallback_color } else { format.color };
                    words_drawn.push((rect, format.font_id.clone(), text.override_text_color.unwrap_or(colour)));
                }
            }
        }
        let first = words_drawn.first().map_or(Rect::NOTHING, |(r, _, _)| *r);
        let frame = shapes
            .iter()
            .rev()
            .find_map(|s| match s {
                egui::Shape::Rect(r) if r.blur_width == 0.0 && r.fill == t.raised && r.rect.contains_rect(first) => Some(r.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{theme:?}: the tooltip's frame round {first:?}"));
        let shadow = shapes.iter().find_map(|s| match s {
            egui::Shape::Rect(r) if r.blur_width > 0.0 && close(r.rect.left(), frame.rect.left()) => Some(r.clone()),
            _ => None,
        });
        Seen { button, frame, shadow, words: words_drawn }
    }

    #[test]
    fn a_tooltip_matches_the_page() {
        // Node 162737: `raised`, a 1 px `raised_line` edge inside, corners 4, the shadow (0 6,
        // blur 18); the words 8 in from the outside edge and 4 down, in `text`; one line hugs its
        // words, a long text wraps to keep the tooltip at most 320 wide.
        for (theme, t) in both() {
            for words in [ONE, WRAPPED] {
                let at = format!("{theme:?} {words:?}");
                let seen = hovered(theme, &t, words);
                let frame = seen.frame.rect;
                assert_eq!((seen.frame.stroke.width, seen.frame.stroke.color), (1.0, t.raised_line), "{at}: the edge");
                assert_eq!(seen.frame.corner_radius, CornerRadius::same(4), "{at}: the corners");
                let shadow = seen.shadow.unwrap_or_else(|| panic!("{at}: the shadow"));
                assert_eq!((shadow.blur_width, shadow.fill), (18.0, t.shadow), "{at}: the shadow's blur and colour");
                assert!(close(shadow.rect.top(), frame.top() + 6.0), "{at}: the shadow 6 down: {:?} under {frame:?}", shadow.rect);
                let rows: Vec<Rect> = seen.words.iter().map(|(r, _, _)| *r).collect();
                assert!(!rows.is_empty(), "{at}: the words drawn");
                for (r, font, colour) in &seen.words {
                    assert_eq!(*colour, t.text, "{at}: the words in text");
                    // egui words a hover text in the theme's `body`, not the page's `small`.
                    assert_eq!((font.family.clone(), font.size), (FontFamily::Proportional, Text::Body.font().size), "{at}: {r:?}");
                }
                let words_box = rows.iter().fold(Rect::NOTHING, |a, r| a.union(*r));
                let outside = match seen.frame.stroke_kind {
                    egui::StrokeKind::Inside => frame,
                    egui::StrokeKind::Middle => frame.expand(seen.frame.stroke.width / 2.0),
                    egui::StrokeKind::Outside => frame.expand(seen.frame.stroke.width),
                };
                assert!(close(words_box.left() - outside.left(), 8.0), "{at}: 8 in: {words_box:?} in {outside:?}");
                assert!(close(words_box.top() - outside.top(), 4.0), "{at}: 4 down: {words_box:?} in {outside:?}");
                assert!(close(outside.right() - words_box.right(), 8.0) || words == WRAPPED, "{at}: one line hugs its words");
                assert!(close(outside.bottom() - words_box.bottom(), 4.0), "{at}: 4 under the words");
                if words == WRAPPED {
                    assert!(rows.len() >= 2, "{at}: wrapped: {rows:?}");
                    assert!(outside.width() <= 320.0 + 0.5, "{at}: at most 320 wide: {outside:?}");
                    assert!(outside.width() > 280.0, "{at}: wrapped near 320: {outside:?}");
                } else {
                    assert_eq!(rows.len(), 1, "{at}: one line");
                }
                // egui's place for it: 4 under the widget, its left edge at the widget's.
                assert!(
                    close(outside.top(), seen.button.bottom() + 4.0) && close(outside.left(), seen.button.left()),
                    "{at}: {outside:?} under {:?}",
                    seen.button
                );
            }
        }
    }

    #[test]
    fn a_tooltip_in_place_is_the_hover_texts_frame() {
        // The showcase's tooltip is egui's: the same frame, edge and words.
        for (theme, t) in both() {
            let mut h = harness(theme, Rect::NOTHING, |ui, at| {
                ui.add_space(ROOM);
                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                    ui.add_space(ROOM);
                    *at = in_place(ui, ONE).rect;
                });
            });
            h.run();
            let at = *h.state();
            let shapes = flat(&h.output().shapes);
            let frame = shapes.iter().find_map(|s| match s {
                egui::Shape::Rect(r) if r.rect == at && r.fill == t.raised => Some(r.clone()),
                _ => None,
            });
            let frame = frame.unwrap_or_else(|| panic!("{theme:?}: the frame at {at:?}"));
            assert_eq!((frame.stroke.width, frame.stroke.color, frame.corner_radius), (1.0, t.raised_line, CornerRadius::same(4)), "{theme:?}");
            assert!(at.height() < 30.0 && at.width() > 100.0, "{theme:?}: one line: {:?}", vec2(at.width(), at.height()));
        }
    }
}

//! Tooltips (`tooltip`): every hover text in the app goes through [`Hint`], which words it in
//! `small` and puts it 4 below the pointer, its left edge at the pointer. Its frame comes from the
//! theme's style ([`apply`], called where the theme is applied), so a call names only its words.

use egui::{CornerRadius, Frame, Label, Margin, Response, Stroke, Style, Tooltip, Ui};

use crate::theme::size::{RADIUS_FLOAT, TOOLTIP_GAP, TOOLTIP_PAD_X, TOOLTIP_PAD_Y, TOOLTIP_W};
use crate::theme::{Text, Tokens, float_shadow};

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

/// A widget's hover text, the one way the app shows one.
pub trait Hint {
    /// `words` while the pointer rests on it, if it is on.
    fn hint(self, words: impl Into<String>) -> Response;
    /// `words` while the pointer rests on it, if it is off: why.
    fn hint_disabled(self, words: impl Into<String>) -> Response;
}

impl Hint for Response {
    fn hint(self, words: impl Into<String>) -> Response {
        show(Tooltip::for_enabled(&self), words);
        self
    }

    fn hint_disabled(self, words: impl Into<String>) -> Response {
        show(Tooltip::for_disabled(&self), words);
        self
    }
}

/// `tooltip`, if it is to show, at the pointer, saying `words`. egui's own at-pointer tooltip sits
/// 12 off the pointer; the page's sits 4 below it.
fn show(tooltip: Tooltip<'_>, words: impl Into<String>) {
    tooltip.at_pointer().gap(TOOLTIP_GAP).show(|ui| {
        // As egui's hover text: the area's size doesn't shrink a tooltip whose words change.
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(label(words));
    });
}

/// The tooltip's words: `small`, as written (they can name a table or a column).
fn label(words: impl Into<String>) -> Label {
    Label::new(Text::Small.rich_as_written(words)).selectable(false)
}

/// A tooltip saying `words`, drawn where it stands (the showcase), as [`Hint`] draws one.
pub fn in_place(ui: &mut Ui, words: &str) -> Response {
    Frame::popup(ui.style())
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.spacing().tooltip_width);
                ui.add(label(words));
            });
        })
        .response
}

#[cfg(test)]
mod tests {
    use egui::{Align, Button, CornerRadius, Layout, Rect, Sense, vec2};

    use super::*;
    use crate::components::{ROOM, both, flat, harness};
    use crate::theme::Text;

    const ONE: &str = "New query tab (Ctrl+T)";
    const WRAPPED: &str = "Next page: not now, changes are staged. Apply or reset them first, then go to the next page.";

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// Where the pointer rests on the button, from its top left: off its centre and its left
    /// edge, so a tooltip placed by the widget shows apart from one placed by the pointer.
    const REST: egui::Vec2 = vec2(37.0, 6.0);

    /// The tooltip shown for a button hovered in `theme`, on (`hint`) or off (`hint_disabled`):
    /// the button, where the pointer rests, the tooltip's frame, its shadow and its words (where,
    /// the font, the colour).
    struct Seen {
        pointer: egui::Pos2,
        frame: egui::epaint::RectShape,
        shadow: Option<egui::epaint::RectShape>,
        words: Vec<(Rect, egui::FontId, egui::Color32)>,
    }

    fn hovered(theme: egui::Theme, t: &Tokens, words: &'static str, off: bool) -> Seen {
        let mut h = harness(theme, Rect::NOTHING, move |ui, at| {
            ui.add_space(ROOM);
            ui.horizontal(|ui| {
                ui.add_space(ROOM);
                let r = ui.add_enabled(!off, Button::new("Hover me, a wide button").sense(Sense::click()));
                *at = if off { r.hint_disabled(words) } else { r.hint(words) }.rect;
            });
        });
        h.run();
        let pointer = h.state().min + REST;
        h.hover_at(pointer);
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
        Seen { pointer, frame, shadow, words: words_drawn }
    }

    /// The outside of the tooltip's frame, its edge included.
    fn outside(frame: &egui::epaint::RectShape) -> Rect {
        match frame.stroke_kind {
            egui::StrokeKind::Inside => frame.rect,
            egui::StrokeKind::Middle => frame.rect.expand(frame.stroke.width / 2.0),
            egui::StrokeKind::Outside => frame.rect.expand(frame.stroke.width),
        }
    }

    #[test]
    fn a_tooltip_matches_the_page() {
        // Node 162737: `raised`, a 1 px `raised_line` edge inside, corners 4, the shadow (0 6,
        // the page's blur 18, egui's 36); the words 8 in from the outside edge and 4 down, in
        // `text`; one line hugs its words, a long text wraps to keep the tooltip at most 320 wide.
        for (theme, t) in both() {
            for words in [ONE, WRAPPED] {
                let at = format!("{theme:?} {words:?}");
                let seen = hovered(theme, &t, words, false);
                let frame = seen.frame.rect;
                assert_eq!((seen.frame.stroke.width, seen.frame.stroke.color), (1.0, t.raised_line), "{at}: the edge");
                assert_eq!(seen.frame.corner_radius, CornerRadius::same(4), "{at}: the corners");
                let shadow = seen.shadow.unwrap_or_else(|| panic!("{at}: the shadow"));
                assert_eq!((shadow.blur_width, shadow.fill), (f32::from(float_shadow(&t).blur), t.shadow), "{at}: the shadow's blur and colour");
                assert!(close(shadow.rect.top(), frame.top() + 6.0), "{at}: the shadow 6 down: {:?} under {frame:?}", shadow.rect);
                let rows: Vec<Rect> = seen.words.iter().map(|(r, _, _)| *r).collect();
                assert!(!rows.is_empty(), "{at}: the words drawn");
                for (_, _, colour) in &seen.words {
                    assert_eq!(*colour, t.text, "{at}: the words in text");
                }
                let words_box = rows.iter().fold(Rect::NOTHING, |a, r| a.union(*r));
                let outside = outside(&seen.frame);
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
            }
        }
    }

    #[test]
    fn a_tooltip_is_small_text_at_the_pointer() {
        // Node 162737: the words in `small` (Inter 12, a 16 line) in `text`, one line 24 tall; the
        // tooltip 4 below the pointer, its left edge at the pointer. A widget that is off says
        // why the same way.
        for (theme, t) in both() {
            for off in [false, true] {
                let at = format!("{theme:?} off {off}");
                let seen = hovered(theme, &t, ONE, off);
                assert!(!seen.words.is_empty(), "{at}: the words drawn");
                for (r, font, colour) in &seen.words {
                    assert_eq!(*font, Text::Small.font(), "{at}: the words in small: {r:?}");
                    assert_eq!(*colour, t.text, "{at}: the words in text");
                    assert!(close(r.height(), Text::Small.line()), "{at}: a 16 line: {r:?}");
                }
                let outside = outside(&seen.frame);
                assert!(close(outside.height(), 24.0), "{at}: one line is 24 tall: {outside:?}");
                assert!(
                    close(outside.top(), seen.pointer.y + 4.0) && close(outside.left(), seen.pointer.x),
                    "{at}: {outside:?} 4 below the pointer at {:?}, its left edge there",
                    seen.pointer
                );
            }
        }
    }

    #[test]
    fn no_hover_text_bypasses_the_hint() {
        // Every hover text goes through `Hint`, so each one is `small` at the pointer.
        const BYPASSES: [&str; 8] = [
            ".on_hover_text(",
            ".on_disabled_hover_text(",
            ".on_hover_text_at_pointer(",
            ".on_hover_ui(",
            ".on_disabled_hover_ui(",
            ".on_hover_ui_at_pointer(",
            ".show_tooltip_text(",
            ".show_tooltip_ui(",
        ];
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["src", "tests", "examples"] {
            walk(&root.join(dir), &mut files);
        }
        assert!(files.len() > 20, "the crate's files: {files:?}");
        let this = root.join("src/components/tooltip.rs");
        let mut found = Vec::new();
        for file in files.iter().filter(|f| **f != this) {
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if BYPASSES.iter().any(|b| line.contains(b)) {
                    found.push(format!("{}:{}: {}", file.strip_prefix(root).unwrap().display(), n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "hover texts outside `Hint`:\n{}", found.join("\n"));
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

//! The strip (`strip`): a result or a note across the width it is given, as the connection form
//! shows `Test passed: …` or `No databases to list.` above its bar. On `card` with a 1 px `line`
//! along its top and a 2 px edge down its left (`success` for a result, `line_strong` for a
//! note), the words in `body` `text` [`INSET`] past the edge, [`STRIP_PAD_Y`] above and below;
//! they can be selected. It shows at most [`STRIP_MAX_LINES`] lines; more scroll inside it.
//! An error is not a strip: it is the error band ([`super::error_band`]).

use egui::{Align, CornerRadius, Id, Label, Layout, Rect, Response, ScrollArea, Sense, Ui, UiBuilder, pos2, vec2};

use crate::theme::size::{BAR_LINE, INSET, STRIP_EDGE, STRIP_MAX_LINES, STRIP_PAD_Y};
use crate::theme::{Text, tokens};

/// What a strip says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something worked (a test passed, a save): a `success` edge.
    Success,
    /// Something to know: a `line_strong` edge.
    Neutral,
}

/// One strip: its tone and its words, as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strip<'a> {
    pub tone: Tone,
    pub text: &'a str,
}

/// The height of `lines` of `text` wrapped to `width`: all of them when there are no more than
/// [`STRIP_MAX_LINES`], else that many lines' worth.
pub fn lines_h(ui: &Ui, text: Text, words: &str, width: f32) -> f32 {
    let mut job = text.job_as_written(words, tokens(ui.ctx()).text);
    job.wrap.max_width = width.max(1.0);
    let galley = ui.painter().layout_job(job);
    if galley.rows.len() <= STRIP_MAX_LINES { galley.size().y } else { STRIP_MAX_LINES as f32 * text.line() }
}

impl Strip<'_> {
    /// The words' room in a strip `width` wide.
    fn words_w(width: f32) -> f32 {
        width - STRIP_EDGE - 2.0 * INSET
    }

    /// How tall the strip is across `width`.
    pub fn height(&self, ui: &Ui, width: f32) -> f32 {
        BAR_LINE + 2.0 * STRIP_PAD_Y + lines_h(ui, Text::Body, self.text, Self::words_w(width))
    }

    /// Draws the strip across what is left of `ui`'s width, [`Strip::height`] tall; its response
    /// covers it.
    pub fn show(&self, ui: &mut Ui, id: Id) -> Response {
        let t = tokens(ui.ctx());
        let width = ui.available_width();
        let (rect, r) = ui.allocate_exact_size(vec2(width, self.height(ui, width)), Sense::hover());
        let edge = match self.tone {
            Tone::Success => t.success,
            Tone::Neutral => t.line_strong,
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, t.card);
        p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), BAR_LINE)), CornerRadius::ZERO, t.line);
        p.rect_filled(
            Rect::from_min_max(pos2(rect.left(), rect.top() + BAR_LINE), pos2(rect.left() + STRIP_EDGE, rect.bottom())),
            CornerRadius::ZERO,
            edge,
        );
        let left = rect.left() + STRIP_EDGE + INSET;
        let words = Rect::from_min_max(
            pos2(left, rect.top() + BAR_LINE + STRIP_PAD_Y),
            pos2((rect.right() - INSET).max(left + 1.0), rect.bottom() - STRIP_PAD_Y),
        );
        let mut inner = ui.new_child(UiBuilder::new().id_salt(id.with("words")).max_rect(words).layout(Layout::top_down(Align::Min)));
        ScrollArea::vertical().id_salt(id.with("scroll")).max_height(words.height()).auto_shrink([false, true]).show(&mut inner, |ui| {
            ui.add(Label::new(Text::Body.rich_as_written(self.text).color(t.text)).wrap().selectable(true));
        });
        r
    }
}

#[cfg(test)]
mod tests {
    use egui::{Theme, UiBuilder};

    use super::*;
    use crate::components::{both, fills, painted_in, texts};

    const W: f32 = 600.0;

    fn draw(theme: Theme, strip: Strip<'_>) -> (Vec<egui::epaint::ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted_in(vec2(W + 40.0, 300.0), theme, |ui| {
            let room = Rect::from_min_size(pos2(10.0, 10.0), vec2(W, 280.0));
            ui.scope_builder(UiBuilder::new().max_rect(room), |ui| at = strip.show(ui, Id::new("strip")).rect);
        });
        (shapes, at)
    }

    #[test]
    fn a_strip_draws_its_edge_and_words() {
        for (theme, t) in both() {
            for (tone, colour) in [(Tone::Success, t.success), (Tone::Neutral, t.line_strong)] {
                let words = "Test passed: PostgreSQL 14.12.";
                let (shapes, r) = draw(theme, Strip { tone, text: words });
                assert!((r.width() - W).abs() < 0.5, "{theme:?} {tone:?}: across the width: {r:?}");
                let fills: Vec<_> = fills(&shapes).into_iter().filter(|(f, _)| r.expand(0.5).contains_rect(*f)).collect();
                let edge = Rect::from_min_max(pos2(r.left(), r.top() + 1.0), pos2(r.left() + 2.0, r.bottom()));
                assert!(fills.contains(&(edge, colour)), "{theme:?} {tone:?}: the 2 px edge: {fills:?}");
                let line = Rect::from_min_size(r.min, vec2(r.width(), 1.0));
                assert!(fills.contains(&(line, t.line)), "{theme:?} {tone:?}: the line on top: {fills:?}");
                for (f, c) in &fills {
                    assert!([t.card, t.line, colour].contains(c), "{theme:?} {tone:?}: only `card` under it: {f:?} {c:?}");
                }
                let (_, at, ink) = texts(&shapes).into_iter().find(|(w, _, _)| w == words).unwrap_or_else(|| panic!("{theme:?}: the words"));
                assert_eq!(ink, t.text, "{theme:?} {tone:?}: the words in `text`");
                assert!((at.left() - (r.left() + 2.0 + 12.0)).abs() < 0.5, "{theme:?} {tone:?}: 12 past the edge: {at:?} {r:?}");
                assert!((at.top() - (r.top() + 1.0 + 8.0)).abs() < 0.5, "{theme:?} {tone:?}: 8 under the line: {at:?} {r:?}");
                assert!((r.bottom() - at.bottom() - 8.0).abs() < 0.5, "{theme:?} {tone:?}: 8 under the words: {at:?} {r:?}");
            }
        }
    }

    #[test]
    fn a_long_strip_shows_three_lines() {
        let long = "a long message that wraps ".repeat(60);
        let (_, r) = draw(Theme::Light, Strip { tone: Tone::Neutral, text: &long });
        let three = 1.0 + 2.0 * 8.0 + 3.0 * Text::Body.line();
        assert!((r.height() - three).abs() < 0.5, "three lines, then it scrolls: {r:?}");
    }
}

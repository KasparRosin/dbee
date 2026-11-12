//! The SQL editor's parts from the page: a line's number in the gutter (`editor/line number`) and
//! the mark under the token an error points at (`editor/error token`).
//!
//! A line number is `code`, flush with the right of its box (on the page [`LINE_NUMBER_W`] wide) and
//! centred down it: `text_muted`, the cursor's line in `text`, the error's line in `danger` (the
//! error wins). An error token is its text in `code` with a 1.5 px `danger` line 17 down its
//! 20 tall row, as wide as the token.
//!
//! [`LINE_NUMBER_W`]: crate::theme::size::LINE_NUMBER_W

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Ui, pos2, vec2};

use crate::theme::size::{ERROR_UNDERLINE_H, ERROR_UNDERLINE_Y};
use crate::theme::{Text, Tokens, tokens};

/// One line's number in the gutter: its number (1-based), whether the cursor is on it, and
/// whether the error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineNumber {
    pub n: usize,
    pub current: bool,
    pub error: bool,
}

impl LineNumber {
    /// Its colour: `danger` on the error's line, `text` on the cursor's, else `text_muted`.
    pub fn colour(self, t: &Tokens) -> Color32 {
        if self.error {
            t.danger
        } else if self.current {
            t.text
        } else {
            t.text_muted
        }
    }

    /// Draws the number flush with the right of `slot` (a line's box in the gutter), centred down
    /// it.
    pub fn paint(self, painter: &Painter, slot: Rect) {
        let colour = self.colour(tokens(painter.ctx()));
        let number = painter.layout_job(Text::Code.job_as_written(&self.n.to_string(), colour));
        let size = number.size();
        painter.galley(pos2(slot.right() - size.x, slot.center().y - size.y / 2.0), number, colour);
    }

    /// How wide the numbers of `lines` lines get: the digits of the last, at least two.
    pub fn room(ui: &Ui, lines: usize) -> f32 {
        let digits = lines.max(1).ilog10().saturating_add(1).max(2);
        let digit = ui.fonts_mut(|f| f.glyph_width(&Text::Code.font(), '0'));
        digits as f32 * digit
    }
}

/// The token an error points at, as the page draws it: its text, underlined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorToken<'a> {
    pub token: &'a str,
}

impl ErrorToken<'_> {
    /// Draws the token in `code` `text` with its row's top left at `at`, and its underline; the
    /// token's box.
    pub fn paint(&self, painter: &Painter, at: Pos2) -> Rect {
        let ink = tokens(painter.ctx()).text;
        let galley = painter.layout_job(Text::Code.job_as_written(self.token, ink));
        let token = Rect::from_min_size(at, galley.size());
        painter.galley(at, galley, ink);
        ErrorToken::underline(painter, token);
        token
    }

    /// The underline under `token`, a token's box on an editor line (its row's top and height):
    /// for the editor, whose text is drawn by its text edit.
    pub fn underline(painter: &Painter, token: Rect) {
        let line = Rect::from_min_size(pos2(token.left(), token.top() + ERROR_UNDERLINE_Y), vec2(token.width(), ERROR_UNDERLINE_H));
        painter.rect_filled(line, CornerRadius::ZERO, tokens(painter.ctx()).danger);
    }
}

#[cfg(test)]
mod tests {
    use egui::{FontFamily, pos2, vec2};

    use super::*;
    use crate::components::{both, fills, painted, sections, texts};
    use crate::theme::Text;
    use crate::theme::size::LINE_NUMBER_W;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn line_numbers_match_the_page() {
        // Node 168934: default, current, error, each 36 by 20, the number flush right; and line 12
        // with the cursor and the error on it.
        let numbers = [
            LineNumber { n: 1, current: false, error: false },
            LineNumber { n: 2, current: true, error: false },
            LineNumber { n: 3, current: false, error: true },
            LineNumber { n: 12, current: true, error: true },
        ];
        for (theme, t) in both() {
            let slots: Vec<Rect> =
                (0..numbers.len()).map(|i| Rect::from_min_size(pos2(20.0 + 56.0 * i as f32, 20.0), vec2(LINE_NUMBER_W, 20.0))).collect();
            let mut rooms = [0.0; 4];
            let shapes = painted(theme, |ui| {
                for (number, slot) in numbers.iter().zip(&slots) {
                    number.paint(ui.painter(), *slot);
                }
                rooms = [9, 99, 100, 12_345].map(|lines| LineNumber::room(ui, lines));
            });
            let seen = texts(&shapes);
            for ((number, slot), colour) in numbers.iter().zip(&slots).zip([t.text_muted, t.text, t.danger, t.danger]) {
                let words = number.n.to_string();
                let (_, r, c) = seen.iter().find(|(w, _, _)| *w == words).unwrap_or_else(|| panic!("{theme:?}: {words} drawn: {seen:?}"));
                assert_eq!(*c, colour, "{theme:?} {number:?}");
                assert_eq!(number.colour(&t), colour, "{theme:?} {number:?}");
                assert!(
                    close(r.right(), slot.right()) && close(r.top(), slot.top()) && close(r.height(), 20.0),
                    "{theme:?} {number:?}: {r:?} in {slot:?}"
                );
                let font = sections(&shapes, &words).first().map(|s| s.1.clone());
                assert_eq!(font, Some(Text::Code.font()), "{theme:?} {number:?}: `code`");
            }
            assert_eq!(Text::Code.font().family, FontFamily::Monospace);
            let digit = rooms[2] / 3.0;
            assert!(
                digit > 7.0 && close(rooms[0], 2.0 * digit) && close(rooms[1], 2.0 * digit) && close(rooms[3], 5.0 * digit),
                "{theme:?}: {rooms:?}"
            );
        }
    }

    #[test]
    fn the_error_token_matches_the_page() {
        // Node 168939: `o.totl` in `code` `text`, a 1.5 `danger` line 17 down its 20 tall row,
        // as wide as the token.
        for (theme, t) in both() {
            let at = pos2(10.0, 10.0);
            let mut token = Rect::NOTHING;
            let lone = Rect::from_min_size(pos2(100.0, 50.0), vec2(30.0, 20.0));
            let shapes = painted(theme, |ui| {
                token = ErrorToken { token: "o.totl" }.paint(ui.painter(), at);
                ErrorToken::underline(ui.painter(), lone);
            });
            let seen = texts(&shapes);
            let (_, r, c) = seen.iter().find(|(w, _, _)| w == "o.totl").unwrap_or_else(|| panic!("{theme:?}: the token drawn: {seen:?}"));
            assert_eq!(*c, t.text, "{theme:?}");
            assert_eq!(sections(&shapes, "o.totl").first().map(|s| s.1.clone()), Some(Text::Code.font()), "{theme:?}");
            assert!(close(r.left(), at.x) && close(r.top(), at.y) && close(r.height(), 20.0), "{theme:?}: {r:?}");
            assert_eq!(token, *r, "{theme:?}: its box");
            let fills = fills(&shapes);
            let line = Rect::from_min_size(pos2(r.left(), r.top() + 17.0), vec2(r.width(), 1.5));
            assert!(fills.contains(&(line, t.danger)), "{theme:?}: the underline {line:?}: {fills:?}");
            let alone = Rect::from_min_size(pos2(lone.left(), lone.top() + 17.0), vec2(lone.width(), 1.5));
            assert!(fills.contains(&(alone, t.danger)), "{theme:?}: the underline alone {alone:?}: {fills:?}");
            assert_eq!(fills.iter().filter(|(_, c)| *c == t.danger).count(), 2, "{theme:?}: nothing else: {fills:?}");
        }
    }
}

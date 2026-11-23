//! The one grey phrase of an area with nothing in it (README 209): `body` in `text_muted`, said
//! only where no bar already says why the area is empty (`0 ROWS`, `0 TABLES`, `LOADING`, the
//! result bar's facts). Every screen's phrase is drawn here: the table tab's `No rows are
//! loaded.`, the tree's no-schemas sentence, the connection list's no-match line ([`Place::Top`])
//! and the value panel's `No cell selected` ([`Place::Centre`]).

use egui::{Align, Rect, Response, Sense, Ui, pos2, vec2};

use super::painter;
use crate::theme::size::EMPTY_INSET;
use crate::theme::{Text, tokens};

/// Where in its area a phrase sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// [`EMPTY_INSET`] in from the area's left and down from its top.
    Top,
    /// Centred both ways.
    Centre,
}

/// One short grey phrase in an empty area; its words as written.
#[derive(Clone, Copy, Debug)]
pub struct Phrase<'a> {
    text: &'a str,
    place: Place,
}

impl<'a> Phrase<'a> {
    pub fn new(text: &'a str, place: Place) -> Phrase<'a> {
        Phrase { text, place }
    }

    /// Draws it in `area`: `body` in `text_muted`, wrapping within `area` less [`EMPTY_INSET`]
    /// each side; [`Place::Top`] from the inset in and down, [`Place::Centre`] centred both ways.
    /// Takes no room and no click: the response (hover only) is the words' rect, for the
    /// screen's tag.
    pub fn show(self, ui: &mut Ui, area: Rect) -> Response {
        let colour = tokens(ui.ctx()).text_muted;
        let mut job = Text::Body.job_as_written(self.text, colour);
        job.wrap.max_width = (area.width() - 2.0 * EMPTY_INSET).max(0.0);
        if self.place == Place::Centre {
            job.halign = Align::Center;
        }
        let p = painter(ui).with_clip_rect(area.intersect(ui.clip_rect()));
        let galley = p.layout_job(job);
        let at = match self.place {
            Place::Top => area.min + vec2(EMPTY_INSET, EMPTY_INSET),
            Place::Centre => pos2(area.center().x, area.center().y - galley.size().y / 2.0),
        };
        let rect = galley.rect.translate(at.to_vec2());
        p.galley(at, galley, colour);
        ui.interact(rect, ui.id().with("dbee-empty-phrase"), Sense::hover())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{both, flat, painted_in, texts};

    /// The phrase's painted rows in the last frame: each row's rect, the colour and the font.
    fn drawn(shapes: &[egui::epaint::ClippedShape], first: &str) -> Option<(Vec<Rect>, egui::Color32, egui::FontId)> {
        let colour = texts(shapes).into_iter().find(|(w, _, _)| w.starts_with(first))?.2;
        flat(shapes).into_iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text().starts_with(first) => {
                let rows = t.galley.rows.iter().map(|r| r.rect().translate(t.pos.to_vec2())).collect();
                Some((rows, colour, t.galley.job.sections.first()?.format.font_id.clone()))
            }
            _ => None,
        })
    }

    fn shown(theme: egui::Theme, area: Rect, text: &'static str, place: Place) -> (Vec<egui::epaint::ClippedShape>, Rect) {
        let mut got = Rect::NOTHING;
        let shapes = painted_in(vec2(600.0, 400.0), theme, |ui| got = Phrase::new(text, place).show(ui, area).rect);
        (shapes, got)
    }

    #[test]
    fn a_phrase_at_the_top_starts_12_in_and_12_down() {
        let area = Rect::from_min_size(pos2(40.0, 30.0), vec2(400.0, 200.0));
        for (theme, t) in both() {
            let (shapes, r) = shown(theme, area, "No rows are loaded.", Place::Top);
            let (rows, colour, font) = drawn(&shapes, "No rows").unwrap_or_else(|| panic!("{theme:?}: the phrase is drawn"));
            assert_eq!(rows.len(), 1, "{theme:?}: one line");
            assert!((rows[0].left() - 52.0).abs() < 0.5 && (rows[0].top() - 42.0).abs() < 0.5, "{theme:?}: 12 in and 12 down: {rows:?}");
            assert_eq!(colour, t.text_muted, "{theme:?}");
            assert_eq!(font, Text::Body.font(), "{theme:?}");
            assert!((r.left() - 52.0).abs() < 0.5 && (r.top() - 42.0).abs() < 0.5, "{theme:?}: the response is the words' rect: {r:?}");
        }
    }

    #[test]
    fn a_centred_phrase_sits_in_the_middle() {
        let area = Rect::from_min_size(pos2(100.0, 50.0), vec2(300.0, 240.0));
        for (theme, t) in both() {
            let (shapes, _) = shown(theme, area, "No cell selected", Place::Centre);
            let (rows, colour, font) = drawn(&shapes, "No cell selected").unwrap_or_else(|| panic!("{theme:?}: the phrase is drawn"));
            let c = rows[0].center();
            assert!((c.x - area.center().x).abs() <= 1.0 && (c.y - area.center().y).abs() <= 1.0, "{theme:?}: centred: {c:?}");
            assert_eq!(colour, t.text_muted, "{theme:?}");
            assert_eq!(font, Text::Body.font(), "{theme:?}");
        }
    }

    #[test]
    fn a_long_phrase_wraps_inside_the_insets() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 300.0));
        let words = "This connection has no schemas dbee can show.";
        for (theme, _) in both() {
            for place in [Place::Top, Place::Centre] {
                let (shapes, _) = shown(theme, area, words, place);
                let (rows, _, _) = drawn(&shapes, "This").unwrap_or_else(|| panic!("{theme:?} {place:?}: the phrase is drawn"));
                assert!(rows.len() >= 2, "{theme:?} {place:?}: wraps: {rows:?}");
                for r in &rows {
                    assert!(r.left() >= 12.0 - 0.5 && r.right() <= 148.0 + 0.5, "{theme:?} {place:?}: inside the insets: {r:?}");
                }
            }
        }
    }
}

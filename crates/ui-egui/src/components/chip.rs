//! A short fact about a card (`Upcoming`, `Available`): page component `chip`.

use egui::{CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::painter;
use crate::theme::size::{CHIP_GAP, CHIP_H, CHIP_MARK, CHIP_PAD_X};
use crate::theme::{Text, tokens};

#[derive(Clone, Copy)]
enum Kind {
    Next,
    Available,
}

/// A chip: its words in capitals, as written to accessibility.
pub struct Chip<'a> {
    text: &'a str,
    kind: Kind,
}

impl<'a> Chip<'a> {
    /// What comes later: grey words in a `line` edge.
    pub fn next(text: &'a str) -> Chip<'a> {
        Chip { text, kind: Kind::Next }
    }

    /// What is there now: a `success` mark and the words in a `line_strong` edge.
    pub fn available(text: &'a str) -> Chip<'a> {
        Chip { text, kind: Kind::Available }
    }
}

impl Widget for Chip<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let t = tokens(ui.ctx());
        let (edge, words, mark) = match self.kind {
            Kind::Next => (t.line, t.text_muted, None),
            Kind::Available => (t.line_strong, t.text, Some(t.success)),
        };
        let galley = ui.painter().layout_job(Text::Label.job(self.text, words));
        let inset = 1.0 + CHIP_PAD_X;
        let before = if mark.is_some() { CHIP_MARK + CHIP_GAP } else { 0.0 };
        let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 2.0 * inset + before, CHIP_H), Sense::hover());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), self.text));
        if ui.is_rect_visible(rect) {
            let p = painter(ui);
            p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, edge), StrokeKind::Inside);
            if let Some(colour) = mark {
                let at = pos2(rect.left() + inset, rect.center().y - CHIP_MARK / 2.0);
                p.rect_filled(Rect::from_min_size(at, vec2(CHIP_MARK, CHIP_MARK)), CornerRadius::ZERO, colour);
            }
            p.galley(pos2(rect.left() + inset + before, rect.center().y - galley.size().y / 2.0), galley, words);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{showcase, texts};

    #[test]
    fn a_chip_reports_its_words_as_written() {
        let mut ready = false;
        let h = Harness::new_ui(|ui| {
            if !ready {
                showcase::prepare(ui.ctx(), egui::Theme::Light);
                ready = true;
                ui.ctx().request_repaint();
                return;
            }
            ui.add(Chip::next("Upcoming"));
        });
        assert!(h.query_by_role_and_label(egui::accesskit::Role::Label, "Upcoming").is_some(), "a label, as written");
        assert!(h.query_by_label("UPCOMING").is_none(), "the capitals are only drawn");
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        assert_eq!(drawn, ["UPCOMING"]);
    }
}

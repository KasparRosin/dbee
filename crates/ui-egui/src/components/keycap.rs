//! A key of a shortcut, as menus show it (`Ctrl`, `Shift`, `Z`): page component `keycap`.

use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::{Look, painter};
use crate::theme::size::{KEYCAP_H, KEYCAP_PAD_X, RADIUS_CONTROL};
use crate::theme::{Text, tokens};

/// One key, drawn in capitals on `raised`; off when added with `ui.add_enabled(false, …)`.
pub struct Keycap<'a> {
    key: &'a str,
    look: Option<Look>,
}

impl<'a> Keycap<'a> {
    pub fn new(key: &'a str) -> Keycap<'a> {
        Keycap { key, look: None }
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Keycap<'a> {
        self.look = Some(look);
        self
    }
}

impl Widget for Keycap<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let galley = ui.painter().layout_job(Text::Label.job(self.key, Color32::PLACEHOLDER));
        let inset = 1.0 + KEYCAP_PAD_X;
        let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 2.0 * inset, KEYCAP_H), Sense::hover());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), self.key));
        if ui.is_rect_visible(rect) {
            let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
            let t = tokens(ui.ctx());
            let p = painter(ui);
            p.rect(rect, CornerRadius::same(RADIUS_CONTROL), t.raised, Stroke::new(1.0, t.raised_line), StrokeKind::Inside);
            let colour = if look.disabled { t.text_faint } else { t.text_muted };
            p.galley(pos2(rect.left() + inset, rect.center().y - galley.size().y / 2.0), galley, colour);
        }
        r
    }
}

//! A key of a shortcut, as menus show it (`Ctrl`, `Shift`, `Z`): page component `keycap`.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::{Look, painter};
use crate::theme::size::{KEYCAP_H, KEYCAP_PAD_X, RADIUS_CONTROL};
use crate::theme::{Text, tokens};

/// Between the keycap's edge and its key, the edge included.
const INSET: f32 = 1.0 + KEYCAP_PAD_X;

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

    /// How wide it draws, measured with `painter`'s fonts.
    pub fn width(&self, painter: &Painter) -> f32 {
        painter.layout_job(Text::Label.job(self.key, Color32::PLACEHOLDER)).size().x + 2.0 * INSET
    }

    /// Draws it with `painter`, its left edge centred on `left`, off when `disabled`: for a
    /// control that shows keys without adding them as widgets of their own (a menu's item).
    pub fn paint(&self, painter: &Painter, left: Pos2, disabled: bool) -> Rect {
        let t = tokens(painter.ctx());
        let galley = painter.layout_job(Text::Label.job(self.key, Color32::PLACEHOLDER));
        let rect = Rect::from_min_size(pos2(left.x, left.y - KEYCAP_H / 2.0), vec2(galley.size().x + 2.0 * INSET, KEYCAP_H));
        painter.rect(rect, CornerRadius::same(RADIUS_CONTROL), t.raised, Stroke::new(1.0, t.raised_line), StrokeKind::Inside);
        let colour = if disabled { t.text_faint } else { t.text_muted };
        painter.galley(pos2(rect.left() + INSET, rect.center().y - galley.size().y / 2.0), galley, colour);
        rect
    }
}

impl Widget for Keycap<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, r) = ui.allocate_exact_size(vec2(self.width(ui.painter()), KEYCAP_H), Sense::hover());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), self.key));
        if ui.is_rect_visible(rect) {
            let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
            self.paint(&painter(ui), rect.left_center(), look.disabled);
        }
        r
    }
}

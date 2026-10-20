//! Icons drawn in code: original shapes made of lines and ellipses, so no icon file (and no
//! other vendor's artwork) is involved. Each takes the painter, the square to draw in and a
//! colour; a rect that is empty or not finite draws nothing.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// The square inside `rect` an icon draws in, or `None` when there is nothing to draw on.
fn square(rect: Rect) -> Option<Rect> {
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let side = rect.width().min(rect.height());
    Some(Rect::from_center_size(rect.center(), vec2(side, side)))
}

fn stroke(side: f32, color: Color32) -> Stroke {
    Stroke::new((side / 10.0).clamp(1.0, 2.5), color)
}

/// A plus sign: "new".
pub fn plus(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let inset = r.width() * 0.2;
    let c = r.center();
    p.line_segment([pos2(r.left() + inset, c.y), pos2(r.right() - inset, c.y)], s);
    p.line_segment([pos2(c.x, r.top() + inset), pos2(c.x, r.bottom() - inset)], s);
}

/// The points of the lower half of the ellipse around `center`: a cylinder's front edge.
fn lower_arc(center: Pos2, rx: f32, ry: f32) -> Vec<Pos2> {
    const STEPS: usize = 12;
    (0..=STEPS)
        .map(|i| {
            let t = std::f32::consts::PI * (i as f32) / (STEPS as f32);
            pos2(center.x - rx * t.cos(), center.y + ry * t.sin())
        })
        .collect()
}

/// A database: a cylinder of three stacked discs.
pub fn database(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let rx = r.width() * 0.36;
    let ry = r.height() * 0.12;
    let top = r.top() + r.height() * 0.2;
    let bottom = r.bottom() - r.height() * 0.2;
    let c = r.center().x;
    p.add(Shape::ellipse_stroke(pos2(c, top), vec2(rx, ry), s));
    p.line_segment([pos2(c - rx, top), pos2(c - rx, bottom)], s);
    p.line_segment([pos2(c + rx, top), pos2(c + rx, bottom)], s);
    for y in [(top + bottom) / 2.0, bottom] {
        p.add(Shape::line(lower_arc(pos2(c, y), rx, ry), s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_draw_in_any_rect_without_panicking() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter();
            for r in [
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(16.0, 16.0)),
                egui::Rect::NOTHING,
                egui::Rect::EVERYTHING,
                egui::Rect::from_min_max(egui::pos2(f32::NAN, 0.0), egui::pos2(1.0, f32::INFINITY)),
            ] {
                plus(p, r, egui::Color32::BLACK);
                database(p, r, egui::Color32::BLACK);
            }
        })
        .drop_without_applying_deltas();
    }
}

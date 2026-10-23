//! Icons drawn in code: original shapes made of lines and ellipses, so no icon file (and no
//! other vendor's artwork) is involved. Each takes the painter, the square to draw in and a
//! colour; a rect that is empty or not finite draws nothing.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

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

/// `(x, y)` as fractions of the square `r`.
fn at(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

/// The rect from `(x0, y0)` to `(x1, y1)`, as fractions of the square `r`.
fn part(r: Rect, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(at(r, x0, y0), at(r, x1, y1))
}

/// A schema: a folder, a box with a tab on its top left edge.
pub fn schema(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.rect_stroke(part(r, 0.1, 0.32, 0.9, 0.84), CornerRadius::same(2), s, StrokeKind::Middle);
    p.add(Shape::line(vec![at(r, 0.1, 0.32), at(r, 0.1, 0.18), at(r, 0.4, 0.18), at(r, 0.5, 0.32)], s));
}

/// A table: a box with a filled header band and a two-by-two grid of cells below it.
pub fn table(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let outer = part(r, 0.12, 0.16, 0.88, 0.84);
    p.rect_filled(part(r, 0.12, 0.16, 0.88, 0.38), CornerRadius::same(2), color.gamma_multiply(0.45));
    p.rect_stroke(outer, CornerRadius::same(2), s, StrokeKind::Middle);
    p.line_segment([at(r, 0.12, 0.38), at(r, 0.88, 0.38)], s);
    p.line_segment([at(r, 0.12, 0.61), at(r, 0.88, 0.61)], s);
    p.line_segment([at(r, 0.5, 0.38), at(r, 0.5, 0.84)], s);
}

/// The outline of an eye, a lens between `x0` and `x1` at height `cy` and `half` high, with a
/// round pupil.
fn eye(p: &Painter, r: Rect, cy: f32, half: f32, s: Stroke, color: Color32) {
    const STEPS: usize = 10;
    let lens = |sign: f32| -> Vec<Pos2> {
        (0..=STEPS)
            .map(|i| {
                let t = (i as f32) / (STEPS as f32);
                at(r, 0.08 + 0.84 * t, cy + sign * half * (std::f32::consts::PI * t).sin())
            })
            .collect()
    };
    p.add(Shape::line(lens(-1.0), s));
    p.add(Shape::line(lens(1.0), s));
    p.circle_filled(at(r, 0.5, cy), r.width() * 0.13, color);
}

/// A view: an eye.
pub fn view(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    eye(p, r, 0.5, 0.26, stroke(r.width(), color), color);
}

/// A materialized view: an eye standing on a solid bar (the rows it keeps).
pub fn materialized_view(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    eye(p, r, 0.4, 0.22, s, color);
    p.rect_filled(part(r, 0.18, 0.74, 0.82, 0.86), CornerRadius::same(1), color);
}

/// A column: one tall cell stack.
pub fn column(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.rect_stroke(part(r, 0.32, 0.14, 0.68, 0.86), CornerRadius::same(1), s, StrokeKind::Middle);
    p.line_segment([at(r, 0.32, 0.38), at(r, 0.68, 0.38)], s);
    p.line_segment([at(r, 0.32, 0.62), at(r, 0.68, 0.62)], s);
}

/// A primary key: a ring and a shaft with two teeth.
pub fn key_primary(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.circle_stroke(at(r, 0.3, 0.5), r.width() * 0.19, s);
    p.line_segment([at(r, 0.49, 0.5), at(r, 0.92, 0.5)], s);
    p.line_segment([at(r, 0.86, 0.5), at(r, 0.86, 0.7)], s);
    p.line_segment([at(r, 0.72, 0.5), at(r, 0.72, 0.66)], s);
}

/// A foreign key: a smaller ring whose shaft ends in an arrow head, pointing at the other table.
pub fn key_foreign(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.circle_stroke(at(r, 0.24, 0.5), r.width() * 0.15, s);
    p.line_segment([at(r, 0.39, 0.5), at(r, 0.9, 0.5)], s);
    p.add(Shape::line(vec![at(r, 0.72, 0.32), at(r, 0.9, 0.5), at(r, 0.72, 0.68)], s));
}

/// A query: a prompt, a chevron and a cursor line.
pub fn query(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.add(Shape::line(vec![at(r, 0.14, 0.26), at(r, 0.44, 0.5), at(r, 0.14, 0.74)], s));
    p.line_segment([at(r, 0.52, 0.76), at(r, 0.88, 0.76)], s);
}

/// A cross: "close".
pub fn close(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.line_segment([at(r, 0.25, 0.25), at(r, 0.75, 0.75)], s);
    p.line_segment([at(r, 0.75, 0.25), at(r, 0.25, 0.75)], s);
}

/// Two arrows chasing each other round a circle: "reload".
pub fn refresh(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let c = r.center();
    let radius = r.width() * 0.3;
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        const STEPS: usize = 10;
        (0..=STEPS)
            .map(|i| {
                let a = from + (to - from) * (i as f32) / (STEPS as f32);
                pos2(c.x + radius * a.cos(), c.y + radius * a.sin())
            })
            .collect()
    };
    let pi = std::f32::consts::PI;
    for (from, to) in [(-0.35 * pi, 0.55 * pi), (0.65 * pi, 1.55 * pi)] {
        let points = arc(from, to);
        if let Some(&end) = points.last() {
            // The arrow head at the arc's end, along the circle's tangent.
            let dir = vec2(-to.sin(), to.cos());
            let normal = vec2(to.cos(), to.sin());
            let head = r.width() * 0.16;
            p.add(Shape::line(vec![end - dir * head + normal * head * 0.7, end, end - dir * head - normal * head * 0.7], s));
        }
        p.add(Shape::line(points, s));
    }
}

/// A chevron pointing right (`open: false`) or down (`open: true`): a tree row's expander.
pub fn chevron(p: &Painter, rect: Rect, color: Color32, open: bool) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let points = if open {
        vec![at(r, 0.28, 0.38), at(r, 0.5, 0.62), at(r, 0.72, 0.38)]
    } else {
        vec![at(r, 0.38, 0.28), at(r, 0.62, 0.5), at(r, 0.38, 0.72)]
    };
    p.add(Shape::line(points, s));
}

/// A minus sign: "remove".
pub fn minus(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.line_segment([at(r, 0.2, 0.5), at(r, 0.8, 0.5)], s);
}

/// Which way an arrow of the paging bar points, and whether it stops at a bar (to the first or
/// the last page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    First,
    Prev,
    Next,
    Last,
}

/// A paging arrow: a chevron, with a bar at its tip for the first and last page.
pub fn step(p: &Painter, rect: Rect, color: Color32, step: Step) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let left = matches!(step, Step::First | Step::Prev);
    let (tip, tail) = if left { (0.36, 0.62) } else { (0.64, 0.38) };
    p.add(Shape::line(vec![at(r, tail, 0.26), at(r, tip, 0.5), at(r, tail, 0.74)], s));
    let bar = match step {
        Step::First => Some(0.26),
        Step::Last => Some(0.74),
        Step::Prev | Step::Next => None,
    };
    if let Some(x) = bar {
        p.line_segment([at(r, x, 0.26), at(r, x, 0.74)], s);
    }
}

/// Angle brackets: the filter bar's switch to a raw `WHERE` condition.
pub fn brackets(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.add(Shape::line(vec![at(r, 0.4, 0.26), at(r, 0.16, 0.5), at(r, 0.4, 0.74)], s));
    p.add(Shape::line(vec![at(r, 0.6, 0.26), at(r, 0.84, 0.5), at(r, 0.6, 0.74)], s));
}

/// A filled triangle pointing up (`up`) or down: a column's sort.
pub fn sort(p: &Painter, rect: Rect, color: Color32, up: bool) {
    let Some(r) = square(rect) else { return };
    let points =
        if up { vec![at(r, 0.5, 0.3), at(r, 0.78, 0.7), at(r, 0.22, 0.7)] } else { vec![at(r, 0.22, 0.3), at(r, 0.78, 0.3), at(r, 0.5, 0.7)] };
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// A circle with an arrow through it, pointing right: "follow this reference".
pub fn follow(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.circle_stroke(r.center(), r.width() * 0.4, s);
    p.line_segment([at(r, 0.28, 0.5), at(r, 0.7, 0.5)], s);
    p.add(Shape::line(vec![at(r, 0.52, 0.33), at(r, 0.7, 0.5), at(r, 0.52, 0.67)], s));
}

/// A line break as the grid shows it inside a cell's one line: a short drop, then an arrow back
/// to the left.
pub fn line_break(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = Stroke::new((r.width() / 10.0).clamp(1.0, 1.5), color);
    p.add(Shape::line(vec![at(r, 0.8, 0.15), at(r, 0.8, 0.65), at(r, 0.2, 0.65)], s));
    p.add(Shape::line(vec![at(r, 0.42, 0.42), at(r, 0.2, 0.65), at(r, 0.42, 0.88)], s));
}

/// A magnifying glass: "search".
pub fn search(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.circle_stroke(at(r, 0.42, 0.42), r.width() * 0.24, s);
    p.line_segment([at(r, 0.6, 0.6), at(r, 0.84, 0.84)], s);
}

/// A window with a pane at its right side: the value panel, its pane filled while it shows.
pub fn side_panel(p: &Painter, rect: Rect, color: Color32, open: bool) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    let outer = Rect::from_min_max(at(r, 0.1, 0.2), at(r, 0.9, 0.8));
    let split = outer.left() + outer.width() * 0.6;
    if open {
        p.rect_filled(Rect::from_min_max(pos2(split, outer.top()), outer.max), CornerRadius::same(1), color.gamma_multiply(0.5));
    }
    p.rect_stroke(outer, CornerRadius::same(2), s, StrokeKind::Middle);
    p.line_segment([pos2(split, outer.top()), pos2(split, outer.bottom())], s);
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
                let c = egui::Color32::BLACK;
                plus(p, r, c);
                database(p, r, c);
                for icon in [schema, table, view, materialized_view, column, key_primary, key_foreign, query, close, refresh, search] {
                    icon(p, r, c);
                }
                chevron(p, r, c, true);
                chevron(p, r, c, false);
                for icon in [minus, brackets, follow, line_break] {
                    icon(p, r, c);
                }
                for s in [Step::First, Step::Prev, Step::Next, Step::Last] {
                    step(p, r, c, s);
                }
                sort(p, r, c, true);
                sort(p, r, c, false);
                side_panel(p, r, c, true);
                side_panel(p, r, c, false);
            }
        })
        .drop_without_applying_deltas();
    }
}

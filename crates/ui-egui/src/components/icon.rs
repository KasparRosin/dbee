//! The icons of the design's Components page (`icon/*`), drawn in code from the page's own
//! vectors: the same lines, curves and fills at the same coordinates and stroke widths, round
//! ends where the page has them. Each takes the colour its use gives it.
//!
//! A stroke is centred on its line, as on the page, so a line along an icon's box reaches half
//! its width past it. Corners are joined the way egui joins a path (mitred, a sharp one cut);
//! the page's round joins sit inside that by under a third of a pixel.
//!
//! The icons Screens uses that the page doesn't have yet ([`column`], [`follow`],
//! [`line_break`]) stay as they were drawn before, until their screens.

use egui::epaint::RectShape;
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

/// One of the page's 32 icons, named as on the page (`icon/none` is [`Icon::NoColour`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    ChevronDown,
    ChevronRight,
    SortAscending,
    SortDescending,
    Key,
    Plus,
    Minus,
    Close,
    Refresh,
    TabTable,
    TabQuery,
    Panel,
    Export,
    Import,
    Code,
    First,
    Previous,
    Next,
    Last,
    Check,
    CheckBox,
    Copy,
    Wrap,
    Search,
    NoColour,
    Link,
    Schema,
    Table,
    View,
    MaterializedView,
    KeyPrimary,
    KeyForeign,
}

/// The table's header band: the page's 45 % of the icon's colour.
const TABLE_BAND: f32 = 0.45;
/// Straight pieces a curve is drawn with.
const CURVE_STEPS: usize = 12;

impl Icon {
    pub const ALL: [Icon; 32] = [
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::SortAscending,
        Icon::SortDescending,
        Icon::Key,
        Icon::Plus,
        Icon::Minus,
        Icon::Close,
        Icon::Refresh,
        Icon::TabTable,
        Icon::TabQuery,
        Icon::Panel,
        Icon::Export,
        Icon::Import,
        Icon::Code,
        Icon::First,
        Icon::Previous,
        Icon::Next,
        Icon::Last,
        Icon::Check,
        Icon::CheckBox,
        Icon::Copy,
        Icon::Wrap,
        Icon::Search,
        Icon::NoColour,
        Icon::Link,
        Icon::Schema,
        Icon::Table,
        Icon::View,
        Icon::MaterializedView,
        Icon::KeyPrimary,
        Icon::KeyForeign,
    ];

    /// The page's name for it, after `icon/`.
    pub fn name(self) -> &'static str {
        match self {
            Icon::ChevronDown => "chevron-down",
            Icon::ChevronRight => "chevron-right",
            Icon::SortAscending => "sort-ascending",
            Icon::SortDescending => "sort-descending",
            Icon::Key => "key",
            Icon::Plus => "plus",
            Icon::Minus => "minus",
            Icon::Close => "close",
            Icon::Refresh => "refresh",
            Icon::TabTable => "tab-table",
            Icon::TabQuery => "tab-query",
            Icon::Panel => "panel",
            Icon::Export => "export",
            Icon::Import => "import",
            Icon::Code => "code",
            Icon::First => "first",
            Icon::Previous => "previous",
            Icon::Next => "next",
            Icon::Last => "last",
            Icon::Check => "check",
            Icon::CheckBox => "check-box",
            Icon::Copy => "copy",
            Icon::Wrap => "wrap",
            Icon::Search => "search",
            Icon::NoColour => "none",
            Icon::Link => "link",
            Icon::Schema => "schema",
            Icon::Table => "table",
            Icon::View => "view",
            Icon::MaterializedView => "materialized-view",
            Icon::KeyPrimary => "key-primary",
            Icon::KeyForeign => "key-foreign",
        }
    }

    /// The page node's box.
    pub fn size(self) -> Vec2 {
        match self {
            Icon::ChevronDown => vec2(8.0, 4.0),
            Icon::ChevronRight => vec2(4.0, 8.0),
            Icon::SortAscending | Icon::SortDescending => vec2(6.0, 8.0),
            Icon::Key => vec2(9.5, 9.5),
            Icon::Plus | Icon::Minus | Icon::Copy => vec2(10.0, 10.0),
            Icon::Close => vec2(7.0, 7.0),
            Icon::Refresh => vec2(10.0, 11.2),
            Icon::TabTable => vec2(12.0, 10.0),
            Icon::TabQuery => vec2(10.0, 6.0),
            Icon::Panel => vec2(14.0, 10.0),
            Icon::Export | Icon::Import => vec2(10.0, 9.0),
            Icon::Code => vec2(12.0, 8.0),
            Icon::First | Icon::Last => vec2(7.0, 10.0),
            Icon::Previous | Icon::Next => vec2(5.0, 10.0),
            Icon::Check => vec2(9.0, 6.0),
            Icon::CheckBox => vec2(8.0, 5.4),
            Icon::Wrap => vec2(10.0, 8.0),
            Icon::NoColour => vec2(12.0, 12.0),
            Icon::Link => vec2(9.8, 9.8),
            Icon::Schema => vec2(13.0, 9.0),
            Icon::Search | Icon::Table | Icon::View | Icon::MaterializedView | Icon::KeyPrimary | Icon::KeyForeign => vec2(14.0, 14.0),
        }
    }

    /// The page's stroke width for it.
    fn weight(self) -> f32 {
        match self {
            Icon::ChevronDown | Icon::ChevronRight | Icon::SortAscending | Icon::SortDescending | Icon::KeyPrimary | Icon::KeyForeign => 1.3,
            Icon::TabTable
            | Icon::Panel
            | Icon::Export
            | Icon::Import
            | Icon::Copy
            | Icon::Wrap
            | Icon::Link
            | Icon::Table
            | Icon::View
            | Icon::MaterializedView => 1.1,
            Icon::CheckBox => 1.5,
            _ => 1.2,
        }
    }

    /// How far past whole points from its box's corner its straight lines sit: half a point down
    /// for the table's rows, none for the others.
    fn grid(self) -> Vec2 {
        if self == Icon::Table { vec2(0.0, 0.5) } else { Vec2::ZERO }
    }

    /// Draws it in `colour` with its box ([`Icon::size`]) centred on `center`, moved by less than
    /// a pixel so its straight lines fall on the pixel grid. A centre that isn't finite draws
    /// nothing.
    pub fn paint(self, painter: &Painter, center: Pos2, colour: Color32) {
        if !center.is_finite() {
            return;
        }
        let stroke = Stroke::new(self.weight(), colour);
        let ppp = painter.pixels_per_point();
        let mut line = center - self.size() / 2.0 + self.grid();
        stroke.round_center_to_pixel(ppp, &mut line.x);
        stroke.round_center_to_pixel(ppp, &mut line.y);
        let pen = Pen { painter, origin: line.to_vec2() - self.grid(), stroke };
        match self {
            Icon::ChevronDown => pen.round(&[p(0.0, 0.0), p(4.0, 4.0), p(8.0, 0.0)]),
            Icon::ChevronRight => pen.round(&[p(0.0, 0.0), p(4.0, 4.0), p(0.0, 8.0)]),
            Icon::SortAscending => {
                pen.round(&[p(3.0, 8.0), p(3.0, 0.0)]);
                pen.round(&[p(0.0, 3.0), p(3.0, 0.0), p(6.0, 3.0)]);
            }
            Icon::SortDescending => {
                pen.round(&[p(3.0, 0.0), p(3.0, 8.0)]);
                pen.round(&[p(0.0, 5.0), p(3.0, 8.0), p(6.0, 5.0)]);
            }
            Icon::Key => {
                pen.round(&[p(4.3, 4.3), p(9.5, 9.5)]);
                pen.round(&[p(7.5, 7.5), p(9.0, 6.0)]);
                pen.ring(p(2.5, 2.5), 2.5);
            }
            Icon::Plus => {
                pen.round(&[p(5.0, 0.0), p(5.0, 10.0)]);
                pen.round(&[p(0.0, 5.0), p(10.0, 5.0)]);
            }
            Icon::Minus => pen.round(&[p(0.0, 5.0), p(10.0, 5.0)]),
            Icon::Close => {
                pen.round(&[p(0.0, 0.0), p(7.0, 7.0)]);
                pen.round(&[p(7.0, 0.0), p(0.0, 7.0)]);
            }
            Icon::Refresh => {
                let arc = Trace::from(9.997, 6.0)
                    .curve([10.081, 8.395, 8.453, 10.513, 6.117, 11.048])
                    .curve([3.78, 11.582, 1.393, 10.385, 0.426, 8.192])
                    .curve([-0.541, 5.999, 0.183, 3.428, 2.153, 2.064])
                    .curve([4.123, 0.699, 6.785, 0.924, 8.497, 2.6]);
                pen.round(&arc.0);
                pen.round(&[p(8.997, 0.0), p(8.997, 3.0), p(5.997, 3.0)]);
            }
            Icon::TabTable => {
                pen.line(&[p(0.0, 3.5), p(12.0, 3.5)]);
                pen.line(&[p(4.0, 3.5), p(4.0, 10.0)]);
                pen.closed(&[p(0.0, 0.0), p(12.0, 0.0), p(12.0, 10.0), p(0.0, 10.0)]);
            }
            Icon::TabQuery => {
                pen.round(&[p(0.0, 0.0), p(3.5, 3.0), p(0.0, 6.0)]);
                pen.round(&[p(5.0, 6.0), p(10.0, 6.0)]);
            }
            Icon::Panel => {
                pen.line(&[p(9.0, 0.0), p(9.0, 10.0)]);
                pen.closed(&[p(0.0, 0.0), p(14.0, 0.0), p(14.0, 10.0), p(0.0, 10.0)]);
            }
            Icon::Export => {
                pen.round(&[p(5.0, 7.0), p(5.0, 0.0)]);
                pen.round(&[p(2.0, 3.0), p(5.0, 0.0), p(8.0, 3.0)]);
                pen.round(&[p(0.0, 6.0), p(0.0, 9.0), p(10.0, 9.0), p(10.0, 6.0)]);
            }
            Icon::Import => {
                pen.round(&[p(5.0, 0.0), p(5.0, 7.0)]);
                pen.round(&[p(2.0, 4.0), p(5.0, 7.0), p(8.0, 4.0)]);
                pen.round(&[p(0.0, 6.0), p(0.0, 9.0), p(10.0, 9.0), p(10.0, 6.0)]);
            }
            Icon::Code => {
                pen.round(&[p(4.0, 0.0), p(0.0, 4.0), p(4.0, 8.0)]);
                pen.round(&[p(8.0, 0.0), p(12.0, 4.0), p(8.0, 8.0)]);
            }
            Icon::First => {
                pen.round(&[p(0.0, 0.0), p(0.0, 10.0)]);
                pen.round(&[p(7.0, 0.0), p(2.0, 5.0), p(7.0, 10.0)]);
            }
            Icon::Previous => pen.round(&[p(5.0, 0.0), p(0.0, 5.0), p(5.0, 10.0)]),
            Icon::Next => pen.round(&[p(0.0, 0.0), p(5.0, 5.0), p(0.0, 10.0)]),
            Icon::Last => {
                pen.round(&[p(0.0, 0.0), p(5.0, 5.0), p(0.0, 10.0)]);
                pen.round(&[p(7.0, 0.0), p(7.0, 10.0)]);
            }
            Icon::Check => pen.round(&[p(0.0, 3.103), p(3.068, 6.0), p(9.0, 0.0)]),
            Icon::CheckBox => pen.round(&[p(0.0, 2.7), p(2.8, 5.4), p(8.0, 0.0)]),
            Icon::Copy => {
                pen.round(&[p(3.0, 3.0), p(3.0, 0.0), p(10.0, 0.0), p(10.0, 7.0), p(7.0, 7.0)]);
                pen.closed(&[p(0.0, 3.0), p(7.0, 3.0), p(7.0, 10.0), p(0.0, 10.0)]);
            }
            Icon::Wrap => {
                let turn =
                    Trace::from(0.0, 0.0).to(7.0, 0.0).curve([8.7, 0.0, 10.0, 1.3, 10.0, 3.0]).curve([10.0, 4.7, 8.7, 6.0, 7.0, 6.0]).to(3.0, 6.0);
                pen.round(&turn.0);
                pen.round(&[p(5.0, 4.0), p(3.0, 6.0), p(5.0, 8.0)]);
            }
            Icon::Search => {
                let pen = pen.moved(1.4, 1.4);
                pen.line(&[p(7.3, 7.3), p(10.8, 10.8)]);
                pen.ring(p(4.2, 4.2), 4.2);
            }
            Icon::NoColour => pen.round(&[p(0.0, 12.0), p(12.0, 0.0)]),
            Icon::Link => {
                pen.round(&[p(2.875, 5.875), p(4.875, 3.875)]);
                let lower = Trace::from(2.075, 4.675)
                    .to(0.675, 6.075)
                    .curve([-0.225, 6.975, -0.225, 8.275, 0.675, 9.075])
                    .curve([1.475, 9.975, 2.775, 9.975, 3.675, 9.075])
                    .to(5.075, 7.675);
                pen.round(&lower.0);
                let upper = Trace::from(4.675, 2.075)
                    .to(6.075, 0.675)
                    .curve([6.975, -0.225, 8.275, -0.225, 9.075, 0.675])
                    .curve([9.975, 1.475, 9.975, 2.775, 9.075, 3.675])
                    .to(7.675, 5.075);
                pen.round(&upper.0);
            }
            Icon::Schema => pen.closed(&[p(0.0, 0.0), p(5.0, 0.0), p(6.5, 1.5), p(13.0, 1.5), p(13.0, 9.0), p(0.0, 9.0)]),
            Icon::Table => {
                let pen = pen.moved(2.0, 2.5);
                pen.fill(Rect::from_min_max(p(0.0, 0.0), p(10.0, 3.0)), 0, colour.gamma_multiply(TABLE_BAND));
                pen.line(&[p(0.0, 3.0), p(10.0, 3.0)]);
                pen.line(&[p(0.0, 6.0), p(10.0, 6.0)]);
                pen.line(&[p(5.0, 3.0), p(5.0, 9.0)]);
                pen.closed(&[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 9.0), p(0.0, 9.0)]);
            }
            Icon::View => {
                let eye = Trace::from(0.0, 3.6).curve([3.0, -1.2, 9.0, -1.2, 12.0, 3.6]).curve([9.0, 8.4, 3.0, 8.4, 0.0, 3.6]);
                pen.moved(1.0, 3.4).closed(&eye.closed());
                pen.dot(p(7.0, 7.0), 1.8);
            }
            Icon::MaterializedView => {
                let eye = Trace::from(0.0, 3.098).curve([3.0, -1.033, 9.0, -1.033, 12.0, 3.098]).curve([9.0, 7.228, 3.0, 7.228, 0.0, 3.098]);
                pen.moved(1.0, 2.502).closed(&eye.closed());
                pen.dot(p(7.0, 5.6), 1.6);
                pen.fill(Rect::from_min_max(p(2.5, 10.4), p(11.5, 12.0)), 1, colour);
            }
            Icon::KeyPrimary => {
                pen.ring(p(4.2, 7.0), 2.01);
                pen.line(&[p(6.86, 7.0), p(12.88, 7.0)]);
                pen.line(&[p(12.04, 7.0), p(12.04, 9.8)]);
                pen.line(&[p(10.08, 7.0), p(10.08, 9.24)]);
            }
            Icon::KeyForeign => {
                pen.ring(p(3.36, 7.0), 1.45);
                pen.line(&[p(5.46, 7.0), p(12.6, 7.0)]);
                pen.line(&[p(10.08, 4.48), p(12.6, 7.0), p(10.08, 9.52)]);
            }
        }
    }
}

/// A point of an icon's box, from its top left corner.
const fn p(x: f32, y: f32) -> Pos2 {
    pos2(x, y)
}

/// Draws an icon's parts, given in its box, with its stroke.
struct Pen<'a> {
    painter: &'a Painter,
    /// Where the box's top left corner is drawn.
    origin: Vec2,
    stroke: Stroke,
}

impl Pen<'_> {
    /// The same pen, `(x, y)` further into the box: a part the page draws in a frame of its own.
    fn moved(&self, x: f32, y: f32) -> Self {
        Pen { origin: self.origin + vec2(x, y), ..*self }
    }

    fn at(&self, points: &[Pos2]) -> Vec<Pos2> {
        points.iter().map(|&q| q + self.origin).collect()
    }

    /// Lines through `points`, their ends cut square.
    fn line(&self, points: &[Pos2]) {
        self.painter.add(Shape::line(self.at(points), self.stroke));
    }

    /// Lines through `points`, with round ends.
    fn round(&self, points: &[Pos2]) {
        self.line(points);
        for end in [points.first(), points.last()].into_iter().flatten() {
            self.painter.circle_filled(*end + self.origin, self.stroke.width / 2.0, self.stroke.color);
        }
    }

    /// The outline through `points`, back to the first.
    fn closed(&self, points: &[Pos2]) {
        self.painter.add(Shape::closed_line(self.at(points), self.stroke));
    }

    fn ring(&self, center: Pos2, radius: f32) {
        self.painter.circle_stroke(center + self.origin, radius, self.stroke);
    }

    fn dot(&self, center: Pos2, radius: f32) {
        self.painter.circle_filled(center + self.origin, radius, self.stroke.color);
    }

    /// `rect` filled, where the page has it: not moved to whole pixels.
    fn fill(&self, rect: Rect, radius: u8, colour: Color32) {
        let rect = rect.translate(self.origin);
        self.painter.add(RectShape::filled(rect, CornerRadius::same(radius), colour).with_round_to_pixels(false));
    }
}

/// A path of straight lines and cubic curves, as points.
struct Trace(Vec<Pos2>);

impl Trace {
    fn from(x: f32, y: f32) -> Trace {
        Trace(vec![p(x, y)])
    }

    fn to(mut self, x: f32, y: f32) -> Trace {
        self.0.push(p(x, y));
        self
    }

    /// A cubic curve from the last point, through the controls `(x1, y1)` and `(x2, y2)`, to
    /// `(x, y)`.
    fn curve(mut self, [x1, y1, x2, y2, x, y]: [f32; 6]) -> Trace {
        let Some(&start) = self.0.last() else { return self };
        let (c1, c2, end) = (p(x1, y1), p(x2, y2), p(x, y));
        for i in 1..=CURVE_STEPS {
            let t = i as f32 / CURVE_STEPS as f32;
            let u = 1.0 - t;
            let at =
                start.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + end.to_vec2() * (t * t * t);
            self.0.push(at.to_pos2());
        }
        self
    }

    /// The points of a path that ends where it began, without the end again.
    fn closed(mut self) -> Vec<Pos2> {
        if self.0.len() > 1 && self.0.first() == self.0.last() {
            self.0.pop();
        }
        self.0
    }
}

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

/// `(x, y)` as fractions of the square `r`.
fn at(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

/// The rect from `(x0, y0)` to `(x1, y1)`, as fractions of the square `r`.
fn part(r: Rect, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(at(r, x0, y0), at(r, x1, y1))
}

/// A column: one tall cell stack.
pub fn column(p: &Painter, rect: Rect, color: Color32) {
    let Some(r) = square(rect) else { return };
    let s = stroke(r.width(), color);
    p.rect_stroke(part(r, 0.32, 0.14, 0.68, 0.86), CornerRadius::same(1), s, StrokeKind::Middle);
    p.line_segment([at(r, 0.32, 0.38), at(r, 0.68, 0.38)], s);
    p.line_segment([at(r, 0.32, 0.62), at(r, 0.68, 0.62)], s);
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use egui::epaint::ColorMode;

    use super::*;

    /// The page's icon nodes, 160919 to 160986: name and box.
    const PAGE: [(&str, f32, f32); 32] = [
        ("icon/chevron-down", 8.0, 4.0),
        ("icon/chevron-right", 4.0, 8.0),
        ("icon/sort-ascending", 6.0, 8.0),
        ("icon/sort-descending", 6.0, 8.0),
        ("icon/key", 9.5, 9.5),
        ("icon/plus", 10.0, 10.0),
        ("icon/minus", 10.0, 10.0),
        ("icon/close", 7.0, 7.0),
        ("icon/refresh", 10.0, 11.2),
        ("icon/tab-table", 12.0, 10.0),
        ("icon/tab-query", 10.0, 6.0),
        ("icon/panel", 14.0, 10.0),
        ("icon/export", 10.0, 9.0),
        ("icon/import", 10.0, 9.0),
        ("icon/code", 12.0, 8.0),
        ("icon/first", 7.0, 10.0),
        ("icon/previous", 5.0, 10.0),
        ("icon/next", 5.0, 10.0),
        ("icon/last", 7.0, 10.0),
        ("icon/check", 9.0, 6.0),
        ("icon/check-box", 8.0, 5.4),
        ("icon/copy", 10.0, 10.0),
        ("icon/wrap", 10.0, 8.0),
        ("icon/search", 14.0, 14.0),
        ("icon/none", 12.0, 12.0),
        ("icon/link", 9.8, 9.8),
        ("icon/schema", 13.0, 9.0),
        ("icon/table", 14.0, 14.0),
        ("icon/view", 14.0, 14.0),
        ("icon/materialized-view", 14.0, 14.0),
        ("icon/key-primary", 14.0, 14.0),
        ("icon/key-foreign", 14.0, 14.0),
    ];

    /// Every shape `icon` paints centred on `center`, at `ppp` pixels per point.
    fn drawn(icon: Icon, center: Pos2, colour: Color32, ppp: f32) -> Vec<Shape> {
        fn walk(shape: Shape, out: &mut Vec<Shape>) {
            match shape {
                Shape::Vec(shapes) => shapes.into_iter().for_each(|s| walk(s, out)),
                s => out.push(s),
            }
        }
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| icon.paint(ui.painter(), center, colour));
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut out = Vec::new();
        for s in shapes {
            walk(s.shape, &mut out);
        }
        out
    }

    /// The colours `shape` paints with, the invisible ones left out.
    fn colours(shape: &Shape) -> Vec<Color32> {
        let solid = |c: &ColorMode| match c {
            ColorMode::Solid(c) => *c,
            ColorMode::UV(_) => Color32::RED,
        };
        let all = match shape {
            Shape::LineSegment { stroke, .. } => vec![stroke.color],
            Shape::Path(p) => vec![p.fill, solid(&p.stroke.color)],
            Shape::Circle(c) => vec![c.fill, c.stroke.color],
            Shape::Ellipse(e) => vec![e.fill, e.stroke.color],
            Shape::Rect(r) => vec![r.fill, r.stroke.color],
            Shape::CubicBezier(b) => vec![b.fill, solid(&b.stroke.color)],
            Shape::QuadraticBezier(b) => vec![b.fill, solid(&b.stroke.color)],
            other => panic!("an icon draws lines, curves and fills only: {other:?}"),
        };
        all.into_iter().filter(|c| *c != Color32::TRANSPARENT).collect()
    }

    #[test]
    fn there_are_the_pages_32_icons() {
        let names: Vec<String> = Icon::ALL.iter().map(|i| format!("icon/{}", i.name())).collect();
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), 32, "32 distinct names: {names:?}");
        for (icon, name) in Icon::ALL.into_iter().zip(&names) {
            let page = PAGE.iter().find(|(n, _, _)| n == name);
            let Some((_, w, h)) = page else { panic!("{icon:?}: {name} isn't on the page") };
            assert_eq!(icon.size(), vec2(*w, *h), "{icon:?}: the page node's box");
        }
    }

    #[test]
    fn each_icon_stays_in_its_box() {
        // A stroke is centred on its line, as on the page, so a line along the box's edge reaches
        // half its width past it; half a pixel more is the room for keeping lines crisp.
        for ppp in [1.0, 1.5, 2.0] {
            for center in [pos2(40.0, 30.0), pos2(40.5, 30.5), pos2(40.3, 30.8)] {
                for icon in Icon::ALL {
                    let shapes = drawn(icon, center, Color32::BLACK, ppp);
                    assert!(!shapes.is_empty(), "{icon:?} draws");
                    let room = Rect::from_center_size(center, icon.size()).expand(icon.weight() / 2.0 + 0.5 + 1e-3);
                    for s in &shapes {
                        let drawn = s.visual_bounding_rect();
                        assert!(room.contains_rect(drawn), "{icon:?} at {center:?}, {ppp} px a point: {drawn:?} outside {room:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_icon_takes_the_colour_given() {
        for colour in [Color32::from_rgb(0x12, 0x9A, 0x56), Color32::from_rgba_unmultiplied(0x5A, 0x5A, 0x5A, 0x80)] {
            for icon in Icon::ALL {
                let shapes = drawn(icon, pos2(40.0, 30.0), colour, 1.0);
                assert!(!shapes.is_empty(), "{icon:?} draws");
                for s in &shapes {
                    for c in colours(s) {
                        // The table's header band is the page's 45 % of the same colour.
                        let band = icon == Icon::Table && c == colour.gamma_multiply(TABLE_BAND);
                        assert!(c == colour || band, "{icon:?}: {c:?} isn't {colour:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn icons_draw_anywhere_without_panicking() {
        for center in [pos2(0.0, 0.0), pos2(f32::NAN, 1.0), pos2(f32::INFINITY, f32::NEG_INFINITY), pos2(-1e9, 1e9)] {
            for icon in Icon::ALL {
                drawn(icon, center, Color32::BLACK, 1.0);
            }
        }
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for r in [
                Rect::from_min_size(pos2(0.0, 0.0), vec2(16.0, 16.0)),
                Rect::NOTHING,
                Rect::EVERYTHING,
                Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(1.0, f32::INFINITY)),
            ] {
                for draw in [column, follow, line_break] {
                    draw(ui.painter(), r, Color32::BLACK);
                }
            }
        });
        output.drop_without_applying_deltas();
    }
}

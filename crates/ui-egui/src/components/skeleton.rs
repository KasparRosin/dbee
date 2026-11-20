//! The skeleton (LC1, LC4): grey bars where content is on its way and nothing is there yet to
//! show. A bar is `fill`, [`SKELETON_BAR_H`] tall with corners of [`SKELETON_RADIUS`] ([`bar`]);
//! its width comes from a fixed pattern ([`width`]), the same on every frame and every run, so the
//! rows don't flicker. [`GridRows`] draws the rows of a grid's first load under the header: the
//! table tab's grid, and any other screen that shows rows in columns (the structure view).
//!
//! The skeleton only shows once its job has run 300 ms ([`crate::quiet`]): the caller decides.

use egui::{CornerRadius, Rect, Sense, Ui, pos2, vec2};

use super::painter;
use crate::theme::size::{
    GRID_CELL_PAD_X, ROW_H, SKELETON_BAR_H, SKELETON_FRACTIONS, SKELETON_MAX_W, SKELETON_MIN_W, SKELETON_RADIUS, SKELETON_ROWS,
};
use crate::theme::tokens;

/// A bar's width in the cell of row `row` and column `col` whose room (its width less its insets)
/// is `room`: that room times the fraction the pattern gives the cell, within
/// [`SKELETON_MIN_W`]..=[`SKELETON_MAX_W`], and never more than the room; 0 (no bar) without room.
pub fn width(row: usize, col: usize, room: f32) -> f32 {
    if room.is_nan() || room <= 0.0 {
        return 0.0;
    }
    let at = row.wrapping_mul(3).wrapping_add(col) % SKELETON_FRACTIONS.len();
    let fraction = SKELETON_FRACTIONS.get(at).copied().unwrap_or(1.0);
    (room * fraction).clamp(SKELETON_MIN_W, SKELETON_MAX_W).min(room)
}

/// Paints one bar `w` wide from `left`, centred down on `mid_y`: `fill`, [`SKELETON_BAR_H`] tall,
/// corners of [`SKELETON_RADIUS`]. Takes no room and no input.
pub fn bar(ui: &Ui, left: f32, mid_y: f32, w: f32) {
    if w.is_nan() || w <= 0.0 {
        return;
    }
    let rect = Rect::from_min_size(pos2(left, mid_y - SKELETON_BAR_H / 2.0), vec2(w, SKELETON_BAR_H));
    painter(ui).rect_filled(rect, CornerRadius::same(SKELETON_RADIUS), tokens(ui.ctx()).fill);
}

/// The rows of a grid whose first rows are on their way, under its header: [`SKELETON_ROWS`]
/// rows [`ROW_H`] tall (fewer when the room is shorter: whole rows only), each with the grid's
/// 1 px `line` under it across the room's full width. The row numbers' column is there and empty;
/// every other cell holds one bar, [`GRID_CELL_PAD_X`] in from its left, or from its right in a
/// column whose values sit right.
#[derive(Clone, Copy, Debug)]
pub struct GridRows<'a> {
    /// Each column's width, from the left after the row numbers.
    pub widths: &'a [f32],
    /// Whether each column's values sit right (numbers); missing ones sit left.
    pub right: &'a [bool],
    /// The row numbers' width.
    pub number_w: f32,
}

impl GridRows<'_> {
    /// Draws the rows from the top of `ui`'s room across its width and takes their room: the rows'
    /// rect and how many were drawn.
    pub fn show(&self, ui: &mut Ui) -> (Rect, usize) {
        let room = ui.available_rect_before_wrap();
        let fit = (room.height().max(0.0) / ROW_H).floor();
        let rows = if fit.is_finite() { (fit as usize).min(SKELETON_ROWS) } else { 0 };
        let (rect, _) = ui.allocate_exact_size(vec2(room.width().max(0.0), ROW_H * rows as f32), Sense::hover());
        let p = painter(ui);
        let line = tokens(ui.ctx()).line;
        for row in 0..rows {
            let top = rect.top() + ROW_H * row as f32;
            p.rect_filled(Rect::from_min_max(pos2(rect.left(), top + ROW_H - 1.0), pos2(rect.right(), top + ROW_H)), CornerRadius::ZERO, line);
            let mid = top + ROW_H / 2.0;
            let mut x = rect.left() + self.number_w;
            for (col, &w) in self.widths.iter().enumerate() {
                if x >= rect.right() {
                    break;
                }
                let bar_w = width(row, col, w - 2.0 * GRID_CELL_PAD_X);
                let right = self.right.get(col).copied().unwrap_or(false);
                let left = if right { x + w - GRID_CELL_PAD_X - bar_w } else { x + GRID_CELL_PAD_X };
                bar(ui, left, mid, bar_w);
                x += w;
            }
        }
        (rect, rows)
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Shape};

    use super::*;
    use crate::components::{both, flat, painted_in};

    /// The rects `shapes` fill with `colour` that are `h` tall, with their corners.
    fn rects(shapes: &[egui::epaint::ClippedShape], colour: Color32, h: f32) -> Vec<(Rect, CornerRadius)> {
        flat(shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Rect(r) if r.fill == colour && (r.rect.height() - h).abs() < 0.01 => Some((r.rect, r.corner_radius)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bars_are_fill_8_tall_radius_4() {
        for (theme, t) in both() {
            let shapes = painted_in(vec2(400.0, 200.0), theme, |ui| bar(ui, 20.0, 50.0, 40.0));
            let bars = rects(&shapes, t.fill, 8.0);
            assert_eq!(bars.len(), 1, "{theme:?}: one bar: {bars:?}");
            let (rect, radius) = bars[0];
            assert_eq!(rect, Rect::from_min_size(pos2(20.0, 46.0), vec2(40.0, 8.0)), "{theme:?}");
            assert_eq!(radius, CornerRadius::same(4), "{theme:?}");
        }
    }

    #[test]
    fn ten_rows_with_a_bar_per_cell_and_none_in_the_numbers() {
        let (widths, right, number_w) = ([120.0, 80.0, 60.0], [false, true, false], 36.0);
        for (theme, t) in both() {
            let mut got = (Rect::NOTHING, 0);
            let shapes = painted_in(vec2(600.0, 400.0), theme, |ui| {
                let room = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
                let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(room));
                got = GridRows { widths: &widths, right: &right, number_w }.show(&mut inner);
            });
            let (area, rows) = got;
            assert_eq!(rows, 10, "{theme:?}");
            assert!((area.height() - 260.0).abs() < 0.01 && (area.width() - 600.0).abs() < 0.01, "{theme:?}: {area:?}");
            let lines = rects(&shapes, t.line, 1.0);
            assert_eq!(lines.len(), 10, "{theme:?}: a line under each row");
            for (i, (line, _)) in lines.iter().enumerate() {
                assert!((line.bottom() - 26.0 * (i + 1) as f32).abs() < 0.01, "{theme:?}: row {i} 26 tall: {line:?}");
                assert!(line.left() == 0.0 && line.right() == 600.0, "{theme:?}: across the area: {line:?}");
            }
            let bars = rects(&shapes, t.fill, 8.0);
            assert_eq!(bars.len(), 30, "{theme:?}");
            assert!(bars.iter().all(|(b, _)| b.left() >= number_w), "{theme:?}: none in the numbers");
            let lefts = [number_w, number_w + 120.0, number_w + 200.0];
            for (i, (b, _)) in bars.iter().enumerate() {
                let (row, col) = (i / 3, i % 3);
                assert!((b.center().y - (26.0 * row as f32 + 13.0)).abs() < 0.01, "{theme:?}: bar {i} on its row's middle");
                assert!((b.width() - width(row, col, widths[col] - 16.0)).abs() < 0.01, "{theme:?}: bar {i}'s width");
                if right[col] {
                    assert!((b.right() - (lefts[col] + widths[col] - 8.0)).abs() <= 0.5, "{theme:?}: bar {i} ends 8 from the right");
                } else {
                    assert!((b.left() - (lefts[col] + 8.0)).abs() <= 0.5, "{theme:?}: bar {i} starts 8 in");
                }
            }
        }
    }

    #[test]
    fn a_short_area_draws_whole_rows_only() {
        let mut rows = 0;
        let _ = painted_in(vec2(600.0, 400.0), egui::Theme::Light, |ui| {
            let room = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 100.0));
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(room));
            rows = GridRows { widths: &[100.0], right: &[], number_w: 36.0 }.show(&mut inner).1;
        });
        assert_eq!(rows, 3);
    }

    #[test]
    fn width_is_fixed_by_row_and_column() {
        assert_eq!(width(4, 2, 300.0), width(4, 2, 300.0));
        assert!((width(0, 0, 100.0) - 60.0).abs() < 0.001, "the room times 0.6");
        assert_eq!(width(0, 1, 300.0), 96.0, "at most 96");
        assert_eq!(width(0, 2, 20.0), 12.0, "at least 12");
        assert_eq!(width(0, 0, 4.0), 4.0, "never more than the room");
        assert_eq!(width(3, 1, 0.0), 0.0);
        assert_eq!(width(3, 1, -5.0), 0.0);
    }
}

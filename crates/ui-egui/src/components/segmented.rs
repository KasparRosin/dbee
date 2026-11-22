//! The segmented control (`segmented`) and its segments (`segmented/segment`): a 28 box holding
//! 26 tall segments, one of them chosen. Style caps (words in `label` capitals, each segment as
//! wide as its words) or small (words in `small` as written, the segments sharing the box's
//! width, each one's words cut with "…" when its share is too narrow, whole on hover). Each
//! segment is its own control, reached with Tab and chosen with a click or Space.
//!
//! The box's edge is drawn over its segments, and a segment's ground runs on under the edge where
//! the segment meets it. The edge is 1 point wide from the box's outside, which is not a whole
//! number of pixels at every scale (1.25 at 125 %), while a ground ends on a whole pixel: a ground
//! that stopped at the edge's inside left a sliver of the box's `card` between the two.

use std::sync::Arc;

use egui::{
    Align, Color32, CornerRadius, Galley, Layout, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Widget, WidgetInfo, WidgetType,
    pos2, vec2,
};

use super::tooltip::Hint;
use super::{Look, focus, painter};
use crate::theme::size::{RADIUS_CONTROL, SEGMENT_DIVIDER, SEGMENT_H, SEGMENT_PAD_X, SEGMENTED_H};
use crate::theme::{Text, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Caps,
    Small,
}

/// One segment, 26 tall; off when added with `ui.add_enabled(false, …)`.
pub struct Segment<'a> {
    words: &'a str,
    style: Style,
    selected: bool,
    divider: bool,
    width: Option<f32>,
    look: Option<Look>,
}

impl<'a> Segment<'a> {
    /// `words` in `label` capitals, 12 either side.
    pub fn caps(words: &'a str, selected: bool) -> Segment<'a> {
        Segment { words, style: Style::Caps, selected, divider: false, width: None, look: None }
    }

    /// `words` in `small`, as written, centred.
    pub fn small(words: &'a str, selected: bool) -> Segment<'a> {
        Segment { words, style: Style::Small, selected, divider: false, width: None, look: None }
    }

    /// A 1 px `line` at its right end, between it and the next.
    pub fn divider(mut self, divider: bool) -> Segment<'a> {
        self.divider = divider;
        self
    }

    /// This wide, its words centred (a small segment's share of the box).
    pub fn width(mut self, width: f32) -> Segment<'a> {
        self.width = Some(width);
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Segment<'a> {
        self.look = Some(look);
        self
    }

    /// The words as drawn, how wide they read, and the segment's own width: a caps segment 12
    /// either side of its words, a small one 1 either side (its divider's slot and the same on
    /// the left).
    fn parts(&self, ui: &Ui) -> (Arc<Galley>, f32, f32) {
        let text = match self.style {
            Style::Caps => Text::Label,
            Style::Small => Text::Small,
        };
        let mut job = match self.style {
            Style::Caps => text.job(self.words, Color32::PLACEHOLDER),
            Style::Small => text.job_as_written(self.words, Color32::PLACEHOLDER),
        };
        if let (Style::Small, Some(w)) = (self.style, self.width) {
            // A share too narrow for the words cuts them with "…".
            job.wrap = egui::text::TextWrapping::truncate_at_width((w - 2.0 * SEGMENT_DIVIDER).max(1.0));
        }
        let words = ui.painter().layout_job(job);
        // egui puts the letter spacing after the last letter too; the page's measure has none there.
        let reads = words.size().x - text.spacing();
        let natural = match self.style {
            Style::Caps => (reads + 2.0 * SEGMENT_PAD_X).ceil(),
            Style::Small => reads + 2.0 * SEGMENT_DIVIDER,
        };
        (words, reads, self.width.unwrap_or(natural))
    }
}

impl Widget for Segment<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (words, reads, width) = self.parts(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(width, SEGMENT_H), Sense::click());
        r.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), self.selected, self.words));
        // Cut words read whole on hover.
        let r = match (words.elided, ui.is_enabled()) {
            (false, _) => r,
            (true, true) => r.hint(self.words),
            (true, false) => r.hint_disabled(self.words),
        };
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        if let Some(ground) = ground(t, self.selected, look.hovered, look.disabled) {
            p.rect_filled(rect, CornerRadius::ZERO, ground);
        }
        let colour = match (self.selected, look.disabled) {
            (_, true) => t.text_faint,
            (true, false) => t.on_ink,
            (false, false) => t.text,
        };
        p.galley(pos2(rect.center().x - reads / 2.0, rect.center().y - words.size().y / 2.0), words, colour);
        if self.divider {
            let line = Rect::from_min_size(pos2(rect.right() - SEGMENT_DIVIDER, rect.top()), vec2(SEGMENT_DIVIDER, rect.height()));
            p.rect_filled(line, CornerRadius::ZERO, t.line);
        }
        if look.focused {
            // The focus keeps to the cell, off the divider's slot.
            let cell = Rect::from_min_max(rect.min, pos2(rect.right() - SEGMENT_DIVIDER, rect.bottom()));
            if self.selected {
                focus::on_ink_line(&p, cell);
            } else {
                focus::own_edge(&p, cell, 0);
            }
        }
        r
    }
}

/// A segment's ground: `ink` when chosen (`fill` while off), `hover` under the pointer, else none.
fn ground(t: &crate::theme::Tokens, selected: bool, hovered: bool, disabled: bool) -> Option<Color32> {
    match (selected, disabled) {
        (true, true) => Some(t.fill),
        (true, false) => Some(t.ink),
        (false, false) if hovered => Some(t.hover),
        (false, _) => None,
    }
}

/// A segmented control: `choices` (a value and its words) side by side in a box, the one equal to
/// `selected` chosen; a click or Space on another chooses it.
pub struct Segmented<'a, T> {
    selected: &'a mut T,
    choices: &'a [(T, &'a str)],
    style: Style,
    width: f32,
    /// The choice drawn off, which a click can't choose.
    off: Option<T>,
}

/// What a [`Segmented`] drew: the box's response, and each segment's.
pub struct Shown {
    pub row: Response,
    pub segments: Vec<Response>,
}

impl<'a, T: PartialEq + Copy> Segmented<'a, T> {
    /// Caps segments, each as wide as its words: the box hugs them.
    pub fn caps(selected: &'a mut T, choices: &'a [(T, &'a str)]) -> Segmented<'a, T> {
        Segmented { selected, choices, style: Style::Caps, width: f32::INFINITY, off: None }
    }

    /// Small segments sharing the box's width.
    pub fn small(selected: &'a mut T, choices: &'a [(T, &'a str)]) -> Segmented<'a, T> {
        Segmented { selected, choices, style: Style::Small, width: f32::INFINITY, off: None }
    }

    /// The small style's box this wide, or as wide as there is room for (the default fills the
    /// row). The caps style hugs its words.
    pub fn width(mut self, width: f32) -> Segmented<'a, T> {
        self.width = width;
        self
    }

    /// Draws the choice `off` (when there is one) off: faint, and no click chooses it.
    pub fn off(mut self, off: Option<T>) -> Segmented<'a, T> {
        self.off = off;
        self
    }

    /// Its segments as `ui` draws them.
    fn segments(&self, ui: &Ui) -> Vec<Segment<'a>> {
        let (style, width, n) = (self.style, self.width, self.choices.len());
        let segment = |words, on, i: usize| {
            let s = Segment { words, style, selected: on, divider: i + 1 < n && !on, width: None, look: None };
            match style {
                Style::Caps => s,
                // Each small segment gets its share of the room inside the edge.
                Style::Small => s.width((width.min(ui.available_width()) - 2.0).max(0.0) / n.max(1) as f32),
            }
        };
        self.choices.iter().enumerate().map(|(i, (value, words))| segment(*words, *value == *self.selected, i)).collect()
    }

    /// How wide it draws in `ui`: its segments and its edge.
    pub fn drawn_width(&self, ui: &Ui) -> f32 {
        self.segments(ui).iter().map(|s| s.parts(ui).2).sum::<f32>() + 2.0
    }

    /// Draws it; the box's response (changed when a click or Space chose another) and each
    /// segment's.
    pub fn show(self, ui: &mut Ui) -> Shown {
        let segments = self.segments(ui);
        let Segmented { selected, choices, off, .. } = self;
        let n = choices.len();
        let widths: Vec<f32> = segments.iter().map(|s| s.parts(ui).2).collect();
        let (rect, mut row) = ui.allocate_exact_size(vec2(widths.iter().sum::<f32>() + 2.0, SEGMENTED_H), Sense::hover());
        let visible = ui.is_rect_visible(rect);
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let radius = CornerRadius::same(RADIUS_CONTROL);
        // The grounds under the edge go here, between the box's ground and the segments.
        let under = visible.then(|| {
            p.rect_filled(rect, radius, t.card);
            p.add(Shape::Noop)
        });
        let chosen: Vec<bool> = segments.iter().map(|s| s.selected).collect();
        let mut grounds = Vec::new();
        let mut inside = ui.new_child(UiBuilder::new().max_rect(rect.shrink(1.0)).layout(Layout::left_to_right(Align::Min)));
        let mut drawn = Vec::with_capacity(n);
        let mut x = rect.left() + 1.0;
        for (((value, _), s), w) in choices.iter().zip(segments).zip(widths) {
            // Each in its own place, so egui's rounding of the widths doesn't add up along the row.
            let at = Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(w, SEGMENT_H));
            let r = inside.add_enabled_ui(off != Some(*value), |ui| ui.put(at, s)).inner;
            x += w;
            let i = drawn.len();
            if let Some(colour) = ground(t, chosen.get(i).copied().unwrap_or(false), r.hovered(), !r.enabled()) {
                // The segment's ground again, out to the box's outside where it meets the edge.
                let (first, last) = (i == 0, i + 1 == n);
                let reach = Rect::from_min_max(
                    pos2(if first { rect.left() } else { at.left() }, rect.top()),
                    pos2(if last { rect.right() } else { at.right() }, rect.bottom()),
                );
                let (west, east) = (if first { RADIUS_CONTROL } else { 0 }, if last { RADIUS_CONTROL } else { 0 });
                let corners = CornerRadius { nw: west, sw: west, ne: east, se: east };
                grounds.push(Shape::rect_filled(reach, corners, colour));
            }
            if r.clicked() && *selected != *value {
                *selected = *value;
                row.mark_changed();
            }
            drawn.push(r);
        }
        if let Some(under) = under {
            p.set(under, Shape::Vec(grounds));
            p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        }
        Shown { row, segments: drawn }
    }
}

impl<T: PartialEq + Copy> Widget for Segmented<'_, T> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui).row
    }
}

#[cfg(test)]
mod tests {
    use egui::{CornerRadius, Key, Rect, pos2, vec2};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, ROOM, both, edges_in, fills, fills_in, harness, painted, texts};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// A segment's cell: all of it but the divider's slot at its right end.
    fn cell(at: Rect) -> Rect {
        Rect::from_min_max(at.min, pos2(at.right() - 1.0, at.bottom()))
    }

    #[test]
    fn a_small_segment_short_of_room_is_cut_and_whole_on_hover() {
        // The SSL modes in a 150 wide box: each share is 29.6, too narrow for `verify-full`.
        const MODES: [(u8, &str); 5] = [(0, "disable"), (1, "prefer"), (2, "require"), (3, "verify-ca"), (4, "verify-full")];
        let shown = |shapes: &[egui::epaint::ClippedShape]| -> Vec<String> {
            crate::components::flat(shapes)
                .into_iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t) => Some(t.galley.rows.iter().map(|r| r.text()).collect()),
                    _ => None,
                })
                .collect()
        };
        for (width, cut) in [(150.0, true), (600.0, false)] {
            let mut h = crate::components::harness(egui::Theme::Light, Vec::new(), move |ui, at: &mut Vec<Rect>| {
                ui.add_space(ROOM);
                let mut chosen = 0u8;
                let drawn = Segmented::small(&mut chosen, &MODES).width(width).show(ui);
                *at = drawn.segments.iter().map(|r| r.rect).collect();
            });
            h.run();
            let whole = |h: &egui_kittest::Harness<'_, Vec<Rect>>| shown(&h.output().shapes).iter().filter(|w| *w == "verify-full").count();
            let drawn = shown(&h.output().shapes);
            assert_eq!(drawn.iter().any(|w| w.ends_with('…')), cut, "{width}: cut {drawn:?}");
            let before = whole(&h);
            assert_eq!(before, usize::from(!cut), "{width}: {drawn:?}");
            let last = h.state()[4];
            h.get_by_label("verify-full").hover();
            h.run();
            h.run();
            assert_eq!(whole(&h) > before, cut, "{width}: whole on hover only when cut ({last:?})");
        }
    }

    #[test]
    fn each_segment_state_matches_the_page() {
        // Node 161186: 26 tall. Selected: `ink`, its words `on_ink`; hovering it changes nothing;
        // off on `fill`. Not selected: no ground, `hover` when hovered, words `text`. Off: words
        // `text_faint`. Focus within the cell (the segment less its divider slot): a selected one
        // a 1 px `on_ink` line 2 px inside, corners 1; another its edge 2 px `ink`, square.
        // Caps: `label` capitals, 12 either side; small: `small` as written, centred.
        let hovered = Look { hovered: true, ..Look::default() };
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for style in [Style::Caps, Style::Small] {
                for selected in [true, false] {
                    for state in ["default", "hover", "focus", "disabled"] {
                        let at_ = format!("{theme:?} {style:?} selected {selected} {state}");
                        let mut at = Rect::NOTHING;
                        let shapes = painted(theme, |ui| {
                            ui.add_space(ROOM);
                            ui.horizontal(|ui| {
                                ui.add_space(ROOM);
                                let s = match style {
                                    Style::Caps => Segment::caps("Data", selected),
                                    Style::Small => Segment::small("prefer", selected).width(77.0),
                                };
                                at = match state {
                                    "disabled" => ui.add_enabled(false, s).rect,
                                    "hover" => ui.add(s.look(hovered)).rect,
                                    "focus" => ui.add(s.look(focused)).rect,
                                    _ => ui.add(s).rect,
                                };
                            });
                        });
                        assert_eq!(at.height(), 26.0, "{at_}: 26 tall");
                        match style {
                            Style::Caps => assert!((at.width() - 51.0).abs() <= 1.0, "{at_}: 12, DATA, 12: {at:?}"),
                            Style::Small => assert_eq!(at.width(), 77.0, "{at_}: as wide as given"),
                        }
                        let want: Vec<(Rect, egui::Color32)> = match (selected, state) {
                            (true, "disabled") => vec![(at, t.fill)],
                            (true, _) => vec![(at, t.ink)],
                            (false, "hover") => vec![(at, t.hover)],
                            (false, _) => vec![],
                        };
                        assert_eq!(fills_in(&shapes, at), want, "{at_}: the ground");
                        let want = match (selected, state) {
                            (true, "focus") => {
                                vec![Edge { outer: cell(at).shrink(2.0), width: 1.0, colour: t.on_ink, radius: CornerRadius::same(1) }]
                            }
                            (false, "focus") => vec![Edge { outer: cell(at), width: 2.0, colour: t.ink, radius: CornerRadius::ZERO }],
                            _ => vec![],
                        };
                        assert_eq!(edges_in(&shapes, at, 0.0), want, "{at_}: the focus");
                        let colour = match (selected, state) {
                            (_, "disabled") => t.text_faint,
                            (true, _) => t.on_ink,
                            (false, _) => t.text,
                        };
                        let (shown, line) = match style {
                            Style::Caps => ("DATA", 14.0),
                            Style::Small => ("prefer", 16.0),
                        };
                        let drawn = texts(&shapes);
                        let (_, words, c) = drawn.iter().find(|(s, _, _)| s == shown).unwrap_or_else(|| panic!("{at_}: `{shown}`: {drawn:?}"));
                        assert_eq!(*c, colour, "{at_}: the words' colour");
                        assert!(close(words.height(), line) && close(words.center().y, at.center().y), "{at_}: {words:?} centred in {at:?}");
                        match style {
                            Style::Caps => assert!(close(words.left(), at.left() + 12.0), "{at_}: {words:?} 12 into {at:?}"),
                            Style::Small => assert!(close(words.center().x, at.center().x), "{at_}: {words:?} centred in {at:?}"),
                        }
                        assert!(!drawn.iter().any(|(s, _, _)| s == "Data" || s == "PREFER"), "{at_}: caps only drawn, small as written");
                    }
                }
            }
        }
    }

    #[test]
    fn each_segmented_control_matches_the_page() {
        // Node 161251: a 28 box, `card` in a 1 px `line_strong` edge, corners 2; the segments 26
        // tall inside the edge, small ones sharing its width, caps ones as wide as their words. A
        // segment that isn't chosen and isn't the last ends in a 1 px `line` divider.
        const MODES: [(u8, &str); 5] = [(0, "disable"), (1, "prefer"), (2, "require"), (3, "verify-ca"), (4, "verify-full")];
        const VIEWS: [(u8, &str); 2] = [(0, "Data"), (1, "Structure")];
        for (theme, t) in both() {
            let mut shown = (Rect::NOTHING, Vec::new(), Rect::NOTHING);
            let shapes = painted(theme, |ui| {
                ui.add_space(ROOM);
                ui.horizontal(|ui| {
                    ui.add_space(ROOM);
                    let mut mode = 1;
                    let s = Segmented::small(&mut mode, &MODES).width(386.0).show(ui);
                    shown.0 = s.row.rect;
                    shown.1 = s.segments.iter().map(|r| r.rect).collect();
                    let mut view = 0;
                    shown.2 = ui.add(Segmented::caps(&mut view, &VIEWS)).rect;
                });
            });
            let (at, segments, caps) = shown;
            assert_eq!(at.size(), vec2(386.0, 28.0), "{theme:?}: the small box");
            assert_eq!(fills_in(&shapes, at).first(), Some(&(at, t.card)), "{theme:?}: on card");
            let edge = Edge { outer: at, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, at, 0.0), [edge], "{theme:?}: its edge");
            assert_eq!(segments.len(), 5, "{theme:?}: five segments");
            let each = (386.0 - 2.0) / 5.0;
            let fills = fills(&shapes);
            for (i, s) in segments.iter().enumerate() {
                let want = Rect::from_min_size(pos2(at.left() + 1.0 + i as f32 * each, at.top() + 1.0), vec2(each, 26.0));
                assert!(
                    (s.min - want.min).length() < 0.05 && (s.max - want.max).length() < 0.05,
                    "{theme:?}: segment {i} {s:?}, the page's {want:?}"
                );
                let divider = (Rect::from_min_size(pos2(s.right() - 1.0, s.top()), vec2(1.0, 26.0)), t.line);
                assert_eq!(fills.contains(&divider), [0, 2, 3].contains(&i), "{theme:?}: segment {i}'s divider");
            }
            assert!(fills.contains(&(segments[1], t.ink)), "{theme:?}: prefer chosen, on ink");
            assert_eq!(caps.height(), 28.0, "{theme:?}: the caps box");
            assert!((caps.width() - (2.0 + 51.0 + 86.0)).abs() <= 2.0, "{theme:?}: the caps box hugs DATA and STRUCTURE: {caps:?}");
        }
    }

    #[test]
    fn a_chosen_segments_ground_runs_under_the_boxs_edge() {
        // The edge is 1 point from the box's outside (1.25 px at 125 %) and a ground ends on a
        // whole pixel, so a ground that stopped at the edge's inside left a sliver of `card`
        // beside the chosen end segment. The ground runs on to the box's outside, with the box's
        // corners there, and the edge is drawn after it; a middle segment's reaches up and down.
        const VIEWS: [(u8, &str); 2] = [(0, "Data"), (1, "Structure")];
        const AUTHS: [(u8, &str); 3] = [(0, "Key"), (1, "Agent"), (2, "Password")];
        let order = |shapes: &[egui::epaint::ClippedShape], ground: (Rect, egui::Color32), edge: Rect| {
            let flat = crate::components::flat(shapes);
            let at = |want: &dyn Fn(&egui::epaint::RectShape) -> bool| flat.iter().position(|s| matches!(s, Shape::Rect(r) if want(r)));
            let same = |a: Rect, b: Rect| (a.min - b.min).length() < 0.05 && (a.max - b.max).length() < 0.05;
            let ground = at(&|r| same(r.rect, ground.0) && r.fill == ground.1);
            (ground, at(&|r| r.rect == edge && r.stroke.width == 1.0))
        };
        for (theme, t) in both() {
            for chosen in [0u8, 1] {
                let mut shown = (Rect::NOTHING, Vec::new());
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    let mut view = chosen;
                    let s = Segmented::caps(&mut view, &VIEWS).show(ui);
                    shown = (s.row.rect, s.segments.iter().map(|r| r.rect).collect());
                });
                let (at, segments) = shown;
                let own = segments[usize::from(chosen)];
                let reach = match chosen {
                    0 => Rect::from_min_max(at.min, pos2(own.right(), at.bottom())),
                    _ => Rect::from_min_max(pos2(own.left(), at.top()), at.max),
                };
                let (ground, edge) = order(&shapes, (reach, t.ink), at);
                assert!(ground.is_some(), "{theme:?} {chosen}: the chosen ground out to the box's outside {reach:?}: {:?}", fills_in(&shapes, at));
                assert!(ground < edge, "{theme:?} {chosen}: the edge is drawn over it: {ground:?}, {edge:?}");
                let corners: Vec<CornerRadius> = crate::components::flat(&shapes)
                    .into_iter()
                    .filter_map(|s| match s {
                        Shape::Rect(r) if r.rect == reach && r.fill == t.ink => Some(r.corner_radius),
                        _ => None,
                    })
                    .collect();
                let (west, east) = if chosen == 0 { (2, 0) } else { (0, 2) };
                assert_eq!(corners, [CornerRadius { nw: west, sw: west, ne: east, se: east }], "{theme:?} {chosen}: the box's corners at its end");
                let other = segments[usize::from(1 - chosen)];
                assert!(
                    !fills_in(&shapes, at).iter().any(|(r, c)| *c != t.card && r.intersects(other.shrink(1.5))),
                    "{theme:?} {chosen}: none under the other"
                );
            }
            // A middle segment's ground reaches the box's top and bottom, and no further sideways.
            let mut shown = (Rect::NOTHING, Vec::new());
            let shapes = painted(theme, |ui| {
                ui.add_space(ROOM);
                let mut auth = 1u8;
                let s = Segmented::small(&mut auth, &AUTHS).width(300.0).show(ui);
                shown = (s.row.rect, s.segments.iter().map(|r| r.rect).collect());
            });
            let (at, segments) = shown;
            let reach = Rect::from_x_y_ranges(segments[1].x_range(), at.y_range());
            let (ground, edge) = order(&shapes, (reach, t.ink), at);
            assert!(ground.is_some() && ground < edge, "{theme:?}: the middle one's ground from the box's top to its bottom, under the edge");
        }
    }

    #[test]
    fn a_segment_is_chosen_with_the_keyboard() {
        const AUTHS: [(u8, &str); 3] = [(0, "Key"), (1, "Agent"), (2, "Password")];
        for (theme, t) in both() {
            let mut h = harness(theme, (0u8, Vec::<Rect>::new()), |ui, (chosen, at)| {
                ui.add_space(ROOM);
                let s = Segmented::small(chosen, &AUTHS).width(300.0).show(ui);
                *at = s.segments.iter().map(|r| r.rect).collect();
            });
            h.run();
            h.key_press(Key::Tab);
            h.run();
            let at = h.state().1.clone();
            let line = Edge { outer: cell(at[0]).shrink(2.0), width: 1.0, colour: t.on_ink, radius: CornerRadius::same(1) };
            assert_eq!(edges_in(&h.output().shapes, at[0], 0.0), [line], "{theme:?}: Tab to the chosen one: the on_ink line");
            h.key_press(Key::Tab);
            h.run();
            let edge = Edge { outer: cell(at[1]), width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
            assert_eq!(edges_in(&h.output().shapes, at[1], 0.0), [edge], "{theme:?}: Tab on: its edge turns ink");
            h.key_press(Key::Space);
            h.run();
            h.run();
            assert_eq!(h.state().0, 1, "{theme:?}: Space chose Agent");
            let line = Edge { outer: cell(at[1]).shrink(2.0), width: 1.0, colour: t.on_ink, radius: CornerRadius::same(1) };
            assert_eq!(edges_in(&h.output().shapes, at[1], 0.0), [line], "{theme:?}: chosen, its focus is the on_ink line");
            h.get_by_label("Password").click();
            h.run();
            h.run();
            assert_eq!(h.state().0, 2, "{theme:?}: a click chose Password");
            assert_eq!(edges_in(&h.output().shapes, at[2], 0.0), [], "{theme:?}: a click shows no focus");
            assert!(h.query_by_label("PASSWORD").is_none(), "{theme:?}: its words as written");
        }
    }
}

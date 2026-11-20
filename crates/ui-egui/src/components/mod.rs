//! The shared controls of the design's Components page, one file per control, built only from
//! [`crate::theme`]: each is an [`egui::Widget`] that works out its own look ([`Look`]) from its
//! response, so a screen never sets a control's colour, size, padding or edge.

pub mod button;
pub mod checkbox;
pub mod chip;
pub mod dialog;
pub mod dropdown;
pub mod editor;
pub mod error_band;
pub mod focus;
pub mod grid_cell;
pub mod icon;
pub mod input;
pub mod keycap;
pub mod logo;
pub mod menu;
pub mod quick_search;
pub mod results;
pub mod rows;
pub mod segmented;
pub mod showcase;
pub mod sidebar;
pub mod spinner;
pub mod status;
pub mod strip;
pub mod structure;
pub mod swatch;
pub mod tab;
pub mod table_bars;
pub mod toast;
pub mod tooltip;
pub mod value;

/// How a control is drawn this frame: hovered, showing keyboard focus, off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub hovered: bool,
    pub focused: bool,
    pub disabled: bool,
}

impl Look {
    /// The look `r` has now: `focused` only for focus that came from the keyboard
    /// ([`focus::shown`]), `disabled` when it is off.
    pub fn of(ui: &egui::Ui, r: &egui::Response) -> Look {
        Look { hovered: r.hovered(), focused: focus::shown(ui, r), disabled: !r.enabled() }
    }
}

/// The painter a control draws with: `ui`'s, without the fading egui gives a disabled `Ui`, since
/// a control that is off draws its own look (`text_faint`).
pub(crate) fn painter(ui: &egui::Ui) -> egui::Painter {
    let mut p = ui.painter().clone();
    let alpha = ui.visuals().disabled_alpha();
    if !ui.is_enabled() && alpha > 0.0 {
        p.set_opacity(p.opacity() / alpha);
    }
    p
}

/// Draws `add` in a scope of `ui`. When `on`, at half strength in its normal look, taking no
/// input: a cover over the scope takes the pointer, and the keyboard leaves any widget in it
/// (those of `guard`, and any drawn inside the scope), before and after `add` draws, so neither a
/// click nor Tab nor typing reaches them. Tab and Shift+Tab step over the scope: the keyboard
/// goes on to the widget after it, or back to the one before it. The scope is the same whether
/// `on` or not, so what is in it keeps its ids (a scroll area its offset).
pub fn dim<R>(ui: &mut egui::Ui, on: bool, guard: &[egui::Id], add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let area = ui.max_rect().intersect(ui.clip_rect());
    if on {
        // Shift+Tab hands the keyboard over a frame late: it arrives here from after the scope.
        let_go(ui.ctx(), guard, area, egui::FocusDirection::Previous);
    }
    let inner = ui
        .scope_builder(egui::UiBuilder::new(), |ui| {
            if on {
                ui.multiply_opacity(crate::theme::size::DIM_OPACITY);
            }
            add(ui)
        })
        .inner;
    if on {
        // Tab hands it over within the frame: it arrives here from before the scope.
        let_go(ui.ctx(), guard, area, egui::FocusDirection::Next);
        // The cover takes the pointer, never the keyboard.
        ui.interact(area, ui.id().with("dbee-dim"), egui::Sense::CLICK | egui::Sense::DRAG);
    }
    inner
}

/// The keyboard leaves the widget that has it when it is one of `guard` or lies in `area`. When
/// it got there this frame (Tab or Shift+Tab), it goes on in `onward` to the next widget outside.
fn let_go(ctx: &egui::Context, guard: &[egui::Id], area: egui::Rect, onward: egui::FocusDirection) {
    let Some(focused) = ctx.memory(|m| m.focused()) else { return };
    let inside = guard.contains(&focused) || ctx.read_response(focused).is_some_and(|r| area.contains_rect(r.rect));
    if inside {
        let arrived = !ctx.memory(|m| m.had_focus_last_frame(focused));
        ctx.memory_mut(|m| {
            m.surrender_focus(focused);
            if arrived {
                m.move_focus(onward);
            }
        });
    }
}

/// What [`painted`] gives: every shape of the last frame, nested lists flattened.
#[cfg(test)]
pub(crate) fn flat(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Shape> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            s => out.push(s.clone()),
        }
    }
    let mut out = Vec::new();
    for c in shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// One frame's shapes of `add`, drawn in `theme` with dbee's theme and fonts: the frame after the
/// fonts are bound.
#[cfg(test)]
pub(crate) fn painted(theme: egui::Theme, add: impl FnMut(&mut egui::Ui)) -> Vec<egui::epaint::ClippedShape> {
    painted_in(egui::vec2(800.0, 400.0), theme, add)
}

/// [`painted`] in a window of `size`.
#[cfg(test)]
pub(crate) fn painted_in(size: egui::Vec2, theme: egui::Theme, mut add: impl FnMut(&mut egui::Ui)) -> Vec<egui::epaint::ClippedShape> {
    let mut ready = false;
    let h = egui_kittest::Harness::builder().with_size(size).build_ui(move |ui| {
        if !ready {
            showcase::prepare(ui.ctx(), theme);
            ready = true;
            ui.ctx().request_repaint();
            return;
        }
        add(ui);
    });
    h.output().shapes.clone()
}

/// Both modes with their tokens.
#[cfg(test)]
pub(crate) fn both() -> [(egui::Theme, crate::theme::Tokens); 2] {
    [(egui::Theme::Light, crate::theme::LIGHT), (egui::Theme::Dark, crate::theme::DARK)]
}

/// Room above a control drawn alone: the harness paints a ground the size of what it holds.
#[cfg(test)]
pub(crate) const ROOM: f32 = 10.0;

/// A harness drawing `add` in `theme` with dbee's theme and fonts, from its second frame on, with
/// hover texts at once.
#[cfg(test)]
pub(crate) fn harness<S>(theme: egui::Theme, state: S, add: impl FnMut(&mut egui::Ui, &mut S) + 'static) -> egui_kittest::Harness<'static, S> {
    harness_in(egui::vec2(600.0, 200.0), theme, state, add)
}

/// [`harness`] in a window of `size`.
#[cfg(test)]
pub(crate) fn harness_in<S>(
    size: egui::Vec2,
    theme: egui::Theme,
    state: S,
    mut add: impl FnMut(&mut egui::Ui, &mut S) + 'static,
) -> egui_kittest::Harness<'static, S> {
    let mut ready = false;
    egui_kittest::Harness::builder().with_size(size).build_ui_state(
        move |ui, s: &mut S| {
            if !ready {
                showcase::prepare(ui.ctx(), theme);
                ui.ctx().all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
                ready = true;
                ui.ctx().request_repaint();
                return;
            }
            add(ui, s);
        },
        state,
    )
}

/// A rect's edge as drawn: the outside of its stroke, its width, colour and corners.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Edge {
    pub outer: egui::Rect,
    pub width: f32,
    pub colour: egui::Color32,
    pub radius: egui::CornerRadius,
}

/// The edges of the rects in `shapes`.
#[cfg(test)]
pub(crate) fn edges(shapes: &[egui::epaint::ClippedShape]) -> Vec<Edge> {
    flat(shapes)
        .into_iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color.a() > 0 => {
                let out = match r.stroke_kind {
                    egui::StrokeKind::Inside => 0.0,
                    egui::StrokeKind::Middle => r.stroke.width / 2.0,
                    egui::StrokeKind::Outside => r.stroke.width,
                };
                Some(Edge { outer: r.rect.expand(out), width: r.stroke.width, colour: r.stroke.color, radius: r.corner_radius })
            }
            _ => None,
        })
        .collect()
}

/// The edges drawn on or within `at` grown by `out`.
#[cfg(test)]
pub(crate) fn edges_in(shapes: &[egui::epaint::ClippedShape], at: egui::Rect, out: f32) -> Vec<Edge> {
    edges(shapes).into_iter().filter(|e| at.expand(out).contains_rect(e.outer)).collect()
}

/// The fills drawn within `at`.
#[cfg(test)]
pub(crate) fn fills_in(shapes: &[egui::epaint::ClippedShape], at: egui::Rect) -> Vec<(egui::Rect, egui::Color32)> {
    fills(shapes).into_iter().filter(|(r, _)| at.contains_rect(*r)).collect()
}

/// The filled rects in `shapes`, each with its fill.
#[cfg(test)]
pub(crate) fn fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Color32)> {
    flat(shapes)
        .into_iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 => Some((r.rect, r.fill)),
            _ => None,
        })
        .collect()
}

/// The colours of the icon drawn within `at` (its lines and discs), and the box it fills.
#[cfg(test)]
pub(crate) fn icon_in(shapes: &[egui::epaint::ClippedShape], at: egui::Rect) -> (Vec<egui::Color32>, egui::Rect) {
    let solid = |c: &egui::epaint::ColorMode| match c {
        egui::epaint::ColorMode::Solid(c) => *c,
        egui::epaint::ColorMode::UV(_) => egui::Color32::RED,
    };
    let mut colours = Vec::new();
    let mut bounds = egui::Rect::NOTHING;
    for s in flat(shapes) {
        let drawn = match &s {
            egui::Shape::Path(p) => vec![p.fill, solid(&p.stroke.color)],
            egui::Shape::Circle(c) => vec![c.fill, c.stroke.color],
            egui::Shape::LineSegment { stroke, .. } => vec![stroke.color],
            _ => continue,
        };
        let r = s.visual_bounding_rect();
        if !at.contains_rect(r) {
            continue;
        }
        bounds = bounds.union(r);
        for c in drawn.into_iter().filter(|c| *c != egui::Color32::TRANSPARENT) {
            if !colours.contains(&c) {
                colours.push(c);
            }
        }
    }
    (colours, bounds)
}

/// The images drawn in `shapes` (a mesh on a texture other than the fonts'): each one's rect, its
/// tint, and its texture's name and size in pixels in `ctx`.
#[cfg(test)]
pub(crate) fn images(ctx: &egui::Context, shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Color32, String, [usize; 2])> {
    let textures = ctx.tex_manager();
    let textures = textures.read();
    flat(shapes)
        .into_iter()
        .filter_map(|s| match s {
            egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                let tint = m.vertices.first().map_or(egui::Color32::TRANSPARENT, |v| v.color);
                assert!(m.vertices.iter().all(|v| v.color == tint), "an image in one tint: {:?}", m.vertices);
                let (name, size) = textures.meta(m.texture_id).map(|t| (t.name.clone(), t.size)).unwrap_or_default();
                Some((m.calc_bounds(), tint, name, size))
            }
            _ => None,
        })
        .collect()
}

/// The texts in `shapes`: each section's words as drawn, where its text was drawn, and its colour
/// as seen (the shape's fallback for a placeholder, its fading applied).
#[cfg(test)]
pub(crate) fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect, egui::Color32)> {
    let mut out = Vec::new();
    for s in flat(shapes) {
        if let egui::Shape::Text(t) = s {
            let rect = t.galley.rect.translate(t.pos.to_vec2());
            for section in &t.galley.job.sections {
                let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
                let words = t.galley.job.text.get(start..end).unwrap_or_default().to_owned();
                let colour = match t.override_text_color {
                    Some(c) => c,
                    None if section.format.color == egui::Color32::PLACEHOLDER => t.fallback_color,
                    None => section.format.color,
                };
                let colour = if t.opacity_factor < 1.0 { colour.gamma_multiply(t.opacity_factor) } else { colour };
                out.push((words, rect, colour));
            }
        }
    }
    out
}

/// The parts of the text `whole` drawn in `shapes`: each section's words, font and colour as seen.
#[cfg(test)]
pub(crate) fn sections(shapes: &[egui::epaint::ClippedShape], whole: &str) -> Vec<(String, egui::FontId, egui::Color32)> {
    let drawn = flat(shapes).into_iter().find_map(|s| match s {
        egui::Shape::Text(t) if t.galley.text() == whole => Some(t),
        _ => None,
    });
    let Some(t) = drawn else { return Vec::new() };
    let fonts = t.galley.job.sections.iter().map(|s| s.format.font_id.clone());
    let seen = texts(&[egui::epaint::ClippedShape { clip_rect: egui::Rect::EVERYTHING, shape: egui::Shape::Text(t.clone()) }]);
    seen.into_iter().zip(fonts).map(|((words, _, colour), font)| (words, font, colour)).collect()
}

#[cfg(test)]
mod tests {
    use egui::{CornerRadius, Rect, pos2, vec2};

    use super::*;
    use crate::theme::{DARK, LIGHT};

    /// A click at the middle of `at`: pressed in one frame, released in the next.
    fn click_at<S>(h: &mut egui_kittest::Harness<'_, S>, at: Rect) {
        let (pos, modifiers) = (at.center(), egui::Modifiers::NONE);
        let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
        h.input_mut().events.extend([egui::Event::PointerMoved(pos), press(true)]);
        h.step();
        h.input_mut().events.push(press(false));
        h.step();
        h.step();
    }

    #[test]
    fn dim_draws_at_half_and_takes_no_click() {
        let mut alpha = [0u8; 2];
        for (n, on) in [false, true].into_iter().enumerate() {
            let mut h = harness(egui::Theme::Light, (0u32, Rect::NOTHING), move |ui, (clicks, at)| {
                ui.add_space(ROOM);
                dim(ui, on, &[], |ui| {
                    let r = ui.add(button::BoxButton::primary("Go"));
                    *at = r.rect;
                    if r.clicked() {
                        *clicks += 1;
                    }
                });
            });
            h.run();
            let at = h.state().1;
            let fill = fills(&h.output().shapes).into_iter().find(|(r, _)| *r == at).map(|(_, c)| c);
            alpha[n] = fill.unwrap_or_else(|| panic!("on {on}: the button's ground drawn")).a();
            click_at(&mut h, at);
            assert_eq!(h.state().0, u32::from(!on), "on {on}: clicked only when not dimmed");
        }
        assert!(alpha[1].abs_diff(alpha[0] / 2) <= 1, "half: {alpha:?}");
    }

    /// Tab and Shift+Tab step over a dimmed area: from the button before it to the one after it,
    /// and back, the keyboard never landing in it (nor nowhere).
    #[test]
    fn tab_steps_over_a_dimmed_area() {
        let ids = [egui::Id::new("before"), egui::Id::new("in-1"), egui::Id::new("in-2"), egui::Id::new("after")];
        let mut h = harness(egui::Theme::Light, (), move |ui, _| {
            let [before, in_1, in_2, after] = ids;
            let _ = ui.interact(Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0)), before, egui::Sense::click());
            dim(ui, true, &[], |ui| {
                let _ = ui.add(egui::TextEdit::singleline(&mut String::from("one")).id(in_1));
                let _ = ui.add(egui::TextEdit::singleline(&mut String::from("two")).id(in_2));
            });
            let _ = ui.interact(Rect::from_min_size(pos2(1.0, 0.0), vec2(1.0, 1.0)), after, egui::Sense::click());
        });
        h.run();
        let [before, _, _, after] = ids;
        let tab = |h: &mut egui_kittest::Harness<'_, ()>, shift: bool| {
            let modifiers = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
            h.input_mut().events.push(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers });
            h.run();
            h.ctx.memory(|m| m.focused())
        };
        h.ctx.memory_mut(|m| m.request_focus(before));
        h.run();
        assert_eq!(tab(&mut h, false), Some(after), "Tab from before the area goes past it");
        assert_eq!(tab(&mut h, true), Some(before), "Shift+Tab from after it goes back past it");
    }

    #[test]
    fn keycap_and_chip_match_the_page() {
        // Nodes 161025 (keycap) and 161031 (chip) of the Components page.
        for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
            let mut at = [Rect::NOTHING; 4];
            let shapes = painted(theme, |ui| {
                ui.horizontal(|ui| {
                    at[0] = ui.add(keycap::Keycap::new("Ctrl")).rect;
                    at[1] = ui.add_enabled(false, keycap::Keycap::new("Z")).rect;
                    at[2] = ui.add(chip::Chip::next("Upcoming")).rect;
                    at[3] = ui.add(chip::Chip::available("Available")).rect;
                });
            });
            let (edges, fills, texts) = (edges(&shapes), fills(&shapes), texts(&shapes));
            let edge = |r: Rect| edges.iter().find(|e| e.outer == r).copied();
            let text = |words: &str| texts.iter().find(|(w, _, _)| w == words).map(|(_, r, c)| (*r, *c));
            let close = |a: f32, b: f32| (a - b).abs() < 0.05;
            let [key, off, next, available] = at;
            for r in at {
                assert_eq!(r.height(), 18.0, "{theme:?}: {r:?} is 18 tall");
            }
            for (rect, words, colour) in [(key, "CTRL", t.text_muted), (off, "Z", t.text_faint)] {
                let edge_of = Edge { outer: rect, width: 1.0, colour: t.raised_line, radius: CornerRadius::same(2) };
                assert_eq!(edge(rect), Some(edge_of), "{theme:?} {words}: a 1 px raised_line edge, radius 2");
                assert!(fills.contains(&(rect, t.raised)), "{theme:?} {words}: on raised, unfaded: {fills:?}");
                let (r, c) = text(words).unwrap_or_else(|| panic!("{theme:?}: {words} drawn: {texts:?}"));
                assert_eq!(c, colour, "{theme:?} {words}");
                assert!(close(r.left(), rect.left() + 5.0) && close(r.center().y, rect.center().y), "{theme:?} {words} at {r:?} in {rect:?}");
                assert!(close(rect.width(), r.width() + 10.0), "{theme:?} {words}: 4 px each side inside the edge");
            }
            assert!(text("Ctrl").is_none(), "{theme:?}: the key only in capitals");

            assert_eq!(edge(next), Some(Edge { outer: next, width: 1.0, colour: t.line, radius: CornerRadius::ZERO }), "{theme:?}: next");
            assert!(!fills.iter().any(|(r, _)| next.contains_rect(*r)), "{theme:?}: next has no ground and no mark");
            let (r, c) = text("UPCOMING").unwrap_or_else(|| panic!("{theme:?}: UPCOMING drawn: {texts:?}"));
            assert_eq!(c, t.text_muted, "{theme:?}: next's words");
            assert!(close(r.left(), next.left() + 7.0) && close(next.width(), r.width() + 14.0), "{theme:?}: next {r:?} in {next:?}");

            let edge_of = Edge { outer: available, width: 1.0, colour: t.line_strong, radius: CornerRadius::ZERO };
            assert_eq!(edge(available), Some(edge_of), "{theme:?}: available");
            let mark = Rect::from_min_size(pos2(available.left() + 7.0, available.center().y - 3.0), vec2(6.0, 6.0));
            assert!(fills.contains(&(mark, t.success)), "{theme:?}: available's mark {mark:?}: {fills:?}");
            let (r, c) = text("AVAILABLE").unwrap_or_else(|| panic!("{theme:?}: AVAILABLE drawn: {texts:?}"));
            assert_eq!(c, t.text, "{theme:?}: available's words");
            assert!(close(r.left(), available.left() + 19.0) && close(available.width(), r.width() + 26.0), "{theme:?}: available {r:?}");
        }
    }
}

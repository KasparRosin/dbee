//! The checkbox (`checkbox`): a 14 box and its label in `body`, 8 apart. Its label is reported to
//! accessibility as written; its focus is a box around the box and the label together.

use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::icon::Icon;
use super::{Look, focus, painter};
use crate::theme::size::{CHECKBOX_BOX, CHECKBOX_GAP, RADIUS_CONTROL};
use crate::theme::{Text, tokens};

/// A checkbox, 18 tall, ticking `checked` on a click or Space; off when added with
/// `ui.add_enabled(false, …)`.
pub struct Checkbox<'a> {
    checked: &'a mut bool,
    label: &'a str,
    look: Option<Look>,
}

impl<'a> Checkbox<'a> {
    pub fn new(checked: &'a mut bool, label: &'a str) -> Checkbox<'a> {
        Checkbox { checked, label, look: None }
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Checkbox<'a> {
        self.look = Some(look);
        self
    }
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let words = ui.painter().layout_job(Text::Body.job(self.label, Color32::PLACEHOLDER));
        let size = vec2(CHECKBOX_BOX + CHECKBOX_GAP + words.size().x, Text::Body.line());
        let (rect, mut r) = ui.allocate_exact_size(size, Sense::click());
        if r.clicked() {
            *self.checked = !*self.checked;
            r.mark_changed();
        }
        let checked = *self.checked;
        r.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), checked, self.label));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let radius = CornerRadius::same(RADIUS_CONTROL);
        let boxed = Rect::from_min_size(pos2(rect.left(), rect.center().y - CHECKBOX_BOX / 2.0), Vec2::splat(CHECKBOX_BOX));
        if checked {
            p.rect_filled(boxed, radius, if look.disabled { t.text_faint } else { t.ink });
            Icon::CheckBox.paint(&p, boxed.center(), t.on_ink);
        } else {
            p.rect_filled(boxed, radius, t.card);
            let edge = if look.disabled { t.text_faint } else { t.line_strong };
            p.rect_stroke(boxed, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        let colour = if look.disabled { t.text_faint } else { t.text };
        p.galley(pos2(boxed.right() + CHECKBOX_GAP, rect.center().y - words.size().y / 2.0), words, colour);
        if look.focused {
            focus::box_around(&p, rect);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use egui::{CornerRadius, Key, Rect, Theme, Vec2, vec2};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, harness, icon_in, painted, texts};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    #[test]
    fn each_checkbox_state_has_its_look() {
        // 18 tall; a 14 box with corners 2, 2 down, then 8, then the label in `body`.
        // On: `ink` with the check in `on_ink`; off: `card` in a 1 px `line_strong` edge. Disabled:
        // the box `text_faint` (on) or a `text_faint` edge (off), the label `text_faint`. Focus: a
        // 2 px `ink` box 4 px around the box and the label together.
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for checked in [true, false] {
                for state in ["default", "focus", "disabled"] {
                    let at_ = format!("{theme:?} checked {checked} {state}");
                    let mut at = Rect::NOTHING;
                    let shapes = painted(theme, |ui| {
                        ui.add_space(ROOM);
                        ui.horizontal(|ui| {
                            ui.add_space(ROOM);
                            let mut on = checked;
                            let c = Checkbox::new(&mut on, "Read-only");
                            at = match state {
                                "disabled" => ui.add_enabled(false, c).rect,
                                "focus" => ui.add(c.look(focused)).rect,
                                _ => ui.add(c).rect,
                            };
                        });
                    });
                    assert_eq!(at.height(), 18.0, "{at_}: 18 tall");
                    let boxed = Rect::from_min_size(at.min + vec2(0.0, 2.0), Vec2::splat(14.0));
                    let off = state == "disabled";
                    let (ground, edge) = match (checked, off) {
                        (true, false) => (t.ink, None),
                        (true, true) => (t.text_faint, None),
                        (false, false) => (t.card, Some(t.line_strong)),
                        (false, true) => (t.card, Some(t.text_faint)),
                    };
                    assert_eq!(fills_in(&shapes, at), [(boxed, ground)], "{at_}: the box's ground");
                    let mut want: Vec<Edge> =
                        edge.map(|colour| Edge { outer: boxed, width: 1.0, colour, radius: CornerRadius::same(2) }).into_iter().collect();
                    if state == "focus" {
                        want.push(Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) });
                    }
                    assert_eq!(edges_in(&shapes, at, 4.0), want, "{at_}: the edges");
                    let (colours, bounds) = icon_in(&shapes, boxed);
                    if checked {
                        assert_eq!(colours, [t.on_ink], "{at_}: the check");
                        assert!((bounds.center() - boxed.center()).length() <= 1.0, "{at_}: the check {bounds:?} centred in {boxed:?}");
                    } else {
                        assert_eq!(colours, [], "{at_}: no check");
                    }
                    let drawn = texts(&shapes);
                    let (_, words, colour) =
                        drawn.iter().find(|(s, _, _)| s == "Read-only").unwrap_or_else(|| panic!("{at_}: the label as written: {drawn:?}"));
                    assert_eq!(*colour, if off { t.text_faint } else { t.text }, "{at_}: the label's colour");
                    assert!(close(words.left(), at.left() + 22.0), "{at_}: the label {words:?} 8 after the box in {at:?}");
                    assert!(close(words.height(), 18.0) && close(words.center().y, at.center().y), "{at_}: the label in body, centred");
                    assert!(close(at.right(), words.right()), "{at_}: as wide as the box, 8 and the label");
                    assert!(!drawn.iter().any(|(s, _, _)| s == "READ-ONLY"), "{at_}: never in capitals");
                }
            }
        }
    }

    /// What the keyboard test's window saw: the checkbox's value, its rect, and the focus box.
    struct Seen {
        on: bool,
        at: Rect,
    }

    fn focus_box(h: &egui_kittest::Harness<'_, Seen>, theme: Theme) -> bool {
        let t = if theme == Theme::Dark { crate::theme::DARK } else { crate::theme::LIGHT };
        let at = h.state().at;
        let ring = Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) };
        edges_in(&h.output().shapes, at, 4.0).contains(&ring)
    }

    #[test]
    fn space_toggles_a_focused_checkbox() {
        for (theme, _) in both() {
            let mut h = harness(theme, Seen { on: false, at: Rect::NOTHING }, |ui, s| {
                ui.add_space(ROOM);
                ui.horizontal(|ui| {
                    ui.add_space(ROOM);
                    ui.add(crate::components::button::TextButton::new("Before"));
                    s.at = ui.add(Checkbox::new(&mut s.on, "Save in keyring")).rect;
                });
            });
            h.run();
            h.key_press(Key::Tab);
            h.run();
            h.key_press(Key::Tab);
            h.run();
            assert!(focus_box(&h, theme), "{theme:?}: Tab reached it, and its focus shows");
            h.key_press(Key::Space);
            h.run();
            assert!(h.state().on, "{theme:?}: Space ticked it");
            h.key_press(Key::Space);
            h.run();
            assert!(!h.state().on, "{theme:?}: Space again unticked it");
            assert!(focus_box(&h, theme), "{theme:?}: and it keeps the keyboard");
            let node = h.get_by_label("Save in keyring");
            assert_eq!(node.accesskit_node().role(), egui::accesskit::Role::CheckBox, "{theme:?}: a checkbox, its label as written");
            node.click();
            h.run();
            assert!(h.state().on, "{theme:?}: a click ticks it");
            assert!(!focus_box(&h, theme), "{theme:?}: a click shows no focus");
        }
    }
}

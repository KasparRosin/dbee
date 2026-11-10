//! Keyboard focus: whether a control shows it ([`shown`]), and the four looks foundations' Focus
//! rule gives it. Every control draws focus through these.
//!
//! The look shows whenever a control has focus, unless that focus came from a mouse or touch
//! press on it: Tab, Shift+Tab and the arrows show it, and so does focus given by code (a dialog
//! focusing its field as it opens). egui doesn't say where focus came from, so [`shown`] judges a
//! control when it gains focus, from its own response: the pointer on it in that frame, or in the
//! frame before (a control may ask for focus after working out its look), makes it a pointer
//! focus. The judgement holds while the control keeps focus, so egui handing focus over a frame
//! after the key (Shift+Tab, Tab past the last control) still shows; a press on the control makes
//! its focus a pointer one again.

use egui::{CornerRadius, Id, Painter, Rect, Response, Stroke, StrokeKind, Ui};

use crate::theme::size::{FOCUS_AROUND, FOCUS_INSET, FOCUS_LINE_W, FOCUS_W, RADIUS_CONTROL, RADIUS_FOCUS_AROUND, RADIUS_FOCUS_LINE};
use crate::theme::tokens;

/// What [`shown`] keeps between frames, in the context's memory.
#[derive(Clone, Copy, Default)]
struct Seen {
    /// The control judged when it gained focus, and whether its look shows.
    judged: Option<(Id, bool)>,
    /// The last control the pointer was pressed on, and in which frame.
    pointer: Option<(Id, u64)>,
}

/// Whether `r` shows focus now: it has focus, and that focus didn't come from a mouse or touch
/// press on it.
pub fn shown(ui: &Ui, r: &Response) -> bool {
    let ctx = ui.ctx();
    let key = Id::new("dbee-focus-seen");
    let frame = ctx.cumulative_frame_nr();
    let mut seen: Seen = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let pressed = r.is_pointer_button_down_on() || r.interact_pointer_pos().is_some();
    if pressed {
        seen.pointer = Some((r.id, frame));
    }
    let show = r.has_focus() && {
        let gained = !ctx.memory(|m| m.had_focus_last_frame(r.id));
        let lately = seen.pointer.is_some_and(|(id, at)| id == r.id && at.saturating_add(1) >= frame);
        let show = match seen.judged {
            _ if pressed => false,
            Some((id, show)) if id == r.id && !gained => show,
            _ => !lately,
        };
        seen.judged = Some((r.id, show));
        show
    };
    ctx.data_mut(|d| d.insert_temp(key, seen));
    show
}

fn ink(painter: &Painter, rect: Rect, radius: u8) {
    let t = tokens(painter.ctx());
    painter.rect_stroke(rect, CornerRadius::same(radius), Stroke::new(FOCUS_W, t.ink), StrokeKind::Inside);
}

/// A control's own edge, `rect` with corners `radius`, turned 2 px `ink`, drawn inside.
pub fn own_edge(painter: &Painter, rect: Rect, radius: u8) {
    ink(painter, rect, radius);
}

/// A 2 px `ink` box around a control with no edge, reaching 4 px out from `rect`.
pub fn box_around(painter: &Painter, rect: Rect) {
    ink(painter, rect.expand(FOCUS_AROUND), RADIUS_FOCUS_AROUND);
}

/// A 2 px `ink` box 2 px inside a bar button's cell.
pub fn box_inside(painter: &Painter, rect: Rect) {
    ink(painter, rect.shrink(FOCUS_INSET), RADIUS_CONTROL);
}

/// A 1 px `on_ink` line 2 px inside `rect`, whatever is filled with `ink` (the main button, a
/// selected segment). The page gives it corners of 1 whatever the corners of the ground.
pub fn on_ink_line(painter: &Painter, rect: Rect) {
    let t = tokens(painter.ctx());
    let stroke = Stroke::new(FOCUS_LINE_W, t.on_ink);
    painter.rect_stroke(rect.shrink(FOCUS_INSET), CornerRadius::same(RADIUS_FOCUS_LINE), stroke, StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use egui::{Key, Modifiers, Sense, WidgetInfo, WidgetType, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, Look, edges, painted};
    use crate::theme::{DARK, LIGHT};

    /// Two focusable probes, as later controls are built: what each one's look and focus were in
    /// the last frame. `give` hands one focus by code, after its look is worked out, as a dialog
    /// focuses its field when it opens.
    #[derive(Default)]
    struct Probes {
        looks: [Look; 2],
        focus: [bool; 2],
        give: Option<usize>,
    }

    fn probes() -> Harness<'static, Probes> {
        Harness::builder().with_size(vec2(300.0, 100.0)).build_ui_state(
            |ui, s: &mut Probes| {
                ui.horizontal(|ui| {
                    for (i, name) in ["first", "second"].into_iter().enumerate() {
                        let (_, r) = ui.allocate_exact_size(vec2(60.0, 28.0), Sense::click());
                        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
                        // egui gives focus on a click only to what asks for it, as a text field does:
                        // the first asks before working out its look, the second after.
                        if i == 0 && r.clicked() {
                            r.request_focus();
                        }
                        s.looks[i] = Look::of(ui, &r);
                        s.focus[i] = r.has_focus();
                        if i == 1 && r.clicked() {
                            r.request_focus();
                        }
                        if s.give == Some(i) {
                            r.request_focus();
                            s.give = None;
                            ui.ctx().request_repaint();
                        }
                    }
                });
            },
            Probes::default(),
        )
    }

    fn focused(h: &Harness<'_, Probes>) -> [bool; 2] {
        h.state().looks.map(|l| l.focused)
    }

    #[test]
    fn focus_shows_after_tab_not_after_a_click() {
        let mut h = probes();
        assert_eq!(focused(&h), [false, false], "nothing has focus yet");
        h.key_press(Key::Tab);
        h.run();
        assert_eq!(focused(&h), [true, false], "Tab gives the first focus, and it shows");
        h.get_by_label("second").click();
        h.run();
        assert_eq!(h.state().focus, [false, true], "the click gave the second egui's focus");
        assert_eq!(focused(&h), [false, false], "a click never shows it");
        // Shift+Tab hands focus over a frame later; from the last, Tab comes round to the first.
        h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        h.run();
        assert_eq!(focused(&h), [true, false], "Shift+Tab back to the first");
        h.key_press(Key::Tab);
        h.run();
        assert_eq!(focused(&h), [false, true], "Tab on to the second");
        h.key_press(Key::Tab);
        h.run();
        assert_eq!(focused(&h), [true, false], "Tab from the last comes round to the first");
        // A click on what the keyboard focused keeps its focus, and the look goes.
        h.get_by_label("first").click();
        h.run();
        assert_eq!(h.state().focus, [true, false]);
        assert_eq!(focused(&h), [false, false], "the click on it hides the look");
    }

    #[test]
    fn focus_given_by_code_shows() {
        let mut h = probes();
        h.get_by_label("second").click();
        h.run();
        assert_eq!((h.state().focus, focused(&h)), ([false, true], [false, false]), "the click's focus shows nothing");
        // Right after that click, code gives the first focus, as a dialog opened from a menu does.
        h.state_mut().give = Some(0);
        h.run();
        assert_eq!(h.state().focus, [true, false]);
        assert_eq!(focused(&h), [true, false], "focus given by code shows");
        // A click on it keeps the focus and hides the look; Tab away and back shows it again.
        h.get_by_label("first").click();
        h.run();
        assert_eq!((h.state().focus, focused(&h)), ([true, false], [false, false]));
        h.key_press(Key::Tab);
        h.run();
        h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        h.run();
        assert_eq!(focused(&h), [true, false], "back with Shift+Tab, the look shows again");
    }

    #[test]
    fn focus_handed_over_late_to_the_first_control_ever_focused_shows() {
        // A control, then a widget that isn't one and takes focus with a click. Shift+Tab from that
        // widget hands focus to the control a frame after the key.
        let mut h = Harness::builder().with_size(vec2(300.0, 100.0)).build_ui_state(
            |ui, s: &mut Probes| {
                ui.horizontal(|ui| {
                    let (_, r) = ui.allocate_exact_size(vec2(60.0, 28.0), Sense::click());
                    s.looks[0] = Look::of(ui, &r);
                    s.focus[0] = r.has_focus();
                    let (_, other) = ui.allocate_exact_size(vec2(60.0, 28.0), Sense::click());
                    other.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "other"));
                    if other.clicked() {
                        other.request_focus();
                    }
                });
            },
            Probes::default(),
        );
        h.get_by_label("other").click();
        h.run();
        h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        h.run();
        assert!(h.state().focus[0], "Shift+Tab gave the control focus");
        assert!(h.state().looks[0].focused, "and it shows");
    }

    #[test]
    fn the_four_focus_looks_sit_where_foundations_says() {
        let rect = Rect::from_min_size(egui::pos2(100.0, 100.0), vec2(60.0, 28.0));
        for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
            let look = |draw: fn(&Painter, Rect)| -> Vec<Edge> { edges(&painted(theme, |ui| draw(ui.painter(), rect))) };
            let ink = |outer: Rect, radius: u8| Edge { outer, width: 2.0, colour: t.ink, radius: egui::CornerRadius::same(radius) };
            assert_eq!(look(|p, r| own_edge(p, r, 2)), [ink(rect, 2)], "{theme:?}: own edge, 2 px ink inside");
            assert_eq!(look(|p, r| own_edge(p, r, 0)), [ink(rect, 0)], "{theme:?}: a square cell's edge");
            assert_eq!(look(box_around), [ink(rect.expand(4.0), 4)], "{theme:?}: a box 4 px around, 2 px from the control");
            assert_eq!(look(box_inside), [ink(rect.shrink(2.0), 2)], "{theme:?}: a box 2 px inside the cell");
            let line = Edge { outer: rect.shrink(2.0), width: 1.0, colour: t.on_ink, radius: egui::CornerRadius::same(1) };
            assert_eq!(look(on_ink_line), [line], "{theme:?}: a 1 px on_ink line 2 px inside");
        }
    }
}

//! Keyboard focus: whether a control shows it ([`shown`]), and the four looks foundations' Focus
//! rule gives it. Every control draws focus through these.
//!
//! egui doesn't say where focus came from, so [`Source`] reads every frame's input before egui
//! does, from the first frame a control is drawn: the keys that move focus (Tab, Shift+Tab, the
//! arrows) make the next focus a keyboard one, a pointer press a pointer one. A control is judged
//! when it gains focus and keeps that judgement while it holds focus; a pointer press clears it,
//! so a click on a control the keyboard focused hides the look. Judging on gaining, not in the
//! frame of the key, also covers egui handing focus over a frame late (Shift+Tab, Tab past the
//! last control).

use egui::{CornerRadius, Event, Id, Key, Painter, RawInput, Rect, Response, Stroke, StrokeKind, Ui};

use crate::theme::size::{FOCUS_AROUND, FOCUS_INSET, FOCUS_LINE_W, FOCUS_W, RADIUS_CONTROL, RADIUS_FOCUS_AROUND, RADIUS_FOCUS_LINE};
use crate::theme::tokens;

/// How focus last moved, read from the input ([`egui::Plugin::input_hook`]).
#[derive(Default)]
struct Source {
    /// The last key or press that moves focus was a key.
    keyboard: bool,
    /// The control judged when it gained focus, and whether that focus came from the keyboard.
    judged: Option<(Id, bool)>,
}

impl Source {
    fn read(&mut self, events: &[Event]) {
        for e in events {
            match e {
                Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if !modifiers.any() || modifiers.shift_only() => {
                    self.keyboard = true;
                }
                Event::Key { key: Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight, pressed: true, modifiers, .. }
                    if !modifiers.any() =>
                {
                    self.keyboard = true;
                }
                Event::PointerButton { pressed: true, .. } => {
                    self.keyboard = false;
                    self.judged = None;
                }
                _ => {}
            }
        }
    }
}

impl egui::Plugin for Source {
    fn debug_name(&self) -> &'static str {
        "dbee keyboard focus"
    }

    fn setup(&mut self, ctx: &egui::Context) {
        // Added by the first control drawn, after this frame's input went by.
        ctx.input(|i| self.read(&i.raw.events));
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut RawInput) {
        self.read(&input.events);
    }
}

/// Whether `r` shows focus now: it has focus, and that focus came from the keyboard.
pub fn shown(ui: &Ui, r: &Response) -> bool {
    let ctx = ui.ctx();
    let source = ctx.plugin_or_default::<Source>();
    if !r.has_focus() {
        return false;
    }
    let gained = !ctx.memory(|m| m.had_focus_last_frame(r.id));
    let mut s = source.lock();
    match s.judged {
        Some((id, keyboard)) if id == r.id && !gained => keyboard,
        _ => {
            let keyboard = s.keyboard;
            s.judged = Some((r.id, keyboard));
            keyboard
        }
    }
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
    /// the last frame.
    #[derive(Default)]
    struct Probes {
        looks: [Look; 2],
        focus: [bool; 2],
    }

    fn probes() -> Harness<'static, Probes> {
        Harness::builder().with_size(vec2(300.0, 100.0)).build_ui_state(
            |ui, s: &mut Probes| {
                ui.horizontal(|ui| {
                    for (i, name) in ["first", "second"].into_iter().enumerate() {
                        let (_, r) = ui.allocate_exact_size(vec2(60.0, 28.0), Sense::click());
                        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
                        // egui gives focus on a click only to what asks for it, as a text field does.
                        if r.clicked() {
                            r.request_focus();
                        }
                        s.looks[i] = Look::of(ui, &r);
                        s.focus[i] = r.has_focus();
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

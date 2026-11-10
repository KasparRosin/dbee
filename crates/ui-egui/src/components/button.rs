//! Boxed and text buttons: page components `button/box` (styles primary, outline and danger) and
//! `button/text`. Their words are drawn in `button-caps` and reported to accessibility as written.

use std::sync::Arc;

use egui::epaint::TextShape;
use egui::{Color32, CornerRadius, Galley, Response, Sense, Stroke, StrokeKind, Ui, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::{Look, focus, painter};
use crate::theme::size::{BUTTON_GAP, BUTTON_H, BUTTON_PAD_X, RADIUS_CONTROL, TEXT_BUTTON_PAD_X};
use crate::theme::{Text, tokens};

/// A shortcut on a filled button is its words' colour at this opacity, as the page draws it.
const SHORTCUT_ON_FILL: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Primary,
    Outline,
    Danger,
}

/// A boxed button, 28 tall; off when added with `ui.add_enabled(false, …)`.
pub struct BoxButton<'a> {
    label: &'a str,
    style: Style,
    shortcut: Option<&'a str>,
    min_width: f32,
    look: Option<Look>,
}

impl<'a> BoxButton<'a> {
    /// The main button: `ink`, its words `on_ink`.
    pub fn primary(label: &'a str) -> BoxButton<'a> {
        BoxButton { label, style: Style::Primary, shortcut: None, min_width: 0.0, look: None }
    }

    /// A `line_strong` edge and no ground of its own: it shows what it sits on.
    pub fn outline(label: &'a str) -> BoxButton<'a> {
        BoxButton { label, style: Style::Outline, shortcut: None, min_width: 0.0, look: None }
    }

    /// A destructive answer (Close anyway, Delete): `danger`, its words `on_danger`.
    pub fn danger(label: &'a str) -> BoxButton<'a> {
        BoxButton { label, style: Style::Danger, shortcut: None, min_width: 0.0, look: None }
    }

    /// `keys` drawn after the words.
    pub fn shortcut(mut self, keys: &'a str) -> BoxButton<'a> {
        self.shortcut = Some(keys);
        self
    }

    /// At least `width` wide, its words centred.
    pub fn min_width(mut self, width: f32) -> BoxButton<'a> {
        self.min_width = width;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> BoxButton<'a> {
        self.look = Some(look);
        self
    }

    /// How wide it draws in `ui`.
    pub fn width(&self, ui: &Ui) -> f32 {
        self.parts(ui).2
    }

    /// The words, the shortcut, and the width they make with the padding and the edge.
    fn parts(&self, ui: &Ui) -> (Arc<Galley>, Option<Arc<Galley>>, f32) {
        let words = ui.painter().layout_job(Text::ButtonCaps.job(self.label, Color32::PLACEHOLDER));
        let keys = self.shortcut.map(|k| ui.painter().layout_job(Text::Label.job(k, Color32::PLACEHOLDER)));
        let edge = if self.style == Style::Outline { 1.0 } else { 0.0 };
        let inner = words.size().x + keys.as_ref().map_or(0.0, |k| BUTTON_GAP + k.size().x);
        let width = (inner + 2.0 * (BUTTON_PAD_X + edge)).ceil().max(self.min_width);
        (words, keys, width)
    }
}

impl Widget for BoxButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (words, keys, width) = self.parts(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(width, BUTTON_H), Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let radius = CornerRadius::same(RADIUS_CONTROL);
        let (ground, on) = match self.style {
            Style::Primary => (Some(t.ink), t.on_ink),
            Style::Danger => (Some(t.danger), t.on_danger),
            Style::Outline => (None, t.text),
        };
        let (words_c, keys_c, keys_opacity) = match (look.disabled, ground) {
            (true, _) => (t.text_faint, t.text_faint, 1.0),
            (false, Some(_)) => (on, on, SHORTCUT_ON_FILL),
            (false, None) => (on, t.text_muted, 1.0),
        };
        match ground {
            Some(_) if look.disabled => {
                p.rect_filled(rect, radius, t.fill);
            }
            Some(fill) => {
                p.rect_filled(rect, radius, fill);
                if look.focused {
                    match self.style {
                        Style::Danger => focus::on_danger_line(&p, rect),
                        _ => focus::on_ink_line(&p, rect),
                    }
                }
            }
            None => {
                if look.hovered && !look.disabled {
                    p.rect_filled(rect, radius, t.hover);
                }
                if look.focused {
                    focus::own_edge(&p, rect, RADIUS_CONTROL);
                } else {
                    p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
                }
            }
        }
        let inner = words.size().x + keys.as_ref().map_or(0.0, |k| BUTTON_GAP + k.size().x);
        let mut x = rect.center().x - inner / 2.0;
        let words_w = words.size().x;
        p.galley(pos2(x, rect.center().y - words.size().y / 2.0), words, words_c);
        if let Some(keys) = keys {
            x += words_w + BUTTON_GAP;
            let at = pos2(x, rect.center().y - keys.size().y / 2.0);
            p.add(TextShape::new(at, keys, keys_c).with_opacity_factor(keys_opacity));
        }
        r
    }
}

/// A button that is only its words, 28 tall; off when added with `ui.add_enabled(false, …)`.
pub struct TextButton<'a> {
    label: &'a str,
    look: Option<Look>,
}

impl<'a> TextButton<'a> {
    pub fn new(label: &'a str) -> TextButton<'a> {
        TextButton { label, look: None }
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> TextButton<'a> {
        self.look = Some(look);
        self
    }

    /// How wide it draws in `ui`.
    pub fn width(&self, ui: &Ui) -> f32 {
        self.parts(ui).1
    }

    fn parts(&self, ui: &Ui) -> (Arc<Galley>, f32) {
        let words = ui.painter().layout_job(Text::ButtonCaps.job(self.label, Color32::PLACEHOLDER));
        let width = (words.size().x + 2.0 * TEXT_BUTTON_PAD_X).ceil();
        (words, width)
    }
}

impl Widget for TextButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (words, width) = self.parts(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(width, BUTTON_H), Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        if look.hovered && !look.disabled {
            p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), t.hover);
        }
        let colour = if look.disabled { t.text_faint } else { t.text };
        let at = pos2(rect.center().x - words.size().x / 2.0, rect.center().y - words.size().y / 2.0);
        p.galley(at, words, colour);
        if look.focused {
            focus::box_around(&p, rect);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, CornerRadius, Key, Rect, Theme};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, edges, fills, painted, showcase, texts};
    use crate::theme::{DARK, LIGHT, Tokens};

    fn both() -> [(Theme, Tokens); 2] {
        [(Theme::Light, LIGHT), (Theme::Dark, DARK)]
    }

    /// Fills drawn within `at`.
    fn fills_in(shapes: &[egui::epaint::ClippedShape], at: Rect) -> Vec<(Rect, Color32)> {
        fills(shapes).into_iter().filter(|(r, _)| at.contains_rect(*r)).collect()
    }

    /// Edges drawn on or within `at` grown by `out`.
    fn edges_in(shapes: &[egui::epaint::ClippedShape], at: Rect, out: f32) -> Vec<Edge> {
        edges(shapes).into_iter().filter(|e| at.expand(out).contains_rect(e.outer)).collect()
    }

    /// Room above a button drawn alone: the harness paints a ground the size of what it holds.
    const ROOM: f32 = 10.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// A harness drawing `add` in `theme` with dbee's theme and fonts, from its second frame on.
    fn harness<S>(theme: Theme, state: S, mut add: impl FnMut(&mut Ui, &mut S) + 'static) -> Harness<'static, S> {
        let mut ready = false;
        Harness::builder().with_size(egui::vec2(600.0, 200.0)).build_ui_state(
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

    #[test]
    fn each_box_button_state_matches_the_page() {
        // Node 160992: 28 tall, corners 2, the words 12 inside the box (and inside the outline's
        // 1 px edge), a shortcut 8 after them.
        for (theme, t) in both() {
            for style in [Style::Primary, Style::Outline, Style::Danger] {
                for state in ["default", "hover", "focus", "disabled"] {
                    let at_ = format!("{theme:?} {style:?} {state}");
                    let mut at = Rect::NOTHING;
                    let shapes = painted(theme, |ui| {
                        ui.add_space(ROOM);
                        let b = BoxButton { label: "Run", style, shortcut: Some("Ctrl+Enter"), min_width: 0.0, look: None };
                        at = match state {
                            "disabled" => ui.add_enabled(false, b).rect,
                            "hover" => ui.add(b.look(Look { hovered: true, ..Look::default() })).rect,
                            "focus" => ui.add(b.look(Look { focused: true, ..Look::default() })).rect,
                            _ => ui.add(b).rect,
                        };
                    });
                    assert_eq!(at.height(), 28.0, "{at_}: 28 tall");
                    let (ground, on) = match style {
                        Style::Primary => (t.ink, t.on_ink),
                        Style::Danger => (t.danger, t.on_danger),
                        Style::Outline => (Color32::TRANSPARENT, t.text),
                    };
                    let radius = CornerRadius::same(2);
                    let (want_fills, want_edges, words, keys) = match (style, state) {
                        (Style::Outline, "disabled") => {
                            (vec![], vec![Edge { outer: at, width: 1.0, colour: t.line_strong, radius }], t.text_faint, t.text_faint)
                        }
                        (Style::Outline, "focus") => (vec![], vec![Edge { outer: at, width: 2.0, colour: t.ink, radius }], t.text, t.text_muted),
                        (Style::Outline, "hover") => {
                            (vec![(at, t.hover)], vec![Edge { outer: at, width: 1.0, colour: t.line_strong, radius }], t.text, t.text_muted)
                        }
                        (Style::Outline, _) => (vec![], vec![Edge { outer: at, width: 1.0, colour: t.line_strong, radius }], t.text, t.text_muted),
                        (_, "disabled") => (vec![(at, t.fill)], vec![], t.text_faint, t.text_faint),
                        (_, "focus") => {
                            let line = Edge { outer: at.shrink(2.0), width: 1.0, colour: on, radius: CornerRadius::same(1) };
                            (vec![(at, ground)], vec![line], on, on.gamma_multiply(0.7))
                        }
                        // The filled styles have no hover look.
                        _ => (vec![(at, ground)], vec![], on, on.gamma_multiply(0.7)),
                    };
                    assert_eq!(fills_in(&shapes, at), want_fills, "{at_}: the ground");
                    assert_eq!(edges_in(&shapes, at, 0.0), want_edges, "{at_}: the edge");
                    let texts = texts(&shapes);
                    let text = |w: &str| texts.iter().find(|(s, _, _)| s == w).map(|(_, r, c)| (*r, *c));
                    let (run, run_c) = text("RUN").unwrap_or_else(|| panic!("{at_}: RUN drawn: {texts:?}"));
                    let (ctrl, ctrl_c) = text("CTRL+ENTER").unwrap_or_else(|| panic!("{at_}: CTRL+ENTER drawn: {texts:?}"));
                    assert_eq!((run_c, ctrl_c), (words, keys), "{at_}: the words' and the shortcut's colours");
                    let inset = if style == Style::Outline { 13.0 } else { 12.0 };
                    assert!(close(run.left(), at.left() + inset), "{at_}: RUN {run:?} starts {inset} into {at:?}");
                    assert!(
                        close(at.right() - ctrl.right(), inset) || close(at.right() - ctrl.right(), inset - 0.6),
                        "{at_}: {ctrl:?} ends {inset} before {at:?}"
                    );
                    assert!((ctrl.left() - run.right() - 8.0).abs() <= 0.7, "{at_}: 8 between {run:?} and {ctrl:?}");
                    assert!(close(run.center().y, at.center().y) && close(ctrl.center().y, at.center().y), "{at_}: centred");
                }
            }
        }
    }

    #[test]
    fn an_outline_button_in_a_dialog_shows_the_dialog_ground() {
        let t = DARK;
        let mut at = [Rect::NOTHING; 2];
        let shapes = painted(Theme::Dark, |ui| {
            egui::Frame::new().fill(t.raised).inner_margin(20).show(ui, |ui| {
                at[0] = ui.add(BoxButton::outline("Cancel")).rect;
                at[1] = ui.add(BoxButton::outline("Cancel").look(Look { hovered: true, ..Look::default() })).rect;
            });
        });
        assert!(at[0].height() > 0.0, "drawn");
        assert_eq!(fills_in(&shapes, at[0]), [], "no ground of its own: the dialog's raised shows");
        assert_eq!(fills_in(&shapes, at[1]), [(at[1], t.hover)], "hovered: hover over raised");
        assert_ne!(t.raised.blend(t.hover), t.raised, "and hover reads on raised");
    }

    /// A harness with one button `add` draws, tabbed to: what it drew, and its rect.
    fn tabbed(theme: Theme, add: fn(&mut Ui) -> Response) -> (Vec<egui::epaint::ClippedShape>, Rect) {
        let mut h = harness(theme, Rect::NOTHING, move |ui, at| {
            ui.add_space(ROOM);
            *at = add(ui).rect;
        });
        h.run();
        h.key_press(Key::Tab);
        h.run();
        (h.output().shapes.clone(), *h.state())
    }

    #[test]
    fn the_main_button_focus_is_an_on_ink_line() {
        // In dark `ink` is light, so the line is dark: `on_ink`.
        for (theme, t) in both() {
            let (shapes, at) = tabbed(theme, |ui| ui.add(BoxButton::primary("Apply")));
            let line = Edge { outer: at.shrink(2.0), width: 1.0, colour: t.on_ink, radius: CornerRadius::same(1) };
            assert_eq!(edges_in(&shapes, at, 0.0), [line], "{theme:?}: a 1 px on_ink line 2 px inside");
            assert_eq!(fills_in(&shapes, at), [(at, t.ink)], "{theme:?}: on ink");
            let (shapes, at) = tabbed(theme, |ui| ui.add(BoxButton::danger("Delete")));
            let line = Edge { outer: at.shrink(2.0), width: 1.0, colour: t.on_danger, radius: CornerRadius::same(1) };
            assert_eq!(edges_in(&shapes, at, 0.0), [line], "{theme:?}: danger's line is on_danger");
        }
    }

    #[test]
    fn text_button_focus_is_a_box_around_it() {
        // Node 161015: no ground, no edge; the words 10 in from either end; focus a 2 px ink box
        // 4 px around it.
        for (theme, t) in both() {
            let mut at = [Rect::NOTHING; 3];
            let shapes = painted(theme, |ui| {
                at[0] = ui.add(TextButton::new("Reset")).rect;
                at[1] = ui.add(TextButton::new("Reset").look(Look { hovered: true, ..Look::default() })).rect;
                at[2] = ui.add_enabled(false, TextButton::new("Reset")).rect;
            });
            let texts = texts(&shapes);
            for (i, (fill, colour)) in [(None, t.text), (Some(t.hover), t.text), (None, t.text_faint)].into_iter().enumerate() {
                let r = at[i];
                assert_eq!(r.height(), 28.0, "{theme:?}: 28 tall");
                assert_eq!(fills_in(&shapes, r), fill.map(|f| (r, f)).into_iter().collect::<Vec<_>>(), "{theme:?} {i}: the ground");
                assert_eq!(edges_in(&shapes, r, 0.0), [], "{theme:?} {i}: no edge");
                let (_, w, c) = texts.iter().find(|(s, w, _)| s == "RESET" && r.contains_rect(*w)).unwrap_or_else(|| panic!("{texts:?}"));
                assert_eq!(*c, colour, "{theme:?} {i}");
                assert!(close(w.left(), r.left() + 10.0), "{theme:?} {i}: {w:?} 10 into {r:?}");
            }
            let (shapes, at) = tabbed(theme, |ui| ui.add(TextButton::new("Reset")));
            let ring = Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) };
            assert_eq!(edges_in(&shapes, at, 4.0), [ring], "{theme:?}: the box around it");
        }
    }

    /// What the off test's window saw: clicks on each button and which has focus.
    struct Seen {
        clicks: [u32; 3],
        focus: [bool; 3],
        at: Rect,
    }

    #[test]
    fn a_button_that_is_off_cannot_be_clicked_or_tabbed_to_and_says_why() {
        for (theme, t) in both() {
            let seen = Seen { clicks: [0; 3], focus: [false; 3], at: Rect::NOTHING };
            let mut h = harness(theme, seen, |ui, s| {
                ui.horizontal(|ui| {
                    let rs = [
                        ui.add(TextButton::new("Before")),
                        ui.add_enabled(false, BoxButton::primary("Run")).on_disabled_hover_text("Run: not now, nothing to run"),
                        ui.add(TextButton::new("After")),
                    ];
                    s.at = rs[1].rect;
                    for (i, r) in rs.iter().enumerate() {
                        s.clicks[i] += u32::from(r.clicked());
                        s.focus[i] = r.has_focus();
                    }
                });
            });
            h.run();
            h.get_by_label("Run").click();
            h.run();
            assert_eq!(h.state().clicks, [0, 0, 0], "{theme:?}: a click on it does nothing");
            h.key_press(Key::Tab);
            h.run();
            h.key_press(Key::Tab);
            h.run();
            assert_eq!(h.state().focus, [false, false, true], "{theme:?}: Tab goes past it");
            h.get_by_label("Run").hover();
            h.run();
            h.run();
            let shapes = h.output().shapes.clone();
            let drawn = texts(&shapes);
            assert!(drawn.iter().any(|(s, _, _)| s == "Run: not now, nothing to run"), "{theme:?}: hovered, it says why: {drawn:?}");
            let at = h.state().at;
            let run = drawn.iter().find(|(s, _, _)| s == "RUN").map(|(_, _, c)| *c);
            assert_eq!(run, Some(t.text_faint), "{theme:?}: its words are faint");
            assert_eq!(fills_in(&shapes, at), [(at, t.fill)], "{theme:?}: a main button off is on fill");
        }
    }

    #[test]
    fn buttons_draw_capitals_and_report_their_words() {
        let mut h = harness(Theme::Light, (), |ui, _| {
            ui.add(BoxButton::outline("Run current").shortcut("Ctrl+Shift+Enter"));
            ui.add(TextButton::new("Save connection"));
        });
        h.run();
        for words in ["Run current", "Save connection"] {
            let node = h.query_by_role_and_label(egui::accesskit::Role::Button, words);
            assert!(node.is_some(), "a button labelled `{words}`, as written");
        }
        assert!(h.query_by_label("RUN CURRENT").is_none(), "the capitals are only drawn");
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        assert_eq!(drawn, ["RUN CURRENT", "CTRL+SHIFT+ENTER", "SAVE CONNECTION"]);
    }

    #[test]
    fn enter_and_space_press_a_focused_button() {
        let mut h = harness(Theme::Light, [0u32; 2], |ui, clicks| {
            clicks[0] += u32::from(ui.add(BoxButton::primary("Apply")).clicked());
            clicks[1] += u32::from(ui.add(TextButton::new("Reset")).clicked());
        });
        h.run();
        h.key_press(Key::Tab);
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(*h.state(), [1, 0], "Enter pressed the focused one");
        h.key_press(Key::Tab);
        h.run();
        h.key_press(Key::Space);
        h.run();
        assert_eq!(*h.state(), [1, 1], "Space pressed the next");
    }
}

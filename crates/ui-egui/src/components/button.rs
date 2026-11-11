//! The buttons of the page: `button/box` (styles primary, outline and danger), `button/text`,
//! `button/icon` (boxed and plain) and `button/bar` (an icon with words, or an icon alone). Their
//! words are drawn in capitals and reported to accessibility as written; an icon alone is named
//! by its icon.

use std::sync::Arc;

use egui::epaint::TextShape;
use egui::{Color32, CornerRadius, Galley, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::icon::Icon;
use super::{Look, focus, painter};
use crate::theme::size::{
    BAR_BUTTON_DIVIDER, BAR_BUTTON_GAP, BAR_BUTTON_H, BAR_BUTTON_PAD_X, BAR_CELL_H, BUTTON_GAP, BUTTON_H, BUTTON_PAD_X, ICON_BUTTON, RADIUS_CONTROL,
    TEXT_BUTTON_PAD_X,
};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconStyle {
    Boxed,
    Plain,
}

/// An icon button, 28 square (`button/icon`); off when added with `ui.add_enabled(false, …)`.
pub struct IconButton {
    icon: Icon,
    style: IconStyle,
    on: Option<bool>,
    look: Option<Look>,
}

impl IconButton {
    /// A `line_strong` edge and no ground of its own: the filter bar's add, remove and raw SQL.
    pub fn boxed(icon: Icon) -> IconButton {
        IconButton { icon, style: IconStyle::Boxed, on: None, look: None }
    }

    /// No edge: the paging arrows.
    pub fn plain(icon: Icon) -> IconButton {
        IconButton { icon, style: IconStyle::Plain, on: None, look: None }
    }

    /// A toggle, on or off (the raw SQL toggle). On, a boxed one has a `fill` ground inside its
    /// edge, hovered or not; the page draws no on look for a plain one.
    pub fn on(mut self, on: bool) -> IconButton {
        self.on = Some(on);
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> IconButton {
        self.look = Some(look);
        self
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, r) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), Sense::click());
        r.widget_info(|| match self.on {
            Some(on) => WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), on, self.icon.name()),
            None => WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.icon.name()),
        });
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let radius = CornerRadius::same(RADIUS_CONTROL);
        if self.on == Some(true) && self.style == IconStyle::Boxed {
            p.rect_filled(rect, radius, t.fill);
        } else if look.hovered && !look.disabled {
            p.rect_filled(rect, radius, t.hover);
        }
        self.icon.paint(&p, rect.center(), if look.disabled { t.text_faint } else { t.text });
        match (self.style, look.focused) {
            (IconStyle::Boxed, true) => focus::own_edge(&p, rect, RADIUS_CONTROL),
            (IconStyle::Boxed, false) => {
                p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
            }
            (IconStyle::Plain, true) => focus::box_around(&p, rect),
            (IconStyle::Plain, false) => {}
        }
        r
    }
}

/// A cell of a bar (`button/bar`), 36 tall: an icon with words, or an icon alone in a 36 square.
/// Off when added with `ui.add_enabled(false, …)`.
pub struct BarButton<'a> {
    icon: Icon,
    label: Option<&'a str>,
    on: bool,
    done: bool,
    divider: bool,
    height: f32,
    min_width: f32,
    look: Option<Look>,
}

impl<'a> BarButton<'a> {
    /// `icon` and `label`, as REFRESH and + ROW in the table bar.
    pub fn label(icon: Icon, label: &'a str) -> BarButton<'a> {
        BarButton { icon, label: Some(label), on: false, done: false, divider: true, height: BAR_BUTTON_H, min_width: 0.0, look: None }
    }

    /// `icon` alone, as the value panel's toggle.
    pub fn icon(icon: Icon) -> BarButton<'a> {
        BarButton { icon, label: None, on: false, done: false, divider: true, height: BAR_BUTTON_H, min_width: 0.0, look: None }
    }

    /// In a bar with a line along its bottom: as tall as the room above the line.
    pub fn over_line(mut self) -> BarButton<'a> {
        self.height = BAR_CELL_H;
        self
    }

    /// On: what it shows is showing (`fill` over the cell, whether hovered or not).
    pub fn on(mut self, on: bool) -> BarButton<'a> {
        self.on = on;
        self
    }

    /// Done: what it did has just happened, and it says so in `text_muted` (the value panel's
    /// ✓ COPIED).
    pub fn done(mut self, done: bool) -> BarButton<'a> {
        self.done = done;
        self
    }

    /// At least `width` wide, what it holds centred: COPY keeps COPIED's width, so nothing moves.
    pub fn min_width(mut self, width: f32) -> BarButton<'a> {
        self.min_width = width;
        self
    }

    /// A 1 px `line` at its left end, between it and what is before it in the bar (the default).
    pub fn divider(mut self, divider: bool) -> BarButton<'a> {
        self.divider = divider;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> BarButton<'a> {
        self.look = Some(look);
        self
    }

    /// How wide it draws in `ui`.
    pub fn width(&self, ui: &Ui) -> f32 {
        self.parts(ui).1
    }

    /// The words, if any, and the width.
    fn parts(&self, ui: &Ui) -> (Option<Arc<Galley>>, f32) {
        let Some(label) = self.label else {
            return (None, BAR_BUTTON_H);
        };
        let words = ui.painter().layout_job(Text::Label.job(label, Color32::PLACEHOLDER));
        // egui puts the letter spacing after the last letter too; the page's measure has none there.
        let words_w = words.size().x - Text::Label.spacing();
        let width = (2.0 * BAR_BUTTON_PAD_X + self.icon.size().x + BAR_BUTTON_GAP + words_w).ceil();
        (Some(words), width.max(self.min_width))
    }
}

impl Widget for BarButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (words, width) = self.parts(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(width, self.height), Sense::click());
        let name = self.label.unwrap_or(self.icon.name());
        r.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), self.on, name));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let ground = if self.on {
            Some(t.fill)
        } else if look.hovered && !look.disabled {
            Some(t.hover)
        } else {
            None
        };
        if let Some(ground) = ground {
            p.rect_filled(rect, CornerRadius::ZERO, ground);
        }
        let colour = if look.disabled {
            t.text_faint
        } else if self.done {
            t.text_muted
        } else {
            t.text
        };
        match words {
            Some(words) => {
                let icon_w = self.icon.size().x;
                let held = icon_w + BAR_BUTTON_GAP + words.size().x - Text::Label.spacing();
                let natural = (2.0 * BAR_BUTTON_PAD_X + held).ceil();
                let x = if rect.width() > natural { rect.center().x - held / 2.0 } else { rect.left() + BAR_BUTTON_PAD_X };
                self.icon.paint(&p, pos2(x + icon_w / 2.0, rect.center().y), colour);
                p.galley(pos2(x + icon_w + BAR_BUTTON_GAP, rect.center().y - words.size().y / 2.0), words, colour);
            }
            None => self.icon.paint(&p, rect.center(), colour),
        }
        if self.divider {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(BAR_BUTTON_DIVIDER, rect.height())), CornerRadius::ZERO, t.line);
        }
        if look.focused {
            focus::box_inside(&p, rect);
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
    use crate::components::tooltip::Hint;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, harness, icon_in, painted, texts};
    use crate::theme::DARK;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
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
                        ui.add_enabled(false, BoxButton::primary("Run")).hint_disabled("Run: not now, nothing to run"),
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

    #[test]
    fn each_icon_button_state_matches_the_page() {
        // Node 161038: 28 square, corners 2, the icon centred in `text`. Boxed: a 1 px line_strong
        // edge and no ground; plain: no edge, so its focus is a box around it.
        let hovered = Look { hovered: true, ..Look::default() };
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for style in [IconStyle::Boxed, IconStyle::Plain] {
                for state in ["default", "hover", "focus", "disabled"] {
                    let at_ = format!("{theme:?} {style:?} {state}");
                    let mut at = Rect::NOTHING;
                    let shapes = painted(theme, |ui| {
                        ui.add_space(ROOM);
                        ui.horizontal(|ui| {
                            ui.add_space(ROOM);
                            let b = IconButton { icon: Icon::Plus, style, on: None, look: None };
                            at = match state {
                                "disabled" => ui.add_enabled(false, b).rect,
                                "hover" => ui.add(b.look(hovered)).rect,
                                "focus" => ui.add(b.look(focused)).rect,
                                _ => ui.add(b).rect,
                            };
                        });
                    });
                    assert_eq!(at.size(), egui::vec2(28.0, 28.0), "{at_}: 28 square");
                    let radius = CornerRadius::same(2);
                    let want_fills: Vec<(Rect, Color32)> = if state == "hover" { vec![(at, t.hover)] } else { vec![] };
                    let want_edges = match (style, state) {
                        (IconStyle::Boxed, "focus") => vec![Edge { outer: at, width: 2.0, colour: t.ink, radius }],
                        (IconStyle::Boxed, _) => vec![Edge { outer: at, width: 1.0, colour: t.line_strong, radius }],
                        (IconStyle::Plain, "focus") => {
                            vec![Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) }]
                        }
                        (IconStyle::Plain, _) => vec![],
                    };
                    assert_eq!(fills_in(&shapes, at), want_fills, "{at_}: the ground");
                    assert_eq!(edges_in(&shapes, at, 4.0), want_edges, "{at_}: the edge");
                    let (colours, bounds) = icon_in(&shapes, at);
                    let colour = if state == "disabled" { t.text_faint } else { t.text };
                    assert_eq!(colours, [colour], "{at_}: the icon's colour");
                    assert!((bounds.center() - at.center()).length() <= 1.0, "{at_}: the icon {bounds:?} centred in {at:?}");
                }
            }
            for (style, add) in [
                (IconStyle::Boxed, (|ui: &mut Ui| ui.add(IconButton::boxed(Icon::Minus))) as fn(&mut Ui) -> Response),
                (IconStyle::Plain, |ui: &mut Ui| ui.add(IconButton::plain(Icon::Next))),
            ] {
                let (shapes, at) = tabbed(theme, add);
                let want = match style {
                    IconStyle::Boxed => Edge { outer: at, width: 2.0, colour: t.ink, radius: CornerRadius::same(2) },
                    IconStyle::Plain => Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) },
                };
                assert_eq!(edges_in(&shapes, at, 4.0), [want], "{theme:?} {style:?}: Tab shows its focus");
            }
        }
    }

    #[test]
    fn each_bar_button_state_matches_the_page() {
        // Node 161067: 36 tall; the label kind 12, its icon, 8, its words in `label` caps, 12; the
        // icon kind 36 square, its icon centred. Hover `hover`, on `fill` over the whole cell (and
        // hovering it changes nothing), the divider a 1 px `line` at its left end over the ground,
        // focus a 2 px ink box 2 px inside.
        let hovered = Look { hovered: true, ..Look::default() };
        let focused = Look { focused: true, ..Look::default() };
        let icon = Icon::Refresh.size();
        for (theme, t) in both() {
            for words in [Some("Refresh"), None] {
                for divider in [false, true] {
                    for state in ["default", "hover", "on", "on hovered", "disabled", "focus"] {
                        let at_ = format!("{theme:?} {words:?} divider {divider} {state}");
                        let mut at = Rect::NOTHING;
                        let mut width = 0.0;
                        let shapes = painted(theme, |ui| {
                            ui.add_space(ROOM);
                            let b = match words {
                                Some(w) => BarButton::label(Icon::Refresh, w),
                                None => BarButton::icon(Icon::Refresh),
                            };
                            let b = b.divider(divider).on(state.starts_with("on"));
                            width = b.width(ui);
                            at = match state {
                                "disabled" => ui.add_enabled(false, b).rect,
                                "hover" | "on hovered" => ui.add(b.look(hovered)).rect,
                                "focus" => ui.add(b.look(focused)).rect,
                                _ => ui.add(b).rect,
                            };
                        });
                        assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                        assert_eq!(at.width(), width, "{at_}: as wide as it measures");
                        let ground = match state {
                            "hover" => Some(t.hover),
                            "on" | "on hovered" => Some(t.fill),
                            _ => None,
                        };
                        let mut want_fills: Vec<(Rect, Color32)> = ground.map(|g| (at, g)).into_iter().collect();
                        if divider {
                            want_fills.push((Rect::from_min_size(at.min, egui::vec2(1.0, 36.0)), t.line));
                        }
                        assert_eq!(fills_in(&shapes, at), want_fills, "{at_}: the ground and the divider");
                        let box_inside = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                        let want_edges = if state == "focus" { vec![box_inside] } else { vec![] };
                        assert_eq!(edges_in(&shapes, at, 0.0), want_edges, "{at_}: the edge");
                        let colour = if state == "disabled" { t.text_faint } else { t.text };
                        let (colours, bounds) = icon_in(&shapes, at);
                        assert_eq!(colours, [colour], "{at_}: the icon's colour");
                        let drawn = texts(&shapes);
                        match words {
                            Some(_) => {
                                let (_, w, c) = drawn.iter().find(|(s, _, _)| s == "REFRESH").unwrap_or_else(|| panic!("{at_}: {drawn:?}"));
                                assert_eq!(*c, colour, "{at_}: the words' colour");
                                assert!(close(w.left(), at.left() + 12.0 + icon.x + 8.0), "{at_}: {w:?} after the icon in {at:?}");
                                assert!(close(w.center().y, at.center().y), "{at_}: the words centred");
                                // The page's measure: egui puts the letter spacing after the last
                                // letter too, which takes no room.
                                let page = (12.0 + icon.x + 8.0 + w.width() - Text::Label.spacing() + 12.0).ceil();
                                assert_eq!(at.width(), page, "{at_}: 12, icon, 8, words, 12");
                                let centre = egui::pos2(at.left() + 12.0 + icon.x / 2.0, at.center().y);
                                assert!((bounds.center() - centre).length() <= 1.0, "{at_}: the icon {bounds:?} 12 in");
                            }
                            None => {
                                assert_eq!(at.width(), 36.0, "{at_}: 36 square");
                                assert!(!drawn.iter().any(|(_, r, _)| at.intersects(*r)), "{at_}: no words: {drawn:?}");
                                assert!((bounds.center() - at.center()).length() <= 1.0, "{at_}: the icon {bounds:?} centred");
                            }
                        }
                    }
                }
            }
            let (shapes, at) = tabbed(theme, |ui| ui.add(BarButton::label(Icon::Plus, "Row")));
            let want = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, at, 0.0), [want], "{theme:?}: Tab shows the box inside");
        }
    }

    #[test]
    fn bar_buttons_are_as_wide_as_the_pages() {
        // The instances of the page's bar row (161306) and the icon kind, to a point: egui
        // measures these words a little narrower than the page's font does.
        let mut widths = Vec::new();
        painted(Theme::Light, |ui| {
            widths = [
                BarButton::label(Icon::Refresh, "Refresh"),
                BarButton::label(Icon::Plus, "Row"),
                BarButton::label(Icon::Import, "Import"),
                BarButton::label(Icon::Wrap, "Wrap"),
                BarButton::label(Icon::Copy, "Copy"),
                BarButton::icon(Icon::Panel),
            ]
            .iter()
            .map(|b| b.width(ui))
            .collect();
        });
        let page = [90.0, 63.0, 83.0, 69.0, 69.0, 36.0];
        assert_eq!(widths.len(), page.len());
        for (ours, page) in widths.iter().zip(page) {
            assert!((ours - page).abs() <= 1.0, "{widths:?} against the page's {page}");
        }
    }

    #[test]
    fn a_boxed_icon_button_that_is_on_matches_the_page() {
        // Node 174286 (`style=boxed, state=on`): a `fill` ground inside its 1 px `line_strong`
        // edge, the icon in `text` as in default; hovering changes nothing. Read out as on.
        use egui::accesskit::Toggled;
        use egui_kittest::kittest::NodeT;
        let hovered = Look { hovered: true, ..Look::default() };
        for (theme, t) in both() {
            for look in [None, Some(hovered)] {
                let at_ = format!("{theme:?} {look:?}");
                let mut at = Rect::NOTHING;
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    ui.horizontal(|ui| {
                        ui.add_space(ROOM);
                        let b = IconButton::boxed(Icon::Code).on(true);
                        at = ui.add(if let Some(l) = look { b.look(l) } else { b }).rect;
                    });
                });
                assert_eq!(at.size(), egui::vec2(28.0, 28.0), "{at_}: 28 square");
                assert_eq!(fills_in(&shapes, at), [(at, t.fill)], "{at_}: the fill ground, hovered or not");
                let edge = Edge { outer: at, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
                assert_eq!(edges_in(&shapes, at, 4.0), [edge], "{at_}: the edge as in default");
                assert_eq!(icon_in(&shapes, at).0, [t.text], "{at_}: the icon as in default");
            }
        }
        let mut h = harness(Theme::Light, (), |ui, _| {
            ui.horizontal(|ui| {
                ui.add(IconButton::boxed(Icon::Code).on(true));
                ui.add(IconButton::boxed(Icon::Plus));
            });
        });
        h.run();
        assert_eq!(h.get_by_label("code").accesskit_node().toggled(), Some(Toggled::True), "on, it says so");
        assert_eq!(h.get_by_label("plus").accesskit_node().toggled(), None, "an icon button that is never on says nothing");
    }

    #[test]
    fn a_bar_button_that_is_on_says_so() {
        use egui::accesskit::{Role, Toggled};
        use egui_kittest::kittest::NodeT;
        let mut h = harness(Theme::Light, false, |ui, on: &mut bool| {
            ui.horizontal(|ui| {
                if ui.add(BarButton::icon(Icon::Panel).on(*on)).clicked() {
                    *on = !*on;
                }
                ui.add(BarButton::label(Icon::Wrap, "Wrap").on(true));
                ui.add(IconButton::plain(Icon::First));
            });
        });
        h.run();
        let toggled = |h: &Harness<'_, bool>, label: &str| h.get_by_label(label).accesskit_node().toggled();
        assert_eq!(toggled(&h, "panel"), Some(Toggled::False), "off, it says so");
        assert_eq!(toggled(&h, "Wrap"), Some(Toggled::True), "on, it says so");
        assert!(h.query_by_role_and_label(Role::Button, "Wrap").is_some(), "a button, its words as written");
        assert!(h.query_by_label("WRAP").is_none(), "the capitals are only drawn");
        assert!(h.query_by_role_and_label(Role::Button, "first").is_some(), "an icon button is named by its icon");
        h.get_by_label("panel").click();
        h.run();
        assert!(*h.state());
        assert_eq!(toggled(&h, "panel"), Some(Toggled::True), "clicked on, it says so");
    }
}

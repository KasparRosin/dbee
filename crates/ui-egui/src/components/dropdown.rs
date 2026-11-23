//! The dropdown (`dropdown`): the closed control, its chosen text as written in `body` and a
//! chevron, opening a list under it: the menu's frame ([`menu::style`]) with the keyboard of
//! egui's combo box; its rows are the caller's, the menu's items.

use egui::text::TextWrapping;
use egui::{
    Color32, CornerRadius, Id, InnerResponse, Popup, PopupCloseBehavior, ScrollArea, Sense, Stroke, StrokeKind, TextWrapMode, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};

use super::icon::Icon;
use super::tooltip::Hint;
use super::{Look, focus, menu, painter};
use crate::theme::size::{DROPDOWN_GAP, DROPDOWN_H, DROPDOWN_PAD_X, RADIUS_CONTROL};
use crate::theme::{Text, tokens};

/// A dropdown, 28 tall, showing `selected_text`; off when added inside `ui.add_enabled_ui(false,
/// …)`.
pub struct Dropdown<'a> {
    id_salt: Id,
    selected: &'a str,
    width: f32,
    list_h: Option<f32>,
    muted: bool,
    note: Option<&'a str>,
    look: Option<Look>,
}

impl<'a> Dropdown<'a> {
    /// `selected_text` is drawn as written, cut short with `…` when it doesn't fit.
    pub fn new(id_salt: impl std::hash::Hash + std::fmt::Debug, selected_text: &'a str) -> Dropdown<'a> {
        Dropdown { id_salt: Id::new(id_salt), selected: selected_text, width: f32::INFINITY, list_h: None, muted: false, note: None, look: None }
    }

    /// This wide, or as wide as there is room for (the default fills the row).
    pub fn width(mut self, width: f32) -> Dropdown<'a> {
        self.width = width;
        self
    }

    /// The open list scrolls past this height (egui's combo box height by default).
    pub fn list_height(mut self, height: f32) -> Dropdown<'a> {
        self.list_h = Some(height);
        self
    }

    /// Its text in `text_muted`: what it chose counts for nothing now (a filter row left out of
    /// the condition). Not on the page yet.
    pub fn muted(mut self, muted: bool) -> Dropdown<'a> {
        self.muted = muted;
        self
    }

    /// Said on hover, after the chosen text when that is cut short (an operator's note).
    pub fn note(mut self, note: Option<&'a str>) -> Dropdown<'a> {
        self.note = note;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Dropdown<'a> {
        self.look = Some(look);
        self
    }

    /// Draws it; while it is open, `list` draws the list under it, at least as wide as its widest
    /// row, so every row reads whole. Its chosen text, when cut short, reads whole on hover. A
    /// click or Space opens and closes it, as egui's combo box does. A click on a row closes it; a row chosen with the
    /// keyboard closes it when `list` calls `ui.close()`, as in a menu.
    pub fn show<R>(self, ui: &mut Ui, list: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
        let id = ui.make_persistent_id(self.id_salt);
        let width = self.width.min(ui.available_width()).max(0.0);
        let (_, rect) = ui.allocate_space(vec2(width, DROPDOWN_H));
        let r = ui.interact(rect, id, Sense::click());
        r.widget_info(|| {
            let mut info = WidgetInfo::new(WidgetType::ComboBox);
            info.enabled = ui.is_enabled();
            info.current_text_value = Some(self.selected.to_owned());
            info
        });
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        let mut cut = false;
        if ui.is_rect_visible(rect) {
            let t = tokens(ui.ctx());
            let p = painter(ui);
            let radius = CornerRadius::same(RADIUS_CONTROL);
            p.rect_filled(rect, radius, t.card);
            if look.focused {
                focus::own_edge(&p, rect, RADIUS_CONTROL);
            } else {
                p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
            }
            // The words and the chevron keep this far in whatever the edge does.
            let inset = DROPDOWN_PAD_X + 1.0;
            let chevron = Icon::ChevronDown.size().x;
            let mut job = Text::Body.job_as_written(self.selected, Color32::PLACEHOLDER);
            job.wrap = TextWrapping::truncate_at_width((width - 2.0 * inset - DROPDOWN_GAP - chevron).max(0.0));
            let words = p.layout_job(job);
            cut = words.elided;
            let (text, mark) = match (look.disabled, self.muted) {
                (true, _) => (t.text_faint, t.text_faint),
                (false, true) => (t.text_muted, t.text_muted),
                (false, false) => (t.text, t.text_muted),
            };
            p.galley(pos2(rect.left() + inset, rect.center().y - words.size().y / 2.0), words, text);
            Icon::ChevronDown.paint(&p, pos2(rect.right() - inset - chevron / 2.0, rect.center().y), mark);
        }
        let hover = match (cut, self.note) {
            (true, Some(note)) => Some(format!("{}\n{note}", self.selected)),
            (true, None) => Some(self.selected.to_owned()),
            (false, note) => note.map(str::to_owned),
        };
        let r = match hover {
            Some(words) => r.hint(words),
            None => r,
        };
        // As egui's combo box opens its list: the same popup, its id, width and closing.
        let height = self.list_h.unwrap_or_else(|| ui.spacing().combo_height);
        let inner = Popup::menu(&r)
            .id(id.with("popup"))
            .width(rect.width())
            .close_behavior(PopupCloseBehavior::CloseOnClick)
            .style(menu::style(ui.ctx()))
            .show(|ui| {
                ui.set_min_width(ui.available_width());
                ScrollArea::vertical()
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                        list(ui)
                    })
                    .inner
            })
            .map(|shown| shown.inner);
        InnerResponse::new(inner, r)
    }
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui::{CornerRadius, Key, Rect};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::menu::Item;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness, icon_in, painted, texts};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    #[test]
    fn each_dropdown_state_matches_the_page() {
        // 160 by 28, `card` in a 1 px `line_strong` edge, corners 2; the chosen text
        // in `body`, `text`, 9 in and centred; the chevron in `text_muted`, its right end 9 from
        // the box's. Focus: the edge turns 2 px `ink`. Disabled: text and chevron `text_faint`.
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for state in ["default", "focus", "disabled"] {
                let at_ = format!("{theme:?} {state}");
                let mut at = Rect::NOTHING;
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    ui.horizontal(|ui| {
                        ui.add_space(ROOM);
                        let d = Dropdown::new("op", "equals (=)").width(160.0);
                        at = match state {
                            "disabled" => ui.add_enabled_ui(false, |ui| d.show(ui, |_| ()).response).inner.rect,
                            "focus" => d.look(focused).show(ui, |_| ()).response.rect,
                            _ => d.show(ui, |_| ()).response.rect,
                        };
                    });
                });
                assert_eq!(at.size(), egui::vec2(160.0, 28.0), "{at_}: 160 by 28");
                assert_eq!(fills_in(&shapes, at), [(at, t.card)], "{at_}: on card");
                let radius = CornerRadius::same(2);
                let edge = match state {
                    "focus" => Edge { outer: at, width: 2.0, colour: t.ink, radius },
                    _ => Edge { outer: at, width: 1.0, colour: t.line_strong, radius },
                };
                assert_eq!(edges_in(&shapes, at, 0.0), [edge], "{at_}: the edge");
                let off = state == "disabled";
                let drawn = texts(&shapes);
                let (_, words, colour) = drawn.iter().find(|(s, _, _)| s == "equals (=)").unwrap_or_else(|| panic!("{at_}: {drawn:?}"));
                assert_eq!(*colour, if off { t.text_faint } else { t.text }, "{at_}: the text's colour");
                assert!(close(words.left(), at.left() + 9.0), "{at_}: {words:?} 9 into {at:?}");
                assert!(close(words.height(), 18.0) && close(words.center().y, at.center().y), "{at_}: in body, centred");
                let (colours, bounds) = icon_in(&shapes, at);
                assert_eq!(colours, [if off { t.text_faint } else { t.text_muted }], "{at_}: the chevron's colour");
                let chevron = egui::pos2(at.right() - 9.0 - 4.0, at.center().y);
                assert!((bounds.center() - chevron).length() <= 1.0, "{at_}: the chevron {bounds:?} 9 from the right end of {at:?}");
            }
            // Tab reaches it, and its focus shows.
            let mut h = harness(theme, Rect::NOTHING, |ui, at| {
                ui.add_space(ROOM);
                *at = Dropdown::new("op", "equals (=)").width(160.0).show(ui, |_| ()).response.rect;
            });
            h.run();
            h.key_press(Key::Tab);
            h.run();
            let at = *h.state();
            let edge = Edge { outer: at, width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&h.output().shapes, at, 0.0), [edge], "{theme:?}: Tab shows its focus");
        }
    }

    #[test]
    fn an_open_list_is_as_wide_as_its_widest_item() {
        // The operator choice in a narrow row: closed, its words cut with `…` and whole on hover;
        // open, the list at least as wide as its longest item, so every item reads whole.
        const LONG: &str = "greater than or equal (>=)";
        for (theme, _) in both() {
            let mut h = harness(theme, Rect::NOTHING, |ui, at| {
                ui.add_space(ROOM);
                ui.horizontal(|ui| {
                    ui.add_space(ROOM);
                    *at = Dropdown::new("op", LONG)
                        .width(120.0)
                        .show(ui, |ui| {
                            for words in ["equals (=)", LONG, "in"] {
                                ui.add(Item::new(words).choice(Some(words == LONG)));
                            }
                        })
                        .response
                        .rect;
                });
            });
            h.run();
            let at = *h.state();
            let cut = flat(&h.output().shapes).into_iter().any(|s| matches!(s, egui::Shape::Text(t) if t.galley.elided && at.contains(t.pos)));
            assert!(cut, "{theme:?}: closed, the words are cut");
            // Hovered for longer than a tooltip waits: the words whole.
            h.hover_at(at.center());
            for _ in 0..4 {
                h.step();
            }
            h.run();
            let hover = texts(&h.output().shapes);
            assert!(hover.iter().any(|(w, r, _)| w == LONG && !at.contains_rect(*r)), "{theme:?}: whole on hover: {hover:?}");
            h.get_by_role(Role::ComboBox).click();
            h.run();
            h.run();
            let item = h.get_by_label(LONG);
            let item_w = item.rect().width();
            let open = texts(&h.output().shapes);
            let (_, words, _) = open.iter().find(|(w, _, _)| w == LONG).unwrap_or_else(|| panic!("{theme:?}: the item reads whole: {open:?}"));
            assert!(item_w >= words.width(), "{theme:?}: the item {item_w} holds its words {words:?}");
            assert!(item_w > at.width(), "{theme:?}: the list {item_w} wider than the closed field {at:?}");
        }
    }

    #[test]
    fn a_dropdown_shows_a_column_name_as_written() {
        // Review Focus 3: a column name with capitals and quotes, and a type, keep their case,
        // closed and open.
        const NAMES: [&str; 2] = ["\"Mood\"", "enum('small','Large')"];
        for (theme, _) in both() {
            let mut h = harness(theme, (NAMES[0].to_owned(), Rect::NOTHING), |ui, (chosen, at)| {
                ui.add_space(ROOM);
                let shown = Dropdown::new("column", chosen.as_str()).width(200.0).show(ui, |ui| {
                    let mut picked = None;
                    for name in NAMES {
                        if ui.add(Item::new(name).choice(Some(name == chosen.as_str()))).clicked() {
                            picked = Some(name);
                        }
                    }
                    picked
                });
                *at = shown.response.rect;
                if let Some(name) = shown.inner.flatten() {
                    *chosen = name.to_owned();
                }
            });
            h.run();
            let drawn =
                |h: &egui_kittest::Harness<'_, (String, Rect)>| -> Vec<String> { texts(&h.output().shapes).into_iter().map(|(s, _, _)| s).collect() };
            assert_eq!(drawn(&h), [NAMES[0]], "{theme:?}: closed, the name as written");
            let node = h.get_by_role(Role::ComboBox);
            assert_eq!(node.value().as_deref(), Some(NAMES[0]), "{theme:?}: its value as written");
            node.click();
            h.run();
            h.run();
            let open = drawn(&h);
            for name in NAMES {
                assert!(open.iter().filter(|s| *s == name).count() >= 1, "{theme:?}: open, `{name}` as written: {open:?}");
            }
            assert!(!open.iter().any(|s| s.contains("MOOD") || s.contains("LARGE")), "{theme:?}: nothing in capitals: {open:?}");
            h.get_by_label(NAMES[1]).click();
            h.run();
            h.run();
            assert_eq!(h.state().0, NAMES[1], "{theme:?}: the click chose it");
            assert_eq!(drawn(&h), [NAMES[1]], "{theme:?}: closed again, the type as written");
            let at = h.state().1;
            let words = texts(&h.output().shapes).into_iter().next().map_or(Rect::NOTHING, |(_, r, _)| r);
            assert!(at.contains_rect(words), "{theme:?}: {words:?} in {at:?}");
        }
    }
}

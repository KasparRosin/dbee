//! Menus: the frame every menu and popup list floats in (`menu`, [`style`]), its items
//! (`menu/item`, [`Item`]), the line between groups (`menu/separator`, [`separator`]), an item
//! that opens a submenu ([`submenu`]), the menu bar (`menu bar`, [`bar`]) and its titles
//! (`menubar/title`, [`bar_title`]).
//!
//! An item is a plain egui widget: it takes keyboard focus as a button does (unless a list moves
//! its own highlight, [`Item::focusable`]), so Tab and the arrows reach it and Enter or Space
//! chooses it, and the menu closes when the caller says so (`ui.close()`), as with egui's
//! buttons. egui sizes a menu in a first, unseen pass in which nothing is stretched; an item asks
//! for its own width then, and fills the menu's width after, so a menu is as wide as its longest
//! item.

use std::sync::Arc;

use egui::containers::menu::{MenuConfig, MenuState, SubMenu};
use egui::style::StyleModifier;
use egui::{
    Align, Color32, Context, CornerRadius, Frame, Galley, InnerResponse, Layout, Margin, Popup, Rect, Response, Sense, Stroke, Ui, UiBuilder, UiKind,
    UiStackInfo, Vec2, Widget, WidgetInfo, WidgetType, pos2, vec2,
};

use super::icon::Icon;
use super::keycap::Keycap;
use super::{Look, focus, painter};
use crate::theme::size::{
    KEYCAP_GAP, MENU_BAR_PAD_X, MENU_HEADING_GAP, MENU_ITEM_GAP, MENU_ITEM_H, MENU_ITEM_PAD_X, MENU_KEYS_PAD, MENU_MARK, MENU_PAD,
    MENU_SEPARATOR_PAD, MENU_TITLE_H, MENU_TITLE_PAD_X, RADIUS_CONTROL, RADIUS_FLOAT, SLIM_BAR_H,
};
use crate::theme::{self, Text, tokens};

/// A title's menu opens at the menu bar's bottom edge: the page's bar holds the titles centred.
const UNDER_THE_BAR: f32 = (SLIM_BAR_H - MENU_TITLE_H) / 2.0;
/// The menu bar's `line` along its bottom.
const BAR_LINE: f32 = 1.0;

/// The style of every menu and popup list in the mode `ctx` draws in: egui's menu style, the
/// page's frame (`raised`, a 1 px `line` edge, corners 4, the one shadow, 4 inside the edge) and
/// the items one under the other. Tooltips keep the theme's own frame, with its `raised_line`
/// edge.
pub fn style(ctx: &Context) -> StyleModifier {
    let t = tokens(ctx);
    StyleModifier::new(move |style| {
        egui::containers::menu::menu_style(style);
        let v = &mut style.visuals;
        v.window_fill = t.raised;
        v.window_stroke = Stroke::new(1.0, t.line);
        v.menu_corner_radius = CornerRadius::same(RADIUS_FLOAT);
        v.popup_shadow = theme::float_shadow(t);
        style.spacing.menu_margin = Margin::same(MENU_PAD);
        style.spacing.item_spacing.y = 0.0;
    })
}

/// A menu drawn in place, `width` wide inside its frame, as the showcase draws one that isn't
/// open.
pub fn in_place<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    ui.scope(|ui| {
        style(ui.ctx()).apply(ui.style_mut());
        Frame::menu(ui.style())
            .show(ui, |ui| {
                ui.set_width(width);
                ui.with_layout(Layout::top_down_justified(Align::Min), add).inner
            })
            .inner
    })
}

/// One item of a menu or popup list, 28 tall: its words, after the chosen mark's column in a menu
/// with choices, then its keys or a submenu's arrow at the right end. Off when added with
/// `ui.add_enabled(false, …)`.
pub struct Item<'a> {
    label: &'a str,
    shortcut: Option<&'a str>,
    destructive: bool,
    choice: Option<bool>,
    submenu: bool,
    highlighted: bool,
    focusable: bool,
    look: Option<Look>,
}

impl<'a> Item<'a> {
    /// `label` is drawn as written.
    pub fn new(label: &'a str) -> Item<'a> {
        Item { label, shortcut: None, destructive: false, choice: None, submenu: false, highlighted: false, focusable: true, look: None }
    }

    /// `keys` as `shortcuts.rs` writes them (`Ctrl+Shift+Z`), drawn as one keycap per key.
    pub fn shortcut(mut self, keys: &'a str) -> Item<'a> {
        self.shortcut = Some(keys);
        self
    }

    /// Its words in `danger` (Delete row, Delete).
    pub fn destructive(mut self, destructive: bool) -> Item<'a> {
        self.destructive = destructive;
        self
    }

    /// `None` in a menu without choices; in a menu with choices, the column for the chosen mark,
    /// with the mark when `Some(true)`.
    pub fn choice(mut self, choice: Option<bool>) -> Item<'a> {
        self.choice = choice;
        self
    }

    /// The arrow of an item opening a submenu ([`submenu`] draws it).
    pub fn submenu(mut self, submenu: bool) -> Item<'a> {
        self.submenu = submenu;
        self
    }

    /// The keyboard's highlight, for a list that moves its own (it looks like hover).
    pub fn highlighted(mut self, highlighted: bool) -> Item<'a> {
        self.highlighted = highlighted;
        self
    }

    /// `false` for a list whose items take no keyboard focus because the list moves its own
    /// highlight; `true` (the default) as a button.
    pub fn focusable(mut self, focusable: bool) -> Item<'a> {
        self.focusable = focusable;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Item<'a> {
        self.look = Some(look);
        self
    }
}

impl Widget for Item<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let words = ui.painter().layout_job(Text::Button.job_as_written(self.label, Color32::PLACEHOLDER));
        let keys: Vec<(Keycap<'_>, f32)> = self
            .shortcut
            .map(|k| {
                k.split('+')
                    .map(|key| {
                        let cap = Keycap::new(key);
                        let w = cap.width(ui.painter());
                        (cap, w)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let keys_w = keys.iter().map(|(_, w)| w + KEYCAP_GAP).sum::<f32>() - if keys.is_empty() { 0.0 } else { KEYCAP_GAP };
        let arrow = Icon::ChevronRight.size().x;
        let mut natural = 2.0 * MENU_ITEM_PAD_X + words.size().x;
        if self.choice.is_some() {
            natural += MENU_MARK + MENU_ITEM_GAP;
        }
        if !keys.is_empty() {
            natural += MENU_ITEM_GAP + MENU_KEYS_PAD + keys_w;
        }
        if self.submenu {
            natural += MENU_ITEM_GAP + arrow;
        }
        let natural = natural.ceil();
        let width = if ui.layout().horizontal_justify() { ui.available_width().max(natural) } else { natural };
        let sense = if self.focusable { Sense::click() } else { Sense::CLICK };
        let (rect, r) = ui.allocate_exact_size(vec2(width, MENU_ITEM_H), sense);
        r.widget_info(|| match self.choice {
            Some(chosen) => WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), chosen, self.label),
            None => WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label),
        });
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let off = look.disabled;
        if !off && (look.hovered || look.focused || self.highlighted) {
            p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), t.raised_hover);
        }
        let y = rect.center().y;
        let mut x = rect.left() + MENU_ITEM_PAD_X;
        if let Some(chosen) = self.choice {
            if chosen {
                Icon::Check.paint(&p, pos2(x + MENU_MARK / 2.0, y), if off { t.text_faint } else { t.text });
            }
            x += MENU_MARK + MENU_ITEM_GAP;
        }
        let colour = if off {
            t.text_faint
        } else if self.destructive {
            t.danger
        } else {
            t.text
        };
        p.galley(pos2(x, y - words.size().y / 2.0), words, colour);
        let mut right = rect.right() - MENU_ITEM_PAD_X;
        if self.submenu {
            Icon::ChevronRight.paint(&p, pos2(right - arrow / 2.0, y), if off { t.text_faint } else { t.text_muted });
            right -= arrow + MENU_ITEM_GAP;
        }
        let mut left = right - keys_w;
        for (cap, w) in &keys {
            cap.paint(&p, pos2(left, y), off);
            left += w + KEYCAP_GAP;
        }
        r
    }
}

/// The line between groups of items: 1 px `raised_line` across the menu, 4 above and below it.
pub fn separator(ui: &mut Ui) -> Response {
    let width = if ui.layout().horizontal_justify() { ui.available_width() } else { 0.0 };
    let (rect, r) = ui.allocate_exact_size(vec2(width, 2.0 * MENU_SEPARATOR_PAD + 1.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let t = tokens(ui.ctx());
        painter(ui).hline(rect.x_range(), rect.top() + MENU_SEPARATOR_PAD + 0.5, Stroke::new(1.0, t.raised_line));
    }
    r
}

/// A heading over a menu's items (the database chip's list): its words in `label` capitals,
/// `text_muted`, [`MENU_HEADING_GAP`] above the first item. Reported to accessibility as written.
pub fn heading(ui: &mut Ui, words: &str) -> Response {
    let t = tokens(ui.ctx());
    let galley = ui.painter().layout_job(Text::Label.job(words, t.text_muted));
    let (rect, r) = ui.allocate_exact_size(galley.size(), Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), words));
    painter(ui).galley(rect.min, galley, t.text_muted);
    ui.add_space(MENU_HEADING_GAP);
    r
}

/// An item of an open menu opening the submenu `content` beside it, as egui's submenu button
/// does (hovered, clicked, or Enter or Space on it); it keeps the highlight while its submenu is
/// open.
pub fn submenu<R>(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui) -> R) -> (Response, Option<InnerResponse<R>>) {
    let id = ui.next_auto_id();
    let open = MenuState::from_ui(ui, |state, _| state.open_item == Some(SubMenu::id_from_widget_id(id)));
    let r = ui.add(Item::new(label).submenu(true).highlighted(open));
    let inner = SubMenu::new().show(ui, &r, content);
    (r, inner)
}

/// A title's word, and the size it makes with its padding.
fn title_parts(ui: &Ui, label: &str) -> (Arc<Galley>, Vec2) {
    let words = ui.painter().layout_job(Text::Body.job_as_written(label, Color32::PLACEHOLDER));
    let size = vec2(words.size().x + 2.0 * MENU_TITLE_PAD_X, MENU_TITLE_H);
    (words, size)
}

/// Draws a title at `rect`: its word, on the `fill` box while its menu is open; keyboard focus
/// as a bar button's, 2 px inside it.
fn paint_title(ui: &Ui, rect: Rect, words: Arc<Galley>, open: bool, look: Look) {
    let t = tokens(ui.ctx());
    let p = painter(ui);
    if open {
        p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), t.fill);
    }
    p.galley(pos2(rect.left() + MENU_TITLE_PAD_X, rect.center().y - words.size().y / 2.0), words, t.text);
    if look.focused {
        focus::box_inside(&p, rect);
    }
}

/// A menu bar's title, drawn open or not whatever its menu does (the showcase only).
pub struct BarTitle<'a> {
    label: &'a str,
    open: bool,
    look: Option<Look>,
}

impl<'a> BarTitle<'a> {
    pub fn new(label: &'a str) -> BarTitle<'a> {
        BarTitle { label, open: false, look: None }
    }

    pub fn open(mut self, open: bool) -> BarTitle<'a> {
        self.open = open;
        self
    }

    pub fn look(mut self, look: Look) -> BarTitle<'a> {
        self.look = Some(look);
        self
    }
}

impl Widget for BarTitle<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (words, size) = title_parts(ui, self.label);
        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        paint_title(ui, rect, words, self.open, look);
        r
    }
}

/// The menu bar (`menu bar`): a band [`SLIM_BAR_H`] tall on `bg` with a 1 px `line` along its
/// bottom, as wide as `ui`. `add` puts its titles ([`bar_title`]) side by side in a row from 4 in,
/// centred in what is above the line.
pub fn bar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SLIM_BAR_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, t.bg);
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - BAR_LINE), rect.max), CornerRadius::ZERO, t.line);
    let top = rect.top() + (SLIM_BAR_H - BAR_LINE - MENU_TITLE_H) / 2.0;
    let row = Rect::from_min_max(pos2(rect.left() + MENU_BAR_PAD_X, top), pos2(rect.right() - MENU_BAR_PAD_X, top + MENU_TITLE_H));
    ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Min)), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        add(ui)
    })
    .inner
}

/// A menu bar's title, its word as written in `body`, opening the menu `content` under the bar
/// as egui's menu button does (a click, or Enter or Space on it; Escape or a click closes it).
pub fn bar_title<R>(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui) -> R) -> (Response, Option<InnerResponse<R>>) {
    let (words, size) = title_parts(ui, label);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    let look = Look::of(ui, &r);
    let config = MenuConfig::new().style(style(ui.ctx()));
    let inner = Popup::menu(&r)
        .gap(UNDER_THE_BAR)
        .close_behavior(config.close_behavior)
        .style(config.style.clone())
        .info(UiStackInfo::new(UiKind::Menu).with_tag_value(MenuConfig::MENU_CONFIG_TAG, config))
        .show(content);
    paint_title(ui, rect, words, inner.is_some(), look);
    (r, inner)
}

#[cfg(test)]
mod tests {
    use egui::{Align, Color32, CornerRadius, Key, Label, Layout, Popup, Rect, pos2};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::tooltip::Hint;
    use crate::components::{Edge, ROOM, both, edges_in, flat, harness, icon_in, painted, texts};

    /// The page's items are 230 wide.
    const W: f32 = 230.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `top` with its own opacity laid over an opaque `ground`.
    fn over(top: Color32, ground: Color32) -> Color32 {
        let [r, g, b, a] = top.to_srgba_unmultiplied();
        let a = f64::from(a) / 255.0;
        let mix = |t: u8, g: u8| (f64::from(t) * a + f64::from(g) * (1.0 - a)).round() as u8;
        Color32::from_rgb(mix(r, ground.r()), mix(g, ground.g()), mix(b, ground.b()))
    }

    /// `add` in a column `W` wide laid out as a menu lays out its items, `ROOM` in from the corner.
    fn column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        ui.add_space(ROOM);
        ui.horizontal(|ui| {
            ui.add_space(ROOM);
            ui.allocate_ui_with_layout(vec2(W, 300.0), Layout::top_down_justified(Align::Min), |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing.y = 0.0;
                add(ui)
            })
            .inner
        })
        .inner
    }

    /// The rect shape drawn exactly over `at`, if any: its fill and corners.
    fn ground_of(shapes: &[egui::epaint::ClippedShape], at: Rect) -> Vec<(Color32, CornerRadius)> {
        flat(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.rect == at && r.fill.a() > 0 => Some((r.fill, r.corner_radius)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_menu_item_state_matches_the_page() {
        // Node 162639: 28 tall; the words in `button`, 8 in (30 with the chosen mark's 14 px
        // column and 8), centred; the keys 3 apart, ending 8 from the right, 24 after the words
        // at least; the submenu's arrow in `text_muted`, ending 8 from the right. Hover:
        // `raised_hover`, corners 2. Words `text`, destructive `danger`, disabled `text_faint`.
        let hovered = Look { hovered: true, ..Look::default() };
        type Variant = (Option<&'static str>, Option<bool>, bool);
        let variants: [Variant; 6] = [
            (Some("Ctrl+Z"), None, false),
            (Some("Ctrl+Shift+Z"), None, false),
            (None, None, false),
            (None, Some(true), false),
            (None, Some(false), false),
            (None, None, true),
        ];
        for (theme, t) in both() {
            for destructive in [false, true] {
                for state in ["default", "hover", "disabled"] {
                    for (keys, choice, sub) in variants {
                        let label = if destructive { "Delete row" } else { "Undo" };
                        let at_ = format!("{theme:?} {label} {state} keys {keys:?} choice {choice:?} submenu {sub}");
                        let mut at = Rect::NOTHING;
                        let shapes = painted(theme, |ui| {
                            at = column(ui, |ui| {
                                let mut item = Item::new(label).destructive(destructive).choice(choice).submenu(sub);
                                if let Some(k) = keys {
                                    item = item.shortcut(k);
                                }
                                match state {
                                    "hover" => ui.add(item.look(hovered)).rect,
                                    "disabled" => ui.add_enabled(false, item).rect,
                                    _ => ui.add(item).rect,
                                }
                            });
                        });
                        assert_eq!(at.size(), vec2(W, 28.0), "{at_}: 28 tall, the menu's width");
                        let ground = if state == "hover" { vec![(t.raised_hover, CornerRadius::same(2))] } else { vec![] };
                        assert_eq!(ground_of(&shapes, at), ground, "{at_}: its ground");
                        let off = state == "disabled";
                        let drawn = texts(&shapes);
                        let find = |w: &str| drawn.iter().find(|(s, _, _)| s == w).map(|(_, r, c)| (*r, *c));
                        let (words, colour) = find(label).unwrap_or_else(|| panic!("{at_}: {label} drawn: {drawn:?}"));
                        let want = if off {
                            t.text_faint
                        } else if destructive {
                            t.danger
                        } else {
                            t.text
                        };
                        assert_eq!(colour, want, "{at_}: the words' colour");
                        let left = at.left() + if choice.is_some() { 30.0 } else { 8.0 };
                        assert!(close(words.left(), left), "{at_}: the words at {words:?}, {left} in");
                        assert!(close(words.height(), 18.0) && close(words.center().y, at.center().y), "{at_}: {words:?} in button, centred");
                        let caps: Vec<Edge> =
                            edges_in(&shapes, at, 0.0).into_iter().filter(|e| e.width == 1.0 && close(e.outer.height(), 18.0)).collect();
                        match keys {
                            Some(k) => {
                                let names: Vec<String> = k.split('+').map(str::to_uppercase).collect();
                                assert_eq!(caps.len(), names.len(), "{at_}: a keycap per key: {caps:?}");
                                for (cap, name) in caps.iter().zip(&names) {
                                    assert_eq!((cap.colour, cap.radius), (t.raised_line, CornerRadius::same(2)), "{at_}: {name}'s keycap");
                                    let (r, c) = find(name).unwrap_or_else(|| panic!("{at_}: {name} drawn: {drawn:?}"));
                                    assert!(cap.outer.contains_rect(r), "{at_}: {name} in its keycap");
                                    assert_eq!(c, if off { t.text_faint } else { t.text_muted }, "{at_}: {name}'s colour");
                                }
                                for pair in caps.windows(2) {
                                    assert!(close(pair[1].outer.left() - pair[0].outer.right(), 3.0), "{at_}: keys 3 apart: {caps:?}");
                                }
                                assert!(caps.last().is_some_and(|c| close(c.outer.right(), at.right() - 8.0)), "{at_}: the keys end 8 in");
                                assert!(caps[0].outer.left() >= words.right() + 24.0 - 0.5, "{at_}: 24 after the words");
                                assert!(find(k).is_none() && find("Ctrl").is_none(), "{at_}: no shortcut text, only keycaps");
                            }
                            None => assert!(caps.is_empty(), "{at_}: no keycaps"),
                        }
                        let mark = Rect::from_min_size(pos2(at.left() + 8.0, at.top() + 7.0), vec2(14.0, 14.0));
                        let (marks, bounds) = icon_in(&shapes, mark);
                        if choice == Some(true) {
                            assert_eq!(marks, [if off { t.text_faint } else { t.text }], "{at_}: the chosen mark");
                            assert!((bounds.center() - mark.center()).length() <= 0.6, "{at_}: the mark {bounds:?} centred in {mark:?}");
                        } else {
                            assert!(marks.is_empty(), "{at_}: no mark");
                        }
                        let slot = Rect::from_min_max(at.center_top(), at.right_bottom());
                        let (arrows, bounds) = icon_in(&shapes, slot);
                        if sub {
                            assert_eq!(arrows, [if off { t.text_faint } else { t.text_muted }], "{at_}: the submenu's arrow");
                            let centre = pos2(at.right() - 10.0, at.center().y);
                            assert!((bounds.center() - centre).length() <= 1.0, "{at_}: the arrow {bounds:?} ending 8 in {at:?}");
                        } else {
                            assert!(arrows.is_empty(), "{at_}: no arrow");
                        }
                    }
                }
            }
        }
    }

    /// What an item looks like: its ground and its words' colours.
    fn look_of(shapes: &[egui::epaint::ClippedShape], at: Rect) -> (Vec<(Color32, CornerRadius)>, Vec<(String, Color32)>) {
        let words = texts(shapes).into_iter().filter(|(_, r, _)| at.contains_rect(*r)).map(|(s, _, c)| (s, c)).collect();
        (ground_of(shapes, at), words)
    }

    #[test]
    fn the_keyboard_highlight_looks_like_hover() {
        // Node 162639's hover is also the keyboard's highlight, whether egui's focus moved there
        // (Tab, the arrows) or a list moves its own highlight (`highlighted`).
        struct S {
            at: [Rect; 2],
            chosen: Option<usize>,
            highlight: Option<usize>,
        }
        for (theme, t) in both() {
            let mut h = harness(theme, S { at: [Rect::NOTHING; 2], chosen: None, highlight: None }, |ui, s| {
                column(ui, |ui| {
                    for (n, (label, keys)) in [("Copy", "Ctrl+C"), ("Select All", "Ctrl+A")].into_iter().enumerate() {
                        let r = ui.add(Item::new(label).shortcut(keys).highlighted(s.highlight == Some(n)));
                        s.at[n] = r.rect;
                        if r.clicked() {
                            s.chosen = Some(n);
                        }
                    }
                });
            });
            h.run();
            let [first, second] = h.state().at;
            let plain = look_of(&h.output().shapes, first);
            assert!(plain.0.is_empty(), "{theme:?}: no ground at rest");
            h.hover_at(first.center());
            h.run();
            let hovered = look_of(&h.output().shapes, first);
            assert_eq!(hovered.0, [(t.raised_hover, CornerRadius::same(2))], "{theme:?}: hover");
            assert_eq!(hovered.1, plain.1, "{theme:?}: hover keeps the words' colours");
            h.remove_cursor();
            h.run();
            h.key_press(Key::Tab);
            h.run();
            assert_eq!(look_of(&h.output().shapes, first), hovered, "{theme:?}: Tab's highlight looks like hover");
            h.key_press(Key::ArrowDown);
            h.run();
            let moved = look_of(&h.output().shapes, second);
            assert_eq!(moved.0, hovered.0, "{theme:?}: Down moves the highlight");
            assert!(look_of(&h.output().shapes, first).0.is_empty(), "{theme:?}: and leaves the first");
            h.key_press(Key::Enter);
            h.run();
            assert_eq!(h.state().chosen, Some(1), "{theme:?}: Enter chooses the highlighted item");
            h.key_press(Key::Escape);
            h.state_mut().highlight = Some(0);
            h.run();
            h.run();
            assert_eq!(look_of(&h.output().shapes, first), hovered, "{theme:?}: a list's own highlight looks like hover");
        }
    }

    #[test]
    fn a_destructive_item_reads_on_hover() {
        // Review Focus 2: `danger` on `raised_hover` over the menu's `raised`, from the colours
        // painted, at 4.5:1 or more in both modes (dark is 4.53).
        let hovered = Look { hovered: true, ..Look::default() };
        for (theme, t) in both() {
            let mut at = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                ui.add_space(ROOM);
                ui.horizontal(|ui| {
                    ui.add_space(ROOM);
                    in_place(ui, W, |ui| at = ui.add(Item::new("Delete row").destructive(true).look(hovered)).rect);
                });
            });
            let menu = flat(&shapes)
                .into_iter()
                .rev()
                .find_map(|s| match s {
                    egui::Shape::Rect(r) if r.blur_width == 0.0 && r.rect != at && r.rect.contains_rect(at) && r.fill.a() == 255 => Some(r.fill),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{theme:?}: the menu's ground under the item"));
            assert_eq!(menu, t.raised, "{theme:?}: the menu is on raised");
            let item = ground_of(&shapes, at);
            assert_eq!(item.len(), 1, "{theme:?}: the item's ground: {item:?}");
            let drawn = texts(&shapes);
            let (_, _, words) = drawn.iter().find(|(s, _, _)| s == "Delete row").unwrap_or_else(|| panic!("{theme:?}: {drawn:?}"));
            assert_eq!(*words, t.danger, "{theme:?}: hover keeps danger");
            let seen = over(item[0].0, menu);
            let c = contrast(*words, seen);
            assert!(c >= 4.5, "{theme:?}: danger on raised_hover on raised: {c:.2}");
        }
    }

    #[test]
    fn a_heading_sits_6_over_the_first_item() {
        // The database chip's list: its heading in `label` capitals, `text_muted`, then the menu's
        // 6 to its first item. Read out as written.
        for (theme, t) in both() {
            let mut h = harness(theme, [Rect::NOTHING; 2], |ui, at: &mut [Rect; 2]| {
                at[0] = column(ui, |ui| {
                    let r = heading(ui, "Databases");
                    at[1] = ui.add(Item::new("many").choice(Some(false))).rect;
                    r.rect
                });
            });
            h.run();
            let [head, first] = *h.state();
            let drawn = texts(&h.output().shapes);
            let words = drawn.iter().find(|(s, _, _)| s == "DATABASES").unwrap_or_else(|| panic!("{theme:?}: the heading drawn: {drawn:?}"));
            assert_eq!(words.2, t.text_muted, "{theme:?}: the heading's colour");
            assert_eq!((words.1.min, words.1.height()), (head.min, 14.0), "{theme:?}: the heading's words {:?} in label's 14 in {head:?}", words.1);
            assert!(close(head.height(), 14.0) && close(head.left(), first.left()), "{theme:?}: label's 14, at the menu's left: {head:?}");
            assert!(!drawn.iter().any(|(s, _, _)| s == "Databases"), "{theme:?}: drawn only in capitals");
            assert_eq!(first.top() - head.bottom(), 6.0, "{theme:?}: 6 between the heading {head:?} and the first item {first:?}");
            assert!(h.query_by_label("Databases").is_some(), "{theme:?}: read out as written");
        }
    }

    #[test]
    fn a_menu_has_a_line_edge_and_a_tooltip_doesnt() {
        // Node 162690: `raised`, a 1 px `line` edge, corners 4, the one shadow, 4 inside the
        // edge. Tooltips keep the theme's `raised_line` edge.
        const TIP: &str = "New query tab (Ctrl+T)";
        for (theme, t) in both() {
            // The tooltip of an item in the menu (a disabled item's reason), so both show at once.
            let mut h = harness(theme, Rect::NOTHING, |ui, item| {
                ui.add_space(ROOM);
                let anchor = ui.add(Label::new("menu").sense(Sense::hover()));
                let shown = Popup::from_response(&anchor)
                    .open(true)
                    .style(style(ui.ctx()))
                    .show(|ui| ui.add(Item::new("New Query Tab").shortcut("Ctrl+T")).hint(TIP).rect);
                *item = shown.map_or(Rect::NOTHING, |s| s.inner);
            });
            h.run();
            h.hover_at(h.state().center());
            h.run();
            h.run();
            let item = *h.state();
            let shapes = flat(&h.output().shapes);
            let frame = |inside: Rect| {
                shapes.iter().rev().find_map(|s| match s {
                    egui::Shape::Rect(r) if r.blur_width == 0.0 && r.fill == t.raised && r.rect.contains_rect(inside) => Some(r.clone()),
                    _ => None,
                })
            };
            let menu = frame(item).unwrap_or_else(|| panic!("{theme:?}: the menu's frame around {item:?}"));
            assert_eq!((menu.stroke.width, menu.stroke.color), (1.0, t.line), "{theme:?}: the menu's edge");
            assert_eq!(menu.corner_radius, CornerRadius::same(4), "{theme:?}: the menu's corners");
            assert!(close(item.left() - menu.rect.left(), 5.0) && close(item.top() - menu.rect.top(), 5.0), "{theme:?}: 4 inside the edge");
            let shadow = shapes.iter().any(
                |s| matches!(s, egui::Shape::Rect(r) if r.blur_width == f32::from(theme::float_shadow(&t).blur) && r.fill == t.shadow && close(r.rect.top(), menu.rect.top() + 6.0)),
            );
            assert!(shadow, "{theme:?}: the menu's shadow");
            let words = texts(&h.output().shapes).into_iter().find(|(s, _, _)| s == TIP).map(|(_, r, _)| r);
            let words = words.unwrap_or_else(|| panic!("{theme:?}: the tooltip shows"));
            let tooltip = frame(words).unwrap_or_else(|| panic!("{theme:?}: the tooltip's frame"));
            assert_eq!((tooltip.stroke.width, tooltip.stroke.color), (1.0, t.raised_line), "{theme:?}: the tooltip's edge");
        }
    }

    #[test]
    fn the_float_shadow_reaches_as_far_as_the_design() {
        // The page's shadow (0 6, blur 18) fades out 24 under a menu's bottom edge, 12 over its
        // top and 18 beside it. Measured on the painted mesh: where its fade ends.
        for (theme, t) in both() {
            let mut h = harness(theme, Rect::NOTHING, |ui, items| {
                ui.add_space(ROOM);
                let anchor = ui.add(Label::new("menu").sense(Sense::hover()));
                let shown = Popup::from_response(&anchor)
                    .open(true)
                    .style(style(ui.ctx()))
                    .show(|ui| ["Undo", "Redo", "Cut"].map(|w| ui.add(Item::new(w)).rect).into_iter().fold(Rect::NOTHING, Rect::union));
                *items = shown.map_or(Rect::NOTHING, |s| s.inner);
            });
            h.run();
            let items = *h.state();
            let shapes = flat(&h.output().shapes);
            let menu = shapes.iter().find_map(|s| match s {
                egui::Shape::Rect(r) if r.blur_width == 0.0 && r.fill == t.raised && r.rect.contains_rect(items) => Some(r.rect),
                _ => None,
            });
            let menu = menu.unwrap_or_else(|| panic!("{theme:?}: the menu's frame around {items:?}"));
            let shadow = shapes.iter().find_map(|s| match s {
                egui::Shape::Rect(r) if r.blur_width > 0.0 && r.fill == t.shadow && r.rect.intersects(menu) => Some(r.clone()),
                _ => None,
            });
            let shadow = shadow.unwrap_or_else(|| panic!("{theme:?}: the menu's shadow"));
            let clipped = egui::epaint::ClippedShape { clip_rect: Rect::EVERYTHING, shape: egui::Shape::Rect(shadow) };
            let painted = h.ctx.tessellate(vec![clipped], h.ctx.pixels_per_point()).into_iter().fold(Rect::NOTHING, |r, p| match p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.iter().fold(r, |r, v| r.union(Rect::from_min_size(v.pos, Vec2::ZERO))),
                egui::epaint::Primitive::Callback(_) => r,
            });
            let (below, above, side) = (painted.bottom() - menu.bottom(), menu.top() - painted.top(), menu.left() - painted.left());
            let at = format!("{theme:?}: {below} below, {above} above, {side} beside {menu:?}");
            assert!(below >= 17.0, "{at}: at least 17 under the bottom edge");
            assert!((10.0..=22.0).contains(&above), "{at}: 10 to 22 over the top edge");
            assert!((10.0..=22.0).contains(&side), "{at}: 10 to 22 beside it");
        }
    }

    #[test]
    fn an_open_menu_title_takes_the_fill_box() {
        // Node 162732: the word in `body` as written, 8 in from either end, 24 tall; open, a
        // `fill` box with corners 2. Its menu opens under it.
        for (theme, t) in both() {
            let mut h = harness(theme, (Rect::NOTHING, Rect::NOTHING), |ui, (title, item)| {
                ui.add_space(ROOM);
                ui.horizontal(|ui| {
                    ui.add_space(ROOM);
                    let (r, shown) = bar_title(ui, "Edit", |ui| ui.add(Item::new("Undo").shortcut("Ctrl+Z")).rect);
                    *title = r.rect;
                    *item = shown.map_or(Rect::NOTHING, |s| s.inner);
                });
            });
            h.run();
            let title = h.state().0;
            let drawn = texts(&h.output().shapes);
            let (_, word, colour) = drawn.iter().find(|(s, _, _)| s == "Edit").unwrap_or_else(|| panic!("{theme:?}: {drawn:?}"));
            assert!(!drawn.iter().any(|(s, _, _)| s == "EDIT"), "{theme:?}: as written");
            assert_eq!(*colour, t.text, "{theme:?}: the word's colour");
            assert_eq!(title.height(), 24.0, "{theme:?}: 24 tall");
            assert!(
                close(word.left(), title.left() + 8.0) && close(title.width(), word.width() + 16.0),
                "{theme:?}: 8 each side: {word:?} {title:?}"
            );
            assert!(close(word.height(), 18.0) && close(word.center().y, title.center().y), "{theme:?}: in body, centred");
            assert!(ground_of(&h.output().shapes, title).is_empty(), "{theme:?}: closed, no box");
            h.get_by_label("Edit").click();
            h.run();
            h.run();
            assert_eq!(ground_of(&h.output().shapes, title), [(t.fill, CornerRadius::same(2))], "{theme:?}: open, the fill box");
            let item = h.state().1;
            assert!(item.top() > title.bottom() && item.left() > title.left(), "{theme:?}: its menu under it: {item:?}");
            h.key_press(Key::Escape);
            h.run();
            h.run();
            assert!(ground_of(&h.output().shapes, title).is_empty(), "{theme:?}: closed again");
        }
    }
}

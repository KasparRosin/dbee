//! The colour swatch (`swatch`): a 20 square of a connection colour, or `card` with the no-colour
//! mark; the chosen one has a 2 px `ink` edge. A colour shown as a mark is a square, never a
//! circle (foundations). Reported to accessibility as a radio button named for its colour.

use dbee_config::ConnectionColor;
use egui::{CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget, WidgetInfo, WidgetType};

use super::icon::Icon;
use super::{Look, focus, painter};
use crate::theme::size::{RADIUS_CONTROL, SWATCH};
use crate::theme::{connection_color, tokens};

/// The chosen swatch's edge.
const CHOSEN_EDGE: f32 = 2.0;

/// The colours a connection can have, in the order the form offers them.
pub const COLOURS: [ConnectionColor; 8] = [
    ConnectionColor::None,
    ConnectionColor::Red,
    ConnectionColor::Orange,
    ConnectionColor::Yellow,
    ConnectionColor::Green,
    ConnectionColor::Blue,
    ConnectionColor::Purple,
    ConnectionColor::Gray,
];

/// `colour`'s name, as the form names it to accessibility and automation.
pub fn name(colour: ConnectionColor) -> &'static str {
    match colour {
        ConnectionColor::None => "none",
        ConnectionColor::Red => "red",
        ConnectionColor::Orange => "orange",
        ConnectionColor::Yellow => "yellow",
        ConnectionColor::Green => "green",
        ConnectionColor::Blue => "blue",
        ConnectionColor::Purple => "purple",
        ConnectionColor::Gray => "gray",
    }
}

/// A swatch, 20 by 20.
pub struct Swatch {
    colour: ConnectionColor,
    selected: bool,
    look: Option<Look>,
}

impl Swatch {
    pub fn new(colour: ConnectionColor, selected: bool) -> Swatch {
        Swatch { colour, selected, look: None }
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Swatch {
        self.look = Some(look);
        self
    }
}

impl Widget for Swatch {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, r) = ui.allocate_exact_size(Vec2::splat(SWATCH), Sense::click());
        r.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, ui.is_enabled(), self.selected, name(self.colour)));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let radius = CornerRadius::same(RADIUS_CONTROL);
        match connection_color(self.colour) {
            Some(colour) => {
                p.rect_filled(rect, radius, colour);
            }
            None => {
                p.rect_filled(rect, radius, t.card);
                Icon::NoColour.paint(&p, rect.center(), t.text_muted);
                if !self.selected {
                    p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
                }
            }
        }
        if self.selected {
            p.rect_stroke(rect, radius, Stroke::new(CHOSEN_EDGE, t.ink), StrokeKind::Inside);
        }
        if look.focused {
            focus::box_around(&p, rect);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use egui::accesskit::{Role, Toggled};
    use egui::{CornerRadius, Key, Rect, Theme, Vec2};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, harness, icon_in, painted};
    use crate::theme::connection_color;

    #[test]
    fn each_swatch_matches_the_page() {
        // 20 by 20, corners 2. A colour fills it, with no edge; none is `card` in a
        // 1 px `line_strong` edge with the no-colour mark in `text_muted`. Chosen: a 2 px `ink`
        // edge inside. Focus: a 2 px `ink` box 4 px around it. Review Focus 5: dark too.
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for colour in COLOURS {
                for selected in [false, true] {
                    for focus in [false, true] {
                        let at_ = format!("{theme:?} {colour:?} selected {selected} focus {focus}");
                        let mut at = Rect::NOTHING;
                        let shapes = painted(theme, |ui| {
                            ui.add_space(ROOM);
                            ui.horizontal(|ui| {
                                ui.add_space(ROOM);
                                let s = Swatch::new(colour, selected);
                                at = if focus { ui.add(s.look(focused)) } else { ui.add(s) }.rect;
                            });
                        });
                        assert_eq!(at.size(), Vec2::splat(20.0), "{at_}: 20 square");
                        let ground = connection_color(colour).unwrap_or(t.card);
                        assert_eq!(fills_in(&shapes, at), [(at, ground)], "{at_}: the ground");
                        let radius = CornerRadius::same(2);
                        let mut want = match (selected, colour) {
                            (true, _) => vec![Edge { outer: at, width: 2.0, colour: t.ink, radius }],
                            (false, ConnectionColor::None) => vec![Edge { outer: at, width: 1.0, colour: t.line_strong, radius }],
                            (false, _) => vec![],
                        };
                        if focus {
                            want.push(Edge { outer: at.expand(4.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(4) });
                        }
                        assert_eq!(edges_in(&shapes, at, 4.0), want, "{at_}: the edges");
                        let (colours, bounds) = icon_in(&shapes, at);
                        if colour == ConnectionColor::None {
                            assert_eq!(colours, [t.text_muted], "{at_}: the no-colour mark");
                            assert!((bounds.center() - at.center()).length() <= 1.0, "{at_}: {bounds:?} centred in {at:?}");
                        } else {
                            assert_eq!(colours, [], "{at_}: nothing on a colour, and no circle");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_swatch_says_its_colour_and_whether_it_is_chosen() {
        let mut h = harness(Theme::Light, ConnectionColor::Red, |ui, chosen: &mut ConnectionColor| {
            ui.horizontal(|ui| {
                for colour in COLOURS {
                    if ui.add(Swatch::new(colour, *chosen == colour)).clicked() {
                        *chosen = colour;
                    }
                }
            });
        });
        h.run();
        let toggled =
            |h: &egui_kittest::Harness<'_, ConnectionColor>, name: &str| h.get_by_role_and_label(Role::RadioButton, name).accesskit_node().toggled();
        assert_eq!(toggled(&h, "red"), Some(Toggled::True));
        assert_eq!(toggled(&h, "gray"), Some(Toggled::False));
        h.key_press(Key::Tab);
        h.run();
        h.key_press(Key::Space);
        h.run();
        assert_eq!(*h.state(), ConnectionColor::None, "Tab to the first, Space chose it");
    }
}

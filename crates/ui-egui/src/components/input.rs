//! The one-line text input (`input`) and the field (`field`: a label over an input, an error line
//! under it). What is typed is drawn and kept as typed, never in capitals; only a field's label is.

use std::sync::Arc;

use egui::epaint::RectShape;
use egui::{
    Align, CornerRadius, Frame, Galley, Id, Layout, Margin, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextBuffer, TextEdit, Ui, UiBuilder,
    Widget, WidgetInfo, WidgetType, pos2, vec2,
};

use super::{Look, focus, painter};
use crate::theme::size::{FIELD_GAP, INPUT_H, INPUT_PAD_X, RADIUS_CONTROL};
use crate::theme::{Text, tokens};

/// A one-line text input, 28 tall, in the `body` or the `data` font; off when added with
/// `ui.add_enabled(false, …)`.
pub struct Input<'a> {
    text: &'a mut String,
    font: Text,
    placeholder: &'a str,
    prefix: &'a str,
    password: bool,
    error: bool,
    width: f32,
    id: Option<Id>,
    look: Option<Look>,
}

// The lifetime comes from `text`, so `Input::body` and `Input::data` serve as one fn type.
impl Input<'_> {
    /// In `body`: names, words.
    pub fn body(text: &mut String) -> Input<'_> {
        Input { text, font: Text::Body, placeholder: "", prefix: "", password: false, error: false, width: f32::INFINITY, id: None, look: None }
    }

    /// In `data`: hosts, ports, users, URLs, values.
    pub fn data(text: &mut String) -> Input<'_> {
        Input { font: Text::Data, ..Input::body(text) }
    }
}

impl<'a> Input<'a> {
    /// Shown in `text_muted` while nothing is typed.
    pub fn placeholder(mut self, words: &'a str) -> Input<'a> {
        self.placeholder = words;
        self
    }

    /// `words` in `text_muted` before the text, in its font, one space before it: not part of the
    /// text, so it can't be selected or deleted (a raw condition's `WHERE`).
    pub fn prefix(mut self, words: &'a str) -> Input<'a> {
        self.prefix = words;
        self
    }

    /// Dots in place of what is typed.
    pub fn password(mut self, hidden: bool) -> Input<'a> {
        self.password = hidden;
        self
    }

    /// A `danger` edge, whether focused or not.
    pub fn error(mut self, error: bool) -> Input<'a> {
        self.error = error;
        self
    }

    /// This wide, or as wide as there is room for (the default fills the row).
    pub fn desired_width(mut self, width: f32) -> Input<'a> {
        self.width = width;
        self
    }

    /// The text edit's egui id, for asking for its focus.
    pub fn id(mut self, id: Id) -> Input<'a> {
        self.id = Some(id);
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Input<'a> {
        self.look = Some(look);
        self
    }
}

/// What an [`Input`] drew: its text edit's response, and where its prefix is drawn when it has one.
pub struct Drawn {
    pub field: Response,
    pub prefix: Option<Rect>,
}

impl Widget for Input<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui).field
    }
}

impl Input<'_> {
    /// Draws it; its response and its prefix's place.
    pub fn show(self, ui: &mut Ui) -> Drawn {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        // The ground goes under the words, which egui's text edit draws.
        let ground = p.add(Shape::Noop);
        let off = !ui.is_enabled();
        let colour = if off { t.text_faint } else { t.text };
        let (font, hidden) = (self.font, self.password);
        let mut layouter = move |ui: &Ui, text: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> {
            let shown = if hidden { dots(text.as_str()) } else { text.as_str().to_owned() };
            ui.fonts_mut(|f| f.layout_job(font.job_as_written(&shown, colour)))
        };
        // The words stay this far in whatever the edge does, so nothing moves with focus; a
        // prefix and a space before them push them further in.
        let prefix = (!self.prefix.is_empty()).then(|| {
            let colour = if off { t.text_faint } else { t.text_muted };
            let words = ui.painter().layout_job(font.job_as_written(self.prefix, colour));
            let space = ui.painter().layout_job(font.job_as_written(" ", colour)).size().x;
            (words, space)
        });
        let pad = INPUT_PAD_X + 1.0;
        let lead = prefix.as_ref().map_or(0.0, |(words, space)| words.size().x + space);
        let inset = (INPUT_PAD_X + 1.0) as i8;
        let left = (pad + lead).ceil().min(f32::from(i8::MAX)) as i8;
        let mut edit = TextEdit::singleline(self.text)
            .font(font.font())
            .text_color(colour)
            .password(self.password)
            .frame(Frame::NONE.inner_margin(Margin { left, right: inset, top: 0, bottom: 0 }))
            .desired_width(self.width)
            .min_size(vec2(0.0, INPUT_H))
            .vertical_align(Align::Center)
            .layouter(&mut layouter);
        if let Some(id) = self.id {
            edit = edit.id(id);
        }
        if !self.placeholder.is_empty() {
            edit = edit.hint_text(font.rich_as_written(self.placeholder));
        }
        // egui colours the placeholder with its weak text colour, and fades what an off Ui draws.
        let (weak, opacity) = (ui.visuals().weak_text_color, ui.opacity());
        ui.visuals_mut().weak_text_color = Some(if off { t.text_faint } else { t.text_muted });
        let alpha = ui.visuals().disabled_alpha();
        if off && alpha > 0.0 {
            ui.set_opacity(opacity / alpha);
        }
        let r = ui.add(edit);
        ui.set_opacity(opacity);
        ui.visuals_mut().weak_text_color = weak;
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        let rect = r.rect;
        let radius = CornerRadius::same(RADIUS_CONTROL);
        p.set(ground, RectShape::filled(rect, radius, t.card));
        if self.error {
            p.rect_stroke(rect, radius, Stroke::new(1.0, t.danger), StrokeKind::Inside);
        } else if look.focused {
            focus::own_edge(&p, rect, RADIUS_CONTROL);
        } else {
            p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        }
        let prefix = prefix.map(|(words, _)| {
            let at = Rect::from_min_size(pos2(rect.left() + pad, rect.center().y - words.size().y / 2.0), words.size());
            p.galley(at.min, words, t.text_muted);
            at
        });
        Drawn { field: r, prefix }
    }
}

/// A dot for each letter of `text`, as a password input draws it.
fn dots(text: &str) -> String {
    std::iter::repeat_n(egui::epaint::text::PASSWORD_REPLACEMENT_CHAR, text.chars().count()).collect()
}

/// A field: its label above its input, an error line under it when there is one.
pub struct Field<'a> {
    label: Option<&'a str>,
    input: Input<'a>,
    error: Option<&'a str>,
    keep_error_room: bool,
}

/// What a [`Field`] drew: its input's response, and its error line's when it shows one.
pub struct Shown {
    pub input: Response,
    pub error: Option<Response>,
}

impl<'a> Field<'a> {
    /// `label` over `input`.
    pub fn new(label: &'a str, input: Input<'a>) -> Field<'a> {
        Field { label: Some(label), input, error: None, keep_error_room: false }
    }

    /// `input` with no label, where the words above it already say what to type (a dialog's).
    pub fn unlabelled(input: Input<'a>) -> Field<'a> {
        Field { label: None, ..Field::new("", input) }
    }

    /// The error line under the input, when there is one.
    pub fn error(mut self, error: Option<&'a str>) -> Field<'a> {
        self.error = error;
        self
    }

    /// The error line's room stays when there is no error, so what is under it doesn't move.
    pub fn keep_error_room(mut self, keep: bool) -> Field<'a> {
        self.keep_error_room = keep;
        self
    }

    /// Draws it; the input's response and the error line's.
    pub fn show(self, ui: &mut Ui) -> Shown {
        let Field { label: words, input, error, keep_error_room } = self;
        let width = input.width.min(ui.available_width()).max(0.0);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let named = words.map(|words| label(ui, words, |_| {}));
            let mut r = ui.add(input.desired_width(width));
            if let Some(named) = &named {
                r = r.labelled_by(named.id);
            }
            let error = match error {
                Some(words) => Some(error_line(ui, words, width)),
                None => {
                    if keep_error_room {
                        ui.add_space(FIELD_GAP + Text::Small.line());
                    }
                    None
                }
            };
            Shown { input: r, error }
        })
        .inner
    }
}

impl Widget for Field<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui).input
    }
}

/// A field's label over what follows in `ui` that isn't an input (the connection form's colours,
/// choices and file boxes), as a field draws it: its words in `label` capitals, `text_muted`, on a
/// row of `ui`'s width, 4 above what comes next. `beside` gets the rest of the row (a note).
/// Reported to accessibility as written.
pub fn label(ui: &mut Ui, words: &str, beside: impl FnOnce(&mut Ui)) -> Response {
    let t = tokens(ui.ctx());
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (id, row) = ui.allocate_space(vec2(ui.available_width(), Text::Label.line()));
    ui.spacing_mut().item_spacing.y = spacing;
    ui.add_space(FIELD_GAP);
    let galley = ui.painter().layout_job(Text::Label.job(words, t.text_muted));
    let at = Rect::from_min_size(pos2(row.left(), row.center().y - galley.size().y / 2.0), galley.size());
    let r = ui.interact(at, id, Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), words));
    painter(ui).galley(at.min, galley, t.text_muted);
    let rest = Rect::from_min_max(pos2(at.right(), row.top()), row.max);
    beside(&mut ui.new_child(UiBuilder::new().max_rect(rest).layout(Layout::left_to_right(Align::Center))));
    r
}

/// The error line under a field's input, `width` wide: 4 under it, `words` in `small`, `danger`,
/// wrapping at the width, never cut: the reason is what the user needs. Reported to
/// accessibility as written.
fn error_line(ui: &mut Ui, words: &str, width: f32) -> Response {
    let t = tokens(ui.ctx());
    ui.add_space(FIELD_GAP);
    let mut job = Text::Small.job_as_written(words, t.danger);
    job.wrap.max_width = width;
    let galley = ui.painter().layout_job(job);
    let (rect, r) = ui.allocate_exact_size(vec2(width, galley.size().y.max(Text::Small.line())), Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), words));
    painter(ui).galley(rect.min, galley, t.danger);
    r
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui::{CornerRadius, Key, Modifiers, Rect, Theme, vec2};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::components::button::TextButton;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness, painted, texts};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// The font each text within `at` is drawn in (egui lays out an empty text too).
    fn fonts_in(shapes: &[egui::epaint::ClippedShape], at: Rect) -> Vec<egui::FontId> {
        flat(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) if !t.galley.text().is_empty() && at.contains_rect(t.galley.rect.translate(t.pos.to_vec2())) => {
                    t.galley.job.sections.first().map(|s| s.format.font_id.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_input_state_matches_the_page() {
        // Node 161106: 28 tall, corners 2, on `card`; the words 8 inside the 1 px edge and centred
        // up and down, in `body` or `data`; the placeholder `text_muted`; off, the words
        // `text_faint` and the edge as it was; keyboard focus a 2 px `ink` edge; an error a 1 px
        // `danger` edge, also when focused.
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for font in [Text::Body, Text::Data] {
                for state in ["empty", "placeholder", "filled", "focus", "disabled", "error", "error focused"] {
                    let at_ = format!("{theme:?} {font:?} {state}");
                    let mut at = Rect::NOTHING;
                    let shapes = painted(theme, |ui| {
                        ui.add_space(ROOM);
                        let mut text = if matches!(state, "empty" | "placeholder") { String::new() } else { "shop".to_owned() };
                        let input = match font {
                            Text::Body => Input::body(&mut text),
                            _ => Input::data(&mut text),
                        };
                        let input = input.placeholder(if state == "placeholder" { "Optional" } else { "" }).desired_width(160.0);
                        at = match state {
                            "disabled" => ui.add_enabled(false, input).rect,
                            "focus" => ui.add(input.look(focused)).rect,
                            "error" => ui.add(input.error(true)).rect,
                            "error focused" => ui.add(input.error(true).look(focused)).rect,
                            _ => ui.add(input).rect,
                        };
                    });
                    assert_eq!(at.size(), vec2(160.0, 28.0), "{at_}: 160 wide as asked, 28 tall");
                    assert_eq!(fills_in(&shapes, at), [(at, t.card)], "{at_}: on card");
                    let radius = CornerRadius::same(2);
                    let edge = match state {
                        "focus" => Edge { outer: at, width: 2.0, colour: t.ink, radius },
                        "error" | "error focused" => Edge { outer: at, width: 1.0, colour: t.danger, radius },
                        _ => Edge { outer: at, width: 1.0, colour: t.line_strong, radius },
                    };
                    assert_eq!(edges_in(&shapes, at, 0.0), [edge], "{at_}: the edge");
                    // egui lays out an empty text too.
                    let drawn: Vec<_> = texts(&shapes).into_iter().filter(|(w, r, _)| !w.is_empty() && at.contains_rect(*r)).collect();
                    if state == "empty" {
                        assert!(drawn.is_empty(), "{at_}: nothing drawn: {drawn:?}");
                        continue;
                    }
                    let (words, colour) = match state {
                        "placeholder" => ("Optional", t.text_muted),
                        "disabled" => ("shop", t.text_faint),
                        _ => ("shop", t.text),
                    };
                    let [(w, r, c)] = drawn.as_slice() else { panic!("{at_}: one text in the input: {drawn:?}") };
                    assert_eq!((w.as_str(), *c), (words, colour), "{at_}: the words and their colour");
                    assert!(close(r.left(), at.left() + 9.0), "{at_}: {r:?} 8 inside the edge of {at:?}");
                    assert!(close(r.center().y, at.center().y) && r.height() == font.line(), "{at_}: {r:?} centred, one line");
                    assert_eq!(fonts_in(&shapes, at), [font.font()], "{at_}: the font");
                }
            }
        }
    }

    /// What the focus test's window saw.
    struct Seen {
        text: String,
        at: Rect,
        focus: bool,
    }

    impl Default for Seen {
        fn default() -> Seen {
            Seen { text: String::new(), at: Rect::NOTHING, focus: false }
        }
    }

    #[test]
    fn a_field_clicked_with_the_mouse_keeps_its_edge() {
        // Foundations' Focus rule: a field clicked with the mouse keeps its `line_strong` edge; Tab
        // to it and its edge turns 2 px `ink`; the words don't move between the two.
        for (theme, t) in both() {
            let seen = Seen { text: "db.example.com".into(), ..Seen::default() };
            let mut h = harness(theme, seen, |ui, s: &mut Seen| {
                ui.add_space(ROOM);
                ui.add(TextButton::new("Before"));
                let r = ui.add(Field::new("Host", Input::data(&mut s.text).desired_width(240.0)));
                s.at = r.rect;
                s.focus = r.has_focus();
            });
            h.run();
            h.get_by_label("Host").click();
            h.run();
            let at = h.state().at;
            assert!(h.state().focus, "{theme:?}: the click gave it focus");
            let shapes = h.output().shapes.clone();
            let line = Edge { outer: at, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, at, 0.0), [line], "{theme:?}: clicked, its edge stays");
            let words = |shapes: &[egui::epaint::ClippedShape]| texts(shapes).into_iter().find(|(w, _, _)| w == "db.example.com").map(|(_, r, _)| r);
            let clicked = words(&shapes);
            // Away and back with the keyboard.
            h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
            h.run();
            assert!(!h.state().focus, "{theme:?}: Shift+Tab took the focus away");
            h.key_press(Key::Tab);
            h.run();
            assert!(h.state().focus, "{theme:?}: Tab gave it back");
            let shapes = h.output().shapes.clone();
            let ink = Edge { outer: at, width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, at, 0.0), [ink], "{theme:?}: tabbed to, 2 px ink");
            assert!(clicked.is_some() && words(&shapes) == clicked, "{theme:?}: the words stay where they were: {clicked:?}");
            assert_eq!(h.state().at, at, "{theme:?}: nothing moved");
        }
    }

    #[test]
    fn an_error_edge_wins_over_the_focus_edge() {
        for (theme, t) in both() {
            let mut h = harness(theme, Seen::default(), |ui, s: &mut Seen| {
                ui.add_space(ROOM);
                let r = ui.add(Field::new("URL", Input::data(&mut s.text).error(true).desired_width(240.0)));
                s.at = r.rect;
                s.focus = r.has_focus();
            });
            h.run();
            h.key_press(Key::Tab);
            h.run();
            assert!(h.state().focus, "{theme:?}: Tab gave it focus");
            let at = h.state().at;
            let red = Edge { outer: at, width: 1.0, colour: t.danger, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&h.output().shapes, at, 0.0), [red], "{theme:?}: the red edge");
        }
    }

    #[test]
    fn an_input_keeps_what_is_typed_as_typed() {
        // Review Focus 3: a host and a file path keep their case, drawn and given back; only the
        // field's own label is drawn in capitals.
        let mut h = harness(Theme::Light, [String::new(), String::new()], |ui, s: &mut [String; 2]| {
            let [host, file] = s;
            ui.add(Field::new("Host", Input::data(host)));
            ui.add(Field::new("File", Input::body(file)));
        });
        h.run();
        for (label, typed) in [("Host", "Db.Example.COM"), ("File", "/home/Me/Shop.sqlite")] {
            h.get_by_label(label).click();
            h.run();
            h.get_by_label(label).type_text(typed);
            h.run();
            h.run();
            assert_eq!(h.get_by_label(label).value().as_deref(), Some(typed), "{label}: its value, as typed");
            assert_eq!(h.get_by_label(label).accesskit_node().role(), Role::TextInput, "{label}: a text input");
        }
        assert_eq!(h.state(), &["Db.Example.COM".to_owned(), "/home/Me/Shop.sqlite".to_owned()], "given back as typed");
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        for words in ["HOST", "Db.Example.COM", "FILE", "/home/Me/Shop.sqlite"] {
            assert!(drawn.iter().any(|w| w == words), "`{words}` drawn: {drawn:?}");
        }
        for never in ["DB.EXAMPLE.COM", "/HOME/ME/SHOP.SQLITE", "Host", "File"] {
            assert!(!drawn.iter().any(|w| w == never), "`{never}` never drawn: {drawn:?}");
        }
    }

    #[test]
    fn a_field_shows_its_error_line() {
        // Node 161156: the label (`label` caps, `text_muted`, 14 tall), 4, the input, 4, the error
        // line (`small`, `danger`, 16 tall): 66 with the line, 46 without.
        for (theme, t) in both() {
            let mut at = [Rect::NOTHING; 3];
            let shapes = painted(theme, |ui| {
                ui.add_space(ROOM);
                ui.horizontal_top(|ui| {
                    let mut text = "shop".to_owned();
                    let field = Field::new("Database URL", Input::body(&mut text).error(true).desired_width(160.0));
                    at[0] = ui.vertical(|ui| ui.add(field.error(Some("Not a database URL")))).response.rect;
                    let mut text = String::new();
                    at[1] = ui.vertical(|ui| ui.add(Field::new("Name", Input::body(&mut text).desired_width(160.0)))).response.rect;
                    let mut text = String::new();
                    let kept = Field::new("Name", Input::body(&mut text).desired_width(160.0)).keep_error_room(true);
                    at[2] = ui.vertical(|ui| ui.add(kept)).response.rect;
                });
            });
            assert_eq!(at.map(|r| r.size()), [vec2(160.0, 66.0), vec2(160.0, 46.0), vec2(160.0, 66.0)], "{theme:?}: the fields' sizes");
            let drawn = texts(&shapes);
            let find = |words: &str, within: Rect| drawn.iter().find(|(w, r, _)| w == words && within.contains_rect(*r)).map(|(_, r, c)| (*r, *c));
            let field = at[0];
            let (label, colour) = find("DATABASE URL", field).unwrap_or_else(|| panic!("{theme:?}: the label: {drawn:?}"));
            assert_eq!(colour, t.text_muted, "{theme:?}: the label's colour");
            assert!(close(label.top(), field.top()) && close(label.left(), field.left()) && label.height() == 14.0, "{theme:?}: {label:?}");
            let input = Rect::from_min_size(field.min + vec2(0.0, 18.0), vec2(160.0, 28.0));
            assert_eq!(fills_in(&shapes, input), [(input, t.card)], "{theme:?}: the input 4 under the label");
            let (line, colour) = find("Not a database URL", field).unwrap_or_else(|| panic!("{theme:?}: the error line: {drawn:?}"));
            assert_eq!(colour, t.danger, "{theme:?}: the error line's colour");
            assert!(close(line.top(), input.bottom() + 4.0) && close(line.left(), field.left()) && line.height() == 16.0, "{theme:?}: {line:?}");
            assert_eq!(fonts_in(&shapes, line), [Text::Small.font()], "{theme:?}: in `small`");
            assert!(!drawn.iter().any(|(_, r, _)| at[2].contains_rect(*r) && r.top() > at[2].top() + 46.0), "{theme:?}: the kept room is empty");
        }
        let mut h = harness(Theme::Light, String::from("shop"), |ui, text: &mut String| {
            ui.add(Field::new("Database URL", Input::body(text).error(true)).error(Some("Not a database URL")));
        });
        h.run();
        assert!(h.query_by_label("Not a database URL").is_some(), "the error line is read out");
        assert!(h.query_by_label("Database URL").is_some_and(|n| n.accesskit_node().role() == Role::TextInput), "the input is labelled");
    }

    #[test]
    fn the_caret_is_one_pixel_of_ink() {
        // The page's caret is 1 px of `ink`, 16 tall. egui draws it as tall as the text's row and
        // 1.5 more at either end, which can't be set: 21 in `body`, 19 in `data`.
        for (theme, t) in both() {
            for font in [Text::Body, Text::Data] {
                let mut h = harness(theme, Seen::default(), move |ui, s: &mut Seen| {
                    ui.add_space(ROOM);
                    let input = if font == Text::Body { Input::body(&mut s.text) } else { Input::data(&mut s.text) };
                    let r = ui.add(input.desired_width(160.0));
                    s.at = r.rect;
                    s.focus = r.has_focus();
                });
                h.run();
                h.key_press(Key::Tab);
                h.run();
                h.get_by_role(Role::TextInput).type_text("shop");
                h.run();
                let (at, focus) = (h.state().at, h.state().focus);
                assert!(focus, "{theme:?} {font:?}: Tab gave it focus");
                let carets: Vec<(Rect, egui::Stroke)> = flat(&h.output().shapes)
                    .into_iter()
                    .filter_map(|s| match s {
                        egui::Shape::LineSegment { points: [a, b], stroke } if a.x == b.x && at.contains(a) => {
                            Some((Rect::from_two_pos(a, b), stroke))
                        }
                        _ => None,
                    })
                    .collect();
                let [(caret, stroke)] = carets.as_slice() else { panic!("{theme:?} {font:?}: one caret in {at:?}: {carets:?}") };
                assert_eq!((stroke.width, stroke.color), (1.0, t.ink), "{theme:?} {font:?}: 1 px of ink");
                assert!(close(caret.center().y, at.center().y), "{theme:?} {font:?}: {caret:?} centred in {at:?}");
                assert_eq!(caret.height(), font.line() + 3.0, "{theme:?} {font:?}: as tall as egui draws it");
            }
        }
    }

    #[test]
    fn a_prefix_is_drawn_before_the_text_and_is_not_part_of_it() {
        // 1B: the raw condition's field starts with a grey `WHERE` in the field's font; the
        // condition is typed after it and is the field's whole value. Select all and delete
        // leaves the prefix.
        for (theme, t) in both() {
            let mut h = harness(theme, Seen::default(), |ui, s: &mut Seen| {
                ui.add_space(ROOM);
                let r = ui.add(Input::data(&mut s.text).prefix("WHERE").desired_width(240.0));
                s.at = r.rect;
                s.focus = r.has_focus();
            });
            h.run();
            h.get_by_role(Role::TextInput).click();
            h.run();
            h.get_by_role(Role::TextInput).type_text("a > 1");
            h.run();
            h.run();
            assert_eq!(h.state().text, "a > 1", "{theme:?}: the value is what was typed");
            let at = h.state().at;
            let shapes = h.output().shapes.clone();
            let drawn = texts(&shapes);
            let find = |words: &str| drawn.iter().find(|(w, r, _)| w == words && at.contains_rect(*r)).map(|(_, r, c)| (*r, *c));
            let (prefix, colour) = find("WHERE").unwrap_or_else(|| panic!("{theme:?}: WHERE drawn in the field: {drawn:?}"));
            assert_eq!(colour, t.text_muted, "{theme:?}: WHERE in text_muted");
            assert!(close(prefix.left(), at.left() + 9.0), "{theme:?}: {prefix:?} 8 inside the edge of {at:?}");
            let (typed, colour) = find("a > 1").unwrap_or_else(|| panic!("{theme:?}: the condition drawn: {drawn:?}"));
            assert_eq!(colour, t.text, "{theme:?}: the condition in text");
            assert!(typed.left() > prefix.right() + 2.0, "{theme:?}: {typed:?} after {prefix:?} and a space");
            assert!(close(typed.center().y, prefix.center().y), "{theme:?}: on one line");
            assert_eq!(fonts_in(&shapes, prefix), [Text::Data.font()], "{theme:?}: WHERE in the field's font");
            h.key_press_modifiers(Modifiers::COMMAND, Key::A);
            h.run();
            h.key_press(Key::Delete);
            h.run();
            h.run();
            assert_eq!(h.state().text, "", "{theme:?}: everything typed is gone");
            let drawn = texts(&h.output().shapes);
            assert!(drawn.iter().any(|(w, r, c)| w == "WHERE" && at.contains_rect(*r) && *c == t.text_muted), "{theme:?}: WHERE stays: {drawn:?}");
        }
    }

    #[test]
    fn a_password_input_hides_its_text() {
        let mut h = harness(Theme::Light, Seen::default(), |ui, s: &mut Seen| {
            ui.add(Field::new("Password", Input::data(&mut s.text).password(true)));
        });
        h.run();
        h.get_by_label("Password").click();
        h.run();
        h.get_by_label("Password").type_text("hunter2-Secret");
        h.run();
        h.run();
        assert!(h.state().text == "hunter2-Secret", "kept as typed");
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        // The messages leave out what was drawn and the value: they could hold the secret.
        assert!(!drawn.iter().any(|w| w.contains("hunter2") || w.contains("HUNTER2")), "the password is never drawn");
        assert!(drawn.iter().any(|w| w == &"•".repeat(14)), "a dot for each letter");
        let node = h.get_by_label("Password");
        assert_eq!(node.accesskit_node().role(), Role::PasswordInput);
        assert!(!node.value().unwrap_or_default().contains("hunter2"), "its accessible value is masked");
    }
}

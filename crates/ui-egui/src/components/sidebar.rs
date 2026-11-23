//! The sidebar's header: the filter with the refresh beside it (`sidebar/filter`), and the count
//! under it (`sidebar/count`).
//!
//! Each is a view: it takes what to show and says what was clicked; the screen tags what it drew,
//! adds the hover texts and acts. What is typed in the filter is drawn and kept as typed. The
//! count's words are dbee's own: drawn in `label` capitals, read out as written.

use std::sync::Arc;

use egui::{
    Align, CornerRadius, Frame, Galley, Id, Margin, Rect, Response, Sense, TextBuffer, TextEdit, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2,
    vec2,
};

use super::button::IconButton;
use super::icon::Icon;
use super::status::{Piece, piece};
use super::{Look, focus, painter};
use crate::theme::size::{BAR_CELL_H, BAR_H, BAR_LINE, ICON_BUTTON, SIDEBAR_COUNT_H, SIDEBAR_PAD_RIGHT, SIDEBAR_PAD_X};
use crate::theme::{Text, tokens};
use crate::tree::thousands;

/// What the refresh beside the filter can do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// A click asks for a refresh.
    Idle,
    /// A refresh runs: the plain icon, and a click does nothing (the count says `LOADING`).
    Running,
    /// It can't run now: the icon in `text_faint`, and a click does nothing.
    Off,
}

/// The filter: a field the row's width, with the refresh at its end when given.
pub struct Filter<'a> {
    text: &'a mut String,
    placeholder: &'a str,
    refresh: Option<Refresh>,
    id: Option<Id>,
    look: Option<Look>,
}

/// What a [`Filter`] drew: the field, the refresh when there is one, and whether the refresh was
/// clicked to run.
pub struct FilterShown {
    pub field: Response,
    pub refresh: Option<Response>,
    pub asked: bool,
}

impl<'a> Filter<'a> {
    /// A filter over `text`, showing `placeholder` while nothing is typed; no refresh.
    pub fn new(text: &'a mut String, placeholder: &'a str) -> Filter<'a> {
        Filter { text, placeholder, refresh: None, id: None, look: None }
    }

    /// The refresh at the row's end, as it is now.
    pub fn refresh(mut self, refresh: Refresh) -> Filter<'a> {
        self.refresh = Some(refresh);
        self
    }

    /// The field's egui id, for asking for its focus.
    pub fn id(mut self, id: Id) -> Filter<'a> {
        self.id = Some(id);
        self
    }

    /// Draws the field with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Filter<'a> {
        self.look = Some(look);
        self
    }

    /// Draws it where `ui` puts it next, as wide as `ui`.
    pub fn show(self, ui: &mut Ui) -> FilterShown {
        let t = tokens(ui.ctx());
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        let p = painter(ui);
        p.rect_filled(Rect::from_min_max(pos2(row.left(), row.bottom() - BAR_LINE), row.max), CornerRadius::ZERO, t.line);
        let end = row.right() - SIDEBAR_PAD_RIGHT;
        let mid = row.top() + BAR_CELL_H / 2.0;
        let square = self.refresh.map(|_| Rect::from_min_size(pos2(end - ICON_BUTTON, mid - ICON_BUTTON / 2.0), Vec2::splat(ICON_BUTTON)));
        let room = Rect::from_min_max(row.min, pos2(square.map_or(end, |s| s.left()), row.top() + BAR_CELL_H));
        // Children, so the row's cursor stays past the whole row.
        let mut inside = ui.new_child(UiBuilder::new().id_salt("dbee-sidebar-field").max_rect(room));
        let field = field(&mut inside, self.text, self.placeholder, self.id);
        let look = self.look.unwrap_or_else(|| Look::of(ui, &field));
        if look.focused {
            focus::box_inside(&p, field.rect);
        }
        let (refresh, asked) = match self.refresh.zip(square) {
            Some((state, square)) => {
                let mut cell = ui.new_child(UiBuilder::new().id_salt("dbee-sidebar-refresh").max_rect(square));
                let r = refresh_button(&mut cell, state);
                let asked = state == Refresh::Idle && r.clicked();
                (Some(r), asked)
            }
            None => (None, false),
        };
        FilterShown { field, refresh, asked }
    }
}

/// The filter's field, filling `ui`: no edge or ground of its own, its words in `body` as typed,
/// [`SIDEBAR_PAD_X`] in and centred.
fn field(ui: &mut Ui, text: &mut String, placeholder: &str, id: Option<Id>) -> Response {
    let t = tokens(ui.ctx());
    let colour = t.text;
    let mut layouter = move |ui: &Ui, text: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> {
        ui.fonts_mut(|f| f.layout_job(Text::Body.job_as_written(text.as_str(), colour)))
    };
    let mut edit = TextEdit::singleline(text)
        .font(Text::Body.font())
        .text_color(colour)
        .frame(Frame::NONE.inner_margin(Margin { left: SIDEBAR_PAD_X as i8, right: 0, top: 0, bottom: 0 }))
        .desired_width(f32::INFINITY)
        .min_size(vec2(0.0, BAR_CELL_H))
        .vertical_align(Align::Center)
        .hint_text(Text::Body.rich_as_written(placeholder))
        .layouter(&mut layouter);
    if let Some(id) = id {
        edit = edit.id(id);
    }
    // egui colours the placeholder with its weak text colour.
    ui.visuals_mut().weak_text_color = Some(t.text_muted);
    ui.add(edit)
}

/// The refresh in `ui`'s square: the plain icon button; while a refresh runs the same icon, with no
/// hover ground and no click (the count says `LOADING`); or the button off.
fn refresh_button(ui: &mut Ui, state: Refresh) -> Response {
    let button = IconButton::plain(Icon::Refresh);
    match state {
        Refresh::Idle => ui.add(button),
        Refresh::Off => ui.add_enabled(false, button),
        Refresh::Running => {
            let (square, r) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), Sense::hover());
            r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, Icon::Refresh.name()));
            Icon::Refresh.paint(&painter(ui), square.center(), tokens(ui.ctx()).text);
            r
        }
    }
}

/// How many tables the tree holds: all of them, the filter's matches of them, or none known yet.
/// The word is dbee's, in the singular and the plural (`table`, `tables`): the total picks one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count<'a> {
    All(u64, [&'a str; 2]),
    Some(u64, u64, [&'a str; 2]),
    Loading,
}

impl Count<'_> {
    /// The words as written: `516 tables`, `3 of 516 tables`, `1 table`, `Loading`.
    pub fn words(self) -> String {
        let word = |n: u64, [one, many]: [&str; 2]| if n == 1 { one.to_owned() } else { many.to_owned() };
        match self {
            Count::All(n, words) => format!("{} {}", thousands(n), word(n, words)),
            Count::Some(m, n, words) => format!("{} of {} {}", thousands(m), thousands(n), word(n, words)),
            Count::Loading => "Loading".to_owned(),
        }
    }

    /// Draws it where `ui` puts it next, a row as wide as `ui`; the words' response.
    pub fn show(self, ui: &mut Ui) -> Response {
        let t = tokens(ui.ctx());
        let (row, r) = ui.allocate_exact_size(vec2(ui.available_width(), SIDEBAR_COUNT_H), Sense::hover());
        let words = self.words();
        let drawn = Piece::new(ui, Text::Label.job(&words, t.text_muted), words);
        piece(ui, r.id.with("words"), &drawn, pos2(row.left() + SIDEBAR_PAD_X, row.center().y), t.text_muted)
    }
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui::epaint::ClippedShape;
    use egui::{CornerRadius, Key, Modifiers, Rect, Shape, Theme, pos2, vec2};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::components::button::TextButton;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, flat, harness, icon_in, painted, sections, texts};
    use crate::theme::Text;
    use crate::theme::size::BAR_H;

    /// The header is drawn this wide (`sidebar/filter`, `sidebar/count`).
    const W: f32 = 259.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// What one frame of a filter drew: its shapes, the row, the field and the refresh.
    struct Drawn {
        shapes: Vec<ClippedShape>,
        row: Rect,
        field: Rect,
        refresh: Option<Rect>,
    }

    fn filter(theme: Theme, typed: &str, refresh: Option<Refresh>, look: Option<Look>) -> Drawn {
        let mut text = typed.to_owned();
        let (mut row, mut field, mut button) = (Rect::NOTHING, Rect::NOTHING, None);
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            ui.allocate_ui(vec2(W, BAR_H), |ui| {
                row = Rect::from_min_size(ui.cursor().min, vec2(W, BAR_H));
                let mut f = Filter::new(&mut text, "Filter tables");
                if let Some(r) = refresh {
                    f = f.refresh(r);
                }
                if let Some(l) = look {
                    f = f.look(l);
                }
                let shown = f.show(ui);
                field = shown.field.rect;
                button = shown.refresh.map(|r| r.rect);
            });
        });
        Drawn { shapes, row, field, refresh: button }
    }

    #[test]
    fn the_sidebar_filter_has_its_look() {
        // A row 36 tall with a 1 px `line` along its bottom and no ground. The field
        // fills it up to the refresh, 35 tall above the line, its words 12 in and centred, in
        // `body`: the placeholder `text_muted`, what is typed `text`. Keyboard focus is a 2 px
        // `ink` box 2 inside the field, corners 2. The refresh is a plain icon button, a 28
        // square 4 from the row's end, centred on the field, its icon `text`, also while a
        // refresh runs; off, the icon `text_faint`.
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for state in ["placeholder", "filled", "focus", "running", "off"] {
                let at_ = format!("{theme:?} {state}");
                let typed = if state == "placeholder" { "" } else { "ord" };
                let refresh = match state {
                    "running" => Refresh::Running,
                    "off" => Refresh::Off,
                    _ => Refresh::Idle,
                };
                let look = (state == "focus").then_some(focused);
                let Drawn { shapes, row, field, refresh: button } = filter(theme, typed, Some(refresh), look);
                let line = Rect::from_min_max(pos2(row.left(), row.bottom() - 1.0), row.max);
                assert_eq!(fills_in(&shapes, row), [(line, t.line)], "{at_}: no ground, a 1 px line along the bottom");
                assert_eq!(field, Rect::from_min_size(row.min, vec2(W - 4.0 - 28.0, 35.0)), "{at_}: the field, up to the refresh, above the line");
                let square = Rect::from_min_size(pos2(row.right() - 32.0, row.top() + 3.5), vec2(28.0, 28.0));
                assert_eq!(button, Some(square), "{at_}: the refresh, 4 from the end, centred on the field");
                let ring = Edge { outer: field.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                let want = if state == "focus" { vec![ring] } else { Vec::new() };
                assert_eq!(edges_in(&shapes, row, 0.0), want, "{at_}: the edges");
                let (words, colour) = if typed.is_empty() { ("Filter tables", t.text_muted) } else { ("ord", t.text) };
                let drawn: Vec<_> = texts(&shapes).into_iter().filter(|(w, r, _)| !w.is_empty() && field.contains_rect(*r)).collect();
                let [(w, r, c)] = drawn.as_slice() else { panic!("{at_}: one text in the field: {drawn:?}") };
                assert_eq!((w.as_str(), *c), (words, colour), "{at_}: the words and their colour");
                assert!(close(r.left(), field.left() + 12.0) && close(r.center().y, field.center().y), "{at_}: {r:?} 12 in, centred in {field:?}");
                assert_eq!(sections(&shapes, words).first().map(|s| s.1.clone()), Some(Text::Body.font()), "{at_}: in `body`");
                let (icon, _) = icon_in(&shapes, square);
                match state {
                    "off" => assert_eq!(icon, [t.text_faint], "{at_}: the icon off"),
                    _ => assert_eq!(icon, [t.text], "{at_}: the icon"),
                }
            }
        }
        // Without a refresh (the connection list's), the field takes the row.
        for (theme, _) in both() {
            let Drawn { row, field, refresh, .. } = filter(theme, "", None, None);
            assert_eq!((field, refresh), (Rect::from_min_size(row.min, vec2(W - 4.0, 35.0)), None), "{theme:?}: no refresh");
        }
    }

    #[test]
    fn a_running_refresh_shows_no_spinner() {
        // While a refresh runs the refresh keeps its plain icon, and a click asks nothing.
        for (theme, t) in both() {
            let Drawn { shapes, refresh, .. } = filter(theme, "", Some(Refresh::Running), None);
            let square = refresh.expect("the refresh is drawn");
            assert_eq!(icon_in(&shapes, square).0, [t.text], "{theme:?}: the plain icon");
            let spinner = flat(&shapes)
                .into_iter()
                .any(|s| matches!(s, Shape::Path(p) if p.stroke.width == 3.0 && p.points.iter().all(|q| square.contains(*q))));
            assert!(!spinner, "{theme:?}: no spinner's arc");
            let mut h = harness(theme, 0usize, |ui, asked: &mut usize| {
                ui.add_space(ROOM);
                ui.allocate_ui(vec2(W, BAR_H), |ui| {
                    let mut text = String::new();
                    *asked += usize::from(Filter::new(&mut text, "Filter tables").refresh(Refresh::Running).show(ui).asked);
                });
            });
            h.run();
            h.get_by_label("refresh").click();
            h.run();
            assert_eq!(*h.state(), 0, "{theme:?}: a click asks nothing");
        }
    }

    const TABLES: [&str; 2] = ["table", "tables"];

    #[test]
    fn the_count_has_its_look() {
        // A row 28 tall, its words 12 in and centred, in `label` capitals,
        // `text_muted`; read out as written.
        let counts = [
            (Count::All(516, TABLES), "516 TABLES", "516 tables"),
            (Count::Some(3, 516, TABLES), "3 OF 516 TABLES", "3 of 516 tables"),
            (Count::All(0, TABLES), "0 TABLES", "0 tables"),
            (Count::All(1, TABLES), "1 TABLE", "1 table"),
            (Count::Some(1, 1, TABLES), "1 OF 1 TABLE", "1 of 1 table"),
            (Count::Some(0, 1, TABLES), "0 OF 1 TABLE", "0 of 1 table"),
            (Count::Some(1, 2, TABLES), "1 OF 2 TABLES", "1 of 2 tables"),
            (Count::Loading, "LOADING", "Loading"),
            (Count::Some(1_234, 12_345, TABLES), "1,234 OF 12,345 TABLES", "1,234 of 12,345 tables"),
        ];
        for (theme, t) in both() {
            for (count, drawn, read) in counts {
                let at_ = format!("{theme:?} {drawn}");
                let (mut row, mut used) = (Rect::NOTHING, Rect::NOTHING);
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    ui.allocate_ui(vec2(W, 40.0), |ui| {
                        row = Rect::from_min_size(ui.cursor().min, vec2(W, 28.0));
                        count.show(ui);
                        used = ui.min_rect();
                    });
                });
                assert_eq!(used, row, "{at_}: a row 28 tall");
                let seen = texts(&shapes);
                let Some((_, r, c)) = seen.iter().find(|(w, _, _)| w == drawn) else { panic!("{at_}: drawn: {seen:?}") };
                assert_eq!(*c, t.text_muted, "{at_}: its colour");
                assert!(close(r.left(), row.left() + 12.0) && close(r.center().y, row.center().y), "{at_}: {r:?} 12 in, centred in {row:?}");
                assert_eq!(r.height(), Text::Label.line(), "{at_}: one line");
                assert_eq!(sections(&shapes, drawn).first().map(|s| s.1.clone()), Some(Text::Label.font()), "{at_}: in `label`");
                assert!(!seen.iter().any(|(w, _, _)| w == read && read != drawn), "{at_}: never drawn as written");
            }
        }
        let mut h = harness(Theme::Light, (), move |ui, _| {
            for (count, _, _) in counts {
                count.show(ui);
            }
        });
        h.run();
        for (_, drawn, read) in counts {
            let node = h.get_by_label(read);
            assert_eq!(node.accesskit_node().role(), Role::Label, "{read}: read out as written");
            assert!(h.query_by_label(drawn).is_none(), "{drawn}: never read out in capitals");
        }
    }

    /// What the filter's window saw.
    struct Seen {
        text: String,
        refresh: Refresh,
        field: Rect,
        focus: bool,
        asked: usize,
    }

    #[test]
    fn the_sidebar_filter_by_keyboard_and_pointer() {
        // The focus rule: Tab to the field and its ring shows, a click gives it focus
        // without the ring, and nothing moves between the two. What is typed is kept and drawn as
        // typed. The refresh asks for a refresh only when idle.
        for (theme, t) in both() {
            let seen = Seen { text: String::new(), refresh: Refresh::Idle, field: Rect::NOTHING, focus: false, asked: 0 };
            let mut h = harness(theme, seen, |ui, s: &mut Seen| {
                ui.add_space(ROOM);
                ui.add(TextButton::new("Before"));
                ui.allocate_ui(vec2(W, BAR_H), |ui| {
                    let shown = Filter::new(&mut s.text, "Filter tables").refresh(s.refresh).show(ui);
                    s.field = shown.field.rect;
                    s.focus = shown.field.has_focus();
                    s.asked += usize::from(shown.asked);
                });
            });
            h.run();
            h.get_by_role(Role::TextInput).click();
            h.run();
            assert!(h.state().focus, "{theme:?}: the click gave it focus");
            h.get_by_role(Role::TextInput).type_text("Order_Items");
            h.run();
            h.run();
            let field = h.state().field;
            let ring = Edge { outer: field.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
            let shapes = h.output().shapes.clone();
            assert_eq!(edges_in(&shapes, field, 0.0), [], "{theme:?}: clicked, no ring");
            assert_eq!(h.state().text, "Order_Items", "{theme:?}: kept as typed");
            let words = |shapes: &[ClippedShape]| texts(shapes).into_iter().find(|(w, _, _)| w == "Order_Items").map(|(_, r, _)| r);
            let clicked = words(&shapes);
            assert!(clicked.is_some(), "{theme:?}: drawn as typed: {:?}", texts(&shapes));
            assert!(!texts(&shapes).iter().any(|(w, _, _)| w == "ORDER_ITEMS"), "{theme:?}: never in capitals");
            h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
            h.run();
            assert!(!h.state().focus, "{theme:?}: Shift+Tab took the focus away");
            h.key_press(Key::Tab);
            h.run();
            assert!(h.state().focus, "{theme:?}: Tab gave it back");
            let shapes = h.output().shapes.clone();
            assert_eq!(edges_in(&shapes, field, 0.0), [ring], "{theme:?}: tabbed to, the ring");
            assert_eq!((words(&shapes), h.state().field), (clicked, field), "{theme:?}: nothing moved");
            // The refresh.
            h.get_by_label("refresh").click();
            h.run();
            assert_eq!(h.state().asked, 1, "{theme:?}: idle, a click asks");
            // The spinner keeps asking for frames: steps, not runs.
            for refresh in [Refresh::Running, Refresh::Off] {
                h.state_mut().refresh = refresh;
                h.run_steps(2);
                h.get_by_label("refresh").click();
                h.run_steps(2);
                assert_eq!(h.state().asked, 1, "{theme:?} {refresh:?}: a click asks nothing");
            }
        }
    }
}

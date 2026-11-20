//! The quick search's parts (QC with R4's rows): the box ([`frame`]), its field ([`Field`]), and
//! what its list holds: a heading over a connection's group ([`Heading`]), a hit ([`Row`]) and a
//! note in place of the list ([`Note`]). Each part is drawn from plain data into the rect the
//! window gives it; the window ([`crate::quick_search`]) keeps the state, the keys, the scrolling
//! and the tags.
//!
//! Headings and rows are one height, so the list scrolls by whole items. Names keep their case:
//! a schema, a table and a connection's name are drawn as written; only a heading's words are
//! drawn in capitals (`label`). A connection's colour is a square mark, outlined in `text_muted`
//! when it has none; no words are drawn on it.

use dbee_core::RelKind;
use egui::epaint::RectShape;
use egui::{Align, Color32, CornerRadius, Frame, Galley, Id, Margin, Painter, Rect, Response, Shape, Stroke, StrokeKind, TextBuffer, TextEdit, Ui};
use egui::{Vec2, WidgetInfo, WidgetType, pos2, vec2};
use std::sync::Arc;

use super::grid_cell::one_line;
use super::icon::Icon;
use super::rows::relation_icon;
use super::{focus, painter};
use crate::theme::size::{
    QS_END_PAD, QS_FIELD_H, QS_HEADING_LINE_Y, QS_HEADING_MARK, QS_HEADING_MARK_GAP, QS_HEADING_X, QS_ICON_X, QS_KIND_GAP, QS_MARK, QS_PAD,
    QS_SCHEMA_SHARE, QS_TEXT_X, RADIUS_CONTROL, RADIUS_FLOAT,
};
use crate::theme::{Text, Tokens, float_shadow, tokens};

/// The box: on `raised` with the one shadow and no edge, corners of the floating radius, its
/// content `QS_PAD` inside.
pub fn frame(t: &Tokens) -> Frame {
    Frame::new().fill(t.raised).corner_radius(CornerRadius::same(RADIUS_FLOAT)).inner_margin(Margin::same(QS_PAD)).shadow(float_shadow(t))
}

/// The search field: `QS_FIELD_H` tall on `card`, the search icon on the rows' icon column, the
/// query in `subtitle` from their text column. Its edge is 2 px `ink` while it has the keyboard,
/// else 1 px `line_strong`.
pub struct Field<'a> {
    pub text: &'a mut String,
    /// Shown in `text_muted` while nothing is typed.
    pub placeholder: &'a str,
    /// The text edit's egui id, for asking for its focus.
    pub id: Id,
}

impl Field<'_> {
    /// Draws it `width` wide; the text edit's response.
    pub fn show(self, ui: &mut Ui, width: f32) -> Response {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        // The ground goes under the words, which egui's text edit draws.
        let ground = p.add(Shape::Noop);
        let colour = t.text;
        let mut layouter = move |ui: &Ui, text: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> {
            ui.fonts_mut(|f| f.layout_job(Text::Subtitle.job_as_written(text.as_str(), colour)))
        };
        let margin = Margin { left: QS_TEXT_X as i8, right: QS_END_PAD as i8, top: 0, bottom: 0 };
        let edit = TextEdit::singleline(self.text)
            .id(self.id)
            .font(Text::Subtitle.font())
            .text_color(colour)
            .frame(Frame::NONE.inner_margin(margin))
            .desired_width(width)
            .min_size(vec2(width, QS_FIELD_H))
            .vertical_align(Align::Center)
            .hint_text(Text::Subtitle.rich_as_written(self.placeholder))
            .layouter(&mut layouter);
        // egui colours the placeholder with its weak text colour.
        let weak = ui.visuals().weak_text_color;
        ui.visuals_mut().weak_text_color = Some(t.text_muted);
        let r = ui.add(edit);
        ui.visuals_mut().weak_text_color = weak;
        let rect = r.rect;
        let radius = CornerRadius::same(RADIUS_CONTROL);
        p.set(ground, RectShape::filled(rect, radius, t.card));
        if r.has_focus() {
            focus::own_edge(&p, rect, RADIUS_CONTROL);
        } else {
            p.rect_stroke(rect, radius, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        }
        Icon::Search.paint(&p, pos2(rect.left() + QS_ICON_X, rect.center().y), t.text_muted);
        r
    }
}

/// A heading over a group of the list, as tall as a row; never highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading<'a> {
    /// The open connection's group of tables: its colour's mark, then its name.
    Connection { name: &'a str, colour: Option<Color32> },
    /// The saved connections.
    Others,
}

impl Heading<'_> {
    /// Its words as written, before the capitals they are drawn in.
    pub fn words(&self) -> String {
        match self {
            Heading::Connection { name, .. } => (*name).to_owned(),
            Heading::Others => "Other connections".to_owned(),
        }
    }

    /// Draws it into `rect`: the words in `label` capitals, `text_muted`, after the mark. When
    /// they were cut: the whole words, as written, for the window to show on hover.
    pub fn paint(&self, ui: &Ui, rect: Rect) -> Option<String> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let line = rect.top() + QS_HEADING_LINE_Y;
        let mut left = rect.left() + QS_HEADING_X;
        if let Heading::Connection { colour, .. } = self {
            let at = Rect::from_min_size(pos2(left, line - QS_HEADING_MARK / 2.0), Vec2::splat(QS_HEADING_MARK));
            mark(&p, t, at, *colour);
            left = at.right() + QS_HEADING_MARK_GAP;
        }
        let words = self.words();
        let mut job = Text::Label.job(&words, t.text_muted);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - QS_END_PAD - left).max(1.0));
        let galley = ui.painter().layout_job(job);
        let cut = galley.elided;
        p.galley(pos2(left, line - galley.size().y / 2.0), galley, t.text_muted);
        cut.then_some(words)
    }
}

/// What a row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit<'a> {
    Table {
        schema: &'a str,
        name: &'a str,
        kind: RelKind,
    },
    /// A saved connection: its name, its database type's label, its colour.
    Connection {
        name: &'a str,
        label: &'a str,
        colour: Option<Color32>,
    },
}

/// A row of the list: a table or a connection.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub hit: Hit<'a>,
    /// The keyboard's row.
    pub highlighted: bool,
    /// Under the pointer.
    pub hovered: bool,
}

impl Row<'_> {
    /// Draws it into `rect`. When its words were cut: the whole `schema.name`, or the
    /// connection's whole name, for the window to show on hover.
    pub fn paint(&self, ui: &Ui, rect: Rect) -> Option<String> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        if self.highlighted || self.hovered {
            p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), t.raised_hover);
        }
        let y = rect.center().y;
        let icon = pos2(rect.left() + QS_ICON_X, y);
        let left = rect.left() + QS_TEXT_X;
        let end = rect.right() - QS_END_PAD;
        let room = (end - left).max(0.0);
        match self.hit {
            Hit::Table { schema, name, kind } => {
                relation_icon(kind).paint(&p, icon, t.text_muted);
                let prefix = one_line(ui, &format!("{schema}."), Text::Data, t.text_muted, room * QS_SCHEMA_SHARE);
                let prefix_w = prefix.size().x;
                let named = one_line(ui, name, Text::Data, t.text, room - prefix_w);
                let cut = prefix.elided || named.elided;
                p.galley(pos2(left, y - prefix.size().y / 2.0), prefix, t.text_muted);
                p.galley(pos2(left + prefix_w, y - named.size().y / 2.0), named, t.text);
                cut.then(|| format!("{schema}.{name}"))
            }
            Hit::Connection { name, label, colour } => {
                mark(&p, t, Rect::from_center_size(icon, Vec2::splat(QS_MARK)), colour);
                let kind = one_line(ui, label, Text::Body, t.text_muted, f32::INFINITY);
                let kind_x = end - kind.size().x;
                let named = one_line(ui, name, Text::Body, t.text, kind_x - QS_KIND_GAP - left);
                let cut = named.elided;
                p.galley(pos2(left, y - named.size().y / 2.0), named, t.text);
                p.galley(pos2(kind_x, y - kind.size().y / 2.0), kind, t.text_muted);
                cut.then(|| name.to_owned())
            }
        }
    }

    /// What it reads out: its value as written (`schema.name`, or a connection's name and its
    /// database type's label), and whether it is the keyboard's row.
    pub fn widget_info(&self) -> WidgetInfo {
        let label = match self.hit {
            Hit::Table { schema, name, .. } => format!("{schema}.{name}"),
            Hit::Connection { name, label, .. } => format!("{name}, {label}"),
        };
        WidgetInfo::selected(WidgetType::SelectableLabel, true, self.highlighted, label)
    }
}

/// A note in place of the list, one row tall, never highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note<'a> {
    /// Nothing matches what is typed.
    NoMatch,
    /// The search failed: the engine's message.
    Error(&'a str),
}

impl Note<'_> {
    /// Its words.
    pub fn words(&self) -> String {
        match self {
            Note::NoMatch => "No match".to_owned(),
            Note::Error(message) => (*message).to_owned(),
        }
    }

    /// Draws it into `rect`, in `body`, from the rows' text column: `text_muted`, or `danger` for
    /// an error. When cut: the whole words, for the window to show on hover.
    pub fn paint(&self, ui: &Ui, rect: Rect) -> Option<String> {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let colour = match self {
            Note::NoMatch => t.text_muted,
            Note::Error(_) => t.danger,
        };
        let left = rect.left() + QS_TEXT_X;
        let words = self.words();
        let galley = one_line(ui, &words, Text::Body, colour, rect.right() - QS_END_PAD - left);
        let cut = galley.elided;
        p.galley(pos2(left, rect.center().y - galley.size().y / 2.0), galley, colour);
        cut.then_some(words)
    }
}

/// A connection's mark, the square `at`: filled with its colour, or outlined 1 px `text_muted`
/// inside with none. Square corners.
fn mark(p: &Painter, t: &Tokens, at: Rect, colour: Option<Color32>) {
    match colour {
        Some(c) => {
            p.rect_filled(at, CornerRadius::ZERO, c);
        }
        None => {
            p.rect_stroke(at, CornerRadius::ZERO, Stroke::new(1.0, t.text_muted), StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::ClippedShape;
    use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Shape, Theme, Ui, Vec2, pos2, vec2};

    use super::*;
    use crate::components::icon::Icon;
    use crate::components::{Edge, ROOM, both, edges, edges_in, fills, fills_in, flat, harness_in, icon_in, painted, painted_in, sections, texts};
    use crate::theme::{self, Text};

    /// The box's inner width at 1100.
    const W: f32 = 616.0;

    fn close(a: f32, b: f32, by: f32) -> bool {
        (a - b).abs() <= by
    }

    /// One frame of `paint` drawing into a rect of `size`: the shapes and the rect.
    fn drawn(theme: Theme, size: Vec2, mut paint: impl FnMut(&Ui, Rect)) -> (Vec<ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted_in(vec2(800.0, 200.0), theme, |ui| {
            ui.add_space(ROOM);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            at = rect;
            paint(ui, rect);
        });
        (shapes, at)
    }

    /// The words drawn within `r`, each with where and in what colour.
    fn words_in(shapes: &[ClippedShape], r: Rect) -> Vec<(String, Rect, Color32)> {
        texts(shapes).into_iter().filter(|(w, at, _)| !w.is_empty() && r.contains_rect(*at)).collect()
    }

    /// The texts egui cut with `…`.
    fn cut(shapes: &[ClippedShape]) -> Vec<String> {
        flat(shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) if t.galley.elided => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn table<'a>(schema: &'a str, name: &'a str, kind: RelKind) -> Row<'a> {
        Row { hit: Hit::Table { schema, name, kind }, highlighted: false, hovered: false }
    }

    fn connection<'a>(name: &'a str, label: &'a str, colour: Option<Color32>) -> Row<'a> {
        Row { hit: Hit::Connection { name, label, colour }, highlighted: false, hovered: false }
    }

    /// `icon` drawn alone in `colour`, centred on `centre`: its colours and the box it fills.
    fn alone(theme: Theme, icon: Icon, centre: Pos2, colour: Color32) -> (Vec<Color32>, Rect) {
        icon_in(&painted(theme, |ui| icon.paint(ui.painter(), centre, colour)), Rect::EVERYTHING)
    }

    #[test]
    fn the_box_has_no_edge_and_the_shadow() {
        // QC: `raised`, no edge (foundations: "none on quick search"), the one shadow, 12 inside.
        for (theme, t) in both() {
            let mut child = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                frame(&t).show(ui, |ui| child = ui.allocate_exact_size(vec2(W, 100.0), Sense::hover()).0);
            });
            let outer = child.expand(12.0);
            assert!(fills(&shapes).contains(&(outer, t.raised)), "{theme:?}: the box on raised: {:?}", fills(&shapes));
            assert_eq!(edges(&shapes), [], "{theme:?}: no edge");
            let shadow = theme::float_shadow(&t);
            let blurred: Vec<_> = flat(&shapes)
                .into_iter()
                .filter_map(|s| match s {
                    Shape::Rect(r) if r.blur_width > 0.0 => Some((r.rect, r.fill, r.blur_width)),
                    _ => None,
                })
                .collect();
            let offset = vec2(f32::from(shadow.offset[0]), f32::from(shadow.offset[1]));
            assert_eq!(blurred, [(outer.translate(offset), shadow.color, f32::from(shadow.blur))], "{theme:?}: the one shadow");
            // The harness's own ground lies under it, the size of what it holds.
            let corners: Vec<_> = flat(&shapes)
                .into_iter()
                .filter_map(|s| match s {
                    Shape::Rect(r) if r.rect == outer && r.fill == t.raised => Some(r.corner_radius),
                    _ => None,
                })
                .collect();
            assert!(corners.contains(&CornerRadius::same(4)), "{theme:?}: the box's corners 4: {corners:?}");
        }
    }

    /// What the field test draws: the field's text, whether it should have focus, its rect.
    struct Probe {
        text: String,
        focus: bool,
        rect: Rect,
    }

    #[test]
    fn the_field_matches_the_screen() {
        for (theme, t) in both() {
            let mut h =
                harness_in(vec2(700.0, 120.0), theme, Probe { text: String::new(), focus: true, rect: Rect::NOTHING }, |ui, s: &mut Probe| {
                    ui.add_space(ROOM);
                    let r = Field { text: &mut s.text, placeholder: "Find a table or connection", id: Id::new("qs-field-test") }.show(ui, W);
                    s.rect = r.rect;
                    if s.focus && !r.has_focus() {
                        r.request_focus();
                    } else if !s.focus && r.has_focus() {
                        r.surrender_focus();
                        ui.ctx().request_repaint();
                    }
                });
            h.run();
            let r = h.state().rect;
            assert!(close(r.width(), W, 0.01) && close(r.height(), 40.0, 0.01), "{theme:?}: 616 by 40: {r:?}");
            let ink = Edge { outer: r, width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&h.output().shapes, r, 0.0), [ink], "{theme:?}: focused, 2 px ink inside");
            assert!(fills_in(&h.output().shapes, r).contains(&(r, t.card)), "{theme:?}: on card");
            h.state_mut().focus = false;
            h.run();
            let shapes = h.output().shapes.clone();
            let line = Edge { outer: r, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
            assert_eq!(edges_in(&shapes, r, 0.0), [line], "{theme:?}: without focus, 1 px line_strong");
            let slot = Rect::from_min_max(r.min, pos2(r.left() + 30.0, r.bottom()));
            assert_eq!(
                icon_in(&shapes, slot),
                alone(theme, Icon::Search, pos2(r.left() + 15.0, r.center().y), t.text_muted),
                "{theme:?}: the search icon centred 15 in"
            );
            let words = words_in(&shapes, r);
            let Some((_, at, colour)) = words.iter().find(|(w, ..)| w == "Find a table or connection") else {
                panic!("{theme:?}: the placeholder: {words:?}")
            };
            assert_eq!(*colour, t.text_muted, "{theme:?}: the placeholder in text_muted");
            assert!(at.left() >= r.left() + 29.0 && close(at.center().y, r.center().y, 1.0), "{theme:?}: from 30 in, centred: {at:?}");
            let fonts = sections(&shapes, "Find a table or connection");
            assert_eq!(fonts.first().map(|f| f.1.size), Some(14.0), "{theme:?}: in subtitle's size");
        }
    }

    #[test]
    fn the_typed_query_is_drawn_from_the_rows_text_column() {
        let mut h =
            harness_in(vec2(700.0, 120.0), Theme::Light, Probe { text: "ord".into(), focus: false, rect: Rect::NOTHING }, |ui, s: &mut Probe| {
                ui.add_space(ROOM);
                s.rect = Field { text: &mut s.text, placeholder: "Find a table or connection", id: Id::new("qs-field-test") }.show(ui, W).rect;
            });
        h.run();
        let r = h.state().rect;
        let words = words_in(&h.output().shapes, r);
        let Some((_, at, colour)) = words.iter().find(|(w, ..)| w == "ord") else { panic!("the query: {words:?}") };
        assert_eq!(*colour, theme::LIGHT.text, "the query in text");
        assert!(close(at.left(), r.left() + 30.0, 1.0), "the query from 30 in: {at:?}");
        assert_eq!(sections(&h.output().shapes, "ord").first().map(|f| f.1.clone()), Some(Text::Subtitle.font()), "in subtitle");
    }

    #[test]
    fn each_row_matches_the_screen() {
        for (theme, t) in both() {
            // A highlighted table row: raised_hover across it; the kind's icon at 15; `shop.` grey
            // and `orders` in text, both in data, from 30.
            let row = Row { highlighted: true, ..table("shop", "orders", RelKind::Table) };
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                row.paint(ui, rect);
            });
            let slot = Rect::from_center_size(pos2(r.left() + 15.0, r.center().y), Vec2::splat(16.0));
            let grounds: Vec<_> = fills_in(&shapes, r).into_iter().filter(|(at, _)| !slot.contains_rect(*at)).collect();
            assert_eq!(grounds, [(r, t.raised_hover)], "{theme:?}: the highlight across the row");
            assert_eq!(
                icon_in(&shapes, slot),
                alone(theme, Icon::Table, slot.center(), t.text_muted),
                "{theme:?}: the table's icon centred 15 in, text_muted"
            );
            let words = words_in(&shapes, r);
            let [(schema, s, sc), (name, n, nc)] = words.as_slice() else { panic!("{theme:?}: schema and name: {words:?}") };
            assert_eq!((schema.as_str(), *sc, name.as_str(), *nc), ("shop.", t.text_muted, "orders", t.text), "{theme:?}");
            assert!(close(s.left(), r.left() + 30.0, 1.0) && close(n.left(), s.right(), 1.0), "{theme:?}: from 30, one run: {s:?} {n:?}");
            assert!(close(s.center().y, r.center().y, 1.0), "{theme:?}: centred: {s:?}");
            for part in ["shop.", "orders"] {
                assert_eq!(sections(&shapes, part).first().map(|f| f.1.clone()), Some(Text::Data.font()), "{theme:?}: `{part}` in data");
            }
            // A connection: its 8 square at 15, the name from 30, the label ending 8 from the right.
            let blue = t.conn_blue;
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                connection("Orders replica", "PostgreSQL", Some(blue)).paint(ui, rect);
            });
            let mark = Rect::from_center_size(pos2(r.left() + 15.0, r.center().y), Vec2::splat(8.0));
            assert_eq!(fills_in(&shapes, r), [(mark, blue)], "{theme:?}: the 8 square, nothing else filled");
            let words = words_in(&shapes, r);
            let [(name, n, nc), (label, l, lc)] = words.as_slice() else { panic!("{theme:?}: name and label: {words:?}") };
            assert_eq!((name.as_str(), *nc, label.as_str(), *lc), ("Orders replica", t.text, "PostgreSQL", t.text_muted), "{theme:?}");
            assert!(close(n.left(), r.left() + 30.0, 1.0) && close(l.right(), r.right() - 8.0, 1.0), "{theme:?}: {n:?} {l:?}");
            for part in ["Orders replica", "PostgreSQL"] {
                assert_eq!(sections(&shapes, part).first().map(|f| f.1.clone()), Some(Text::Body.font()), "{theme:?}: `{part}` in body");
            }
            // Hovered and highlighted: the same ground, once.
            let row = Row { highlighted: true, hovered: true, ..connection("Orders replica", "PostgreSQL", Some(blue)) };
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                row.paint(ui, rect);
            });
            let grounds: Vec<_> = fills_in(&shapes, r).into_iter().filter(|(_, c)| *c == t.raised_hover).collect();
            assert_eq!(grounds, [(r, t.raised_hover)], "{theme:?}: one ground");
            // Hovered alone: the same ground.
            let row = Row { hovered: true, ..table("shop", "orders", RelKind::View) };
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                row.paint(ui, rect);
            });
            assert!(fills_in(&shapes, r).contains(&(r, t.raised_hover)), "{theme:?}: hovered");
        }
    }

    #[test]
    fn marks_are_squares_never_circles() {
        for (theme, t) in both() {
            let red = t.conn_red;
            for colour in [Some(red), None] {
                let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                    connection("Shop live", "PostgreSQL", colour).paint(ui, rect);
                });
                assert!(!flat(&shapes).iter().any(|s| matches!(s, Shape::Circle(_))), "{theme:?} {colour:?}: no circle in a row");
                let mark = Rect::from_center_size(pos2(r.left() + 15.0, r.center().y), Vec2::splat(8.0));
                match colour {
                    Some(c) => assert_eq!(fills_in(&shapes, r), [(mark, c)], "{theme:?}: a filled 8 square"),
                    None => {
                        assert_eq!(fills_in(&shapes, r), [], "{theme:?}: nothing filled");
                        let line = Edge { outer: mark, width: 1.0, colour: t.text_muted, radius: CornerRadius::ZERO };
                        assert_eq!(edges_in(&shapes, r, 0.0), [line], "{theme:?}: an outlined 8 square");
                    }
                }
                let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                    Heading::Connection { name: "Shop live", colour }.paint(ui, rect);
                });
                assert!(!flat(&shapes).iter().any(|s| matches!(s, Shape::Circle(_))), "{theme:?} {colour:?}: no circle in a heading");
                let mark = Rect::from_min_size(pos2(r.left() + 8.0, r.top() + 19.0 - 3.0), Vec2::splat(6.0));
                match colour {
                    Some(c) => assert_eq!(fills_in(&shapes, r), [(mark, c)], "{theme:?}: a filled 6 square"),
                    None => {
                        assert_eq!(fills_in(&shapes, r), [], "{theme:?}: nothing filled");
                        let line = Edge { outer: mark, width: 1.0, colour: t.text_muted, radius: CornerRadius::ZERO };
                        assert_eq!(edges_in(&shapes, r, 0.0), [line], "{theme:?}: an outlined 6 square");
                    }
                }
            }
        }
    }

    #[test]
    fn a_heading_draws_the_connection_name_in_caps_and_others_without_a_mark() {
        for (theme, t) in both() {
            let red = t.conn_red;
            let heading = Heading::Connection { name: "Shop live", colour: Some(red) };
            assert_eq!(heading.words(), "Shop live", "the words as written");
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                assert_eq!(heading.paint(ui, rect), None, "nothing cut");
            });
            let words = words_in(&shapes, r);
            let [(w, at, c)] = words.as_slice() else { panic!("{theme:?}: one line of words: {words:?}") };
            assert_eq!((w.as_str(), *c), ("SHOP LIVE", t.text_muted), "{theme:?}");
            assert!(close(at.left(), r.left() + 8.0 + 6.0 + 10.0, 1.0), "{theme:?}: 10 after the mark: {at:?}");
            assert!(close(at.center().y, r.top() + 19.0, 1.0), "{theme:?}: centred 19 down: {at:?}");
            assert_eq!(sections(&shapes, "SHOP LIVE").first().map(|f| f.1.clone()), Some(Text::Label.font()), "{theme:?}: in label");
            let mark = Rect::from_min_size(pos2(r.left() + 8.0, r.top() + 16.0), Vec2::splat(6.0));
            assert_eq!(fills_in(&shapes, r), [(mark, red)], "{theme:?}: the 6 square 8 in");

            assert_eq!(Heading::Others.words(), "Other connections");
            let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                Heading::Others.paint(ui, rect);
            });
            let words = words_in(&shapes, r);
            let [(w, at, c)] = words.as_slice() else { panic!("{theme:?}: one line of words: {words:?}") };
            assert_eq!((w.as_str(), *c), ("OTHER CONNECTIONS", t.text_muted), "{theme:?}");
            assert!(close(at.left(), r.left() + 8.0, 1.0) && close(at.center().y, r.top() + 19.0, 1.0), "{theme:?}: 8 in: {at:?}");
            assert_eq!((fills_in(&shapes, r), edges_in(&shapes, r, 0.0)), (vec![], vec![]), "{theme:?}: no mark");
        }
    }

    #[test]
    fn names_keep_their_case() {
        let words = |row: Row<'_>| {
            let (shapes, r) = drawn(Theme::Light, vec2(W, 32.0), |ui, rect| {
                row.paint(ui, rect);
            });
            words_in(&shapes, r).into_iter().map(|(w, ..)| w).collect::<Vec<_>>()
        };
        assert_eq!(words(table("Mixed", "Orders", RelKind::Table)), ["Mixed.", "Orders"]);
        assert_eq!(words(connection("Shop (Live)", "MySQL", None)), ["Shop (Live)", "MySQL"]);
        let row = connection("Shop (Live)", "MySQL", None);
        assert_eq!(row.widget_info().label.as_deref(), Some("Shop (Live), MySQL"), "read out as written, with its label");
        assert_eq!(table("Mixed", "Orders", RelKind::Table).widget_info().label.as_deref(), Some("Mixed.Orders"));
    }

    #[test]
    fn cut_rows_say_their_whole_name() {
        let said = |row: Row<'_>, width: f32| {
            let mut said = None;
            let (shapes, r) = drawn(Theme::Light, vec2(width, 32.0), |ui, rect| said = row.paint(ui, rect));
            (said, cut(&shapes), words_in(&shapes, r))
        };
        const LONG: &str = "order_totals_by_customer_and_month";
        let (whole, cut_texts, words) = said(table("reporting", LONG, RelKind::MaterializedView), 200.0);
        assert_eq!(whole, Some(format!("reporting.{LONG}")), "the whole value");
        assert!(!cut_texts.is_empty(), "drawn with …: {words:?}");
        for (_, at, _) in &words {
            assert!(at.right() <= 10.0 + 200.0, "inside the row: {at:?}");
        }
        assert_eq!(said(table("shop", "orders", RelKind::Table), 200.0).0, None, "a short one");
        const NAME: &str = "Reporting replica for the Customer Success team";
        let (whole, cut_texts, words) = said(connection(NAME, "PostgreSQL", None), 200.0);
        assert_eq!(whole, Some(NAME.to_owned()), "the whole name");
        assert_eq!(cut_texts, [NAME], "the name cut, the label whole: {words:?}");
        assert!(words.iter().any(|(w, ..)| w == "PostgreSQL"), "the label drawn: {words:?}");
        let name = words.iter().find(|(w, ..)| w == NAME).map(|(_, at, _)| *at);
        let label = words.iter().find(|(w, ..)| w == "PostgreSQL").map(|(_, at, _)| *at);
        if let (Some(n), Some(l)) = (name, label) {
            assert!(n.right() <= l.left() - 12.0 + 1.0, "the name cut 12 before the label: {n:?} {l:?}");
        }
    }

    #[test]
    fn the_notes() {
        for (theme, t) in both() {
            for (note, words, colour) in
                [(Note::NoMatch, "No match", t.text_muted), (Note::Error("The search gave no list."), "The search gave no list.", t.danger)]
            {
                assert_eq!(note.words(), words);
                let (shapes, r) = drawn(theme, vec2(W, 32.0), |ui, rect| {
                    assert_eq!(note.paint(ui, rect), None, "whole");
                });
                let drawn = words_in(&shapes, r);
                let [(w, at, c)] = drawn.as_slice() else { panic!("{theme:?}: one line: {drawn:?}") };
                assert_eq!((w.as_str(), *c), (words, colour), "{theme:?}");
                assert!(close(at.left(), r.left() + 30.0, 1.0) && close(at.center().y, r.center().y, 1.0), "{theme:?}: from 30, centred: {at:?}");
                assert_eq!(sections(&shapes, words).first().map(|f| f.1.clone()), Some(Text::Body.font()), "{theme:?}: in body");
                assert_eq!(fills_in(&shapes, r), [], "{theme:?}: never highlighted");
            }
            let long = "The search failed because the catalog could not be read from the server at all";
            let mut said = None;
            drawn(theme, vec2(200.0, 32.0), |ui, rect| said = Note::Error(long).paint(ui, rect));
            assert_eq!(said, Some(long.to_owned()), "{theme:?}: a cut error says itself whole");
        }
    }
}

//! The error band (`error band`): what stopped a run or a page load, across the width above the
//! results. On the `deleted` tint with a 2 px `danger` edge down its left, what it holds 12 in
//! from its sides past that edge: the heading in `subtitle` `danger` and, at the right end of its
//! line, where the error is (`Line 1, column 14 →`, `small` `text_muted`); under it the message in
//! `body` `text`; then each further field the server gives, its name in `label` capitals and
//! `text_muted` over its text in `body` `text`, the detail before the hint (Postgres's and psql's
//! order); then dbee's note in `small` `text_muted`. Its parts are 4 apart, a field and the note 8
//! more. Last, its actions (R2): a left-aligned row of outline `button/box` buttons 12 under the
//! last text, 8 apart (ROLLBACK on a failed transaction).
//!
//! The position takes room from the heading only while the heading keeps at least
//! [`ERROR_BAND_HEADING_MIN`]; with less it isn't drawn, and the band still jumps on a click. A
//! cancel dbee sent has no band (the result bar says it), so the band has no neutral state.
//!
//! A band with a position is a link to it: a click anywhere on it (but its actions), or Enter or
//! Space with the keyboard on it, asks to jump there, and its words can't be selected. Without
//! one, they can.

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, CornerRadius, CursorIcon, Id, Label, Layout, Rect, Response, Sense, Shape, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use super::button::BoxButton;
use super::tooltip::Hint;
use super::{focus, painter};
use crate::theme::size::{
    BUTTON_H, ERROR_BAND_ACTION_GAP, ERROR_BAND_ACTIONS_GAP, ERROR_BAND_EDGE, ERROR_BAND_FIELD_GAP, ERROR_BAND_GAP, ERROR_BAND_HEAD_GAP,
    ERROR_BAND_HEADING_MIN, ERROR_BAND_NAME_GAP, ERROR_BAND_PAD,
};
use crate::theme::{Text, tokens};

/// One button of a band's actions row: its words (drawn in capitals), whether it is on, and its
/// hover (why it is off, when it is).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Action<'a> {
    pub label: &'a str,
    pub enabled: bool,
    pub hover: &'a str,
}

/// One band: its heading (dbee's words), where the error is (line and column, 1-based), the
/// message, detail and hint as the server wrote them, dbee's note under them, and its actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Band<'a> {
    pub heading: &'a str,
    pub position: Option<(usize, usize)>,
    pub message: &'a str,
    pub detail: Option<&'a str>,
    pub hint: Option<&'a str>,
    pub note: Option<&'a str>,
    pub actions: &'a [Action<'a>],
}

/// What a band drew: the band (clicked when it asks to jump to the position), and each action's
/// button, in order.
#[derive(Debug)]
pub struct Shown {
    pub response: Response,
    pub actions: Vec<Response>,
}

impl Shown {
    /// The index of the action clicked in this frame.
    pub fn clicked(&self) -> Option<usize> {
        self.actions.iter().position(Response::clicked)
    }
}

/// The position's words: `Line 1, column 14 →`.
pub fn position_words(line: usize, column: usize) -> String {
    format!("Line {line}, column {column} →")
}

impl Band<'_> {
    /// Draws the band across what is left of `ui`'s width, as tall as what it holds. Its response
    /// is clicked when it asks to jump to the position (only a band with one).
    pub fn show(&self, ui: &mut Ui, id: Id) -> Shown {
        let t = tokens(ui.ctx());
        let (ground, edge, heading_ink, message_ink) = (t.deleted, t.danger, t.danger, t.text);
        let jump = self.position.is_some();
        let outer = ui.available_rect_before_wrap();
        let left = outer.left() + ERROR_BAND_EDGE + ERROR_BAND_PAD;
        let inner = Rect::from_min_max(pos2(left, outer.top() + ERROR_BAND_PAD), pos2((outer.right() - ERROR_BAND_PAD).max(left), outer.bottom()));
        let p = painter(ui);
        let ground_at = p.add(Shape::Noop);
        let mut body = ui.new_child(UiBuilder::new().id_salt(id.with("band")).max_rect(inner).layout(Layout::top_down(Align::Min)));
        body.spacing_mut().item_spacing = Vec2::ZERO;
        // The head: the heading, and where the error is at the right end of its line.
        let (head, _) = body.allocate_exact_size(vec2(inner.width(), Text::Subtitle.line()), Sense::hover());
        let link = self.position.map(|(line, column)| {
            let words = position_words(line, column);
            let galley = body.painter().layout_job(Text::Small.job_as_written(&words, t.text_muted));
            (words, galley)
        });
        // The position only while the heading keeps its least room (gap 68).
        let link_room = link.as_ref().map_or(0.0, |(_, g)| g.size().x + ERROR_BAND_HEAD_GAP);
        let link_drawn = link.is_some() && head.width() - link_room >= ERROR_BAND_HEADING_MIN;
        let link_room = if link_drawn { link_room } else { 0.0 };
        let heading = cut(Text::Subtitle.job_as_written(self.heading, heading_ink), head.width() - link_room);
        let heading = body.painter().layout_job(heading);
        let heading_at = Rect::from_min_size(head.min, heading.size());
        p.galley(heading_at.min, heading, heading_ink);
        body.interact(heading_at, id.with("heading"), Sense::hover()).widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, self.heading));
        if let Some((_, galley)) = link.as_ref().filter(|_| link_drawn) {
            p.galley(pos2(head.right() - galley.size().x, head.center().y - galley.size().y / 2.0), std::sync::Arc::clone(galley), t.text_muted);
        }
        // The message, the fields and the note: selectable words, unless a click jumps.
        let words = |ui: &mut Ui, text: &str, style: Text, ink| {
            ui.add(Label::new(style.rich_as_written(text).color(ink)).wrap().selectable(!jump));
        };
        body.add_space(ERROR_BAND_GAP);
        words(&mut body, self.message, Text::Body, message_ink);
        for (name, text) in [("Detail", self.detail), ("Hint", self.hint)] {
            let Some(text) = text else { continue };
            body.add_space(ERROR_BAND_GAP + ERROR_BAND_FIELD_GAP);
            let galley = body.painter().layout_job(Text::Label.job(name, t.text_muted));
            let (at, _) = body.allocate_exact_size(galley.size(), Sense::hover());
            p.galley(at.min, galley, t.text_muted);
            body.interact(at, id.with(name), Sense::hover()).widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, name));
            body.add_space(ERROR_BAND_NAME_GAP);
            words(&mut body, text, Text::Body, t.text);
        }
        if let Some(note) = self.note {
            body.add_space(ERROR_BAND_GAP + ERROR_BAND_FIELD_GAP);
            words(&mut body, note, Text::Small, t.text_muted);
        }
        // The actions' row: its room now, its buttons once the band's own click is placed under
        // them.
        let row = (!self.actions.is_empty()).then(|| {
            body.add_space(ERROR_BAND_ACTIONS_GAP);
            let widths: Vec<f32> = self.actions.iter().map(|a| BoxButton::outline(a.label).width(&body)).collect();
            let gaps = ERROR_BAND_ACTION_GAP * widths.len().saturating_sub(1) as f32;
            let (row, _) = body.allocate_exact_size(vec2(widths.iter().sum::<f32>() + gaps, BUTTON_H), Sense::hover());
            (row, widths)
        });
        let rect = Rect::from_min_max(outer.min, pos2(outer.right(), body.min_rect().bottom() + ERROR_BAND_PAD));
        ui.advance_cursor_after_rect(rect);
        let edge_rect = Rect::from_min_max(rect.min, pos2(rect.left() + ERROR_BAND_EDGE, rect.bottom()));
        p.set(
            ground_at,
            Shape::Vec(vec![Shape::rect_filled(rect, CornerRadius::ZERO, ground), Shape::rect_filled(edge_rect, CornerRadius::ZERO, edge)]),
        );
        let r = ui.interact(rect, id, if jump { Sense::click() } else { Sense::hover() });
        let mut actions = Vec::new();
        if let Some((row, widths)) = row {
            let mut x = row.left();
            for (action, w) in self.actions.iter().zip(widths) {
                let at = Rect::from_min_size(pos2(x, row.top()), vec2(w, BUTTON_H));
                let mut child =
                    ui.new_child(UiBuilder::new().id_salt(id.with(("action", action.label))).max_rect(at).layout(Layout::left_to_right(Align::Min)));
                let b = child.add_enabled(action.enabled, BoxButton::outline(action.label));
                actions.push(if action.enabled { b.hint(action.hover) } else { b.hint_disabled(action.hover) });
                x += w + ERROR_BAND_ACTION_GAP;
            }
        }
        if let Some((words, _)) = link {
            r.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), &words));
            if r.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if focus::shown(ui, &r) {
                focus::own_edge(&p, rect, 0);
            }
        }
        Shown { response: r, actions }
    }
}

/// `job` on one line, cut with `…` to fit `width`.
fn cut(mut job: LayoutJob, width: f32) -> LayoutJob {
    job.wrap = TextWrapping::truncate_at_width(width.max(0.0));
    job
}

#[cfg(test)]
mod tests {
    use dbee_engine::{QueryError, TabId, TableTab};
    use egui::epaint::ClippedShape;
    use egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, Rect, Theme, UiBuilder, pos2};
    use serde_json::json;

    use super::*;
    use crate::automation::Tags;
    use crate::components::showcase::{APP_BANDS, PAGE_BANDS};
    use crate::components::{both, edges, fills, harness_in, painted_in, sections, texts};
    use crate::theme::Text;

    /// The page's band is this wide; the bands drawn here start at [`AT`], [`APART`] apart.
    const W: f32 = 840.0;
    const AT: Pos2 = pos2(10.0, 10.0);
    const APART: f32 = 12.0;

    /// `bands` drawn one under the other in `theme`: the shapes, and each band's rect.
    fn draw(theme: Theme, bands: &[Band<'_>]) -> (Vec<ClippedShape>, Vec<Rect>) {
        let mut rects = Vec::new();
        let shapes = painted_in(vec2(W + 40.0, 1400.0), theme, |ui| {
            rects.clear();
            let room = Rect::from_min_size(AT, vec2(W, 1300.0));
            ui.scope_builder(UiBuilder::new().max_rect(room), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, band) in bands.iter().enumerate() {
                    rects.push(band.show(ui, Id::new(("band", i))).response.rect);
                    ui.add_space(APART);
                }
            });
        });
        (shapes, rects)
    }

    /// Where `words` were drawn, and their colour; the first drawing of them inside `within`.
    fn text_in(shapes: &[ClippedShape], within: Rect, words: &str) -> Option<(Rect, Color32)> {
        texts(shapes).into_iter().find(|(w, r, _)| w == words && within.contains_rect(*r)).map(|(_, r, c)| (r, c))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn the_error_band_matches_the_page() {
        // Node 168920 (its default: hint, detail and a position) and the Error band frame's five
        // bands (171342 / 173961), then a note and a server's detail and hint without a
        // position, which the page doesn't draw.
        let default = Band { detail: Some("Failing row contains (691, 42, null)."), ..PAGE_BANDS[0] };
        let [noted, both_fields] = [APP_BANDS[1], APP_BANDS[5]];
        let mut bands = PAGE_BANDS.to_vec();
        bands.extend([default, noted, both_fields]);
        for (theme, t) in both() {
            let (shapes, rects) = draw(theme, &bands);
            assert_eq!(rects.len(), bands.len(), "{theme:?}");
            let fills = fills(&shapes);
            // The page's heights: the five bands 112, 112, 66, 66, 112; its default 158.
            for (i, r) in rects.iter().enumerate().take(6) {
                let tall = [112.0, 112.0, 66.0, 66.0, 112.0, 158.0][i];
                assert!(close(r.height(), tall) && close(r.width(), W), "{theme:?} band {i}: {r:?}, the page's {W} by {tall}");
                let (ground, edge) = (t.deleted, t.danger);
                assert!(fills.contains(&(*r, ground)), "{theme:?} band {i}: on `deleted`, square: {fills:?}");
                let left = Rect::from_min_max(r.min, pos2(r.left() + 2.0, r.bottom()));
                assert!(fills.contains(&(left, edge)), "{theme:?} band {i}: a 2 px `danger` edge down its left");
            }
            assert!(edges(&shapes).is_empty(), "{theme:?}: no edge round a band: {:?}", edges(&shapes));
            for (i, (band, r)) in bands.iter().zip(&rects).enumerate() {
                let within = r.expand(0.5);
                let (heading_ink, message_ink) = (t.danger, t.text);
                // The heading, 14 in (past the 2 px edge) and 12 down, `subtitle`.
                let (h, c) = text_in(&shapes, within, band.heading).unwrap_or_else(|| panic!("{theme:?} band {i}: its heading"));
                assert!(close(h.left(), r.left() + 14.0) && close(h.top(), r.top() + 12.0) && close(h.height(), 20.0), "{theme:?} band {i}: {h:?}");
                assert_eq!(c, heading_ink, "{theme:?} band {i}: the heading's colour");
                assert_eq!(sections(&shapes, band.heading).first().map(|s| s.1.clone()), Some(Text::Subtitle.font()), "{theme:?} band {i}");
                // The position at the right end of the head, `small` `text_muted`, centred on it.
                let link = band.position.map(|(line, column)| position_words(line, column));
                match &link {
                    Some(words) => {
                        let (l, c) = text_in(&shapes, within, words).unwrap_or_else(|| panic!("{theme:?} band {i}: {words}"));
                        assert!(close(l.right(), r.right() - 12.0) && close(l.top(), r.top() + 14.0), "{theme:?} band {i}: the link at {l:?}");
                        assert_eq!(c, t.text_muted, "{theme:?} band {i}: the link's colour");
                        assert_eq!(sections(&shapes, words).first().map(|s| s.1.clone()), Some(Text::Small.font()), "{theme:?} band {i}");
                    }
                    None => assert!(
                        !texts(&shapes).iter().any(|(w, at, _)| w.starts_with("Line ") && within.contains_rect(*at)),
                        "{theme:?} band {i}: no position, no link"
                    ),
                }
                // The message under it, as written, `body`.
                let (m, c) = text_in(&shapes, within, band.message).unwrap_or_else(|| panic!("{theme:?} band {i}: its message as written"));
                assert!(close(m.left(), r.left() + 14.0) && close(m.top(), r.top() + 36.0) && close(m.height(), 18.0), "{theme:?} band {i}: {m:?}");
                assert_eq!(c, message_ink, "{theme:?} band {i}: the message's colour");
                assert_eq!(sections(&shapes, band.message).first().map(|s| s.1.clone()), Some(Text::Body.font()), "{theme:?} band {i}");
                // Each field, the detail before the hint: its name in `label` capitals 12 under the
                // part before, its text 2 under.
                let mut bottom = m.bottom();
                for (name, text) in [("DETAIL", band.detail), ("HINT", band.hint)] {
                    let Some(text) = text else { continue };
                    let (n, c) = text_in(&shapes, Rect::from_min_max(pos2(r.left(), bottom), r.max).expand(0.5), name)
                        .unwrap_or_else(|| panic!("{theme:?} band {i}: {name}"));
                    assert!(
                        close(n.left(), r.left() + 14.0) && close(n.top(), bottom + 12.0) && close(n.height(), 14.0),
                        "{theme:?} band {i} {name}: {n:?}"
                    );
                    assert_eq!(c, t.text_muted, "{theme:?} band {i}: {name}'s colour");
                    assert_eq!(sections(&shapes, name).first().map(|s| s.1.clone()), Some(Text::Label.font()), "{theme:?} band {i}");
                    let (f, c) = text_in(&shapes, within, text).unwrap_or_else(|| panic!("{theme:?} band {i}: {name}'s text"));
                    assert!(
                        close(f.left(), r.left() + 14.0) && close(f.top(), n.bottom() + 2.0) && close(f.height(), 18.0),
                        "{theme:?} band {i}: {f:?}"
                    );
                    assert_eq!(c, t.text, "{theme:?} band {i}: {name}'s text colour");
                    bottom = f.bottom();
                }
                if band.hint.is_none() {
                    assert!(text_in(&shapes, within, "HINT").is_none(), "{theme:?} band {i}: no hint, no HINT");
                }
                if band.detail.is_none() {
                    assert!(text_in(&shapes, within, "DETAIL").is_none(), "{theme:?} band {i}: no detail, no DETAIL");
                }
                // dbee's note last, 12 under the part before, `small` `text_muted`.
                if let Some(note) = band.note {
                    let (n, c) = text_in(&shapes, within, note).unwrap_or_else(|| panic!("{theme:?} band {i}: its note"));
                    assert!(close(n.left(), r.left() + 14.0) && close(n.top(), bottom + 12.0), "{theme:?} band {i}: the note at {n:?}");
                    assert_eq!(c, t.text_muted, "{theme:?} band {i}: the note's colour");
                    assert_eq!(sections(&shapes, note).first().map(|s| s.1.clone()), Some(Text::Small.font()), "{theme:?} band {i}");
                    bottom = n.bottom();
                }
                assert!(close(r.bottom(), bottom + 12.0), "{theme:?} band {i}: 12 under its last part, {bottom} in {r:?}");
            }
            let words = [noted.heading, noted.message, both_fields.message];
            assert!(
                words.iter().all(|w| text_in(&shapes, Rect::EVERYTHING, &w.to_uppercase()).is_none()),
                "{theme:?}: nothing in capitals but the names"
            );
        }
    }

    /// What the query tab's band draws and says for `e` (a run that kept `kept` results, its
    /// position at `at`), and how often it asked to jump.
    #[derive(Default)]
    struct Seen {
        tags: Tags,
        jumped: usize,
        rolled: usize,
    }

    fn query_band(e: QueryError, kept: usize, at: Option<(usize, usize)>) -> egui_kittest::Harness<'static, Seen> {
        query_band_with(e, kept, at, None)
    }

    fn query_band_with(
        e: QueryError,
        kept: usize,
        at: Option<(usize, usize)>,
        rollback: Option<Action<'static>>,
    ) -> egui_kittest::Harness<'static, Seen> {
        let mut h = harness_in(vec2(900.0, 400.0), Theme::Light, Seen::default(), move |ui, s| {
            s.tags.begin_frame();
            let asked = crate::query_tab::band(ui, &mut s.tags, TabId(1), &e, kept, at, rollback);
            if asked.jump {
                s.jumped += 1;
            }
            if asked.rollback {
                s.rolled += 1;
            }
        });
        h.run();
        h
    }

    fn table_band(text: (String, String, String)) -> egui_kittest::Harness<'static, Seen> {
        let mut h = harness_in(vec2(900.0, 400.0), Theme::Light, Seen::default(), move |ui, s| {
            s.tags.begin_frame();
            let band = Band { heading: &text.0, message: &text.1, note: (!text.2.is_empty()).then_some(text.2.as_str()), ..Band::default() };
            crate::table_tab::band(ui, &mut s.tags, TabId(2), &band, &text.1);
        });
        h.run();
        h
    }

    fn click(h: &mut egui_kittest::Harness<'static, Seen>, at: Pos2) {
        h.event(Event::PointerMoved(at));
        h.run();
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.input_mut().events.extend([press(true), press(false)]);
        h.run();
    }

    fn drawn(h: &egui_kittest::Harness<'static, Seen>) -> Vec<(String, Color32)> {
        texts(&h.output().shapes).into_iter().map(|(w, _, c)| (w, c)).collect()
    }

    #[test]
    fn the_band_keeps_what_the_cards_showed() {
        let t = crate::theme::LIGHT;
        let message = "column \"o.totl\" does not exist (SQLSTATE 42703)";
        let hint = "Perhaps you meant to reference the column \"o.total\".";
        let detail = "Failing row contains (691, 42, null).";
        let failed = QueryError {
            code: "server",
            message: message.into(),
            server_code: Some("42703".into()),
            meaning: None,
            detail: Some(detail.into()),
            hint: Some(hint.into()),
            position: Some(13),
            statement: 1,
            ms: Some(12),
        };
        // Statement 2 failed after a result was kept: its heading, position, message, hint and
        // detail, with the tag's value as the card had it.
        let mut h = query_band(failed.clone(), 1, Some((1, 14)));
        let value = h.state().tags.value(crate::query_tab::ERROR).cloned();
        let expected = json!({
            "code": "server", "heading": "Statement 2 failed", "message": message, "detail": detail, "hint": hint,
            "rollback": false, "position": 13, "statement": 1, "line": 1, "column": 14, "jump": true,
        });
        assert_eq!(value, Some(expected));
        let seen = drawn(&h);
        for (words, colour) in [
            ("Statement 2 failed", t.danger),
            ("Line 1, column 14 →", t.text_muted),
            (message, t.text),
            ("DETAIL", t.text_muted),
            (detail, t.text),
            ("HINT", t.text_muted),
            (hint, t.text),
        ] {
            assert!(seen.contains(&(words.to_owned(), colour)), "`{words}` in {colour:?}: {seen:?}");
        }
        // Postgres's and psql's order: the message, the detail, then the hint.
        let tops: Vec<f32> = [message, "DETAIL", detail, "HINT", hint]
            .iter()
            .map(|w| texts(&h.output().shapes).into_iter().find(|(s, _, _)| s == w).map_or(f32::NAN, |(_, r, _)| r.top()))
            .collect();
        assert!(tops.windows(2).all(|p| p[0] < p[1]), "the message, DETAIL, the detail, HINT, the hint, top to bottom: {tops:?}");
        // It reads out as a link, by its position's words.
        let link = egui_kittest::kittest::Queryable::query_by_role_and_label(&h, egui::accesskit::Role::Link, "Line 1, column 14 →");
        assert!(link.is_some(), "the band reads out as a link to the error's position");
        // A click on it asks to go there; so does Enter with the keyboard on it.
        let rect = h.state().tags.rect(crate::query_tab::ERROR).unwrap_or(Rect::NOTHING);
        click(&mut h, rect.center());
        assert_eq!(h.state().jumped, 1, "a click jumps");
        let mut h = query_band(failed.clone(), 1, Some((1, 14)));
        h.key_press(Key::Tab);
        h.run();
        let focus = crate::components::Edge { outer: rect, width: 2.0, colour: t.ink, radius: egui::CornerRadius::ZERO };
        assert!(edges(&h.output().shapes).contains(&focus), "keyboard focus: 2 px `ink` inside the band: {:?}", edges(&h.output().shapes));
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().jumped, 1, "Enter with the keyboard on it jumps");

        // The first statement, nothing kept: `Query failed`; with no position, no jump.
        let mut h = query_band(QueryError { position: None, statement: 0, ..failed.clone() }, 0, None);
        let value = h.state().tags.value(crate::query_tab::ERROR).cloned().unwrap_or_default();
        assert_eq!((value["heading"].clone(), value["jump"].clone(), value["line"].clone()), (json!("Query failed"), json!(false), json!(null)));
        assert!(drawn(&h).iter().all(|(w, _)| !w.starts_with("Line ")), "no position, no link");
        let rect = h.state().tags.rect(crate::query_tab::ERROR).unwrap_or(Rect::NOTHING);
        click(&mut h, rect.center());
        assert_eq!(h.state().jumped, 0, "a click does nothing");

        // A failed transaction: ROLLBACK, in the tag and drawn; a click on it asks for it.
        let aborted = QueryError {
            message: "current transaction is aborted, commands ignored until end of transaction block (SQLSTATE 25P02)".into(),
            server_code: Some("25P02".into()),
            meaning: Some(dbee_engine::Meaning::TransactionAborted),
            detail: None,
            hint: None,
            position: None,
            statement: 0,
            ..failed.clone()
        };
        let rollback = Action { label: "Rollback", enabled: true, hover: "End the failed transaction with ROLLBACK" };
        let mut h = query_band_with(aborted, 0, None, Some(rollback));
        let value = h.state().tags.value(crate::query_tab::ERROR).cloned().unwrap_or_default();
        assert_eq!((value["rollback"].clone(), value.get("note")), (json!(true), None), "{value}");
        assert!(drawn(&h).contains(&("ROLLBACK".to_owned(), t.text)), "ROLLBACK drawn: {:?}", drawn(&h));
        assert_eq!(h.state().tags.value(crate::query_tab::ROLLBACK), Some(&json!("Rollback")));
        let at = h.state().tags.rect(crate::query_tab::ROLLBACK).unwrap_or(Rect::NOTHING);
        click(&mut h, at.center());
        assert_eq!((h.state().rolled, h.state().jumped), (1, 0), "a click on ROLLBACK asks for it");

        // The table tab's three headings and their notes; `table-error` is the message.
        let view = dbee_engine::PageView { applied: dbee_sql::filter::Condition::None, sort: None, page: 0 };
        let mut tab =
            TableTab::new(TabId(2), dbee_engine::ConnectionId(1), dbee_core::TableRef { schema: "s".into(), name: "t".into() }, vec![], view);
        let failed = |code| dbee_engine::Failed { row: 0, column: None, message: "bad".into(), code, detail: None, hint: None };
        let mut texts = Vec::new();
        std::sync::Arc::make_mut(&mut tab.edits).failed = Some(failed("server"));
        texts.extend(
            crate::table_tab::card_text(&tab).map(|(b, _)| (b.heading.to_owned(), b.message.to_owned(), b.note.unwrap_or_default().to_owned())),
        );
        std::sync::Arc::make_mut(&mut tab.edits).failed = Some(failed("cancelled"));
        texts.extend(
            crate::table_tab::card_text(&tab).map(|(b, _)| (b.heading.to_owned(), b.message.to_owned(), b.note.unwrap_or_default().to_owned())),
        );
        std::sync::Arc::make_mut(&mut tab.edits).failed = None;
        tab.error = Some("invalid input syntax for type integer: \"x\"".into());
        texts.extend(
            crate::table_tab::card_text(&tab).map(|(b, _)| (b.heading.to_owned(), b.message.to_owned(), b.note.unwrap_or_default().to_owned())),
        );
        // The lost connection's heading has no note (its edits' doubt is the engine's to set).
        texts.push(("The connection was lost during Apply".to_owned(), "the connection was lost".to_owned(), String::new()));
        let headings: Vec<&str> = texts.iter().map(|(h, _, _)| h.as_str()).collect();
        assert_eq!(
            headings,
            ["The changes were not applied", "The changes were not applied", "The page did not load", "The connection was lost during Apply"]
        );
        for text in texts {
            let (heading, message, note) = (text.0.clone(), text.1.clone(), text.2.clone());
            let h = table_band(text);
            assert_eq!(h.state().tags.value("table-error"), Some(&json!(message)), "{heading}");
            let seen = drawn(&h);
            assert!(seen.contains(&(heading.to_owned(), t.danger)), "{heading}: {seen:?}");
            assert!(seen.contains(&(message.clone(), t.text)), "{heading}: {seen:?}");
            if note.is_empty() {
                assert_eq!(seen.len(), 2, "{heading}: no note: {seen:?}");
            } else {
                assert!(seen.contains(&(note.to_owned(), t.text_muted)), "{heading}: {seen:?}");
            }
        }
    }

    /// R2: one action is an outline box 12 under the message, at the text's left; a click on it
    /// reports it, not a jump.
    #[test]
    fn an_action_row_sits_12_under_the_message() {
        let band = APP_BANDS[0];
        assert_eq!(band.actions.len(), 1, "the showcase's band with ROLLBACK");
        for (theme, t) in both() {
            let (shapes, rects) = draw(theme, &[band]);
            let r = rects[0];
            let (m, _) = text_in(&shapes, r.expand(0.5), band.message).unwrap_or_else(|| panic!("{theme:?}: the message"));
            let (words, c) = text_in(&shapes, r.expand(0.5), "ROLLBACK").unwrap_or_else(|| panic!("{theme:?}: ROLLBACK"));
            assert_eq!(c, t.text, "{theme:?}: its words");
            let boxed = edges(&shapes).into_iter().find(|e| e.outer.contains_rect(words)).unwrap_or_else(|| panic!("{theme:?}: its box"));
            assert_eq!((boxed.colour, boxed.width), (t.line_strong, 1.0), "{theme:?}: an outline box: {boxed:?}");
            let b = boxed.outer;
            assert!((b.top() - (m.bottom() + 12.0)).abs() <= 1.0, "{theme:?}: 12 under the message: {b:?}, {m:?}");
            assert!(close(b.left(), m.left()), "{theme:?}: at the text's left: {b:?}, {m:?}");
            assert!(close(r.bottom(), b.bottom() + 12.0), "{theme:?}: the band ends 12 under it: {r:?}");
            assert!(!fills(&shapes).iter().any(|(f, _)| b.contains_rect(*f) && f.contains_rect(words)), "{theme:?}: no ground of its own");
        }
        // Two actions: 8 apart. A click on one reports it.
        let actions = [Action { label: "Rollback", enabled: true, hover: "" }, Action { label: "Other", enabled: true, hover: "" }];
        let mut h = harness_in(vec2(900.0, 300.0), Theme::Light, (None, Vec::new()), move |ui, s: &mut (Option<usize>, Vec<Rect>)| {
            let band = Band { heading: "Query failed", position: Some((1, 2)), message: "m", actions: &actions, ..Band::default() };
            let shown = band.show(ui, Id::new("two"));
            s.1 = shown.actions.iter().map(|r| r.rect).collect();
            if let Some(i) = shown.clicked() {
                s.0 = Some(i);
            }
            assert!(!shown.response.clicked(), "a click on an action is not a jump");
        });
        h.run();
        let boxes = h.state().1.clone();
        assert!(boxes.len() == 2 && close(boxes[1].left(), boxes[0].right() + 8.0), "8 apart: {boxes:?}");
        let at = boxes[1].center();
        h.event(Event::PointerMoved(at));
        h.run();
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.input_mut().events.extend([press(true), press(false)]);
        h.run();
        assert_eq!(h.state().0, Some(1), "the second action clicked");
    }

    /// Gap 68: with less than 120 left for the heading the position isn't drawn; a click still jumps.
    #[test]
    fn no_link_when_the_heading_would_get_less_than_120() {
        let mut h = harness_in(vec2(200.0, 300.0), Theme::Light, 0usize, |ui, jumped: &mut usize| {
            let band = Band { heading: "Statement 2 failed", position: Some((1, 14)), message: "m", ..Band::default() };
            if band.show(ui, Id::new("narrow")).response.clicked() {
                *jumped += 1;
            }
        });
        h.run();
        let shapes = h.output().shapes.clone();
        assert!(!texts(&shapes).iter().any(|(w, _, _)| w.starts_with("Line ")), "no link: {:?}", texts(&shapes));
        let link = egui_kittest::kittest::Queryable::query_by_role_and_label(&h, egui::accesskit::Role::Link, "Line 1, column 14 →");
        assert!(link.is_some(), "it still reads out as a link");
        let at = pos2(100.0, 30.0);
        h.event(Event::PointerMoved(at));
        h.run();
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.input_mut().events.extend([press(true), press(false)]);
        h.run();
        assert_eq!(*h.state(), 1, "a click jumps");
        // At 900 it is drawn.
        let mut wide = harness_in(vec2(900.0, 300.0), Theme::Light, (), |ui, _| {
            Band { heading: "Statement 2 failed", position: Some((1, 14)), message: "m", ..Band::default() }.show(ui, Id::new("wide"));
        });
        wide.run();
        assert!(texts(&wide.output().shapes).iter().any(|(w, _, _)| w == "Line 1, column 14 →"), "drawn with room");
    }
}

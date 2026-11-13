//! The value panel (`value panel`): a bar with the column's name and type, the value under it, and
//! its facts along the bottom, in each of the page's states ([`State`]). A view: it draws into the
//! rect it is given and says where the value goes ([`Shown::text`]). The panel's screen
//! ([`crate::value_panel`]) keeps the value's rows, their colours, the selection and the
//! scrolling, and lays out each row it shows with [`row_job`]: `code`, JSON coloured from the
//! tokens, plain text in `text`.
//!
//! NULL and `empty` sit where a value starts, in `code` `text_muted`; only "No cell selected" is
//! centred. Names keep their case: the column's name and type are drawn as written.
//!
//! The bar's COPY and WRAP (CANCEL and SAVE while editing) come with the value panel screen: the
//! app's panel has none yet, and nothing in the app sets the copied, editing and row states, which
//! the showcase shows. Today's kind chip (`JSONB`) and the note on a cut value keep today's look,
//! under the bar and before the value, until it is decided.

use std::ops::Range;

use dbee_core::json::JsonToken;
use egui::text::{CCursor, LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, Rect, Response, Ui, pos2, vec2};

use super::button::{BarButton, BoxButton};
use super::grid_cell::one_line;
use super::icon::Icon;
use super::painter;
use crate::editors::size_text;
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, BUTTON_H, CARET_W, RADIUS_FLOAT, VALUE_BAR_GAP, VALUE_CARET_H, VALUE_EDIT_GAP, VALUE_FACT_GAP, VALUE_FACTS_H,
    VALUE_INSET, VALUE_KIND_GAP, VALUE_KIND_H, VALUE_KIND_PAD_X, VALUE_NOTE_GAP,
};
use crate::theme::{Text, Tokens, Words, tokens};
use crate::tree::thousands;

/// A row of the value's text: [`Text::Code`]'s line.
pub const LINE_H: f32 = Text::Code.line();
/// The value's text is this far in from the sides of its box.
pub const PAD: f32 = VALUE_INSET;

/// The page's states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// JSON, pretty-printed and coloured.
    Json,
    /// Any other text, in `text`.
    Text,
    Null,
    /// A value with no text: `empty`, drawn like NULL.
    Empty,
    /// No cell selected: no name, COPY off, the note centred.
    NoCell,
    /// Just copied: COPY reads ✓ COPIED in `text_muted`.
    Copied,
    /// Several lines being edited (MC+): CANCEL and SAVE in the bar, the caret in the text.
    Editing,
    /// A whole row selected (#17), as JSON, titled with the row ("Row 3") and no type.
    Row,
}

impl State {
    /// It shows a value's text (not a word in its place, nor none).
    fn has_text(self) -> bool {
        !matches!(self, State::Null | State::Empty | State::NoCell)
    }
}

/// The facts along the bottom: the value's lines and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub lines: usize,
    pub bytes: usize,
}

impl Facts {
    /// What the facts say, as written (capitals are drawn): `12 lines`, `137 B`.
    pub fn words(self) -> [String; 2] {
        let lines = format!("{} {}", thousands(self.lines as u64), if self.lines == 1 { "line" } else { "lines" });
        let size = if self.bytes < 1024 { format!("{} B", self.bytes) } else { size_text(self.bytes) };
        [lines, size]
    }
}

/// A coloured token of a JSON text: its bytes (the text is at most 10 MiB) and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub token: JsonToken,
}

/// The value panel in one of its states, drawn into a rect.
#[derive(Clone, Debug)]
pub struct Panel<'a> {
    pub state: State,
    /// The column's name as written, or the row's title ("Row 3").
    pub title: &'a str,
    /// The column's type as written; a row has none.
    pub ty: &'a str,
    /// Today's kind chip: what JSON the value is, a type from the database as a name.
    pub kind: Option<Words>,
    /// Today's note on a cut value: how much of it shows.
    pub note: Option<&'a str>,
    /// The value's facts, along the bottom of a state that shows a value.
    pub facts: Option<Facts>,
    /// COPY and WRAP in the bar, CANCEL and SAVE while editing (the value panel screen's).
    pub buttons: bool,
    /// WRAP is on.
    pub wrap: bool,
    /// Its own 1 px `line` down its left side; off in a panel that draws that line itself.
    pub edge: bool,
}

/// What [`Panel::show`] drew: where the value goes, and the bar's buttons.
#[derive(Debug)]
pub struct Shown {
    /// Between the bar and the facts.
    pub body: Rect,
    /// The value's box: its first row at the top, [`PAD`] in from its sides.
    pub text: Rect,
    pub copy: Option<Response>,
    pub wrap: Option<Response>,
    pub cancel: Option<Response>,
    pub save: Option<Response>,
}

impl Panel<'_> {
    /// Draws the panel into `rect`: its ground, its bar, the facts and the state's word; the value
    /// itself is the caller's, in [`Shown::text`].
    pub fn show(&self, ui: &mut Ui, rect: Rect) -> Shown {
        let t = tokens(ui.ctx());
        let p = painter(ui);
        p.rect_filled(rect, CornerRadius::ZERO, t.card);
        let mut inner = rect;
        if self.edge {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(BAR_LINE, rect.height())), CornerRadius::ZERO, t.line);
            inner.min.x += BAR_LINE;
        }
        let bar = Rect::from_min_size(inner.min, vec2(inner.width(), BAR_H.min(inner.height())));
        p.rect_filled(Rect::from_min_max(pos2(bar.left(), bar.bottom() - BAR_LINE), bar.max), CornerRadius::ZERO, t.line);
        let facts = self.facts.filter(|_| self.state.has_text());
        let facts_top = if facts.is_some() { (inner.bottom() - VALUE_FACTS_H).max(bar.bottom()) } else { inner.bottom() };
        if let Some(facts) = facts {
            let strip = Rect::from_min_max(pos2(inner.left(), facts_top), inner.max);
            p.rect_filled(Rect::from_min_size(strip.min, vec2(strip.width(), BAR_LINE)), CornerRadius::ZERO, t.line);
            let mut x = strip.left() + VALUE_INSET;
            for words in facts.words() {
                let drawn = Text::Label.paint(&p, pos2(x, strip.center().y), Align2::LEFT_CENTER, &words, t.text_muted);
                x = drawn.right() - Text::Label.spacing() + VALUE_FACT_GAP;
            }
        }
        let body = Rect::from_min_max(pos2(inner.left(), bar.bottom()), pos2(inner.right(), facts_top.max(bar.bottom())));
        let mut shown = Shown { body, text: body, copy: None, wrap: None, cancel: None, save: None };
        let buttons_left = if self.buttons { self.buttons(ui, bar, &mut shown) } else { bar.right() };
        self.title(ui, t, bar, buttons_left);
        let top = self.under_bar(ui, t, body);
        shown.text = Rect::from_min_max(pos2(body.left(), top), pos2(body.right(), (body.bottom() - VALUE_INSET).max(top)));
        let p = painter(ui).with_clip_rect(body.intersect(ui.clip_rect()));
        match self.state {
            State::Null | State::Empty => {
                let word = if self.state == State::Null { "NULL" } else { "empty" };
                let galley = p.layout_job(Text::Code.job_as_written(word, t.text_muted));
                let at = pos2(shown.text.left() + PAD, shown.text.top() + (LINE_H - galley.size().y) / 2.0);
                p.galley(at, galley, t.text_muted);
            }
            State::NoCell => {
                Text::Body.paint(&p, body.center(), Align2::CENTER_CENTER, "No cell selected", t.text_muted);
            }
            _ => {}
        }
        shown
    }

    /// The name and type at the bar's left, cut to end before `right`.
    fn title(&self, ui: &Ui, t: &Tokens, bar: Rect, right: f32) {
        if self.state == State::NoCell {
            return;
        }
        let p = painter(ui);
        let left = bar.left() + VALUE_INSET;
        let room = right - VALUE_INSET - left;
        let ty = if self.state == State::Row { "" } else { self.ty };
        // Today's share: the name takes at most 70 % when there is a type after it.
        let name_max = if ty.is_empty() { room } else { room * 0.7 };
        let name = one_line(ui, self.title, Text::Header, t.text, name_max);
        let name_w = name.size().x;
        p.galley(pos2(left, bar.center().y - name.size().y / 2.0), name, t.text);
        let type_left = left + name_w + VALUE_BAR_GAP;
        if !ty.is_empty() && right - VALUE_INSET - type_left > VALUE_INSET {
            let ty = one_line(ui, ty, Text::Data, t.text_muted, right - VALUE_INSET - type_left);
            p.galley(pos2(type_left, bar.center().y - ty.size().y / 2.0), ty, t.text_muted);
        }
    }

    /// The bar's buttons at its right end; returns where they start.
    fn buttons(&self, ui: &mut Ui, bar: Rect, shown: &mut Shown) -> f32 {
        let right = bar.right();
        if self.state == State::Editing {
            let save = BoxButton::primary("Save");
            let cancel = BoxButton::outline("Cancel");
            let (save_w, cancel_w) = (save.width(ui), cancel.width(ui));
            let y = bar.center().y - BUTTON_H / 2.0;
            let save_at = Rect::from_min_size(pos2(right - VALUE_INSET - save_w, y), vec2(save_w, BUTTON_H));
            let cancel_at = Rect::from_min_size(pos2(save_at.left() - VALUE_EDIT_GAP - cancel_w, y), vec2(cancel_w, BUTTON_H));
            shown.cancel = Some(ui.put(cancel_at, cancel));
            shown.save = Some(ui.put(save_at, save));
            return cancel_at.left();
        }
        let wrap = BarButton::label(Icon::Wrap, "Wrap").over_line().on(self.wrap);
        let wrap_w = wrap.width(ui);
        let copied = self.state == State::Copied;
        let copy_w = BarButton::label(Icon::Check, "Copied").width(ui);
        let copy = if copied { BarButton::label(Icon::Check, "Copied").done(true) } else { BarButton::label(Icon::Copy, "Copy") };
        let copy = copy.over_line().min_width(copy_w);
        let wrap_at = Rect::from_min_size(pos2(right - wrap_w, bar.top()), vec2(wrap_w, BAR_CELL_H));
        let copy_at = Rect::from_min_size(pos2(wrap_at.left() - VALUE_BAR_GAP - copy_w, bar.top()), vec2(copy_w, BAR_CELL_H));
        shown.copy = Some(ui.add_enabled_ui(self.state != State::NoCell, |ui| ui.put(copy_at, copy)).inner);
        shown.wrap = Some(ui.put(wrap_at, wrap));
        copy_at.left()
    }

    /// Today's kind chip and note under the bar; returns where the value starts.
    fn under_bar(&self, ui: &Ui, t: &Tokens, body: Rect) -> f32 {
        let left = body.left() + VALUE_INSET;
        let width = (body.width() - 2.0 * VALUE_INSET).max(1.0);
        let mut y = body.top() + VALUE_INSET;
        if self.kind.is_none() && self.note.is_none() {
            return y;
        }
        let p = painter(ui);
        if let Some(kind) = &self.kind {
            let mut job = kind.caps_job(t.ink);
            job.wrap = TextWrapping::truncate_at_width((width - 2.0 * VALUE_KIND_PAD_X).max(1.0));
            let words = p.layout_job(job);
            let pill = Rect::from_min_size(pos2(left, y), vec2(words.size().x + 2.0 * VALUE_KIND_PAD_X, VALUE_KIND_H));
            p.rect_filled(pill, CornerRadius::same(RADIUS_FLOAT), t.selection);
            p.galley(pos2(pill.left() + VALUE_KIND_PAD_X, pill.center().y - words.size().y / 2.0), words, t.ink);
            y = pill.bottom() + VALUE_KIND_GAP;
        }
        if let Some(note) = self.note {
            let mut job = Text::Small.job(note, t.text_muted);
            job.wrap.max_width = width;
            let words = p.layout_job(job);
            let h = words.size().y;
            p.galley(pos2(left, y), words, t.text_muted);
            y += h;
        }
        y + VALUE_NOTE_GAP
    }
}

/// The colour of a JSON token.
pub fn token_colour(token: JsonToken, t: &Tokens) -> Color32 {
    match token {
        JsonToken::Key => t.syn_keyword,
        JsonToken::String => t.syn_string,
        JsonToken::Number => t.syn_number,
        JsonToken::Literal => t.syn_literal,
        JsonToken::Punct => t.text_muted,
    }
}

/// The layout of row `range` of `text`: in `code`, coloured as JSON by `spans` (sorted, without
/// the punctuation, which is `text_muted` like everything between them), else in `text`; never
/// wrapped (the row is as long as fits).
pub fn row_job(text: &str, range: Range<usize>, spans: Option<&[Span]>, t: &Tokens) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut add = |from: usize, to: usize, color: Color32| {
        if let Some(s) = text.get(from..to).filter(|s| !s.is_empty()) {
            job.append(s, 0.0, Text::Code.format(color));
        }
    };
    let Some(spans) = spans else {
        add(range.start, range.end, t.text);
        return job;
    };
    let mut at = range.start;
    let from = spans.partition_point(|s| s.end as usize <= range.start);
    for s in spans.get(from..).unwrap_or_default().iter().take_while(|s| (s.start as usize) < range.end) {
        let (a, b) = ((s.start as usize).max(range.start), (s.end as usize).min(range.end));
        add(at, a, t.text_muted);
        add(a, b, token_colour(s.token, t));
        at = b;
    }
    add(at, range.end, t.text_muted);
    job
}

/// A short value drawn whole in the value's box `at` (the showcase's): a row a line, never
/// wrapped, coloured as JSON by `spans`; with the editing caret before byte `caret`.
pub fn paint_value(ui: &Ui, at: Rect, value: &str, spans: Option<&[Span]>, caret: Option<usize>) {
    let t = tokens(ui.ctx());
    let p = painter(ui).with_clip_rect(at.intersect(ui.clip_rect()));
    let mut start = 0;
    for (i, line) in value.split('\n').enumerate() {
        let range = start..start + line.len();
        let galley = p.layout_job(row_job(value, range.clone(), spans, t));
        let top = at.top() + LINE_H * i as f32;
        let left = at.left() + PAD;
        if let Some(c) = caret.filter(|c| range.contains(c) || *c == range.end) {
            let chars = value.get(range.start..c).map_or(0, |s| s.chars().count());
            let x = left + galley.pos_from_cursor(CCursor::new(chars)).min.x;
            let caret = Rect::from_min_size(pos2(x, top + (LINE_H - VALUE_CARET_H) / 2.0), vec2(CARET_W, VALUE_CARET_H));
            p.rect_filled(caret, CornerRadius::ZERO, t.ink);
        }
        p.galley(pos2(left, top + (LINE_H - galley.size().y) / 2.0), galley, t.text);
        start = range.end + 1;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use dbee_core::{Column, TypeClass};
    use dbee_engine::ValueView;
    use egui::epaint::ClippedShape;
    use egui::{Event, Shape, Theme, UiBuilder, pos2, vec2};
    use serde_json::{Value as Json, json};

    use super::*;
    use crate::automation::Tags;
    use crate::components::showcase::{value_panel, value_samples};
    use crate::components::{both, fills, flat, harness_in, painted_in, sections, texts};
    use crate::value_panel::{PanelState, contents};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// Where a sample is drawn: the page's 340 by 372.
    const AT: Rect = Rect { min: pos2(10.0, 10.0), max: pos2(350.0, 382.0) };

    /// The drawn text whose words are `words`: where, its colour and its first section's font.
    fn drawn(shapes: &[ClippedShape], words: &str) -> Option<(Rect, Color32, egui::FontId, bool)> {
        flat(shapes).into_iter().find_map(|s| match s {
            Shape::Text(t) if t.galley.text() == words => {
                let format = &t.galley.job.sections.first()?.format;
                let colour = texts(&[ClippedShape { clip_rect: Rect::EVERYTHING, shape: Shape::Text(t.clone()) }]).first()?.2;
                Some((t.galley.rect.translate(t.pos.to_vec2()), colour, format.font_id.clone(), format.italics))
            }
            _ => None,
        })
    }

    #[test]
    fn each_value_panel_state_matches_the_page() {
        // Node 167904: 340 by 372 on `card`, a 1 px `line` down the left; the bar 36 tall with its
        // line along the bottom, the name 12 in (`header` `text`), the type 6 after (`data`
        // `text_muted`); COPY 82 wide, 6 before WRAP at the right end; the value 12 in and 12
        // under the bar, in `code`; the facts in a 30 tall strip with its line along the top.
        let (x0, y0) = (AT.left(), AT.top());
        for (theme, t) in both() {
            for (i, sample) in value_samples().iter().enumerate() {
                let state = sample.panel.state;
                let at_ = format!("{theme:?} {state:?}");
                let mut seen = None;
                let shapes = painted_in(vec2(400.0, 420.0), theme, |ui| {
                    let s = value_panel(ui, &value_samples()[i], AT);
                    let rect = |r: Option<Response>| r.map(|r| r.rect);
                    seen = Some((s.body, s.text, rect(s.copy), rect(s.wrap), rect(s.cancel), rect(s.save)));
                });
                let (body, text, copy, wrap, cancel, save) = seen.unwrap_or_else(|| panic!("{at_}: drawn"));
                let fills = fills(&shapes);
                let filled = |r: Rect, c: Color32| {
                    fills.iter().any(|(f, fc)| {
                        *fc == c
                            && close(f.left(), r.left())
                            && close(f.top(), r.top())
                            && close(f.right(), r.right())
                            && close(f.bottom(), r.bottom())
                    })
                };
                assert_eq!(fills.iter().find(|(r, _)| AT.contains_rect(*r)), Some(&(AT, t.card)), "{at_}: the ground first, `card`");
                assert!(filled(Rect::from_min_size(AT.min, vec2(1.0, 372.0)), t.line), "{at_}: the line down the left: {fills:?}");
                assert!(filled(Rect::from_min_size(pos2(x0 + 1.0, y0 + 35.0), vec2(339.0, 1.0)), t.line), "{at_}: the bar's line");
                assert_eq!(
                    body,
                    Rect::from_min_max(pos2(x0 + 1.0, y0 + 36.0), pos2(AT.right(), if state.has_text() { y0 + 342.0 } else { AT.bottom() })),
                    "{at_}: the body"
                );
                assert_eq!((text.left(), text.top(), text.right()), (x0 + 1.0, y0 + 48.0, AT.right()), "{at_}: the value 12 under the bar");

                // The name and the type.
                let title = sample.panel.title;
                if state == State::NoCell {
                    assert!(drawn(&shapes, "meta").is_none() && drawn(&shapes, "coupon").is_none(), "{at_}: no name");
                } else {
                    let (r, c, font, _) = drawn(&shapes, title).unwrap_or_else(|| panic!("{at_}: {title} drawn"));
                    assert_eq!((c, font), (t.text, Text::Header.font()), "{at_}: the name");
                    assert!(close(r.left(), x0 + 13.0) && close(r.center().y, y0 + 18.0), "{at_}: the name at {r:?}");
                    let ty = sample.panel.ty;
                    if state == State::Row {
                        assert!(ty.is_empty() && drawn(&shapes, "jsonb").is_none(), "{at_}: a row has no type");
                    } else {
                        let (tr, tc, tfont, _) = drawn(&shapes, ty).unwrap_or_else(|| panic!("{at_}: {ty} drawn"));
                        assert_eq!((tc, tfont), (t.text_muted, Text::Data.font()), "{at_}: the type");
                        assert!(close(tr.left(), r.right() + 6.0) && close(tr.center().y, y0 + 18.0), "{at_}: the type 6 after the name: {tr:?}");
                    }
                }

                // The bar's buttons.
                if state == State::Editing {
                    let (cancel, save) = (cancel.unwrap(), save.unwrap());
                    assert!(
                        close(save.right(), AT.right() - 12.0) && close(save.top(), y0 + 4.0) && close(save.height(), 28.0),
                        "{at_}: SAVE {save:?}"
                    );
                    assert!(close(cancel.right(), save.left() - 8.0) && close(cancel.top(), y0 + 4.0), "{at_}: CANCEL {cancel:?}");
                    assert!(drawn(&shapes, "CANCEL").is_some() && drawn(&shapes, "SAVE").is_some(), "{at_}: their words");
                    assert!(copy.is_none() && wrap.is_none() && drawn(&shapes, "COPY").is_none(), "{at_}: no COPY or WRAP");
                } else {
                    let (copy, wrap) = (copy.unwrap(), wrap.unwrap());
                    assert_eq!(wrap, Rect::from_min_max(pos2(AT.right() - 69.0, y0), pos2(AT.right(), y0 + 35.0)), "{at_}: WRAP at the right end");
                    assert_eq!(copy, Rect::from_min_max(pos2(x0 + 183.0, y0), pos2(x0 + 265.0, y0 + 35.0)), "{at_}: COPY 82 wide, 6 before WRAP");
                    assert!(filled(Rect::from_min_size(wrap.min, vec2(1.0, 35.0)), t.line), "{at_}: WRAP's divider");
                    assert!(filled(Rect::from_min_size(copy.min, vec2(1.0, 35.0)), t.line), "{at_}: COPY's divider");
                    let (word, colour) = match state {
                        State::NoCell => ("COPY", t.text_faint),
                        State::Copied => ("COPIED", t.text_muted),
                        _ => ("COPY", t.text),
                    };
                    let (r, c, _, _) = drawn(&shapes, word).unwrap_or_else(|| panic!("{at_}: {word} drawn"));
                    assert_eq!(c, colour, "{at_}: {word}");
                    assert!(close(r.center().y, copy.center().y), "{at_}: {word} centred in its cell");
                    assert_eq!(drawn(&shapes, "WRAP").map(|d| d.1), Some(t.text), "{at_}: WRAP");
                    assert!(cancel.is_none() && save.is_none(), "{at_}: no CANCEL or SAVE");
                }

                // The facts.
                let facts_line = Rect::from_min_size(pos2(x0 + 1.0, y0 + 342.0), vec2(339.0, 1.0));
                if let Some(facts) = sample.panel.facts.filter(|_| state.has_text()) {
                    assert!(filled(facts_line, t.line), "{at_}: the facts' line");
                    let [lines, size] = facts.words().map(|w| w.to_uppercase());
                    let (lr, lc, lfont, _) = drawn(&shapes, &lines).unwrap_or_else(|| panic!("{at_}: {lines} drawn"));
                    let (sr, sc, _, _) = drawn(&shapes, &size).unwrap_or_else(|| panic!("{at_}: {size} drawn"));
                    assert_eq!((lc, sc, lfont), (t.text_muted, t.text_muted, Text::Label.font()), "{at_}: the facts in `label` `text_muted`");
                    assert!(close(lr.left(), x0 + 13.0) && close(lr.center().y, y0 + 357.0), "{at_}: {lines} at {lr:?}");
                    assert!(
                        close(sr.left(), lr.right() - Text::Label.spacing() + 12.0) && close(sr.center().y, lr.center().y),
                        "{at_}: {size} 12 after"
                    );
                } else {
                    assert!(!filled(facts_line, t.line), "{at_}: no facts");
                    assert!(!texts(&shapes).iter().any(|(w, _, _)| w.ends_with("LINES")), "{at_}: no facts");
                }

                // The value, or the word in its place.
                match state {
                    State::Null | State::Empty => {
                        let word = if state == State::Null { "NULL" } else { "empty" };
                        let (r, c, font, italics) = drawn(&shapes, word).unwrap_or_else(|| panic!("{at_}: {word} drawn"));
                        assert_eq!((c, font, italics), (t.text_muted, Text::Code.font(), false), "{at_}: `code` `text_muted`, regular");
                        assert!(close(r.left(), x0 + 13.0) && close(r.top(), y0 + 48.0), "{at_}: where a value starts: {r:?}");
                    }
                    State::NoCell => {
                        let (r, c, font, _) = drawn(&shapes, "No cell selected").unwrap_or_else(|| panic!("{at_}: the note"));
                        assert_eq!((c, font), (t.text_muted, Text::Body.font()), "{at_}: the note");
                        assert!(close(r.center().x, body.center().x) && close(r.center().y, body.center().y), "{at_}: centred: {r:?} in {body:?}");
                    }
                    _ => {
                        let (value, json, _) = sample.value.clone().unwrap();
                        let first = value.split('\n').next().unwrap();
                        let (r, _, font, _) = drawn(&shapes, first).unwrap_or_else(|| panic!("{at_}: {first:?} drawn"));
                        assert_eq!(font, Text::Code.font(), "{at_}: `code`");
                        assert!(close(r.left(), x0 + 13.0) && close(r.top(), y0 + 48.0), "{at_}: the first row at {r:?}");
                        if json {
                            let row = value.split('\n').nth(1).unwrap();
                            let parts = sections(&shapes, row);
                            let key = parts.iter().find(|(w, _, _)| w.starts_with('"')).unwrap_or_else(|| panic!("{at_}: a key in {parts:?}"));
                            assert_eq!(key.2, t.syn_keyword, "{at_}: the key {key:?}");
                            assert!(
                                parts.iter().any(|(w, _, c)| w.trim() == ":" && *c == t.text_muted)
                                    || parts.iter().any(|(w, _, c)| w.contains(':') && *c == t.text_muted),
                                "{at_}: `:` in `text_muted`: {parts:?}"
                            );
                            let value_colour = |w: &str| parts.iter().find(|(p, _, _)| p.trim() == w).map(|p| p.2);
                            match state {
                                State::Row => assert_eq!(value_colour("691"), Some(t.syn_number), "{at_}: {parts:?}"),
                                _ => assert_eq!(value_colour("\"web\""), Some(t.syn_string), "{at_}: {parts:?}"),
                            }
                        } else {
                            assert_eq!(drawn(&shapes, first).map(|d| d.1), Some(t.text), "{at_}: plain text in `text`");
                        }
                    }
                }
                let caret: Vec<Rect> = fills.iter().filter(|(r, c)| *c == t.ink && close(r.width(), 1.0)).map(|(r, _)| *r).collect();
                if state == State::Editing {
                    let row = "  \"gift\": false,";
                    let (r, _, _, _) = drawn(&shapes, row).unwrap_or_else(|| panic!("{at_}: {row} drawn"));
                    let x = r.left() + r.width() * 15.0 / 16.0;
                    let want = Rect::from_min_size(pos2(x, y0 + 48.0 + 40.0 + 2.0), vec2(1.0, 16.0));
                    assert!(
                        caret.iter().any(|c| close(c.left(), want.left()) && close(c.top(), want.top()) && close(c.height(), 16.0)),
                        "{at_}: the caret after `false`: {caret:?}, want {want:?}"
                    );
                } else {
                    assert!(caret.is_empty(), "{at_}: no caret: {caret:?}");
                }
            }
        }
    }

    /// A column `name` of `ty`.
    fn col(name: &str, ty: &str) -> Column {
        let class = if ty.contains("json") { TypeClass::Json } else { TypeClass::LongText };
        Column { name: name.into(), base_type: ty.into(), type_label: ty.into(), class }
    }

    fn view(name: &str, ty: &str, value: Option<&str>) -> Arc<ValueView> {
        Arc::new(ValueView::new(&col(name, ty), value))
    }

    /// The app's panel: what it shows, what it keeps, its tags.
    struct App {
        view: Option<Arc<ValueView>>,
        panel: PanelState,
        tags: Tags,
    }

    /// The app's panel over `view` in a window of `size`, drawn until it settles.
    fn app(theme: Theme, size: egui::Vec2, view: Option<Arc<ValueView>>) -> egui_kittest::Harness<'static, App> {
        let mut h = harness_in(size, theme, App { view, panel: PanelState::default(), tags: Tags::default() }, |ui, s| {
            s.tags.begin_frame();
            let mut child = ui.new_child(UiBuilder::new().max_rect(ui.max_rect()));
            contents(&mut child, s.view.as_ref(), &mut s.panel, &mut s.tags);
        });
        h.run();
        h
    }

    #[test]
    fn null_and_empty_sit_where_a_value_starts() {
        // As the design draws it: NULL and `empty` at the top left in `code` `text_muted`;
        // only "No cell selected" is centred.
        for (theme, t) in both() {
            let h = app(theme, vec2(360.0, 500.0), Some(view("note", "text", Some("plain"))));
            let (start, ..) = drawn(&h.output().shapes, "plain").unwrap();
            let (panel, text) = (h.state().tags.rect("value-panel").unwrap(), h.state().tags.rect("value-text").unwrap());
            let want = pos2(panel.left() + PAD, panel.top() + BAR_H + VALUE_INSET);
            assert!(close(start.left(), want.x) && close(start.top(), want.y), "{theme:?}: a value's first row at {start:?}, want {want:?}");
            assert!(close(text.top(), want.y) && close(text.right(), panel.right()), "{theme:?}: the text's box {text:?} from its first row");
            for (value, word) in [(None, "NULL"), (Some(""), "empty")] {
                let h = app(theme, vec2(360.0, 500.0), Some(view("note", "text", value)));
                let (r, c, font, italics) = drawn(&h.output().shapes, word).unwrap_or_else(|| panic!("{theme:?}: {word} drawn"));
                assert_eq!((c, font, italics), (t.text_muted, Text::Code.font(), false), "{theme:?}: {word} in `code` `text_muted`, regular");
                assert!(close(r.left(), start.left()) && close(r.top(), start.top()), "{theme:?}: {word} at {r:?}, where a value starts {start:?}");
            }
            let h = app(theme, vec2(360.0, 500.0), None);
            let shapes = &h.output().shapes;
            let panel = h.state().tags.rect("value-panel").unwrap();
            let body = Rect::from_min_max(pos2(panel.left(), panel.top() + 36.0), panel.max);
            let (r, c, font, _) = drawn(shapes, "No cell selected").unwrap_or_else(|| panic!("{theme:?}: the note: {:?}", texts(shapes)));
            assert_eq!((c, font), (t.text_muted, Text::Body.font()), "{theme:?}: the note");
            assert!(close(r.center().x, body.center().x) && close(r.center().y, body.center().y), "{theme:?}: centred in {body:?}: {r:?}");
            assert_eq!(texts(shapes).len(), 1, "{theme:?}: the note alone (today's second line goes): {:?}", texts(shapes));
        }
    }

    #[test]
    fn json_keys_and_strings_keep_their_case() {
        // Review Focus 2: a JSON key and string, a column's name and type draw as written; NULL is
        // never the text 'NULL'.
        let raw = r#"{"Src": "Web", "GIFT": true, "MixedCase": "Leave At The Door", "n": null}"#;
        for (theme, t) in both() {
            let h = app(theme, vec2(420.0, 500.0), Some(view("Meta", "jsonb", Some(raw))));
            let shapes = &h.output().shapes;
            let all: Vec<String> = texts(shapes).into_iter().map(|(w, _, _)| w).collect();
            let row = |key: &str| {
                flat(shapes).into_iter().find_map(|s| match s {
                    Shape::Text(g) if g.galley.text().contains(key) => Some(g.galley.text().to_owned()),
                    _ => None,
                })
            };
            for (key, value, colour) in
                [("\"Src\"", "\"Web\"", t.syn_string), ("\"GIFT\"", "true", t.syn_literal), ("\"MixedCase\"", "\"Leave At The Door\"", t.syn_string)]
            {
                let line = row(key).unwrap_or_else(|| panic!("{theme:?}: {key} drawn: {all:?}"));
                let parts = sections(shapes, &line);
                assert!(parts.iter().any(|(w, _, c)| w == key && *c == t.syn_keyword), "{theme:?}: {key} as written: {parts:?}");
                assert!(parts.iter().any(|(w, _, c)| w.trim() == value && *c == colour), "{theme:?}: {value} as written: {parts:?}");
            }
            for upper in ["\"SRC\"", "\"WEB\"", "\"MIXEDCASE\"", "\"LEAVE AT THE DOOR\""] {
                assert!(!all.iter().any(|w| w.contains(upper)), "{theme:?}: never {upper}");
            }
            assert_eq!(drawn(shapes, "Meta").map(|d| (d.1, d.2)), Some((t.text, Text::Header.font())), "{theme:?}: the column as written");
            assert_eq!(drawn(shapes, "jsonb").map(|d| d.1), Some(t.text_muted), "{theme:?}: the type as written");
            // Today's chip names the column's type: as written, never `JSONB`.
            assert!(
                all.iter().filter(|w| *w == "jsonb").count() == 2 && !all.iter().any(|w| w == "JSONB"),
                "{theme:?}: the chip as written: {all:?}"
            );
            // dbee's own words in the chip stay in capitals.
            let h = app(theme, vec2(420.0, 500.0), Some(view("raw", "text", Some(r#"{"k": 1}"#))));
            assert!(
                drawn(&h.output().shapes, "TEXT CONTAINING JSON").is_some(),
                "{theme:?}: dbee's label in capitals: {:?}",
                texts(&h.output().shapes)
            );

            // The text 'NULL' and NULL, in a column whose type has capitals.
            let ty = "enum('small','Large')";
            let text = app(theme, vec2(420.0, 500.0), Some(view("Mood", ty, Some("NULL"))));
            let null = app(theme, vec2(420.0, 500.0), Some(view("Mood", ty, None)));
            for h in [&text, &null] {
                assert_eq!(drawn(&h.output().shapes, ty).map(|d| d.1), Some(t.text_muted), "{theme:?}: {ty} as written");
            }
            assert_eq!(drawn(&text.output().shapes, "NULL").map(|d| d.1), Some(t.text), "{theme:?}: the text 'NULL' in `text`");
            assert_eq!(drawn(&null.output().shapes, "NULL").map(|d| d.1), Some(t.text_muted), "{theme:?}: NULL in `text_muted`");
            assert_eq!(text.state().tags.value("value-panel").map(|v| v["null"].clone()), Some(json!(false)));
            assert_eq!(null.state().tags.value("value-panel").map(|v| v["null"].clone()), Some(json!(true)));
            assert_eq!(text.state().tags.value("value-text").map(|v| v["text"].clone()), Some(json!("NULL")));
            assert_eq!(null.state().tags.value("value-text").map(|v| v["text"].clone()), Some(json!("")));
        }
    }

    /// The rows of the value's text drawn in `shapes` within `text`: each row's words.
    fn rows_in(shapes: &[ClippedShape], text: Rect) -> Vec<String> {
        flat(shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(g) if g.galley.job.sections.first().is_some_and(|s| s.format.font_id == Text::Code.font()) => {
                    let r = g.galley.rect.translate(g.pos.to_vec2());
                    text.intersects(r).then(|| g.galley.text().to_owned())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_10_mib_value_lays_out_only_what_shows() {
        // Review Focus 3, as a count: a 20 MB text, cut to 10 MiB, in a 600 tall panel lays out
        // the rows that fit and one more, at the top and far down.
        let line = "0123456789 abcdefghij klmnopqrst uvwxyz ABCDEFGHIJ KLMNOPQRST UVWXYZ 0123456789\n";
        let big = line.repeat(20_000_000 / line.len());
        let mut h = app(Theme::Light, vec2(360.0, 600.0), Some(view("note", "text", Some(&big))));
        let check = |h: &egui_kittest::Harness<'static, App>, at: &str| {
            let text = h.state().tags.rect("value-text").unwrap();
            let v = h.state().tags.value("value-text").unwrap().clone();
            let fit = (text.height() / LINE_H).ceil() as usize;
            let columns = ((text.width() - 2.0 * PAD) / 7.8).floor() as usize;
            let rows = rows_in(&h.output().shapes, text);
            assert!(!rows.is_empty() && rows.len() <= fit + 1, "{at}: {} rows laid out, {fit} fit", rows.len());
            assert!(rows.iter().all(|r| r.chars().count() <= columns), "{at}: no row longer than {columns} columns");
            assert!(v["rows"].as_u64().unwrap() > 200_000, "{at}: every row counted: {}", v["rows"]);
            v["firstRow"].as_u64().unwrap()
        };
        assert_eq!(check(&h, "at the top"), 0);
        let head = h.state().tags.value("value-panel").unwrap();
        assert_eq!((head["truncated"].clone(), head["bytes"].clone()), (json!(true), json!(big.len())), "{head}");
        let at = h.state().tags.rect("value-text").unwrap().center();
        for _ in 0..5 {
            h.input_mut().events.push(Event::PointerMoved(at));
            h.input_mut().events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -400_000.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            // egui eases a wheel's scroll over several frames.
            h.run_steps(12);
        }
        let first = check(&h, "far down");
        assert!(first > 10_000, "scrolled far down: row {first}");
    }

    #[test]
    fn value_panel_tags_keep_their_values() {
        let keys = |v: &Json| -> Vec<String> {
            let mut k: Vec<String> = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            k.sort();
            k
        };
        let size = vec2(400.0, 500.0);
        // No cell: the panel is there, `null`, with no text.
        let h = app(Theme::Light, size, None);
        assert_eq!(h.state().tags.value("value-panel"), Some(&Json::Null));
        assert!(h.state().tags.value("value-text").is_none());
        let whole = h.state().tags.rect("value-panel").unwrap();
        assert_eq!(whole.size(), size - vec2(16.0, 16.0), "the whole of what it was given");
        // JSON.
        let raw = r#"{"a": [1, 2], "b": "x"}"#;
        let json_view = view("billing", "jsonb", Some(raw));
        let h = app(Theme::Light, size, Some(json_view.clone()));
        let head = h.state().tags.value("value-panel").unwrap().clone();
        assert_eq!(keys(&head), ["bytes", "column", "label", "note", "null", "truncated", "type"]);
        assert_eq!(
            head,
            json!({ "column": "billing", "type": "jsonb", "label": "jsonb", "null": false, "bytes": raw.len(), "truncated": false, "note": null })
        );
        let v = h.state().tags.value("value-text").unwrap().clone();
        assert_eq!(keys(&v), ["cut", "firstRow", "highlighted", "rows", "selection", "text"]);
        let lines = json_view.text.split('\n').count();
        assert_eq!(v, json!({ "text": json_view.text, "cut": false, "selection": null, "highlighted": true, "rows": lines, "firstRow": 0 }));
        let text = h.state().tags.rect("value-text").unwrap();
        assert!(whole.contains_rect(text) && text.top() >= whole.top() + 36.0, "the text under the bar: {text:?} in {whole:?}");
        // Its facts along the bottom: the lines shown and the whole value's size.
        for fact in [format!("{lines} LINES"), format!("{} B", raw.len())] {
            let (r, ..) = drawn(&h.output().shapes, &fact).unwrap_or_else(|| panic!("{fact} drawn: {:?}", texts(&h.output().shapes)));
            assert!(r.bottom() <= whole.bottom() && r.top() >= whole.bottom() - 30.0, "{fact} along the bottom: {r:?}");
        }
        // NULL and empty.
        let h = app(Theme::Light, size, Some(view("note", "text", None)));
        assert_eq!(h.state().tags.value("value-panel").unwrap()["null"], true);
        assert_eq!(
            h.state().tags.value("value-text").unwrap(),
            &json!({ "text": "", "cut": false, "selection": null, "highlighted": false, "rows": 0, "firstRow": 0 })
        );
        let h = app(Theme::Light, size, Some(view("note", "text", Some(""))));
        assert_eq!(
            h.state().tags.value("value-text").unwrap(),
            &json!({ "text": "", "cut": false, "selection": null, "highlighted": false, "rows": 1, "firstRow": 0 })
        );
        // A cut value says so.
        let h = app(Theme::Light, size, Some(view("note", "text", Some(&"x".repeat(20_000_000)))));
        let head = h.state().tags.value("value-panel").unwrap();
        assert_eq!((head["truncated"].clone(), head["note"].clone()), (json!(true), json!("Showing the first 10.0 MiB of 19.1 MiB.")));
        assert_eq!(h.state().tags.value("value-text").unwrap()["cut"], true);
        assert!(drawn(&h.output().shapes, "Showing the first 10.0 MiB of 19.1 MiB.").is_some(), "today's note, drawn");
        // The status bar's toggle.
        for open in [true, false] {
            let mut h = harness_in(vec2(200.0, 60.0), Theme::Light, Tags::default(), move |ui, tags| {
                tags.begin_frame();
                crate::value_panel::toggle(ui, open, tags);
            });
            h.run();
            assert_eq!(h.state().value("value-toggle"), Some(&json!(open)));
        }
    }
}

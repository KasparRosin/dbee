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
//! The bar: the name and type at its left, COPY and WRAP at its right (CANCEL and SAVE while
//! editing). Short of room it gives way in order ([`BarFit`]): the type is cut, then goes; the
//! name is cut, keeping [`VALUE_NAME_MIN`]; COPY and WRAP lose their words; last the name is cut
//! to `…`. A cut name or type reads whole on hover. The facts along the bottom say how many
//! lines, how big, whether the text was formatted as JSON and how much of a cut value shows
//! ([`Facts`]); no kind chip or note stands above the value.

use std::ops::Range;

use dbee_core::json::JsonToken;
use egui::text::{CCursor, LayoutJob};
use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};

use super::button::{BarButton, BoxButton};
use super::grid_cell::one_line;
use super::icon::Icon;
use super::painter;
use super::tooltip::Hint;
use crate::editors::size_text;
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, BUTTON_H, CARET_W, VALUE_BAR_GAP, VALUE_CARET_H, VALUE_EDIT_GAP, VALUE_FACT_GAP, VALUE_FACTS_H, VALUE_ICON_CELL_W,
    VALUE_INSET, VALUE_NAME_MIN,
};
use crate::theme::{Text, Tokens, tokens};
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

/// The facts along the bottom: the value's lines and its size, whether its text was formatted
/// as JSON, and for a cut value how long the whole is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub lines: usize,
    /// The value's size; for a cut value, the size of the part shown.
    pub bytes: usize,
    /// The text was pretty-printed from text the database doesn't call JSON (K2).
    pub formatted: bool,
    /// A cut value's whole length (N2).
    pub whole: Option<usize>,
}

impl Facts {
    /// What the facts say, as written (capitals are drawn): `Formatted as JSON`, `12 lines`,
    /// `137 B`; a cut value `1 line`, `First 10 MiB of 19.1 MiB`.
    pub fn words(self) -> Vec<String> {
        let mut words = Vec::with_capacity(3);
        if self.formatted {
            words.push("Formatted as JSON".to_owned());
        }
        words.push(format!("{} {}", thousands(self.lines as u64), if self.lines == 1 { "line" } else { "lines" }));
        words.push(match self.whole {
            Some(whole) => format!("First {} of {}", fact_size(self.bytes), fact_size(whole)),
            None => fact_size(self.bytes),
        });
        words
    }
}

/// A size as the facts say it: under 1 KiB in bytes (`137 B`), else as the app writes sizes
/// (`300 KiB`, `19.1 MiB`), a whole number of MiB without its `.0` (`10 MiB`).
pub fn fact_size(n: usize) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let size = size_text(n);
    match size.strip_suffix(".0 MiB") {
        Some(whole) => format!("{whole} MiB"),
        None => size,
    }
}

/// COPY's hover while it can copy a cell's value, and a row's.
pub const COPY_HOVER: &str = "Copy the whole value";
pub const COPY_ROW_HOVER: &str = "Copy the row as JSON";
/// WRAP's hover while it is off, and on.
pub const WRAP_HOVER: &str = "Wrap long lines";
pub const UNWRAP_HOVER: &str = "Stop wrapping long lines";

/// COPY as the bar draws it: whether it can copy, whether it just did (✓ COPIED), and its hover
/// (while off, why).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyButton<'a> {
    pub enabled: bool,
    pub copied: bool,
    pub hover: &'a str,
}

impl CopyButton<'static> {
    /// On, not just pressed.
    pub const ON: CopyButton<'static> = CopyButton { enabled: true, copied: false, hover: COPY_HOVER };
}

/// How the bar fits its width, from the widest form to the narrowest: each gives way one step
/// more than the one before it, and a narrower bar never takes a form before a wider one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BarFit {
    /// The name and the type whole, COPY and WRAP with their words.
    Whole,
    /// The type cut with `…`, keeping at least [`VALUE_NAME_MIN`] (or its whole width).
    TypeCut,
    /// The type gone, the name whole.
    TypeGone,
    /// The name cut with `…`, keeping at least [`VALUE_NAME_MIN`].
    NameCut,
    /// COPY and WRAP icons alone ([`VALUE_ICON_CELL_W`] wide), the name given their room, keeping
    /// at least [`VALUE_NAME_MIN`].
    Icons,
    /// The name cut to what is left, down to `…`.
    NameShort,
}

impl BarFit {
    /// COPY and WRAP keep their words.
    pub fn words(self) -> bool {
        self < BarFit::Icons
    }

    /// The type is drawn.
    pub fn ty(self) -> bool {
        self <= BarFit::TypeCut
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
    /// The value's facts, along the bottom of a state that shows a value.
    pub facts: Option<Facts>,
    /// COPY and WRAP in the bar, CANCEL and SAVE while editing.
    pub buttons: bool,
    /// COPY: whether it can copy, whether it just did, its hover.
    pub copy: CopyButton<'a>,
    /// WRAP is on.
    pub wrap: bool,
    /// Its own 1 px `line` down its left side; off in a panel that draws that line itself.
    pub edge: bool,
}

/// What [`Panel::show`] drew: where the value goes, the bar's buttons, how the bar fitted, and
/// the name and type whole when they were cut (their hover).
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
    pub fit: BarFit,
    /// The name and type whole (`name type`, as written), when the bar cut either.
    pub hover: Option<String>,
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
        let fit = self.fit(ui, bar.width());
        let mut shown = Shown { body, text: body, copy: None, wrap: None, cancel: None, save: None, fit, hover: None };
        let buttons_left = if self.buttons { self.buttons(ui, bar, fit, &mut shown) } else { bar.right() };
        shown.hover = self.title(ui, t, bar, buttons_left, fit);
        let top = body.top() + VALUE_INSET;
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

    /// The name and the type the bar draws: none with no cell, no type for a row.
    fn names(&self) -> (&str, &str) {
        match self.state {
            State::NoCell => ("", ""),
            State::Row => (self.title, ""),
            _ => (self.title, self.ty),
        }
    }

    /// The width of the bar's buttons at its right end, with their words or without.
    fn buttons_width(&self, ui: &Ui, words: bool) -> f32 {
        if !self.buttons {
            return 0.0;
        }
        if self.state == State::Editing {
            return BoxButton::outline("Cancel").width(ui) + VALUE_EDIT_GAP + BoxButton::primary("Save").width(ui) + VALUE_INSET;
        }
        if !words {
            return 2.0 * VALUE_ICON_CELL_W + VALUE_BAR_GAP;
        }
        copy_width(ui) + VALUE_BAR_GAP + wrap_button(true, false).width(ui)
    }

    /// How the bar fits `width` (the bar's, its left line included or not as drawn): the widest
    /// of [`BarFit`]'s forms whose name and type fit beside the buttons, 12 in from the bar's left
    /// and 12 before the buttons.
    pub fn fit(&self, ui: &Ui, width: f32) -> BarFit {
        let (name, ty) = self.names();
        let measure = |text: &str, style: Text| {
            if text.is_empty() { 0.0 } else { ui.painter().layout_job(style.job_as_written(text, Color32::PLACEHOLDER)).size().x }
        };
        let (name_w, ty_w) = (measure(name, Text::Header), measure(ty, Text::Data));
        let room = |words: bool| width - self.buttons_width(ui, words) - 2.0 * VALUE_INSET;
        let with_words = room(true);
        if !ty.is_empty() {
            if name_w + VALUE_BAR_GAP + ty_w <= with_words {
                return BarFit::Whole;
            }
            if name_w + VALUE_BAR_GAP + ty_w.min(VALUE_NAME_MIN) <= with_words {
                return BarFit::TypeCut;
            }
            if name_w <= with_words {
                return BarFit::TypeGone;
            }
        } else if name_w <= with_words {
            return BarFit::Whole;
        }
        let keep = name_w.min(VALUE_NAME_MIN);
        if keep <= with_words {
            return BarFit::NameCut;
        }
        if self.buttons && self.state != State::Editing && keep <= room(false) {
            return BarFit::Icons;
        }
        BarFit::NameShort
    }

    /// The name and type at the bar's left, cut as `fit` says to end 12 before `right`; with a
    /// hover saying them whole when either was cut, which it returns.
    fn title(&self, ui: &Ui, t: &Tokens, bar: Rect, right: f32, fit: BarFit) -> Option<String> {
        let (name, ty) = self.names();
        if name.is_empty() {
            return None;
        }
        let p = painter(ui);
        let left = bar.left() + VALUE_INSET;
        let room = (right - VALUE_INSET - left).max(0.0);
        let name = one_line(ui, name, Text::Header, t.text, room);
        let mut end = left + name.size().x;
        let cut = name.elided;
        p.galley(pos2(left, bar.center().y - name.size().y / 2.0), name, t.text);
        let mut ty_cut = !ty.is_empty() && !fit.ty();
        if fit.ty() && !ty.is_empty() {
            let type_left = end + VALUE_BAR_GAP;
            let ty = one_line(ui, ty, Text::Data, t.text_muted, right - VALUE_INSET - type_left);
            ty_cut = ty.elided;
            end = type_left + ty.size().x;
            p.galley(pos2(type_left, bar.center().y - ty.size().y / 2.0), ty, t.text_muted);
        }
        if !cut && !ty_cut {
            return None;
        }
        let whole = if ty.is_empty() { self.title.to_owned() } else { format!("{} {ty}", self.title) };
        let at = Rect::from_min_max(pos2(left, bar.top()), pos2(end.max(left), bar.bottom() - BAR_LINE));
        ui.interact(at, ui.id().with("dbee-value-title"), Sense::hover()).hint(whole.clone());
        Some(whole)
    }

    /// The bar's buttons at its right end, with their words as `fit` says; returns where they
    /// start.
    fn buttons(&self, ui: &mut Ui, bar: Rect, fit: BarFit, shown: &mut Shown) -> f32 {
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
        let words = fit.words();
        let wrap = wrap_button(words, self.wrap);
        let wrap_w = if words { wrap.width(ui) } else { VALUE_ICON_CELL_W };
        let copied = self.state == State::Copied || self.copy.copied;
        let copy_w = if words { copy_width(ui) } else { VALUE_ICON_CELL_W };
        let copy = if copied { BarButton::label(Icon::Check, "Copied").done(true) } else { BarButton::label(Icon::Copy, "Copy") };
        let copy = copy.over_line().min_width(copy_w).words(words);
        let wrap_at = Rect::from_min_size(pos2(right - wrap_w, bar.top()), vec2(wrap_w, BAR_CELL_H));
        let copy_at = Rect::from_min_size(pos2(wrap_at.left() - VALUE_BAR_GAP - copy_w, bar.top()), vec2(copy_w, BAR_CELL_H));
        let enabled = self.copy.enabled && self.state != State::NoCell;
        let r = ui.add_enabled_ui(enabled, |ui| ui.put(copy_at, copy)).inner;
        shown.copy = Some(if enabled { r.hint(self.copy.hover) } else { r.hint_disabled(self.copy.hover) });
        shown.wrap = Some(ui.put(wrap_at, wrap).hint(if self.wrap { UNWRAP_HOVER } else { WRAP_HOVER }));
        copy_at.left()
    }
}

/// COPY's cell with its words: as wide as ✓ COPIED, so nothing moves when it changes.
fn copy_width(ui: &Ui) -> f32 {
    BarButton::label(Icon::Check, "Copied").width(ui)
}

/// WRAP, with its word or without, on or off.
fn wrap_button(words: bool, on: bool) -> BarButton<'static> {
    BarButton::label(Icon::Wrap, "Wrap").over_line().on(on).words(words)
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
                    let words: Vec<String> = facts.words().into_iter().map(|w| w.to_uppercase()).collect();
                    let (lines, size) = (words[0].clone(), words[1].clone());
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
            let in_body: Vec<_> = texts(shapes).into_iter().filter(|(_, r, _)| body.contains(r.center())).collect();
            assert_eq!(in_body.len(), 1, "{theme:?}: the note alone (today's second line goes): {in_body:?}");
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
            // No kind chip (K2): the type once, never `JSONB`.
            assert!(
                all.iter().filter(|w| *w == "jsonb").count() == 1 && !all.iter().any(|w| w == "JSONB"),
                "{theme:?}: the type alone, as written: {all:?}"
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
        assert_eq!(keys(&head), ["bytes", "column", "facts", "label", "note", "null", "row", "truncated", "type"]);
        let lines = json_view.text.split('\n').count();
        assert_eq!(
            head,
            json!({ "column": "billing", "row": null, "type": "jsonb", "label": "jsonb", "null": false, "bytes": raw.len(), "truncated": false, "note": null, "facts": format!("{lines} lines {} B", raw.len()) })
        );
        let v = h.state().tags.value("value-text").unwrap().clone();
        assert_eq!(keys(&v), ["cut", "firstRow", "highlighted", "rows", "selection", "text"]);
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
        assert_eq!(head["facts"], "1 line First 10 MiB of 19.1 MiB");
        assert_eq!(h.state().tags.value("value-text").unwrap()["cut"], true);
        assert!(drawn(&h.output().shapes, "Showing the first 10.0 MiB of 19.1 MiB.").is_none(), "the note no longer drawn (N2)");
        // NULL: no facts.
        let h = app(Theme::Light, size, Some(view("note", "text", None)));
        assert_eq!(h.state().tags.value("value-panel").unwrap()["facts"], Json::Null);
        // A whole row: its number, its JSON coloured, `Row <n>` in the bar.
        let object = r#"{"id": 3, "doc": {"a": 1}}"#.to_string();
        let len = object.len();
        let h = app(Theme::Light, size, Some(Arc::new(ValueView::of_row(1_203, object, len))));
        let head = h.state().tags.value("value-panel").unwrap().clone();
        let lines = 6;
        assert_eq!(
            head,
            json!({ "column": "", "row": 1_203, "type": "", "label": null, "null": false, "bytes": len, "truncated": false, "note": null, "facts": format!("{lines} lines {len} B") })
        );
        let v = h.state().tags.value("value-text").unwrap();
        assert_eq!((v["text"].as_str(), v["highlighted"].clone()), (Some("{\n  \"id\": 3,\n  \"doc\": {\n    \"a\": 1\n  }\n}"), json!(true)));
        assert!(drawn(&h.output().shapes, "Row 1,203").is_some(), "the title: {:?}", texts(&h.output().shapes));
    }

    /// What `panel` drew into `at`: the shapes and what it says it drew, with COPIED's width.
    fn shown_in(theme: Theme, panel: &Panel<'_>, at: Rect) -> (Vec<ClippedShape>, Shown, f32) {
        let mut seen = None;
        let shapes = painted_in(vec2(at.right() + 20.0, at.bottom() + 20.0), theme, |ui| {
            seen = Some((panel.show(ui, at), copy_width(ui)));
        });
        let (shown, copied) = seen.unwrap();
        (shapes, shown, copied)
    }

    #[test]
    fn the_bar_matches_b1() {
        // B1: COPY then WRAP at the bar's right end, a 1 px `line` at the left of each; COPY's
        // cell as wide as ✓ COPIED's, so nothing moves when it reads COPIED (`text_muted`); with
        // no cell COPY is off (`text_faint`).
        for (theme, t) in both() {
            let mut first = None;
            for (state, copied) in [(State::Json, false), (State::Json, true), (State::Copied, false), (State::Row, true), (State::NoCell, false)] {
                let at_ = format!("{theme:?} {state:?} copied {copied}");
                let base = value_samples()[if state == State::Row { 7 } else { 0 }].panel.clone();
                let panel = Panel { state, copy: CopyButton { copied, ..CopyButton::ON }, ..base };
                let (shapes, shown, copied_w) = shown_in(theme, &panel, AT);
                let (copy, wrap) = (shown.copy.unwrap().rect, shown.wrap.unwrap().rect);
                assert!(close(wrap.right(), AT.right()) && close(wrap.top(), AT.top()) && close(wrap.height(), 35.0), "{at_}: WRAP {wrap:?}");
                assert!(close(copy.right(), wrap.left() - 6.0) && close(copy.width(), copied_w), "{at_}: COPY {copy:?}, COPIED {copied_w}");
                let fills = fills(&shapes);
                for cell in [copy, wrap] {
                    let line = Rect::from_min_size(cell.min, vec2(1.0, 35.0));
                    assert!(
                        fills.iter().any(|(r, c)| *c == t.line && close(r.left(), line.left()) && close(r.width(), 1.0)),
                        "{at_}: a line before {cell:?}"
                    );
                }
                let first = *first.get_or_insert(copy);
                assert_eq!(copy, first, "{at_}: nothing moves");
                let (word, colour) = match (state, copied) {
                    (State::NoCell, _) => ("COPY", t.text_faint),
                    (State::Copied, _) | (_, true) => ("COPIED", t.text_muted),
                    _ => ("COPY", t.text),
                };
                assert_eq!(drawn(&shapes, word).map(|d| d.1), Some(colour), "{at_}: {word}");
            }
        }
    }

    #[test]
    fn facts_read_as_drawn() {
        let mib = 1024 * 1024;
        let f = |lines, bytes, formatted, whole| Facts { lines, bytes, formatted, whole };
        assert_eq!(f(12, 137, false, None).words(), ["12 lines", "137 B"]);
        assert_eq!(f(12, 137, true, None).words(), ["Formatted as JSON", "12 lines", "137 B"]);
        assert_eq!(f(1, 10 * mib, false, Some(20_000_000)).words(), ["1 line", "First 10 MiB of 19.1 MiB"]);
        assert_eq!(f(1_250, 300 * 1024, false, None).words(), ["1,250 lines", "300 KiB"]);
        assert_eq!((fact_size(1023), fact_size(10 * mib), fact_size(20 * mib + 1)), ("1023 B".into(), "10 MiB".into(), "20 MiB".into()));
        assert_eq!(fact_size(20_000_000), "19.1 MiB");
        // Drawn in capitals along the bottom, 12 apart, `FORMATTED AS JSON` first.
        for (theme, t) in both() {
            let panel = Panel { facts: Some(f(12, 137, true, None)), ..value_samples()[0].panel.clone() };
            let (shapes, ..) = shown_in(theme, &panel, AT);
            let mut x = AT.left() + 13.0;
            for word in ["FORMATTED AS JSON", "12 LINES", "137 B"] {
                let (r, c, font, _) = drawn(&shapes, word).unwrap_or_else(|| panic!("{theme:?}: {word} drawn"));
                assert_eq!((c, font), (t.text_muted, Text::Label.font()), "{theme:?}: {word}");
                assert!(close(r.left(), x) && close(r.center().y, AT.top() + 357.0), "{theme:?}: {word} at {r:?}, want x {x}");
                x = r.right() - Text::Label.spacing() + 12.0;
            }
        }
        // No chip or note above the value: the first row 12 under the bar in every state.
        let size = vec2(400.0, 500.0);
        for (what, v) in [
            ("jsonb", view("meta", "jsonb", Some(r#"{"a": 1}"#))),
            ("text holding JSON", view("raw", "text", Some(r#"{"a": 1}"#))),
            ("cut", view("note", "text", Some(&"x".repeat(20_000_000)))),
            ("NULL", view("note", "text", None)),
        ] {
            let h = app(Theme::Light, size, Some(v));
            let (panel, text) = (h.state().tags.rect("value-panel").unwrap(), h.state().tags.rect("value-text").unwrap());
            assert!(close(text.top(), panel.top() + BAR_H + 12.0), "{what}: the value at {text:?} in {panel:?}");
            let all: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
            assert!(!all.iter().any(|w| w.contains("CONTAINING") || w.starts_with("Showing")), "{what}: no chip or note: {all:?}");
        }
        let h = app(Theme::Light, size, Some(view("raw", "text", Some(r#"{"a": 1}"#))));
        assert_eq!(h.state().tags.value("value-panel").unwrap()["facts"], "Formatted as JSON 3 lines 8 B");
        assert!(drawn(&h.output().shapes, "FORMATTED AS JSON").is_some());
    }

    #[test]
    fn names_keep_their_case_in_the_panel_bar() {
        let ty = "enum('small','Large')";
        for (theme, t) in both() {
            let panel = Panel { title: "Notes", ty, ..value_samples()[1].panel.clone() };
            let wide = Rect::from_min_size(AT.min, vec2(460.0, AT.height()));
            let (shapes, shown, _) = shown_in(theme, &panel, wide);
            assert_eq!(drawn(&shapes, "Notes").map(|d| d.1), Some(t.text), "{theme:?}: the name as written");
            assert_eq!(drawn(&shapes, ty).map(|d| d.1), Some(t.text_muted), "{theme:?}: the type as written");
            assert_eq!((shown.fit, shown.hover), (BarFit::Whole, None), "{theme:?}: whole, no hover");
            let narrow = Rect::from_min_size(AT.min, vec2(240.0, AT.height()));
            let (shapes, shown, _) = shown_in(theme, &panel, narrow);
            assert_eq!(shown.hover.as_deref(), Some("Notes enum('small','Large')"), "{theme:?}: whole on hover, as written");
            assert!(drawn(&shapes, "Notes").is_some() && drawn(&shapes, "NOTES").is_none(), "{theme:?}: {:?}", texts(&shapes));
            // The value keeps its case too.
            let h = app(theme, vec2(256.0, 500.0), Some(view("Notes", ty, Some("MiXeD"))));
            assert_eq!(drawn(&h.output().shapes, "MiXeD").map(|d| d.1), Some(t.text), "{theme:?}: the value as written");
            let facts = h.state().tags.value("value-panel").unwrap()["facts"].as_str().unwrap().to_owned();
            assert!(!facts.contains("Notes") && !facts.contains("MiXeD"), "{theme:?}: no data in the facts: {facts}");
        }
    }

    #[test]
    fn the_panel_bar_gives_way_in_order() {
        let panel = Panel { title: "delivery_instructions", ty: "character varying(200)", ..value_samples()[0].panel.clone() };
        let mut last = BarFit::Whole;
        let mut seen = Vec::new();
        for w in (120..=600).rev().step_by(5) {
            let at = Rect::from_min_size(AT.min, vec2(w as f32, AT.height()));
            let (shapes, shown, _) = shown_in(Theme::Light, &panel, at);
            let at_ = format!("{w} wide, {:?}", shown.fit);
            assert!(shown.fit >= last, "{at_}: after {last:?}");
            last = shown.fit;
            if seen.last() != Some(&shown.fit) {
                seen.push(shown.fit);
            }
            let (copy, wrap) = (shown.copy.unwrap().rect, shown.wrap.unwrap().rect);
            assert!(at.contains_rect(copy) && at.contains_rect(wrap) && copy.right() <= wrap.left(), "{at_}: COPY {copy:?} and WRAP {wrap:?}");
            let words = shown.fit.words();
            assert_eq!(drawn(&shapes, "COPY").is_some(), words, "{at_}: COPY's word");
            assert_eq!(drawn(&shapes, "WRAP").is_some(), words, "{at_}: WRAP's word");
            if !words {
                assert!(close(copy.width(), 36.0) && close(wrap.width(), 36.0), "{at_}: icons alone, 36 wide");
            }
            // The name and type end 12 before COPY; nothing past the bar.
            let bar = Rect::from_min_max(at.min, pos2(at.right(), at.top() + 35.0));
            for (word, r, _) in texts(&shapes).into_iter().filter(|(_, r, _)| bar.contains(r.center()) && r.center().x < copy.left()) {
                assert!(r.right() <= copy.left() - 12.0 + 0.5 && r.left() >= at.left(), "{at_}: {word:?} at {r:?}, COPY at {copy:?}");
            }
            let ty_drawn = texts(&shapes).iter().any(|(w, r, _)| bar.contains(r.center()) && w.starts_with("char"));
            assert_eq!(ty_drawn, shown.fit.ty(), "{at_}: the type");
            assert_eq!(shown.hover.is_some(), shown.fit != BarFit::Whole, "{at_}: whole on hover once cut");
        }
        assert_eq!(
            seen,
            [BarFit::Whole, BarFit::TypeCut, BarFit::TypeGone, BarFit::NameCut, BarFit::Icons, BarFit::NameShort],
            "every form, in order"
        );
    }
}

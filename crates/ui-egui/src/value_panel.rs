//! The value panel: the right side of the workspace, showing the whole value of the selected cell
//! (the grid's focus cell: the one last clicked or moved to).
//!
//! **What it shows** is the engine's value view (`Session::value_view_shared`): the value as the
//! grid shows it, a staged one included, pretty-printed when it is JSON and cut to
//! `VALUE_VIEW_MAX_BYTES` (10 MiB). The header names the column, its real type and, when one
//! applies, what JSON it is (`jsonb`, `json`, `text containing JSON`, `JSON string containing
//! JSON`); a cut value says how much of it shows ([`note`]). With no cell selected it says how
//! to fill it. NULL shows as a dim `NULL`, with no text to select.
//!
//! **The text** is read-only, in a monospace font, wrapped at the panel's width by characters
//! ([`Rows`]: a row ends at a line break or after as many characters as fit). Only the rows in
//! view are laid out, so a 10 MiB text costs a frame what a short one does: the rows are found
//! from marks kept every [`MARK_EVERY`] rows, which one pass over the text makes when the value
//! or the width changes. JSON that parsed (or the start of one that was cut) gets syntax colours
//! from `json::highlight` ([`spans`], theme [`crate::theme::Syntax`]); up to
//! [`SYNC_COLOUR_BYTES`] they are worked out in the frame, above that on a thread while the text
//! shows uncoloured.
//!
//! **Selecting.** A press places the caret and takes the keyboard, a drag or a shift-click
//! selects from there (scrolling when the drag leaves the text), a double-click selects a word.
//! With text selected, Ctrl+C copies it, not the grid's cells ([`take_keys`], before the
//! shortcuts); while the panel has the keyboard, Ctrl+A selects all of its text. A press outside
//! the text, another value, or hiding the panel ends the selection.
//!
//! **The panel** starts [`DEFAULT_WIDTH`] wide, less in a narrow window ([`start_width`]), is
//! resizable from its left edge, and is hidden or shown with `ui.toggleValuePanel` (the View
//! menu, and `value-toggle` at the right end of the status bar). Its width (egui's
//! panel state) and whether it shows (`View::value_panel`) last for the session, in memory only.
//! It is drawn after the tab's body ([`reserve`], then [`fill`]), so a click on a cell shows its
//! value in the same frame.
//!
//! Automation ids: `value-panel` (value `{column, type, label, null, bytes, truncated, note}`,
//! `label` the JSON label or null, `bytes` the whole value's length, `note` what a cut value
//! says; `null` when no cell is selected), `value-text` (value `{text, cut, selection,
//! highlighted, rows, firstRow}`: the text's first [`TAG_BYTES`] bytes and whether there is more,
//! the selection as `[start, end]` byte offsets or null, whether the JSON colours show, the
//! number of rows and the first row in view) and `value-toggle` (value: whether the panel shows).

use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use dbee_core::json::{self, JsonKind, JsonToken};
use dbee_engine::{Screen, ValueView};
use egui::containers::scroll_area::{DragScroll, ScrollSource};
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    Align, Color32, CornerRadius, Event, FontId, Frame, Galley, Id, Key, Label, Layout, Modifiers, Pos2, Rect, Response, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, UiBuilder, pos2, vec2,
};
use serde_json::{Value as Json, json};

use crate::DbeeApp;
use crate::automation::Tags;
use crate::connections::line_galley;
use crate::editors::size_text;
use crate::theme::{RADIUS, Syntax, TOKENS, syntax};
use crate::{icons, shortcuts};

/// The automation ids.
pub const PANEL: &str = "value-panel";
pub const TEXT: &str = "value-text";
pub const TOGGLE: &str = "value-toggle";

/// The width the panel starts at, and how narrow and wide it can be made.
pub const DEFAULT_WIDTH: f32 = 360.0;
pub const MIN_WIDTH: f32 = 240.0;
pub const MAX_WIDTH: f32 = 960.0;
/// The height of a row of text, and the text's inset from the left of its box.
pub const LINE_H: f32 = 18.0;
pub const PAD: f32 = 8.0;
/// The text's size, in the monospace font.
pub const FONT_SIZE: f32 = 12.5;
/// How much of the text the `value-text` tag carries.
pub const TAG_BYTES: usize = 4 * 1024;
/// The longest JSON text coloured in the frame that shows it; longer is coloured on a thread.
pub const SYNC_COLOUR_BYTES: usize = 256 * 1024;
/// Rows between two marks (see [`Rows`]): finding a row walks at most this many.
pub const MARK_EVERY: usize = 128;

/// The panel's inset, and the room kept at the text's right for the scroll bar.
const MARGIN: f32 = 8.0;
const BAR_ROOM: f32 = 12.0;
/// How often a panel waiting for its colours looks again.
const COLOUR_POLL: Duration = Duration::from_millis(30);
/// The most a drag past the text's edge scrolls in a frame.
const DRAG_SCROLL: f32 = 40.0;

/// The text's id, which holds the keyboard after a press on it.
fn text_id() -> Id {
    Id::new("dbee-value-text")
}

/// The longest start of `text` of at most `max` bytes that ends on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default()
}

// ---- rows ---------------------------------------------------------------------------------------

/// The rows a text wraps into at a width of so many characters: a row ends at a line break
/// (which belongs to no row) or after `width` characters, so a text of `n` line breaks has at
/// least `n + 1` rows, the empty text one. Only marks are kept, the start of every
/// [`MARK_EVERY`]-th row; a row is found by walking from the mark before it.
#[derive(Clone, Debug)]
pub struct Rows {
    width: usize,
    count: usize,
    marks: Vec<usize>,
}

impl Rows {
    /// The rows of `text` at `width` characters (0 counts as 1): one pass over the text.
    pub fn new(text: &str, width: usize) -> Rows {
        let width = width.max(1);
        let mut rows = Rows { width, count: 0, marks: Vec::new() };
        let mut start = 0;
        loop {
            let end = next_break(text, start);
            rows.line(text, start..end.unwrap_or(text.len()));
            match end {
                Some(at) => start = at + 1,
                None => break,
            }
        }
        rows
    }

    /// The rows of the line `text[line]`.
    fn line(&mut self, text: &str, line: Range<usize>) {
        let start = line.start;
        if line.len() <= self.width {
            // No more characters than bytes: one row.
            self.push(start);
            return;
        }
        let line = text.get(line).unwrap_or_default();
        if line.is_ascii() {
            let mut at = 0;
            while at < line.len() {
                self.push(start + at);
                at = at.saturating_add(self.width);
            }
        } else {
            // Characters left in the row: a count down, cheaper than dividing at each one.
            let mut left = 0;
            for (i, _) in line.char_indices() {
                if left == 0 {
                    self.push(start + i);
                    left = self.width;
                }
                left -= 1;
            }
        }
    }

    fn push(&mut self, at: usize) {
        if self.count.is_multiple_of(MARK_EVERY) {
            self.marks.push(at);
        }
        self.count += 1;
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// The bytes of row `row` of `text` (the text the rows were made of), without its line
    /// break; `None` past the last row.
    pub fn range(&self, text: &str, row: usize) -> Option<Range<usize>> {
        self.ranges(text, row, 1).into_iter().next()
    }

    /// Rows `from..from + n` of `text` (fewer at its end), as byte ranges.
    pub fn ranges(&self, text: &str, from: usize, n: usize) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let to = from.saturating_add(n).min(self.count);
        let Some(&mark) = self.marks.get(from / MARK_EVERY) else { return out };
        let mut row = from - from % MARK_EVERY;
        let mut at = Some(mark);
        while let Some(start) = at
            && row < to
        {
            let (end, next) = step(text, start, self.width);
            if row >= from {
                out.push(start..end);
            }
            at = next;
            row += 1;
        }
        out
    }
}

/// The first line break of `text` at or after byte `from` (a character boundary). A short line
/// is found by looking at its bytes, a long one by `str::find`, which is fast over many bytes
/// but costs more to start: so ten million short lines cost about what one long one does.
fn next_break(text: &str, from: usize) -> Option<usize> {
    const NEAR: usize = 64;
    let bytes = text.as_bytes();
    let near = bytes.get(from..bytes.len().min(from.saturating_add(NEAR))).unwrap_or_default();
    if let Some(i) = near.iter().position(|&b| b == b'\n') {
        return Some(from + i);
    }
    if from.saturating_add(near.len()) >= bytes.len() {
        return None;
    }
    text.get(from..)?.find('\n').map(|i| from + i)
}

/// The row of `text` that starts at `start`, `width` characters at most: where it ends, and
/// where the next row starts (`None` when it is the last).
fn step(text: &str, start: usize, width: usize) -> (usize, Option<usize>) {
    let rest = text.get(start..).unwrap_or_default();
    let limit = rest.char_indices().nth(width).map_or(rest.len(), |(i, _)| i);
    match rest.get(..limit).unwrap_or_default().find('\n') {
        Some(i) => (start + i, Some(start + i + 1)),
        None if limit >= rest.len() => (start + rest.len(), None),
        // A line of exactly so many rows: its line break ends this one.
        None if rest.as_bytes().get(limit) == Some(&b'\n') => (start + limit, Some(start + limit + 1)),
        None => (start + limit, Some(start + limit)),
    }
}

/// The word at byte `at` of `text`: its letters, digits and underscores around it, or the one
/// other character there; empty at the end.
pub fn word_at(text: &str, at: usize) -> Range<usize> {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at = at.saturating_sub(1);
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let (before, after) = (text.get(..at).unwrap_or_default(), text.get(at..).unwrap_or_default());
    let Some(c) = after.chars().next() else { return at..at };
    if !is_word(c) {
        return at..at + c.len_utf8();
    }
    let start = before.char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = after.char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// What a cut value says: how much of it shows, and for JSON that it is not formatted.
pub fn note(view: &ValueView) -> Option<String> {
    if !view.truncated {
        return None;
    }
    let shown = format!("Showing the first {} of {}.", size_text(view.text.len()), size_text(view.byte_len));
    Some(if view.kind == JsonKind::NotJson { shown } else { format!("{shown} Too long to format.") })
}

// ---- colours ------------------------------------------------------------------------------------

/// A coloured token of a JSON text: its bytes (the text is at most 10 MiB) and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub token: JsonToken,
}

/// The tokens `json::highlight` finds in `text`, without the punctuation (the colour of
/// everything between tokens), in order.
pub fn spans(text: &str) -> Vec<Span> {
    json::highlight(text)
        .into_iter()
        .filter(|(_, token)| *token != JsonToken::Punct)
        .filter_map(|(r, token)| Some(Span { start: u32::try_from(r.start).ok()?, end: u32::try_from(r.end).ok()?, token }))
        .collect()
}

/// The text's colours.
enum Colours {
    /// Not JSON, or JSON that did not parse: one colour.
    Plain,
    /// Being worked out on a thread.
    Pending(Receiver<Vec<Span>>),
    Ready(Vec<Span>),
}

/// Whether `view` gets JSON colours: JSON that was pretty-printed, or the start of a cut one.
fn coloured(view: &ValueView) -> bool {
    view.kind != JsonKind::NotJson && (view.pretty || view.truncated)
}

/// The colours of `view`'s text: in this frame when it is short, else from a thread.
fn colours_of(view: &Arc<ValueView>) -> Colours {
    if !coloured(view) {
        return Colours::Plain;
    }
    if view.text.len() <= SYNC_COLOUR_BYTES {
        return Colours::Ready(spans(&view.text));
    }
    let (tx, rx) = mpsc::channel();
    let shown = Arc::clone(view);
    match std::thread::Builder::new().name("dbee-value-colours".into()).spawn(move || {
        // The panel may have moved on: then nobody listens.
        let _ = tx.send(spans(&shown.text));
    }) {
        Ok(_) => Colours::Pending(rx),
        Err(e) => {
            log::debug!("value panel: no thread for the colours: {e}");
            Colours::Plain
        }
    }
}

impl Colours {
    /// Takes the thread's colours once they are there.
    fn poll(&mut self) {
        if let Colours::Pending(rx) = self {
            match rx.try_recv() {
                Ok(spans) => *self = Colours::Ready(spans),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => *self = Colours::Plain,
            }
        }
    }
}

// ---- state --------------------------------------------------------------------------------------

/// The value shown, wrapped at a width, with its colours.
struct Doc {
    view: Arc<ValueView>,
    rows: Rows,
    colours: Colours,
    /// Not drawn yet: it starts at its top.
    fresh: bool,
}

/// What the panel keeps between frames: the value it shows (the engine's view, shared), its rows
/// and colours, and the selection in it.
#[derive(Default)]
pub struct PanelState {
    doc: Option<Doc>,
    /// Where the selection started and where it ends now, as byte offsets into the text; empty
    /// when they are the same.
    selection: Option<(usize, usize)>,
}

/// Leaves the value out: it is a cell's.
impl std::fmt::Debug for PanelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelState")
            .field("bytes", &self.doc.as_ref().map(|d| d.view.text.len()))
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

impl PanelState {
    /// Forgets the value and the selection.
    pub fn clear(&mut self) {
        self.doc = None;
        self.selection = None;
    }

    /// Ends the selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// The selection, as a byte range of the text shown, when it is not empty.
    pub fn selection(&self) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        let (start, end) = (a.min(b), a.max(b));
        (start < end && self.doc.is_some()).then_some(start..end)
    }

    /// The selected text, when some is selected.
    pub fn selected_text(&self) -> Option<&str> {
        self.doc.as_ref()?.view.text.get(self.selection()?)
    }

    /// Selects all of the text shown.
    pub fn select_all(&mut self) {
        if let Some(doc) = &self.doc {
            self.selection = Some((0, doc.view.text.len()));
        }
    }

    /// Whether it shows `view` (that one, not an equal one).
    fn shows(&self, view: &Arc<ValueView>) -> bool {
        self.doc.as_ref().is_some_and(|d| Arc::ptr_eq(&d.view, view))
    }

    /// Makes it show `view` wrapped at `width` characters: another value starts afresh (no
    /// selection, at its top, its colours to work out); another width wraps it again.
    fn prepare(&mut self, view: &Arc<ValueView>, width: usize) {
        if !self.shows(view) {
            self.selection = None;
            let rows = Rows::new(&view.text, width);
            self.doc = Some(Doc { view: Arc::clone(view), rows, colours: colours_of(view), fresh: true });
        } else if let Some(doc) = self.doc.as_mut()
            && doc.rows.width() != width.max(1)
        {
            doc.rows = Rows::new(&doc.view.text, width);
        }
    }
}

// ---- the panel ----------------------------------------------------------------------------------

/// The width the panel starts at in a window `window` wide: [`DEFAULT_WIDTH`], less in a narrow
/// window (about a quarter of it), so the table tab keeps room for its filter row.
pub fn start_width(window: f32) -> f32 {
    if window.is_finite() { (window * 0.27).clamp(MIN_WIDTH, DEFAULT_WIDTH) } else { DEFAULT_WIDTH }
}

/// Before the tab's body: the panel's place at the right, when it shows. Returns where its
/// contents go ([`fill`] draws them once the body has run).
pub(crate) fn reserve(app: &DbeeApp, ui: &mut egui::Ui) -> Option<Rect> {
    if !app.view.value_panel {
        return None;
    }
    let frame = Frame::new().fill(TOKENS.panel);
    let start = start_width(ui.ctx().content_rect().width());
    let panel = egui::Panel::right("dbee-value-panel").resizable(true).default_size(start).size_range(MIN_WIDTH..=MAX_WIDTH).frame(frame);
    let out = panel.show(ui, |ui| {
        // The whole width: the panel keeps the size its contents take.
        let rect = ui.max_rect();
        ui.expand_to_include_rect(rect);
        rect
    });
    Some(out.inner)
}

/// After the tab's body: the panel's contents in `rect`, for the value the session shows now.
pub(crate) fn fill(app: &mut DbeeApp, ui: &mut egui::Ui, rect: Rect) {
    let view = app.session.value_view_shared();
    let DbeeApp { view: v, tags, .. } = app;
    let mut child = ui.new_child(UiBuilder::new().id_salt("dbee-value-contents").max_rect(rect).layout(Layout::top_down(Align::Min)));
    contents(&mut child, view.as_ref(), &mut v.panel, tags);
}

/// The toggle at the right end of the status bar; true when it was clicked.
pub(crate) fn toggle(ui: &mut egui::Ui, open: bool, tags: &mut Tags) -> bool {
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 20.0), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), t.hover);
    }
    let colour = if open { t.accent } else { t.text_muted };
    icons::side_panel(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), colour, open);
    let r = r.on_hover_text(if open { "Hide the value panel" } else { "Show the value panel" });
    tags.tag_value(TOGGLE, &r, open);
    r.clicked()
}

/// Step 4 of the frame, before the shortcuts, unless a text field or a text editor has the
/// keyboard: with text selected in the panel, the copy (Ctrl+C, and Ctrl+Shift+C) is the panel's;
/// while the panel has the keyboard, so is Ctrl+A.
pub(crate) fn take_keys(app: &mut DbeeApp, ctx: &egui::Context) {
    let shown = app.session.value_view_shared();
    let state = &mut app.view.panel;
    let showing = app.view.value_panel && app.session.state().screen == Screen::Workspace;
    // The selection is in the value the panel drew last; another one since has none.
    if !showing || !shown.as_ref().is_some_and(|v| state.shows(v)) {
        state.clear_selection();
        return;
    }
    let focused = ctx.memory(|m| m.has_focus(text_id()));
    let selected = state.selection().is_some();
    let (all, copy) = ctx.input_mut(|i| {
        let held = i.modifiers;
        let all = focused && shortcuts::take(&mut i.events, Modifiers::COMMAND, Key::A, held);
        let before = i.events.len();
        if selected {
            i.events.retain(|e| !matches!(e, Event::Copy));
        }
        (all, i.events.len() < before)
    });
    if all {
        state.select_all();
    }
    if copy && let Some(text) = state.selected_text() {
        let text = text.to_string();
        app.copy_text(text);
    }
}

/// The panel's contents for `view` (`None`: no cell is selected), in all of `ui`.
pub fn contents(ui: &mut egui::Ui, view: Option<&Arc<ValueView>>, state: &mut PanelState, tags: &mut Tags) {
    let t = TOKENS;
    let full = ui.max_rect();
    let panel = ui.interact(full, ui.id().with("dbee-value-panel"), Sense::hover());
    let mut ui = ui.new_child(UiBuilder::new().max_rect(full.shrink(MARGIN)).layout(Layout::top_down(Align::Min)));
    let Some(view) = view else {
        state.clear();
        tags.tag_value(PANEL, &panel, Json::Null);
        ui.add_space(full.height() * 0.3);
        ui.vertical_centered(|ui| {
            ui.add(Label::new(RichText::new("No cell is selected").size(14.0).color(t.text_muted)));
            ui.add_space(4.0);
            ui.add(Label::new(RichText::new("Select a cell to see its whole value here.").size(12.0).color(t.text_muted)).wrap());
        });
        return;
    };
    header(&mut ui, view);
    let note = note(view);
    if let Some(n) = &note {
        ui.add_space(2.0);
        ui.add(Label::new(RichText::new(n.as_str()).size(12.0).color(t.text_muted)).wrap());
    }
    ui.add_space(6.0);
    body(&mut ui, view, state, tags);
    let label = (!view.kind_label.is_empty()).then_some(view.kind_label);
    let head = json!({
        "column": view.column,
        "type": view.type_label,
        "label": label,
        "null": view.is_null,
        "bytes": view.byte_len,
        "truncated": view.truncated,
        "note": note,
    });
    tags.tag_value(PANEL, &panel, head);
}

/// The column's name and type, and the JSON label when one applies.
fn header(ui: &mut egui::Ui, view: &ValueView) {
    let t = TOKENS;
    let width = ui.available_width();
    let name = line_galley(ui, &view.column, FontId::proportional(14.0), t.text, width * 0.7);
    let name_w = name.size().x;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    ui.painter().galley(pos2(rect.left(), rect.center().y - name.size().y / 2.0), name, t.text);
    let left = rect.left() + name_w + 8.0;
    if rect.right() - left > 12.0 {
        let ty = line_galley(ui, &view.type_label, FontId::proportional(12.0), t.text_muted, rect.right() - left);
        ui.painter().galley(pos2(left, rect.center().y - ty.size().y / 2.0 + 1.0), ty, t.text_muted);
    }
    if !view.kind_label.is_empty() {
        ui.add_space(2.0);
        let label = line_galley(ui, view.kind_label, FontId::proportional(11.5), t.accent, width - 12.0);
        let (pill, _) = ui.allocate_exact_size(vec2(label.size().x + 12.0, 18.0), Sense::hover());
        ui.painter().rect_filled(pill, CornerRadius::same(RADIUS.saturating_mul(2)), t.selection);
        ui.painter().galley(pos2(pill.left() + 6.0, pill.center().y - label.size().y / 2.0), label, t.accent);
    }
}

/// A dim, italic word in the text's box where there is no text (NULL, an empty value).
fn placeholder(ui: &mut egui::Ui, area: Rect, word: &str) -> Response {
    let t = TOKENS;
    let format = TextFormat { font_id: FontId::monospace(FONT_SIZE), color: t.text_muted, italics: true, ..TextFormat::default() };
    let galley = ui.painter().layout_job(LayoutJob::single_section(word.to_string(), format));
    // Where the first row of text would be.
    ui.painter().galley(pos2(area.left() + PAD, area.top() + (LINE_H - galley.size().y) / 2.0), galley, t.text_muted);
    ui.interact(area, text_id(), Sense::hover())
}

/// One row as it is drawn.
struct Drawn {
    range: Range<usize>,
    galley: Arc<Galley>,
    /// The top of the row, in window points.
    top: f32,
}

/// The text's box: the rows in view, the selection, and the pointer's part in it.
fn body(ui: &mut egui::Ui, view: &Arc<ValueView>, state: &mut PanelState, tags: &mut Tags) {
    let t = TOKENS;
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect(rect, CornerRadius::same(RADIUS), t.card, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let area = rect.shrink(1.0);
    let mut tag = |resp: &Response, selection: Option<Range<usize>>, highlighted: bool, rows: usize, first: usize| {
        let shown = cut(&view.text, TAG_BYTES);
        let value = json!({
            "text": shown,
            "cut": shown.len() < view.text.len(),
            "selection": selection.map(|s| [s.start, s.end]),
            "highlighted": highlighted,
            "rows": rows,
            "firstRow": first,
        });
        tags.tag_value(TEXT, resp, value);
    };
    if view.is_null || view.text.is_empty() {
        state.clear();
        let r = placeholder(ui, area, if view.is_null { "NULL" } else { "empty" });
        tag(&r, None, false, usize::from(!view.is_null), 0);
        return;
    }
    let font = FontId::monospace(FONT_SIZE);
    let glyph = ui.fonts_mut(|f| f.glyph_width(&font, 'x'));
    let glyph = if glyph.is_finite() && glyph > 0.0 { glyph } else { FONT_SIZE * 0.6 };
    let columns = ((area.width() - 2.0 * PAD - BAR_ROOM) / glyph).floor().max(1.0) as usize;
    state.prepare(view, columns);
    let PanelState { doc, selection } = state;
    let Some(doc) = doc.as_mut() else { return };
    doc.colours.poll();
    if matches!(doc.colours, Colours::Pending(_)) {
        ui.ctx().request_repaint_after(COLOUR_POLL);
    }
    let text = doc.view.text.as_str();
    let total = doc.rows.count();
    let palette = syntax(ui.visuals().dark_mode);
    let mut scroll = ScrollArea::vertical()
        .id_salt("dbee-value-scroll")
        .auto_shrink([false, false])
        .scroll_source(ScrollSource { drag: DragScroll::Never, ..ScrollSource::default() });
    if doc.fresh {
        doc.fresh = false;
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut inner = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
    let out = scroll.show_viewport(&mut inner, |ui, viewport| {
        ui.set_min_size(vec2(area.width(), total as f32 * LINE_H));
        // The rows in view, and where each is drawn (from the area's top, in f64: a row far down
        // is millions of points into the content).
        let first = ((viewport.min.y / LINE_H).floor().max(0.0) as usize).min(total.saturating_sub(1));
        let last = ((viewport.max.y / LINE_H).ceil().max(0.0) as usize).clamp(first.saturating_add(1), total);
        let top_of = |row: usize| area.top() + (row as f64 * f64::from(LINE_H) - f64::from(viewport.min.y)) as f32;
        let left = area.left() + PAD;
        let rows: Vec<Drawn> = doc
            .rows
            .ranges(text, first, last - first)
            .into_iter()
            .enumerate()
            .map(|(i, range)| {
                let galley = ui.painter().layout_job(row_job(text, range.clone(), &doc.colours, &font, t.text, palette));
                Drawn { range, galley, top: top_of(first + i) }
            })
            .collect();
        let resp = ui.interact(area, text_id(), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Text);
        // The byte under `p`: the row it is over (the first or last in view past the ends), then
        // the character.
        let caret = |p: Pos2| -> usize {
            let Some(row) = rows.iter().rev().find(|r| p.y >= r.top).or(rows.first()) else { return 0 };
            if p.y > row.top + LINE_H && std::ptr::eq(row, rows.last().unwrap_or(row)) {
                return row.range.end;
            }
            let at = row.galley.cursor_from_pos(vec2(p.x - left, row.galley.size().y / 2.0)).index.0;
            let line = text.get(row.range.clone()).unwrap_or_default();
            row.range.start + line.char_indices().nth(at).map_or(line.len(), |(b, _)| b)
        };
        if let Some(p) = resp.interact_pointer_pos() {
            let (pressed, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
            let at = caret(p);
            if pressed {
                *selection = match *selection {
                    Some((anchor, _)) if shift => Some((anchor, at)),
                    _ => Some((at, at)),
                };
                resp.request_focus();
            } else if resp.dragged_by(egui::PointerButton::Primary) {
                if let Some((anchor, _)) = *selection {
                    *selection = Some((anchor, at));
                }
                // Past the top or the bottom: scroll on, and the selection with it.
                let past = if p.y < area.top() { area.top() - p.y } else { (area.bottom() - p.y).min(0.0) };
                if past != 0.0 {
                    ui.scroll_with_delta_animation(vec2(0.0, past.clamp(-DRAG_SCROLL, DRAG_SCROLL)), egui::style::ScrollAnimation::none());
                    ui.ctx().request_repaint();
                }
            }
            if resp.double_clicked() {
                let word = word_at(text, at);
                *selection = Some((word.start, word.end));
            }
        }
        // The selection behind the text.
        if let Some((a, b)) = *selection {
            let (s0, s1) = (a.min(b), a.max(b));
            for row in rows.iter().filter(|_| s0 < s1) {
                let (from, to) = (s0.max(row.range.start), s1.min(row.range.end));
                // The row's line break is selected too: a little room after its text shows it.
                let line_break = s0 <= row.range.end && s1 > row.range.end && text.as_bytes().get(row.range.end) == Some(&b'\n');
                if from >= to && !line_break {
                    continue;
                }
                let x = |byte: usize| {
                    let chars = text.get(row.range.start..byte.max(row.range.start)).map_or(0, |s| s.chars().count());
                    left + row.galley.pos_from_cursor(CCursor::new(chars)).min.x
                };
                let (x0, mut x1) = (x(from.min(row.range.end)), x(to.max(from).min(row.range.end)));
                if line_break {
                    x1 += glyph * 0.6;
                }
                let r = Rect::from_min_max(pos2(x0, row.top), pos2(x1, row.top + LINE_H));
                ui.painter().rect_filled(r, CornerRadius::ZERO, t.selection);
            }
        }
        for row in rows {
            let y = row.top + (LINE_H - row.galley.size().y) / 2.0;
            ui.painter().galley(pos2(left, y), row.galley, t.text);
        }
        (resp, first)
    });
    let (resp, first) = out.inner;
    // A press anywhere but the text's box ends the selection and gives the keyboard back.
    let elsewhere = ui.input(|i| i.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, pos, .. } if !rect.contains(*pos))));
    if elsewhere {
        *selection = None;
        ui.memory_mut(|m| m.surrender_focus(text_id()));
    }
    let shown = selection.map(|(a, b)| a.min(b)..a.max(b)).filter(|r| !r.is_empty());
    tag(&resp, shown, matches!(doc.colours, Colours::Ready(_)), total, first);
}

/// The layout of row `range` of `text`: the JSON colours over it when it has them, else one
/// colour; never wrapped (the row is as long as fits).
fn row_job(text: &str, range: Range<usize>, colours: &Colours, font: &FontId, plain: Color32, palette: Syntax) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut add = |from: usize, to: usize, color: Color32| {
        if let Some(s) = text.get(from..to).filter(|s| !s.is_empty()) {
            job.append(s, 0.0, TextFormat::simple(font.clone(), color));
        }
    };
    let Colours::Ready(spans) = colours else {
        add(range.start, range.end, plain);
        return job;
    };
    let mut at = range.start;
    let from = spans.partition_point(|s| s.end as usize <= range.start);
    for s in spans.get(from..).unwrap_or_default().iter().take_while(|s| (s.start as usize) < range.end) {
        let (a, b) = ((s.start as usize).max(range.start), (s.end as usize).min(range.end));
        add(at, a, palette.punct);
        let colour = match s.token {
            JsonToken::Key => palette.key,
            JsonToken::String => palette.string,
            JsonToken::Number => palette.number,
            JsonToken::Literal => palette.literal,
            JsonToken::Punct => palette.punct,
        };
        add(a, b, colour);
        at = b;
    }
    add(at, range.end, palette.punct);
    job
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use dbee_core::Column;
    use dbee_core::json::JsonToken;
    use dbee_engine::{VALUE_VIEW_MAX_BYTES, ValueView};

    use super::*;
    use crate::automation::Tags;

    fn col(name: &str, base: &str) -> Column {
        Column { name: name.into(), base_type: base.into(), type_label: base.into() }
    }

    /// The rows of `text` at `width`, as text.
    fn rows_of(text: &str, width: usize) -> Vec<String> {
        let rows = Rows::new(text, width);
        (0..rows.count()).map(|r| text[rows.range(text, r).unwrap()].to_string()).collect()
    }

    /// The rows by a plain reference: lines split at line breaks, each cut every `width`
    /// characters.
    fn naive(text: &str, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        for line in text.split('\n') {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                out.push(String::new());
            }
            for chunk in chars.chunks(width.max(1)) {
                out.push(chunk.iter().collect());
            }
        }
        out
    }

    #[test]
    fn rows_wrap_at_the_width_and_end_at_line_breaks() {
        assert_eq!(rows_of("abcdefg\nhi\n", 3), ["abc", "def", "g", "hi", ""]);
        assert_eq!(rows_of("abcdef", 3), ["abc", "def"], "a line of exactly two rows");
        assert_eq!(rows_of("abc\ndef", 3), ["abc", "def"], "a line break right after a full row");
        assert_eq!(rows_of("", 3), [""]);
        assert_eq!(rows_of("\n\n", 3), ["", "", ""]);
        assert_eq!(rows_of("äöüõx", 2), ["äö", "üõ", "x"], "characters, not bytes");
        assert_eq!(rows_of("ab", 0), ["a", "b"], "a width of 0 counts as 1");
        let rows = Rows::new("abc", 3);
        assert_eq!((rows.count(), rows.range("abc", 1)), (1, None));
    }

    #[test]
    fn rows_far_down_match_a_plain_split() {
        // Many marks' worth of rows, of lines of every length, with characters of 1 to 4 bytes.
        let mut text = String::new();
        for i in 0..3000 {
            text.push_str(&"aé🐝".repeat(i % 37));
            text.push('\n');
            if i % 5 == 0 {
                text.push_str(&"x".repeat(i));
            }
        }
        for width in [1, 7, 40, 333] {
            let want = naive(&text, width);
            assert_eq!(rows_of(&text, width), want, "width {width}");
            // From any row on, as many as asked for.
            let rows = Rows::new(&text, width);
            for from in [0, 1, 129, want.len() / 2, want.len() - 1] {
                let got: Vec<&str> = rows.ranges(&text, from, 50).into_iter().map(|r| &text[r]).collect();
                let end = (from + 50).min(want.len());
                assert_eq!(got, want[from..end].iter().map(String::as_str).collect::<Vec<_>>(), "width {width}, from {from}");
            }
            assert!(rows.ranges(&text, want.len(), 5).is_empty());
        }
    }

    #[test]
    fn rows_of_a_long_line_are_found_quickly() {
        // 10 MiB on one line, as the cut 20 MB cell: finding a row far down walks a few rows,
        // not the line.
        let text = "0123456789abcdef".repeat(VALUE_VIEW_MAX_BYTES / 16);
        let start = Instant::now();
        let rows = Rows::new(&text, 40);
        assert_eq!(rows.count(), text.len().div_ceil(40));
        let last = rows.count() - 1;
        for _ in 0..100 {
            assert_eq!(rows.ranges(&text, last - 40, 60).len(), 41);
        }
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    fn the_panel_starts_narrower_in_a_narrow_window() {
        assert_eq!(start_width(1400.0), DEFAULT_WIDTH);
        assert!((start_width(1100.0) - 297.0).abs() < 0.5, "the app's first window");
        assert_eq!(start_width(640.0), MIN_WIDTH);
        assert_eq!(start_width(f32::NAN), DEFAULT_WIDTH);
    }

    #[test]
    fn a_word_is_its_letters_digits_and_underscores() {
        assert_eq!(word_at("say hello_world!", 6), 4..15);
        assert_eq!(word_at("say hello", 3), 3..4, "a space selects itself");
        assert_eq!(word_at("aé b", 1), 0..3, "é is a letter");
        assert_eq!(word_at("\"SKU-12\"", 2), 1..4);
        assert_eq!(word_at("abc", 3), 3..3, "past the end: nothing");
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(word_at("aé", 2), 0..3, "inside a character: its word");
    }

    #[test]
    fn a_cut_value_says_how_much_shows() {
        let text = ValueView::new(&col("t", "text"), Some(&"x".repeat(20_000_000)));
        assert_eq!(note(&text).as_deref(), Some("Showing the first 10.0 MiB of 19.1 MiB."));
        let json = ValueView::new(&col("j", "jsonb"), Some(&format!("[\"{}\"]", "x".repeat(20_000_000))));
        assert_eq!(note(&json).as_deref(), Some("Showing the first 10.0 MiB of 19.1 MiB. Too long to format."));
        assert_eq!(note(&ValueView::new(&col("t", "text"), Some("short"))), None);
        assert_eq!(note(&ValueView::new(&col("t", "text"), None)), None);
    }

    #[test]
    fn spans_are_the_highlight_without_punctuation() {
        let text = "{\n  \"k\": [1, true, \"s\"]\n}";
        let got: Vec<(&str, JsonToken)> = spans(text).iter().map(|s| (&text[s.start as usize..s.end as usize], s.token)).collect();
        assert_eq!(got, [("\"k\"", JsonToken::Key), ("1", JsonToken::Number), ("true", JsonToken::Literal), ("\"s\"", JsonToken::String)]);
        // The start of a cut document: the tokens up to the cut.
        let got: Vec<JsonToken> = spans("[{\"i\": 1, \"s\": \"zz").iter().map(|s| s.token).collect();
        assert_eq!(got, [JsonToken::Key, JsonToken::Number, JsonToken::Key]);
    }

    /// One frame of the panel's contents over `view`.
    fn frame(ctx: &egui::Context, state: &mut PanelState, tags: &mut Tags, view: Option<&Arc<ValueView>>) {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            tags.begin_frame();
            contents(ui, view, state, tags);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn a_selection_belongs_to_its_value_and_debug_leaves_the_value_out() {
        let ctx = egui::Context::default();
        let (mut state, mut tags) = (PanelState::default(), Tags::default());
        let a = Arc::new(ValueView::new(&col("t", "text"), Some("first secret value")));
        let b = Arc::new(ValueView::new(&col("t", "text"), Some("second")));
        frame(&ctx, &mut state, &mut tags, Some(&a));
        assert_eq!(state.selected_text(), None);
        state.select_all();
        assert_eq!(state.selected_text(), Some("first secret value"));
        assert_eq!(tags.value("value-text").map(|v| v["text"].clone()), Some(serde_json::json!("first secret value")));
        assert!(!format!("{state:?}").contains("secret"), "{state:?}");
        // Another value: the selection went with the old one.
        frame(&ctx, &mut state, &mut tags, Some(&b));
        assert_eq!(state.selected_text(), None);
        // No cell: the panel is empty and nothing can be selected.
        frame(&ctx, &mut state, &mut tags, None);
        state.select_all();
        assert_eq!(state.selected_text(), None);
        assert!(tags.value("value-text").is_none());
        assert_eq!(tags.value("value-panel"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn small_json_is_coloured_at_once_and_large_json_off_the_frame() {
        let ctx = egui::Context::default();
        let (mut state, mut tags) = (PanelState::default(), Tags::default());
        let small = Arc::new(ValueView::new(&col("j", "jsonb"), Some(r#"{"a": [1, 2]}"#)));
        frame(&ctx, &mut state, &mut tags, Some(&small));
        assert_eq!(tags.value("value-text").map(|v| v["highlighted"].clone()), Some(serde_json::json!(true)));
        let big = format!("[{}]", vec![r#"{"a": 1, "b": "text"}"#; 40_000].join(","));
        let big = Arc::new(ValueView::new(&col("j", "jsonb"), Some(&big)));
        assert!(big.pretty && big.text.len() > SYNC_COLOUR_BYTES);
        let start = Instant::now();
        loop {
            frame(&ctx, &mut state, &mut tags, Some(&big));
            if tags.value("value-text").is_some_and(|v| v["highlighted"] == true) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "the colours never came");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Plain text is never coloured.
        let plain = Arc::new(ValueView::new(&col("t", "text"), Some("[1, 2]x")));
        frame(&ctx, &mut state, &mut tags, Some(&plain));
        assert_eq!(tags.value("value-text").map(|v| v["highlighted"].clone()), Some(serde_json::json!(false)));
    }

    #[test]
    fn the_tag_carries_the_start_of_a_long_text() {
        let ctx = egui::Context::default();
        let (mut state, mut tags) = (PanelState::default(), Tags::default());
        let long = Arc::new(ValueView::new(&col("t", "text"), Some(&"é".repeat(TAG_BYTES))));
        frame(&ctx, &mut state, &mut tags, Some(&long));
        let v = tags.value("value-text").unwrap();
        assert_eq!(v["cut"], true);
        let text = v["text"].as_str().unwrap();
        assert!(text.len() <= TAG_BYTES && text.len() + 2 > TAG_BYTES && text.chars().all(|c| c == 'é'));
        let head = tags.value("value-panel").unwrap();
        assert_eq!((head["column"].clone(), head["bytes"].clone()), (serde_json::json!("t"), serde_json::json!(TAG_BYTES * 2)));
    }
}

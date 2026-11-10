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
//! ([`Rows`]: a row ends at a line break or before the character that would take it past the
//! box). Widths are counted in columns of an `x` ([`Columns`]): a tab takes the font's four, other
//! ASCII one, any other character its glyph's width rounded up to whole columns (an emoji or a
//! CJK character two), so no row runs past the box. Only the rows in view are laid out, so a
//! 10 MiB text costs a frame what a short one does: the rows are found from marks kept every
//! [`MARK_EVERY`] rows, which one pass over the text makes when the text or the width changes.
//! While the panel's edge is being dragged the text keeps its rows (that pass takes up to ~80 ms
//! for ten million lines) and wraps again once the edge is let go. Positions are f32 as egui's
//! scrolling is: past about a million rows (18 million points) a row's place is off by a point
//! or more, which is accepted.
//!
//! **Colours.** JSON that parsed (or the start of one that was cut) gets syntax colours from
//! `json::highlight` ([`spans`], theme [`crate::theme::Syntax`]); up to [`SYNC_COLOUR_BYTES`]
//! they are worked out in the frame, above that by one worker thread ([`ColourWorker`]) while
//! the text shows uncoloured. The worker takes one value at a time, and a value asked for while
//! it works replaces any still waiting, so at most one `highlight` runs.
//!
//! **Another view of the same text** (a staged edit of another cell, an undo or a redo: the
//! engine builds a new view) keeps the place it is scrolled to, the selection and the colours;
//! another text starts afresh, at its top.
//!
//! **Selecting.** A press places the caret and takes the keyboard, a drag or a shift-click
//! selects from there (scrolling when the drag leaves the text), a double-click selects a word.
//! With text selected, Ctrl+C copies it, not the grid's cells, and so does Ctrl+Shift+C, as in a
//! text field ([`take_keys`], before the shortcuts); while the panel has the keyboard, Ctrl+A
//! selects all of its text. A press outside the text, another text, or hiding the panel ends the
//! selection. A hidden panel asks the session for no value view.
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

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dbee_core::json::{self, JsonKind, JsonToken};
use dbee_engine::view::cut;
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

// ---- columns ------------------------------------------------------------------------------------

/// How many columns each character takes in the panel's monospace font, so that a row as many
/// columns wide as the box never runs past it: a tab as many as the font gives it (four spaces in
/// egui's fonts), any other ASCII character one, and any other character its glyph's width over
/// an `x`'s, rounded up. That is at least one, so every row moves on; a glyph no font has is
/// measured as the replacement character it is drawn as. Each character is measured once
/// ([`Columns::learn`]), and characters below U+10000 are looked up by code point.
pub struct Columns {
    tab: u8,
    /// By code point below U+10000, 0 when not measured yet; made with the first one.
    bmp: Vec<u8>,
    /// The others.
    astral: HashMap<char, u8>,
}

/// What a character not measured yet is taken to need.
const UNMEASURED: usize = 2;

/// Code points looked up by index.
const BMP: usize = 0x1_0000;

/// `width` (in columns, from a measure) as whole columns: rounded up, 1 to 255.
fn whole_columns(width: f32) -> u8 {
    let w = if width.is_finite() { width.ceil().clamp(1.0, 255.0) } else { 1.0 };
    w as u8
}

impl Columns {
    /// Tabs take `tab` columns (1 to 255); nothing else is measured yet.
    pub fn new(tab: usize) -> Columns {
        Columns { tab: u8::try_from(tab.clamp(1, 255)).unwrap_or(u8::MAX), bmp: Vec::new(), astral: HashMap::new() }
    }

    fn known(&self, c: char) -> u8 {
        let at = c as usize;
        if at < BMP { self.bmp.get(at).copied().unwrap_or(0) } else { self.astral.get(&c).copied().unwrap_or(0) }
    }

    /// The columns `c` takes.
    pub fn of(&self, c: char) -> usize {
        if c == '\t' {
            return usize::from(self.tab);
        }
        if c.is_ascii() {
            return 1;
        }
        match self.known(c) {
            0 => UNMEASURED,
            n => usize::from(n),
        }
    }

    /// Measures every character of `text` not measured yet: `measure` gives its width in columns
    /// (its glyph's width over an `x`'s).
    pub fn learn(&mut self, text: &str, mut measure: impl FnMut(char) -> f32) {
        if text.is_ascii() {
            return;
        }
        for c in text.chars().filter(|c| !c.is_ascii()) {
            if self.known(c) != 0 {
                continue;
            }
            let n = whole_columns(measure(c));
            let at = c as usize;
            if at < BMP {
                if self.bmp.is_empty() {
                    self.bmp = vec![0; BMP];
                }
                if let Some(slot) = self.bmp.get_mut(at) {
                    *slot = n;
                }
            } else {
                self.astral.insert(c, n);
            }
        }
    }
}

// ---- rows ---------------------------------------------------------------------------------------

/// The rows a text wraps into at a width of so many columns ([`Columns`]): a row ends at a line
/// break (which belongs to no row) or before the character that would take it past the width (a
/// row holds one character at least), so a text of `n` line breaks has at least `n + 1` rows,
/// the empty text one. Only marks are kept, the start of every [`MARK_EVERY`]-th row; a row is
/// found by walking from the mark before it.
#[derive(Clone, Debug)]
pub struct Rows {
    width: usize,
    count: usize,
    marks: Vec<usize>,
}

impl Rows {
    /// The rows of `text` at `width` columns (0 counts as 1): one pass over the text.
    pub fn new(text: &str, width: usize, columns: &Columns) -> Rows {
        let width = width.max(1);
        let mut rows = Rows { width, count: 0, marks: Vec::new() };
        let mut start = 0;
        loop {
            let end = next_break(text, start);
            rows.line(text, start..end.unwrap_or(text.len()), columns);
            match end {
                Some(at) => start = at + 1,
                None => break,
            }
        }
        rows
    }

    /// The rows of the line `text[line]`.
    fn line(&mut self, text: &str, line: Range<usize>, columns: &Columns) {
        let start = line.start;
        let line = text.get(line).unwrap_or_default();
        // ASCII without a tab: a column a byte.
        if line.is_ascii() && !line.as_bytes().contains(&b'\t') {
            let mut at = 0;
            loop {
                self.push(start + at);
                at = at.saturating_add(self.width);
                if at >= line.len() {
                    break;
                }
            }
            return;
        }
        // The columns used in the row so far (`None` before its first character).
        let mut used: Option<usize> = None;
        for (i, c) in line.char_indices() {
            let w = columns.of(c);
            match used {
                Some(u) if u.saturating_add(w) <= self.width => used = Some(u + w),
                _ => {
                    self.push(start + i);
                    used = Some(w);
                }
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

    /// The bytes of row `row` of `text` (the text the rows were made of, with the same columns),
    /// without its line break; `None` past the last row.
    pub fn range(&self, text: &str, row: usize, columns: &Columns) -> Option<Range<usize>> {
        self.ranges(text, row, 1, columns).into_iter().next()
    }

    /// Rows `from..from + n` of `text` (fewer at its end), as byte ranges.
    pub fn ranges(&self, text: &str, from: usize, n: usize, columns: &Columns) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let to = from.saturating_add(n).min(self.count);
        let Some(&mark) = self.marks.get(from / MARK_EVERY) else { return out };
        let mut row = from - from % MARK_EVERY;
        let mut at = Some(mark);
        while let Some(start) = at
            && row < to
        {
            let (end, next) = step(text, start, self.width, columns);
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

/// The row of `text` that starts at `start`, `width` columns at most (as [`Rows::new`] makes
/// them): where it ends, and where the next row starts (`None` when it is the last).
fn step(text: &str, start: usize, width: usize, columns: &Columns) -> (usize, Option<usize>) {
    let rest = text.get(start..).unwrap_or_default();
    let mut used = 0usize;
    for (i, c) in rest.char_indices() {
        if c == '\n' {
            return (start + i, Some(start + i + 1));
        }
        let w = columns.of(c);
        if i > 0 && used.saturating_add(w) > width {
            return (start + i, Some(start + i));
        }
        used = used.saturating_add(w);
    }
    (start + rest.len(), None)
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

/// What the colour worker holds: the request waiting and the last answer, each with its ticket.
#[derive(Default)]
struct Slot {
    next: u64,
    waiting: Option<(u64, Arc<ValueView>)>,
    done: Option<(u64, Vec<Span>)>,
    stop: bool,
}

/// One thread that works out the colours of large JSON values, one at a time. A request waits in
/// a slot for one, a newer request replacing it, so at most one `highlight` runs (it cannot be
/// stopped half way) and a quick walk over large cells works out only the one it ends on. The
/// thread ends with the worker.
pub struct ColourWorker {
    shared: Arc<(Mutex<Slot>, Condvar)>,
}

fn lock(m: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ColourWorker {
    /// A worker that runs `work` on the text of each request; `None` when no thread starts.
    pub fn spawn(work: impl Fn(&str) -> Vec<Span> + Send + 'static) -> Option<ColourWorker> {
        let shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let theirs = Arc::clone(&shared);
        let started = std::thread::Builder::new().name("dbee-value-colours".into()).spawn(move || {
            let (slot, wake) = &*theirs;
            loop {
                let (ticket, view) = {
                    let mut s = lock(slot);
                    loop {
                        if s.stop {
                            return;
                        }
                        if let Some(job) = s.waiting.take() {
                            break job;
                        }
                        s = wake.wait(s).unwrap_or_else(PoisonError::into_inner);
                    }
                };
                let spans = work(&view.text);
                drop(view);
                lock(slot).done = Some((ticket, spans));
            }
        });
        match started {
            Ok(_) => Some(ColourWorker { shared }),
            Err(e) => {
                log::debug!("value panel: no thread for the colours: {e}");
                None
            }
        }
    }

    /// Asks for the colours of `view`, in place of any request still waiting; the ticket takes
    /// them ([`ColourWorker::take`]).
    pub fn ask(&self, view: Arc<ValueView>) -> u64 {
        let (slot, wake) = &*self.shared;
        let mut s = lock(slot);
        s.next = s.next.wrapping_add(1);
        let ticket = s.next;
        s.waiting = Some((ticket, view));
        wake.notify_one();
        ticket
    }

    /// The colours asked for with `ticket`, once they are worked out (an older answer is dropped).
    pub fn take(&self, ticket: u64) -> Option<Vec<Span>> {
        let mut s = lock(&self.shared.0);
        match s.done.take() {
            Some((t, spans)) if t == ticket => Some(spans),
            _ => None,
        }
    }

    /// Its thread still runs (it ends only with the worker, or if the work panicked).
    fn alive(&self) -> bool {
        Arc::strong_count(&self.shared) > 1
    }
}

impl Drop for ColourWorker {
    fn drop(&mut self) {
        let (slot, wake) = &*self.shared;
        lock(slot).stop = true;
        wake.notify_all();
    }
}

/// The text's colours.
enum Colours {
    /// Not JSON, or JSON that did not parse: one colour.
    Plain,
    /// Being worked out by the colour worker, with this ticket.
    Pending(u64),
    Ready(Vec<Span>),
}

/// Whether `view` gets JSON colours: JSON that was pretty-printed, or the start of a cut one.
fn coloured(view: &ValueView) -> bool {
    view.kind != JsonKind::NotJson && (view.pretty || view.truncated)
}

/// The colours of `view`'s text: in this frame when it is short, else from the worker (started
/// the first time one is needed).
fn colours_of(view: &Arc<ValueView>, worker: &mut Option<ColourWorker>) -> Colours {
    if !coloured(view) {
        return Colours::Plain;
    }
    if view.text.len() <= SYNC_COLOUR_BYTES {
        return Colours::Ready(spans(&view.text));
    }
    if worker.is_none() {
        *worker = ColourWorker::spawn(spans);
    }
    match worker {
        Some(w) => Colours::Pending(w.ask(Arc::clone(view))),
        None => Colours::Plain,
    }
}

impl Colours {
    /// Takes the worker's colours once they are there.
    fn poll(&mut self, worker: Option<&ColourWorker>) {
        if let Colours::Pending(ticket) = *self {
            match worker {
                Some(w) => match w.take(ticket) {
                    Some(spans) => *self = Colours::Ready(spans),
                    None if !w.alive() => *self = Colours::Plain,
                    None => {}
                },
                None => *self = Colours::Plain,
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

/// How the value to show differs from the one shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    /// The same view.
    None,
    /// Another view of the same text (a staged edit elsewhere, an undo): it keeps its place, its
    /// selection and its colours.
    SameText,
    /// Another text: it starts afresh.
    Text,
}

/// What the panel keeps between frames: the value it shows (the engine's view, shared), its rows
/// and colours, the selection in it, the columns its characters take, and the colour worker.
#[derive(Default)]
pub struct PanelState {
    doc: Option<Doc>,
    /// Where the selection started and where it ends now, as byte offsets into the text; empty
    /// when they are the same.
    selection: Option<(usize, usize)>,
    columns: Option<Columns>,
    worker: Option<ColourWorker>,
    /// The panel's edge is being dragged: the text keeps its rows until it is let go.
    holding_rows: bool,
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

    /// While `hold` (the panel's edge is being dragged), a new width does not wrap the text again
    /// (that is one pass over up to 10 MiB); it does once the edge is let go.
    pub fn hold_rows(&mut self, hold: bool) {
        self.holding_rows = hold;
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

    /// How `view` differs from the value shown (the texts are compared only for another view).
    fn change(&self, view: &Arc<ValueView>) -> Change {
        match &self.doc {
            Some(d) if Arc::ptr_eq(&d.view, view) => Change::None,
            Some(d) if d.view.text == view.text => Change::SameText,
            _ => Change::Text,
        }
    }

    /// Makes it show `view` wrapped at `width` columns, `change` being how it differs from the
    /// value shown: another text starts afresh (no selection, at its top, its colours to work
    /// out), the same text in another view keeps all that; a new width wraps it again, unless
    /// the rows are held.
    fn prepare(&mut self, view: &Arc<ValueView>, width: usize, change: Change) {
        let columns = self.columns.get_or_insert_with(|| Columns::new(4));
        match (change, self.doc.as_mut()) {
            (Change::None, Some(_)) => {}
            (Change::SameText, Some(doc)) => {
                if coloured(&doc.view) != coloured(view) {
                    doc.colours = colours_of(view, &mut self.worker);
                }
                doc.view = Arc::clone(view);
            }
            _ => {
                self.selection = None;
                let rows = Rows::new(&view.text, width, columns);
                let colours = colours_of(view, &mut self.worker);
                self.doc = Some(Doc { view: Arc::clone(view), rows, colours, fresh: true });
                return;
            }
        }
        if let Some(doc) = self.doc.as_mut()
            && doc.rows.width() != width.max(1)
            && !self.holding_rows
        {
            doc.rows = Rows::new(&doc.view.text, width, columns);
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
    let panel = egui::Panel::right(panel_id()).resizable(true).default_size(start).size_range(MIN_WIDTH..=MAX_WIDTH).frame(frame);
    let out = panel.show(ui, |ui| {
        // The whole width: the panel keeps the size its contents take.
        let rect = ui.max_rect();
        ui.expand_to_include_rect(rect);
        rect
    });
    Some(out.inner)
}

/// The panel's id, egui's for its size.
fn panel_id() -> Id {
    Id::new("dbee-value-panel")
}

/// After the tab's body: the panel's contents in `rect`, for the value the session shows now.
pub(crate) fn fill(app: &mut DbeeApp, ui: &mut egui::Ui, rect: Rect) {
    let view = app.session.value_view_shared();
    let DbeeApp { view: v, tags, .. } = app;
    // egui's resize handle of a panel is its id with `__resize`: while it is dragged the text
    // keeps its rows, and wraps again once it is let go.
    v.panel.hold_rows(ui.ctx().is_being_dragged(panel_id().with("__resize")));
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
    // A hidden panel has no selection, and asks for no value view (one can be 10 MiB to build).
    if !app.view.value_panel || app.session.state().screen != Screen::Workspace {
        app.view.panel.clear_selection();
        return;
    }
    let shown = app.session.value_view_shared();
    let state = &mut app.view.panel;
    // The selection is in the text the panel drew last; another text since has none.
    if shown.as_ref().is_none_or(|v| state.change(v) == Change::Text) {
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
    let label = (!view.kind_label.is_empty()).then_some(view.kind_label.as_str());
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
        let label = line_galley(ui, &view.kind_label, FontId::proportional(11.5), t.accent, width - 12.0);
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
    let change = state.change(view);
    // An `x`'s width is a column; a new text's characters are measured in columns.
    let known = &mut state.columns;
    let glyph = ui.fonts_mut(|f| {
        let x = f.glyph_width(&font, 'x');
        let x = if x.is_finite() && x > 0.0 { x } else { FONT_SIZE * 0.6 };
        let columns = known.get_or_insert_with(|| {
            let tab = f.glyph_width(&font, '\t') / x;
            Columns::new(if tab > 0.0 { usize::from(whole_columns(tab)) } else { 4 })
        });
        if change == Change::Text {
            let missing = f.glyph_width(&font, '\u{FFFD}');
            columns.learn(&view.text, |c| {
                let w = f.glyph_width(&font, c);
                (if w > 0.0 { w } else { missing }) / x
            });
        }
        x
    });
    let width = ((area.width() - 2.0 * PAD - BAR_ROOM) / glyph).floor().max(1.0) as usize;
    state.prepare(view, width, change);
    let PanelState { doc, selection, columns, worker, .. } = state;
    let (Some(doc), Some(columns)) = (doc.as_mut(), columns.as_ref()) else { return };
    doc.colours.poll(worker.as_ref());
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
            .ranges(text, first, last - first, columns)
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

    use dbee_core::json::JsonToken;
    use dbee_core::{Column, TypeClass};
    use dbee_engine::{VALUE_VIEW_MAX_BYTES, ValueView};

    use super::*;
    use crate::automation::Tags;

    fn col(name: &str, base: &str) -> Column {
        let class = if base == "jsonb" { TypeClass::Json } else { TypeClass::LongText };
        Column { name: name.into(), base_type: base.into(), type_label: base.into(), class }
    }

    /// One column for every character of `text` (no tabs in it).
    fn plain(text: &str) -> Columns {
        let mut c = Columns::new(4);
        c.learn(text, |_| 1.0);
        c
    }

    /// The rows of `text` at `width`, one column a character, as text.
    fn rows_of(text: &str, width: usize) -> Vec<String> {
        let cols = plain(text);
        let rows = Rows::new(text, width, &cols);
        (0..rows.count()).map(|r| text[rows.range(text, r, &cols).unwrap()].to_string()).collect()
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
        let rows = Rows::new("abc", 3, &plain("abc"));
        assert_eq!((rows.count(), rows.range("abc", 1, &plain("abc"))), (1, None));
    }

    #[test]
    fn rows_far_down_match_a_plain_split() {
        // Many marks' worth of rows, of lines of every length, with characters of 1 to 4 bytes.
        let mut text = String::new();
        for i in 0..3000 {
            text.push_str(&"aé🐙".repeat(i % 37));
            text.push('\n');
            if i % 5 == 0 {
                text.push_str(&"x".repeat(i));
            }
        }
        for width in [1, 7, 40, 333] {
            let want = naive(&text, width);
            assert_eq!(rows_of(&text, width), want, "width {width}");
            // From any row on, as many as asked for.
            let cols = plain(&text);
            let rows = Rows::new(&text, width, &cols);
            for from in [0, 1, 129, want.len() / 2, want.len() - 1] {
                let got: Vec<&str> = rows.ranges(&text, from, 50, &cols).into_iter().map(|r| &text[r]).collect();
                let end = (from + 50).min(want.len());
                assert_eq!(got, want[from..end].iter().map(String::as_str).collect::<Vec<_>>(), "width {width}, from {from}");
            }
            assert!(rows.ranges(&text, want.len(), 5, &cols).is_empty());
        }
    }

    /// Rows by a plain reference with columns: lines split at line breaks, a row taking
    /// characters while their columns fit `width` (at least one character a row).
    fn naive_columns(text: &str, width: usize, columns: impl Fn(char) -> usize) -> Vec<String> {
        let mut out = Vec::new();
        for line in text.split('\n') {
            let mut row = String::new();
            let mut used = 0;
            let mut started = false;
            for c in line.chars() {
                let w = columns(c);
                if started && used + w > width {
                    out.push(std::mem::take(&mut row));
                    used = 0;
                }
                row.push(c);
                used += w;
                started = true;
            }
            out.push(row);
        }
        out
    }

    /// Columns as the tests take them: a tab 4, ASCII 1, 🐙 and CJK 2, other characters 1.
    fn test_columns(text: &str) -> Columns {
        let mut c = Columns::new(4);
        c.learn(text, |ch| if ch == '🐙' || ('\u{4E00}'..='\u{9FFF}').contains(&ch) { 1.6 } else { 0.9 });
        c
    }

    #[test]
    fn rows_count_tabs_and_wide_characters_by_their_columns() {
        let text = "a\tb\tc\n🐙🐙🐙\n中文中\nab\t\n\t\t\t\nõõõõõõ";
        let cols = test_columns(text);
        assert_eq!((cols.of('\t'), cols.of('a'), cols.of('🐙'), cols.of('中'), cols.of('õ')), (4, 1, 2, 2, 1));
        let rows = |width: usize| -> Vec<String> {
            let r = Rows::new(text, width, &cols);
            (0..r.count()).map(|i| text[r.range(text, i, &cols).unwrap()].to_string()).collect()
        };
        assert_eq!(rows(5), ["a\t", "b\t", "c", "🐙🐙", "🐙", "中文", "中", "ab", "\t", "\t", "\t", "\t", "õõõõõ", "õ"]);
        // A character wider than a row still takes one.
        assert_eq!(
            rows(1),
            ["a", "\t", "b", "\t", "c", "🐙", "🐙", "🐙", "中", "文", "中", "a", "b", "\t", "\t", "\t", "\t", "õ", "õ", "õ", "õ", "õ", "õ"]
        );
        // Far down, as a plain split with the same columns.
        let mut long = String::new();
        for i in 0..2000 {
            long.push_str(&"\tx🐙中é".repeat(i % 23));
            long.push('\n');
        }
        let cols = test_columns(&long);
        for width in [1, 6, 33] {
            let r = Rows::new(&long, width, &cols);
            let got: Vec<&str> = (0..r.count()).map(|i| &long[r.range(&long, i, &cols).unwrap()]).collect();
            assert_eq!(got, naive_columns(&long, width, |c| cols.of(c)), "width {width}");
        }
    }

    #[test]
    fn one_colour_worker_runs_the_latest_value_only() {
        // A worker whose work waits until the test lets it go, and says what it was given.
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate_rx);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let worker = ColourWorker::spawn(move |text: &str| {
            log.lock().unwrap().push(text.to_string());
            let _ = gate.lock().unwrap().recv();
            Vec::new()
        })
        .unwrap();
        let view = |text: &str| Arc::new(ValueView::new(&col("t", "text"), Some(text)));
        let first = worker.ask(view("first"));
        // Wait until the first is in flight, then ask three more while it runs.
        let start = Instant::now();
        while seen.lock().unwrap().is_empty() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = worker.ask(view("second"));
        let _ = worker.ask(view("third"));
        let last = worker.ask(view("fourth"));
        gate_tx.send(()).unwrap();
        gate_tx.send(()).unwrap();
        let start = Instant::now();
        while worker.take(last).is_none() {
            assert!(start.elapsed() < Duration::from_secs(10), "the latest never came");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*seen.lock().unwrap(), ["first", "fourth"], "one at a time, and only the latest waiting one");
        assert!(worker.take(first).is_none(), "an answer nobody waits for any more is dropped");
    }

    /// 10 MiB on one line, as the cut 20 MB cell: its rows, and the 41 last ones found 100 times;
    /// how long that took.
    fn rows_of_a_long_line() -> Duration {
        let text = "0123456789abcdef".repeat(VALUE_VIEW_MAX_BYTES / 16);
        let start = Instant::now();
        let cols = Columns::new(4);
        let rows = Rows::new(&text, 40, &cols);
        assert_eq!(rows.count(), text.len().div_ceil(40));
        let last = rows.count() - 1;
        for _ in 0..100 {
            assert_eq!(rows.ranges(&text, last - 40, 60, &cols).len(), 41);
        }
        start.elapsed()
    }

    #[test]
    fn rows_of_a_long_line_are_found() {
        rows_of_a_long_line();
    }

    #[test]
    #[ignore = "timing; run with --ignored --nocapture"]
    fn rows_of_a_long_line_are_found_quickly() {
        // Finding a row far down walks a few rows, not the line.
        let took = rows_of_a_long_line();
        println!("rows of a 10 MiB line: {took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
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

    /// The right edge of every row of panel text drawn by `contents` over `view` in a window
    /// `width` wide (two frames: the fonts load in the first), and the text box's rect.
    fn drawn_rows(view: &Arc<ValueView>, width: f32) -> (Vec<f32>, egui::Rect) {
        let ctx = egui::Context::default();
        let (mut state, mut tags) = (PanelState::default(), Tags::default());
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 600.0))), ..Default::default() };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(input.clone(), |ui| {
                tags.begin_frame();
                contents(ui, Some(view), &mut state, &mut tags);
            });
            shapes = std::mem::take(&mut out.shapes);
            out.drop_without_applying_deltas();
        }
        let rights = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.sections.first().is_some_and(|s| s.format.font_id.family == egui::FontFamily::Monospace) => {
                    Some(t.pos.x + t.galley.rect.right())
                }
                _ => None,
            })
            .collect();
        (rights, tags.rect("value-text").unwrap())
    }

    #[test]
    fn no_character_lands_past_the_text_box() {
        let text = [
            "\tindented\twith\ttabs\tand\tmore\ttabs\there\tand\there",
            &"🐙".repeat(40),
            &"中文字符".repeat(12),
            &"\t".repeat(30),
            "plain ascii that is long enough to wrap more than once in a narrow panel like this one",
            "mixed\t🐙\tõ\t中\tend",
        ]
        .join("\n");
        let view = Arc::new(ValueView::new(&col("t", "text"), Some(&text)));
        for width in [300.0, 420.0] {
            let (rights, area) = drawn_rows(&view, width);
            assert!(rights.len() > 10, "{} rows drawn", rights.len());
            for right in rights {
                assert!(right <= area.right() - PAD + 0.5, "a row runs to {right}, past the box's text edge {}", area.right() - PAD);
            }
        }
    }

    #[test]
    fn the_same_text_again_keeps_its_colours_and_selection() {
        // A staged edit elsewhere, an undo: the engine builds a new view of the same text.
        let ctx = egui::Context::default();
        let (mut state, mut tags) = (PanelState::default(), Tags::default());
        let raw = format!("[{}]", vec![r#"{"a": 1, "b": "text"}"#; 40_000].join(","));
        let a = Arc::new(ValueView::new(&col("j", "jsonb"), Some(&raw)));
        let start = Instant::now();
        while !tags.value("value-text").is_some_and(|v| v["highlighted"] == true) {
            assert!(start.elapsed() < Duration::from_secs(30), "the colours never came");
            frame(&ctx, &mut state, &mut tags, Some(&a));
            std::thread::sleep(Duration::from_millis(5));
        }
        state.select_all();
        let b = Arc::new(ValueView::new(&col("j", "jsonb"), Some(&raw)));
        frame(&ctx, &mut state, &mut tags, Some(&b));
        let v = tags.value("value-text").unwrap();
        assert_eq!((v["highlighted"].clone(), v["selection"].clone()), (serde_json::json!(true), serde_json::json!([0, a.text.len()])), "kept");
        // Another text starts afresh.
        let c = Arc::new(ValueView::new(&col("j", "jsonb"), Some(r#"{"other": true}"#)));
        frame(&ctx, &mut state, &mut tags, Some(&c));
        assert_eq!(tags.value("value-text").unwrap()["selection"], serde_json::Value::Null);
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

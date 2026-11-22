//! Inline cell editors over a table tab's grid ([`CellEditor`]): at most one per window, kept in
//! the workspace's state while it is open.
//!
//! **Opening.** A double-click on a cell, or Enter while a table tab shows a selection and no
//! widget has the keyboard, opens an editor on that cell (the selection's focus cell), holding
//! its whole value as shown (a staged value over the loaded one; NULL as empty) with all of it
//! selected, so typing replaces it. Whether the cell can be edited is the engine's rule
//! (`cmd::edit::can_edit`: not read-only, a table with a row key, a connection that is not lost,
//! no page or Apply on its way); when it cannot, no editor opens and a notice says why. Two more:
//! a row marked for deletion takes no edits (as its menu's Set as NULL is disabled), and a value
//! longer than [`MAX_INLINE_BYTES`] does not open inline (a notice says so). Nor does a paste go
//! in that would make the value longer than that, counting the selection it replaces
//! ([`paste_fits`]); typing is not limited.
//!
//! **The cap.** egui lays out a paragraph (text between line breaks) whole on every change, at
//! about 0.4 µs a byte in a release build, so a keystroke in one long line or one long JSON
//! value costs ~13 ms at 32 KiB, ~25 ms at 64 KiB and ~130 ms at 256 KiB; text of short lines
//! costs far less, as each paragraph is cached. 32 KiB keeps a keystroke within about a frame in
//! the worst case (`keystroke_frame_with_a_value_at_the_cap`, docs/testing.md §6).
//!
//! **One draft, two views.** The editor always opens over its cell, whatever the column's type
//! and whether or not the value panel shows, with the keyboard and the caret there
//! ([`Place::Cell`]). It is one line, the cell's own size, with the grid's `ink` outline, and
//! scrolls sideways with the caret. While the value panel shows, the panel shows the same draft
//! live ([`show_in_panel`]): the column's name and type in its bar with CANCEL (as Escape) and
//! SAVE (as Ctrl+Enter) at its right, its foot counting the draft's lines and bytes, the text in
//! `code`, wrapped as WRAP says, coloured as JSON when the column is JSON or the value opened as
//! JSON, up to the first spot that does not parse (plain `text` after it), with no edge. A press
//! in the panel's text moves the keyboard and the caret there ([`Place::Panel`]), a press in the
//! cell's editor moves them back; neither commits, and there is one caret at a time. Each view
//! keeps its own caret and its own undo steps. Opening an editor does not show a hidden panel.
//!
//! **Line breaks.** The cell's editor is one line, so a line break in the value is one character
//! of that line: drawn as the grid's line-break mark where the data font has it, else as a gap
//! one character wide ([`break_mark`]; the bundled font has no such glyph, so today it is the
//! gap). The caret steps over it, it can be selected and deleted like any character, and it
//! stays when it is not: a value of several lines edited in its cell keeps them. A paste keeps
//! its line breaks too ([`paste_whole`]: egui's own one-line field would turn them into spaces).
//! A new line break is typed in the panel.
//!
//! **Kinds** ([`Kind`], from the column's catalog entry): several lines for `text`, `json`,
//! `jsonb` and `xml` (`ColumnInfo::multiline`), one line for the rest. The kind decides one
//! thing: what Enter does while the keyboard is in the panel (below). Booleans are one line of
//! text like the rest: the cell's text as the database prints it (`t`, `1`; MySQL's `TINYINT(1)`
//! holding 5 as `5`), and an emptied editor is NULL. The engine's `cell.edit` stages a typed
//! boolean word in the database's spelling.
//!
//! **Keys.** While an editor is open it has the keyboard. In the cell, Enter (or Shift+Enter, or
//! Ctrl+Enter) commits, for every kind. In the panel, Ctrl+Enter commits, and Enter is a newline
//! in a several-line kind and commits a one-line kind. In both, Tab commits and selects the next
//! cell to the right, then the start of the next row (Shift+Tab goes back; at the grid's last
//! cell the selection stays), and Escape cancels. These keys are taken from the frame's input
//! before the shortcuts and the widgets see it ([`before_frame`]), so the global Ctrl+Enter does
//! not run too, and act once the editor has drawn ([`after_draw`]): text typed earlier in the
//! same frame is in the value, and keys typed after one wait for the next frame
//! (`DbeeApp::feed_input`). Ctrl+Z and Ctrl+Shift+Z are the editor's own undo and redo (see
//! [`crate::shortcuts`]). Any other shortcut commits the editor before it runs, so Ctrl+S writes
//! the value being typed.
//!
//! **The grid's arrows.** With no editor open, this module also gives the grid its arrow keys
//! ([`move_on_arrows`]): they move the selection, and Shift with them extends it.
//!
//! **Committing** ([`CellEditor::command`]): an unchanged value (the text it opened with) sends
//! nothing; empty text sends `cell.clear` (`''` for string types, NULL for the others, booleans
//! included); other text sends `cell.edit`. A press outside the cell's editor and outside the
//! panel commits it before anything else sees the press, so clicking another cell commits and
//! selects that cell; a press in the panel outside its text (its bar, its foot, its resize edge)
//! changes nothing. An editor whose tab, page or cell is gone, or that can no longer stage,
//! closes without committing; so does one whose tab's staged edits changed under it
//! (`Edits::revision`: an agent's `row.delete` can move the row it points at), with a notice that
//! the edit was discarded. When the cell leaves the rows and columns in view (scrolled away
//! either way) while the keyboard is in the cell's editor, it commits, as a click elsewhere
//! would; while the keyboard is in the panel it stays open there, and nothing is drawn over the
//! cell until it is back in view. When the panel goes away (hidden, or no room left) while it
//! has the keyboard, the keyboard goes back to the cell's editor.
//!
//! Automation ids: `cell-editor` (the editor over its cell; value `{text, multiline}`, the text
//! cut to [`TAG_CHARS`] characters, `multiline` whether its kind is several lines) and, while the
//! value panel shows it, `value-editor` (the panel's text; value `{text, multiline, focused}`,
//! `focused` whether the keyboard is the panel's). The editor's text never reaches the logs: its
//! `Debug` leaves it out.

use std::sync::{Arc, Weak};

use dbee_core::json::{self, JsonKind};
use dbee_core::{ColumnInfo, ResultSet};
use dbee_engine::cmd::edit::can_edit;
use dbee_engine::{EngineError, Json, Screen, Session, Tab, TabId, Which};
use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;
use egui::{
    Align, Area, Event, EventFilter, Frame, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers, Order, Rect, ScrollArea, Stroke, TextEdit,
    UiBuilder, pos2, vec2,
};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::components::grid_cell::BREAK;
use crate::components::toast;
use crate::grid::{CELL, GridSource};
use crate::shortcuts;
use crate::theme::{self, Text, Words};

/// The longest value an editor opens inline, in bytes of text (see the module docs).
pub const MAX_INLINE_BYTES: usize = 32 * 1024;

/// The editor's automation id.
pub const EDITOR: &str = "cell-editor";

/// How much of the text the `cell-editor` tag shows.
pub const TAG_CHARS: usize = 200;

/// The value panel's view of the editor: its automation id.
pub const PANEL_EDITOR: &str = "value-editor";

/// The editor's edge, `ink` on `card` as the page's `editing` cell (the grid's outline), and the
/// text's inset as in the grid.
const BORDER: f32 = crate::theme::size::GRID_OUTLINE_W;
const PAD: f32 = crate::theme::size::GRID_CELL_PAD_X;

/// The keys an open editor keeps from focus moves: Tab, the arrows and Escape are its own.
const KEYS_KEPT: EventFilter = EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };

/// The id of the editor's text in the cell (one editor at a time).
fn text_id() -> Id {
    Id::new("dbee-cell-editor-text")
}

/// The id of the same text in the value panel.
fn panel_text_id() -> Id {
    Id::new("dbee-cell-editor-panel-text")
}

/// The id of the text that has the keyboard while the editor's focus is in `place`.
fn id_in(place: Place) -> Id {
    match place {
        Place::Cell => text_id(),
        Place::Panel => panel_text_id(),
    }
}

/// What a cell's value can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One line: Enter commits, in the cell and in the panel.
    Line,
    /// Several lines: in the panel Enter is a newline and Ctrl+Enter commits; in the cell Enter
    /// commits.
    Lines,
}

impl Kind {
    /// The kind for `column` (its catalog entry; one line when it is unknown).
    pub fn of(column: Option<&ColumnInfo>) -> Kind {
        match column {
            Some(c) if c.multiline() => Kind::Lines,
            _ => Kind::Line,
        }
    }
}

/// Which of the editor's two views has the keyboard and the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The one over its cell: where every editor opens.
    Cell,
    /// The one in the value panel, after a press in its text.
    Panel,
}

/// What the editor does once it has drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKey {
    Commit,
    /// Commit, then select the next cell (`back`: the one before).
    Move {
        back: bool,
    },
    Cancel,
}

/// The editor key `event` is for an editor of `kind`, if any. Modifiers must match exactly.
pub fn editor_key(event: &Event, kind: Kind) -> Option<EditorKey> {
    let Event::Key { key, pressed: true, modifiers: m, .. } = event else { return None };
    let is = |pattern: Modifiers| m.matches_exact(pattern);
    match key {
        Key::Escape if is(Modifiers::NONE) => Some(EditorKey::Cancel),
        Key::Tab if is(Modifiers::NONE) => Some(EditorKey::Move { back: false }),
        Key::Tab if is(Modifiers::SHIFT) => Some(EditorKey::Move { back: true }),
        Key::Enter if is(Modifiers::COMMAND) => Some(EditorKey::Commit),
        Key::Enter if kind != Kind::Lines && (is(Modifiers::NONE) || is(Modifiers::SHIFT)) => Some(EditorKey::Commit),
        _ => None,
    }
}

/// Splits `events` at the first editor key for `kind`: returns that key, and the keyboard
/// events after it, which are taken out to wait for the next frame. Pointer events stay: egui has
/// read them already.
pub fn split_at_key(events: &mut Vec<Event>, kind: Kind) -> (Option<EditorKey>, Vec<Event>) {
    let Some(at) = events.iter().position(|e| editor_key(e, kind).is_some()) else { return (None, Vec::new()) };
    let rest = events.split_off(at);
    let mut rest = rest.into_iter();
    let key = rest.next().and_then(|e| editor_key(&e, kind));
    let mut wait = Vec::new();
    for e in rest {
        if matches!(e, Event::Key { .. } | Event::Text(_) | Event::Paste(_) | Event::Copy | Event::Cut | Event::Ime(_)) {
            wait.push(e);
        } else {
            events.push(e);
        }
    }
    (key, wait)
}

/// The cell Tab selects after `at` in a grid of `rows` × `cols`: the next one to the right, then
/// the first of the next row; with `back` the other way. At either end of the grid it stays.
pub fn next_cell(at: (usize, usize), rows: usize, cols: usize, back: bool) -> (usize, usize) {
    let (row, col) = at;
    if rows == 0 || cols == 0 {
        return at;
    }
    let last = cols.saturating_sub(1);
    let col = col.min(last);
    if back {
        if col > 0 {
            (row, col - 1)
        } else if row > 0 {
            (row - 1, last)
        } else {
            (row, col)
        }
    } else if col < last {
        (row, col + 1)
    } else if row.saturating_add(1) < rows {
        (row + 1, 0)
    } else {
        (row, col)
    }
}

/// The open editor.
pub struct CellEditor {
    pub tab: TabId,
    pub row: usize,
    pub col: usize,
    pub kind: Kind,
    /// Which of its views has the keyboard: the cell's as it opens, the panel's after a press in
    /// the panel's text, the cell's again after a press in the cell's.
    pub place: Place,
    /// Its text gets JSON colours in the panel: a JSON column, or a value that opened as JSON.
    pub(crate) json: bool,
    /// The text it opened with (NULL as empty), and the text now.
    opened: String,
    pub text: String,
    /// The page it opened on, and the tab's staged edits then (`Edits::revision`): another page,
    /// or other edits, close it.
    page: Weak<ResultSet>,
    revision: u64,
    /// Where it was drawn over its cell in the last frame that drew it.
    rect: Option<Rect>,
    /// Where the value panel showed it in the last frame: the whole panel, its resize edge
    /// included, and the text's box in it. A press outside the cell's editor and the panel
    /// commits it; one in the text's box moves the keyboard there.
    panel: Option<(Rect, Rect)>,
    /// Not drawn yet: it takes the keyboard and selects its text when first drawn.
    fresh: bool,
    /// When (egui's clock) a fresh editor first found its cell out of view.
    unseen_since: Option<f64>,
    /// What to do once it has drawn this frame.
    pending: Option<EditorKey>,
}

/// Leaves the text out: it is a cell value.
impl std::fmt::Debug for CellEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellEditor")
            .field("tab", &self.tab)
            .field("row", &self.row)
            .field("col", &self.col)
            .field("kind", &self.kind)
            .field("place", &self.place)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl CellEditor {
    /// An editor of `kind` on cell `(row, col)` of tab `tab`, holding `value` (`None`: NULL), on
    /// `page`.
    pub fn new(tab: TabId, (row, col): (usize, usize), kind: Kind, value: Option<&str>, page: &Arc<ResultSet>, revision: u64) -> CellEditor {
        let text = value.unwrap_or_default().to_string();
        CellEditor {
            tab,
            row,
            col,
            kind,
            place: Place::Cell,
            json: false,
            opened: text.clone(),
            text,
            page: Arc::downgrade(page),
            revision,
            rect: None,
            panel: None,
            fresh: true,
            unseen_since: None,
            pending: None,
        }
    }

    /// The cell it edits, for `ui.inspect`: `{row, column}`.
    pub(crate) fn tag_cell(&self) -> Json {
        json!({ "row": self.row, "column": self.col })
    }

    /// Asks it to do `key` once it has drawn this frame (the panel's CANCEL and SAVE).
    pub(crate) fn ask(&mut self, key: EditorKey) {
        self.pending = Some(key);
    }

    /// The command that commits it, or `None` when nothing changed (see the module docs).
    pub fn command(&self) -> Option<(&'static str, Json)> {
        let (row, column) = (self.row, self.col);
        if self.text == self.opened {
            None
        } else if self.text.is_empty() {
            Some(("cell.clear", json!({ "row": row, "column": column })))
        } else {
            Some(("cell.edit", json!({ "row": row, "column": column, "value": self.text })))
        }
    }

    /// What the `cell-editor` and `value-editor` tags show.
    fn tag_value(&self) -> Json {
        let mut text: String = self.text.chars().take(TAG_CHARS).collect();
        if self.text.chars().nth(TAG_CHARS).is_some() {
            text.push('…');
        }
        json!({ "text": text, "multiline": self.kind == Kind::Lines })
    }

    /// How its keys read now: several lines only while the keyboard is in the panel.
    fn keys(&self) -> Kind {
        if self.place == Place::Panel { self.kind } else { Kind::Line }
    }
}

/// `n` bytes for a notice: `19.5 MiB`, `300 KiB`.
pub(crate) fn size_text(n: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if n >= MIB { format!("{:.1} MiB", n as f64 / MIB as f64) } else { format!("{} KiB", n.div_ceil(1024)) }
}

/// Why a value of `len` bytes can't be edited inline.
pub(crate) fn too_large(len: usize) -> Words {
    Words::from("This value is too large to edit inline (").name(size_text(len)).plain("; at most ").name(size_text(MAX_INLINE_BYTES)).plain(").")
}

/// Why a paste that would make the value `len` bytes was dropped.
pub(crate) fn pasted_too_large(len: usize) -> Words {
    Words::from("The pasted text is too large to edit inline: the value would be ")
        .name(size_text(len))
        .plain(" (at most ")
        .name(size_text(MAX_INLINE_BYTES))
        .plain(").")
}

/// The editor for cell `(row, col)` of the active tab; `Err` says why it cannot open, `Ok(None)`
/// when there is no such cell.
pub fn opening(s: &Session, row: usize, col: usize) -> Result<Option<CellEditor>, Words> {
    if let Err(why) = can_edit(s) {
        return Err(crate::app::notice_text(&EngineError::Disabled { id: "cell.edit".into(), why }).into());
    }
    let Some(Tab::Table(t)) = s.state().active_tab() else { return Ok(None) };
    let Some(data) = &t.data else { return Ok(None) };
    if row >= t.edits.grid_rows(data) || col >= data.columns() {
        return Ok(None);
    }
    if t.edits.deleted.contains(&row) {
        return Err("This row is marked for deletion: undo the delete to edit it.".into());
    }
    let value = t.edits.shown(data, row, col).unwrap_or_else(|| data.cell(row, col));
    if let Some(v) = value.filter(|v| v.len() > MAX_INLINE_BYTES) {
        return Err(too_large(v.len()));
    }
    let info = s.state().connection(Which::Tab(t.id)).and_then(|c| c.catalog.table(&t.table)).and_then(|i| i.column(data.column_name(col)));
    if let Some(why) = info.and_then(|i| i.not_editable()) {
        return Err(crate::app::notice_text(&EngineError::Disabled { id: "cell.edit".into(), why: why.into() }).into());
    }
    let mut editor = CellEditor::new(t.id, (row, col), Kind::of(info), value, data, t.edits.revision());
    editor.json = data.columns.get(col).is_some_and(|c| json::classify(c, value.unwrap_or_default()) != JsonKind::NotJson);
    Ok(Some(editor))
}

/// Opens an editor on cell `(row, col)` of the active tab, or says why it cannot.
pub(crate) fn open(app: &mut DbeeApp, ctx: &egui::Context, row: usize, col: usize) {
    // One open already is committed first (and then the new one sees its edit).
    commit(app, ctx);
    match opening(&app.session, row, col) {
        Ok(Some(editor)) => {
            app.view.work.editor = Some(editor);
            // A selection scrolled away: bring its cell into view, where the editor shows.
            let id = format!("{CELL}{row}-{col}");
            if app.tags.hidden(&id) {
                app.tags.scroll_to(&id);
            }
            ctx.request_repaint();
        }
        Ok(None) => {}
        Err(why) => app.notice(toast::Kind::Error, why),
    }
}

/// Why an open editor no longer fits the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unfit {
    /// Its tab is not the active one, shows another page, lost its cell or takes no edits.
    Gone,
    /// The tab's staged edits changed under it: the row it points at may be another one.
    EditsChanged,
}

/// Whether the open editor still fits the session: its tab is the active one and shows the same
/// page with the same staged edits, its cell is there and not marked for deletion, and the tab
/// takes edits.
fn still_fits(app: &DbeeApp) -> Result<(), Unfit> {
    let Some(ed) = &app.view.work.editor else { return Err(Unfit::Gone) };
    let s = &app.session;
    if s.state().screen != Screen::Workspace || can_edit(s).is_err() {
        return Err(Unfit::Gone);
    }
    let Some(Tab::Table(t)) = s.state().active_tab() else { return Err(Unfit::Gone) };
    let Some(data) = t.data.as_ref().filter(|d| t.id == ed.tab && std::ptr::eq(ed.page.as_ptr(), Arc::as_ptr(d))) else {
        return Err(Unfit::Gone);
    };
    if t.edits.revision() != ed.revision {
        return Err(Unfit::EditsChanged);
    }
    if ed.row < t.edits.grid_rows(data) && ed.col < data.columns() && !t.edits.deleted.contains(&ed.row) { Ok(()) } else { Err(Unfit::Gone) }
}

/// The notice when an editor closes without committing because of `why`: always when its
/// edits changed under it, else only when it held a change.
fn discarded(ed: &CellEditor, why: Unfit) -> Option<&'static str> {
    match why {
        Unfit::EditsChanged => Some("The cell edit was discarded: the staged changes changed while it was open."),
        Unfit::Gone if ed.command().is_some() => Some("The cell edit was discarded: its cell can no longer be edited here."),
        Unfit::Gone => None,
    }
}

/// Removes the open editor (its text state and the keyboard go with it) and returns it.
fn take(app: &mut DbeeApp, ctx: &egui::Context) -> Option<CellEditor> {
    let ed = app.view.work.editor.take()?;
    for id in [text_id(), panel_text_id()] {
        ctx.memory_mut(|m| m.surrender_focus(id));
        ctx.data_mut(|d| d.remove::<TextEditState>(id));
    }
    Some(ed)
}

/// How the open editor's keys read now, if one is open: it has the keyboard.
pub(crate) fn open_kind(app: &DbeeApp) -> Option<Kind> {
    app.view.work.editor.as_ref().map(CellEditor::keys)
}

/// Commits the open editor, if any, and closes it.
pub(crate) fn commit(app: &mut DbeeApp, ctx: &egui::Context) {
    let Some(ed) = take(app, ctx) else { return };
    // The cell commands act on the active tab.
    if app.session.state().active != Some(ed.tab) {
        return;
    }
    if let Some((command, params)) = ed.command() {
        app.start(command, params, Origin::Menu);
    }
}

/// Step 4 of the frame, before the shortcuts: with an editor open, checks that it still fits,
/// commits it on a press outside it (or when a dialog or the quick search opened over it), and
/// takes its keys; with none open, opens one on Enter over a selection (see the module docs).
pub(crate) fn before_frame(app: &mut DbeeApp, ctx: &egui::Context) {
    if app.view.work.editor.is_none() {
        // The text of an editor that went with its workspace.
        ctx.data_mut(|d| {
            d.remove::<TextEditState>(text_id());
            d.remove::<TextEditState>(panel_text_id());
        });
        open_on_enter(app, ctx);
        move_on_arrows(app, ctx);
        return;
    }
    if let Err(why) = still_fits(app) {
        if let Some(ed) = take(app, ctx)
            && let Some(text) = discarded(&ed, why)
        {
            app.notice(toast::Kind::Error, text.to_string());
        }
        return;
    }
    if app.view.dialog.is_some() || app.view.quick.open {
        commit(app, ctx);
        return;
    }
    // This frame's presses: one in the cell's editor or in the panel's text moves the keyboard
    // there, one elsewhere in the panel changes nothing, and one outside both commits.
    let Some(ed) = app.view.work.editor.as_mut() else { return };
    let (cell, panel) = (ed.rect, ed.panel.take());
    let presses: Vec<egui::Pos2> = ctx.input(|i| {
        let pressed = |e: &Event| match e {
            Event::PointerButton { pressed: true, pos, .. } => Some(*pos),
            _ => None,
        };
        i.events.iter().filter_map(pressed).collect()
    });
    for pos in presses {
        if cell.is_some_and(|r| r.contains(pos)) {
            ed.place = Place::Cell;
        } else if panel.is_some_and(|(_, text)| text.contains(pos)) {
            ed.place = Place::Panel;
        } else if !panel.is_some_and(|(whole, _)| whole.contains(pos)) {
            commit(app, ctx);
            return;
        }
    }
    let Some(ed) = app.view.work.editor.as_ref() else { return };
    let (kind, place) = (ed.keys(), ed.place);
    // The bytes the next paste replaces: the selection (all of the text until it is drawn).
    let selected = if ed.fresh {
        ed.text.len()
    } else {
        let range = TextEditState::load(ctx, id_in(place)).and_then(|s| s.cursor.char_range()).map(|r| r.as_sorted_char_range());
        range.map_or(0, |r| selected_bytes(&ed.text, r.start.0..r.end.0))
    };
    let length = ed.text.len();
    let (key, wait, dropped, broken) = ctx.input_mut(|i| {
        let (key, wait) = split_at_key(&mut i.events, kind);
        let dropped = take_large_pastes(&mut i.events, length, selected);
        // egui's one-line field would turn a paste's line breaks into spaces: the cell's editor
        // takes such a paste itself.
        let broken = if place == Place::Cell { take_broken_pastes(&mut i.events) } else { Vec::new() };
        (key, wait, dropped, broken)
    });
    if let Some(ed) = app.view.work.editor.as_mut() {
        ed.pending = key;
        for paste in broken {
            paste_whole(ctx, ed, &paste);
        }
    }
    app.carry.extend(wait);
    if let Some(n) = dropped {
        app.notice(toast::Kind::Error, pasted_too_large(n));
    }
}

/// Takes the pastes that hold a line break out of `events`, in order.
pub fn take_broken_pastes(events: &mut Vec<Event>) -> Vec<String> {
    let mut taken = Vec::new();
    events.retain(|e| match e {
        Event::Paste(text) if text.contains(['\n', '\r']) => {
            taken.push(text.clone());
            false
        }
        _ => true,
    });
    taken
}

/// `text` with `paste` in place of the characters `selected` (a range of character indices,
/// brought inside the text): the new text, and the character index just after the paste.
pub fn pasted_over(text: &str, selected: std::ops::Range<usize>, paste: &str) -> (String, usize) {
    let byte = |c: usize| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b);
    let (from, to) = (byte(selected.start), byte(selected.end.max(selected.start)));
    let mut out = String::with_capacity(text.len().saturating_add(paste.len()));
    out.push_str(text.get(..from).unwrap_or_default());
    out.push_str(paste);
    out.push_str(text.get(to..).unwrap_or_default());
    let at = text.get(..from).unwrap_or_default().chars().count().saturating_add(paste.chars().count());
    (out, at)
}

/// Puts `paste` into the cell's text as written, line breaks and all, over the selection (all of
/// the text in an editor not drawn yet), and the caret after it. The field's own undo sees the
/// change as it sees any other.
fn paste_whole(ctx: &egui::Context, ed: &mut CellEditor, paste: &str) {
    let mut state = TextEditState::load(ctx, text_id()).unwrap_or_default();
    let chars = ed.text.chars().count();
    let selected = if ed.fresh {
        0..chars
    } else {
        state.cursor.char_range().map(|r| r.as_sorted_char_range()).map_or(chars..chars, |r| r.start.0.min(chars)..r.end.0.min(chars))
    };
    let (text, at) = pasted_over(&ed.text, selected, paste);
    ed.text = text;
    // It has its selection now: the first frame's select-all would undo it.
    ed.fresh = false;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
    TextEdit::store_state(ctx, text_id(), state);
}

/// The bytes of `text` the characters `chars` (a range of character indices) take.
pub fn selected_bytes(text: &str, chars: std::ops::Range<usize>) -> usize {
    if chars.start >= chars.end {
        return 0;
    }
    let byte = |c: usize| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b);
    byte(chars.end).saturating_sub(byte(chars.start))
}

/// A paste of `paste` bytes over `selected` of a value of `length` bytes fits the cap.
pub fn paste_fits(length: usize, selected: usize, paste: usize) -> bool {
    length.saturating_sub(selected).saturating_add(paste) <= MAX_INLINE_BYTES
}

/// Takes out of `events` each paste that would make the value (`length` bytes, `selected` of
/// them selected) longer than [`MAX_INLINE_BYTES`], which would make every keystroke after it
/// slow; returns the longest value one would have made. A paste that goes in replaces the
/// selection, so the ones after it count from the value it leaves.
pub fn take_large_pastes(events: &mut Vec<Event>, length: usize, selected: usize) -> Option<usize> {
    let (mut length, mut selected) = (length, selected);
    let mut longest = None;
    events.retain(|e| match e {
        Event::Paste(text) if !paste_fits(length, selected, text.len()) => {
            longest = longest.max(Some(length.saturating_sub(selected).saturating_add(text.len())));
            false
        }
        Event::Paste(text) => {
            length = length.saturating_sub(selected).saturating_add(text.len());
            selected = 0;
            true
        }
        _ => true,
    });
    longest
}

/// Opens an editor on the selection's focus cell when Enter is pressed over a table tab with a
/// selection and no widget has the keyboard.
fn open_on_enter(app: &mut DbeeApp, ctx: &egui::Context) {
    let st = app.session.state();
    if st.screen != Screen::Workspace || app.view.dialog.is_some() || app.view.quick.open || ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    let Some(Tab::Table(t)) = st.active_tab() else { return };
    let Some(sel) = t.selection else { return };
    if ctx.input_mut(|i| {
        let held = i.modifiers;
        shortcuts::take(&mut i.events, Modifiers::NONE, Key::Enter, held)
    }) {
        open(app, ctx, sel.focus.0, sel.focus.1);
    }
}

/// The grid's arrow keys: with a selection in the active tab's grid and nothing holding the
/// keyboard (no widget, dialog, quick search, menu or list), an arrow moves the selection one
/// cell, and with Shift extends it (`selection.move`); a row selection moves and extends by whole
/// rows. A focus cell out of view is scrolled to.
fn move_on_arrows(app: &mut DbeeApp, ctx: &egui::Context) {
    let st = app.session.state();
    let selected = match st.active_tab() {
        Some(Tab::Table(t)) => t.selection.is_some() && !shortcuts::grid_unseen(app),
        Some(Tab::Query(q)) => q.selection.is_some(),
        None => false,
    };
    if !selected
        || st.screen != Screen::Workspace
        || app.view.dialog.is_some()
        || app.view.quick.open
        || ctx.memory(|m| m.focused()).is_some()
        || egui::Popup::is_any_open(ctx)
    {
        return;
    }
    const ARROWS: [(Key, i64, i64); 4] = [(Key::ArrowUp, -1, 0), (Key::ArrowDown, 1, 0), (Key::ArrowLeft, 0, -1), (Key::ArrowRight, 0, 1)];
    let mut steps = Vec::new();
    ctx.input_mut(|i| {
        let held = i.modifiers;
        for (key, rows, columns) in ARROWS {
            for (pattern, extend) in [(Modifiers::NONE, false), (Modifiers::SHIFT, true)] {
                while shortcuts::take(&mut i.events, pattern, key, held) {
                    steps.push((rows, columns, extend));
                }
            }
        }
    });
    if steps.is_empty() {
        return;
    }
    for (rows, columns, extend) in steps {
        app.start("selection.move", json!({ "rows": rows, "columns": columns, "extend": extend }), Origin::Menu);
    }
    let selection = match app.session.state().active_tab() {
        Some(Tab::Table(t)) => t.selection,
        Some(Tab::Query(q)) => q.selection,
        None => None,
    };
    if let Some(sel) = selection {
        let (row, col) = sel.focus;
        // Whole rows scroll up and down only: to the row's cell in the first column in view.
        let col = if sel.whole { (0..=col).find(|c| !app.tags.hidden(&format!("grid-header-{c}"))).unwrap_or(0) } else { col };
        let id = format!("{CELL}{row}-{col}");
        if app.tags.hidden(&id) {
            app.tags.scroll_to(&id);
        }
    }
    ctx.request_repaint();
}

/// After step 5 (drawing): does what the editor's key asked for.
pub(crate) fn after_draw(app: &mut DbeeApp, ctx: &egui::Context) {
    let Some(ed) = app.view.work.editor.as_mut() else { return };
    let Some(key) = ed.pending.take() else { return };
    match key {
        EditorKey::Commit => commit(app, ctx),
        EditorKey::Cancel => {
            take(app, ctx);
        }
        EditorKey::Move { back } => {
            let at = (ed.row, ed.col);
            commit(app, ctx);
            let size = match app.session.state().active_tab() {
                Some(Tab::Table(t)) => t.data.as_ref().map(|d| (t.edits.grid_rows(d), d.columns())),
                _ => None,
            };
            if let Some((rows, cols)) = size {
                let (row, col) = next_cell(at, rows, cols, back);
                app.start("selection.set", json!({ "anchor": [row, col], "focus": [row, col] }), Origin::Menu);
                let id = format!("{CELL}{row}-{col}");
                if app.tags.hidden(&id) {
                    app.tags.scroll_to(&id);
                }
            }
        }
    }
}

/// Draws `ed` over its cell, whose rect is `cell`, kept inside `view` (the grid's rows in view).
pub(crate) fn show(ctx: &egui::Context, ed: &mut CellEditor, cell: Rect, view: Rect, tags: &mut Tags) {
    let t = theme::tokens(ctx);
    // One line, the cell's own size, whatever the value holds.
    let (width, height) = (cell.width(), cell.height());
    let inner_w = (width - 2.0 * BORDER).max(1.0);
    let inner_h = (height - 2.0 * BORDER).max(1.0);
    // Placed here rather than by egui's constraint, which goes by the size the area had last:
    // the last editor's, in an editor's first frame.
    let at = place(cell.min, vec2(width, height), view);
    let area = Area::new(Id::new("dbee-cell-editor")).order(Order::Foreground).fixed_pos(at).constrain(false).movable(false).fade_in(false);
    let r = area
        .show(ctx, |ui| {
            Frame::new().fill(t.card).stroke(Stroke::new(BORDER, t.ink)).show(ui, |ui| {
                ui.set_width(inner_w);
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                text_box(ui, ed, vec2(inner_w, inner_h));
            })
        })
        .response;
    ed.rect = Some(r.rect);
    tags.tag_value(EDITOR, &r, ed.tag_value());
}

/// Where an editor of `size` goes to start at `at` and stay inside `view`: moved left or up as
/// far as it has to, but never past `view`'s left or top.
pub fn place(at: egui::Pos2, size: egui::Vec2, view: Rect) -> egui::Pos2 {
    let x = at.x.min(view.right() - size.x).max(view.left());
    let y = at.y.min(view.bottom() - size.y).max(view.top());
    pos2(x, y)
}

/// Whether `cell` still shows among the rows and columns in view, `view`.
pub fn in_view(cell: Rect, view: Rect) -> bool {
    let both = cell.intersect(view);
    both.width() > 0.0 && both.height() > 0.0
}

/// How long (egui's clock, in seconds) a fresh editor waits for its cell to scroll into view.
pub const FRESH_WAIT: f64 = 1.0;

/// Its cell left the rows and columns in view: one drawn over it is not drawn, and commits once this frame is
/// drawn, as a click elsewhere would. One not drawn yet waits for its cell to scroll in, up to
/// [`FRESH_WAIT`]: past that (a grid with no room for a row, say) it closes unchanged, so an
/// editor nobody sees does not go on taking Enter and Escape. `now` is egui's clock.
pub(crate) fn left_view(ed: &mut CellEditor, now: f64) {
    ed.rect = None;
    // While the keyboard is in the panel, the editor stays open there.
    if ed.place == Place::Panel {
        return;
    }
    if ed.pending.is_some() {
        return;
    }
    if !ed.fresh {
        ed.pending = Some(EditorKey::Commit);
    } else if now - *ed.unseen_since.get_or_insert(now) > FRESH_WAIT {
        ed.pending = Some(EditorKey::Cancel);
    }
}

/// Gives the editor's text in `view` the keyboard when that is where its focus is; a fresh
/// editor's cell text also gets a new state with all of it selected.
fn take_keyboard(ui: &egui::Ui, ed: &mut CellEditor, view: Place) {
    if ed.place != view {
        return;
    }
    let id = id_in(view);
    if ed.fresh && view == Place::Cell {
        ed.fresh = false;
        ui.memory_mut(|m| m.request_focus(id));
        // A new state: all of the text selected, and no undo history from another cell.
        let mut state = TextEditState::default();
        let end = ed.text.chars().count();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(end))));
        TextEdit::store_state(ui.ctx(), id, state);
    } else if !ui.memory(|m| m.has_focus(id)) {
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// What the cell's one line draws for a line break: the grid's mark where the font has it, else
/// a space.
fn break_mark(ui: &egui::Ui) -> char {
    if ui.fonts_mut(|f| f.has_glyph(&Text::Data.font(), BREAK)) { BREAK } else { ' ' }
}

/// The cell's text editor, at least `size`: one line in `data`, scrolling sideways with the
/// caret. A line break in the value is a character of the line like any other, drawn as
/// [`break_mark`], so the caret steps over it, it can be selected and deleted, and it stays when
/// it is not; one in pasted text is kept too ([`paste_whole`]).
fn text_box(ui: &mut egui::Ui, ed: &mut CellEditor, size: egui::Vec2) {
    take_keyboard(ui, ed, Place::Cell);
    let (mark, ink) = (break_mark(ui), theme::tokens(ui.ctx()).text);
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
        // A character for a character: the caret's place in the line is its place in the value.
        let shown: String = buf
            .as_str()
            .chars()
            .map(|c| match c {
                '\n' | '\r' => mark,
                c => c,
            })
            .collect();
        let mut job = Text::Data.job_as_written(&shown, ink);
        job.wrap.max_width = f32::INFINITY;
        job.break_on_newline = false;
        job.keep_trailing_whitespace = true;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let edit = TextEdit::singleline(&mut ed.text)
        .return_key(None::<KeyboardShortcut>)
        .id(text_id())
        // The text sits where the grid draws it (the frame's margin is the text's; `margin` is
        // not used with a frame of one's own).
        .frame(Frame::NONE.inner_margin(Margin::symmetric((PAD - BORDER) as i8, 2)))
        .font(Text::Data.font())
        .desired_width(f32::INFINITY)
        .min_size(size)
        .vertical_align(egui::Align::Center)
        .layouter(&mut layouter)
        .event_filter(KEYS_KEPT);
    ui.add(edit);
}

/// Draws `ed`'s text in the value panel (see the module docs), the same draft the cell's editor
/// holds: in `area`, the panel's value box ([`crate::components::value::Shown::text`]), wrapped
/// at the box's width when `wrap`, else scrolling sideways. It has the caret only while the
/// editor's focus is the panel's. `panel` is the whole panel, its resize edge included: a press
/// there does not commit the editor, and one in `area` moves the keyboard here.
pub(crate) fn show_in_panel(ui: &mut egui::Ui, ed: &mut CellEditor, area: Rect, panel: Rect, wrap: bool, tags: &mut Tags) {
    let t = *theme::tokens(ui.ctx());
    take_keyboard(ui, ed, Place::Panel);
    let json = ed.json;
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = panel_job(buf.as_str(), json, &t);
        job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut inner = ui.new_child(UiBuilder::new().id_salt("dbee-cell-editor-panel").max_rect(area).layout(Layout::top_down(Align::Min)));
    inner.set_clip_rect(area.intersect(ui.clip_rect()));
    let scroll = if wrap { ScrollArea::vertical() } else { ScrollArea::both() };
    let inset = PANEL_PAD as i8;
    let out = scroll.id_salt("dbee-cell-editor-panel-scroll").auto_shrink([false, false]).show(&mut inner, |ui| {
        let size = ui.available_size();
        TextEdit::multiline(&mut ed.text)
            .id(panel_text_id())
            // The text starts where the panel's value does: [`PANEL_PAD`] in, at the box's top.
            .frame(Frame::NONE.inner_margin(Margin { left: inset, right: inset, top: 0, bottom: 0 }))
            .font(Text::Code.font())
            .desired_width(f32::INFINITY)
            .min_size(size)
            .layouter(&mut layouter)
            .event_filter(KEYS_KEPT)
            .show(ui)
            .response
    });
    let mut r = out.inner;
    // Its tag's rect is the text's box, not the text scrolled past it.
    r.rect = out.inner_rect;
    r.interact_rect = out.inner_rect;
    ed.panel = Some((panel, out.inner_rect));
    let mut value = ed.tag_value();
    value["focused"] = json!(ed.place == Place::Panel);
    tags.tag_value(PANEL_EDITOR, &r, value);
}

/// The panel's value inset, as the panel draws a value.
const PANEL_PAD: f32 = crate::components::value::PAD;

/// The layout of `text` being edited in the panel, in `code`: with `json`, coloured as JSON up to
/// the first spot `json::highlight` stops at (punctuation `text_muted`), and plain `text` from
/// there on (text that is JSON whole keeps its closing brackets `text_muted`); else all in `text`.
/// Unwrapped: the caller sets the width.
pub fn panel_job(text: &str, json: bool, t: &theme::Tokens) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut add = |range: std::ops::Range<usize>, colour: egui::Color32| {
        if let Some(s) = text.get(range).filter(|s| !s.is_empty()) {
            job.append(s, 0.0, Text::Code.format(colour));
        }
    };
    let mut at = 0;
    if json {
        for (range, token) in json::highlight(text) {
            add(at..range.start, t.text_muted);
            add(range.clone(), crate::components::value::token_colour(token, t));
            at = range.end;
        }
    }
    let tail = if json && json::is_valid(text) { t.text_muted } else { t.text };
    add(at..text.len(), tail);
    job
}

#[cfg(test)]
mod tests {
    use dbee_core::{Column, TypeClass};
    use egui::CornerRadius;

    use super::*;

    fn page() -> Arc<ResultSet> {
        Arc::new(ResultSet {
            columns: vec![Column::untyped("a")],
            rows: vec![vec![Some("x".into())]],
            rows_seen: 1,
            complete: true,
            rows_affected: None,
            database_after: Default::default(),
        })
    }

    fn editor(kind: Kind, value: Option<&str>) -> CellEditor {
        CellEditor::new(TabId(1), (2, 3), kind, value, &page(), 0)
    }

    /// `ed` drawn over `cell` in `theme` until it has the keyboard; with `collapsed`, its caret
    /// then put at the end of its text, as after typing. The last frame's shapes.
    fn drawn(theme: egui::Theme, ed: CellEditor, cell: Rect, collapsed: bool) -> Vec<egui::epaint::ClippedShape> {
        let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        let mut h = crate::components::harness_in(vec2(600.0, 300.0), theme, (ed, Tags::default()), move |ui, (ed, tags)| {
            tags.begin_frame();
            show(ui.ctx(), ed, cell, view, tags);
        });
        h.run_steps(3);
        if collapsed {
            let mut state = TextEdit::load_state(&h.ctx, text_id()).unwrap_or_default();
            let end = h.state().0.text.chars().count();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(end))));
            TextEdit::store_state(&h.ctx, text_id(), state);
        }
        h.run_steps(2);
        h.output().shapes.clone()
    }

    /// Text edited in the panel: JSON whole keeps its closing brackets `text_muted`, as the panel
    /// draws a value; past the first spot that isn't JSON the text is plain `text`.
    #[test]
    fn the_panel_editors_json_keeps_its_closing_bracket_muted() {
        let t = theme::LIGHT;
        let colour = |text: &str| {
            // The last section holds the last character.
            panel_job(text, true, &t).sections.last().map(|s| s.format.color)
        };
        assert_eq!(colour("{\n  \"a\": [1]\n}"), Some(t.text_muted), "JSON whole");
        assert_eq!(colour("{\"a\": 1,}"), Some(t.text), "past the stop");
    }

    #[test]
    fn the_editor_takes_the_pages_editing_look() {
        // `grid/cell` (node 166898), `editing` (167055) and `editing selected` (167068): over its
        // cell, on `card`, square, a 2 px `ink` edge inside it (the grid's outline); the value in
        // `data` `text` 8 in, as the grid draws it, 5 down in the 26 tall row; the caret 1 px
        // `ink` down the value's 16 tall line, after it (egui draws it 1.5 longer at either end,
        // as in every input). As an editor opens, all of its text is selected (`editing
        // selected`): `text_selection` behind it, the caret at its end.
        use crate::components::{Edge, both, edges_in, fills_in, sections, texts};
        let cell = Rect::from_min_size(pos2(20.0, 30.0), vec2(150.0, 26.0));
        for (theme, t) in both() {
            for selected in [true, false] {
                let at_ = format!("{theme:?} {}", if selected { "editing selected" } else { "editing" });
                let shapes = drawn(theme, editor(Kind::Line, Some("Customer 191")), cell, !selected);
                assert_eq!(fills_in(&shapes, cell), [(cell, t.card)], "{at_}: on `card`, over the whole cell");
                let edge = Edge { outer: cell, width: 2.0, colour: t.ink, radius: CornerRadius::ZERO };
                assert_eq!(edges_in(&shapes, cell, 0.0), [edge], "{at_}: a 2 px `ink` edge inside the cell, square");
                let (_, value, colour) = texts(&shapes)
                    .into_iter()
                    .find(|(w, _, _)| w == "Customer 191")
                    .unwrap_or_else(|| panic!("{at_}: the value: {:?}", texts(&shapes)));
                assert_eq!(colour, t.text, "{at_}: the value in `text`");
                assert_eq!(sections(&shapes, "Customer 191").first().map(|s| s.1.clone()), Some(Text::Data.font()), "{at_}: in `data`");
                let close = |a: f32, b: f32| (a - b).abs() <= 0.5;
                assert!(
                    close(value.left(), cell.left() + 8.0) && close(value.top(), cell.top() + 5.0) && close(value.height(), 16.0),
                    "{at_}: the value 8 in, 5 down, 16 tall: {value:?} in {cell:?}"
                );
                // The caret: 1 px `ink` down the value's line, at its end, as tall as egui draws it.
                let carets: Vec<_> = crate::components::flat(&shapes)
                    .into_iter()
                    .filter_map(|s| match s {
                        egui::Shape::LineSegment { points, stroke } => Some((points, stroke)),
                        _ => None,
                    })
                    .collect();
                let [([top, bottom], stroke)] = carets.as_slice() else { panic!("{at_}: one caret: {carets:?}") };
                assert_eq!(*stroke, Stroke::new(1.0, t.ink), "{at_}: the caret 1 px `ink`");
                assert!(
                    close(top.x, value.right()) && close(bottom.x, top.x) && close(top.y, value.top() - 1.5) && close(bottom.y, value.bottom() + 1.5),
                    "{at_}: the caret down the value's line at its end: {top:?} {bottom:?}, the value {value:?}"
                );
                // Selected: `text_selection` behind the value, as wide as it.
                let ground: Vec<egui::Pos2> = crate::components::flat(&shapes)
                    .into_iter()
                    .filter_map(|s| match s {
                        egui::Shape::Text(text) => Some(
                            text.galley
                                .rows
                                .iter()
                                .flat_map(|row| {
                                    row.visuals.mesh.vertices.iter().filter(|v| v.color == t.text_selection).map(|v| v.pos + row.pos.to_vec2())
                                })
                                .map(|p| p + text.pos.to_vec2())
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if selected {
                    let behind = Rect::from_points(&ground);
                    assert!(
                        close(behind.left(), value.left()) && close(behind.right(), value.right()) && close(behind.height(), 16.0),
                        "{at_}: `text_selection` behind the value: {behind:?}, the value {value:?}"
                    );
                } else {
                    assert!(ground.is_empty(), "{at_}: nothing selected: {ground:?}");
                }
            }
        }
    }

    #[test]
    fn a_paste_with_line_breaks_goes_in_as_written() {
        // Taken out of the frame's input, in order; one-line pastes stay for the field.
        let mut events = vec![Event::Paste("a\nb".into()), Event::Text("x".into()), Event::Paste("plain".into()), Event::Paste("c\r\nd".into())];
        assert_eq!(take_broken_pastes(&mut events), ["a\nb", "c\r\nd"]);
        assert_eq!(events, [Event::Text("x".into()), Event::Paste("plain".into())]);
        // Over a selection counted in characters, the caret after it.
        assert_eq!(pasted_over("héllo", 1..3, "A\nB"), ("hA\nBlo".to_string(), 4));
        assert_eq!(pasted_over("one\ntwo", 7..7, "\r\n3"), ("one\ntwo\r\n3".to_string(), 10));
        assert_eq!(pasted_over("abc", 0..3, "x\ny"), ("x\ny".to_string(), 3));
        assert_eq!(pasted_over("", 0..0, "\n"), ("\n".to_string(), 1));
        // A selection past the text, or the wrong way round, is brought inside it.
        assert_eq!(pasted_over("ab", 5..9, "\n"), ("ab\n".to_string(), 3));
        let backwards = std::ops::Range { start: 1, end: 0 };
        assert_eq!(pasted_over("ab", backwards, "\n"), ("a\nb".to_string(), 2));
    }

    #[test]
    fn enter_is_a_newline_only_for_several_lines_with_the_keyboard_in_the_panel() {
        let mut e = editor(Kind::Lines, Some("a"));
        assert_eq!((e.place, e.keys()), (Place::Cell, Kind::Line), "it opens in its cell, where Enter commits");
        e.place = Place::Panel;
        assert_eq!(e.keys(), Kind::Lines);
        let mut one = editor(Kind::Line, Some("1"));
        one.place = Place::Panel;
        assert_eq!(one.keys(), Kind::Line, "a one-line kind commits on Enter in the panel too");
    }

    #[test]
    fn unchanged_sends_nothing_empty_clears_and_text_edits() {
        let mut e = editor(Kind::Line, Some("10.50"));
        assert_eq!(e.command(), None, "unchanged");
        e.text = "7".into();
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": "7" }))));
        e.text.clear();
        assert_eq!(e.command(), Some(("cell.clear", json!({ "row": 2, "column": 3 }))));
        // NULL opens empty: Enter on it alone sends nothing; typing then deleting it neither.
        let mut e = editor(Kind::Lines, None);
        assert_eq!((e.text.as_str(), e.command()), ("", None));
        e.text = "a\nb".into();
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": "a\nb" }))));
        e.text.clear();
        assert_eq!(e.command(), None);
    }

    #[test]
    fn tab_moves_right_then_down_and_stays_at_the_ends() {
        assert_eq!(next_cell((0, 0), 3, 6, false), (0, 1));
        assert_eq!(next_cell((1, 5), 3, 6, false), (2, 0));
        assert_eq!(next_cell((2, 5), 3, 6, false), (2, 5), "the last cell of the last row");
        assert_eq!(next_cell((1, 0), 3, 6, true), (0, 5));
        assert_eq!(next_cell((0, 0), 3, 6, true), (0, 0));
        assert_eq!(next_cell((4, 9), 0, 0, false), (4, 9));
        assert_eq!(next_cell((0, 99), 1, 3, false), (0, 2), "a column past the edge counts as the last");
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn keys_depend_on_the_kind_and_match_exactly() {
        let ctrl = Modifiers::COMMAND | Modifiers::CTRL;
        let enter = key(Key::Enter, Modifiers::NONE);
        assert_eq!(editor_key(&enter, Kind::Line), Some(EditorKey::Commit));
        assert_eq!(editor_key(&enter, Kind::Lines), None, "a newline in several lines");
        assert_eq!(editor_key(&key(Key::Enter, ctrl), Kind::Lines), Some(EditorKey::Commit));
        assert_eq!(editor_key(&key(Key::Enter, ctrl | Modifiers::SHIFT), Kind::Line), None, "Ctrl+Shift+Enter is no editor key");
        assert_eq!(editor_key(&key(Key::Tab, Modifiers::NONE), Kind::Lines), Some(EditorKey::Move { back: false }));
        assert_eq!(editor_key(&key(Key::Tab, Modifiers::SHIFT), Kind::Line), Some(EditorKey::Move { back: true }));
        assert_eq!(editor_key(&key(Key::Escape, Modifiers::NONE), Kind::Line), Some(EditorKey::Cancel));
        assert_eq!(editor_key(&key(Key::ArrowUp, Modifiers::NONE), Kind::Line), None, "the caret's");
        assert_eq!(editor_key(&key(Key::Z, ctrl), Kind::Line), None);
        assert_eq!(editor_key(&Event::Text("a".into()), Kind::Line), None);
        let released = Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE };
        assert_eq!(editor_key(&released, Kind::Line), None);
    }

    #[test]
    fn keys_after_the_first_editor_key_wait_and_text_before_it_stays() {
        let moved = Event::PointerMoved(pos2(1.0, 2.0));
        let mut events =
            vec![Event::Text("a".into()), key(Key::Enter, Modifiers::NONE), Event::Text("b".into()), moved.clone(), key(Key::Tab, Modifiers::NONE)];
        let (k, wait) = split_at_key(&mut events, Kind::Line);
        assert_eq!(k, Some(EditorKey::Commit));
        assert_eq!(events, vec![Event::Text("a".into()), moved], "text before the key, and the pointer, stay");
        assert_eq!(wait, vec![Event::Text("b".into()), key(Key::Tab, Modifiers::NONE)]);
        let mut none = vec![Event::Text("a".into()), key(Key::Enter, Modifiers::NONE)];
        assert_eq!(split_at_key(&mut none, Kind::Lines), (None, Vec::new()), "Enter is text in several lines");
        assert_eq!(none.len(), 2);
    }

    #[test]
    fn the_tag_cuts_long_text_and_debug_leaves_it_out() {
        let long = "é".repeat(TAG_CHARS + 5);
        let e = editor(Kind::Lines, Some(&long));
        let v = e.tag_value();
        assert_eq!(v["multiline"], true);
        assert_eq!(v["text"].as_str().map(|t| t.chars().count()), Some(TAG_CHARS + 1));
        let e = editor(Kind::Line, Some("secret value"));
        assert!(!format!("{e:?}").contains("secret"));
        assert_eq!((size_text(MAX_INLINE_BYTES), size_text(20_000_000)), ("32 KiB".to_string(), "19.1 MiB".to_string()));
    }

    #[test]
    fn a_paste_counts_what_it_replaces_and_what_came_before() {
        let cap = MAX_INLINE_BYTES;
        assert!(paste_fits(cap - 10, 0, 10) && !paste_fits(cap - 10, 0, 11));
        assert!(paste_fits(cap, cap, cap), "a paste over everything replaces it");
        assert!(paste_fits(cap, 5, 5) && !paste_fits(cap, 5, 6));
        // Two pastes into a half-full value: the first fits, the second then does not.
        let half = cap / 2;
        let mut events = vec![Event::Paste("a".repeat(half - 100)), Event::Text("x".into()), Event::Paste("b".repeat(200))];
        assert_eq!(take_large_pastes(&mut events, half, 0), Some(cap + 100));
        assert_eq!(events.len(), 2, "the second paste went");
        // The selection the first paste replaces counts once.
        let mut events = vec![Event::Paste("c".repeat(cap)), Event::Paste("d".into())];
        assert_eq!(take_large_pastes(&mut events, cap, cap), Some(cap + 1));
        assert_eq!(events, vec![Event::Paste("c".repeat(cap))]);
        // Bytes, not characters: `é` is two.
        assert_eq!(selected_bytes("aébc", 1..3), 3);
        assert_eq!((selected_bytes("abc", 2..2), selected_bytes("abc", 1..9), selected_bytes("", 0..1)), (0, 2, 0));
    }

    #[test]
    fn a_panel_editor_is_not_committed_by_scrolling() {
        // In the value panel the editor is not over its cell: its cell scrolled away leaves it
        // open. Over its cell (no room for the panel) it commits, as before.
        let mut e = editor(Kind::Lines, Some("a"));
        e.place = Place::Panel;
        e.fresh = false;
        left_view(&mut e, 10.0);
        assert_eq!(e.pending, None, "in the panel: still open");
        left_view(&mut e, 20.0);
        assert_eq!(e.pending, None, "and later too");
        let mut e = editor(Kind::Lines, Some("a"));
        assert_eq!(e.place, Place::Cell, "over its cell unless placed in the panel");
        e.fresh = false;
        left_view(&mut e, 10.0);
        assert_eq!(e.pending, Some(EditorKey::Commit), "over its cell: it commits");
    }

    #[test]
    fn a_cell_is_in_view_while_any_of_it_shows() {
        let view = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 400.0));
        let cell = |x: f32, y: f32| Rect::from_min_size(pos2(x, y), vec2(80.0, 24.0));
        assert!(in_view(cell(200.0, 100.0), view));
        assert!(in_view(cell(40.0, 100.0), view), "partly, at the left");
        assert!(!in_view(cell(10.0, 100.0), view), "scrolled away to the left");
        assert!(!in_view(cell(500.0, 100.0), view), "touching is not showing");
        assert!(!in_view(cell(200.0, 26.0), view), "under the header");
    }

    #[test]
    fn editors_stay_inside_the_rows_in_view() {
        let view = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 400.0));
        assert_eq!(place(pos2(200.0, 100.0), vec2(80.0, 24.0), view), pos2(200.0, 100.0), "fits where it is");
        assert_eq!(place(pos2(450.0, 100.0), vec2(120.0, 75.0), view), pos2(380.0, 100.0), "moved left at the right edge");
        assert_eq!(place(pos2(200.0, 390.0), vec2(80.0, 75.0), view), pos2(200.0, 325.0), "moved up at the bottom");
        assert_eq!(place(pos2(90.0, 40.0), vec2(80.0, 24.0), view), pos2(100.0, 50.0), "a cell half out of view");
        assert_eq!(place(pos2(200.0, 100.0), vec2(900.0, 24.0), view), pos2(100.0, 100.0), "wider than the view: from its left");
    }

    /// A boolean column, as the catalog lists one.
    fn bool_column() -> ColumnInfo {
        ColumnInfo {
            name: "c".into(),
            type_sql: "boolean".into(),
            base_type: "bool".into(),
            class: TypeClass::Bool,
            not_null: false,
            has_default: false,
            generated: Default::default(),
            hidden: false,
            type_label: String::new(),
            operators: dbee_core::Operators::Unnamed,
        }
    }

    #[test]
    fn a_boolean_column_opens_one_line() {
        // Booleans are plain text: the cell's text as the database prints it, Enter commits, an
        // emptied editor is NULL (`cell.clear`), and the engine spells a boolean word.
        let kind = Kind::of(Some(&bool_column()));
        assert_eq!(kind, Kind::Line);
        let mut e = editor(kind, Some("t"));
        assert_eq!((e.kind, e.text.as_str(), e.command()), (Kind::Line, "t", None), "unchanged: nothing");
        assert_eq!(e.tag_value(), json!({ "text": "t", "multiline": false }));
        assert_eq!(editor_key(&key(Key::Enter, Modifiers::NONE), kind), Some(EditorKey::Commit));
        e.text = "false".into();
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": "false" }))));
        e.text.clear();
        assert_eq!(e.command(), Some(("cell.clear", json!({ "row": 2, "column": 3 }))));
        // NULL opens empty; a MySQL BOOLEAN holding 5 opens on the 5.
        assert_eq!(editor(kind, None).text, "");
        assert_eq!(editor(kind, Some("5")).text, "5");
    }

    #[test]
    fn kinds_follow_the_catalog() {
        let mut c = bool_column();
        assert_eq!(Kind::of(Some(&c)), Kind::Line, "a boolean is one line");
        c.class = TypeClass::Json;
        assert_eq!(Kind::of(Some(&c)), Kind::Lines);
        c.class = TypeClass::LongText;
        assert_eq!(Kind::of(Some(&c)), Kind::Lines);
        for class in [TypeClass::Text, TypeClass::Number, TypeClass::DateTime, TypeClass::Binary, TypeClass::Other] {
            c.class = class;
            assert_eq!(Kind::of(Some(&c)), Kind::Line, "{class:?}");
        }
        assert_eq!(Kind::of(None), Kind::Line);
    }
}

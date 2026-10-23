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
//! longer than [`MAX_INLINE_BYTES`] does not open inline (a notice says so); nor does a paste
//! longer than that go in.
//!
//! **The cap.** egui lays out a paragraph (text between line breaks) whole on every change, at
//! about 0.4 µs a byte in a release build, so a keystroke in one long line or one long JSON
//! value costs ~13 ms at 32 KiB, ~25 ms at 64 KiB and ~130 ms at 256 KiB; text of short lines
//! costs far less, as each paragraph is cached. 32 KiB keeps a keystroke within about a frame in
//! the worst case (`keystroke_frame_with_a_value_at_the_cap`, docs/testing.md §6).
//!
//! **Kinds** ([`Kind`], from the column's catalog entry): one line, where Enter commits; several
//! lines for `text`, `json`, `jsonb` and `xml` (`ColumnInfo::multiline`), where Enter is a newline
//! and Ctrl+Enter commits; and a true / false / NULL choice for booleans (category `B`), moved
//! with the arrow keys or clicked.
//!
//! **Keys.** While an editor is open it has the keyboard: Enter or Ctrl+Enter commits, Tab
//! commits and selects the next cell to the right, then the start of the next row (Shift+Tab goes
//! back; at the grid's last cell the selection stays), Escape cancels. These keys are taken from
//! the frame's input before the shortcuts and the widgets see it ([`before_frame`]), so the
//! global Ctrl+Enter does not run too, and act once the editor has drawn ([`after_draw`]): text
//! typed earlier in the same frame is in the value, and keys typed after one wait for the next
//! frame (`DbeeApp::feed_input`). Ctrl+Z and Ctrl+Shift+Z are the editor's own undo and redo (see
//! [`crate::shortcuts`]); any other shortcut commits the editor before it runs, so Ctrl+S writes
//! the value being typed.
//!
//! **Committing** ([`CellEditor::command`]): an unchanged value (the text it opened with, or the
//! same choice) sends nothing; empty text sends `cell.clear` (`''` for string types, NULL for
//! the others); other text sends `cell.edit`; true and false send `cell.edit`, NULL
//! `cell.setNull`. A press anywhere outside the editor commits it before anything else sees the
//! press, so clicking another cell commits and selects that cell. An editor whose tab, page or
//! cell is gone, or that can no longer stage, closes without committing.
//!
//! Automation id: `cell-editor` (value `{text, multiline}` for text, the text cut to
//! [`TAG_CHARS`] characters; `{choice}` for a boolean, `true`, `false` or `null`), and the
//! choices `cell-editor-true`, `cell-editor-false` and `cell-editor-null`. The editor's text never
//! reaches the logs: its `Debug` leaves it out.

use std::sync::{Arc, Weak};

use dbee_core::{ColumnInfo, ResultSet};
use dbee_engine::cmd::edit::can_edit;
use dbee_engine::{EngineError, Json, Screen, Session, Tab, TabId};
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{
    Area, CornerRadius, Event, EventFilter, FontId, Frame, Id, Key, KeyboardShortcut, Margin, Modifiers, Order, Rect, ScrollArea, Sense, Stroke,
    TextEdit, pos2, vec2,
};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::grid::{CELL, GridSource, ROW_H};
use crate::shortcuts;
use crate::theme::TOKENS;

/// The longest value an editor opens inline, in bytes of text (see the module docs).
pub const MAX_INLINE_BYTES: usize = 32 * 1024;

/// The editor's automation id; its choices add `-true`, `-false` and `-null`.
pub const EDITOR: &str = "cell-editor";

/// How much of the text the `cell-editor` tag shows.
pub const TAG_CHARS: usize = 200;

/// The narrowest a several-lines editor or a boolean choice is.
const LINES_W: f32 = 320.0;
const CHOICE_W: f32 = 120.0;
/// The tallest a several-lines editor grows before it scrolls.
const LINES_MAX_H: f32 = 180.0;
/// The editor's border, and the text's inset as in the grid.
const BORDER: f32 = 1.5;
const PAD: f32 = 6.0;

/// The keys an open editor keeps from focus moves: Tab, the arrows and Escape are its own.
const KEYS_KEPT: EventFilter = EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };

/// The text editor's id (one editor at a time).
fn text_id() -> Id {
    Id::new("dbee-cell-editor-text")
}

/// The boolean choice's id, which holds the keyboard while it is open.
fn choice_id() -> Id {
    Id::new("dbee-cell-editor-choice")
}

/// How a cell's value is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One line: Enter commits.
    Line,
    /// Several lines: Enter is a newline, Ctrl+Enter commits.
    Lines,
    /// True, false or NULL.
    Bool,
}

impl Kind {
    /// The kind for `column` (its catalog entry; one line when it is unknown).
    pub fn of(column: Option<&ColumnInfo>) -> Kind {
        match column {
            Some(c) if c.category == 'B' => Kind::Bool,
            Some(c) if c.multiline() => Kind::Lines,
            _ => Kind::Line,
        }
    }
}

/// A boolean editor's choices, in the order it lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    True,
    False,
    Null,
}

impl Choice {
    pub const ALL: [Choice; 3] = [Choice::True, Choice::False, Choice::Null];

    /// Its name in automation ids and values.
    pub fn id(self) -> &'static str {
        match self {
            Choice::True => "true",
            Choice::False => "false",
            Choice::Null => "null",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Choice::True => "true",
            Choice::False => "false",
            Choice::Null => "NULL",
        }
    }

    /// The choice a boolean cell's value is. Postgres prints `t` and `f`; a staged value is as it
    /// was typed, in any of the spellings Postgres reads. Other text is no choice.
    pub fn of(value: Option<&str>) -> Option<Choice> {
        let Some(v) = value else { return Some(Choice::Null) };
        match v.trim().to_ascii_lowercase().as_str() {
            "t" | "true" | "y" | "yes" | "on" | "1" => Some(Choice::True),
            "f" | "false" | "n" | "no" | "off" | "0" => Some(Choice::False),
            _ => None,
        }
    }

    /// The choice one step up (`-1`) or down (`1`) the list, stopping at its ends.
    fn step(self, down: bool) -> Choice {
        let at = Choice::ALL.iter().position(|c| *c == self).unwrap_or(0);
        let to = if down { at.saturating_add(1).min(Choice::ALL.len() - 1) } else { at.saturating_sub(1) };
        Choice::ALL.get(to).copied().unwrap_or(self)
    }
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
    /// The boolean choice moves up or down.
    Up,
    Down,
    /// A boolean choice was clicked: take it and commit.
    Pick(Choice),
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
        Key::ArrowUp if kind == Kind::Bool && is(Modifiers::NONE) => Some(EditorKey::Up),
        Key::ArrowDown if kind == Kind::Bool && is(Modifiers::NONE) => Some(EditorKey::Down),
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
    /// The text it opened with (NULL as empty), and the text now.
    opened: String,
    pub text: String,
    /// For a boolean: the choice it opened with (`None` for text that is no choice), and the
    /// choice now.
    opened_choice: Option<Choice>,
    pub choice: Choice,
    /// The page it opened on: another page closes it.
    page: Weak<ResultSet>,
    /// Where it was drawn in the last frame that drew it: a press outside commits it.
    rect: Option<Rect>,
    /// Not drawn yet: it takes the keyboard and selects its text when first drawn.
    fresh: bool,
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
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl CellEditor {
    /// An editor of `kind` on cell `(row, col)` of tab `tab`, holding `value` (`None`: NULL), on
    /// `page`.
    pub fn new(tab: TabId, (row, col): (usize, usize), kind: Kind, value: Option<&str>, page: &Arc<ResultSet>) -> CellEditor {
        let text = value.unwrap_or_default().to_string();
        let opened_choice = Choice::of(value);
        CellEditor {
            tab,
            row,
            col,
            kind,
            opened: text.clone(),
            text,
            opened_choice,
            choice: opened_choice.unwrap_or(Choice::True),
            page: Arc::downgrade(page),
            rect: None,
            fresh: true,
            pending: None,
        }
    }

    /// The command that commits it, or `None` when nothing changed (see the module docs).
    pub fn command(&self) -> Option<(&'static str, Json)> {
        let (row, column) = (self.row, self.col);
        match self.kind {
            Kind::Bool => {
                if self.opened_choice == Some(self.choice) {
                    return None;
                }
                Some(match self.choice {
                    Choice::True => ("cell.edit", json!({ "row": row, "column": column, "value": true })),
                    Choice::False => ("cell.edit", json!({ "row": row, "column": column, "value": false })),
                    Choice::Null => ("cell.setNull", json!({ "row": row, "column": column })),
                })
            }
            Kind::Line | Kind::Lines => {
                if self.text == self.opened {
                    None
                } else if self.text.is_empty() {
                    Some(("cell.clear", json!({ "row": row, "column": column })))
                } else {
                    Some(("cell.edit", json!({ "row": row, "column": column, "value": self.text })))
                }
            }
        }
    }

    /// What the `cell-editor` tag shows.
    fn tag_value(&self) -> Json {
        match self.kind {
            Kind::Bool => json!({ "choice": self.choice.id() }),
            Kind::Line | Kind::Lines => {
                let mut text: String = self.text.chars().take(TAG_CHARS).collect();
                if self.text.chars().nth(TAG_CHARS).is_some() {
                    text.push('…');
                }
                json!({ "text": text, "multiline": self.kind == Kind::Lines })
            }
        }
    }
}

/// `n` bytes for a notice: `19.5 MiB`, `300 KiB`.
fn size_text(n: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if n >= MIB { format!("{:.1} MiB", n as f64 / MIB as f64) } else { format!("{} KiB", n.div_ceil(1024)) }
}

/// The editor for cell `(row, col)` of the active tab; `Err` says why it cannot open, `Ok(None)`
/// when there is no such cell.
pub fn opening(s: &Session, row: usize, col: usize) -> Result<Option<CellEditor>, String> {
    if let Err(why) = can_edit(s) {
        return Err(EngineError::Disabled { id: "cell.edit".into(), why }.to_string());
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
        return Err(format!("This value is too large to edit inline ({}; at most {}).", size_text(v.len()), size_text(MAX_INLINE_BYTES)));
    }
    let info = s.state().connected.as_ref().and_then(|c| c.catalog.table(&t.table)).and_then(|i| i.column(data.column_name(col)));
    Ok(Some(CellEditor::new(t.id, (row, col), Kind::of(info), value, data)))
}

/// Opens an editor on cell `(row, col)` of the active tab, or says why it cannot.
pub(crate) fn open(app: &mut DbeeApp, ctx: &egui::Context, row: usize, col: usize) {
    match opening(&app.session, row, col) {
        Ok(Some(editor)) => {
            take(app, ctx);
            app.view.work.editor = Some(editor);
            ctx.request_repaint();
        }
        Ok(None) => {}
        Err(why) => app.notice(why),
    }
}

/// The open editor still fits the session: its tab is the active one and shows the same page,
/// its cell is there and not marked for deletion, and the tab takes edits.
fn still_fits(app: &DbeeApp) -> bool {
    let Some(ed) = &app.view.work.editor else { return false };
    let s = &app.session;
    if s.state().screen != Screen::Workspace || can_edit(s).is_err() {
        return false;
    }
    match s.state().active_tab() {
        Some(Tab::Table(t)) if t.id == ed.tab => match &t.data {
            Some(data) => {
                std::ptr::eq(ed.page.as_ptr(), Arc::as_ptr(data))
                    && ed.row < t.edits.grid_rows(data)
                    && ed.col < data.columns()
                    && !t.edits.deleted.contains(&ed.row)
            }
            None => false,
        },
        _ => false,
    }
}

/// Removes the open editor (its text state and the keyboard go with it) and returns it.
fn take(app: &mut DbeeApp, ctx: &egui::Context) -> Option<CellEditor> {
    let ed = app.view.work.editor.take()?;
    ctx.memory_mut(|m| {
        m.surrender_focus(text_id());
        m.surrender_focus(choice_id());
    });
    ctx.data_mut(|d| d.remove::<TextEditState>(text_id()));
    Some(ed)
}

/// An editor is open: it has the keyboard.
pub(crate) fn is_open(app: &DbeeApp) -> bool {
    app.view.work.editor.is_some()
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
        ctx.data_mut(|d| d.remove::<TextEditState>(text_id()));
        open_on_enter(app, ctx);
        return;
    }
    if !still_fits(app) {
        take(app, ctx);
        return;
    }
    if app.view.dialog.is_some() || app.view.work.quick.open {
        commit(app, ctx);
        return;
    }
    let rect = app.view.work.editor.as_ref().and_then(|e| e.rect);
    let outside = ctx
        .input(|i| i.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, pos, .. } if !rect.is_some_and(|r| r.contains(*pos)))));
    if outside {
        commit(app, ctx);
        return;
    }
    let Some(kind) = app.view.work.editor.as_ref().map(|e| e.kind) else { return };
    let (key, wait, too_large) = ctx.input_mut(|i| {
        let (key, wait) = split_at_key(&mut i.events, kind);
        (key, wait, take_large_pastes(&mut i.events))
    });
    if let Some(ed) = app.view.work.editor.as_mut() {
        ed.pending = key;
    }
    app.carry.extend(wait);
    if let Some(n) = too_large {
        app.notice(format!("The pasted text is too large to edit inline ({}; at most {}).", size_text(n), size_text(MAX_INLINE_BYTES)));
    }
}

/// Takes out of `events` each paste longer than [`MAX_INLINE_BYTES`], which would make every
/// keystroke after it slow; returns the length of the longest.
pub fn take_large_pastes(events: &mut Vec<Event>) -> Option<usize> {
    let mut longest = None;
    events.retain(|e| match e {
        Event::Paste(text) if text.len() > MAX_INLINE_BYTES => {
            longest = longest.max(Some(text.len()));
            false
        }
        _ => true,
    });
    longest
}

/// Opens an editor on the selection's focus cell when Enter is pressed over a table tab with a
/// selection and no widget has the keyboard.
fn open_on_enter(app: &mut DbeeApp, ctx: &egui::Context) {
    let st = app.session.state();
    if st.screen != Screen::Workspace || app.view.dialog.is_some() || app.view.work.quick.open || ctx.memory(|m| m.focused()).is_some() {
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

/// After step 5 (drawing): does what the editor's key, or a click on a choice, asked for.
pub(crate) fn after_draw(app: &mut DbeeApp, ctx: &egui::Context) {
    let Some(ed) = app.view.work.editor.as_mut() else { return };
    let Some(key) = ed.pending.take() else { return };
    match key {
        EditorKey::Up | EditorKey::Down => {
            ed.choice = ed.choice.step(key == EditorKey::Down);
            ctx.request_repaint();
        }
        EditorKey::Pick(choice) => {
            ed.choice = choice;
            commit(app, ctx);
        }
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
    let t = TOKENS;
    let width = match ed.kind {
        Kind::Line => cell.width(),
        Kind::Lines => cell.width().max(LINES_W),
        Kind::Bool => cell.width().max(CHOICE_W),
    };
    let inner_w = (width - 2.0 * BORDER).max(1.0);
    let inner_h = (cell.height() - 2.0 * BORDER).max(1.0);
    // Placed here rather than by egui's constraint, which goes by the size the area had last:
    // the last editor's, in an editor's first frame. Several lines grow: their height is this
    // editor's own from its last frame.
    let height = match ed.kind {
        Kind::Line => cell.height(),
        Kind::Bool => ROW_H * Choice::ALL.len() as f32 + 2.0 * BORDER,
        Kind::Lines => ed.rect.map_or(cell.height(), |r| r.height()),
    };
    let at = place(cell.min, vec2(width, height), view);
    let area = Area::new(Id::new("dbee-cell-editor")).order(Order::Foreground).fixed_pos(at).constrain(false).movable(false).fade_in(false);
    let r = area
        .show(ctx, |ui| {
            Frame::new().fill(t.card).stroke(Stroke::new(BORDER, t.accent)).show(ui, |ui| {
                ui.set_width(inner_w);
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                match ed.kind {
                    Kind::Bool => choices(ui, ed, inner_w, tags),
                    Kind::Line | Kind::Lines => text_box(ui, ed, vec2(inner_w, inner_h)),
                }
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

/// Its cell is out of view this frame: it is not drawn, and a press anywhere commits it.
pub(crate) fn hide(ed: &mut CellEditor) {
    ed.rect = None;
}

/// The text editor, at least `size`.
fn text_box(ui: &mut egui::Ui, ed: &mut CellEditor, size: egui::Vec2) {
    let id = text_id();
    if ed.fresh {
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
    let lines = ed.kind == Kind::Lines;
    let edit = if lines {
        TextEdit::multiline(&mut ed.text).desired_rows(1)
    } else {
        TextEdit::singleline(&mut ed.text).return_key(None::<KeyboardShortcut>)
    };
    let edit = edit
        .id(id)
        // The text sits where the grid draws it (the frame's margin is the text's; `margin` is
        // not used with a frame of one's own).
        .frame(Frame::NONE.inner_margin(Margin::symmetric((PAD - BORDER) as i8, 2)))
        .font(FontId::proportional(13.0))
        .desired_width(f32::INFINITY)
        .min_size(size)
        .vertical_align(egui::Align::Center)
        .event_filter(KEYS_KEPT);
    if lines {
        ScrollArea::vertical().id_salt("dbee-cell-editor-scroll").max_height(LINES_MAX_H).auto_shrink([false, true]).show(ui, |ui| ui.add(edit));
    } else {
        ui.add(edit);
    }
}

/// The boolean choice: three rows, the current one highlighted.
fn choices(ui: &mut egui::Ui, ed: &mut CellEditor, width: f32, tags: &mut Tags) {
    let t = TOKENS;
    let id = choice_id();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H * Choice::ALL.len() as f32), Sense::hover());
    // Holds the keyboard (so Tab and the arrows do not move it elsewhere) without taking clicks.
    let holder = ui.interact(rect, id, Sense::focusable_noninteractive());
    if !holder.has_focus() {
        holder.request_focus();
    }
    ui.memory_mut(|m| m.set_focus_lock_filter(id, KEYS_KEPT));
    ed.fresh = false;
    for (i, choice) in Choice::ALL.iter().copied().enumerate() {
        let row = Rect::from_min_size(pos2(rect.left(), rect.top() + ROW_H * i as f32), vec2(width, ROW_H));
        let r = ui.interact(row, id.with(i), Sense::click());
        let fill = if choice == ed.choice {
            t.selection
        } else if r.hovered() {
            t.hover
        } else {
            t.card
        };
        ui.painter().rect_filled(row, CornerRadius::ZERO, fill);
        let null = choice == Choice::Null;
        let colour = if null { t.text_muted } else { t.text };
        let format = TextFormat { font_id: FontId::proportional(13.0), color: colour, italics: null, ..TextFormat::default() };
        let galley = ui.painter().layout_job(LayoutJob::single_section(choice.label().to_string(), format));
        ui.painter().galley(pos2(row.left() + PAD - BORDER, row.center().y - galley.size().y / 2.0), galley, colour);
        tags.tag_value(&format!("{EDITOR}-{}", choice.id()), &r, choice.label());
        if r.clicked() && ed.pending.is_none() {
            ed.pending = Some(EditorKey::Pick(choice));
        }
    }
}

#[cfg(test)]
mod tests {
    use dbee_core::Column;

    use super::*;

    fn page() -> Arc<ResultSet> {
        Arc::new(ResultSet {
            columns: vec![Column::untyped("a")],
            rows: vec![vec![Some("x".into())]],
            rows_seen: 1,
            complete: true,
            rows_affected: None,
        })
    }

    fn editor(kind: Kind, value: Option<&str>) -> CellEditor {
        CellEditor::new(TabId(1), (2, 3), kind, value, &page())
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
    fn booleans_send_true_false_or_set_null() {
        let mut e = editor(Kind::Bool, Some("t"));
        assert_eq!((e.choice, e.command()), (Choice::True, None));
        e.choice = Choice::False;
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": false }))));
        e.choice = Choice::Null;
        assert_eq!(e.command(), Some(("cell.setNull", json!({ "row": 2, "column": 3 }))));
        let mut e = editor(Kind::Bool, None);
        assert_eq!((e.choice, e.command()), (Choice::Null, None));
        e.choice = Choice::True;
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": true }))));
        // Text that is no boolean: any choice is a change.
        let e = editor(Kind::Bool, Some("maybe"));
        assert_eq!(e.command(), Some(("cell.edit", json!({ "row": 2, "column": 3, "value": true }))));
        for (text, choice) in [("TRUE", Some(Choice::True)), ("off", Some(Choice::False)), (" yes ", Some(Choice::True)), ("2", None)] {
            assert_eq!(Choice::of(Some(text)), choice, "{text}");
        }
        assert_eq!((Choice::True.step(false), Choice::True.step(true), Choice::Null.step(true)), (Choice::True, Choice::False, Choice::Null));
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
        assert_eq!(editor_key(&enter, Kind::Bool), Some(EditorKey::Commit));
        assert_eq!(editor_key(&key(Key::Enter, ctrl), Kind::Lines), Some(EditorKey::Commit));
        assert_eq!(editor_key(&key(Key::Enter, ctrl | Modifiers::SHIFT), Kind::Line), None, "Ctrl+Shift+Enter is no editor key");
        assert_eq!(editor_key(&key(Key::Tab, Modifiers::NONE), Kind::Lines), Some(EditorKey::Move { back: false }));
        assert_eq!(editor_key(&key(Key::Tab, Modifiers::SHIFT), Kind::Line), Some(EditorKey::Move { back: true }));
        assert_eq!(editor_key(&key(Key::Escape, Modifiers::NONE), Kind::Bool), Some(EditorKey::Cancel));
        assert_eq!(editor_key(&key(Key::ArrowUp, Modifiers::NONE), Kind::Bool), Some(EditorKey::Up));
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
    fn a_paste_past_the_cap_does_not_go_in() {
        let big = "x".repeat(MAX_INLINE_BYTES + 1);
        let mut events = vec![Event::Paste("small".into()), Event::Paste(big), Event::Text("a".into())];
        assert_eq!(take_large_pastes(&mut events), Some(MAX_INLINE_BYTES + 1));
        assert_eq!(events, vec![Event::Paste("small".into()), Event::Text("a".into())]);
        assert_eq!(take_large_pastes(&mut events), None);
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

    #[test]
    fn kinds_follow_the_catalog() {
        let mut c = ColumnInfo {
            name: "c".into(),
            type_sql: "boolean".into(),
            type_oid: 16,
            base_type: "bool".into(),
            category: 'B',
            not_null: false,
            has_default: false,
        };
        assert_eq!(Kind::of(Some(&c)), Kind::Bool);
        (c.base_type, c.category) = ("jsonb".into(), 'U');
        assert_eq!(Kind::of(Some(&c)), Kind::Lines);
        (c.base_type, c.category) = ("date".into(), 'D');
        assert_eq!(Kind::of(Some(&c)), Kind::Line);
        assert_eq!(Kind::of(None), Kind::Line);
    }
}

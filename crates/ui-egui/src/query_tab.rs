//! The query tab's body ([`show`]), from top to bottom:
//!
//! - the SQL editor ([`editor`]), with line numbers in a gutter at its left
//!   ([`crate::components::editor::LineNumber`]: the cursor's line in `text`, the line the last
//!   run's error points at in `danger`);
//! - the split: dragged, it moves the line between the editor and the results. Where it is lasts
//!   for the window's run, in memory, and is the same for every query tab (`View::query_split`);
//! - the toolbar: on an engine with `USE` (`Dialect::has_use_database`), first the database chip:
//!   the database the tab's next run starts in, as the engine knows it (`Connected::starts_in`:
//!   the tab's own, else where the query connection is), in the engine's word for one
//!   ([`database_chip`]); a click lists the catalog's databases, the server's own left out, and
//!   choosing one sends `query.setDatabase`. It waits while the tab runs, and while Run cannot.
//!   Then Run (`query.run`), Run current (`query.run` with `current` and the cursor) and Cancel
//!   (`query.cancel`, enabled while the tab's run goes), a spinner while it runs (with its words
//!   when the selector keeps room), and with several results the selector `1 · 2 · 3`
//!   (`query.showResult`; with more than fit, those around the shown one and a `…` that lists
//!   every result, its label only where it fits whole, [`selector_fit`]); at its right, what the
//!   shown result holds: `3 rows`, or the cap notice `First 10,000 of 25,000 rows` ([`rows_text`]);
//! - the last run's error band, when it stopped early ([`band`], [`crate::components::error_band`]);
//! - the shown result: a read-only grid ([`crate::grid`]: no sorting, no editing, no links;
//!   selecting and copying work, and the value panel follows the selected cell), or for a
//!   statement without rows how many it affected ([`affected_text`]). With no result yet, a line
//!   saying how to run.
//!
//! The grid takes the body's whole width; the rest keeps [`size::TAB_BODY_PAD_X`] in from its
//! sides.
//!
//! **The editor.** Monospace text coloured by `dbee_sql::lexer::tokenize` in the open connection's
//! dialect, Postgres's when none is open ([`Tokens::job`], colours from [`crate::theme::Tokens`]; a
//! new dialect lexes the text again from scratch), with no soft wrap: a long line scrolls
//! sideways, so each row of the layout is a line and the gutter's numbers are line numbers. The
//! text lives in the window, a buffer per tab ([`EditorState`]), so a frame does not copy it: the
//! frame compares it with the tab's (an agent's `query.setText` replaces it) and sends each change
//! with `query.setText` (journaled, consecutive ones coalesce). The layouter keeps the job it made
//! for the last text, theme and coloured lines ([`Highlighter`]), and tokenizes only a text it has
//! not seen, lexing again only around the change ([`Tokens`]). Only the lines near the view are
//! coloured ([`colour_window`]: whole chunks of [`CHUNK`] lines, [`MARGIN`] beyond the view each
//! side), so the job of a big script stays small for egui to hash and split; egui then lays out
//! again only the paragraphs (lines) that changed. The editor does not animate its scrolling, so a
//! jump far down is coloured at once. The text has no size cap; the keystroke cost of big scripts
//! is measured by `keystroke_frames_in_big_scripts` (docs/testing.md §6): a script of short lines
//! costs about what egui's plain text editor does, and one very long line costs as much to lay
//! out as it does there, on every keystroke.
//!
//! **Keys.** Ctrl+Enter runs every statement and Ctrl+Shift+Enter the one under the cursor: they
//! are shortcuts ([`crate::shortcuts`]), taken before the editor sees them, so no newline goes
//! in. Tab inserts [`INDENT`] (four spaces) and keeps the keyboard in the editor; Shift+Tab does
//! nothing (no outdent in M1). Ctrl+A, Ctrl+C, Ctrl+Z and Ctrl+Shift+Z are the editor's while it
//! has the keyboard, and the result grid's otherwise. The cursor, a byte offset into the text,
//! goes with Ctrl+Shift+Enter and Run current (`View::query_cursor`); before the editor ever had
//! one it is the start. A tab's editor takes the keyboard when the tab becomes the active one (a
//! new tab too), unless a dialog or the quick search is open.
//!
//! **Errors.** The band shows the server's message (with its code), hint and detail, and where
//! the server points, as a line and column of the text (as it is now: the position is a byte
//! offset into the text the run started with, kept within the text), whose number turns `danger`
//! in the gutter too. A click on the band, or Enter or Space with the keyboard on it, puts the
//! cursor there, with the keyboard; an error without a position offers no click, and its words can
//! be selected. A cancelled run shows the band in neutral colours. The results of the statements
//! before the failing one stay, and can be picked in the selector. The band says the error, so it
//! shows as no notice too (`DbeeApp::start`).
//!
//! **Run buttons** follow the engine's own enabled check for `query.run` (`cmd::query::can_run`:
//! connected, the connection not lost), with its reason on hover. What the run itself refuses (a
//! text with no statement, another run going) it says when clicked, in a notice, as the keyboard
//! gets it.
//!
//! It reads the tab from the engine each frame and acts only through commands, run after drawing.
//! The window keeps a [`QueryState`] per tab, dropped with the tab.
//!
//! Automation ids: `query-editor` (value `{cursor, focused, lines, length, highlighted}`: the
//! cursor as a byte offset, whether the editor has the keyboard, the lines, the text's length in
//! bytes, and whether some token is coloured), `query-split` (value: the editor's share of the
//! height), `query-database` (value: the database shown, or null for none; while its list is open
//! the choices `query-database-<name>`), `query-run`, `query-run-current`, `query-cancel`,
//! `result-<n>` (1-based, with several results; value `{shown, rows, rowsAffected}`), `result-more`
//! (the `…` when not all fit; value: the number of results) and, while its list is open,
//! `result-list-<n>` (value as `result-<n>`; only those in view),
//! `query-rows` (value: what the shown result holds, e.g. `First 10,000 of 25,000 rows`),
//! `query-affected` (value: `2 rows affected`), `query-error` (value `{code, heading, message,
//! detail, hint, note, position, statement, line, column, jump}`, `note` the band's line that a
//! ROLLBACK ends a failed transaction (an error whose meaning is `TransactionAborted`, Postgres's
//! 25P02) or null, `jump` saying whether a click moves the cursor) and `query-hint`.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use dbee_core::ResultSet;
use dbee_engine::cmd::query::{can_run, can_set_database};
use dbee_engine::state::NO_CONNECTION_DIALECT;
use dbee_engine::{CurrentDatabase, Json, Meaning, QueryError, StatementOutcome, Tab, TabId, Which};
use dbee_sql::Dialect;
use dbee_sql::lexer::{self, Token, TokenKind};
use egui::text::{ByteIndex, CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat};
use egui::text_edit::TextEditState;
use egui::{
    Align, Button, Color32, CornerRadius, CursorIcon, Event, Frame, Id, Key, Label, Layout, Margin, Popup, Rect, Response, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextEdit, UiBuilder, Widget, pos2, vec2,
};
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::components::button::{BoxButton, TextButton};
use crate::components::editor::LineNumber;
use crate::components::error_band::Band;
use crate::components::icon::Icon;
use crate::components::menu::{self, Item};
use crate::components::spinner::Spinner;
use crate::components::toast::Kind;
use crate::components::tooltip::Hint;
use crate::connections::line_galley;
use crate::grid::{Grid, GridAct, GridState, RowEdits};
use crate::theme::{
    self, Text,
    size::{self, RADIUS_CONTROL},
};
use crate::tree::thousands;

/// The automation ids.
pub const EDITOR: &str = "query-editor";
pub const SPLIT: &str = "query-split";
/// The database chip; its list's choices are `query-database-<name>`.
pub const DATABASE: &str = "query-database";
pub const RUN: &str = "query-run";
pub const RUN_CURRENT: &str = "query-run-current";
pub const CANCEL: &str = "query-cancel";
/// The selector's buttons: `result-1`, `result-2`, … (`query.showResult` takes the 0-based index).
pub const RESULT: &str = "result-";
/// The selector's `…` when not all results fit, and its list's items `result-list-1`, ….
pub const RESULT_MORE: &str = "result-more";
pub const RESULT_LIST: &str = "result-list-";
pub const ROWS: &str = "query-rows";
pub const AFFECTED: &str = "query-affected";
pub const ERROR: &str = "query-error";
pub const HINT: &str = "query-hint";

/// The height of an editor line.
const LINE_H: f32 = Text::Code.line();
/// What Tab inserts.
const INDENT: &str = "    ";
/// The editor's share of the tab's height until the split is dragged.
pub(crate) const DEFAULT_SPLIT: f32 = 0.45;

/// The run command, whose job errors the band shows.
const QUERY_RUN: &str = "query.run";
/// The editor's height at least (three lines), and the room kept for the results.
const MIN_EDITOR_H: f32 = 3.0 * LINE_H + 10.0;
const MIN_RESULTS_H: f32 = 90.0;
/// The space above the editor, the split's height and the toolbar's.
const TOP_PAD: f32 = 6.0;
const SPLIT_H: f32 = 8.0;
const TOOLBAR_H: f32 = 36.0;
/// The database chip's width; it is as tall as the toolbar's buttons.
const CHIP_W: f32 = 150.0;
/// How tall the chip's list gets before it scrolls.
const CHIP_LIST_H: f32 = 320.0;
/// The gutter's inset at each side of its numbers.
const GUTTER_PAD: f32 = 8.0;
/// The text's inset in the editor.
const TEXT_MARGIN: Margin = Margin { left: 6, right: 6, top: 4, bottom: 4 };

/// The id of tab `tab`'s editor (egui keeps its cursor and undo under it).
pub fn editor_id(tab: TabId) -> Id {
    Id::new(("query-editor", tab.0))
}

// ---- text helpers ------------------------------------------------------------------------------

/// The character index of byte offset `byte` in `text`: clamped to its end, and a byte inside a
/// character counts as that character's start.
fn char_index(text: &str, byte: usize) -> usize {
    let mut end = byte.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).map_or(0, |s| s.chars().count())
}

/// The byte offset of character `chars` of `text`, clamped to its end.
fn byte_offset(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(at, _)| at)
}

/// The 1-based line and column (in characters) of byte offset `byte` in `text`, clamped to it.
fn line_column(text: &str, byte: usize) -> (usize, usize) {
    let chars = char_index(text, byte);
    let before = text.get(..byte_offset(text, chars)).unwrap_or_default();
    let line = before.bytes().filter(|b| *b == b'\n').count().saturating_add(1);
    let start = before.rfind('\n').map_or(0, |at| at.saturating_add(1));
    let column = before.get(start..).map_or(0, |s| s.chars().count()).saturating_add(1);
    (line, column)
}

fn rows_word(n: u64) -> String {
    if n == 1 { "1 row".into() } else { format!("{} rows", thousands(n)) }
}

/// The cap notice of a result cut short: `First 10,000 of 25,000 rows`, from the rows kept and
/// the rows the server sent.
fn cut_notice(kept: usize, seen: u64) -> String {
    let total = rows_word(seen);
    format!("First {} of {total}", thousands(kept as u64))
}

/// What a result with rows holds: `3 rows`, or the cap notice when a cap cut it.
fn rows_text(result: &ResultSet) -> String {
    if result.complete { rows_word(result.rows.len() as u64) } else { cut_notice(result.rows.len(), result.rows_seen) }
}

/// What the cap notice says when hovered: the caps of a run.
fn cap_hover() -> String {
    let limits = dbee_engine::cmd::query::RUN_LIMITS;
    format!(
        "A run keeps at most {} rows of each result, and {} MiB of memory for all its results together.",
        thousands(limits.max_rows as u64),
        limits.max_bytes / (1024 * 1024)
    )
}

/// The line for a statement without rows: `1 row affected`, `2 rows affected`, or a line of its
/// own when the database gave no count.
fn affected_text(rows: Option<u64>) -> String {
    match rows {
        Some(n) => format!("{} affected", rows_word(n)),
        None => "The statement ran; the database gave no row count.".into(),
    }
}

// ---- highlighting ------------------------------------------------------------------------------

/// How far past a change the lexer first looks for the old tokens to fit again.
const RESYNC_BYTES: usize = 1024;
/// The editor colours the lines in view and [`MARGIN`] more each side, in whole chunks of
/// [`CHUNK`] lines, so scrolling changes what is coloured only now and then.
const CHUNK: usize = 128;
const MARGIN: usize = 64;

/// The lines coloured when lines `view` are in view (see [`CHUNK`]).
fn colour_window(view: Range<usize>) -> Range<usize> {
    let start = view.start.saturating_sub(MARGIN) / CHUNK * CHUNK;
    let end = (view.end.saturating_add(MARGIN) / CHUNK).saturating_add(1).saturating_mul(CHUNK);
    start..end
}

/// The colour and slant of a token of `kind`; `None` for whitespace, which joins the text before it.
fn style(kind: TokenKind, syntax: &theme::Tokens, plain: Color32) -> Option<(Color32, bool)> {
    Some(match kind {
        TokenKind::Keyword => (syntax.syn_keyword, false),
        TokenKind::String => (syntax.syn_string, false),
        TokenKind::Number | TokenKind::Param => (syntax.syn_number, false),
        TokenKind::Comment => (syntax.text_muted, true),
        TokenKind::QuotedIdent => (syntax.syn_literal, false),
        TokenKind::Whitespace => return None,
        TokenKind::Ident | TokenKind::Operator | TokenKind::Semicolon | TokenKind::Other | TokenKind::ExecutableMark => (plain, false),
    })
}

/// The bytes `a` and `b` start with alike.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let chunks = a.as_chunks::<64>().0.iter().zip(b.as_chunks::<64>().0).take_while(|(x, y)| x == y).count();
    let at = chunks.saturating_mul(64);
    let rest = a.get(at..).unwrap_or_default().iter().zip(b.get(at..).unwrap_or_default()).take_while(|(x, y)| x == y).count();
    at.saturating_add(rest)
}

/// The bytes `a` and `b` end with alike, at most `most`.
fn common_suffix(a: &[u8], b: &[u8], most: usize) -> usize {
    let chunks = a.as_rchunks::<64>().1.iter().rev().zip(b.as_rchunks::<64>().1.iter().rev()).take_while(|(x, y)| x == y).count();
    let at = chunks.saturating_mul(64).min(most);
    let a_rest = a.get(..a.len().saturating_sub(at)).unwrap_or_default();
    let b_rest = b.get(..b.len().saturating_sub(at)).unwrap_or_default();
    let rest = a_rest.iter().rev().zip(b_rest.iter().rev()).take_while(|(x, y)| x == y).count();
    at.saturating_add(rest).min(most)
}

/// The nearest character boundary of `text` at or before `at`, and at or after it.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at = at.saturating_sub(1);
    }
    at
}

fn ceil_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at = at.saturating_add(1);
    }
    at
}

/// `tokenize(slice)`, its ranges moved to start at `offset`.
fn lex(dialect: Dialect, slice: &str, offset: usize) -> Vec<Token> {
    lexer::tokenize(dialect, slice)
        .into_iter()
        .map(|t| Token { kind: t.kind, range: t.range.start.saturating_add(offset)..t.range.end.saturating_add(offset) })
        .collect()
}

/// The lexer's tokens of a text, and where its lines start, kept across edits.
///
/// `lexer::tokenize` starts every token afresh and reads only forward, so tokens from a token
/// boundary on depend only on the text from there and on whether a MySQL executable comment is
/// open there; each token is decided by the text up to its [`lexer::reach`]. An edit keeps the
/// tokens whose reach ends at or before its first changed byte (and before the end of the text),
/// fewer when they leave an executable comment open, and lexes again from there (a bounded slice
/// first) until a new token starts where an old one did in the unchanged rest of the text, the
/// token before it decided within the slice and the same comment open or not before both: from
/// there on the old tokens hold, moved by the change in length.
/// The result is what lexing the whole text gives (`incremental_tokens_match_a_full_lex`, and key
/// by key for dollar quotes in `typing_and_deleting_dollar_quotes_key_by_key_match_a_full_lex`).
struct Tokens {
    dialect: Dialect,
    text: String,
    list: Vec<Token>,
    line_starts: Vec<usize>,
    /// The texts tokenized, and the bytes lexed for them all.
    updates: usize,
    lexed: usize,
}

impl Default for Tokens {
    fn default() -> Self {
        Self::new(NO_CONNECTION_DIALECT)
    }
}

impl Tokens {
    fn new(dialect: Dialect) -> Self {
        Self { dialect, text: String::new(), list: Vec::new(), line_starts: Vec::new(), updates: 0, lexed: 0 }
    }

    /// The tokens of `text` in order, covering it.
    #[cfg(test)]
    fn list(&self) -> &[Token] {
        &self.list
    }

    /// How many texts were tokenized, and how many bytes were lexed for them.
    fn work(&self) -> (usize, usize) {
        (self.updates, self.lexed)
    }

    /// Tokenizes `new`, lexing again only what the change from the last text needs.
    fn update(&mut self, new: &str) {
        if self.updates > 0 && self.text == new {
            return;
        }
        let old = std::mem::take(&mut self.text);
        let prefix = floor_boundary(new, common_prefix(old.as_bytes(), new.as_bytes()));
        let most = old.len().min(new.len()).saturating_sub(prefix);
        let changed_end = ceil_boundary(new, new.len().saturating_sub(common_suffix(old.as_bytes(), new.as_bytes(), most)));
        // Reach never decreases along the list, so the tokens kept are a prefix of it. One that
        // reaches the end of the old text depends on where that text ends.
        let kept = |k: usize| {
            let reach = lexer::reach(self.dialect, &self.list, k, &old);
            reach <= prefix && reach < old.len()
        };
        let mut keep = if self.updates == 0 { 0 } else { self.list.partition_point(|t| t.range.end <= prefix) };
        while keep > 0 && !kept(keep - 1) {
            keep -= 1;
        }
        // Lexing starts with no executable comment open: before the start of one left open.
        let old_open = lexer::executable_open_states(&self.list, &old);
        while keep > 0 && old_open.get(keep).copied().unwrap_or(false) {
            keep -= 1;
        }
        let restart = keep.checked_sub(1).and_then(|k| self.list.get(k)).map_or(0, |t| t.range.end);
        let mut cut = floor_boundary(new, changed_end.saturating_add(RESYNC_BYTES)).max(changed_end);
        let tail = loop {
            let slice = new.get(restart..cut).unwrap_or_default();
            self.lexed = self.lexed.saturating_add(slice.len());
            let fresh = lex(self.dialect, slice, restart);
            if cut >= new.len() {
                break fresh;
            }
            let fresh_open = lexer::executable_open_states(&fresh, new);
            // A token starting after the change where an old one started too, the token before it
            // decided within the slice (its reach not past the cut) or kept.
            let fits = fresh.iter().enumerate().find_map(|(k, t)| {
                let at = t.range.start;
                let decided = k.checked_sub(1).is_none_or(|before| lexer::reach(self.dialect, &fresh, before, new) <= cut);
                if at < changed_end || !decided {
                    return None;
                }
                let then = at.saturating_add(old.len()).checked_sub(new.len())?;
                let j = self.list.binary_search_by_key(&then, |t| t.range.start).ok()?;
                (j >= keep && old_open.get(j) == fresh_open.get(k)).then_some((k, j))
            });
            if let Some((k, j)) = fits {
                let mut tail = fresh;
                tail.truncate(k);
                let moved = self.list.get(j..).unwrap_or_default().iter().map(|t| Token {
                    kind: t.kind,
                    range: (t.range.start.saturating_add(new.len()).saturating_sub(old.len()))
                        ..(t.range.end.saturating_add(new.len()).saturating_sub(old.len())),
                });
                tail.extend(moved);
                break tail;
            }
            cut = new.len();
        };
        self.list.truncate(keep);
        self.list.extend(tail);
        self.text = new.to_owned();
        self.line_starts = std::iter::once(0).chain(new.match_indices('\n').map(|(at, _)| at.saturating_add(1))).collect();
        self.updates = self.updates.saturating_add(1);
    }

    /// The layout job of the text: the tokens of `lines` (a range of 0-based lines) coloured, one
    /// section per run of the same colour (whitespace joins the run before it), the rest of the
    /// text plain; monospace and unwrapped. Sections that would not cover the text exactly (which
    /// the tokens never make) give one plain section rather than a job egui could not lay out.
    fn job(&self, lines: Range<usize>, syntax: &theme::Tokens, plain: Color32) -> LayoutJob {
        let text = self.text.as_str();
        let section = |start: usize, end: usize, color: Color32, italics: bool| LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(start)..ByteIndex(end),
            format: TextFormat { italics, ..Text::Code.format(color) },
        };
        let mut job = LayoutJob { text: text.to_owned(), keep_trailing_whitespace: true, ..LayoutJob::default() };
        let from = self.line_starts.get(lines.start).copied().unwrap_or(text.len());
        let to = self.line_starts.get(lines.end).copied().unwrap_or(text.len()).max(from);
        // The run being gathered: start, end, colour, italics.
        let mut open: Option<(usize, usize, Color32, bool)> = (from > 0).then_some((0, from, plain, false));
        let push = |job: &mut LayoutJob,
                    open: &mut Option<(usize, usize, Color32, bool)>,
                    start: usize,
                    end: usize,
                    look: Option<(Color32, bool)>| match (open.as_mut(), look) {
            (Some(run), None) => run.1 = end,
            (Some(run), Some((c, i))) if run.2 == c && run.3 == i => run.1 = end,
            (_, look) => {
                if let Some((s, e, c, i)) = open.take() {
                    job.sections.push(section(s, e, c, i));
                }
                let (c, i) = look.unwrap_or((plain, false));
                *open = Some((start, end, c, i));
            }
        };
        let first = self.list.partition_point(|t| t.range.end <= from);
        for t in self.list.get(first..).unwrap_or_default() {
            if t.range.start >= to {
                break;
            }
            let (start, end) = (t.range.start.max(from), t.range.end.min(to));
            push(&mut job, &mut open, start, end, style(t.kind, syntax, plain));
        }
        if to < text.len() {
            push(&mut job, &mut open, to, text.len(), Some((plain, false)));
        }
        if let Some((s, e, c, i)) = open {
            job.sections.push(section(s, e, c, i));
        }
        // Contiguous from the start to the end, on character boundaries.
        let mut end = 0;
        let covers = job.sections.iter().all(|s| {
            let (a, b) = (s.byte_range.start.0, s.byte_range.end.0);
            let ok = a == end && a <= b && text.is_char_boundary(a) && text.is_char_boundary(b);
            end = b;
            ok
        }) && end == text.len();
        if job.sections.is_empty() || !covers {
            job.sections = vec![section(0, text.len(), plain, false)];
        }
        job
    }
}

/// `text` as a layout job for the editor, every token coloured (see [`Tokens::job`]).
#[cfg(test)]
fn highlight(text: &str, syntax: &theme::Tokens, plain: Color32) -> LayoutJob {
    let mut tokens = Tokens::default();
    tokens.update(text);
    tokens.job(0..usize::MAX, syntax, plain)
}

/// The editor's layouter cache: the job made for the last text, theme and coloured lines.
#[derive(Default)]
struct Highlighter {
    tokens: Tokens,
    /// The lines to colour in the next job (see [`colour_window`]).
    window: Range<usize>,
    /// What the job is for (the mode's tokens, the text colour, the coloured lines), and the job,
    /// whose text is the text it is for.
    job: Option<((theme::Tokens, Color32, Range<usize>), LayoutJob)>,
    /// Some token of the job is coloured.
    coloured: bool,
}

impl Highlighter {
    /// Colours with `dialect` from now on. Another dialect starts over: the text is lexed again
    /// from scratch, and no token or job of the old one is kept.
    fn set_dialect(&mut self, dialect: Dialect) {
        if self.tokens.dialect != dialect {
            self.tokens = Tokens::new(dialect);
            self.job = None;
        }
    }

    /// The job for `text` in the mode whose tokens are `syntax`: the last one again when nothing
    /// changed.
    fn job(&mut self, text: &str, syntax: &theme::Tokens, plain: Color32) -> LayoutJob {
        let key = (*syntax, plain, self.window.clone());
        if let Some((made_for, job)) = &self.job
            && *made_for == key
            && job.text == text
        {
            return job.clone();
        }
        self.tokens.update(text);
        let job = self.tokens.job(self.window.clone(), syntax, plain);
        self.coloured = job.sections.iter().any(|s| s.format.color != plain);
        self.job = Some((key, job.clone()));
        job
    }
}

// ---- the editor --------------------------------------------------------------------------------

/// One editor's text and what it keeps between frames.
#[derive(Default)]
pub struct EditorState {
    /// The text being edited.
    pub text: String,
    highlight: Highlighter,
    /// Bumped on every change of the text (the cursor's byte offset below goes with it).
    version: u64,
    /// The cursor as a character index and as a byte offset, for text `version`.
    cursor: Option<(u64, usize, usize)>,
    /// In the next frame drawn: put the cursor at this byte offset, with the keyboard.
    jump: Option<usize>,
    /// In the next frame drawn: take the keyboard.
    focus: bool,
    /// The rows (lines) of the last layout, for the gutter's width.
    lines: usize,
    /// The lines in view in the last frame, around which the text is coloured.
    view: Range<usize>,
    /// The line and column of a byte offset (the error band's), for text `version`.
    place: Option<(u64, usize, (usize, usize))>,
}

/// Leaves out the text: query text never reaches the logs.
impl std::fmt::Debug for EditorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorState").field("bytes", &self.text.len()).field("lines", &self.lines).finish_non_exhaustive()
    }
}

impl EditorState {
    pub fn new(text: String) -> EditorState {
        EditorState { text, view: 0..60, ..EditorState::default() }
    }

    /// Replaces the text (one set elsewhere); the cursor stays where it was, within the text.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.version = self.version.wrapping_add(1);
    }

    /// From the next frame drawn: the cursor at byte offset `at` (clamped), with the keyboard.
    fn jump_to(&mut self, at: usize) {
        self.jump = Some(at);
    }

    /// The editor takes the keyboard in the next frame drawn.
    fn focus(&mut self) {
        self.focus = true;
    }

    /// How many texts the layouter tokenized since its dialect last changed.
    pub fn tokenized(&self) -> usize {
        self.highlight.tokens.work().0
    }

    /// The editor colours the text with `dialect` ([`Highlighter::set_dialect`]).
    fn set_dialect(&mut self, dialect: Dialect) {
        self.highlight.set_dialect(dialect);
    }

    /// Some token of the text is coloured.
    fn highlighted(&self) -> bool {
        self.highlight.coloured
    }

    /// The 1-based line and column of byte offset `at` ([`line_column`]), worked out again only
    /// when the text or `at` changed.
    fn line_column_of(&mut self, at: usize) -> (usize, usize) {
        match self.place {
            Some((version, of, place)) if version == self.version && of == at => place,
            _ => {
                let place = line_column(&self.text, at);
                self.place = Some((self.version, at, place));
                place
            }
        }
    }

    /// The byte offset of character `chars`, worked out again only when either changed.
    fn cursor_bytes(&mut self, chars: usize) -> usize {
        match self.cursor {
            Some((version, at, bytes)) if version == self.version && at == chars => bytes,
            _ => {
                let bytes = byte_offset(&self.text, chars);
                self.cursor = Some((self.version, chars, bytes));
                bytes
            }
        }
    }
}

/// What [`editor`] did in a frame.
pub struct EditorOutput {
    /// The text edit's response.
    pub response: Response,
    /// The text changed.
    pub changed: bool,
    /// The cursor, a byte offset into the text: the start before the editor ever had one.
    pub cursor: usize,
    /// The editor has the keyboard.
    pub focused: bool,
    pub lines: usize,
}

/// Turns this frame's Tab presses into [`INDENT`] and drops every other Tab key event (Shift+Tab,
/// Ctrl+Tab, releases), which the text edit would take as a tab character or an outdent. It keeps
/// the keyboard on all of them: it locks the focus.
fn take_tabs(ui: &egui::Ui) {
    ui.input_mut(|i| {
        i.events.retain_mut(|e| match e {
            Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if !modifiers.any() => {
                *e = Event::Text(INDENT.into());
                true
            }
            Event::Key { key: Key::Tab, .. } => false,
            _ => true,
        });
    });
}

/// The width of the gutter for `lines` lines.
fn gutter_width(ui: &egui::Ui, lines: usize) -> f32 {
    LineNumber::room(ui, lines) + 2.0 * GUTTER_PAD
}

/// Draws editor `id` over the rest of `ui`: the gutter with the line numbers, then the text in a
/// scroll area both ways. `error_line` (1-based) is the line the last run's error points at.
pub fn editor(ui: &mut egui::Ui, id: Id, ed: &mut EditorState, error_line: Option<usize>) -> EditorOutput {
    let t = theme::tokens(ui.ctx());
    let rect = ui.available_rect_before_wrap();
    if ui.memory(|m| m.has_focus(id)) {
        take_tabs(ui);
    }
    // The band's jump, or the tab just became the active one: the keyboard, and the cursor.
    let mut scroll_to = None;
    if ed.focus || ed.jump.is_some() {
        ui.memory_mut(|m| m.request_focus(id));
    }
    ed.focus = false;
    if let Some(at) = ed.jump.take() {
        let chars = char_index(&ed.text, at);
        let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(chars))));
        TextEdit::store_state(ui.ctx(), id, state);
        scroll_to = Some(chars);
    }
    let focused = ui.memory(|m| m.has_focus(id));
    let gutter_w = gutter_width(ui, ed.lines);
    let p = ui.painter().clone();
    p.rect(rect, CornerRadius::same(RADIUS_CONTROL), t.card, Stroke::new(1.0, if focused { t.ink } else { t.line_strong }), StrokeKind::Inside);
    let inner = rect.shrink(1.0);
    let gutter = Rect::from_min_max(inner.min, pos2((inner.left() + gutter_w).min(inner.right()), inner.bottom()));
    p.rect_filled(gutter, CornerRadius { nw: RADIUS_CONTROL, sw: RADIUS_CONTROL, ne: 0, se: 0 }, t.card);
    p.line_segment([gutter.right_top(), gutter.right_bottom()], Stroke::new(1.0, t.line));
    let text_rect = Rect::from_min_max(pos2(gutter.right() + 1.0, inner.top()), inner.max);
    let mut text_ui = ui.new_child(UiBuilder::new().id_salt(id.with("dbee-editor-area")).max_rect(text_rect).layout(Layout::top_down(Align::Min)));
    let (out, view, offset) = {
        let EditorState { text, highlight, view, .. } = &mut *ed;
        highlight.window = colour_window(view.clone());
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
            let job = highlight.job(buf.as_str(), theme::tokens(ui.ctx()), ui.visuals().text_color());
            ui.fonts_mut(|f| f.layout_job(job))
        };
        // Not animated: a jump to an error far down lands at once (and the colours follow it at once).
        let scroll = ScrollArea::both().id_salt(id.with("dbee-editor-scroll")).auto_shrink([false, false]).animated(false).show(&mut text_ui, |ui| {
            let size = ui.available_size();
            let out = TextEdit::multiline(text)
                .id(id)
                .code_editor()
                .font(Text::Code.font())
                .frame(Frame::NONE.inner_margin(TEXT_MARGIN))
                .hint_text("Write or paste SQL here.")
                .desired_width(f32::INFINITY)
                .min_size(size)
                .layouter(&mut layouter)
                .show(ui);
            if let Some(chars) = scroll_to {
                let at = out.galley.pos_from_cursor(CCursor::new(chars)).translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(at.expand2(vec2(2.0 * LINE_H, LINE_H)), Some(Align::Center));
            }
            out
        });
        (scroll.inner, scroll.inner_rect, scroll.state.offset.y)
    };
    // The lines in view, for the next frame's colours.
    let first = (offset / LINE_H).floor().max(0.0) as usize;
    ed.view = first..first.saturating_add((view.height() / LINE_H).ceil().max(0.0) as usize).saturating_add(1);
    let changed = out.response.changed();
    if changed {
        ed.version = ed.version.wrapping_add(1);
    }
    let galley = &out.galley;
    ed.lines = galley.rows.len().max(1);
    let chars = out.state.cursor.char_range().map_or(0, |r| r.primary.index.0);
    let cursor = ed.cursor_bytes(chars);
    let focused = ui.memory(|m| m.has_focus(id));
    // The line numbers of the rows in view: the cursor's and the error's marked.
    let cursor_row = out.cursor_range.filter(|_| focused).map(|r| galley.layout_from_cursor(r.primary).row);
    let gutter_p = ui.painter_at(Rect::from_min_max(pos2(gutter.left(), view.top()), pos2(gutter.right(), view.bottom())));
    let origin = out.galley_pos;
    let first = galley.rows.partition_point(|row| origin.y + row.pos.y + row.row.size.y < view.top());
    for (i, row) in galley.rows.iter().enumerate().skip(first) {
        let top = origin.y + row.pos.y;
        if top > view.bottom() {
            break;
        }
        let n = i.saturating_add(1);
        let slot = Rect::from_min_max(pos2(gutter.left(), top), pos2(gutter.right() - GUTTER_PAD, top + row.row.size.y));
        LineNumber { n, current: cursor_row == Some(i), error: error_line == Some(n) }.paint(&gutter_p, slot);
    }
    EditorOutput { response: out.response.response, changed, cursor, focused, lines: ed.lines }
}

// ---- the tab -----------------------------------------------------------------------------------

/// The result grids of a tab's last run: one state per result (column widths, the drag).
#[derive(Debug, Default)]
struct ResultGrids {
    /// The results the grids are for, by identity: a weak reference keeps an address from being
    /// reused by a later run's result.
    of: Vec<Weak<ResultSet>>,
    /// Unique per run seen, in the grids' ids, so egui keeps no widths or scrolling from another
    /// run's grid.
    run: u64,
    grids: Vec<GridState>,
}

/// Runs seen, for [`ResultGrids::run`].
static RUNS: AtomicU64 = AtomicU64::new(0);

impl ResultGrids {
    /// Fresh grids when `results` are another run's.
    fn follow(&mut self, results: &[StatementOutcome]) {
        let same = self.of.len() == results.len() && self.of.iter().zip(results).all(|(w, o)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(&o.result)));
        if !same {
            self.of = results.iter().map(|o| Arc::downgrade(&o.result)).collect();
            self.run = RUNS.fetch_add(1, Ordering::Relaxed);
            self.grids = results.iter().map(|_| GridState::default()).collect();
        }
    }
}

/// What the window keeps per query tab, dropped with the tab.
#[derive(Debug, Default)]
pub(crate) struct QueryState {
    pub editor: EditorState,
    results: ResultGrids,
}

/// The query tabs' states, by tab.
pub(crate) type Queries = HashMap<TabId, QueryState>;

/// `button` in the toolbar, tagged `id` with `label`, its hover `hover` or why it is off; true
/// when clicked.
fn tool_button(ui: &mut egui::Ui, tags: &mut Tags, id: &str, label: &str, button: impl Widget, (hover, why_not): (&str, Option<&str>)) -> bool {
    let enabled = why_not.is_none();
    let r = ui.add_enabled(enabled, button);
    let r = match why_not {
        Some(why) => r.hint_disabled(format!("{hover}: not now, {why}")),
        None => r.hint(hover),
    };
    tags.tag_value_as(id, &r, label, enabled);
    r.clicked()
}

/// What the database chip shows: the database the tab's next run starts in, as the engine knows
/// it, the ones to choose from, the engine's word for one and for many, and why it cannot be used
/// now.
struct Chip<'a> {
    database: CurrentDatabase,
    choices: Vec<&'a str>,
    word: (&'static str, &'static str),
    why_not: Option<String>,
}

/// The database chip, as tall as the toolbar's buttons and of a fixed width, so nothing beside
/// it moves when the database changes (a long name is cut, and reads whole on hover). A click
/// opens the list of `chip.choices` under its heading; returns the one chosen.
fn database_chip(ui: &mut egui::Ui, tags: &mut Tags, chip: &Chip<'_>) -> Option<String> {
    let t = theme::tokens(ui.ctx());
    let (one, many) = chip.word;
    let enabled = chip.why_not.is_none();
    let mut chosen = None;
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, r) = ui.allocate_exact_size(vec2(CHIP_W, size::BUTTON_H), Sense::click());
        let p = ui.painter();
        let fill = if enabled && r.hovered() { t.hover } else { t.card };
        p.rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        let muted = if enabled { t.text_muted } else { t.text_faint };
        let icon = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(16.0, 16.0));
        Icon::Schema.paint(p, icon.center(), muted);
        let arrow = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(14.0, 14.0));
        Icon::ChevronDown.paint(p, arrow.center(), muted);
        let none = format!("No {one} chosen");
        let unknown = format!("{} not known", capitalized(one));
        let shown = chip.database.database.as_deref().filter(|_| chip.database.known);
        let (text, ink) = match shown {
            Some(name) if enabled => (name, t.text),
            Some(name) => (name, t.text_muted),
            None if chip.database.known => (none.as_str(), t.text_muted),
            None => (unknown.as_str(), t.text_muted),
        };
        let left = icon.right() + 6.0;
        let g = line_galley(ui, text, Text::Body, ink, (arrow.left() - 4.0 - left).max(1.0));
        ui.painter().galley(pos2(left, rect.center().y - g.size().y / 2.0), g, ink);
        let hover = match shown {
            Some(name) => format!("The next run starts in the {one} {name}. Click to choose another."),
            None if chip.database.known => format!("No {one} is chosen for this tab. Click to choose one."),
            None => format!(
                "Which {one} the next run starts in is not known: the server does not say where a statement leaves the connection. Click to choose one."
            ),
        };
        let r = match &chip.why_not {
            Some(why) => r.hint_disabled(format!("Choose the {one}: not now, {why}")),
            None => r.hint(hover),
        };
        tags.tag_value_as(DATABASE, &r, shown, enabled);
        if !enabled {
            // egui closes a popup that is not drawn: no list while the chip waits.
            return;
        }
        Popup::menu(&r).width(CHIP_W).style(menu::style(ui.ctx())).show(|ui| {
            // At least as wide as the chip.
            ui.set_min_width(ui.available_width());
            menu::heading(ui, many);
            ScrollArea::vertical().max_height(CHIP_LIST_H).show(ui, |ui| {
                for name in &chip.choices {
                    let r = ui.add(Item::new(name).choice(Some(shown == Some(*name))));
                    // A menu's first frame is a sizing pass, which reads every widget as disabled.
                    tags.tag_value_as(&format!("{DATABASE}-{name}"), &r, *name, true);
                    if r.clicked() {
                        chosen = Some((*name).to_owned());
                        ui.close();
                    }
                }
            });
        });
    });
    chosen
}

/// `word` with its first letter upper-cased.
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The band's note under an error that means the transaction failed earlier.
pub const ROLLBACK_NOTE: &str = "The transaction this connection is in failed earlier: run ROLLBACK to end it. Until then every statement fails.";

/// The error band's heading for `e` of a run that kept `kept` results.
fn heading(e: &QueryError, kept: usize) -> String {
    if e.code == "cancelled" {
        "The run was cancelled".into()
    } else if kept > 0 || e.statement > 0 {
        format!("Statement {} failed", e.statement.saturating_add(1))
    } else {
        "Query failed".into()
    }
}

/// The error band of `e`, a run that kept `kept` results; `at` is the line and column of its
/// position in the editor's text. True when a click on it asks to go to the error's position.
pub(crate) fn band(ui: &mut egui::Ui, tags: &mut Tags, tab: TabId, e: &QueryError, kept: usize, at: Option<(usize, usize)>) -> bool {
    let title = heading(e, kept);
    let note = (e.meaning == Some(Meaning::TransactionAborted)).then_some(ROLLBACK_NOTE);
    let band = Band {
        heading: &title,
        position: at,
        message: &e.message,
        detail: e.detail.as_deref(),
        hint: e.hint.as_deref(),
        note,
        cancelled: e.code == "cancelled",
    };
    let r = band.show(ui, Id::new(("dbee-query-error", tab.0)));
    let jump = at.is_some();
    let (line, column) = at.unzip();
    let value = json!({
        "code": e.code,
        "heading": title,
        "message": e.message,
        "detail": e.detail,
        "hint": e.hint,
        "note": note,
        "position": e.position,
        "statement": e.statement,
        "line": line,
        "column": column,
        "jump": jump,
    });
    tags.tag_value(ERROR, &r, value);
    jump && r.clicked()
}

/// The room a result's button takes in the selector, with the dot before it, when there are
/// `results` of them: its number's digits (about 7.5 points each at the selector's size, plus the
/// button's padding, at least 22 points), and 12 for the dot and the spacing.
fn result_width(results: usize) -> f32 {
    let digits = results.max(1).ilog10().saturating_add(1) as f32;
    (7.5 * digits + 20.0).max(22.0) + 12.0
}

/// The selector's `…` button with its spacing, and its list's width.
const MORE_W: f32 = 26.0;
const RESULT_LIST_W: f32 = 220.0;

/// What the selector draws: its label, the results from `from` to `to`, and the `…` that lists
/// them all when some are left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectorFit {
    label: bool,
    from: usize,
    to: usize,
    more: bool,
}

/// The selector in `room` points, its label `label_w` wide: all results and the label when they
/// fit; else as many as fit around the shown one beside the `…`, and the label only if it and one
/// result fit too, so it is never cut.
fn selector_fit(results: usize, shown: usize, room: f32, label_w: f32) -> SelectorFit {
    let per = result_width(results);
    let room = if room.is_finite() { room } else { 0.0 };
    if results as f32 * per + label_w <= room {
        return SelectorFit { label: true, from: 0, to: results, more: false };
    }
    let label = room >= label_w + MORE_W + per;
    let left = room - MORE_W - if label { label_w } else { 0.0 };
    let fit = if left > 0.0 { ((left / per).floor() as usize).min(results) } else { 0 };
    let from = shown.min(results.saturating_sub(1)).saturating_sub(fit / 2).min(results.saturating_sub(fit));
    SelectorFit { label, from, to: from.saturating_add(fit).min(results), more: true }
}

/// The results selector, `room` points wide: one button per result, `n` from 1 (with more than
/// fit, those around the shown one, and a `…` listing every result); the index of the one chosen.
fn selector(ui: &mut egui::Ui, tags: &mut Tags, results: &[StatementOutcome], shown: usize, room: f32) -> Option<usize> {
    let t = theme::tokens(ui.ctx());
    let mut picked = None;
    let muted = |text: &str| Text::Label.rich(text).color(t.text_muted);
    let label = |ui: &mut egui::Ui, text: &str| ui.add(Label::new(muted(text)).selectable(false));
    let label_w = ui.painter().layout_job(Text::Label.job("Results", t.text_muted)).size().x + ui.spacing().item_spacing.x;
    let fit = selector_fit(results.len(), shown, room, label_w);
    if fit.label {
        label(ui, "Results");
    }
    let what = |o: &StatementOutcome| if o.result.columns.is_empty() { affected_text(o.result.rows_affected) } else { rows_text(&o.result) };
    let value = |i: usize, o: &StatementOutcome| json!({ "shown": i == shown, "rows": o.result.rows.len(), "rowsAffected": o.result.rows_affected });
    for (i, o) in results.iter().enumerate().take(fit.to).skip(fit.from) {
        if i > fit.from {
            label(ui, "·");
        }
        let n = i.saturating_add(1);
        let r = ui.add(Button::selectable(i == shown, Text::Data.rich(n.to_string())).min_size(vec2(22.0, 22.0)));
        let r = r.hint(format!("Statement {n}: {}", what(o)));
        tags.tag_value(&format!("{RESULT}{n}"), &r, value(i, o));
        if r.clicked() && i != shown {
            picked = Some(i);
        }
    }
    if fit.more {
        let r = ui.add(Button::new(Text::Data.rich("…")).min_size(vec2(22.0, 22.0)));
        let r = r.hint(format!("Choose one of the {} results", results.len()));
        tags.tag_value(RESULT_MORE, &r, results.len());
        Popup::menu(&r).width(RESULT_LIST_W).style(menu::style(ui.ctx())).show(|ui| {
            ui.set_min_width(ui.available_width());
            ScrollArea::vertical().max_height(CHIP_LIST_H).show_rows(ui, size::MENU_ITEM_H, results.len(), |ui, rows| {
                for (i, o) in results.iter().enumerate().take(rows.end).skip(rows.start) {
                    let n = i.saturating_add(1);
                    let r = ui.add(Item::new(&format!("Statement {n}: {}", what(o))).choice(Some(i == shown)));
                    // A menu's first frame is a sizing pass, which reads every widget as disabled.
                    tags.tag_value_as(&format!("{RESULT_LIST}{n}"), &r, value(i, o), true);
                    if r.clicked() {
                        ui.close();
                        if i != shown {
                            picked = Some(i);
                        }
                    }
                }
            });
        });
    }
    picked
}

/// The editor's height in a tab body `h` high, for split `split`.
fn editor_height(h: f32, split: f32) -> f32 {
    let most = (h - TOP_PAD - SPLIT_H - TOOLBAR_H - MIN_RESULTS_H).max(MIN_EDITOR_H);
    let wanted = if split.is_finite() { split * h } else { DEFAULT_SPLIT * h };
    wanted.max(MIN_EDITOR_H).min(most)
}

/// Draws query tab `id` into `ui` and runs what it asks for.
pub(crate) fn show(app: &mut DbeeApp, ui: &mut egui::Ui, id: TabId) {
    let t = theme::tokens(ui.ctx());
    let mut runs: Vec<(&'static str, Json)> = Vec::new();
    let mut set_text: Option<String> = None;
    {
        let DbeeApp { session, view, tags, .. } = app;
        let Some(Tab::Query(q)) = session.state().tab(id) else { return };
        // The engine's own rule for the Run buttons; what the run refuses it says when clicked.
        let run_blocked = can_run(session).err();
        let modal = view.dialog.is_some() || view.quick.open;
        let focus_now = view.work.focused_query != Some(id) && !modal;
        if focus_now {
            view.work.focused_query = Some(id);
        }
        let state = view.work.queries.entry(id).or_insert_with(|| QueryState { editor: EditorState::new(q.text.clone()), ..QueryState::default() });
        // Text set elsewhere (an agent's `query.setText`) replaces the buffer.
        if state.editor.text != q.text {
            state.editor.set_text(q.text.clone());
        }
        if focus_now {
            state.editor.focus();
        }
        state.editor.set_dialect(session.state().dialect(Which::Tab(id)));
        let body = ui.available_rect_before_wrap();
        let full = body.shrink2(vec2(size::TAB_BODY_PAD_X, 0.0));
        let h = full.height();
        // The split first, so a drag moves the editor in this frame.
        let mut editor_h = editor_height(h, view.query_split);
        let split_at = |editor_h: f32| Rect::from_min_size(pos2(full.left(), full.top() + TOP_PAD + editor_h), vec2(full.width(), SPLIT_H));
        let split = ui.interact(split_at(editor_h), Id::new("dbee-query-split"), Sense::drag());
        if split.dragged() && h > 0.0 {
            editor_h = editor_height(h, (editor_h + split.drag_delta().y) / h);
            view.query_split = editor_h / h;
        }
        if split.hovered() || split.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        let split_rect = split_at(editor_h);
        let grip = if split.hovered() || split.dragged() { t.text_muted } else { t.line };
        let c = split_rect.center();
        ui.painter().line_segment([pos2(c.x - 18.0, c.y), pos2(c.x + 18.0, c.y)], Stroke::new(2.0, grip));
        tags.tag_value(SPLIT, &split, f64::from(view.query_split));
        // The editor.
        let editor_rect = Rect::from_min_size(pos2(full.left(), full.top() + TOP_PAD), vec2(full.width(), editor_h));
        let mut editor_ui =
            ui.new_child(UiBuilder::new().id_salt(("dbee-query-editor", id.0)).max_rect(editor_rect).layout(Layout::top_down(Align::Min)));
        let error_line = q.error.as_ref().and_then(|e| e.position).map(|p| state.editor.line_column_of(p).0);
        let out = editor(&mut editor_ui, editor_id(id), &mut state.editor, error_line);
        let value = json!({
            "cursor": out.cursor,
            "focused": out.focused,
            "lines": out.lines,
            "length": state.editor.text.len(),
            "highlighted": state.editor.highlighted(),
        });
        tags.tag_value(EDITOR, &out.response, value);
        view.query_cursor = Some(out.cursor);
        if out.changed {
            set_text = Some(state.editor.text.clone());
        }
        // The toolbar.
        let bar_rect = Rect::from_min_size(pos2(full.left(), split_rect.bottom()), vec2(full.width(), TOOLBAR_H));
        let mut bar =
            ui.new_child(UiBuilder::new().id_salt(("dbee-query-bar", id.0)).max_rect(bar_rect).layout(Layout::left_to_right(Align::Center)));
        bar.spacing_mut().item_spacing.x = 6.0;
        let running = q.running.is_some();
        if let Some(c) = session.state().connection(Which::Tab(id)).filter(|c| c.dialect.has_use_database()) {
            let chip = Chip {
                database: c.starts_in(q),
                choices: c.catalog.schemas.iter().filter(|s| !s.system).map(|s| s.name.as_str()).collect(),
                word: c.dialect.schema_word(),
                why_not: can_set_database(session)
                    .err()
                    .or_else(|| running.then(|| "the tab's query is running".to_owned()))
                    .or_else(|| run_blocked.clone()),
            };
            if let Some(name) = database_chip(&mut bar, tags, &chip) {
                runs.push(("query.setDatabase", json!({ "tab": id, "database": name })));
            }
        }
        let why_not = run_blocked.as_deref();
        if tool_button(&mut bar, tags, RUN, "Run", BoxButton::primary("Run"), ("Run every statement (Ctrl+Enter)", why_not)) {
            runs.push((QUERY_RUN, json!({})));
        }
        let current = ("Run the statement under the cursor (Ctrl+Shift+Enter)", why_not);
        if tool_button(&mut bar, tags, RUN_CURRENT, "Run current", BoxButton::outline("Run current"), current) {
            runs.push((QUERY_RUN, json!({ "current": true, "cursor": out.cursor })));
        }
        let stop = ("Stop the running statement", (!running).then_some("nothing is running"));
        if tool_button(&mut bar, tags, CANCEL, "Cancel", TextButton::new("Cancel"), stop) {
            runs.push(("query.cancel", json!({})));
        }
        let shown = q.shown_result();
        let summary = shown.filter(|o| !o.result.columns.is_empty()).map(|o| (rows_text(&o.result), !o.result.complete));
        // The summary's own width at the right, measured as it is drawn.
        let right_room = summary.as_ref().map_or(0.0, |(text, _)| bar.painter().layout_job(Text::Label.job(text, t.text)).size().x + 16.0);
        if running {
            let spinner = bar.add(Spinner::new());
            let words = Text::Label.rich("Running…").color(t.text_muted);
            let words_w = bar.painter().layout_job(Text::Label.job("Running…", t.text_muted)).size().x;
            let after = bar.cursor().min.x + bar.spacing().item_spacing.x + words_w + 10.0;
            if q.results.len() < 2 || bar_rect.right() - right_room - after >= MORE_W + result_width(q.results.len()) {
                bar.label(words);
            } else {
                spinner.hint("Running…");
            }
        }
        // The selector between the buttons and the summary, kept within its room.
        let left = bar.cursor().min.x + 10.0;
        let right = (bar_rect.right() - right_room).max(left);
        if q.results.len() > 1 && right > left {
            let room = Rect::from_min_max(pos2(left, bar_rect.top()), pos2(right, bar_rect.bottom()));
            let mut sel =
                bar.new_child(UiBuilder::new().id_salt(("dbee-query-selector", id.0)).max_rect(room).layout(Layout::left_to_right(Align::Center)));
            sel.set_clip_rect(room.intersect(bar.clip_rect()));
            sel.spacing_mut().item_spacing.x = 4.0;
            if let Some(index) = selector(&mut sel, tags, &q.results, q.shown, room.width()) {
                runs.push(("query.showResult", json!({ "index": index })));
            }
        }
        if let Some((text, cut)) = &summary {
            let at = Rect::from_min_max(pos2(right, bar_rect.top()), bar_rect.max);
            let mut sum =
                bar.new_child(UiBuilder::new().id_salt(("dbee-query-summary", id.0)).max_rect(at).layout(Layout::right_to_left(Align::Center)));
            {
                let ui = &mut sum;
                let rich = if *cut { Text::Label.rich(text.as_str()).color(t.text) } else { Text::Label.rich(text.as_str()).color(t.text_muted) };
                let r = ui.add(Label::new(rich).selectable(false));
                let r = if *cut { r.hint(cap_hover()) } else { r };
                tags.tag_value(ROWS, &r, text.as_str());
            }
        }
        ui.painter().line_segment([bar_rect.left_bottom(), bar_rect.right_bottom()], Stroke::new(1.0, t.line));
        // The band, then the results.
        let rest = Rect::from_min_max(pos2(full.left(), bar_rect.bottom() + 6.0), full.max);
        let mut rest_ui = ui.new_child(UiBuilder::new().id_salt(("dbee-query-results", id.0)).max_rect(rest).layout(Layout::top_down(Align::Min)));
        if let Some(e) = &q.error {
            let at = e.position.map(|p| state.editor.line_column_of(p));
            if band(&mut rest_ui, tags, id, e, q.results.len(), at)
                && let Some(at) = e.position
            {
                state.editor.jump_to(at);
                ui.ctx().request_repaint();
            }
            rest_ui.add_space(6.0);
        }
        state.results.follow(&q.results);
        match shown {
            Some(o) if !o.result.columns.is_empty() => {
                let data: &ResultSet = &o.result;
                let grid = Grid {
                    id: Id::new(("dbee-result-grid", state.results.run, q.shown)),
                    source: data,
                    keys: &[],
                    selection: q.selection,
                    sort: None,
                    sortable: false,
                    blocked: None,
                    edits: RowEdits::None,
                    links: false,
                    first_number: 1,
                    editing: None,
                };
                let below = rest_ui.available_rect_before_wrap().top().min(rest.bottom());
                let area = Rect::from_min_max(pos2(body.left(), below), pos2(body.right(), rest.bottom()));
                let mut grid_ui = rest_ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
                let mut spare = GridState::default();
                let gs = state.results.grids.get_mut(q.shown).unwrap_or(&mut spare);
                for act in grid.show(&mut grid_ui, gs, tags) {
                    match act {
                        GridAct::Select { anchor, focus } => {
                            runs.push(("selection.set", json!({ "anchor": [anchor.0, anchor.1], "focus": [focus.0, focus.1] })));
                        }
                        GridAct::Copy(format) => runs.push(("clipboard.copy", json!({ "format": format }))),
                        // Read-only: no sorting, editing or links.
                        GridAct::Sort(_) | GridAct::Follow { .. } | GridAct::SetNull { .. } | GridAct::DeleteRows(_) | GridAct::Edit { .. } => {}
                    }
                }
            }
            Some(o) => {
                let text = affected_text(o.result.rows_affected);
                rest_ui.add_space(4.0);
                let r = rest_ui.add(Label::new(Text::Body.rich(text.as_str()).color(t.text)).selectable(false));
                tags.tag_value(AFFECTED, &r, text.as_str());
            }
            // The toolbar says it runs.
            None if running => {}
            None if q.error.is_none() => {
                rest_ui.add_space(rest.height() * 0.25);
                rest_ui.vertical_centered(|ui| {
                    let text = "Ctrl+Enter runs every statement in the editor; Ctrl+Shift+Enter runs only the one the cursor is in.";
                    let r = ui.add(Label::new(RichText::new(text).color(t.text_muted)).wrap());
                    tags.tag_value(HINT, &r, text);
                });
            }
            None => {}
        }
    }
    if let Some(text) = set_text
        && let Err(e) = app.run("query.setText", json!({ "text": text }))
    {
        app.notice(Kind::Error, crate::app::notice_text(&e));
    }
    for (command, params) in runs {
        app.start(command, params, Origin::Menu);
    }
}

/// A query run's error that the tab's band shows (`QueryTab::error`), so no notice repeats it:
/// the engine adds the failing statement to the data of each error it records on the tab.
pub(crate) fn in_card(command: &str, result: &Result<Json, dbee_engine::EngineError>) -> bool {
    command == QUERY_RUN && matches!(result, Err(dbee_engine::EngineError::Db { data, .. }) if data.get("statement").is_some())
}

#[cfg(test)]
mod tests {
    use dbee_core::{Column, ResultSet};
    use egui::Color32;

    use super::*;

    #[test]
    fn the_cap_notice_counts_rows_kept_and_seen() {
        assert_eq!(cut_notice(10_000, 25_000), "First 10,000 of 25,000 rows");
        assert_eq!(cut_notice(0, 1), "First 0 of 1 row");
        let rs = |rows: usize, seen: u64, complete: bool| ResultSet {
            columns: vec![Column::untyped("n")],
            rows: vec![vec![Some("1".to_string())]; rows],
            rows_seen: seen,
            complete,
            rows_affected: Some(seen),
            database_after: Default::default(),
        };
        assert_eq!(rows_text(&rs(3, 3, true)), "3 rows");
        assert_eq!(rows_text(&rs(1, 1, true)), "1 row");
        assert_eq!(rows_text(&rs(0, 0, true)), "0 rows");
        assert_eq!(rows_text(&rs(2, 12_345, false)), "First 2 of 12,345 rows");
    }

    #[test]
    fn rows_affected_reads_as_a_count_or_a_line_without_one() {
        assert_eq!(affected_text(Some(1)), "1 row affected");
        assert_eq!(affected_text(Some(0)), "0 rows affected");
        assert_eq!(affected_text(Some(12_000)), "12,000 rows affected");
        assert_eq!(affected_text(None), "The statement ran; the database gave no row count.");
    }

    #[test]
    fn byte_offsets_and_char_indices_convert_both_ways() {
        let text = "select 'é🐙', nope";
        let nope = text.find("nope").unwrap();
        assert_eq!(char_index(text, nope), 13);
        assert_eq!(byte_offset(text, 13), nope);
        // Past the end, or inside a character: clamped, never a panic.
        assert_eq!(char_index(text, 999), text.chars().count());
        assert_eq!(byte_offset(text, 999), text.len());
        assert_eq!(char_index(text, 9), char_index(text, 8), "inside `é` counts as its start");
        assert_eq!((char_index("", 3), byte_offset("", 3)), (0, 0));
    }

    #[test]
    fn the_cards_line_and_column_follow_the_text_and_the_position() {
        let mut ed = EditorState::new("select 1;\nselect nope".into());
        assert_eq!(ed.line_column_of(17), (2, 8));
        assert_eq!(ed.line_column_of(17), (2, 8), "the same again, from the cache");
        assert_eq!(ed.line_column_of(3), (1, 4), "another position");
        ed.set_text("-- note\nselect 1;\nselect nope".into());
        assert_eq!(ed.line_column_of(3), (1, 4));
        assert_eq!(ed.line_column_of(25), (3, 8), "another text");
    }

    #[test]
    fn the_gutter_draws_the_pages_line_numbers() {
        // `components::editor::LineNumber` in today's gutter: the cursor's line (1) in `text`, the
        // error's line (2) in `danger`, the rest `text_muted`, in `code`, 8 in from the gutter's
        // line; the gutter as wide as two `code` digits and 8 each side.
        for (theme, t) in crate::components::both() {
            let mut ed = EditorState::new("select 1;\nselect nope;\nselect 3".into());
            ed.jump_to(3);
            let mut h = crate::components::harness_in(egui::vec2(500.0, 200.0), theme, (ed, Rect::NOTHING), |ui, (ed, at)| {
                *at = ui.available_rect_before_wrap();
                editor(ui, Id::new("gutter-test"), ed, Some(2));
            });
            h.run_steps(3);
            let shapes = h.output().shapes.clone();
            let line = crate::components::flat(&shapes).into_iter().find_map(|s| match s {
                egui::Shape::LineSegment { points, stroke } if points[0].x == points[1].x && stroke.color == t.line => Some(points[0].x),
                _ => None,
            });
            let line = line.unwrap_or_else(|| panic!("{theme:?}: the gutter's line"));
            // The numbers, left of the gutter's line (the text has a `1` of its own).
            let seen: Vec<_> = crate::components::texts(&shapes).into_iter().filter(|(_, r, _)| r.right() <= line).collect();
            let mut rights = Vec::new();
            for (words, colour) in [("1", t.text), ("2", t.danger), ("3", t.text_muted)] {
                let (_, r, c) = seen.iter().find(|(w, _, _)| w == words).unwrap_or_else(|| panic!("{theme:?}: line {words}: {seen:?}"));
                assert_eq!(*c, colour, "{theme:?}: line {words}");
                assert!((r.height() - Text::Code.line()).abs() < 0.5, "{theme:?}: line {words} in `code`'s 20: {r:?}");
                rights.push(r.right());
            }
            let fonts: Vec<_> = crate::components::flat(&shapes)
                .into_iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(g) if g.pos.x < line && ["1", "2", "3"].contains(&g.galley.text()) => {
                        g.galley.job.sections.first().map(|s| s.format.font_id.clone())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(fonts, vec![Text::Code.font(); 3], "{theme:?}: the numbers in `code`");
            assert!(rights.iter().all(|r| (r - (line - GUTTER_PAD)).abs() < 0.5), "{theme:?}: flush right, 8 from the line at {line}: {rights:?}");
            let digit = h.ctx.fonts_mut(|f| f.glyph_width(&Text::Code.font(), '0'));
            let width = line - (h.state().1.left() + 1.0);
            assert!((width - (2.0 * digit + 2.0 * GUTTER_PAD)).abs() < 0.01, "{theme:?}: the gutter's width {width}, a digit {digit}");
        }
    }

    #[test]
    fn positions_read_as_line_and_column() {
        let text = "select 1;\nselect 'é', nope";
        assert_eq!(line_column(text, 0), (1, 1));
        assert_eq!(line_column(text, text.find("nope").unwrap()), (2, 13));
        assert_eq!(line_column(text, text.len() + 5), (2, 17), "past the end: the end");
        assert_eq!(line_column("", 0), (1, 1));
    }

    /// The colour of the section holding byte `at`.
    fn colour_at(job: &egui::text::LayoutJob, at: usize) -> (Color32, bool) {
        let s = job.sections.iter().find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0).unwrap();
        (s.format.color, s.format.italics)
    }

    /// A small deterministic random sequence (xorshift), so a failing edit can be replayed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self, below: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % below.max(1) as u64) as usize
        }
    }

    /// `tokens` after updating to `text` equals a full lex of `text`.
    fn same_as_full(tokens: &mut Tokens, text: &str, what: &str) {
        tokens.update(text);
        assert_eq!(tokens.list(), lexer::tokenize(tokens.dialect, text).as_slice(), "{what}: {text:?}");
    }

    #[test]
    fn typing_and_deleting_dollar_quotes_key_by_key_match_a_full_lex() {
        // A lone `$` reads its whole tag word, and the byte after it, before it settles.
        for text in [
            "select $body$ select 1; $body$ as x",
            "create function f() returns int language sql as $function$ select 1 $function$;",
            "select $tagé$ é $tagé$, $1",
            "do $$ begin perform 1; end $$; select $x1$a$x1$",
        ] {
            for dialect in Dialect::ALL {
                let mut tokens = Tokens::new(dialect);
                let ends: Vec<usize> = text.char_indices().map(|(at, _)| at).skip(1).chain([text.len()]).collect();
                for &end in &ends {
                    same_as_full(&mut tokens, &text[..end], "typed");
                }
                for &end in ends.iter().rev() {
                    same_as_full(&mut tokens, &text[..end], "deleted from the end");
                }
            }
        }
        // A tag typed in the middle, its closing `$` last; then that `$` taken away again.
        for dialect in Dialect::ALL {
            let mut tokens = Tokens::new(dialect);
            for text in ["as $ select 1", "as $b select 1", "as $bo select 1", "as $bod select 1", "as $body select 1", "as $body$ select 1"] {
                same_as_full(&mut tokens, text, "typed in the middle");
            }
            let mut tokens = Tokens::new(dialect);
            for text in ["as $body", "as $body$ select 1; $body$", "as $body select 1; $body$", "$tagé", "$tagé$$"] {
                same_as_full(&mut tokens, text, "pasted and cut");
            }
        }
    }

    #[test]
    fn incremental_tokens_match_a_full_lex() {
        // Pieces that end tokens late: exponents, `--`, `/*` and `*/` after an operator, `U&` and
        // `E'`, MySQL's executable comments and their versions, a lone `$` before a tag word of any
        // length (its tag read to the end), dollar quotes, quotes that open strings running on, and
        // characters of several bytes (in tags too).
        let pieces = [
            "select ",
            "1e",
            "+5",
            "1.5e-3 ",
            "e",
            "E",
            "5",
            ".",
            "'",
            "''",
            "' ",
            "\"",
            "\"Odd\" ",
            "--",
            " note\n",
            "/*",
            "*/",
            "-",
            "/",
            "*",
            "U",
            "u",
            "&",
            "U&'x'",
            "E'\\'",
            "$",
            "$$",
            "$1",
            "$tag$",
            "$body$",
            "$function$",
            "$tagé",
            "tagé",
            "body",
            "function",
            "x1",
            "a",
            ";",
            "\n",
            "\r\n",
            " ",
            "é",
            "🐙",
            "from t",
            "<>",
            "::",
            "#",
            "`",
            "[",
            "]",
            "x'",
            "B'",
            "0x",
            "0xA",
            "-- ",
            "--\t",
            "+--",
            "\\'",
            "DELIMITER ",
            "create trigger ",
            "begin ",
            "end ",
            "!",
            "M!",
            "/*!",
            "/*M!1",
            "50003",
        ];
        for dialect in Dialect::ALL {
            for seed in [0x9E37_79B9_7F4A_7C15, 12_345, 777, 42, 0xDEAD_BEEF] {
                let mut rng = Rng(seed);
                for round in 0..300 {
                    let mut text: String = (0..rng.next(40)).map(|_| pieces[rng.next(pieces.len())]).collect();
                    let mut tokens = Tokens::new(dialect);
                    tokens.update(&text);
                    for step in 0..60 {
                        // Two character boundaries, a few characters apart at most.
                        let bounds: Vec<usize> = text.char_indices().map(|(at, _)| at).chain([text.len()]).collect();
                        let i = rng.next(bounds.len());
                        let j = (i + rng.next(8)).min(bounds.len() - 1);
                        let (a, b) = (bounds[i], bounds[j]);
                        match rng.next(3) {
                            0 => text.insert_str(a, pieces[rng.next(pieces.len())]),
                            1 => text.replace_range(a..b, ""),
                            _ => text.replace_range(a..b, pieces[rng.next(pieces.len())]),
                        }
                        same_as_full(&mut tokens, &text, &format!("{dialect:?}, seed {seed}, round {round}, step {step}"));
                    }
                }
            }
        }
    }

    #[test]
    fn a_dialect_change_lexes_the_text_again_from_scratch() {
        // Each engine reads these differently: backticks, `#`, `$$` and a backslash in a string.
        let text = "select `a`, \"b\" # note\n, $$x$$ -- c\nselect 'it\\'s'";
        assert_ne!(lexer::tokenize(Dialect::Postgres, text), lexer::tokenize(Dialect::MySql, text), "the text tells them apart");
        let mut ed = EditorState::new(text.into());
        ed.highlight.window = colour_window(0..60);
        let plain = Color32::BLACK;
        let note = text.find("# note").unwrap();
        let job = ed.highlight.job(text, &crate::theme::LIGHT, plain);
        assert_eq!(ed.highlight.tokens.list(), lexer::tokenize(Dialect::Postgres, text).as_slice());
        assert_eq!(colour_at(&job, note), (plain, false), "`#` is an operator on Postgres");
        for dialect in [Dialect::MySql, Dialect::Sqlite, Dialect::Postgres] {
            ed.set_dialect(dialect);
            let job = ed.highlight.job(text, &crate::theme::LIGHT, plain);
            assert_eq!(ed.highlight.tokens.list(), lexer::tokenize(dialect, text).as_slice(), "{dialect:?}");
            let comment = (crate::theme::LIGHT.text_muted, true);
            assert_eq!(colour_at(&job, note) == comment, dialect == Dialect::MySql, "{dialect:?}: the job follows the dialect");
        }
    }

    #[test]
    fn an_edit_lexes_only_near_the_change() {
        let line = "select o.id, 'x' as label, 42.5 from shop.orders o; -- note\n";
        let mut text = line.repeat(4000);
        let mut tokens = Tokens::default();
        tokens.update(&text);
        let (_, full) = tokens.work();
        assert_eq!(full, text.len());
        for at in [text.len(), text.len() / 2, 0] {
            text.insert(at, 'x');
            let (_, before) = tokens.work();
            tokens.update(&text);
            let lexed = tokens.work().1 - before;
            assert!(lexed <= 2 * RESYNC_BYTES, "an edit at {at} lexed {lexed} bytes");
            assert_eq!(tokens.list(), lexer::tokenize(Dialect::Postgres, &text).as_slice());
        }
        // An edit that opens a string changes the rest of the text: all of it is lexed again.
        text.insert(10, '\'');
        let (_, before) = tokens.work();
        tokens.update(&text);
        assert!(tokens.work().1 - before > text.len() / 2);
        assert_eq!(tokens.list(), lexer::tokenize(Dialect::Postgres, &text).as_slice());
    }

    #[test]
    fn only_the_lines_near_the_view_are_coloured() {
        let syntax = &crate::theme::LIGHT;
        let plain = Color32::BLACK;
        let text = "select 1;\n".repeat(1000);
        let mut tokens = Tokens::default();
        tokens.update(&text);
        let job = tokens.job(128..256, syntax, plain);
        let line = |n: usize| n * "select 1;\n".len();
        assert_eq!(colour_at(&job, line(0)), (plain, false), "before the window");
        assert_eq!(colour_at(&job, line(128)), (syntax.syn_keyword, false));
        assert_eq!(colour_at(&job, line(255) + 7), (syntax.syn_number, false));
        assert_eq!(colour_at(&job, line(256)), (plain, false), "after the window");
        assert!(job.sections.len() < 2 * 128 * 3, "{} sections", job.sections.len());
        // The window moves in whole chunks: a few lines of scrolling keep it.
        assert_eq!(colour_window(0..40), 0..CHUNK);
        assert_eq!(colour_window(300..340), CHUNK..4 * CHUNK);
        assert_eq!(colour_window(301..341), colour_window(300..340));
        assert_eq!(colour_window(usize::MAX - 5..usize::MAX), colour_window(usize::MAX - 5..usize::MAX), "no overflow");
    }

    #[test]
    fn the_selector_shows_the_results_around_the_shown_one_when_not_all_fit() {
        let fit = |results: usize, shown: usize, room: f32| {
            let f = selector_fit(results, shown, room, 50.0);
            (f.label, f.from, f.to, f.more)
        };
        let room = |n: usize| 50.0 + MORE_W + n as f32 * result_width(100);
        assert_eq!(fit(3, 2, 50.0 + 3.0 * result_width(3)), (true, 0, 3, false), "all fit, and the label");
        assert_eq!(fit(100, 0, room(10)), (true, 0, 10, true));
        assert_eq!(fit(100, 50, room(10)), (true, 45, 55, true));
        assert_eq!(fit(100, 99, room(10)), (true, 90, 100, true));
        // Short of room the label goes first, never cut; the `…` stays and lists them all.
        assert_eq!(fit(3, 2, MORE_W + result_width(3)), (false, 2, 3, true));
        assert_eq!(fit(3, 2, MORE_W + 2.0 * result_width(3)), (false, 1, 3, true));
        assert_eq!(fit(3, 0, MORE_W + 1.0), (false, 0, 0, true), "room for the `…` alone");
        assert_eq!(fit(100, 7, -5.0), (false, 7, 7, true));
        assert_eq!(fit(100, 7, f32::NAN), (false, 7, 7, true));
        assert_eq!(fit(0, 0, room(10)), (true, 0, 0, false));
        // Longer numbers take more room.
        assert!(result_width(3000) > result_width(30) && result_width(30) > result_width(3));
        assert_eq!(fit(3000, 2999, 50.0 + MORE_W + 10.0 * result_width(3000)), (true, 2990, 3000, true));
    }

    #[test]
    fn highlighting_colours_tokens_and_covers_the_text() {
        let syntax = &crate::theme::LIGHT;
        let plain = Color32::BLACK;
        let text = "SELECT a, \"Odd\" FROM t WHERE x = 'it''s' AND y > 4.5 -- why\n/* block */ $1";
        let job = highlight(text, syntax, plain);
        assert_eq!(job.text, text);
        let at = |needle: &str| colour_at(&job, text.find(needle).unwrap());
        assert_eq!(at("SELECT"), (syntax.syn_keyword, false));
        assert_eq!(at("WHERE"), (syntax.syn_keyword, false));
        assert_eq!(at("a,"), (plain, false));
        assert_eq!(at("\"Odd\""), (syntax.syn_literal, false));
        assert_eq!(at("'it"), (syntax.syn_string, false));
        assert_eq!(at("4.5"), (syntax.syn_number, false));
        assert_eq!(at("-- why"), (syntax.text_muted, true));
        assert_eq!(at("/* block"), (syntax.text_muted, true));
        assert_eq!(at("$1"), (syntax.syn_number, false));
        // Contiguous sections over the whole text, none empty, no two alike side by side.
        let mut end = 0;
        for pair in job.sections.windows(2) {
            assert!(pair[0].format != pair[1].format, "alike sections are merged");
        }
        for s in &job.sections {
            assert_eq!(s.byte_range.start.0, end);
            assert!(s.byte_range.end.0 > s.byte_range.start.0);
            end = s.byte_range.end.0;
        }
        assert_eq!(end, text.len());
        assert!(job.wrap.max_width.is_infinite(), "no soft wrap: a row is a line");
        // Whitespace joins the section before it.
        assert_eq!(highlight("select  \n  1", syntax, plain).sections.len(), 2);
        // Empty text still lays out one (empty) row.
        assert_eq!(highlight("", syntax, plain).sections.len(), 1);
    }
}

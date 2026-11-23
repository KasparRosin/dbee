//! A table tab's STRUCTURE view ([`show`]), between the table bar in its structure state and the
//! tab's bottom bar with the view switch (both drawn by [`crate::table_tab`]), each area the
//! body's full width:
//!
//! - the error band (`structure-error`, [`crate::components::error_band`]) when the last read
//!   failed: `The structure did not load`, the reason, and a note when the page from before still
//!   shows under it;
//! - the page, one scroll both ways: five sections stacked, COLUMNS, INDEXES, FOREIGN KEYS, REFERENCED BY and
//!   TRIGGERS ([`components::structure::Table`]), each a heading, a header and its rows, the next
//!   one [`STRUCTURE_SECTION_GAP`] under the last line of the one before. COLUMNS has a row per
//!   visible column of the table read: its place, name, type, whether it may be NULL, its default
//!   and its key (the primary key, else the column a foreign key points at). INDEXES: name, parts
//!   (` desc`, ` where …`), unique, method, primary key. FOREIGN KEYS: name, columns, the columns
//!   referenced, on delete. REFERENCED BY: the keys of other tables pointing here, from where.
//!   TRIGGERS: name, when, events, for each, the definition on one line (its first lines on
//!   hover). An empty section keeps its heading and header. Everything the engine wrote is drawn
//!   as written, never in capitals; the cells' words are worked out once per read ([`Kept`]).
//!   Cut words read whole on hover.
//!
//! **Widths.** Each column has a least width: its drawn one, a section's Name column widened for
//! its widest name up to twice that. Every table runs as wide as the widest table's least widths,
//! or the body when that is wider; narrower than that the page scrolls sideways. Room the body
//! has past a section's least widths goes to the section's columns whose content is cut (a long
//! default, type, index or reference), shared evenly until each shows whole ([`spread`]); the
//! last column takes what is then left, and gives up what its own content doesn't need of its
//! least width before another column stays cut. A column never widens past the body's edge by
//! itself: nothing in view is pushed out of it. Content is measured once per read.
//!
//! A titled column's right edge is dragged to resize it, and double-clicked to fit it to its
//! content, as in the data grid (`structure-<section>-edge-<key>`, value the column's width).
//! From the first resize in a section its columns keep the widths they have (the last still takes
//! the rest), so only the dragged one changes; the widths last as long as the tab ([`Kept`]),
//! through its reloads.
//!
//! A foreign key, a reference and a referencing table are links: a click, or Enter or Space with
//! the keyboard, opens that table (`table.open`; the table tab switches to its tab when one is
//! open). While the connection is lost they are off and say why.
//!
//! While a read runs (once it has run 300 ms, [`crate::quiet`]): a first read draws COLUMNS'
//! heading and header over ten rows of bars ([`crate::components::skeleton`]), a reload the
//! page from before at [`theme::DIM`]. A first read cancelled leaves `The structure is not
//! loaded.` where the page would be.
//!
//! It reads the tab's [`TabStructure`](dbee_engine::TabStructure) each frame and starts nothing
//! itself but the links' commands: the bottom bar's STRUCTURE (`table.view`) and the table bar's REFRESH (`table.refresh`) and CANCEL
//! (`table.cancel`) are the table tab's. Nothing on the page is selectable.

use std::sync::Arc;
use std::time::Duration;

use dbee_core::{FkAction, IndexInfo, TableRef, TriggerEvent, TriggerTiming};
use dbee_engine::cmd::LOST;
use dbee_engine::{StructureRead, TabId, TableTab};
use egui::{Align, CornerRadius, Layout, ScrollArea, Sense, UiBuilder, vec2};
use serde_json::{Value as Json, json};

use crate::automation::Tags;
use crate::components::error_band::Band;
use crate::components::skeleton::GridRows;
use crate::components::structure::{Cell, Column, PRIMARY_KEY, Resized, Table, widened};
use crate::components::tooltip::Hint;
use crate::theme::size::{
    BAR_LINE, STRUCTURE_CELL_PAD_X, STRUCTURE_COLUMNS_LAST_MIN, STRUCTURE_COLUMNS_W, STRUCTURE_EMPTY_PAD, STRUCTURE_FOREIGN_KEYS_LAST_MIN,
    STRUCTURE_FOREIGN_KEYS_W, STRUCTURE_INDEXES_LAST_MIN, STRUCTURE_INDEXES_W, STRUCTURE_POSITION_W, STRUCTURE_REFERENCED_BY_LAST_MIN,
    STRUCTURE_REFERENCED_BY_W, STRUCTURE_SECTION_GAP, STRUCTURE_TRIGGERS_LAST_MIN, STRUCTURE_TRIGGERS_W,
};
use crate::theme::{self, Text};

/// A resized column is at least as wide as the data grid lets its columns get…
const RESIZE_MIN: f32 = crate::grid::MIN_WIDTH;
/// …and at most this wide: past anything a drag can ask for on a screen.
const RESIZE_MAX: f32 = 8000.0;

/// What the page says after a first read was cancelled.
pub const EMPTY: &str = "The structure is not loaded.";
/// The band's heading when a read failed.
pub const FAILED: &str = "The structure did not load";
/// The band's note when the page from before shows under it.
pub const FAILED_NOTE: &str = "The structure below is the one from before.";

/// A section of the page: its id (`structure-<section>`), heading, and its columns' titles,
/// widths and the last one's least width, after the position's.
struct Section {
    id: &'static str,
    heading: &'static str,
    titles: &'static [&'static str],
    widths: &'static [f32],
    last_min: f32,
}

impl Section {
    /// The table's columns: the position's, then the section's, the last at its least.
    fn columns(&self) -> Vec<Column<'static>> {
        let widths = self.widths.iter().copied().chain([self.last_min]);
        std::iter::once(Column { title: "", width: STRUCTURE_POSITION_W })
            .chain(self.titles.iter().zip(widths).map(|(title, width)| Column { title, width }))
            .collect()
    }
}

/// The five sections, top to bottom.
const SECTIONS: [Section; 5] = [
    Section {
        id: "structure-columns",
        heading: "Columns",
        titles: &["Name", "Type", "Nullable", "Default", "Keys"],
        widths: &STRUCTURE_COLUMNS_W,
        last_min: STRUCTURE_COLUMNS_LAST_MIN,
    },
    Section {
        id: "structure-indexes",
        heading: "Indexes",
        titles: &["Name", "Columns", "Unique", "Method", "Keys"],
        widths: &STRUCTURE_INDEXES_W,
        last_min: STRUCTURE_INDEXES_LAST_MIN,
    },
    Section {
        id: "structure-foreign-keys",
        heading: "Foreign keys",
        titles: &["Name", "Columns", "References", "On delete"],
        widths: &STRUCTURE_FOREIGN_KEYS_W,
        last_min: STRUCTURE_FOREIGN_KEYS_LAST_MIN,
    },
    Section {
        id: "structure-referenced-by",
        heading: "Referenced by",
        titles: &["Name", "Columns", "From"],
        widths: &STRUCTURE_REFERENCED_BY_W,
        last_min: STRUCTURE_REFERENCED_BY_LAST_MIN,
    },
    Section {
        id: "structure-triggers",
        heading: "Triggers",
        titles: &["Name", "When", "Events", "For each", "Definition"],
        widths: &STRUCTURE_TRIGGERS_W,
        last_min: STRUCTURE_TRIGGERS_LAST_MIN,
    },
];

/// Each section's cells' keys in `structure-<section>-<row>-<key>`, by column (the position's
/// column untagged outside COLUMNS).
const KEYS: [&[&str]; 5] = [
    &["position", "name", "type", "nullable", "default", "keys"],
    &["", "name", "columns", "unique", "method", "keys"],
    &["", "name", "columns", "references", "onDelete"],
    &["", "name", "columns", "from"],
    &["", "name", "when", "events", "forEach", "definition"],
];

/// A trigger's definition on one line is cut at this many bytes: wider than any cell.
const LINE_BYTES: usize = 1024;
/// A definition's hover shows at most this many of its lines…
pub const STRUCTURE_HOVER_LINES: usize = 40;
/// …and at most this many bytes of them.
pub const STRUCTURE_HOVER_BYTES: usize = 4096;

/// What a cell of the page shows, worked out once per read.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Empty,
    Position(usize),
    /// A name, in `text`.
    Text(String),
    /// What is said about it, in `text_muted`.
    Muted(String),
    /// A check, or nothing (Nullable, Unique).
    Check(bool),
    PrimaryKey,
    /// A link to `to`: its words, and whether it is COLUMNS' foreign key (with the key's icon).
    Link {
        words: String,
        to: TableRef,
        key: bool,
    },
    /// Long text drawn on one line, `whole` (its first lines) on hover.
    Long {
        line: String,
        whole: String,
    },
}

impl Shown {
    /// The component's cell for it.
    fn cell(&self) -> Cell<'_> {
        match self {
            Shown::Empty | Shown::Check(false) => Cell::Muted(""),
            Shown::Position(n) => Cell::Position(*n),
            Shown::Text(s) => Cell::Text(s),
            Shown::Muted(s) | Shown::Long { line: s, .. } => Cell::Muted(s),
            Shown::Check(true) => Cell::Nullable,
            Shown::PrimaryKey => Cell::PrimaryKey,
            Shown::Link { words, key: true, .. } => Cell::ForeignKey(words),
            Shown::Link { words, key: false, .. } => Cell::Link(words),
        }
    }

    /// Its tag's value: the words as written, `true`/`false` for a check, the number for a
    /// position.
    fn value(&self) -> Json {
        match self {
            Shown::Empty => json!(""),
            Shown::Position(n) => json!(n),
            Shown::Text(s) | Shown::Muted(s) | Shown::Long { line: s, .. } | Shown::Link { words: s, .. } => json!(s),
            Shown::Check(on) => json!(on),
            Shown::PrimaryKey => json!(PRIMARY_KEY),
        }
    }
}

/// `text` kept to `max` bytes, cut at a character with `…` (as the drivers' `kept_text`).
fn kept(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!("{}…", text.get(..text.floor_char_boundary(max)).unwrap_or_default())
}

/// A definition on one line: each run of whitespace (line breaks too) one space, kept short.
fn one_line(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if out.len() > LINE_BYTES {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    kept(&out, LINE_BYTES)
}

/// A long text's hover (a definition, or any cut cell): as written, its first [`STRUCTURE_HOVER_LINES`] lines and
/// [`STRUCTURE_HOVER_BYTES`], then `…` when there is more.
fn hover_text(text: &str) -> String {
    let text = text.trim_end();
    let mut end = 0;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if n >= STRUCTURE_HOVER_LINES {
            break;
        }
        end += line.len();
    }
    let head = text.get(..end).unwrap_or_default().trim_end();
    if head.len() > STRUCTURE_HOVER_BYTES {
        kept(head, STRUCTURE_HOVER_BYTES)
    } else if head.len() < text.len() {
        format!("{head}…")
    } else {
        head.to_owned()
    }
}

/// `table` and its columns as a link writes them: `schema.table.column` for one,
/// `schema.table (a, b)` for several.
fn reference(table: &TableRef, columns: &[String]) -> String {
    match columns {
        [] => table.display(),
        [one] => format!("{}.{one}", table.display()),
        many => format!("{} ({})", table.display(), many.join(", ")),
    }
}

/// A foreign key action's words.
fn action(a: FkAction) -> &'static str {
    match a {
        FkAction::NoAction => "no action",
        FkAction::Restrict => "restrict",
        FkAction::Cascade => "cascade",
        FkAction::SetNull => "set null",
        FkAction::SetDefault => "set default",
    }
}

/// An index's Columns: its parts (a column, an expression, `?` for neither; ` desc` after a
/// descending one), then ` where <predicate>` for a partial index.
fn index_columns(index: &IndexInfo) -> String {
    let parts: Vec<String> = index
        .parts
        .iter()
        .map(|p| {
            let part = p.column.as_deref().or(p.expression.as_deref()).unwrap_or("?");
            if p.descending { format!("{part} desc") } else { part.to_owned() }
        })
        .collect();
    let mut words = parts.join(", ");
    if let Some(p) = &index.predicate {
        words.push_str(" where ");
        words.push_str(p);
    }
    words
}

/// COLUMNS' rows for `read`: its table's visible columns in order, each with its place, name,
/// type, whether it may be NULL, its default and its key.
fn column_rows(read: &StructureRead) -> Vec<Vec<Shown>> {
    let info = &read.info;
    info.visible_columns()
        .enumerate()
        .map(|(n, column)| {
            let default = read.structure.defaults.iter().find(|d| d.column == column.name).and_then(|d| d.default.clone()).unwrap_or_default();
            let key = if info.primary_key.contains(&column.name) {
                Shown::PrimaryKey
            } else {
                match info.fk_for(&column.name) {
                    Some(fk) => {
                        let at = fk.columns.iter().position(|c| *c == column.name).unwrap_or(0);
                        let target = fk.target_columns.get(at).map_or("", String::as_str);
                        Shown::Link { words: format!("{}.{target}", fk.target.display()), to: fk.target.clone(), key: true }
                    }
                    None => Shown::Empty,
                }
            };
            vec![
                Shown::Position(n.saturating_add(1)),
                Shown::Text(column.name.clone()),
                Shown::Muted(column.label().to_owned()),
                Shown::Check(!column.not_null),
                Shown::Muted(default),
                key,
            ]
        })
        .collect()
}

/// The page's rows for `read`, section by section.
fn rows_of(read: &StructureRead) -> [Vec<Vec<Shown>>; 5] {
    let st = &read.structure;
    let indexes = st
        .indexes
        .iter()
        .map(|i| {
            vec![
                Shown::Empty,
                Shown::Text(i.name.clone()),
                Shown::Text(index_columns(i)),
                Shown::Check(i.unique),
                Shown::Muted(i.method.clone().unwrap_or_default()),
                if i.primary { Shown::PrimaryKey } else { Shown::Empty },
            ]
        })
        .collect();
    let foreign_keys = read
        .info
        .foreign_keys
        .iter()
        .map(|fk| {
            // Matched as the engine matches them: SQLite may spell the target another way.
            let columns: Vec<&str> = fk.columns.iter().map(String::as_str).collect();
            let actions = st.actions_of(&columns, &fk.target.schema, &fk.target.name);
            let on_delete = actions.and_then(|a| a.on_delete).map_or("", action);
            vec![
                Shown::Empty,
                Shown::Text(fk.name.clone()),
                Shown::Text(fk.columns.join(", ")),
                Shown::Link { words: reference(&fk.target, &fk.target_columns), to: fk.target.clone(), key: false },
                Shown::Muted(on_delete.to_owned()),
            ]
        })
        .collect();
    let referenced_by = read
        .referenced_by
        .iter()
        .map(|(from, fk)| {
            vec![
                Shown::Empty,
                Shown::Text(fk.name.clone()),
                Shown::Text(fk.target_columns.join(", ")),
                Shown::Link { words: reference(from, &fk.columns), to: from.clone(), key: false },
            ]
        })
        .collect();
    let triggers = st
        .triggers
        .iter()
        .map(|t| {
            let when = match t.timing {
                Some(TriggerTiming::Before) => "before",
                Some(TriggerTiming::After) => "after",
                Some(TriggerTiming::InsteadOf) => "instead of",
                None => "",
            };
            let events: Vec<&str> = t
                .events
                .iter()
                .map(|e| match e {
                    TriggerEvent::Insert => "insert",
                    TriggerEvent::Update => "update",
                    TriggerEvent::Delete => "delete",
                    TriggerEvent::Truncate => "truncate",
                })
                .collect();
            let each = match t.for_each_row {
                Some(true) => "row",
                Some(false) => "statement",
                None => "",
            };
            vec![
                Shown::Empty,
                Shown::Text(t.name.clone()),
                Shown::Muted(when.to_owned()),
                Shown::Muted(events.join(", ")),
                Shown::Muted(each.to_owned()),
                Shown::Long { line: one_line(&t.definition), whole: hover_text(&t.definition) },
            ]
        })
        .collect();
    [column_rows(read), indexes, foreign_keys, referenced_by, triggers]
}

/// The page for one read: each section's rows, its columns' least widths and the widths their
/// content needs, and its links' cells; the widest table's least width.
#[derive(Debug, Default)]
struct Page {
    rows: [Vec<Vec<Shown>>; 5],
    /// Each column's least width: as drawn, the Name column widened for its widest name.
    least: [Vec<f32>; 5],
    /// The width each column needs to show its title and its widest content whole.
    needs: [Vec<f32>; 5],
    links: [Vec<(usize, usize)>; 5],
    width: f32,
}

/// Each section's Name column, after the position's.
const NAME_COLUMN: usize = 1;

/// The least width of a section's column `col`, drawn `drawn` wide, whose widest text is `widest`
/// wide: the Name column widens for a long name ([`widened`]); every other starts as drawn.
fn least_width(col: usize, drawn: f32, widest: f32) -> f32 {
    if col == NAME_COLUMN { widened(drawn, widest) } else { drawn }
}

/// A section's columns' widths in `room`: each one's set width, or else its least (`least`), and
/// what room is left past those shared among the columns that are not set and need more (`needs`)
/// to show their content whole, evenly, none getting more than it needs. Never wider in all than
/// `room` unless the set and least widths already are.
///
/// The last column takes the rest of the table when it is drawn, so its width here is only what
/// it must keep: its least, or less when its content needs less. What it can spare that way is
/// room for the columns that are cut, before the table has any to spare.
fn spread(least: &[f32], needs: &[f32], set: &[Option<f32>], room: f32) -> Vec<f32> {
    let last = least.len().saturating_sub(1);
    let kept = |i: usize, w: f32| if i == last { needs.get(i).map_or(w, |n| w.min(*n)) } else { w };
    let mut widths: Vec<f32> = least.iter().enumerate().map(|(i, w)| set.get(i).copied().flatten().unwrap_or(kept(i, *w)).max(0.0)).collect();
    let mut free = room - widths.iter().sum::<f32>();
    // Each round gives every column still short an even share, or all it still needs when that
    // is less: at least one column is filled per round, so the rounds are at most the columns.
    for _ in 0..widths.len() {
        let short: Vec<(usize, f32)> = widths
            .iter()
            .enumerate()
            .filter(|(i, _)| set.get(*i).copied().flatten().is_none())
            .filter_map(|(i, w)| needs.get(i).map(|n| (i, n - w)).filter(|(_, d)| *d > 0.0))
            .collect();
        if short.is_empty() || !free.is_finite() || free <= 0.0 {
            break;
        }
        let share = free / short.len() as f32;
        for (i, lack) in short {
            let give = lack.min(share);
            if let Some(w) = widths.get_mut(i) {
                *w += give;
                free -= give;
            }
        }
    }
    widths
}

impl Page {
    /// The page for `read`, measured in `ui`.
    fn of(ui: &egui::Ui, read: &StructureRead) -> Page {
        let rows = rows_of(read);
        let mut page = Page::default();
        let pad = 2.0 * STRUCTURE_CELL_PAD_X;
        for (n, (section, rows)) in SECTIONS.iter().zip(&rows).enumerate() {
            let columns = section.columns();
            let widest = |col: usize| rows.iter().filter_map(|r| r.get(col)).map(|s| s.cell().width(ui)).fold(0.0, f32::max);
            let mut least = Vec::with_capacity(columns.len());
            let mut needs = Vec::with_capacity(columns.len());
            for (col, c) in columns.iter().enumerate() {
                let content = widest(col);
                let title = if c.title.is_empty() {
                    0.0
                } else {
                    ui.painter().layout_job(Text::Header.job_as_written(c.title, egui::Color32::PLACEHOLDER)).size().x
                };
                least.push(least_width(col, c.width, content));
                // The position's column keeps its width: it has no title and no edge to drag.
                needs.push(if c.title.is_empty() { c.width } else { (content.max(title) + pad).ceil() });
            }
            let links = rows
                .iter()
                .enumerate()
                .flat_map(|(row, cells)| cells.iter().enumerate().filter(|(_, s)| matches!(s, Shown::Link { .. })).map(move |(col, _)| (row, col)))
                .collect();
            page.width = page.width.max(least.iter().sum());
            if let (Some(w), Some(d), Some(l)) = (page.least.get_mut(n), page.needs.get_mut(n), page.links.get_mut(n)) {
                *w = least;
                *d = needs;
                *l = links;
            }
        }
        page.rows = rows;
        page
    }
}

/// What the window keeps for a table tab's page: the page worked out from the last read shown
/// (a definition's one line and its hover built once, the widths measured once), and the widths
/// its columns were resized to; dropped with the tab.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    read: Option<Arc<StructureRead>>,
    page: Page,
    /// Each section's columns' set widths: none until a column of the section is resized, then
    /// every column's but the last's (which is set only when it is resized itself).
    set: [Vec<Option<f32>>; 5],
}

impl Kept {
    /// Works the page out for `read` when it is another read than the one kept.
    fn read(&mut self, ui: &egui::Ui, read: &Arc<StructureRead>) {
        if !self.read.as_ref().is_some_and(|r| Arc::ptr_eq(r, read)) {
            self.page = Page::of(ui, read);
            self.read = Some(Arc::clone(read));
        }
    }

    /// Section `n`'s columns' widths in a body `body` wide ([`spread`]): in the body's width, or
    /// in the section's least widths when those are wider. A page wider than the body (another
    /// section's table is) gives the section no room of its own: what is out of view to the side
    /// is the last column's rest, so that widening a cut column pushes nothing out of view.
    fn widths(&self, n: usize, body: f32) -> Vec<f32> {
        let least = self.page.least.get(n).map_or(&[][..], Vec::as_slice);
        let needs = self.page.needs.get(n).map_or(&[][..], Vec::as_slice);
        let set = self.set.get(n).map_or(&[][..], Vec::as_slice);
        spread(least, needs, set, body.max(least.iter().sum()))
    }

    /// Takes what a handle of section `n` asked for column `col`, the section drawn with `drawn`
    /// widths: every column but the last keeps the width it has, and `col` gets the one asked.
    fn resize(&mut self, n: usize, col: usize, asked: Resized, drawn: &[f32]) {
        let need = self.page.needs.get(n).and_then(|d| d.get(col)).copied();
        let Some(set) = self.set.get_mut(n) else { return };
        let last = drawn.len().saturating_sub(1);
        if col == 0 || col > last {
            return;
        }
        set.resize(drawn.len(), None);
        for (i, (kept, now)) in set.iter_mut().zip(drawn).enumerate() {
            if i < last && kept.is_none() {
                *kept = Some(*now);
            }
        }
        let width = match asked {
            Resized::To(w) => Some(w),
            Resized::Fit => need,
        };
        if let (Some(w), Some(kept)) = (width.filter(|w| w.is_finite()), set.get_mut(col)) {
            *kept = Some(w.clamp(RESIZE_MIN, RESIZE_MAX));
        }
    }
}

/// How many rows COLUMNS has for `read`: the facts' column count.
pub(crate) fn columns_of(read: &StructureRead) -> usize {
    read.info.visible_columns().count()
}

/// Draws tab `id`'s STRUCTURE view of `tab` into `ui` (all of its room, under the table bar), from
/// what `kept` keeps for it. `loading` is how long the running read has run, once past 300 ms;
/// `lost` turns the links off. Returns the commands its clicks asked for.
pub(crate) fn show(
    ui: &mut egui::Ui,
    tags: &mut Tags,
    id: TabId,
    tab: &TableTab,
    kept: &mut Kept,
    loading: Option<Duration>,
    lost: bool,
) -> Vec<(&'static str, Json)> {
    let st = &tab.structure;
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    if let Some(error) = st.error.as_deref() {
        // The server's DETAIL and HINT as fields of their own, out of the message (as a failed
        // Apply's band shows them).
        let (detail, hint) = (st.detail.as_deref(), st.hint.as_deref());
        let message = crate::table_tab::without_fields(error, detail, hint);
        let band = Band { heading: FAILED, message, detail, hint, note: st.shown.is_some().then_some(FAILED_NOTE), ..Band::default() };
        let r = band.show(ui, egui::Id::new(("dbee-structure-error", id.0))).response;
        tags.tag_value("structure-error", &r, error);
        let (line, _) = ui.allocate_exact_size(vec2(r.rect.width(), BAR_LINE), Sense::hover());
        ui.painter().rect_filled(line, CornerRadius::ZERO, theme::tokens(ui.ctx()).line);
    }
    let area = ui.available_rect_before_wrap();
    let mut page = ui.new_child(UiBuilder::new().id_salt(("dbee-structure-page", id.0)).max_rect(area).layout(Layout::top_down(Align::Min)));
    page.set_clip_rect(ui.clip_rect().intersect(area));
    page.spacing_mut().item_spacing = egui::Vec2::ZERO;
    let mut asked = Vec::new();
    match st.shown.as_ref() {
        Some(read) => {
            // A reload: the page from before at half until the new one lands.
            if loading.is_some() {
                page.multiply_opacity(theme::DIM);
            }
            kept.read(&page, read);
            ScrollArea::both().id_salt(("dbee-structure", id.0)).auto_shrink([false, false]).show(&mut page, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                asked = sections(ui, tags, id, kept, lost);
            });
        }
        // A first read on its way: from 300 ms, COLUMNS' heading and header over rows of bars.
        None if st.loading.is_some() => {
            if loading.is_some() {
                skeleton(&mut page, tags, id);
            }
        }
        // A failed first read: the band says it all.
        None if st.error.is_some() => {}
        // A first read cancelled: the words stand where the page would, 12 in and 12 down.
        None => {
            page.add_space(STRUCTURE_EMPTY_PAD);
            page.horizontal(|ui| {
                ui.add_space(STRUCTURE_EMPTY_PAD);
                let r = ui.label(Text::Body.rich_as_written(EMPTY).color(theme::tokens(ui.ctx()).text_muted));
                tags.tag_value("structure-empty", &r, EMPTY);
            });
        }
    }
    asked
}

/// The page's sections from what `kept` keeps, top to bottom, every table as wide as the widest;
/// the commands a link's click asked for. A link out of view is listed where it is laid out, and
/// scrolled to when a click asks for it (as the tree's rows are) or the keyboard reaches it. What
/// a column's edge asked is kept for the next frame.
fn sections(ui: &mut egui::Ui, tags: &mut Tags, id: TabId, kept: &mut Kept, lost: bool) -> Vec<(&'static str, Json)> {
    let mut asked = Vec::new();
    // Every table as wide as the body, or as the widest table when that is wider.
    let body = ui.available_width();
    let all: Vec<Vec<f32>> = (0..SECTIONS.len()).map(|n| kept.widths(n, body)).collect();
    let width = all.iter().map(|w| w.iter().sum::<f32>()).fold(body.max(kept.page.width), f32::max);
    let mut resized = None;
    let page = &kept.page;
    let parts = SECTIONS.iter().zip(&page.rows).zip(&all).zip(&page.links).zip(KEYS);
    for (n, ((((section, rows), widths), links), keys)) in parts.enumerate() {
        if n > 0 {
            ui.add_space(STRUCTURE_SECTION_GAP);
        }
        let columns: Vec<Column<'_>> = section.columns().into_iter().zip(widths).map(|(c, w)| Column { width: *w, ..c }).collect();
        let table = Table { heading: section.heading, columns: &columns, rows: rows.len(), links: !lost, link_cells: links, width, resize: true };
        let drawn =
            table.show(ui, egui::Id::new((section.id, id.0)), |row, col| rows.get(row).and_then(|r| r.get(col)).map_or(Cell::Muted(""), Shown::cell));
        tags.tag_value(section.id, &drawn.heading, section.heading);
        for (col, r) in &drawn.handles {
            if let (Some(key), Some(w)) = (keys.get(*col).filter(|k| !k.is_empty()), drawn.widths.get(*col)) {
                tags.tag_value(&format!("{}-edge-{key}", section.id), r, json!(w));
            }
        }
        if let Some((col, what)) = drawn.resized {
            resized = Some((n, col, what, drawn.widths.clone()));
        }
        for (row, col, r, cut) in drawn.cells {
            let (Some(shown), Some(key)) = (rows.get(row).and_then(|r| r.get(col)), keys.get(col).filter(|k| !k.is_empty())) else { continue };
            let tag = format!("{}-{row}-{key}", section.id);
            match shown {
                Shown::Link { to, .. } if lost => {
                    // Not a disabled widget to egui (it draws itself off): its hover as a hint.
                    let r = r.hint(format!("Can't open {} now: {LOST}", to.display()));
                    tags.tag_value_as(&tag, &r, shown.value(), false);
                }
                Shown::Link { to, words, .. } => {
                    if r.clicked() {
                        asked.push(("table.open", json!({ "schema": to.schema, "table": to.name })));
                    }
                    let r = if cut && r.hovered() { r.hint(hover_text(words)) } else { r };
                    tags.tag_value(&tag, &r, shown.value());
                }
                Shown::Long { whole, .. } => {
                    let r = r.hint(whole.as_str());
                    tags.tag_value(&tag, &r, shown.value());
                }
                // Cut words read whole on hover, as much of them as a definition's hover shows (a
                // default or an index's parts can run to many KiB).
                Shown::Text(words) | Shown::Muted(words) if cut => {
                    let r = if r.hovered() { r.hint(hover_text(words)) } else { r };
                    tags.tag_value(&tag, &r, shown.value());
                }
                _ => tags.tag_value(&tag, &r, shown.value()),
            }
        }
    }
    if let Some((n, col, what, drawn)) = resized {
        kept.resize(n, col, what, &drawn);
    }
    asked
}

/// A first read's skeleton: COLUMNS' heading and header over [`crate::theme::size::SKELETON_ROWS`]
/// rows of bars, tagged `structure-skeleton` with how many rows it drew.
fn skeleton(ui: &mut egui::Ui, tags: &mut Tags, id: TabId) {
    let Some(section) = SECTIONS.first() else { return };
    let columns = section.columns();
    let table = Table { heading: section.heading, columns: &columns, rows: 0, links: false, link_cells: &[], width: 0.0, resize: false };
    let drawn = table.show(ui, egui::Id::new((section.id, id.0)), |_, _| Cell::Muted(""));
    tags.tag_value(section.id, &drawn.heading, section.heading);
    let widths = table.widths(drawn.rect.width());
    let (rows_rect, rows) = GridRows { widths: widths.get(1..).unwrap_or_default(), right: &[], number_w: STRUCTURE_POSITION_W }.show(ui);
    let r = ui.interact(rows_rect, ui.id().with(("dbee-structure-skeleton", id.0)), Sense::hover());
    tags.tag_value("structure-skeleton", &r, json!({ "rows": rows }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed read's band shows the server's DETAIL and HINT as fields of their own,
    /// the message without them; `structure-error`'s value keeps the whole text.
    #[test]
    fn the_error_band_shows_detail_and_hint() {
        let view = dbee_engine::PageView { applied: dbee_sql::filter::Condition::None, sort: None, page: 0 };
        let mut tab =
            TableTab::new(TabId(1), dbee_engine::ConnectionId(1), dbee_core::TableRef { schema: "s".into(), name: "t".into() }, vec![], view);
        let whole = "permission denied for table t\nDetail: the role has no grant\nHint: ask the owner";
        tab.structure.error = Some(whole.into());
        tab.structure.detail = Some("the role has no grant".into());
        tab.structure.hint = Some("ask the owner".into());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (mut tags, mut kept) = (Tags::default(), Kept::default());
        let mut shapes = Vec::new();
        for _ in 0..2 {
            tags.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = show(ui, &mut tags, TabId(1), &tab, &mut kept, None, false);
            });
            shapes = std::mem::take(&mut out.shapes);
            out.drop_without_applying_deltas();
        }
        let words: Vec<String> = crate::components::texts(&shapes).into_iter().map(|(w, _, _)| w).collect();
        for want in ["permission denied for table t", "the role has no grant", "ask the owner"] {
            assert!(words.iter().any(|w| w == want), "`{want}` painted: {words:?}");
        }
        assert!(
            words.iter().any(|w| w.eq_ignore_ascii_case("detail")) && words.iter().any(|w| w.eq_ignore_ascii_case("hint")),
            "the fields' names: {words:?}"
        );
        assert!(words.iter().all(|w| !w.contains("Detail: ")), "the detail once, as a field: {words:?}");
        assert_eq!(tags.value("structure-error"), Some(&serde_json::json!(whole)));
    }

    /// A column's least width: a section's Name column widens for a long name (up to twice as
    /// drawn); a long default, type or definition starts as drawn.
    #[test]
    fn only_the_name_column_has_a_wider_least() {
        assert_eq!(least_width(NAME_COLUMN, 150.0, 297.0), widened(150.0, 297.0), "a long name widens its column");
        assert_eq!(least_width(NAME_COLUMN, 150.0, 60.0), 150.0, "a short one keeps it");
        for col in [0, 2, 3, 4, 5] {
            assert_eq!(least_width(col, 140.0, 297.0), 140.0, "column {col} starts at its drawn width");
        }
    }

    /// Room past the least widths goes to the columns whose content is cut, evenly, none past what
    /// it needs; with no room to spare every column keeps its least, and a set column its width.
    #[test]
    fn spare_room_goes_to_the_columns_that_are_cut() {
        // COLUMNS as drawn: the position, Name, Type, Nullable, Default, Keys.
        let least = [40.0, 150.0, 140.0, 76.0, 150.0, 240.0];
        let fits = [40.0, 60.0, 80.0, 30.0, 100.0, 120.0];
        let cut = [40.0, 60.0, 200.0, 30.0, 400.0, 120.0];
        let none = [None; 6];
        let sum = |w: &[f32]| w.iter().sum::<f32>();
        // The last column (Keys) is given what it must keep, 120 for its content: the table draws
        // it with the rest of the room.
        let (drawn, keys) = ([40.0, 150.0, 140.0, 76.0, 150.0], 120.0);
        let with = |name_to_default: [f32; 5]| name_to_default.into_iter().chain([keys]).collect::<Vec<f32>>();
        assert_eq!(spread(&least, &fits, &none, 1200.0), with(drawn), "nothing cut: as drawn");
        assert_eq!(spread(&least, &cut, &none, 600.0), with(drawn), "no room at all: as drawn");
        // A table at its least widths, 796: the 120 Keys can spare go to Type and Default, 60 each.
        assert_eq!(spread(&least, &cut, &none, sum(&least)), with([40.0, 150.0, 200.0, 76.0, 210.0]));
        // 100 more: Type is whole, Default takes them.
        assert_eq!(spread(&least, &cut, &none, 896.0), with([40.0, 150.0, 200.0, 76.0, 310.0]));
        // Room for all: each as wide as it needs and no wider.
        assert_eq!(spread(&least, &cut, &none, 996.0), with([40.0, 150.0, 200.0, 76.0, 400.0]));
        assert_eq!(spread(&least, &cut, &none, 2000.0), with([40.0, 150.0, 200.0, 76.0, 400.0]));
        // A last column whose own content is cut keeps its least, and shares the spare room.
        let long_keys = [40.0, 60.0, 80.0, 30.0, 100.0, 300.0];
        assert_eq!(spread(&least, &long_keys, &none, 826.0), [40.0, 150.0, 140.0, 76.0, 150.0, 270.0]);
        // A set column keeps its width, cut or not, and leaves its room to the others.
        let set = [None, None, Some(100.0), None, None, None];
        assert_eq!(spread(&least, &cut, &set, 796.0), with([40.0, 150.0, 100.0, 76.0, 310.0]));
        // Hostile numbers stay finite.
        for room in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(spread(&least, &cut, &none, room).iter().all(|w| w.is_finite()), "{room}");
        }
        let endless = spread(&least, &[f32::INFINITY; 6], &none, 996.0).iter().sum::<f32>();
        assert!((endless - 996.0).abs() < 0.01, "endless needs share the room and no more: {endless}");
    }

    /// A resize in a section sets every column of it but the last at the width it has, and the
    /// column asked at the width asked, within the bounds; a fit takes the content's width.
    #[test]
    fn a_resize_keeps_the_other_columns_where_they_are() {
        let mut kept = Kept::default();
        kept.page.least[0] = vec![40.0, 150.0, 140.0, 76.0, 150.0, 240.0];
        kept.page.needs[0] = vec![40.0, 60.0, 200.0, 30.0, 400.0, 120.0];
        // As the table draws them 996 wide: Keys with the rest.
        assert_eq!(kept.widths(0, 996.0), [40.0, 150.0, 200.0, 76.0, 400.0, 120.0]);
        let drawn = [40.0, 150.0, 200.0, 76.0, 400.0, 130.0];
        kept.resize(0, 4, Resized::To(180.0), &drawn);
        assert_eq!(kept.set[0], [Some(40.0), Some(150.0), Some(200.0), Some(76.0), Some(180.0), None]);
        assert_eq!(kept.widths(0, 2000.0), [40.0, 150.0, 200.0, 76.0, 180.0, 120.0], "more room changes none of them");
        kept.resize(0, 4, Resized::Fit, &kept.widths(0, 996.0));
        assert_eq!(kept.set[0][4], Some(400.0), "fit: as wide as the content needs");
        kept.resize(0, 3, Resized::Fit, &kept.widths(0, 996.0));
        assert_eq!(kept.set[0][3], Some(RESIZE_MIN), "never narrower than the grid's least");
        kept.resize(0, 2, Resized::To(f32::INFINITY), &kept.widths(0, 996.0));
        kept.resize(0, 2, Resized::To(f32::NAN), &kept.widths(0, 996.0));
        assert_eq!(kept.set[0][2], Some(200.0), "a width that is no number changes nothing");
        kept.resize(0, 2, Resized::To(1e9), &kept.widths(0, 996.0));
        assert_eq!(kept.set[0][2], Some(RESIZE_MAX));
        // The last column's set width is its least: it still takes the rest.
        kept.resize(0, 5, Resized::To(300.0), &kept.widths(0, 996.0));
        assert_eq!(kept.set[0][5], Some(300.0));
        // The position's column and a column that isn't there take nothing.
        let before = kept.set.clone();
        kept.resize(0, 0, Resized::To(90.0), &kept.widths(0, 996.0));
        kept.resize(0, 9, Resized::To(90.0), &kept.widths(0, 996.0));
        kept.resize(7, 1, Resized::To(90.0), &[]);
        assert_eq!(kept.set, before);
    }
}

//! A table tab's STRUCTURE view ([`show`]), under the table bar in its structure state (drawn by
//! [`crate::table_tab`]), each area the body's full width (SA):
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
//!   A column other than the first and the last widens for its widest text, up to twice its
//!   drawn width (measured once per read); every table runs as wide as the widest, and the page
//!   scrolls sideways when that is wider than the body. Cut words read whole on hover.
//!
//! A foreign key, a reference and a referencing table are links: a click, or Enter or Space with
//! the keyboard, opens that table (`table.open`; the table tab switches to its tab when one is
//! open). While the connection is lost they are off and say why.
//!
//! While a read runs (once it has run 300 ms, [`crate::quiet`]): a first read draws COLUMNS'
//! heading and header over ten rows of bars ([`crate::components::skeleton`], LC1), a reload the
//! page from before at [`theme::DIM`] (LC2). A first read cancelled leaves `The structure is not
//! loaded.` where the page would be.
//!
//! It reads the tab's [`TabStructure`](dbee_engine::TabStructure) each frame and starts nothing
//! itself but the links' commands: the table bar's STRUCTURE (`table.view`), REFRESH (`table.refresh`) and CANCEL
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
use crate::components::structure::{Cell, Column, PRIMARY_KEY, Table, widened};
use crate::components::tooltip::Hint;
use crate::theme::size::{
    BAR_LINE, STRUCTURE_COLUMNS_LAST_MIN, STRUCTURE_COLUMNS_W, STRUCTURE_EMPTY_PAD, STRUCTURE_FOREIGN_KEYS_LAST_MIN, STRUCTURE_FOREIGN_KEYS_W,
    STRUCTURE_INDEXES_LAST_MIN, STRUCTURE_INDEXES_W, STRUCTURE_POSITION_W, STRUCTURE_REFERENCED_BY_LAST_MIN, STRUCTURE_REFERENCED_BY_W,
    STRUCTURE_SECTION_GAP, STRUCTURE_TRIGGERS_LAST_MIN, STRUCTURE_TRIGGERS_W,
};
use crate::theme::{self, Text};

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

/// A definition's hover: as written, its first [`STRUCTURE_HOVER_LINES`] lines and
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

/// A foreign key action's words (brief §1.2).
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
            let actions = st.actions.iter().find(|a| a.columns == fk.columns && a.target == fk.target);
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

/// The page for one read: each section's rows, its columns' widths and its links' cells, and
/// the widest table's width.
#[derive(Debug, Default)]
struct Page {
    rows: [Vec<Vec<Shown>>; 5],
    widths: [Vec<f32>; 5],
    links: [Vec<(usize, usize)>; 5],
    width: f32,
}

impl Page {
    /// The page for `read`, measured in `ui`: a column other than the position's and the last
    /// widens for its widest text ([`widened`]); the last keeps its least width.
    fn of(ui: &egui::Ui, read: &StructureRead) -> Page {
        let rows = rows_of(read);
        let mut page = Page::default();
        for (n, (section, rows)) in SECTIONS.iter().zip(&rows).enumerate() {
            let columns = section.columns();
            let last = columns.len().saturating_sub(1);
            let widths: Vec<f32> = columns
                .iter()
                .enumerate()
                .map(|(col, c)| {
                    if col == 0 || col == last {
                        return c.width;
                    }
                    let widest = rows.iter().filter_map(|r| r.get(col)).map(|s| s.cell().width(ui)).fold(0.0, f32::max);
                    widened(c.width, widest)
                })
                .collect();
            let links = rows
                .iter()
                .enumerate()
                .flat_map(|(row, cells)| cells.iter().enumerate().filter(|(_, s)| matches!(s, Shown::Link { .. })).map(move |(col, _)| (row, col)))
                .collect();
            page.width = page.width.max(widths.iter().sum());
            if let (Some(w), Some(l)) = (page.widths.get_mut(n), page.links.get_mut(n)) {
                *w = widths;
                *l = links;
            }
        }
        page.rows = rows;
        page
    }
}

/// What the window keeps for a table tab's page: the page worked out from the last read shown
/// (a definition's one line and its hover built once, the widths measured once), dropped with
/// the tab.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    read: Option<Arc<StructureRead>>,
    page: Page,
}

impl Kept {
    /// The page for `read`, worked out again only when it is another read.
    fn page(&mut self, ui: &egui::Ui, read: &Arc<StructureRead>) -> &Page {
        if !self.read.as_ref().is_some_and(|r| Arc::ptr_eq(r, read)) {
            self.page = Page::of(ui, read);
            self.read = Some(Arc::clone(read));
        }
        &self.page
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
        let band = Band { heading: FAILED, message: error, note: st.shown.is_some().then_some(FAILED_NOTE), ..Band::default() };
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
            let shown = kept.page(&page, read);
            ScrollArea::both().id_salt(("dbee-structure", id.0)).auto_shrink([false, false]).show(&mut page, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                asked = sections(ui, tags, id, shown, lost);
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

/// The page's sections from `page`, top to bottom, every table as wide as the widest; the
/// commands a link's click asked for. A link out of view is listed where it is laid out, and
/// scrolled to when a click asks for it (as the tree's rows are) or the keyboard reaches it.
fn sections(ui: &mut egui::Ui, tags: &mut Tags, id: TabId, page: &Page, lost: bool) -> Vec<(&'static str, Json)> {
    let mut asked = Vec::new();
    let parts = SECTIONS.iter().zip(&page.rows).zip(&page.widths).zip(&page.links).zip(KEYS);
    for (n, ((((section, rows), widths), links), keys)) in parts.enumerate() {
        if n > 0 {
            ui.add_space(STRUCTURE_SECTION_GAP);
        }
        let columns: Vec<Column<'_>> = section.columns().into_iter().zip(widths).map(|(c, w)| Column { width: *w, ..c }).collect();
        let table = Table { heading: section.heading, columns: &columns, rows: rows.len(), links: !lost, link_cells: links, width: page.width };
        let drawn =
            table.show(ui, egui::Id::new((section.id, id.0)), |row, col| rows.get(row).and_then(|r| r.get(col)).map_or(Cell::Muted(""), Shown::cell));
        tags.tag_value(section.id, &drawn.heading, section.heading);
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
                    let r = if cut { r.hint(words.as_str()) } else { r };
                    tags.tag_value(&tag, &r, shown.value());
                }
                Shown::Long { whole, .. } => {
                    let r = r.hint(whole.as_str());
                    tags.tag_value(&tag, &r, shown.value());
                }
                // Cut words read whole on hover.
                Shown::Text(words) | Shown::Muted(words) if cut => {
                    let r = r.hint(words.as_str());
                    tags.tag_value(&tag, &r, shown.value());
                }
                _ => tags.tag_value(&tag, &r, shown.value()),
            }
        }
    }
    asked
}

/// A first read's skeleton: COLUMNS' heading and header over [`crate::theme::size::SKELETON_ROWS`]
/// rows of bars, tagged `structure-skeleton` with how many rows it drew.
fn skeleton(ui: &mut egui::Ui, tags: &mut Tags, id: TabId) {
    let Some(section) = SECTIONS.first() else { return };
    let columns = section.columns();
    let table = Table { heading: section.heading, columns: &columns, rows: 0, links: false, link_cells: &[], width: 0.0 };
    let drawn = table.show(ui, egui::Id::new((section.id, id.0)), |_, _| Cell::Muted(""));
    tags.tag_value(section.id, &drawn.heading, section.heading);
    let widths = table.widths(drawn.rect.width());
    let (rows_rect, rows) = GridRows { widths: widths.get(1..).unwrap_or_default(), right: &[], number_w: STRUCTURE_POSITION_W }.show(ui);
    let r = ui.interact(rows_rect, ui.id().with(("dbee-structure-skeleton", id.0)), Sense::hover());
    tags.tag_value("structure-skeleton", &r, json!({ "rows": rows }));
}

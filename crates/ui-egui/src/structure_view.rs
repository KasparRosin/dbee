//! A table tab's STRUCTURE view ([`show`]), under the table bar in its structure state (drawn by
//! [`crate::table_tab`]), each area the body's full width (SA):
//!
//! - the error band (`structure-error`, [`crate::components::error_band`]) when the last read
//!   failed: `The structure did not load`, the reason, and a note when the page from before still
//!   shows under it;
//! - the page, one scroll: five sections stacked, COLUMNS, INDEXES, FOREIGN KEYS, REFERENCED BY and
//!   TRIGGERS ([`components::structure::Table`]), each a heading, a header and its rows, the next
//!   one [`STRUCTURE_SECTION_GAP`] under the last line of the one before. COLUMNS has a row per
//!   visible column of the table read: its place, name, type, whether it may be NULL, its default
//!   and its key (the primary key, else the column a foreign key points at). The other sections
//!   show their heading and header. Everything the engine wrote is drawn as written.
//!
//! While a read runs (once it has run 300 ms, [`crate::quiet`]): a first read draws COLUMNS'
//! heading and header over ten rows of bars ([`crate::components::skeleton`], LC1), a reload the
//! page from before at [`theme::DIM`] (LC2). A first read cancelled leaves `The structure is not
//! loaded.` where the page would be.
//!
//! It reads the tab's [`TabStructure`](dbee_engine::TabStructure) each frame and starts nothing
//! itself: the table bar's STRUCTURE (`table.view`), REFRESH (`table.refresh`) and CANCEL
//! (`table.cancel`) are the table tab's. Nothing on the page is selectable.

use std::time::Duration;

use dbee_core::ColumnInfo;
use dbee_engine::{StructureRead, TabId, TableTab};
use egui::{Align, CornerRadius, Layout, ScrollArea, Sense, UiBuilder, vec2};
use serde_json::{Value as Json, json};

use crate::automation::Tags;
use crate::components::error_band::Band;
use crate::components::skeleton::GridRows;
use crate::components::structure::{Cell, Column, PRIMARY_KEY, Table};
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

/// COLUMNS' cells' keys in `structure-columns-<row>-<key>`, by column.
const COLUMN_KEYS: [&str; 6] = ["position", "name", "type", "nullable", "default", "keys"];

/// A column's key in COLUMNS' Keys.
enum Key {
    None,
    Primary,
    /// The column a foreign key points at, `schema.table.column`.
    Foreign(String),
}

/// A row of COLUMNS.
struct ColumnRow<'a> {
    column: &'a ColumnInfo,
    default: &'a str,
    key: Key,
}

impl ColumnRow<'_> {
    /// Cell `col` and its tag's value.
    fn cell(&self, row: usize, col: usize) -> (Cell<'_>, Json) {
        let c = self.column;
        match col {
            0 => (Cell::Position(row.saturating_add(1)), json!(row.saturating_add(1))),
            1 => (Cell::Text(&c.name), json!(c.name)),
            2 => (Cell::Muted(c.label()), json!(c.label())),
            3 if !c.not_null => (Cell::Nullable, json!(true)),
            3 => (Cell::Muted(""), json!(false)),
            4 => (Cell::Muted(self.default), json!(self.default)),
            _ => match &self.key {
                Key::Primary => (Cell::PrimaryKey, json!(PRIMARY_KEY)),
                Key::Foreign(target) => (Cell::ForeignKey(target), json!(target)),
                Key::None => (Cell::Muted(""), json!("")),
            },
        }
    }
}

/// COLUMNS' rows for `read`: its table's visible columns in order, each with its default and key.
fn column_rows(read: &StructureRead) -> Vec<ColumnRow<'_>> {
    let info = &read.info;
    info.visible_columns()
        .map(|column| {
            let default = read.structure.defaults.iter().find(|d| d.column == column.name).and_then(|d| d.default.as_deref()).unwrap_or("");
            let key = if info.primary_key.contains(&column.name) {
                Key::Primary
            } else {
                match info.fk_for(&column.name) {
                    Some(fk) => {
                        let at = fk.columns.iter().position(|c| *c == column.name).unwrap_or(0);
                        let target = fk.target_columns.get(at).map_or("", String::as_str);
                        Key::Foreign(format!("{}.{}.{}", fk.target.schema, fk.target.name, target))
                    }
                    None => Key::None,
                }
            };
            ColumnRow { column, default, key }
        })
        .collect()
}

/// How many rows COLUMNS has for `read`: the facts' column count.
pub(crate) fn columns_of(read: &StructureRead) -> usize {
    read.info.visible_columns().count()
}

/// Draws tab `id`'s STRUCTURE view of `tab` into `ui` (all of its room, under the table bar).
/// `loading` is how long the running read has run, once past 300 ms. Returns the commands its
/// clicks asked for.
pub(crate) fn show(ui: &mut egui::Ui, tags: &mut Tags, id: TabId, tab: &TableTab, loading: Option<Duration>) -> Vec<(&'static str, Json)> {
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
    match st.shown.as_deref() {
        Some(read) => {
            // A reload: the page from before at half until the new one lands.
            if loading.is_some() {
                page.multiply_opacity(theme::DIM);
            }
            ScrollArea::vertical().id_salt(("dbee-structure", id.0)).auto_shrink([false, false]).show(&mut page, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                sections(ui, tags, id, read);
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
    Vec::new()
}

/// The page's sections for `read`, top to bottom.
fn sections(ui: &mut egui::Ui, tags: &mut Tags, id: TabId, read: &StructureRead) {
    let rows = column_rows(read);
    for (n, section) in SECTIONS.iter().enumerate() {
        if n > 0 {
            ui.add_space(STRUCTURE_SECTION_GAP);
        }
        let columns = section.columns();
        let count = if n == 0 { rows.len() } else { 0 };
        let table = Table { heading: section.heading, columns: &columns, rows: count };
        let mut values: Vec<(usize, usize, Json)> = Vec::new();
        let drawn = table.show(ui, egui::Id::new((section.id, id.0)), |row, col| match rows.get(row) {
            Some(r) => {
                let (cell, value) = r.cell(row, col);
                values.push((row, col, value));
                cell
            }
            None => Cell::Muted(""),
        });
        tags.tag_value(section.id, &drawn.heading, section.heading);
        for ((row, col, r), (_, _, value)) in drawn.cells.iter().zip(values) {
            if let Some(key) = COLUMN_KEYS.get(*col) {
                tags.tag_value(&format!("{}-{row}-{key}", section.id), r, value);
            }
        }
    }
}

/// A first read's skeleton: COLUMNS' heading and header over [`crate::theme::size::SKELETON_ROWS`]
/// rows of bars, tagged `structure-skeleton` with how many rows it drew.
fn skeleton(ui: &mut egui::Ui, tags: &mut Tags, id: TabId) {
    let Some(section) = SECTIONS.first() else { return };
    let columns = section.columns();
    let table = Table { heading: section.heading, columns: &columns, rows: 0 };
    let drawn = table.show(ui, egui::Id::new((section.id, id.0)), |_, _| Cell::Muted(""));
    tags.tag_value(section.id, &drawn.heading, section.heading);
    let widths = table.widths(drawn.rect.width());
    let (rows_rect, rows) = GridRows { widths: widths.get(1..).unwrap_or_default(), right: &[], number_w: STRUCTURE_POSITION_W }.show(ui);
    let r = ui.interact(rows_rect, ui.id().with(("dbee-structure-skeleton", id.0)), Sense::hover());
    tags.tag_value("structure-skeleton", &r, json!({ "rows": rows }));
}

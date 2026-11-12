//! Every control of the design's Components page in every state, with no database, for looking
//! at them against the page's light and dark frames (`examples/components.rs`): one section per
//! page component, in the page's order, each headed with the page's caption.

use std::sync::LazyLock;
use std::time::Duration;

use egui::{Align, Align2, Color32, CornerRadius, Label, Layout, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};

use dbee_config::ConnectionColor;
use dbee_core::RelKind;

use super::Look;
use super::button::{BarButton, BoxButton, IconButton, TextButton};
use super::checkbox::Checkbox;
use super::chip::Chip;
use super::dialog::{Button, Dialog};
use super::dropdown::Dropdown;
use super::editor::{ErrorToken, LineNumber};
use super::error_band::Band;
use super::grid_cell::{self, Align as CellAlign, Cell, CellText, Header, Key, Mark, RowNumber, Sort};
use super::icon::Icon;
use super::input::{Field, Input};
use super::keycap::Keycap;
use super::menu::{self, BarTitle, Item};
use super::results::{ResultBar, ResultTab, RowCount, RunRow, RunState};
use super::rows::{ConnectionRow, TreeKind, TreeRow};
use super::segmented::{Segment, Segmented};
use super::sidebar::{self, Count};
use super::status::{self, Activity, Connection, Facts};
use super::swatch::{COLOURS, Swatch};
use super::tab::{self, NewTab, Tab};
use super::table_bars::{self, Loading, PagingBar, TableBar, View};
use super::toast::Toast;
use super::tooltip;
use super::value::{self, Facts as ValueFacts, Panel, State as ValueState};
use crate::theme::size::{
    BAR_H, CONNECTION_ROW_H, GRID_HEADER_H, KEYCAP_GAP, LINE_NUMBER_W, MENU_PAD, RADIUS_CONTROL, ROW_H, SLIM_BAR_H, SPACE_8, SPACE_12, SPACE_16,
    SPACE_24, SPACE_32, TAB_H, TREE_ROW_H,
};
use crate::theme::{Text, Tokens, Words, connection_color, tokens};

/// The page's frames: 20 between sections.
const BETWEEN: f32 = 20.0;
/// Between the controls of a row of buttons.
const ROW_GAP: f32 = 20.0;
/// The Field and input frame's inputs and fields, and the Dropdown frame's dropdowns.
const INPUT_W: f32 = 160.0;
/// The Segmented frame's small boxes, and the width of a small segment shown alone.
const SEGMENTED_W: f32 = 386.0;
const SMALL_SEGMENT_W: f32 = 77.0;
/// The Icons frame: 20 under its caption; cells of 120 by 40, nine to a row, 8 apart and 20
/// between rows. In a cell the icon's box ends 18 down, centred, and its name starts 24 down.
const ICONS_UNDER_CAPTION: f32 = 20.0;
const ICON_CELL: Vec2 = vec2(120.0, 40.0);
const ICONS_PER_ROW: usize = 9;
const ICON_CELL_GAP: Vec2 = vec2(8.0, 20.0);
const ICON_BOTTOM: f32 = 18.0;
const ICON_NAME_TOP: f32 = 24.0;
/// The check box's mark is shown on an `ink` tile, as in a checkbox.
const ICON_TILE: f32 = 14.0;
/// The Menus frame: the items' widths of its menus (Edit, View, Theme), its scene with the menu
/// bar, and the Theme submenu's overlap of the View menu.
const MENU_W: f32 = 230.0;
const VIEW_MENU_W: f32 = 190.0;
const THEME_MENU_W: f32 = 130.0;
const SCENE: Vec2 = vec2(560.0, 190.0);
const MENU_BAR_W: f32 = 240.0;
const SUBMENU_OVERLAP: f32 = 4.0;
/// The Tabs and bars frame's tab strip and menu bars.
const STRIP_W: f32 = 840.0;
/// The filter bar frame's column, operator and value, as wide as the page draws them.
const FILTER_COLUMN_W: f32 = 140.0;
const FILTER_OP_W: f32 = 160.0;
const FILTER_VALUE_W: f32 = 140.0;
/// The sidebar header frame's filters and counts, and the Tree and sidebar frame's tree rows, as
/// wide as the page draws them; its connection rows.
const SIDEBAR_W: f32 = 259.0;
const CONNECTION_LIST_W: f32 = 279.0;
/// The status bar frames: bars this wide, this far apart.
const STATUS_W: f32 = 1100.0;
const STATUS_APART: f32 = 10.0;
/// The status bars' addresses, as the page writes them, and a long path.
static PG14: LazyLock<status::Address> = LazyLock::new(|| status::Address::Server("postgres@pg14:5432/dbee".into()));
static TUNNEL: LazyLock<status::Address> = LazyLock::new(|| status::Address::Server("postgres@db.internal:5432/dbee via bastion.example.com".into()));
static SHOP_FILE: LazyLock<status::Address> = LazyLock::new(|| status::Address::File("~/data/shop.sqlite".into()));
static LONG_PATH: LazyLock<status::Address> =
    LazyLock::new(|| status::Address::File("~/Projects/clients/northwind-traders/archive/2026/exports/nightly/shop.sqlite".into()));
/// A bar too narrow for the long path.
const LONG_PATH_W: f32 = 560.0;
/// The Dialogs frame's scrim scene: a window of this size, its query text this far in, the dialog
/// this far in.
const DIALOG_SCENE: Vec2 = vec2(640.0, 240.0);
const BEHIND_AT: Vec2 = vec2(24.0, 20.0);
const DIALOG_AT: Vec2 = vec2(100.0, 40.0);
const BEHIND: &str = "select o.id, c.name as customer, o.total\nfrom shop.orders o\njoin shop.customers c on c.id = o.customer_id\nwhere o.total > 100\norder by o.id desc\nlimit 50;";

/// The icons in the order of the page's Icons frame.
const ICONS: [Icon; 32] = [
    Icon::ChevronDown,
    Icon::KeyForeign,
    Icon::KeyPrimary,
    Icon::MaterializedView,
    Icon::View,
    Icon::Table,
    Icon::Schema,
    Icon::Link,
    Icon::NoColour,
    Icon::Search,
    Icon::Wrap,
    Icon::Copy,
    Icon::CheckBox,
    Icon::Check,
    Icon::Last,
    Icon::Next,
    Icon::Previous,
    Icon::First,
    Icon::Code,
    Icon::Import,
    Icon::Export,
    Icon::Panel,
    Icon::TabQuery,
    Icon::TabTable,
    Icon::Refresh,
    Icon::Close,
    Icon::Minus,
    Icon::Plus,
    Icon::Key,
    Icon::SortDescending,
    Icon::SortAscending,
    Icon::ChevronRight,
];

/// Sets dbee's fonts and theme on `ctx`, drawing in `theme`. The fonts are there from the next
/// frame on.
pub fn prepare(ctx: &egui::Context, theme: egui::Theme) {
    crate::theme::apply(ctx);
    ctx.set_theme(theme);
}

/// Every control in every state, one section per page component in the page's order.
pub fn show(ui: &mut Ui) {
    egui::Frame::NONE.inner_margin(SPACE_24).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        section(ui, "icon/, today's drawings in their usual colour", ICONS_UNDER_CAPTION, icons);
        buttons(ui);
        inputs(ui);
        choices(ui);
        section(ui, "keycap: default, disabled", SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = KEYCAP_GAP;
            for key in ["Ctrl", "Shift", "Z"] {
                ui.add(Keycap::new(key));
            }
            // The page's 24 between the groups is an item of the row, with the gap either side.
            ui.add_space(SPACE_24 + KEYCAP_GAP);
            for key in ["Ctrl", "Z"] {
                ui.add_enabled(false, Keycap::new(key));
            }
        });
        section(ui, "chip: next, available", SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_16;
            ui.add(Chip::next("Upcoming"));
            ui.add(Chip::available("Available"));
        });
        menus(ui);
        tooltips_and_toasts(ui);
        dialogs(ui);
        tabs(ui);
        bars(ui);
        grid(ui);
        sidebar_rows(ui);
        value_panels(ui);
        error_bands(ui);
        editor_sample(ui);
    });
}

/// The Grid frame's columns: the row numbers, then each column's width, name, type, key and
/// whether its values sit right.
const GRID_ROW_NUMBERS: f32 = 36.0;
const GRID_COLUMNS: [(f32, &str, &str, Option<Key>, bool); 5] = [
    (100.0, "id", "int8", Some(Key::Primary), true),
    (150.0, "customer", "text", None, false),
    (110.0, "total", "numeric", None, true),
    (150.0, "customer_id", "int4", Some(Key::Foreign), true),
    (90.0, "paid", "bool", None, false),
];

/// One of the Grid frame's rows: its values (`None` for NULL), and how it is drawn.
struct GridRow {
    values: [Option<&'static str>; 5],
    hovered: bool,
    /// Every cell selected; with the focus in this column.
    selected: bool,
    focus: Option<usize>,
    /// The whole row selected (#17).
    whole: bool,
    new: bool,
    deleted: bool,
    /// Cells with a mark, and whether each is selected.
    marks: &'static [(usize, Mark, bool)],
}

const PLAIN: GridRow =
    GridRow { values: [None; 5], hovered: false, selected: false, focus: None, whole: false, new: false, deleted: false, marks: &[] };

/// The Grid frame's twelve rows, as the page draws them. Rows 10 and 11, where the page shows the
/// cell editor over a cell, are plain: the editor is not one of the cells.
const GRID_ROWS: [GridRow; 12] = [
    GridRow { values: [Some("91"), Some("Customer 91"), Some("207.29"), Some("12"), Some("true")], ..PLAIN },
    GridRow { values: [Some("391"), Some("Customer 391"), Some("464.29"), Some("7"), Some("false")], hovered: true, ..PLAIN },
    GridRow { values: [Some("691"), Some("Customer 191"), Some("221.29"), Some("42"), Some("true")], selected: true, focus: Some(3), ..PLAIN },
    GridRow {
        values: [Some("991"), Some("Customer 491"), Some("512.00"), Some("9"), Some("false")],
        marks: &[(2, Mark::Staged, false), (4, Mark::Staged, true)],
        ..PLAIN
    },
    GridRow { values: [Some("1291"), Some("Customer 291"), Some("240.00"), Some("3"), Some("true")], marks: &[(2, Mark::Saved, false)], ..PLAIN },
    GridRow { values: [Some("1591"), Some("Customer 91"), Some("492.29"), Some("12"), Some("true")], marks: &[(2, Mark::Failed, false)], ..PLAIN },
    GridRow {
        values: [Some(""), Some("Customer 999"), Some("100.00"), Some("42"), Some("true")],
        new: true,
        marks: &[(0, Mark::New, false), (1, Mark::New, false), (2, Mark::New, false), (3, Mark::New, false), (4, Mark::New, false)],
        ..PLAIN
    },
    GridRow {
        values: [Some("2791"), Some("Customer 791"), Some("263.29"), Some("42"), Some("false")],
        deleted: true,
        marks: &[
            (0, Mark::Deleted, false),
            (1, Mark::Deleted, false),
            (2, Mark::Deleted, false),
            (3, Mark::Deleted, false),
            (4, Mark::Deleted, false),
        ],
        ..PLAIN
    },
    GridRow { values: [Some("3091"), Some("Leave at↵Ring twice"), None, Some("42"), Some("")], ..PLAIN },
    GridRow { values: [Some("3691"), Some("Customer 3691"), Some("291.29"), Some("42"), Some("false")], ..PLAIN },
    GridRow { values: [Some("4291"), Some("Customer 91"), Some("305.29"), Some("42"), Some("false")], ..PLAIN },
    GridRow {
        values: [Some("4891"), Some("Customer 491"), Some("319.29"), Some("42"), Some("true")],
        selected: true,
        whole: true,
        marks: &[(2, Mark::Staged, true)],
        ..PLAIN
    },
];

/// The grid's cells as the page's Grid frame shows them, then the two icons it draws with.
fn grid(ui: &mut Ui) {
    section(
        ui,
        "grid: header (sorted, text, number, foreign key hovered); rows: plain, hover, selected with the focus on a key (its link), staged and staged focus, saved, failed, new, deleted, NULL / line break / empty, two plain rows where the page shows the cell editor, #17 the whole row selected",
        SPACE_8,
        |ui| {
            let width = GRID_ROW_NUMBERS + GRID_COLUMNS.iter().map(|c| c.0).sum::<f32>();
            let (table, _) = ui.allocate_exact_size(vec2(width, GRID_HEADER_H + ROW_H * GRID_ROWS.len() as f32), Sense::hover());
            ui.painter().rect_filled(table, CornerRadius::ZERO, tokens(ui.ctx()).card);
            let mut x = table.left();
            grid_cell::corner(ui, Rect::from_min_size(table.min, vec2(GRID_ROW_NUMBERS, GRID_HEADER_H)));
            x += GRID_ROW_NUMBERS;
            for (i, &(w, name, ty, key, right)) in GRID_COLUMNS.iter().enumerate() {
                let head = Header {
                    name,
                    ty,
                    key,
                    sort: (i == 0).then_some(Sort::Asc),
                    align: if right { CellAlign::Right } else { CellAlign::Left },
                    sortable: true,
                    hovered: i == 3,
                };
                head.paint(ui, Rect::from_min_size(pos2(x, table.top()), vec2(w, GRID_HEADER_H)));
                x += w;
            }
            for (r, row) in GRID_ROWS.iter().enumerate() {
                let top = table.top() + GRID_HEADER_H + ROW_H * r as f32;
                let number =
                    RowNumber { n: r as u64 + 1, new: row.new, deleted: row.deleted, selected: row.selected, whole: row.whole, hovered: row.hovered };
                number.paint(ui, Rect::from_min_size(pos2(table.left(), top), vec2(GRID_ROW_NUMBERS, ROW_H)));
                let mut x = table.left() + GRID_ROW_NUMBERS;
                for (c, &(w, _, _, key, right)) in GRID_COLUMNS.iter().enumerate() {
                    let mark = row.marks.iter().find(|m| m.0 == c);
                    let cell = Cell {
                        text: row.values[c].map_or(CellText::Null, CellText::Text),
                        align: if right { CellAlign::Right } else { CellAlign::Left },
                        mark: mark.map(|m| m.1),
                        selected: row.selected || mark.is_some_and(|m| m.2),
                        focused: row.focus == Some(c),
                        deleted: row.deleted,
                        link: key == Some(Key::Foreign),
                        row_selected: row.focus.is_some(),
                        hovered: row.hovered,
                    };
                    let at = Rect::from_min_size(pos2(x, top), vec2(w, ROW_H));
                    cell.show(ui, at, ui.id().with(("showcase-grid-link", r, c)));
                    x += w;
                }
            }
        },
    );
    section(ui, "icon/follow, icon/line-break: the grid's follow link and a line break in a cell", ICONS_UNDER_CAPTION, |ui| {
        let t = tokens(ui.ctx());
        ui.spacing_mut().item_spacing.x = ICON_CELL_GAP.x;
        for (icon, colour) in [(Icon::Follow, t.text_muted), (Icon::LineBreak, t.text)] {
            let (cell, _) = ui.allocate_exact_size(ICON_CELL, Sense::hover());
            let p = ui.painter();
            icon.paint(p, pos2(cell.center().x, cell.top() + ICON_BOTTOM - icon.size().y / 2.0), colour);
            Text::Small.paint(p, pos2(cell.center().x, cell.top() + ICON_NAME_TOP), Align2::CENTER_TOP, icon.name(), t.text_muted);
        }
    });
}

/// The tree's rows and the connection list's, as the page's Tree and sidebar frame shows them:
/// each list on `bg` with a 1 px `line` at its right.
fn sidebar_rows(ui: &mut Ui) {
    let rel = TreeKind::Relation;
    let row = |level, kind, name, right, open| TreeRow { level, kind, name, right, open, selected: false, focused: false, hovered: false };
    let table = |name| row(1, rel(RelKind::Table), name, None, Some(false));
    let tree = [
        row(0, TreeKind::Schema, "shop", None, Some(true)),
        table("customers"),
        row(1, rel(RelKind::Table), "orders", None, Some(true)),
        row(2, TreeKind::Column(Some(Key::Primary)), "id", Some("int8"), None),
        row(2, TreeKind::Column(None), "dealer_id", Some("int4"), None),
        row(2, TreeKind::Column(Some(Key::Foreign)), "customer_id", Some("int8"), None),
        row(2, TreeKind::Column(None), "total", Some("numeric(12,2)"), None),
        row(1, rel(RelKind::View), "orders_archive", None, Some(false)),
        row(1, rel(RelKind::MaterializedView), "order_totals", None, Some(false)),
        TreeRow { hovered: true, ..table("payments") },
        TreeRow { selected: true, ..table("shipments") },
        TreeRow { focused: true, ..table("users") },
        row(0, TreeKind::System, "System schemas", None, Some(false)),
    ];
    section(
        ui,
        "tree/row: schema open, table, table open with its columns (primary key, plain, foreign key), view, materialized view, hover, selected, focus (inferred), schema",
        SPACE_8,
        |ui| list(ui, SIDEBAR_W, TREE_ROW_H, tree.len(), |ui, i, rect| tree[i].paint(ui, rect)),
    );
    let server = |engine: &str, address: &str| Words::from(format!("{engine} · ")).name(address);
    let connection =
        |name, line, colour| ConnectionRow { name, line, colour: connection_color(colour), selected: false, focused: false, hovered: false };
    let connections = [
        connection("Analytics replica", server("PostgreSQL", "analytics@replica:5432/analytics"), ConnectionColor::Blue),
        ConnectionRow {
            selected: true,
            ..connection("Shop live", server("PostgreSQL", "postgres@pg14:5432/dbee").plain(" · SSH"), ConnectionColor::Red)
        },
        ConnectionRow { hovered: true, ..connection("Billing staging", server("MySQL", "billing@staging:3306/billing"), ConnectionColor::Orange) },
        ConnectionRow { focused: true, ..connection("Shop dev", server("PostgreSQL", "postgres@localhost:5432/shop"), ConnectionColor::Green) },
        connection("Local postgres", server("PostgreSQL", "postgres@localhost:5432/postgres"), ConnectionColor::None),
        connection("Notes", server("SQLite", "~/data/notes.sqlite"), ConnectionColor::Purple),
    ];
    section(
        ui,
        "sidebar/connection: default, selected, hover, focus (inferred), no colour, other engines; their logos wait for the logo task, the line names the engine",
        SPACE_8,
        |ui| list(ui, CONNECTION_LIST_W, CONNECTION_ROW_H, connections.len(), |ui, i, rect| connections[i].paint(ui, rect)),
    );
}

/// The value panel's frame on the page: 340 by 372, three to a row, 16 apart.
const VALUE_PANEL: Vec2 = vec2(340.0, 372.0);
const VALUE_PANELS_PER_ROW: usize = 3;
const VALUE_PANEL_GAP: f32 = 16.0;

/// The page's values: the `meta` jsonb, the `notes` text, the value being edited, row 3.
const VALUE_JSON: &str =
    r#"{"src": "web", "gift": true, "tags": ["express", "fragile"], "items": 3, "notes": "Leave at the door", "coupon": null, "weight_kg": 1.25}"#;
const VALUE_TEXT: &str = "Leave at the door.\nRing twice, then wait.";
const VALUE_EDITING: &str = "{\n  \"src\": \"web\",\n  \"gift\": false,\n  \"tags\": [\"new\", \"eu\"],\n  \"note\": null\n}";
const VALUE_ROW: &str = r#"{"id": 691, "customer": "Customer 191", "total": 221.29, "shipped_at": "2025-04-02", "note": null, "paid": true}"#;

/// One of the value panel's states as the page shows it: the panel, and the value in it (its text,
/// whether it is JSON, where the editing caret is).
pub(crate) struct ValueSample {
    pub panel: Panel<'static>,
    pub value: Option<(String, bool, Option<usize>)>,
}

/// The value panel's eight states with the page's values, in the page's order.
pub(crate) fn value_samples() -> [ValueSample; 8] {
    let json = dbee_core::json::pretty(VALUE_JSON).unwrap_or_else(|| VALUE_JSON.to_owned());
    let row = dbee_core::json::pretty(VALUE_ROW).unwrap_or_else(|| VALUE_ROW.to_owned());
    let lines = |s: &str| s.split('\n').count();
    let panel = |state, title, ty, facts| Panel { state, title, ty, kind: None, note: None, facts, buttons: true, wrap: false, edge: true };
    let json_facts = Some(ValueFacts { lines: lines(&json), bytes: VALUE_JSON.len() });
    // The value being edited, the caret after `false`.
    let caret = VALUE_EDITING.find("false").map(|at| at + "false".len());
    [
        ValueSample { panel: panel(ValueState::Json, "meta", "jsonb", json_facts), value: Some((json.clone(), true, None)) },
        ValueSample {
            panel: panel(ValueState::Text, "notes", "text", Some(ValueFacts { lines: lines(VALUE_TEXT), bytes: VALUE_TEXT.len() })),
            value: Some((VALUE_TEXT.to_owned(), false, None)),
        },
        ValueSample { panel: panel(ValueState::Null, "coupon", "text", None), value: None },
        ValueSample { panel: panel(ValueState::Empty, "coupon", "text", None), value: None },
        ValueSample { panel: panel(ValueState::NoCell, "", "", None), value: None },
        ValueSample { panel: panel(ValueState::Copied, "meta", "jsonb", json_facts), value: Some((json, true, None)) },
        ValueSample {
            panel: panel(ValueState::Editing, "meta", "json", Some(ValueFacts { lines: lines(VALUE_EDITING), bytes: 58 })),
            value: Some((VALUE_EDITING.to_owned(), true, caret)),
        },
        // The page's facts for the row: a row's size is the value panel screen's to work out.
        ValueSample {
            panel: panel(ValueState::Row, "Row 3", "", Some(ValueFacts { lines: lines(&row), bytes: 134 })),
            value: Some((row, true, None)),
        },
    ]
}

/// Draws `sample` into `at`.
pub(crate) fn value_panel(ui: &mut Ui, sample: &ValueSample, at: Rect) -> value::Shown {
    let shown = sample.panel.show(ui, at);
    if let Some((text, json, caret)) = &sample.value {
        let spans = json.then(|| crate::value_panel::spans(text));
        value::paint_value(ui, shown.text, text, spans.as_deref(), *caret);
    }
    shown
}

/// The value panel in each state as the page's Value panel frame shows it; then as the app draws it
/// until the value panel screen, with today's kind chip and note.
fn value_panels(ui: &mut Ui) {
    let samples = value_samples();
    section(
        ui,
        "value panel: json, text, NULL, empty (inferred), no cell selected (B4), COPIED, editing several lines (MC+), #17 a row as JSON (inferred)",
        SPACE_8,
        |ui| panels(ui, samples.len(), |ui, i, at| _ = value_panel(ui, &samples[i], at)),
    );
    let json = dbee_core::json::pretty(VALUE_JSON).unwrap_or_default();
    let apps = [
        ValueSample {
            panel: Panel { kind: Some(Words::default().name("jsonb")), buttons: false, ..value_samples()[0].panel.clone() },
            value: Some((json, true, None)),
        },
        ValueSample {
            panel: Panel {
                kind: Some(Words::from("text containing JSON")),
                note: Some("Showing the first 10.0 MiB of 19.1 MiB. Too long to format."),
                buttons: false,
                ..value_samples()[0].panel.clone()
            },
            value: Some((VALUE_JSON.to_owned(), true, None)),
        },
        ValueSample { panel: Panel { buttons: false, ..value_samples()[4].panel.clone() }, value: None },
    ];
    section(
        ui,
        "not on the page yet: the app's panel until the value panel screen, no COPY or WRAP; today's kind chip (a type as written) and the note on a cut value, under the bar",
        SPACE_8,
        |ui| panels(ui, apps.len(), |ui, i, at| _ = value_panel(ui, &apps[i], at)),
    );
}

/// The Error band frame's bands: 840 wide, 12 apart.
const BAND_W: f32 = 840.0;
const BANDS_APART: f32 = 12.0;
const NO_BAND: Band<'static> = Band { heading: "", position: None, message: "", hint: None, detail: None, note: None, cancelled: false };
const BAND_TOTL: &str = "column o.totl does not exist";
const BAND_HINT: &str = "Perhaps you meant to reference the column \"o.total\".";

/// The Error band frame's five bands, as the page draws them.
pub(crate) const PAGE_BANDS: [Band<'static>; 5] = [
    Band { heading: "Query failed", position: Some((1, 14)), message: BAND_TOTL, hint: Some(BAND_HINT), ..NO_BAND },
    Band {
        heading: "Query failed",
        message: "insert or update on table \"orders\" violates foreign key constraint \"orders_customer_id_fkey\"",
        detail: Some("Failing row contains (691, 42, null)."),
        ..NO_BAND
    },
    Band { heading: "Query failed", message: "Unknown column 'o.totl' in 'field list'", ..NO_BAND },
    Band { heading: "Query failed", position: Some((1, 14)), message: "no such column: o.totl", ..NO_BAND },
    Band { heading: "Statement 2 failed", position: Some((1, 14)), message: BAND_TOTL, hint: Some(BAND_HINT), ..NO_BAND },
];

/// The bands the app draws until the query tab and table tab screens, which the page doesn't: a
/// cancelled run, today's note under a failed transaction, the table tab's headings and notes.
pub(crate) const APP_BANDS: [Band<'static>; 6] = [
    Band { heading: "The run was cancelled", message: "the statement was cancelled", cancelled: true, ..NO_BAND },
    Band {
        heading: "Query failed",
        message: "current transaction is aborted, commands ignored until end of transaction block (SQLSTATE 25P02)",
        note: Some(crate::query_tab::ROLLBACK_NOTE),
        ..NO_BAND
    },
    Band {
        heading: "The changes were not applied",
        message: "invalid input syntax for type date: \"not a date\"; nothing was written",
        note: Some(crate::table_tab::FAILED_NOTE),
        ..NO_BAND
    },
    Band {
        heading: "The changes were not applied",
        message: "the Apply was cancelled at change 1, so nothing was written",
        note: Some(crate::table_tab::CANCELLED_NOTE),
        ..NO_BAND
    },
    Band { heading: "The connection was lost during Apply", message: "the connection to the server was lost", ..NO_BAND },
    Band {
        heading: "The page did not load",
        message: "invalid input syntax for type integer: \"not a number\"",
        note: Some(crate::table_tab::PAGE_NOTE),
        ..NO_BAND
    },
];

/// The error bands as the page's Error band frame shows them; then the app's until its screens.
fn error_bands(ui: &mut Ui) {
    section(
        ui,
        "error band (EC1): Postgres, hint and position; Postgres, DETAIL and no position; MySQL, the message only; SQLite, a position; statement 2 failed",
        SPACE_8,
        |ui| bands(ui, "page", &PAGE_BANDS),
    );
    section(
        ui,
        "not on the page yet: until the query tab screen, a cancelled run in neutral colours and today's note under a failed transaction (no ROLLBACK button yet); the table tab's headings and notes",
        SPACE_8,
        |ui| bands(ui, "app", &APP_BANDS),
    );
}

fn bands(ui: &mut Ui, salt: &str, bands: &[Band<'_>]) {
    ui.vertical(|ui| {
        ui.set_max_width(BAND_W);
        ui.spacing_mut().item_spacing.y = BANDS_APART;
        for (i, band) in bands.iter().enumerate() {
            band.show(ui, egui::Id::new(("showcase-band", salt, i)));
        }
    });
}

/// The Error band frame's editor: 600 wide on `card`, its lines 12 down; the line numbers in their
/// boxes at its left, the code 12 after them.
const EDITOR_W: f32 = 600.0;
const EDITOR_PAD: f32 = 12.0;

/// The editor as the page draws it: the error on line 1, its token underlined; the cursor on line 3.
fn editor_sample(ui: &mut Ui) {
    section(ui, "editor: the error's line number in danger, the token underlined in danger; the current line's number in text", SPACE_8, |ui| {
        let t = tokens(ui.ctx());
        let line_h = Text::Code.line();
        let (frame, _) = ui.allocate_exact_size(vec2(EDITOR_W, 2.0 * EDITOR_PAD + 3.0 * line_h), Sense::hover());
        let p = ui.painter();
        p.rect_filled(frame, CornerRadius::ZERO, t.card);
        let lines: [(&[(&str, Color32)], Option<&str>, &[(&str, Color32)]); 3] = [
            (&[("SELECT", t.syn_keyword), (" o.id, ", t.text)], Some("o.totl"), &[(", c.email", t.text)]),
            (&[("FROM", t.syn_keyword), (" shop.orders o", t.text)], None, &[]),
            (&[("WHERE", t.syn_keyword), (" o.total > ", t.text), ("100", t.syn_number)], None, &[]),
        ];
        for (i, (before, token, after)) in lines.into_iter().enumerate() {
            let top = frame.top() + EDITOR_PAD + line_h * i as f32;
            let slot = Rect::from_min_size(pos2(frame.left(), top), vec2(LINE_NUMBER_W, line_h));
            LineNumber { n: i + 1, current: i == 2, error: i == 0 }.paint(p, slot);
            let mut x = code(p, pos2(slot.right() + EDITOR_PAD, top), before);
            if let Some(token) = token {
                x = ErrorToken { token }.paint(p, pos2(x, top)).right();
            }
            code(p, pos2(x, top), after);
        }
    });
}

/// `parts` in `code` from `at`, each in its colour; where the text ends.
fn code(p: &egui::Painter, at: Pos2, parts: &[(&str, Color32)]) -> f32 {
    let mut job = egui::text::LayoutJob::default();
    for (words, colour) in parts {
        job.append(words, 0.0, Text::Code.format(*colour));
    }
    let chars = job.text.chars().count();
    let galley = p.layout_job(job);
    let end = galley.pos_from_cursor(egui::text::CCursor::new(chars)).min.x;
    p.galley(at, galley, Color32::PLACEHOLDER);
    at.x + end
}

/// `n` value panels, three to a row, drawn by `panel`.
fn panels(ui: &mut Ui, n: usize, mut panel: impl FnMut(&mut Ui, usize, Rect)) {
    let rows = n.div_ceil(VALUE_PANELS_PER_ROW);
    let width = VALUE_PANEL.x * VALUE_PANELS_PER_ROW as f32 + VALUE_PANEL_GAP * (VALUE_PANELS_PER_ROW - 1) as f32;
    let height = VALUE_PANEL.y * rows as f32 + VALUE_PANEL_GAP * rows.saturating_sub(1) as f32;
    let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    for i in 0..n {
        let (row, col) = (i / VALUE_PANELS_PER_ROW, i % VALUE_PANELS_PER_ROW);
        let at = pos2(frame.left() + (VALUE_PANEL.x + VALUE_PANEL_GAP) * col as f32, frame.top() + (VALUE_PANEL.y + VALUE_PANEL_GAP) * row as f32);
        panel(ui, i, Rect::from_min_size(at, VALUE_PANEL));
    }
}

/// `n` rows `w` wide and `h` tall, one under the other, drawn by `row`, on `bg` with a 1 px `line`
/// at the right.
fn list(ui: &mut Ui, w: f32, h: f32, n: usize, row: impl Fn(&Ui, usize, Rect)) {
    let t = tokens(ui.ctx());
    let (frame, _) = ui.allocate_exact_size(vec2(w + 1.0, h * n as f32), Sense::hover());
    ui.painter().rect_filled(frame, CornerRadius::ZERO, t.bg);
    ui.painter().rect_filled(Rect::from_min_max(pos2(frame.right() - 1.0, frame.top()), frame.max), CornerRadius::ZERO, t.line);
    for i in 0..n {
        row(ui, i, Rect::from_min_size(pos2(frame.left(), frame.top() + h * i as f32), vec2(w, h)));
    }
}

/// The menu bar, the query tab's bars and the status bars, as the page's Tabs and bars frame and
/// its Status bar frame show them.
fn bars(ui: &mut Ui) {
    section(ui, "menu bar, 30 tall, from menubar/title; then with View open", SPACE_8, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_8;
            for open in [None, Some("View")] {
                let (rect, _) = ui.allocate_exact_size(vec2(STRIP_W, SLIM_BAR_H), Sense::hover());
                let titles = ["File", "Edit", "View", "Query"].map(|w| (w, Some(w) == open));
                menu_bar(ui, rect.min, STRIP_W, &titles);
            }
        });
    });
    results(ui);
    table_tab_bars(ui);
    sidebar_header(ui);
    let shop = |colour| Connection { name: "Shop live", colour, own: true };
    let stack = |ui: &mut Ui, rows: Vec<(Connection<'static>, Facts<'static>, Option<Activity>)>| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = STATUS_APART;
            for (connection, facts, activity) in &rows {
                ui.allocate_ui(vec2(STATUS_W, SLIM_BAR_H), |ui| status::bar(ui, connection, facts, activity.as_ref(), |_| {}));
            }
        });
    };
    let pg14 = Some(&*PG14);
    section(ui, "status bar: the connection colour as a 2 px top edge and the name block; no colour keeps a quiet footer", SPACE_8, |ui| {
        let mut colours: Vec<ConnectionColor> = COLOURS.into_iter().filter(|c| *c != ConnectionColor::None).collect();
        colours.push(ConnectionColor::None);
        stack(ui, colours.into_iter().map(|c| (shop(c), Facts { address: pg14, ..Facts::default() }, None)).collect());
    });
    section(
        ui,
        "new: address as typed, server version, link security (only when weaker than fully checked), read-only, SAVE CONNECTION; right: one activity at a time",
        SPACE_8,
        |ui| {
            let red = shop(ConnectionColor::Red);
            let server = Some("PostgreSQL 14.24");
            let link = |words| Some((words, "The link's longer text, on hover."));
            let s = Duration::from_millis;
            let at = |address: &'static status::Address| Facts { address: Some(address), ..Facts::default() };
            stack(
                ui,
                vec![
                    (red, Facts { server, link: link("Not encrypted"), unsaved: true, ..at(&PG14) }, Some(Activity::InTransaction)),
                    (red, Facts { server, read_only: true, ..at(&PG14) }, Some(Activity::Disconnected)),
                    (
                        red,
                        Facts { link: link("Encrypted, server not checked"), ..at(&PG14) },
                        Some(Activity::Reconnecting { elapsed: Some(s(4100)) }),
                    ),
                    (
                        red,
                        Facts { link: link("Encrypted, certificate checked"), ..at(&PG14) },
                        Some(Activity::Exporting { elapsed: Some(s(12_400)) }),
                    ),
                    (red, at(&TUNNEL), Some(Activity::Importing { elapsed: Some(s(3900)) })),
                    (red, at(&SHOP_FILE), Some(Activity::Job { text: Words::from("Counting rows"), elapsed: Some(s(1200)), cancel: true })),
                ],
            );
        },
    );
    section(
        ui,
        "not on the page yet: a job before the app gives timers, today's spinner in the timer's place; a table's name keeps its case; a name dbee made, as written, with no address; a long path cut in the middle",
        SPACE_8,
        |ui| {
            let job = Activity::Job { text: Words::from("Loading ").name("Orders").plain("…"), elapsed: None, cancel: true };
            let made = Connection { name: "postgres@pg14.internal/Shop", colour: ConnectionColor::None, own: false };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = STATUS_APART;
                stack(
                    ui,
                    vec![
                        (shop(ConnectionColor::Blue), Facts { address: pg14, ..Facts::default() }, Some(job)),
                        (made, Facts { server: Some("PostgreSQL 14.24"), ..Facts::default() }, None),
                    ],
                );
                ui.allocate_ui(vec2(LONG_PATH_W, SLIM_BAR_H), |ui| {
                    let facts = Facts { address: Some(&LONG_PATH), read_only: true, ..Facts::default() };
                    status::bar(ui, &shop(ConnectionColor::Red), &facts, None, |_| {});
                });
            });
        },
    );
}

/// The run row, the result tabs, the result bar and the row count, as the page's Tabs and bars
/// frame shows them, and their states before the app gives timers.
fn results(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    let stack = |ui: &mut Ui, rows: &mut dyn FnMut(&mut Ui)| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_12;
            rows(ui);
        });
    };
    let bar = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui(vec2(STRIP_W, BAR_H), |ui| add(ui));
    };
    section(
        ui,
        "run row: idle; running (RUN off, CANCEL the ink button); RUN CURRENT as an option, hidden by default: Kaspar hasn't decided on it",
        SPACE_8,
        |ui| {
            stack(ui, &mut |ui| {
                for (running, run_current) in [(false, false), (true, false), (false, true)] {
                    bar(ui, &mut |ui| {
                        RunRow { running, run_current, can_run: true }.show(ui);
                    });
                }
            });
        },
    );
    section(ui, "result tab: inactive, hover, active, focus (inferred)", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        ui.add(ResultTab::new("select ... series"));
        ui.add(ResultTab::new("select ... series").look(hovered));
        ui.add(ResultTab::new("select ... series").active(true));
        ui.add(ResultTab::new("select ... series").look(focused));
    });
    let two = || vec![ResultTab::new("select ... orders").active(true), ResultTab::new("select ... series")];
    let s = Duration::from_millis;
    section(
        ui,
        "result bar: done, running, failed, cancelled; new facts: 2 rows affected, first 10,000 of 25,000 rows; the value panel open (on)",
        SPACE_8,
        |ui| {
            let done = RunState::Done { rows: 10, ms: Some(24) };
            let rows = [
                ResultBar { tabs: two(), state: done, first_of: None, export: true, value_on: false },
                ResultBar { tabs: two(), state: RunState::Running { elapsed: Some(s(2400)) }, first_of: None, export: true, value_on: false },
                ResultBar { tabs: Vec::new(), state: RunState::Failed { ms: Some(12) }, first_of: None, export: false, value_on: false },
                ResultBar { tabs: Vec::new(), state: RunState::Cancelled { elapsed: Some(s(3200)) }, first_of: None, export: false, value_on: false },
                ResultBar { tabs: two(), state: RunState::Affected { rows: 2, ms: Some(5) }, first_of: None, export: false, value_on: false },
                ResultBar {
                    tabs: two(),
                    state: RunState::Done { rows: 10_000, ms: Some(80) },
                    first_of: Some((10_000, 25_000)),
                    export: true,
                    value_on: false,
                },
                ResultBar { tabs: two(), state: done, first_of: None, export: true, value_on: true },
            ];
            stack(ui, &mut |ui| {
                for row in &rows {
                    bar(ui, &mut |ui| {
                        row.clone().show(ui);
                    });
                }
            });
        },
    );
    section(
        ui,
        "rows/count: exact; estimate with COUNT; unknown with COUNT ROWS; counting with its timer (its CANCEL is the footer job)",
        SPACE_8,
        |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_32;
            let first = Some((1, 100));
            RowCount::Exact(25_000).show(ui, first);
            RowCount::Estimate(100_000).show(ui, first);
            RowCount::Unknown.show(ui, first);
            RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) }.show(ui, first);
        },
    );
    section(
        ui,
        "not on the page yet: before the app gives timers, today's spinner in the timer's place; a result's name keeps its case",
        SPACE_8,
        |ui| {
            stack(ui, &mut |ui| {
                bar(ui, &mut |ui| {
                    let tabs = vec![ResultTab::new("select … Orders").active(true), ResultTab::new("select … Line_Items")];
                    ResultBar { tabs, state: RunState::Running { elapsed: None }, first_of: None, export: true, value_on: false }.show(ui);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACE_32;
                    RowCount::Counting { estimate: None, elapsed: None }.show(ui, Some((101, 200)));
                    RowCount::Exact(1).show(ui, Some((1, 1)));
                });
            });
        },
    );
}

/// The table bar, the filter bar and the paging bar, as the page's Tabs and bars frame shows
/// them, and a page loading before the app gives timers.
fn table_tab_bars(ui: &mut Ui) {
    let stack = |ui: &mut Ui, rows: &mut dyn FnMut(&mut Ui)| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_12;
            rows(ui);
        });
    };
    let bar = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui(vec2(STRIP_W, BAR_H), |ui| add(ui));
    };
    let s = Duration::from_millis;
    let first = Some((1, 100));
    let table = |view, loading, count| TableBar { view, loading, count, shown: first, columns: 8, can_refresh: true, can_add: true, value_on: false };
    section(
        ui,
        "table bar: data, structure, loading; the row count: estimate with COUNT, unknown with COUNT ROWS, counting (its CANCEL is the footer job)",
        SPACE_8,
        |ui| {
            let rows = [
                table(View::Data, None, RowCount::Exact(25_000)),
                table(View::Structure, None, RowCount::Estimate(25_000)),
                table(View::Data, Some(Loading { elapsed: Some(s(800)) }), RowCount::Exact(25_000)),
                table(View::Data, None, RowCount::Estimate(100_000)),
                table(View::Data, None, RowCount::Unknown),
                table(View::Data, None, RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) }),
            ];
            stack(ui, &mut |ui| {
                for row in rows {
                    bar(ui, &mut |ui| {
                        row.show(ui);
                    });
                }
            });
        },
    );
    section(ui, "not on the page yet: a page loading before the app gives timers, today's spinner in the timer's place", SPACE_8, |ui| {
        bar(ui, &mut |ui| {
            table(View::Data, Some(Loading { elapsed: None }), RowCount::Exact(25_000)).show(ui);
        });
    });
    section(
        ui,
        "filter bar: filter row first (raw SQL toggle, column, operator, value, remove, add, APPLY), then AND rows; on card, 4 by 12 inset, line under",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.set_width(STRIP_W);
                table_bars::filter_bar(ui, |ui| {
                    let rows = [("dealer_id", "equals (=)", "42"), ("status", "equals (=)", "completed"), ("total", "greater than (>)", "100")];
                    for (n, (column, op, value)) in rows.into_iter().enumerate() {
                        table_bars::filter_row(ui, |ui| {
                            if n == 0 {
                                ui.add(IconButton::boxed(Icon::Code));
                            } else {
                                table_bars::and_mark(ui);
                            }
                            Dropdown::new(("showcase-filter-column", n), column).width(FILTER_COLUMN_W).show(ui, |_| ());
                            Dropdown::new(("showcase-filter-op", n), op).width(FILTER_OP_W).show(ui, |_| ());
                            let mut value = value.to_owned();
                            ui.add(Input::data(&mut value).desired_width(FILTER_VALUE_W));
                            ui.add(IconButton::boxed(Icon::Minus));
                            if n == 0 {
                                ui.add(IconButton::boxed(Icon::Plus));
                                ui.add(BoxButton::outline("Apply"));
                            }
                        });
                    }
                });
            });
        },
    );
    section(ui, "paging bar: with staged changes (1 CHANGE, RESET, APPLY); without", SPACE_8, |ui| {
        stack(ui, &mut |ui| {
            for changes in [1, 0] {
                bar(ui, &mut |ui| {
                    PagingBar { page: "Page 1 of 250", back: false, forward: true, changes, can_apply: true }.show(ui);
                });
            }
        });
    });
}

/// The sidebar's filter and count, as the page's Tabs and bars frame shows them, and the
/// refresh's states the page doesn't draw.
fn sidebar_header(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let row = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui, usize)| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACE_24;
            for n in 0..4 {
                ui.allocate_ui(vec2(SIDEBAR_W, BAR_H), |ui| add(ui, n));
            }
        });
    };
    section(
        ui,
        "sidebar header: the filter (placeholder, filled, focus) with REFRESH; the count under it (516 tables, 3 of 516 tables, 0 tables, loading)",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = SPACE_8;
                row(ui, &mut |ui, n| {
                    let (typed, look) = match n {
                        0 => ("", None),
                        1 => ("ord", None),
                        2 => ("ord", Some(focused)),
                        _ => return,
                    };
                    let mut text = typed.to_owned();
                    let filter = sidebar::Filter::new(&mut text, "Filter tables").refresh(sidebar::Refresh::Idle);
                    match look {
                        Some(look) => filter.look(look).show(ui),
                        None => filter.show(ui),
                    };
                });
                let counts = [Count::All(516, "tables"), Count::Some(3, 516, "tables"), Count::All(0, "tables"), Count::Loading];
                row(ui, &mut |ui, n| {
                    if let Some(count) = counts.get(n) {
                        count.show(ui);
                    }
                });
            });
        },
    );
    section(
        ui,
        "not on the page yet: the refresh while a refresh runs and off; the connection list's filter, no refresh; typed text keeps its case",
        SPACE_8,
        |ui| {
            row(ui, &mut |ui, n| {
                let (typed, refresh) = match n {
                    0 => ("", Some(sidebar::Refresh::Running)),
                    1 => ("", Some(sidebar::Refresh::Off)),
                    2 => ("", None),
                    _ => ("Order_Items", Some(sidebar::Refresh::Idle)),
                };
                let mut text = typed.to_owned();
                let placeholder = if refresh.is_some() { "Filter tables" } else { "Filter" };
                let filter = sidebar::Filter::new(&mut text, placeholder);
                match refresh {
                    Some(refresh) => filter.refresh(refresh).show(ui),
                    None => filter.show(ui),
                };
            });
        },
    );
}

/// The tabs and the strip, as the page's Tabs and bars frame shows them, and a busy tab.
fn tabs(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(
        ui,
        "tab: table and query; inactive, hover (shows close), active (unsaved dot, close), focus (inferred, like a bar button)",
        SPACE_8,
        |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_16;
            for (tab, title) in [(Tab::table as fn(&'static str) -> Tab<'static>, "orders [3]"), (Tab::query, "Query 1")] {
                tab(title).show(ui);
                tab(title).look(hovered).show(ui);
                tab(title).active(true).unsaved(true).show(ui);
                tab(title).look(focused).show(ui);
            }
        },
    );
    section(ui, "tab strip: tabs, then the new tab button (default, hover, focus), on card with the line under the inactive tabs", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        ui.allocate_ui(vec2(STRIP_W, TAB_H), |ui| {
            tab::strip(ui, |ui| {
                Tab::table("orders [3]").show(ui);
                Tab::table("customers [1]").show(ui);
                Tab::query("Query 1").active(true).unsaved(true).show(ui);
                ui.add(NewTab::new());
            });
        });
        ui.add(NewTab::new().look(hovered));
        ui.add(NewTab::new().look(focused));
    });
    section(ui, "not on the page yet: a busy tab, the spinner in its icon's place; inactive, active", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        Tab::table("orders").busy(true).show(ui);
        Tab::query("Query 1").busy(true).active(true).show(ui);
    });
}

/// The dialogs and the scrim, as the page's Dialogs frame shows them: two to a row, 24 apart and
/// 32 between rows.
fn dialogs(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let id = |n: &str| egui::Id::new("showcase-dialog").with(n);
    let cancel = Button::outline("Cancel", "dialog-cancel");
    section(
        ui,
        "dialog: Screens' dialogs from the components (database password, fill from URL with an error, close anyway, delete connection)",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(SPACE_24, SPACE_32);
                ui.horizontal_top(|ui| {
                    let (mut password, mut save, mut url) = ("secretpass".to_owned(), true, "shop@pg14:5432".to_owned());
                    Dialog::new(id("password"), "Database password")
                        .text(Words::from("Enter the password of ").name("shop").plain(" on ").name("pg14.internal").plain("."))
                        .body(|ui, _| {
                            ui.add(Input::body(&mut password).password(true).look(focused));
                            ui.add(Checkbox::new(&mut save, "Save in keyring"));
                        })
                        .buttons([Button::primary("Connect", "dialog-ok"), cancel])
                        .in_place(ui);
                    Dialog::new(id("url"), "Fill from URL")
                        .body(|ui, _| {
                            Field::unlabelled(Input::body(&mut url).error(true)).error(Some("Not a database URL")).show(ui);
                        })
                        .buttons([Button::primary("Fill", "dialog-ok"), cancel])
                        .in_place(ui);
                });
                ui.horizontal_top(|ui| {
                    Dialog::new(id("close"), "Close anyway?")
                        .text("Close Query 1? Its query text will be lost.")
                        .buttons([Button::danger("Close anyway", "dialog-ok"), cancel])
                        .in_place(ui);
                    Dialog::new(id("delete"), "Delete connection")
                        .text(Words::from("Delete “").name("Shop live").plain("”? Its saved passwords are deleted with it."))
                        .buttons([Button::danger("Delete", "dialog-ok"), cancel])
                        .in_place(ui);
                });
            });
        },
    );
    section(
        ui,
        "new: Apply again (an Apply whose outcome is unknown); the commit prompt on closing a tab or switching connections, COMMIT is the main button",
        SPACE_8,
        |ui| {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = SPACE_24;
                Dialog::new(id("apply"), "Apply again?")
                    .text(
                        "Connection lost during Apply — the outcome is unknown. Reset and reload to check what was written, or Apply again to retry.",
                    )
                    .buttons([Button::danger("Apply again", "dialog-ok"), cancel])
                    .in_place(ui);
                // The page's placeholder words: the commit prompt isn't built yet.
                Dialog::new(id("commit"), "Commit the transaction?")
                    .text("Query 1 has changes that aren't committed.")
                    .buttons([Button::primary("Commit", "commit"), Button::outline("Rollback", "rollback"), cancel])
                    .in_place(ui);
            });
        },
    );
    section(ui, "scrim: over the whole window; the dialog centred, near the top", SPACE_8, |ui| {
        let t = tokens(ui.ctx());
        let (scene, _) = ui.allocate_exact_size(DIALOG_SCENE, Sense::hover());
        let p = ui.painter();
        p.rect_filled(scene, CornerRadius::ZERO, t.bg);
        Text::Code.paint(p, scene.min + BEHIND_AT, Align2::LEFT_TOP, BEHIND, t.text);
        p.rect_filled(scene, CornerRadius::ZERO, t.scrim);
        // The page draws the scene's edge over what it holds, the scrim too.
        p.rect_stroke(scene, CornerRadius::ZERO, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        let at = UiBuilder::new().max_rect(Rect::from_min_size(scene.min + DIALOG_AT, DIALOG_SCENE)).layout(Layout::top_down(Align::Min));
        ui.scope_builder(at, |ui| {
            Dialog::new(id("scrim"), "Close anyway?")
                .text("Close Query 1? Its query text will be lost.")
                .buttons([Button::danger("Close anyway", "dialog-ok"), cancel])
                .in_place(ui);
        });
    });
}

/// The tooltips, the toasts and the toasts' icons, as the page's Tooltips and toasts frame shows
/// them.
fn tooltips_and_toasts(ui: &mut Ui) {
    section(ui, "tooltip: one line, wrapped at 320; 4 px below the pointer, its left edge at the pointer", SPACE_8, |ui| {
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_24;
            tooltip::in_place(ui, "New query tab (Ctrl+T)");
            tooltip::in_place(ui, "Next page: not now, changes are staged. Apply or reset them first, then go to the next page.");
        });
    });
    section(ui, "toast: success, error, error without an action; icon/success, icon/error", SPACE_8, |ui| {
        let t = tokens(ui.ctx());
        ui.spacing_mut().item_spacing.x = SPACE_24;
        ui.add(Toast::success(Words::from("Exported ").name("25,000").plain(" rows to ").name("orders.csv")).action("Open folder"));
        ui.add(Toast::error("Export failed: disk full").action("Details"));
        ui.add(Toast::error("Export failed: disk full"));
        for icon in [Icon::Success, Icon::Error] {
            let (cell, _) = ui.allocate_exact_size(icon.size(), Sense::hover());
            icon.paint(ui.painter(), cell.center(), t.text_muted);
        }
    });
}

/// The menus, their items and the menu bar's titles, as the page's Menus frame shows them.
fn menus(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    section(ui, "menu: Edit as on Screens; menu/item, menu/separator, keycaps", SPACE_8, |ui| {
        menu::in_place(ui, MENU_W, |ui| {
            ui.add(Item::new("Undo").shortcut("Ctrl+Z"));
            ui.add(Item::new("Redo").shortcut("Ctrl+Shift+Z"));
            menu::separator(ui);
            ui.add(Item::new("Copy").shortcut("Ctrl+C").look(hovered));
            ui.add(Item::new("Select All").shortcut("Ctrl+A"));
            menu::separator(ui);
            ui.add_enabled(false, Item::new("Apply").shortcut("Ctrl+S"));
            ui.add_enabled(false, Item::new("Reset"));
        });
    });
    section(ui, "menubar/title open, View menu, Theme submenu: the chosen mark sits in a left column", SPACE_8, |ui| {
        let (scene, _) = ui.allocate_exact_size(SCENE, Sense::hover());
        let titles = menu_bar(ui, scene.min, SCENE.x, &[("File", false), ("Edit", false), ("View", true), ("Query", false)]);
        let view = titles.get(2).map_or(scene.min.x, |r| r.left());
        let at = |pos| UiBuilder::new().max_rect(Rect::from_min_size(pos, SCENE)).layout(Layout::top_down(Align::Min));
        let shown = ui.scope_builder(at(pos2(view, scene.top() + SLIM_BAR_H)), |ui| {
            menu::in_place(ui, VIEW_MENU_W, |ui| {
                ui.add(Item::new("Refresh Schema"));
                ui.add(Item::new("Toggle Value Panel"));
                ui.add(Item::new("Theme").submenu(true).highlighted(true)).rect
            })
        });
        let (menu_rect, theme) = (shown.response.rect, shown.inner.inner);
        let edge = f32::from(MENU_PAD) + 1.0;
        ui.scope_builder(at(pos2(menu_rect.right() - SUBMENU_OVERLAP, theme.top() - edge)), |ui| {
            menu::in_place(ui, THEME_MENU_W, |ui| {
                for (words, chosen) in [("System", false), ("Light", true), ("Dark", false)] {
                    ui.add(Item::new(words).choice(Some(chosen)));
                }
            });
        });
    });
    section(
        ui,
        "menu/item: default, hover (the keyboard highlight looks the same), disabled, destructive, destructive hover; menubar/title: default, open",
        SPACE_8,
        |ui| {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = SPACE_24;
                menu::in_place(ui, MENU_W, |ui| {
                    ui.add(Item::new("Copy").shortcut("Ctrl+C"));
                    ui.add(Item::new("Copy").shortcut("Ctrl+C").look(hovered));
                    ui.add_enabled(false, Item::new("Apply").shortcut("Ctrl+S"));
                    menu::separator(ui);
                    ui.add(Item::new("Delete row").destructive(true));
                    ui.add(Item::new("Delete row").destructive(true).look(hovered));
                });
                let (bar, _) = ui.allocate_exact_size(vec2(MENU_BAR_W, SLIM_BAR_H), Sense::hover());
                menu_bar(ui, bar.min, MENU_BAR_W, &[("File", false), ("Edit", true)]);
            });
        },
    );
}

/// A menu bar `width` wide at `at` holding `titles` (each with whether it is open), as the
/// page's Menus frame draws one ([`menu::bar`]). Returns where the titles are.
fn menu_bar(ui: &mut Ui, at: egui::Pos2, width: f32, titles: &[(&str, bool)]) -> Vec<Rect> {
    let mut bar = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(at, vec2(width, SLIM_BAR_H))).layout(Layout::top_down(Align::Min)));
    menu::bar(&mut bar, |ui| titles.iter().map(|(words, open)| ui.add(BarTitle::new(words).open(*open)).rect).collect())
}

/// The boxed and text buttons in each of their states, as the page's Buttons frame shows them.
fn buttons(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(ui, "button/box primary: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::primary("Run").shortcut("Ctrl+Enter"));
        ui.add(BoxButton::primary("Apply"));
        ui.add(BoxButton::primary("Save"));
        ui.add(BoxButton::primary("Apply").look(focused));
        ui.add_enabled(false, BoxButton::primary("Run").shortcut("Ctrl+Enter"));
    });
    section(ui, "button/box outline: default, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::outline("Cancel"));
        ui.add(BoxButton::outline("Cancel").look(hovered));
        ui.add(BoxButton::outline("Cancel").look(focused));
        ui.add_enabled(false, BoxButton::outline("Clear"));
    });
    section(ui, "button/box danger: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::danger("Delete"));
        ui.add(BoxButton::danger("Delete").look(focused));
        ui.add_enabled(false, BoxButton::danger("Delete"));
    });
    // Bar buttons stand side by side, as in a bar.
    section(ui, "button/bar: default, with divider, hover, on, disabled, focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add(BarButton::label(Icon::Refresh, "Refresh").divider(false));
        ui.add(BarButton::label(Icon::Plus, "Row"));
        ui.add(BarButton::label(Icon::Import, "Import").look(hovered));
        ui.add(BarButton::label(Icon::Wrap, "Wrap").on(true));
        ui.add_enabled(false, BarButton::label(Icon::Copy, "Copy"));
        ui.add(BarButton::label(Icon::Refresh, "Refresh").look(focused));
    });
    section(ui, "button/bar icon: default, hover, on, disabled, focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add(BarButton::icon(Icon::Panel).divider(false));
        ui.add(BarButton::icon(Icon::Panel).divider(false).look(hovered));
        ui.add(BarButton::icon(Icon::Panel).divider(false).on(true));
        ui.add_enabled(false, BarButton::icon(Icon::Panel).divider(false));
        ui.add(BarButton::icon(Icon::Refresh).look(focused));
    });
    section(ui, "button/text: default, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(TextButton::new("Reset"));
        ui.add(TextButton::new("Reset").look(hovered));
        ui.add(TextButton::new("Reset").look(focused));
        ui.add_enabled(false, TextButton::new("Reset"));
    });
    section(ui, "button/icon boxed: plus, minus, code, hover, focus, disabled, on (code: raw WHERE; no change on hover)", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(IconButton::boxed(Icon::Plus));
        ui.add(IconButton::boxed(Icon::Minus));
        ui.add(IconButton::boxed(Icon::Code));
        ui.add(IconButton::boxed(Icon::Plus).look(hovered));
        ui.add(IconButton::boxed(Icon::Plus).look(focused));
        ui.add_enabled(false, IconButton::boxed(Icon::Minus));
        ui.add(IconButton::boxed(Icon::Code).on(true));
    });
    section(ui, "button/icon plain: first, previous, next, last, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        for icon in [Icon::First, Icon::Previous, Icon::Next, Icon::Last] {
            ui.add(IconButton::plain(icon));
        }
        ui.add(IconButton::plain(Icon::Next).look(hovered));
        ui.add(IconButton::plain(Icon::Next).look(focused));
        ui.add_enabled(false, IconButton::plain(Icon::First));
    });
}

/// The inputs in each of their states and two fields, as the page's Field and input frame shows
/// them.
fn inputs(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let rows: [(&str, fn(&mut String) -> Input<'_>, &str, &str, &str); 2] = [
        ("input body: empty, placeholder, filled, focus, disabled, error", Input::body, "shop", "Optional", "shop"),
        ("input data: empty, placeholder, filled, focus, disabled, error", Input::data, "42", "None", "pg14.internal"),
    ];
    for (caption, new, typed, placeholder, wrong) in rows {
        section(ui, caption, SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = ROW_GAP;
            let mut text = [String::new(), String::new(), typed.to_owned(), typed.to_owned(), typed.to_owned(), wrong.to_owned()];
            let [empty, hint, filled, focus, off, error] = &mut text;
            ui.add(new(empty).desired_width(INPUT_W));
            ui.add(new(hint).placeholder(placeholder).desired_width(INPUT_W));
            ui.add(new(filled).desired_width(INPUT_W));
            ui.add(new(focus).look(focused).desired_width(INPUT_W));
            ui.add_enabled(false, new(off).desired_width(INPUT_W));
            ui.add(new(error).error(true).desired_width(INPUT_W));
        });
    }
    section(ui, "field: label plus input; with an error line", SPACE_8, |ui| {
        // The page's fields stand on one line, their tops level.
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = ROW_GAP;
            let (mut name, mut url) = ("shop".to_owned(), "shop".to_owned());
            ui.add(Field::new("Name", Input::body(&mut name).desired_width(INPUT_W)));
            ui.add(Field::new("Database URL", Input::body(&mut url).desired_width(INPUT_W)).error(Some("Not a database URL")));
        });
    });
}

/// The dropdown, checkbox, segmented control and swatch in each of their states, as the page's
/// Dropdown, Checkbox, Segmented and Swatch frames show them.
fn choices(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(ui, "dropdown: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        Dropdown::new("showcase-dropdown", "equals (=)").width(INPUT_W).show(ui, |_| ());
        Dropdown::new("showcase-dropdown-focus", "equals (=)").width(INPUT_W).look(focused).show(ui, |_| ());
        ui.add_enabled_ui(false, |ui| Dropdown::new("showcase-dropdown-off", "equals (=)").width(INPUT_W).show(ui, |_| ()));
    });
    section(ui, "checkbox: on, off, on disabled, off disabled, on focus, off focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_24;
        for (mut on, enabled, look) in [
            (true, true, None),
            (false, true, None),
            (true, false, None),
            (false, false, None),
            (true, true, Some(focused)),
            (false, true, Some(focused)),
        ] {
            let c = Checkbox::new(&mut on, "Read-only");
            ui.add_enabled(enabled, if let Some(look) = look { c.look(look) } else { c });
        }
    });
    let views = [(0, "Data"), (1, "Structure")];
    let modes = [(0, "disable"), (1, "prefer"), (2, "require"), (3, "verify-ca"), (4, "verify-full")];
    let three = [(3, "verify-ca"), (4, "verify-full"), (1, "prefer")];
    section(ui, "segmented caps, 2 segments", SPACE_8, |ui| {
        ui.add(Segmented::caps(&mut 0, &views));
    });
    section(ui, "segmented small, 3 and 5 segments", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(Segmented::small(&mut 1, &three).width(SEGMENTED_W));
        ui.add(Segmented::small(&mut 1, &modes).width(SEGMENTED_W));
    });
    let states = |ui: &mut Ui, segment: &dyn Fn(bool) -> Segment<'static>| {
        ui.spacing_mut().item_spacing.x = SPACE_12;
        ui.add(segment(true));
        ui.add(segment(false));
        ui.add(segment(false).look(hovered));
        ui.add_enabled(false, segment(false));
        ui.add_enabled(false, segment(true));
        ui.add(segment(true).look(focused));
        ui.add(segment(false).look(focused));
    };
    section(ui, "segmented/segment: selected, default, hover, disabled, selected disabled, selected focus, focus", SPACE_8, |ui| {
        states(ui, &|selected| Segment::caps("Data", selected));
    });
    section(ui, "segmented/segment small: selected, default, hover, disabled, selected disabled, selected focus, focus", SPACE_8, |ui| {
        states(ui, &|selected| Segment::small(if selected { "prefer" } else { "require" }, selected).width(SMALL_SEGMENT_W));
    });
    for (caption, selected) in [("swatch: not selected; the last one has focus", false), ("swatch: selected; the last one has focus", true)] {
        section(ui, caption, SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_12;
            for colour in COLOURS {
                ui.add(Swatch::new(colour, selected));
            }
            ui.add(Swatch::new(ConnectionColor::Red, selected).look(focused));
        });
    }
}

/// A section: the page's caption, `gap` under it, then `row`.
fn section(ui: &mut Ui, caption: &str, gap: f32, row: impl FnOnce(&mut Ui)) {
    let t = tokens(ui.ctx());
    ui.add(Label::new(Text::Label.rich(caption).color(t.text_muted)).selectable(false));
    ui.add_space(gap);
    ui.horizontal(row);
    ui.add_space(BETWEEN);
}

/// Each icon with its name, in its usual colour.
fn icons(ui: &mut Ui) {
    let t = tokens(ui.ctx());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = ICON_CELL_GAP;
        for row in ICONS.chunks(ICONS_PER_ROW) {
            ui.horizontal(|ui| {
                for &icon in row {
                    let (cell, _) = ui.allocate_exact_size(ICON_CELL, Sense::hover());
                    let p = ui.painter();
                    let bottom = cell.top() + ICON_BOTTOM;
                    if icon == Icon::CheckBox {
                        let tile = Rect::from_center_size(pos2(cell.center().x, bottom - ICON_TILE / 2.0), Vec2::splat(ICON_TILE));
                        p.rect_filled(tile, CornerRadius::same(RADIUS_CONTROL), t.ink);
                        icon.paint(p, tile.center(), t.on_ink);
                    } else {
                        icon.paint(p, pos2(cell.center().x, bottom - icon.size().y / 2.0), usual(icon, t));
                    }
                    Text::Small.paint(p, pos2(cell.center().x, cell.top() + ICON_NAME_TOP), Align2::CENTER_TOP, icon.name(), t.text_muted);
                }
            });
        }
    });
}

/// The colour the page's Icons frame draws `icon` in.
fn usual(icon: Icon, t: &Tokens) -> Color32 {
    match icon {
        Icon::Key | Icon::KeyPrimary => t.key,
        Icon::SortAscending | Icon::SortDescending => t.ink,
        Icon::CheckBox => t.on_ink,
        Icon::ChevronDown
        | Icon::ChevronRight
        | Icon::Close
        | Icon::TabTable
        | Icon::Check
        | Icon::Search
        | Icon::NoColour
        | Icon::Schema
        | Icon::Table
        | Icon::View
        | Icon::MaterializedView
        | Icon::KeyForeign => t.text_muted,
        _ => t.text,
    }
}

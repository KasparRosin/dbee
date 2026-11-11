//! Every control of the design's Components page in every state, with no database, for looking
//! at them against the page's light and dark frames (`examples/components.rs`): one section per
//! page component, in the page's order, each headed with the page's caption.

use egui::{Align, Align2, Color32, CornerRadius, Label, Layout, Rect, Sense, Ui, Vec2, pos2, vec2};

use dbee_config::ConnectionColor;

use super::Look;
use super::button::{BarButton, BoxButton, IconButton, TextButton};
use super::checkbox::Checkbox;
use super::chip::Chip;
use super::dropdown::Dropdown;
use super::icon::Icon;
use super::input::{Field, Input};
use super::keycap::Keycap;
use super::segmented::{Segment, Segmented};
use super::swatch::{COLOURS, Swatch};
use crate::theme::size::{RADIUS_CONTROL, SPACE_8, SPACE_12, SPACE_16, SPACE_24};
use crate::theme::{Text, Tokens, tokens};

/// The page's frames: 20 between sections.
const BETWEEN: f32 = 20.0;
/// Between the controls of a row of buttons.
const ROW_GAP: f32 = 20.0;
/// Between the keys of one shortcut.
const KEY_GAP: f32 = 3.0;
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
            ui.spacing_mut().item_spacing.x = KEY_GAP;
            for key in ["Ctrl", "Shift", "Z"] {
                ui.add(Keycap::new(key));
            }
            // The page's 24 between the groups is an item of the row, with the gap either side.
            ui.add_space(SPACE_24 + KEY_GAP);
            for key in ["Ctrl", "Z"] {
                ui.add_enabled(false, Keycap::new(key));
            }
        });
        section(ui, "chip: next, available", SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_16;
            ui.add(Chip::next("Upcoming"));
            ui.add(Chip::available("Available"));
        });
    });
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
    section(ui, "button/icon boxed: plus, minus, code, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(IconButton::boxed(Icon::Plus));
        ui.add(IconButton::boxed(Icon::Minus));
        ui.add(IconButton::boxed(Icon::Code));
        ui.add(IconButton::boxed(Icon::Plus).look(hovered));
        ui.add(IconButton::boxed(Icon::Plus).look(focused));
        ui.add_enabled(false, IconButton::boxed(Icon::Minus));
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

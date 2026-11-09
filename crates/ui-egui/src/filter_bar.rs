//! The table tab's filter bar ([`show`]): rows of column ▾ · operator ▾ · value · `−`, joined
//! with AND, or one raw `WHERE` condition.
//!
//! The bar shows the tab's filters as the engine keeps them (`TableTab::filters`, `raw`,
//! `raw_mode`), and every change goes back at once with `table.setFilters {…, apply: false}`, so
//! `document.inspect` shows what is typed. With no filter yet the bar shows one empty row (the
//! first column, `equals`), which becomes a filter when it is changed. Apply, or Enter in a value
//! field, applies the bar (`table.setFilters {apply: true}`). The `<>` toggle switches between the
//! rows and the raw field without applying. The columns come from the schema; the operators are
//! the 13 of `dbee_sql::filter::Op`, each read as its words with its symbol in brackets (`equals
//! (=)`), a word operator by its words alone (`like`, `is null`; [`op_text`]); `is null` and `is
//! not null` take no value, so choosing one clears the value and hides its field. An operator
//! that does on this engine what its words do not say (`Dialect::op_note`: MySQL compares by the
//! column's collation) says so on hover, in its list and as the chosen one. A filter whose
//! operator takes a value and that has none is left out of the condition: its row is muted, its
//! field's hint and hover say so, and the tab title does not count it. While changes
//! are staged typing goes on but Apply waits (its hover says why; Enter asks anyway, and the
//! command says why); while the connection is lost the bar is disabled.
//!
//! Every control of a row is [`ROW_H`] tall and level with the others: the toggle, the choices,
//! the value field, `−`, `+` and Apply, and in the raw form the field and Apply. The operator
//! choice is as wide as its longest label, so a chosen operator always reads whole; in a narrow
//! tab (a small window with the value panel open) the column choice narrows first, then the
//! operator's, their text cut short with `…` ([`choice_widths`]), so a row's buttons stay in view.
//! A choice's list draws its items at one height whether hovered or not, so nothing in it moves
//! under the pointer (egui's own items gain a frame, and its stroke, while hovered).
//!
//! Automation ids: `filter-<n>-column` (value: the column; while open its choices
//! `filter-<n>-column-<column>`), `filter-<n>-op` (value: the operator's id; its choices
//! `filter-<n>-op-<id>`), `filter-<n>-value`, `filter-<n>-unapplied` (on the value field of a row
//! left out; value: its hint), `filter-<n>-remove`, `filter-add`,
//! `filter-raw-toggle` (value: whether the raw field shows), `filter-raw`, `filter-apply`.

use dbee_engine::{Json, TabId};
use dbee_sql::Dialect;
use dbee_sql::filter::{Filter, Op};
use egui::{
    Align, Button, ComboBox, CornerRadius, InnerResponse, Key, Layout, Rect, Response, RichText, Sense, TextEdit, TextWrapMode, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::automation::Tags;
use crate::icons;
use crate::theme::{self, Text, size::RADIUS_CONTROL};

/// The height of every control in a row: the toggle, the choices, the value field and the
/// buttons.
pub const ROW_H: f32 = 26.0;
/// The gap between a row's controls.
const GAP: f32 = 6.0;
/// The square buttons: the toggle, `−` and `+`.
const SQUARE: f32 = ROW_H;
/// Apply's width.
const APPLY_W: f32 = 56.0;
/// The toggle (or `AND`) at a row's left, with the gap after it.
const LEAD_W: f32 = SQUARE + GAP;
/// A row's width without its choices and value field: the lead, `−`, `+`, Apply and the gaps
/// between them (`+` and Apply stand on the first row only; the others keep their room, so the
/// rows line up).
const FIXED_W: f32 = LEAD_W + 2.0 * SQUARE + APPLY_W + 5.0 * GAP;
/// The narrowest and the widest the column choice gets.
const COLUMN_W: (f32, f32) = (77.0, 180.0);
/// The narrowest the operator choice gets (its labels cut short), in a tab too narrow for its
/// longest label.
const OP_MIN_W: f32 = 90.0;
/// The value field's hint on a row left out of the condition.
pub const UNAPPLIED_HINT: &str = "No value: left out";
/// Its hover.
const UNAPPLIED_HOVER: &str = "This filter has no value, so it is left out of the condition. Type a value, or remove the row.";

/// The narrowest a value field gets.
const VALUE_MIN_W: f32 = 56.0;
/// A choice's arrow, the gap before it, and the room left and right of its text.
const ARROW_W: f32 = 10.0;
const ARROW_GAP: f32 = 4.0;
const CHOICE_PAD: f32 = 7.0;
/// A list item's height, the gap between items and the room left and right of an item's text.
pub(crate) const ITEM_H: f32 = 24.0;
pub(crate) const ITEM_GAP: f32 = 2.0;
const ITEM_PAD: f32 = 8.0;

/// The widths of the column and operator choices in a filter row `available` wide, the operator
/// choice needing `op_fit` to show its longest label whole ([`op_fit`]). The operator choice gets
/// its `op_fit` and the column choice up to its widest, with the value field at its narrowest
/// (the value field takes what is left). In a narrow tab (a small window with the value panel
/// open beside it) the column choice gives way first, down to its narrowest, then the operator
/// choice, down to its own, so the row's buttons stay in view.
pub fn choice_widths(available: f32, op_fit: f32) -> (f32, f32) {
    let room = (available - FIXED_W - VALUE_MIN_W).max(0.0);
    let op = op_fit.min(room - COLUMN_W.0).max(OP_MIN_W);
    let column = (room - op).clamp(COLUMN_W.0, COLUMN_W.1);
    (column, op)
}

/// The width of a value field in a row `available` wide with choices `column` and `op` wide:
/// the rest, no narrower than [`VALUE_MIN_W`].
fn value_width(available: f32, column: f32, op: f32) -> f32 {
    (available - FIXED_W - column - op).max(VALUE_MIN_W)
}

/// The width an operator choice needs to show its longest label whole, in `ui`'s button font.
fn op_fit(ui: &egui::Ui) -> f32 {
    let color = theme::tokens(ui.ctx()).text;
    let widest = Op::ALL.into_iter().map(|o| ui.painter().layout_job(Text::Button.job(&op_text(o), color)).size().x).fold(0.0, f32::max);
    // One point more than the text, so rounding never cuts the widest label short.
    (widest + ARROW_GAP + ARROW_W + 2.0 * CHOICE_PAD + 1.0).ceil()
}

/// What the bar shows.
pub(crate) struct Bar<'a> {
    pub tab: TabId,
    /// The table's columns: name and type.
    pub columns: Vec<(&'a str, &'a str)>,
    pub filters: &'a [Filter],
    pub raw: &'a str,
    pub raw_mode: bool,
    /// Why the bar cannot be used now, if it cannot.
    pub disabled: Option<&'static str>,
    /// Why the filters cannot be applied now (staged changes), if they cannot.
    pub blocked: Option<&'static str>,
    /// The connection's, for its operator notes.
    pub dialect: Dialect,
}

/// A command the bar asks for: its id and params.
pub(crate) type Run = (&'static str, Json);

fn set(filters: &[Filter]) -> Run {
    ("table.setFilters", json!({ "filters": filters, "apply": false }))
}

fn apply() -> Run {
    ("table.setFilters", json!({ "apply": true }))
}

/// A square button with an icon; `true` when clicked.
fn icon_button(ui: &mut egui::Ui, tags: &mut Tags, id: &str, hover: &str, selected: bool, icon: impl Fn(&egui::Painter, Rect)) -> bool {
    let t = theme::tokens(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(SQUARE, ROW_H), Sense::click());
    let fill = if selected {
        t.selection
    } else if r.hovered() && r.enabled() {
        t.hover
    } else {
        t.card
    };
    ui.painter().rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, egui::Stroke::new(1.0, t.line_strong), egui::StrokeKind::Inside);
    icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)));
    let r = r.on_hover_text(hover);
    tags.tag_value(id, &r, selected);
    r.clicked()
}

/// Draws the bar into `ui` and returns the commands its changes ask for, in order.
pub(crate) fn show(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags) -> Vec<Run> {
    let mut runs = Vec::new();
    let enabled = bar.disabled.is_none();
    ui.add_enabled_ui(enabled, |ui| {
        if bar.raw_mode {
            raw_row(ui, bar, tags, &mut runs);
        } else {
            filter_rows(ui, bar, tags, &mut runs);
        }
    });
    if let Some(why) = bar.disabled {
        ui.label(Text::Small.rich(format!("The filters can't change now: {why}.")).color(theme::tokens(ui.ctx()).text_muted));
    }
    runs
}

fn toggle(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = theme::tokens(ui.ctx());
    let hover = if bar.raw_mode { "Back to filter rows" } else { "Write a raw WHERE condition" };
    if icon_button(ui, tags, "filter-raw-toggle", hover, bar.raw_mode, |p, r| icons::brackets(p, r, t.text_muted)) {
        runs.push(("table.setFilters", json!({ "rawMode": !bar.raw_mode, "apply": false })));
    }
}

fn apply_button(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = theme::tokens(ui.ctx());
    let enabled = bar.blocked.is_none();
    let b = Button::new(RichText::new("Apply").color(t.on_ink)).fill(t.ink).min_size(vec2(APPLY_W, ROW_H));
    let r = ui.add_enabled(enabled, b);
    let r = match bar.blocked {
        Some(why) => r.on_disabled_hover_text(format!("Apply the filters: not now, {why}")),
        None => r.on_hover_text("Apply the filters (Enter)"),
    };
    tags.tag_value("filter-apply", &r, "Apply");
    if r.clicked() {
        runs.push(apply());
    }
}

/// The hover text of a field where Enter applies the bar.
fn enter_hover(bar: &Bar<'_>) -> String {
    match bar.blocked {
        Some(why) => format!("Enter applies the filters: not now, {why}"),
        None => "Enter applies the filters".into(),
    }
}

/// `true` when Enter was pressed in the text field `r` this frame.
fn entered(ui: &egui::Ui, r: &egui::Response) -> bool {
    r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// A row of the bar, `available` wide: its controls left to right, [`GAP`] apart, all
/// [`ROW_H`] tall and level.
fn row<R>(ui: &mut egui::Ui, available: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(available, ROW_H), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        add(ui)
    })
    .inner
}

/// A one-line field `width` wide and [`ROW_H`] tall, its text centred on the row.
fn field<'t>(edit: TextEdit<'t>, width: f32) -> TextEdit<'t> {
    edit.desired_width(width).min_size(vec2(width, ROW_H)).vertical_align(Align::Center)
}

fn raw_row(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let available = ui.available_width();
    row(ui, available, |ui| {
        toggle(ui, bar, tags, runs);
        let mut text = bar.raw.to_string();
        let width = (available - LEAD_W - APPLY_W - GAP).max(VALUE_MIN_W);
        let r = ui.add(field(
            TextEdit::singleline(&mut text)
                .id(egui::Id::new(("dbee-filter-raw", bar.tab.0)))
                .hint_text("A WHERE condition, e.g. total > 100 AND status = 'paid'")
                .code_editor(),
            width,
        ));
        let r = r.on_hover_text(enter_hover(bar));
        tags.tag_value("filter-raw", &r, text.as_str());
        if r.changed() {
            runs.push(("table.setFilters", json!({ "raw": text, "apply": false })));
        }
        if entered(ui, &r) {
            runs.push(apply());
        }
        apply_button(ui, bar, tags, runs);
    });
}

/// A choice `width` wide and [`ROW_H`] tall showing `text`, cut short with `…` when it does not
/// fit; while open, its list (`list`, at most `list_h` tall before it scrolls).
fn choice<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    list_h: f32,
    text: impl Into<egui::WidgetText>,
    list: impl FnOnce(&mut egui::Ui) -> R,
) -> InnerResponse<Option<R>> {
    ui.allocate_ui_with_layout(vec2(width, ROW_H), Layout::left_to_right(Align::Center), |ui| {
        // The text centred on the row: as much room above and below it as the row leaves.
        let text_h = ui.fonts_mut(|f| f.row_height(&Text::Button.font())).max(ARROW_W);
        let s = ui.spacing_mut();
        s.icon_width = ARROW_W;
        s.icon_spacing = ARROW_GAP;
        s.button_padding = vec2(CHOICE_PAD, ((ROW_H - text_h) / 2.0).max(0.0));
        s.interact_size.y = ROW_H;
        ComboBox::from_id_salt(salt).selected_text(text).width(width).height(list_h).wrap_mode(TextWrapMode::Truncate).show_ui(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ITEM_GAP;
            list(ui)
        })
    })
    .inner
}

/// An item of a choice's list reading `text`, marked when `selected`. It is [`ITEM_H`] tall, and
/// as wide as the list or its text, whether hovered, pressed or not, so the list never moves
/// under the pointer.
pub(crate) fn item(ui: &mut egui::Ui, selected: bool, text: &str) -> Response {
    item_sensing(ui, selected, text, Sense::click())
}

/// [`item`] sensing `sense`: `Sense::CLICK` for a list the keyboard moves through by its own
/// keys, whose items take no focus.
pub(crate) fn item_sensing(ui: &mut egui::Ui, selected: bool, text: &str, sense: Sense) -> Response {
    let t = theme::tokens(ui.ctx());
    let galley = ui.painter().layout_job(Text::Button.job(text, t.text));
    let width = ui.available_width().max(galley.size().x + 2.0 * ITEM_PAD);
    let (rect, r) = ui.allocate_exact_size(vec2(width, ITEM_H), sense);
    let fill = if selected {
        t.selection
    } else if r.hovered() {
        t.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if r.hovered() { egui::Stroke::new(1.0, t.ink) } else { egui::Stroke::NONE };
    // The stroke inside the rect: the item's size never changes.
    ui.painter().rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(pos2(rect.left() + ITEM_PAD, rect.center().y - galley.size().y / 2.0), galley, t.text);
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, text));
    r
}

fn filter_rows(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = theme::tokens(ui.ctx());
    let blank = || Filter { column: bar.columns.first().map_or_else(String::new, |(name, _)| (*name).to_string()), op: Op::Eq, value: String::new() };
    let rows: Vec<Filter> = if bar.filters.is_empty() { vec![blank()] } else { bar.filters.to_vec() };
    let available = ui.available_width();
    let (column_w, op_w) = choice_widths(available, op_fit(ui));
    // The value: as wide on every row, so the rows line up under the first one's buttons.
    let value_w = value_width(available, column_w, op_w);
    for (n, row_filter) in rows.iter().enumerate() {
        row(ui, available, |ui| {
            let row = row_filter;
            // A filter (not the bar's empty first row) left out of the condition: muted.
            let unapplied = !bar.filters.is_empty() && !row.restricts();
            let shown = |text: String| if unapplied { RichText::new(text).color(t.text_muted) } else { RichText::new(text) };
            if n == 0 {
                toggle(ui, bar, tags, runs);
            } else {
                let (rect, _) = ui.allocate_exact_size(vec2(SQUARE, ROW_H), Sense::hover());
                Text::Label.paint(ui.painter(), rect.center(), egui::Align2::CENTER_CENTER, "AND", t.text_muted);
            }
            let mut changed: Option<Filter> = None;
            // The column.
            let mut column = None;
            let combo = choice(ui, ("dbee-filter-column", bar.tab.0, n), column_w, 360.0, shown(row.column.clone()), |ui| {
                for (name, ty) in &bar.columns {
                    let r = item(ui, *name == row.column, name).on_hover_text(*ty);
                    tags.tag_value_as(&format!("filter-{n}-column-{name}"), &r, *ty, true);
                    if r.clicked() {
                        column = Some((*name).to_string());
                    }
                }
            });
            tags.tag_value(&format!("filter-{n}-column"), &combo.response, row.column.as_str());
            if let Some(c) = column.filter(|c| *c != row.column) {
                changed = Some(Filter { column: c, ..row.clone() });
            }
            // The operator.
            let mut op = None;
            let combo = choice(ui, ("dbee-filter-op", bar.tab.0, n), op_w, 400.0, shown(op_text(row.op)), |ui| {
                for o in Op::ALL {
                    let r = noted(item(ui, o == row.op, &op_text(o)), bar.dialect.op_note(o));
                    tags.tag_value_as(&format!("filter-{n}-op-{}", o.id()), &r, o.label(), true);
                    if r.clicked() {
                        op = Some(o);
                    }
                }
            });
            let r = noted(combo.response, bar.dialect.op_note(row.op));
            tags.tag_value(&format!("filter-{n}-op"), &r, row.op.id());
            if let Some(o) = op.filter(|o| *o != row.op) {
                // An operator without a value drops the one typed.
                let base = changed.clone().unwrap_or_else(|| row.clone());
                let value = if o.takes_value() { base.value.clone() } else { String::new() };
                changed = Some(Filter { op: o, value, ..base });
            }
            let mut enter = false;
            if row.op.takes_value() {
                let mut text = row.value.clone();
                let hint = match row.op {
                    _ if unapplied => UNAPPLIED_HINT,
                    Op::In => "Values, separated by commas",
                    Op::Like | Op::NotLike | Op::ILike | Op::NotILike => "A pattern: % for any text, _ for one character",
                    _ => "Value",
                };
                let r =
                    ui.add(field(TextEdit::singleline(&mut text).id(egui::Id::new(("dbee-filter-value", bar.tab.0, n))).hint_text(hint), value_w));
                let r = if unapplied { r.on_hover_text(UNAPPLIED_HOVER) } else { r.on_hover_text(enter_hover(bar)) };
                tags.tag_value(&format!("filter-{n}-value"), &r, text.as_str());
                if unapplied {
                    tags.tag_value(&format!("filter-{n}-unapplied"), &r, UNAPPLIED_HINT);
                }
                if r.changed() {
                    changed = Some(Filter { value: text, ..changed.clone().unwrap_or_else(|| row.clone()) });
                }
                enter = entered(ui, &r);
            } else {
                // No value to give: the field's room stays, so the rows keep lining up.
                ui.allocate_exact_size(vec2(value_w, ROW_H), Sense::hover());
            }
            // Remove, then (first row) add and apply.
            let removed = icon_button(ui, tags, &format!("filter-{n}-remove"), "Remove this filter", false, |p, r| icons::minus(p, r, t.text_muted));
            let added = n == 0 && icon_button(ui, tags, "filter-add", "Add a filter (AND)", false, |p, r| icons::plus(p, r, t.text_muted));
            if n == 0 {
                apply_button(ui, bar, tags, runs);
            }
            if changed.is_some() || removed || added {
                let mut next = rows.clone();
                if let Some(f) = changed {
                    if let Some(slot) = next.get_mut(n) {
                        *slot = f;
                    }
                    runs.push(set(&next));
                }
                if removed {
                    if n < next.len() {
                        next.remove(n);
                    }
                    runs.push(set(&next));
                } else if added {
                    next.push(blank());
                    runs.push(set(&next));
                }
            }
            if enter {
                runs.push(apply());
            }
        });
    }
}

/// `r` with `note` as its hover, when there is one.
fn noted(r: Response, note: Option<&str>) -> Response {
    match note {
        Some(note) => r.on_hover_text(note),
        None => r,
    }
}

/// An operator as its choice reads: its words, with its symbol in brackets when the symbol is not
/// words itself (`equals (=)`, but `like`, `is null`).
fn op_text(op: Op) -> String {
    if op.symbol().chars().any(char::is_alphabetic) { op.label().to_string() } else { format!("{} ({})", op.label(), op.symbol()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operators_read_as_words_with_the_symbol_in_brackets() {
        let texts: Vec<String> = Op::ALL.into_iter().map(op_text).collect();
        assert_eq!(
            texts,
            [
                "equals (=)",
                "does not equal (<>)",
                "like",
                "not like",
                "ilike",
                "not ilike",
                "less than (<)",
                "less than or equal (<=)",
                "greater than (>)",
                "greater than or equal (>=)",
                "in",
                "is null",
                "is not null",
            ]
        );
    }

    #[test]
    fn the_choices_give_way_in_a_narrow_tab() {
        // What the operator choice needs in egui's default font at 13 points: "greater than or
        // equal (>=)" is 148.6 wide.
        let fit = 178.0;
        // Wide: the operator whole, the column as wide as it gets, the value field the rest.
        assert_eq!(choice_widths(1000.0, fit), (COLUMN_W.1, fit));
        assert_eq!(value_width(1000.0, COLUMN_W.1, fit), 1000.0 - FIXED_W - COLUMN_W.1 - fit);
        // The default window with the value panel open (a row about 490 wide), and a little
        // narrower or wider: everything in a row fits, the operator whole.
        for available in [484.0, 491.0, 500.0, 560.0, 640.0] {
            let (column, op) = choice_widths(available, fit);
            let value = value_width(available, column, op);
            assert!(FIXED_W + column + op + value <= available + 0.5, "{available}: {column} + {op} + {value}");
            assert!(column >= COLUMN_W.0 && value >= VALUE_MIN_W, "{available}: {column}, {value}");
            assert_eq!(op, fit, "{available}: the operator reads whole");
        }
        // Narrower: the column gives way first, then the operator.
        let (column, op) = choice_widths(440.0, fit);
        assert!(column == COLUMN_W.0 && op < fit && op >= OP_MIN_W, "{column}, {op}");
        assert!(FIXED_W + column + op + VALUE_MIN_W <= 440.0 + 0.5);
        // Too narrow for anything: no narrower than readable.
        assert_eq!(choice_widths(100.0, fit), (COLUMN_W.0, OP_MIN_W));
    }

    #[test]
    fn changes_go_back_unapplied_and_apply_applies() {
        let f = vec![Filter { column: "id".into(), op: Op::Gt, value: "1".into() }];
        assert_eq!(set(&f), ("table.setFilters", json!({ "filters": [{ "column": "id", "op": "gt", "value": "1" }], "apply": false })));
        assert_eq!(apply(), ("table.setFilters", json!({ "apply": true })));
    }
}

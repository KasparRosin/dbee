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
//! The bar is the page's (`filter row`, [`table_bars::filter_bar`]): on `card`, a line under it,
//! each row 36 tall with its controls level, the shared controls 28 tall and 8 apart: the toggle,
//! `−` and `+` (`button/icon` boxed), the column and the operator (`dropdown`), the value
//! (`input`, data) and Apply (`button/box` outline); in the raw form the toggle, the field and
//! Apply. The operator choice is as wide as its longest label, so a chosen operator always reads
//! whole; in a narrow tab (a small window with the value panel open) the column choice narrows
//! first, then the operator's, their text cut short with `…` ([`choice_widths`]), so a row's
//! buttons stay in view. A choice's list draws the menu's items
//! ([`crate::components::menu::Item`]), the chosen one marked in the column on its left. Tab goes
//! through a row's controls in that order, then the next row's.
//!
//! Automation ids: `filter-<n>-column` (value: the column; while open its choices
//! `filter-<n>-column-<column>`), `filter-<n>-op` (value: the operator's id; its choices
//! `filter-<n>-op-<id>`), `filter-<n>-value`, `filter-<n>-unapplied` (on the value field of a row
//! left out; value: its hint), `filter-<n>-remove`, `filter-add`,
//! `filter-raw-toggle` (value: whether the raw field shows), `filter-raw`, `filter-apply`.

use dbee_engine::{Json, TabId};
use dbee_sql::Dialect;
use dbee_sql::filter::{Filter, Op};
use egui::{Key, Response, Sense, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::components::button::{BoxButton, IconButton};
use crate::components::dropdown::Dropdown;
use crate::components::icon::Icon;
use crate::components::input::Input;
use crate::components::menu::Item;
use crate::components::table_bars;
use crate::components::tooltip::Hint;
use crate::theme::size::{DROPDOWN_GAP, DROPDOWN_PAD_X, FILTER_GAP, ICON_BUTTON};
use crate::theme::{self, Text};

/// The square buttons: the toggle, `−` and `+`; and the `AND` before a row after the first.
const SQUARE: f32 = ICON_BUTTON;
/// The toggle (or `AND`) at a row's left, with the gap after it.
const LEAD_W: f32 = SQUARE + FILTER_GAP;
/// A row's width without its choices and value field, Apply being `apply_w` wide: the lead, `−`,
/// `+`, Apply and the gaps between them (`+` and Apply stand on the first row only; the others
/// keep their room, so the rows line up).
fn fixed_w(apply_w: f32) -> f32 {
    LEAD_W + 2.0 * SQUARE + apply_w + 5.0 * FILTER_GAP
}
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

/// The widths of the column and operator choices in a filter row `available` wide with Apply
/// `apply_w` wide, the operator choice needing `op_fit` to show its longest label whole
/// ([`op_fit`]). The operator choice gets its `op_fit` and the column choice up to its widest,
/// with the value field at its narrowest (the value field takes what is left). In a narrow tab (a
/// small window with the value panel open beside it) the column choice gives way first, down to
/// its narrowest, then the operator choice, down to its own, so the row's buttons stay in view.
pub fn choice_widths(available: f32, op_fit: f32, apply_w: f32) -> (f32, f32) {
    let room = (available - fixed_w(apply_w) - VALUE_MIN_W).max(0.0);
    let op = op_fit.min(room - COLUMN_W.0).max(OP_MIN_W);
    let column = (room - op).clamp(COLUMN_W.0, COLUMN_W.1);
    (column, op)
}

/// The width of a value field in a row `available` wide with choices `column` and `op` wide:
/// the rest, no narrower than [`VALUE_MIN_W`].
fn value_width(available: f32, column: f32, op: f32, apply_w: f32) -> f32 {
    (available - fixed_w(apply_w) - column - op).max(VALUE_MIN_W)
}

/// The width an operator choice needs to show its longest label whole, in the dropdown's font
/// with its padding and chevron.
fn op_fit(ui: &egui::Ui) -> f32 {
    let color = theme::tokens(ui.ctx()).text;
    let widest = Op::ALL.into_iter().map(|o| ui.painter().layout_job(Text::Body.job_as_written(&op_text(o), color)).size().x).fold(0.0, f32::max);
    // The dropdown's words and chevron keep 1 px of edge and its padding from either end; one
    // point more than the text, so rounding never cuts the widest label short.
    (widest + 2.0 * (DROPDOWN_PAD_X + 1.0) + DROPDOWN_GAP + Icon::ChevronDown.size().x + 1.0).ceil()
}

/// Apply as the bar draws it: outline, as the page.
fn apply_button() -> BoxButton<'static> {
    BoxButton::outline("Apply")
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

/// Draws the bar into `ui` and returns the commands its changes ask for, in order.
pub(crate) fn show(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags) -> Vec<Run> {
    let mut runs = Vec::new();
    let enabled = bar.disabled.is_none();
    table_bars::filter_bar(ui, |ui| {
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
    });
    runs
}

fn toggle(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let hover = if bar.raw_mode { "Back to filter rows" } else { "Write a raw WHERE condition" };
    let r = ui.add(IconButton::boxed(Icon::Code).on(bar.raw_mode)).hint(hover);
    tags.tag_value("filter-raw-toggle", &r, bar.raw_mode);
    if r.clicked() {
        runs.push(("table.setFilters", json!({ "rawMode": !bar.raw_mode, "apply": false })));
    }
}

fn apply_control(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let enabled = bar.blocked.is_none();
    let r = ui.add_enabled(enabled, apply_button());
    let r = match bar.blocked {
        Some(why) => r.hint_disabled(format!("Apply the filters: not now, {why}")),
        None => r.hint("Apply the filters (Enter)"),
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

fn raw_row(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let available = ui.available_width();
    let apply_w = apply_button().width(ui);
    table_bars::filter_row(ui, |ui| {
        toggle(ui, bar, tags, runs);
        let mut text = bar.raw.to_string();
        let width = (available - LEAD_W - apply_w - FILTER_GAP).max(VALUE_MIN_W);
        let r = ui.add(
            Input::data(&mut text)
                .id(egui::Id::new(("dbee-filter-raw", bar.tab.0)))
                .placeholder("A WHERE condition, e.g. total > 100 AND status = 'paid'")
                .desired_width(width),
        );
        let r = r.hint(enter_hover(bar));
        tags.tag_value("filter-raw", &r, text.as_str());
        if r.changed() {
            runs.push(("table.setFilters", json!({ "raw": text, "apply": false })));
        }
        if entered(ui, &r) {
            runs.push(apply());
        }
        apply_control(ui, bar, tags, runs);
    });
}

fn filter_rows(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let blank = || Filter { column: bar.columns.first().map_or_else(String::new, |(name, _)| (*name).to_string()), op: Op::Eq, value: String::new() };
    let rows: Vec<Filter> = if bar.filters.is_empty() { vec![blank()] } else { bar.filters.to_vec() };
    let available = ui.available_width();
    let apply_w = apply_button().width(ui);
    let (column_w, op_w) = choice_widths(available, op_fit(ui), apply_w);
    // The value: as wide on every row, so the rows line up under the first one's buttons.
    let value_w = value_width(available, column_w, op_w, apply_w);
    for (n, row_filter) in rows.iter().enumerate() {
        table_bars::filter_row(ui, |ui| {
            let row = row_filter;
            // A filter (not the bar's empty first row) left out of the condition: muted.
            let unapplied = !bar.filters.is_empty() && !row.restricts();
            if n == 0 {
                toggle(ui, bar, tags, runs);
            } else {
                table_bars::and_mark(ui);
            }
            let mut changed: Option<Filter> = None;
            // The column.
            let mut column = None;
            let dropdown = Dropdown::new(("dbee-filter-column", bar.tab.0, n), &row.column).width(column_w).list_height(360.0).muted(unapplied);
            let combo = dropdown.show(ui, |ui| {
                for (name, ty) in &bar.columns {
                    let r = ui.add(Item::new(name).choice(Some(*name == row.column))).hint(*ty);
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
            let op_words = op_text(row.op);
            let dropdown = Dropdown::new(("dbee-filter-op", bar.tab.0, n), &op_words).width(op_w).list_height(400.0).muted(unapplied);
            let combo = dropdown.show(ui, |ui| {
                for o in Op::ALL {
                    let r = noted(ui.add(Item::new(&op_text(o)).choice(Some(o == row.op))), bar.dialect.op_note(o));
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
                    ui.add(Input::data(&mut text).id(egui::Id::new(("dbee-filter-value", bar.tab.0, n))).placeholder(hint).desired_width(value_w));
                let r = if unapplied { r.hint(UNAPPLIED_HOVER) } else { r.hint(enter_hover(bar)) };
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
                ui.allocate_exact_size(vec2(value_w, SQUARE), Sense::hover());
            }
            // Remove, then (first row) add and apply.
            // − and + keep the value `false` their tags have always had.
            let r = ui.add(IconButton::boxed(Icon::Minus)).hint("Remove this filter");
            tags.tag_value(&format!("filter-{n}-remove"), &r, false);
            let removed = r.clicked();
            let added = n == 0 && {
                let r = ui.add(IconButton::boxed(Icon::Plus)).hint("Add a filter (AND)");
                tags.tag_value("filter-add", &r, false);
                r.clicked()
            };
            if n == 0 {
                apply_control(ui, bar, tags, runs);
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
        Some(note) => r.hint(note),
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
        // What the operator choice needs in the dropdown: "greater than or equal (>=)" in `body`
        // with the dropdown's padding and chevron; Apply as the page's outline button.
        let (fit, apply) = (190.0, 62.0);
        let fixed = fixed_w(apply);
        // Wide: the operator whole, the column as wide as it gets, the value field the rest.
        assert_eq!(choice_widths(1000.0, fit, apply), (COLUMN_W.1, fit));
        assert_eq!(value_width(1000.0, COLUMN_W.1, fit, apply), 1000.0 - fixed - COLUMN_W.1 - fit);
        // Room for every control at its narrowest with the operator whole, and a little wider:
        // everything in a row fits, the operator whole.
        let least = fixed + COLUMN_W.0 + fit + VALUE_MIN_W;
        for available in [least, least + 7.0, least + 16.0, least + 76.0, least + 156.0] {
            let (column, op) = choice_widths(available, fit, apply);
            let value = value_width(available, column, op, apply);
            assert!(fixed + column + op + value <= available + 0.5, "{available}: {column} + {op} + {value}");
            assert!(column >= COLUMN_W.0 && value >= VALUE_MIN_W, "{available}: {column}, {value}");
            assert_eq!(op, fit, "{available}: the operator reads whole");
        }
        // Narrower: the column gives way first, then the operator.
        let (column, op) = choice_widths(least - 40.0, fit, apply);
        assert!(column == COLUMN_W.0 && op < fit && op >= OP_MIN_W, "{column}, {op}");
        assert!(fixed + column + op + VALUE_MIN_W <= least - 40.0 + 0.5);
        // Too narrow for anything: no narrower than readable.
        assert_eq!(choice_widths(100.0, fit, apply), (COLUMN_W.0, OP_MIN_W));
    }

    #[test]
    fn changes_go_back_unapplied_and_apply_applies() {
        let f = vec![Filter { column: "id".into(), op: Op::Gt, value: "1".into() }];
        assert_eq!(set(&f), ("table.setFilters", json!({ "filters": [{ "column": "id", "op": "gt", "value": "1" }], "apply": false })));
        assert_eq!(apply(), ("table.setFilters", json!({ "apply": true })));
    }
}

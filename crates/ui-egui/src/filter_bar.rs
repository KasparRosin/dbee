//! The table tab's filter bar ([`show`]): rows of column ▾ · operator ▾ · value · `−`, joined
//! with AND, or one raw `WHERE` condition.
//!
//! The bar shows the tab's filters as the engine keeps them (`TableTab::filters`, `raw`,
//! `raw_mode`), and every change goes back at once with `table.setFilters {…, apply: false}`, so
//! `document.inspect` shows what is typed. With no filter yet the bar shows one empty row (the
//! first column, `equals`), which becomes a filter when it is changed. Apply, or Enter in a value
//! field, applies the bar (`table.setFilters {apply: true}`). The `<>` toggle switches between the
//! rows and the raw field without applying. The columns come from the schema; the operators are
//! the 13 of `dbee_sql::filter::Op`, each with its symbol; `is null` and `is not null` take no
//! value, so choosing one clears the value and hides its field. While changes are staged typing
//! goes on but Apply waits (its hover says why; Enter asks anyway, and the command says why); while
//! the connection is lost the bar is disabled. In a narrow tab (the value panel open in a small
//! window) the column and operator choices narrow ([`choice_widths`]), so a row's buttons stay in
//! view.
//!
//! Automation ids: `filter-<n>-column` (value: the column; while open its choices
//! `filter-<n>-column-<column>`), `filter-<n>-op` (value: the operator's id; its choices
//! `filter-<n>-op-<id>`), `filter-<n>-value`, `filter-<n>-remove`, `filter-add`,
//! `filter-raw-toggle` (value: whether the raw field shows), `filter-raw`, `filter-apply`.

use dbee_engine::{Json, TabId};
use dbee_sql::filter::{Filter, Op};
use egui::{Button, ComboBox, CornerRadius, Key, Rect, RichText, Sense, TextEdit, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::icons;
use crate::theme::{RADIUS, TOKENS};

/// A row's height.
const ROW_H: f32 = 28.0;
/// Room for the toggle (or `AND`) at a row's left.
const LEAD_W: f32 = 40.0;
/// The widest the column and operator choices get, and the narrowest.
const COLUMN_W: (f32, f32) = (90.0, 180.0);
const OP_W: (f32, f32) = (110.0, 190.0);
/// The narrowest a value field gets.
const VALUE_MIN_W: f32 = 80.0;

/// The widths of the column and operator choices in a filter row `available` wide, with
/// `spacing` between its parts: their widest when there is room, narrower in a narrow window (the
/// value panel open beside it), so the row's buttons stay in view.
pub fn choice_widths(available: f32, spacing: f32) -> (f32, f32) {
    // The lead, the value field at its narrowest, remove, add and Apply, and the gaps between.
    let fixed = LEAD_W + VALUE_MIN_W + 26.0 + 26.0 + 64.0 + 6.0 * spacing;
    let room = (available - fixed).max(0.0);
    let op = (room * 0.52).clamp(OP_W.0, OP_W.1);
    let column = (room - op).clamp(COLUMN_W.0, COLUMN_W.1);
    (column, op)
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
    let t = TOKENS;
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let fill = if selected {
        t.selection
    } else if r.hovered() && r.enabled() {
        t.hover
    } else {
        t.card
    };
    ui.painter().rect(rect, CornerRadius::same(RADIUS), fill, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
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
        ui.label(RichText::new(format!("The filters cannot change now: {why}.")).size(11.5).color(TOKENS.text_muted));
    }
    runs
}

fn toggle(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = TOKENS;
    let hover = if bar.raw_mode { "Back to filter rows" } else { "Write a raw WHERE condition" };
    if icon_button(ui, tags, "filter-raw-toggle", hover, bar.raw_mode, |p, r| icons::brackets(p, r, t.text_muted)) {
        runs.push(("table.setFilters", json!({ "rawMode": !bar.raw_mode, "apply": false })));
    }
    ui.add_space(LEAD_W - 26.0 - ui.spacing().item_spacing.x);
}

fn apply_button(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = TOKENS;
    let enabled = bar.blocked.is_none();
    let b = Button::new(RichText::new("Apply").color(t.accent_text)).fill(t.accent).min_size(vec2(64.0, 26.0));
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

fn raw_row(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        toggle(ui, bar, tags, runs);
        let mut text = bar.raw.to_string();
        let width = (ui.available_width() - 64.0 - 2.0 * ui.spacing().item_spacing.x).max(80.0);
        let r = ui.add(
            TextEdit::singleline(&mut text)
                .id(egui::Id::new(("dbee-filter-raw", bar.tab.0)))
                .hint_text("A WHERE condition, e.g. total > 100 AND status = 'paid'")
                .code_editor()
                .desired_width(width),
        );
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

fn filter_rows(ui: &mut egui::Ui, bar: &Bar<'_>, tags: &mut Tags, runs: &mut Vec<Run>) {
    let t = TOKENS;
    let blank = || Filter { column: bar.columns.first().map_or_else(String::new, |(name, _)| (*name).to_string()), op: Op::Eq, value: String::new() };
    let rows: Vec<Filter> = if bar.filters.is_empty() { vec![blank()] } else { bar.filters.to_vec() };
    let (column_w, op_w) = choice_widths(ui.available_width(), ui.spacing().item_spacing.x);
    for (n, row) in rows.iter().enumerate() {
        ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if n == 0 {
                toggle(ui, bar, tags, runs);
            } else {
                let (rect, _) = ui.allocate_exact_size(vec2(LEAD_W - ui.spacing().item_spacing.x, ROW_H), Sense::hover());
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "AND", egui::FontId::proportional(11.5), t.text_muted);
            }
            let mut changed: Option<Filter> = None;
            // The column.
            let mut column = None;
            let combo = ComboBox::from_id_salt(("dbee-filter-column", bar.tab.0, n))
                .selected_text(row.column.as_str())
                .width(column_w)
                .height(360.0)
                .show_ui(ui, |ui| {
                    for (name, ty) in &bar.columns {
                        let r = ui.selectable_label(*name == row.column, *name).on_hover_text(*ty);
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
            let combo =
                ComboBox::from_id_salt(("dbee-filter-op", bar.tab.0, n)).selected_text(op_text(row.op)).width(op_w).height(400.0).show_ui(ui, |ui| {
                    for o in Op::ALL {
                        let r = ui.selectable_label(o == row.op, op_text(o));
                        tags.tag_value_as(&format!("filter-{n}-op-{}", o.id()), &r, o.label(), true);
                        if r.clicked() {
                            op = Some(o);
                        }
                    }
                });
            tags.tag_value(&format!("filter-{n}-op"), &combo.response, row.op.id());
            if let Some(o) = op.filter(|o| *o != row.op) {
                // An operator without a value drops the one typed.
                let base = changed.clone().unwrap_or_else(|| row.clone());
                let value = if o.takes_value() { base.value.clone() } else { String::new() };
                changed = Some(Filter { op: o, value, ..base });
            }
            // The value: as wide on every row, so the rows line up under the first one's buttons.
            let width = (ui.available_width() - (26.0 + 26.0 + 64.0) - 4.0 * ui.spacing().item_spacing.x).max(VALUE_MIN_W);
            let mut enter = false;
            if row.op.takes_value() {
                let mut text = row.value.clone();
                let hint = match row.op {
                    Op::In => "Values, separated by commas",
                    Op::Like | Op::NotLike | Op::ILike | Op::NotILike => "A pattern: % for any text, _ for one character",
                    _ => "Value",
                };
                let r = ui
                    .add(TextEdit::singleline(&mut text).id(egui::Id::new(("dbee-filter-value", bar.tab.0, n))).hint_text(hint).desired_width(width));
                let r = r.on_hover_text(enter_hover(bar));
                tags.tag_value(&format!("filter-{n}-value"), &r, text.as_str());
                if r.changed() {
                    changed = Some(Filter { value: text, ..changed.clone().unwrap_or_else(|| row.clone()) });
                }
                enter = entered(ui, &r);
            } else {
                // No value to give: the field's room stays, so the rows keep lining up.
                ui.allocate_exact_size(vec2(width, ROW_H - 6.0), Sense::hover());
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

/// An operator as its choice reads: its label and its symbol.
fn op_text(op: Op) -> String {
    format!("{}   {}", op.label(), op.symbol())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operator_reads_with_its_symbol() {
        let texts: Vec<String> = Op::ALL.into_iter().map(op_text).collect();
        assert_eq!(texts.len(), 13);
        assert_eq!(texts[0], "equals   =");
        assert_eq!(texts[12], "is not null   IS NOT NULL");
    }

    #[test]
    fn the_choices_give_way_in_a_narrow_tab() {
        // Wide: as wide as they get.
        assert_eq!(choice_widths(1000.0, 8.0), (COLUMN_W.1, OP_W.1));
        // The default window with the value panel open: everything in a row fits.
        let fixed = LEAD_W + VALUE_MIN_W + 26.0 + 26.0 + 64.0 + 6.0 * 8.0;
        for available in [484.0, 500.0, 560.0, 640.0] {
            let (column, op) = choice_widths(available, 8.0);
            assert!(fixed + column + op <= available + 0.5, "{available}: {column} + {op}");
            assert!(column >= COLUMN_W.0 && op >= OP_W.0);
        }
        // Too narrow for anything: no narrower than readable.
        assert_eq!(choice_widths(100.0, 8.0), (COLUMN_W.0, OP_W.0));
    }

    #[test]
    fn changes_go_back_unapplied_and_apply_applies() {
        let f = vec![Filter { column: "id".into(), op: Op::Gt, value: "1".into() }];
        assert_eq!(set(&f), ("table.setFilters", json!({ "filters": [{ "column": "id", "op": "gt", "value": "1" }], "apply": false })));
        assert_eq!(apply(), ("table.setFilters", json!({ "apply": true })));
    }
}

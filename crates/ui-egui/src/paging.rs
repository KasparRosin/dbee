//! The table tab's paging bar ([`show`]), along its bottom: first · prev · the page · next · last,
//! the row count, refresh, a note when the page was cut by the size limit, then at the right
//! "+ Row" and the staged changes with Reset and Apply.
//!
//! The row count ([`count_text`]) shows the planner's estimate as `~100,000 rows` while that is
//! all there is: the engine's, or the catalog's for an unfiltered table, materialized view or
//! partitioned table. Views and foreign tables have none, and neither does a table the planner
//! puts at 0, so it never reads `~0`. A click counts the rows exactly (`table.count`), and the
//! count then reads `100,000 rows`. With an exact count the page reads `Page 3 of 10`.
//!
//! Paging and refresh wait while changes are staged (the engine's rule) and while the connection
//! is lost; the buttons say why when hovered. "+ Row" (`row.add`) is enabled as the engine's
//! edit commands are, and says why not when hovered.
//!
//! Automation ids: `page-first`, `page-prev`, `page-number` (value: `Page 3` or `Page 3 of 10`),
//! `page-next`, `page-last`, `page-count` (value: what it shows), `table-refresh`, `page-cut`
//! (value: the note), `row-add`, `edits-count` (value: `2 changes`), `edit-reset`, `edit-apply`.

use dbee_engine::{Json, Total};
use dbee_sql::select::PAGE_SIZE;
use egui::{Align, Button, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::icons::{self, Step};
use crate::theme::{RADIUS, TOKENS};
use crate::tree::thousands;

/// The bar's height.
pub const BAR_H: f32 = 36.0;

/// What the bar shows.
#[derive(Debug, Default)]
pub(crate) struct Paging {
    /// The page shown, 0-based.
    pub page: u64,
    /// The page shown is the one asked for and nothing follows it.
    pub at_end: bool,
    pub total: Total,
    /// The catalog's estimate for the table, when its rows are not filtered.
    pub estimate: Option<u64>,
    /// Rows the page shows, and whether that is all of them ([`cut_note`]).
    pub rows: usize,
    pub complete: bool,
    pub loading: bool,
    /// Staged changes.
    pub edits: usize,
    /// Why paging, sorting and refresh cannot run now (staged changes, a lost connection).
    pub blocked: Option<&'static str>,
    /// Why counting cannot run now.
    pub count_blocked: Option<&'static str>,
    /// Why Apply cannot run now.
    pub apply_blocked: Option<&'static str>,
    /// Why no row can be added now (the engine's reason).
    pub add_blocked: Option<String>,
}

/// A command the bar asks for.
pub(crate) type Run = (&'static str, Json);

fn rows_word(n: u64) -> String {
    if n == 1 { "1 row".into() } else { format!("{} rows", thousands(n)) }
}

/// What the row count shows: the exact count, the estimate as `~n rows` (never `~0`), or
/// `Count rows` when there is neither.
pub fn count_text(total: Total, estimate: Option<u64>) -> String {
    let approx = |n: Option<u64>| n.filter(|n| *n > 0).map(|n| format!("~{}", rows_word(n)));
    match total {
        Total::Exact(n) => rows_word(n),
        Total::Estimate(n) => approx(Some(n)).unwrap_or_else(|| "Count rows".into()),
        Total::Counting { estimate: e } => approx(e).unwrap_or_else(|| "Counting…".into()),
        Total::Unknown => approx(estimate).unwrap_or_else(|| "Count rows".into()),
    }
}

/// `Page 3`, or `Page 3 of 10` with an exact count.
pub fn page_text(page: u64, total: Total) -> String {
    let shown = thousands(page.saturating_add(1));
    match total {
        Total::Exact(n) => format!("Page {shown} of {}", thousands(n.saturating_sub(1) / PAGE_SIZE + 1)),
        _ => format!("Page {shown}"),
    }
}

/// The note for a page the size limit cut short, or `None` for a whole page.
pub fn cut_note(rows: usize, complete: bool) -> Option<String> {
    (!complete).then(|| format!("Only the first {} of this page are shown: its values passed the size limit.", rows_word(rows as u64)))
}

/// `1 change`, `3 changes`.
pub fn edits_text(n: usize) -> String {
    if n == 1 { "1 change".into() } else { format!("{n} changes") }
}

/// A square button with a paging arrow; `true` when clicked.
fn step_button(ui: &mut egui::Ui, tags: &mut Tags, id: &str, step: Step, why_not: Option<&str>, hover: &str) -> bool {
    let t = TOKENS;
    let enabled = why_not.is_none();
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && r.hovered() { t.hover } else { t.card };
    ui.painter().rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(1.0, t.border), StrokeKind::Inside);
    icons::step(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), if enabled { t.text } else { t.border }, step);
    let r = match why_not {
        Some(why) => r.on_hover_text(format!("{hover}: not now, {why}")),
        None => r.on_hover_text(hover),
    };
    tags.tag_value_as(id, &r, hover, enabled);
    enabled && r.clicked()
}

/// Draws the bar into `ui` and returns the commands it asks for.
pub(crate) fn show(ui: &mut egui::Ui, p: &Paging, tags: &mut Tags) -> Vec<Run> {
    let t = TOKENS;
    let mut runs: Vec<Run> = Vec::new();
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.panel);
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.border));
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    let ui = &mut bar;
    ui.spacing_mut().item_spacing.x = 4.0;
    let first = p.page == 0;
    let back = p.blocked.or(if first { Some("this is the first page") } else { None });
    let on = p.blocked.or(if p.at_end { Some("this is the last page") } else { None });
    if step_button(ui, tags, "page-first", Step::First, back, "First page") {
        runs.push(("table.page", json!({ "to": "first" })));
    }
    if step_button(ui, tags, "page-prev", Step::Prev, back, "Previous page") {
        runs.push(("table.page", json!({ "to": "prev" })));
    }
    let label = page_text(p.page, p.total);
    let r = ui.add(egui::Label::new(RichText::new(label.as_str()).size(12.5)).selectable(false));
    tags.tag_value("page-number", &r, label.as_str());
    if p.loading {
        ui.add(egui::Spinner::new().size(12.0).color(t.text_muted));
    }
    if step_button(ui, tags, "page-next", Step::Next, on, "Next page") {
        runs.push(("table.page", json!({ "to": "next" })));
    }
    if step_button(ui, tags, "page-last", Step::Last, on, "Last page (counts the rows)") {
        runs.push(("table.page", json!({ "to": "last" })));
    }
    ui.add_space(10.0);
    let counting = matches!(p.total, Total::Counting { .. });
    let count = count_text(p.total, p.estimate);
    let enabled = p.count_blocked.is_none() && !counting;
    let r = ui.add_enabled(enabled, Button::new(RichText::new(count.as_str()).size(12.5)).min_size(vec2(0.0, 26.0)));
    let r = match p.count_blocked {
        Some(why) => r.on_disabled_hover_text(format!("Cannot count now: {why}")),
        None => r.on_hover_text("Count the rows exactly"),
    };
    tags.tag_value_as("page-count", &r, count.as_str(), enabled);
    if r.clicked() {
        runs.push(("table.count", json!({})));
    }
    if counting {
        ui.add(egui::Spinner::new().size(12.0).color(t.text_muted));
    }
    let refresh_enabled = p.blocked.is_none();
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), if refresh_enabled { Sense::click() } else { Sense::hover() });
    let fill = if refresh_enabled && r.hovered() { t.hover } else { t.card };
    ui.painter().rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(1.0, t.border), StrokeKind::Inside);
    icons::refresh(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), if refresh_enabled { t.text } else { t.border });
    let r = match p.blocked {
        Some(why) => r.on_hover_text(format!("Refresh: not now, {why}")),
        None => r.on_hover_text("Load the page again"),
    };
    tags.tag_value_as("table-refresh", &r, "Refresh", refresh_enabled);
    if refresh_enabled && r.clicked() {
        runs.push(("table.refresh", json!({})));
    }
    if let Some(note) = cut_note(p.rows, p.complete) {
        ui.add_space(10.0);
        let r = ui.add(egui::Label::new(RichText::new(note.as_str()).size(12.0).color(t.text_muted)).truncate());
        tags.tag_value("page-cut", &r, note.as_str());
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if p.edits > 0 {
            let apply_enabled = p.apply_blocked.is_none();
            let b = Button::new(RichText::new("Apply").color(t.accent_text)).fill(t.accent).min_size(vec2(64.0, 26.0));
            let r = ui.add_enabled(apply_enabled, b);
            let r = match p.apply_blocked {
                Some(why) => r.on_disabled_hover_text(format!("Cannot apply now: {why}")),
                None => r.on_hover_text("Write the changes in one transaction (Ctrl+S)"),
            };
            tags.tag_value_as("edit-apply", &r, "Apply", apply_enabled);
            if r.clicked() {
                runs.push(("edit.apply", json!({})));
            }
            let r = ui.add(Button::new("Reset").min_size(vec2(0.0, 26.0))).on_hover_text("Drop the staged changes");
            tags.tag_value("edit-reset", &r, "Reset");
            if r.clicked() {
                runs.push(("edit.reset", json!({})));
            }
            let text = edits_text(p.edits);
            let r = ui.label(RichText::new(text.as_str()).size(12.5).color(t.text_muted));
            tags.tag_value("edits-count", &r, text.as_str());
            ui.add_space(10.0);
        }
        let enabled = p.add_blocked.is_none();
        let r = ui.add_enabled(enabled, Button::new(RichText::new("+ Row").size(12.5)).min_size(vec2(0.0, 26.0)));
        let r = match &p.add_blocked {
            Some(why) => r.on_disabled_hover_text(format!("Cannot add a row now: {why}")),
            None => r.on_hover_text("Add a row, written with the other changes on Apply"),
        };
        tags.tag_value_as("row-add", &r, "+ Row", enabled);
        if r.clicked() {
            runs.push(("row.add", json!({})));
        }
    });
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_estimates_or_numbers_and_never_as_zero_estimates() {
        assert_eq!(count_text(Total::Unknown, Some(100_000)), "~100,000 rows");
        assert_eq!(count_text(Total::Unknown, None), "Count rows", "a view has no estimate");
        assert_eq!(count_text(Total::Unknown, Some(0)), "Count rows", "never ~0");
        assert_eq!(count_text(Total::Estimate(0), None), "Count rows");
        assert_eq!(count_text(Total::Estimate(1), None), "~1 row");
        assert_eq!(count_text(Total::Counting { estimate: Some(5_000) }, None), "~5,000 rows");
        assert_eq!(count_text(Total::Counting { estimate: None }, Some(7)), "Counting…");
        assert_eq!(count_text(Total::Exact(1_000_000), Some(5)), "1,000,000 rows");
        assert_eq!(count_text(Total::Exact(0), None), "0 rows");
    }

    #[test]
    fn pages_read_from_one_with_the_last_once_counted() {
        assert_eq!(page_text(0, Total::Unknown), "Page 1");
        assert_eq!(page_text(9_999, Total::Exact(1_000_000)), "Page 10,000 of 10,000");
        assert_eq!(page_text(0, Total::Exact(0)), "Page 1 of 1");
        assert_eq!(page_text(1, Total::Exact(101)), "Page 2 of 2");
        assert_eq!(page_text(u64::MAX, Total::Exact(u64::MAX)), format!("Page {} of {}", thousands(u64::MAX), thousands((u64::MAX - 1) / 100 + 1)));
    }

    #[test]
    fn a_cut_page_says_how_many_rows_show() {
        assert_eq!(cut_note(100, true), None);
        let note = cut_note(13, false).unwrap();
        assert!(note.contains("first 13 rows"), "{note}");
        assert!(cut_note(1, false).unwrap().contains("first 1 row "));
        assert_eq!((edits_text(1), edits_text(2)), ("1 change".to_string(), "2 changes".to_string()));
    }
}

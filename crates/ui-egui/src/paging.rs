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
//! **Room.** No control is drawn over another, at any width ([`show`], [`fit`]). Short of room,
//! the row count becomes an icon button first (a number sign, its text on hover), then the
//! "n changes" text goes (Apply's hover says how many). At the app's default window (1100 × 720)
//! with the value panel open and a change staged, the arrows, refresh, "+ Row", Reset and Apply
//! all show whole. Narrower than both groups at their least, the right group follows the left
//! one and is cut at the bar's edge.
//!
//! Automation ids: `page-first`, `page-prev`, `page-number` (value: `Page 3` or `Page 3 of 10`),
//! `page-next`, `page-last`, `page-count` (value: the count's text, also while it shows as an
//! icon), `table-refresh`, `page-cut` (value: the note; not drawn without room for it),
//! `row-add`, `edits-count` (value: `2 changes`; not drawn while short of room), `edit-reset`,
//! `edit-apply`.

use dbee_engine::{Json, Total};
use dbee_sql::select::PAGE_SIZE;
use egui::{Align, Button, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::automation::Tags;
use crate::components::icon::Icon;
use crate::theme::{
    self, Text,
    size::{self, RADIUS_CONTROL, SPINNER},
};
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
fn step_button(ui: &mut egui::Ui, tags: &mut Tags, id: &str, step: Icon, why_not: Option<&str>, hover: &str) -> bool {
    let enabled = why_not.is_none();
    let r = square_button(ui, enabled, |p, rect, colour| step.paint(p, rect.center(), colour));
    let r = match why_not {
        Some(why) => r.on_hover_text(format!("{hover}: not now, {why}")),
        None => r.on_hover_text(hover),
    };
    tags.tag_value_as(id, &r, hover, enabled);
    enabled && r.clicked()
}

/// Between two controls.
const SPACING: f32 = 4.0;
/// Before the row count, and between "+ Row" and the staged changes.
const SPACE: f32 = 10.0;
/// Between the bar's left group and its right one.
const GAP: f32 = 12.0;
/// A square button's side.
const SQUARE: f32 = 26.0;
/// Apply's least width.
const APPLY_W: f32 = 64.0;
/// The narrowest a cut page's note is shown in (it truncates, with its whole text on hover).
const MIN_NOTE: f32 = 24.0;

/// The width the bar's two groups take in each of their forms, measured before they are drawn
/// ([`widths`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Widths {
    /// The left group (paging, the row count, refresh) with the count as text, and as an icon.
    pub left_text: f32,
    pub left_icon: f32,
    /// The right group ("+ Row", and with staged changes their text, Reset and Apply) with the
    /// changes' text, and without it.
    pub right_text: f32,
    pub right_bare: f32,
}

/// How the bar fits its width ([`fit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    /// The row count shows as an icon, its text on hover.
    pub count_icon: bool,
    /// The "n changes" text shows (else Apply's hover says it).
    pub edits_text: bool,
}

/// The first form of the bar that fits `room`: everything; the count as an icon; that and no
/// changes' text. When even the last does not fit (or `room` is not a number) it is the last,
/// and the right group is cut at the bar's edge.
pub(crate) fn fit(room: f32, w: Widths) -> Fit {
    let fits = |left: f32, right: f32| left + GAP + right <= room;
    if fits(w.left_text, w.right_text) {
        Fit { count_icon: false, edits_text: true }
    } else if fits(w.left_icon, w.right_text) {
        Fit { count_icon: true, edits_text: true }
    } else {
        Fit { count_icon: true, edits_text: false }
    }
}

/// `widths` side by side, [`SPACING`] apart.
fn row(widths: &[f32]) -> f32 {
    widths.iter().sum::<f32>() + SPACING * widths.len().saturating_sub(1) as f32
}

/// The width of `text` drawn in `style`.
fn text_width(ui: &egui::Ui, text: &str, style: Text) -> f32 {
    ui.painter().layout_job(style.job(text, egui::Color32::PLACEHOLDER)).size().x.ceil()
}

/// A text button's width: its text with the button's padding on both sides, rounded up. The
/// bar draws its buttons at least this wide (`min_size`), so its sums are what they take.
fn button_width(ui: &egui::Ui, text: &str, style: Text) -> f32 {
    (text_width(ui, text, style) + 2.0 * ui.spacing().button_padding.x + 1.0).ceil()
}

/// The texts the bar shows, and the widths of its buttons.
struct Parts {
    page: String,
    count: String,
    count_w: f32,
    edits: String,
    add_w: f32,
    reset_w: f32,
    apply_w: f32,
}

impl Parts {
    fn of(ui: &egui::Ui, p: &Paging) -> Parts {
        let count = count_text(p.total, p.estimate);
        Parts {
            page: page_text(p.page, p.total),
            count_w: button_width(ui, &count, Text::Label),
            count,
            edits: edits_text(p.edits),
            add_w: button_width(ui, "+ Row", Text::Label),
            reset_w: button_width(ui, "Reset", Text::Button),
            apply_w: button_width(ui, "Apply", Text::Button).max(APPLY_W),
        }
    }
}

/// What each group takes, in each form; the same parts, spaces and spinners [`show`] draws.
fn widths(ui: &egui::Ui, p: &Paging, parts: &Parts) -> Widths {
    let mut paging = vec![SQUARE, SQUARE, text_width(ui, &parts.page, Text::Label)];
    if p.loading {
        paging.push(SPINNER);
    }
    paging.extend([SQUARE, SQUARE]);
    let counting = matches!(p.total, Total::Counting { .. });
    let left = |count: f32| {
        let mut tail = vec![count];
        if counting {
            tail.push(SPINNER);
        }
        tail.push(SQUARE);
        row(&paging) + SPACING + SPACE + row(&tail)
    };
    let (right_text, right_bare) = if p.edits > 0 {
        let text = text_width(ui, &parts.edits, Text::Label);
        let start = parts.add_w + SPACING + SPACE;
        (start + row(&[text, parts.reset_w, parts.apply_w]), start + row(&[parts.reset_w, parts.apply_w]))
    } else {
        (parts.add_w, parts.add_w)
    };
    Widths { left_text: left(parts.count_w), left_icon: left(SQUARE), right_text, right_bare }
}

/// A number sign: the row count as an icon. A rect that is empty or not finite draws nothing.
fn count_icon(p: &egui::Painter, rect: Rect, color: egui::Color32) {
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let side = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(side, side));
    let s = Stroke::new((side / 10.0).clamp(1.0, 2.5), color);
    let at = |x: f32, y: f32| r.min + vec2(r.width() * x, r.height() * y);
    p.line_segment([at(0.44, 0.16), at(0.34, 0.84)], s);
    p.line_segment([at(0.68, 0.16), at(0.58, 0.84)], s);
    p.line_segment([at(0.18, 0.38), at(0.84, 0.38)], s);
    p.line_segment([at(0.16, 0.62), at(0.82, 0.62)], s);
}

/// A square button framed like the paging arrows, drawn with `icon`.
fn square_button(ui: &mut egui::Ui, enabled: bool, icon: impl FnOnce(&egui::Painter, Rect, egui::Color32)) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(SQUARE, SQUARE), if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && r.hovered() { t.hover } else { t.card };
    ui.painter().rect(rect, CornerRadius::same(RADIUS_CONTROL), fill, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
    icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), if enabled { t.text } else { t.text_faint });
    r
}

/// Draws the bar into `ui` and returns the commands it asks for.
///
/// The left group (paging, the row count, refresh) runs from the bar's left end and the right
/// group ("+ Row", the staged changes, Reset, Apply) ends at its right end, [`GAP`] apart at
/// least. Short of room ([`fit`]), the row count turns into an icon button first (its text on
/// hover), then the "n changes" text goes (Apply's hover says it). Narrower still, the right
/// group starts after the left one and is cut at the bar's edge. So no control is ever drawn over
/// another, and nothing is drawn past the bar.
pub(crate) fn show(ui: &mut egui::Ui, p: &Paging, tags: &mut Tags) -> Vec<Run> {
    let t = theme::tokens(ui.ctx());
    let mut runs: Vec<Run> = Vec::new();
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg);
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.line));
    let inner = rect.shrink2(vec2(8.0, 0.0));
    let clip = ui.clip_rect().intersect(inner);
    let child = |ui: &mut egui::Ui, salt: &str, max: Rect, clip: Rect| {
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(salt).max_rect(max).layout(Layout::left_to_right(Align::Center)));
        c.set_clip_rect(clip);
        c.spacing_mut().item_spacing.x = SPACING;
        c
    };
    let mut bar = child(ui, "dbee-paging-left", inner, clip);
    let parts = Parts::of(&bar, p);
    let w = widths(&bar, p, &parts);
    let fit = fit(inner.width(), w);
    left_group(&mut bar, p, tags, &parts, fit, &mut runs);
    let left_end = bar.min_rect().right();
    let right_w = if fit.edits_text { w.right_text } else { w.right_bare };
    let start = (inner.right() - right_w).max(left_end + GAP);
    // The right group's child comes before the note's, which is not always drawn: a child's
    // widget ids follow from the children made before it, so the buttons keep theirs.
    let lane = Rect::from_min_size(pos2(start, inner.top()), vec2(right_w.max(0.0), inner.height()));
    let cut = clip.intersect(Rect::from_min_max(pos2(start, inner.top()), inner.max));
    let mut right = child(ui, "dbee-paging-right", lane, cut);
    right_group(&mut right, p, tags, &parts, fit, &mut runs);
    if let Some(note) = cut_note(p.rows, p.complete) {
        let room = Rect::from_min_max(pos2(left_end + SPACE, inner.top()), pos2(start - GAP, inner.bottom()));
        if room.width() >= MIN_NOTE {
            let mut n = child(ui, "dbee-paging-note", room, clip.intersect(room));
            let r = n.add(egui::Label::new(Text::Small.rich(note.as_str()).color(t.text_muted)).truncate());
            tags.tag_value("page-cut", &r, note.as_str());
        }
    }
    runs
}

/// First · prev · the page · next · last, the row count and refresh.
fn left_group(ui: &mut egui::Ui, p: &Paging, tags: &mut Tags, parts: &Parts, fit: Fit, runs: &mut Vec<Run>) {
    let t = theme::tokens(ui.ctx());
    let first = p.page == 0;
    let back = p.blocked.or(if first { Some("this is the first page") } else { None });
    let on = p.blocked.or(if p.at_end { Some("this is the last page") } else { None });
    if step_button(ui, tags, "page-first", Icon::First, back, "First page") {
        runs.push(("table.page", json!({ "to": "first" })));
    }
    if step_button(ui, tags, "page-prev", Icon::Previous, back, "Previous page") {
        runs.push(("table.page", json!({ "to": "prev" })));
    }
    let r = ui.add(egui::Label::new(Text::Label.rich(parts.page.as_str()).color(t.text_muted)).selectable(false));
    tags.tag_value("page-number", &r, parts.page.as_str());
    if p.loading {
        ui.add(egui::Spinner::new().size(size::SPINNER).color(t.text_muted));
    }
    if step_button(ui, tags, "page-next", Icon::Next, on, "Next page") {
        runs.push(("table.page", json!({ "to": "next" })));
    }
    if step_button(ui, tags, "page-last", Icon::Last, on, "Last page (counts the rows)") {
        runs.push(("table.page", json!({ "to": "last" })));
    }
    ui.add_space(SPACE);
    let counting = matches!(p.total, Total::Counting { .. });
    let count = parts.count.as_str();
    let enabled = p.count_blocked.is_none() && !counting;
    let r = if fit.count_icon {
        let r = square_button(ui, enabled, count_icon);
        match (p.count_blocked, counting) {
            (Some(why), _) => r.on_hover_text(format!("{count}. Can't count now: {why}.")),
            (None, true) => r.on_hover_text(format!("{count}: counting the rows")),
            (None, false) => r.on_hover_text(format!("{count}: count the rows exactly")),
        }
    } else {
        let r = ui.add_enabled(enabled, Button::new(Text::Label.rich(count)).min_size(vec2(parts.count_w, SQUARE)));
        match p.count_blocked {
            Some(why) => r.on_disabled_hover_text(format!("Can't count now: {why}.")),
            None => r.on_hover_text("Count the rows exactly"),
        }
    };
    tags.tag_value_as("page-count", &r, count, enabled);
    if enabled && r.clicked() {
        runs.push(("table.count", json!({})));
    }
    if counting {
        ui.add(egui::Spinner::new().size(size::SPINNER).color(t.text_muted));
    }
    let refresh_enabled = p.blocked.is_none();
    let r = square_button(ui, refresh_enabled, |p, rect, colour| Icon::Refresh.paint(p, rect.center(), colour));
    let r = match p.blocked {
        Some(why) => r.on_hover_text(format!("Refresh: not now, {why}")),
        None => r.on_hover_text("Load the page again"),
    };
    tags.tag_value_as("table-refresh", &r, "Refresh", refresh_enabled);
    if refresh_enabled && r.clicked() {
        runs.push(("table.refresh", json!({})));
    }
}

/// "+ Row", then with staged changes their count, Reset and Apply.
fn right_group(ui: &mut egui::Ui, p: &Paging, tags: &mut Tags, parts: &Parts, fit: Fit, runs: &mut Vec<Run>) {
    let t = theme::tokens(ui.ctx());
    let enabled = p.add_blocked.is_none();
    let r = ui.add_enabled(enabled, Button::new(Text::Label.rich("+ Row")).min_size(vec2(parts.add_w, SQUARE)));
    let r = match &p.add_blocked {
        Some(why) => r.on_disabled_hover_text(format!("Cannot add a row now: {why}")),
        None => r.on_hover_text("Add a row, written with the other changes on Apply"),
    };
    tags.tag_value_as("row-add", &r, "+ Row", enabled);
    if r.clicked() {
        runs.push(("row.add", json!({})));
    }
    if p.edits == 0 {
        return;
    }
    ui.add_space(SPACE);
    if fit.edits_text {
        let r = ui.label(Text::Label.rich(parts.edits.as_str()).color(t.text_muted));
        tags.tag_value("edits-count", &r, parts.edits.as_str());
    } else {
        // The label's one widget id, so Reset and Apply keep theirs when it is left out.
        ui.skip_ahead_auto_ids(1);
    }
    let r = ui.add(Button::new("Reset").min_size(vec2(parts.reset_w, SQUARE))).on_hover_text("Drop the staged changes");
    tags.tag_value("edit-reset", &r, "Reset");
    if r.clicked() {
        runs.push(("edit.reset", json!({})));
    }
    let apply_enabled = p.apply_blocked.is_none();
    let b = Button::new(RichText::new("Apply").color(t.on_ink)).fill(t.ink).min_size(vec2(parts.apply_w, SQUARE));
    let r = ui.add_enabled(apply_enabled, b);
    let r = match p.apply_blocked {
        Some(why) => r.on_disabled_hover_text(format!("Cannot apply {} now: {why}", parts.edits)),
        None => r.on_hover_text(format!("Write {} in one transaction (Ctrl+S)", parts.edits)),
    };
    tags.tag_value_as("edit-apply", &r, "Apply", apply_enabled);
    if r.clicked() {
        runs.push(("edit.apply", json!({})));
    }
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
    fn short_of_room_the_count_goes_to_an_icon_first_then_the_changes_text() {
        let w = Widths { left_text: 300.0, left_icon: 230.0, right_text: 260.0, right_bare: 200.0 };
        let all = Fit { count_icon: false, edits_text: true };
        let icon = Fit { count_icon: true, edits_text: true };
        let bare = Fit { count_icon: true, edits_text: false };
        assert_eq!(fit(300.0 + GAP + 260.0, w), all);
        assert_eq!(fit(300.0 + GAP + 260.0 - 1.0, w), icon);
        assert_eq!(fit(230.0 + GAP + 260.0, w), icon);
        // The count's text and no changes' text would fit here too: the count goes first.
        assert_eq!(fit(300.0 + GAP + 200.0, w), icon, "{}", 300.0 + GAP + 200.0);
        assert_eq!(fit(230.0 + GAP + 260.0 - 1.0, w), bare);
        assert_eq!(fit(230.0 + GAP + 200.0, w), bare);
        // Below the minimum, the least: the right group is then cut at the bar's edge.
        for room in [0.0, -5.0, f32::NAN] {
            assert_eq!(fit(room, w), bare, "{room}");
        }
    }

    /// What every bar of `width` × [`BAR_H`] draws of `p`: each widget's rect a click can reach
    /// (`None` when it has none), and the bar's own rect.
    fn drawn(p: Paging, width: f32) -> (Vec<(&'static str, Option<Rect>)>, Rect) {
        struct State {
            p: Paging,
            tags: Tags,
            bar: Rect,
        }
        let state = State { p, tags: Tags::default(), bar: Rect::NOTHING };
        let mut h = egui_kittest::Harness::builder().with_size(vec2(width, BAR_H)).build_ui_state(
            |ui, s: &mut State| {
                if !crate::theme::applied(ui.ctx()) {
                    return;
                }
                s.tags.begin_frame();
                s.bar = ui.max_rect();
                let _ = show(ui, &s.p, &mut s.tags);
            },
            state,
        );
        h.run_steps(2);
        let s = h.state();
        (IDS.iter().map(|&id| (id, s.tags.rect(id))).collect(), s.bar)
    }

    /// The bar's automation ids, in their order along it.
    const IDS: [&str; 12] = [
        "page-first",
        "page-prev",
        "page-number",
        "page-next",
        "page-last",
        "page-count",
        "table-refresh",
        "page-cut",
        "row-add",
        "edits-count",
        "edit-reset",
        "edit-apply",
    ];

    #[test]
    fn no_width_draws_one_control_over_another() {
        // All the bar can show: the longest page label, a cut page's note, three changes; then the
        // spinners of a page and a count on their way.
        let full = || Paging {
            page: 9_999,
            total: Total::Exact(1_000_000),
            rows: 13,
            complete: false,
            edits: 3,
            blocked: Some("changes are staged"),
            ..Paging::default()
        };
        let busy = || Paging { loading: true, total: Total::Counting { estimate: Some(5_000) }, edits: 1, ..Paging::default() };
        for (what, make) in [("full", &full as &dyn Fn() -> Paging), ("busy", &busy)] {
            let mut seen: Vec<u8> = Vec::new();
            for width in (40..=900).rev().step_by(10) {
                let (rects, bar) = drawn(make(), width as f32);
                let at = format!("{what}, {width} wide");
                let shown: Vec<(&str, Rect)> = rects.iter().filter_map(|(id, r)| r.map(|r| (*id, r))).collect();
                for (i, (a, ra)) in shown.iter().enumerate() {
                    assert!(ra.left() >= bar.left() - 0.5 && ra.right() <= bar.right() + 0.5, "{at}: `{a}` {ra:?} leaves the bar {bar:?}");
                    for (b, rb) in shown.iter().skip(i + 1) {
                        let o = ra.intersect(*rb);
                        assert!(o.width() <= 0.5 || o.height() <= 0.5, "{at}: `{a}` {ra:?} overlaps `{b}` {rb:?}");
                    }
                }
                // The order of giving room up, as the bar narrows.
                let rect = |id: &str| rects.iter().find(|(i, _)| *i == id).and_then(|(_, r)| *r);
                let icon = rect("page-count").is_some_and(|r| r.width() <= 27.0);
                let text = rect("edits-count").is_some();
                let cut = rect("edit-apply").is_none_or(|r| r.width() < 63.5);
                // Once the right group is cut the left one may be cut too, so the count's form
                // shows only before.
                assert!(cut || text || icon, "{at}: the changes' text went before the count's");
                assert!(!cut || !text, "{at}: cut while the changes' text could give room up");
                let fit = if cut {
                    3
                } else if !text {
                    2
                } else {
                    u8::from(icon)
                };
                assert!(seen.last().is_none_or(|&last| fit >= last), "{at}: room taken back while narrowing ({seen:?} then {fit})");
                seen.push(fit);
            }
            assert_eq!(seen.first(), Some(&0), "{what}: 900 points wide, everything shows");
            for fit in 0..=3 {
                assert!(seen.contains(&fit), "{what}: the sweep passes through form {fit}: {seen:?}");
            }
        }
    }

    /// The egui ids of the right group's buttons as a bar `width` wide draws `p`, each found
    /// among the pass's widgets by the rect it was tagged with; with whether the changes' text
    /// and a whole Apply show.
    fn right_ids(p: Paging, width: f32) -> (Vec<(&'static str, Option<egui::Id>)>, bool, bool) {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(width, BAR_H)).build_ui_state(
            |ui, s: &mut (Paging, Tags)| {
                if !crate::theme::applied(ui.ctx()) {
                    return;
                }
                s.1.begin_frame();
                let _ = show(ui, &s.0, &mut s.1);
            },
            (p, Tags::default()),
        );
        h.run_steps(2);
        let tags = &h.state().1;
        let rects: Vec<(&'static str, Option<Rect>)> = ["row-add", "edit-reset", "edit-apply"].into_iter().map(|id| (id, tags.rect(id))).collect();
        let text = tags.rect("edits-count").is_some();
        let whole = tags.rect("edit-apply").is_some_and(|r| r.width() >= 63.5);
        let ids = h.ctx.viewport(|vp| {
            let widgets = &vp.prev_pass.widgets;
            let find = |r: Rect| widgets.layers().flat_map(|(_, w)| w.iter()).find(|w| w.interact_rect == r && w.sense.senses_click()).map(|w| w.id);
            rects.iter().map(|(id, r)| (*id, r.and_then(find))).collect()
        });
        (ids, text, whole)
    }

    #[test]
    fn the_right_groups_ids_stay_when_the_note_or_the_changes_text_comes_and_goes() {
        let staged = |complete: bool| Paging { rows: 13, complete, edits: 2, ..Paging::default() };
        // A cut page's note, drawn between the groups, leaves the right group's ids alone.
        let (plain, _, _) = right_ids(staged(true), 900.0);
        let (noted, _, _) = right_ids(staged(false), 900.0);
        assert!(plain.iter().all(|(_, id)| id.is_some()), "every button found: {plain:?}");
        assert_eq!(noted, plain, "with the note");
        // Nor does the changes' text, when the bar is short of room for it.
        let (all, text, _) = right_ids(staged(true), 900.0);
        assert!(text);
        let (bare, _, _) = (300..900)
            .rev()
            .step_by(5)
            .map(|w| right_ids(staged(true), w as f32))
            .find(|(_, text, whole)| !text && *whole)
            .expect("a width with Apply whole and no changes' text");
        assert_eq!(bare, all, "without the changes' text");
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

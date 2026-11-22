//! The paging in the table tab's bottom bar, in words and rules: the page's words
//! ([`page_text`]), the cut page's note ([`cut_note`]), the count of staged changes
//! ([`edits_text`]), and when the arrows and APPLY work and why not ([`Paging`]). The bar is drawn
//! by [`crate::components::table_bars::PagingBar`], which the table tab ([`crate::table_tab`])
//! places and tags, with the DATA / STRUCTURE switch at its left end in both views; the row count,
//! refresh and "+ Row" are the table bar's, and [`count_text`] is the count's words there.
//!
//! The row count ([`count_text`]) shows the planner's estimate as `~100,000 rows` while that is
//! all there is: the engine's, or the catalog's for an unfiltered table, materialized view or
//! partitioned table. Views and foreign tables have none, and neither does a table the planner
//! puts at 0, so it never reads `~0`. With an exact count the page reads `Page 3 of 10`.
//!
//! Paging waits while changes are staged (the engine's rule) and while the connection is lost;
//! the buttons say why when hovered.
//!
//! Automation ids (tagged by the table tab): `table-view-data`, `table-view-structure`,
//! `page-first`, `page-prev`, `page-number` (value: `Page 3` or `Page 3 of 10`), `page-next`,
//! `page-last`, `page-cut` (value: the note; not drawn without room for it), `edits-count` (value:
//! `2 changes`; not drawn while short of room), `edit-reset`, `edit-apply`.

use dbee_engine::Total;
use dbee_sql::select::PAGE_SIZE;

use crate::tree::thousands;

/// The bar's height.
pub const BAR_H: f32 = crate::theme::size::BAR_H;

/// What the bar shows.
#[derive(Debug, Default)]
pub(crate) struct Paging {
    /// The page shown, 0-based.
    pub page: u64,
    /// The page shown is the one asked for and nothing follows it.
    pub at_end: bool,
    pub total: Total,
    /// Rows the page shows, and whether that is all of them ([`cut_note`]).
    pub rows: usize,
    pub complete: bool,
    /// Staged changes.
    pub edits: usize,
    /// Why paging cannot run now (staged changes, a lost connection).
    pub blocked: Option<&'static str>,
    /// Why Apply cannot run now.
    pub apply_blocked: Option<&'static str>,
}

impl Paging {
    /// Why FIRST and PREVIOUS can't run now: paging is blocked, or this is the first page.
    pub fn back_blocked(&self) -> Option<&'static str> {
        self.blocked.or(if self.page == 0 { Some("this is the first page") } else { None })
    }

    /// Why NEXT and LAST can't run now: paging is blocked, or nothing follows this page.
    pub fn forward_blocked(&self) -> Option<&'static str> {
        self.blocked.or(if self.at_end { Some("this is the last page") } else { None })
    }
}

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
    fn the_arrows_say_why_they_wait() {
        let p = Paging { page: 0, at_end: true, ..Paging::default() };
        assert_eq!((p.back_blocked(), p.forward_blocked()), (Some("this is the first page"), Some("this is the last page")));
        let p = Paging { page: 2, at_end: false, ..Paging::default() };
        assert_eq!((p.back_blocked(), p.forward_blocked()), (None, None));
        let p = Paging { page: 0, at_end: true, blocked: Some("apply or reset your changes first"), ..Paging::default() };
        assert_eq!(p.back_blocked(), Some("apply or reset your changes first"), "blocked paging says so first");
        assert_eq!(p.forward_blocked(), Some("apply or reset your changes first"));
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

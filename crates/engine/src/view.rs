//! The grid the selection addresses, and the value view of its focus cell.
//!
//! **The grid.** On a table tab it is the loaded page followed by the new rows staged on it (grid
//! rows `0..data.rows.len()` are loaded, the rest `row - len` into `Edits::new_rows`); on a query
//! tab it is the shown result, in its sorted order when it has one (`query.sortResult`: grid row
//! `i` is the row [`Grid::order`] puts there, and every read here, a copy's and the value view's
//! too, goes through it). A cell's text is its staged value when it has one, else the loaded
//! one; a row shorter than the columns, or a new row's unset cell, reads as NULL. Nothing here
//! indexes: a selection that no longer fits the grid is clamped or dropped, never a panic.
//!
//! **The value view** is what the value panel shows for the focus cell: the text pretty-printed
//! when it is JSON, cut to [`VALUE_VIEW_MAX_BYTES`]. For a whole-row selection (`selection.rows`)
//! it is the focus row instead: the object Copy as JSON writes for it, pretty-printed, cut at the
//! same cap as a long value. Classifying and pretty-printing walk the whole value (up to 20 MB),
//! and the UI asks for the view every frame, so the session keeps the last one ([`Cache`]) and
//! builds a new one only when the tab, the cell or row, the data, the order of a sorted result's
//! rows or the staged values change.

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use dbee_core::copy::{Cells, row_json};
use dbee_core::json::{self, JsonKind, MAX_PRETTY_BYTES};
use dbee_core::{Cell, Column, ResultSet};
use dbee_sql::select::PAGE_SIZE;
use serde::{Serialize, Serializer};

use crate::edits::Edits;
use crate::state::{Selection, Tab, received_row};
use crate::{Json, TabId};

/// The most text a value view holds: the cap `json::pretty` works up to.
pub const VALUE_VIEW_MAX_BYTES: usize = MAX_PRETTY_BYTES;

/// The value panel's content for one cell, or one row.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueView {
    /// The column's name; empty for a row.
    pub column: String,
    /// The row's number as the grid shows it (a page's offset counted in), for a whole-row
    /// selection; `None` for a cell.
    pub row: Option<u64>,
    /// The column's type as the panel names it (`character varying(200)`); empty for a row.
    pub type_label: String,
    #[serde(serialize_with = "kind_id")]
    pub kind: JsonKind,
    /// A JSON column's type as the engine names it (`jsonb`; `json` when it names none), `text
    /// containing JSON`, `JSON string containing JSON`, or `""` (also for a row).
    pub kind_label: String,
    /// Pretty when the value is JSON and under the cap (for a `JsonStringContainingJson`, the
    /// decoded inner document pretty-printed), else the raw text cut to the cap on a character
    /// boundary. Empty for NULL. A row's object is JSON.
    pub text: String,
    /// `text` was pretty-printed.
    pub pretty: bool,
    /// `text` was pretty-printed from text the database doesn't call JSON: a text column holding
    /// JSON, or a JSON string holding JSON. Never for a row or a cut value.
    pub formatted: bool,
    /// The value was longer than the cap and `text` is its start.
    pub truncated: bool,
    pub is_null: bool,
    /// The length of the whole value in bytes (more than `text` when it was cut).
    pub byte_len: usize,
}

fn kind_id<S: Serializer>(kind: &JsonKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(match kind {
        JsonKind::Json => "json",
        JsonKind::TextContainingJson => "textContainingJson",
        JsonKind::JsonStringContainingJson => "jsonStringContainingJson",
        JsonKind::NotJson => "notJson",
    })
}

/// The longest start of `text` of at most `max` bytes that ends on a character boundary (the
/// value panel cuts its automation tag with it too).
pub fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default()
}

impl ValueView {
    /// The view of `value` (`None` for NULL) in `column`.
    pub fn new(column: &Column, value: Option<&str>) -> ValueView {
        let mut view = ValueView {
            column: column.name.clone(),
            row: None,
            type_label: column.type_label.clone(),
            kind: JsonKind::NotJson,
            kind_label: String::new(),
            text: String::new(),
            pretty: false,
            formatted: false,
            truncated: false,
            is_null: value.is_none(),
            byte_len: value.map_or(0, str::len),
        };
        let Some(raw) = value else { return view };
        if raw.len() > VALUE_VIEW_MAX_BYTES {
            // Too long to pretty-print (`json::pretty` gives up above the cap), so the start of
            // the raw text; the kind is the column's, without walking 20 MB again.
            view.kind = json::column_kind(column);
            view.text = cut(raw, VALUE_VIEW_MAX_BYTES).to_owned();
            view.truncated = true;
        } else {
            view.kind = json::classify(column, raw);
            let pretty = match view.kind {
                JsonKind::NotJson => None,
                JsonKind::JsonStringContainingJson => json::decoded_inner(raw).and_then(|inner| json::pretty(&inner)),
                _ => json::pretty(raw),
            };
            view.pretty = pretty.is_some();
            view.formatted = view.pretty && matches!(view.kind, JsonKind::TextContainingJson | JsonKind::JsonStringContainingJson);
            view.text = pretty.unwrap_or_else(|| raw.to_owned());
        }
        view.kind_label = match view.kind {
            JsonKind::Json if !column.type_label.is_empty() => column.type_label.clone(),
            kind => kind.label().to_owned(),
        };
        view
    }

    /// The view of the row numbered `number`, whose object ([`row_json`]) starts with `object`
    /// and is `len` bytes long: pretty-printed when it is whole, else its start, as a long value's.
    pub fn of_row(number: u64, object: String, len: usize) -> ValueView {
        let truncated = len > object.len();
        let pretty = if truncated { None } else { json::pretty(&object) };
        ValueView {
            column: String::new(),
            row: Some(number),
            type_label: String::new(),
            kind: JsonKind::Json,
            kind_label: String::new(),
            pretty: pretty.is_some(),
            formatted: false,
            text: pretty.unwrap_or(object),
            truncated,
            is_null: false,
            byte_len: len,
        }
    }

    /// What the panel's bar says: the column's name, or `Row 3` for a row.
    pub fn title(&self) -> Cow<'_, str> {
        match self.row {
            Some(n) => Cow::Owned(format!("Row {}", crate::cmd::selection::thousands(n))),
            None => Cow::Borrowed(&self.column),
        }
    }
}

/// The grid of the active tab, with its selection.
pub(crate) struct Grid<'a> {
    pub tab: TabId,
    pub data: &'a Arc<ResultSet>,
    /// A sorted query result's row order: grid row `i` is `data.rows[order[i]]`. `None` for rows
    /// as they are held (a table tab's page, a result as received).
    pub order: Option<&'a Arc<[u32]>>,
    /// A table tab's staged edits.
    pub edits: Option<&'a Edits>,
    pub selection: Option<Selection>,
    /// The number the grid shows for row 0: a table page's offset plus one.
    pub first: u64,
}

impl<'a> Grid<'a> {
    /// The grid `tab` shows; `None` while a table tab has no page or a query tab no result.
    pub fn of(tab: &'a Tab) -> Option<Grid<'a>> {
        match tab {
            Tab::Table(t) => Some(Grid {
                tab: t.id,
                data: t.data.as_ref()?,
                order: None,
                edits: Some(&*t.edits),
                selection: t.selection,
                first: t.shown.page.saturating_mul(PAGE_SIZE).saturating_add(1),
            }),
            Tab::Query(q) => {
                let shown = q.shown_result()?;
                Some(Grid { tab: q.id, data: &shown.result, order: shown.order(), edits: None, selection: q.selection, first: 1 })
            }
        }
    }

    pub fn rows(&self) -> usize {
        self.edits.map_or(self.data.rows.len(), |e| e.grid_rows(self.data))
    }

    pub fn columns(&self) -> usize {
        self.data.columns.len()
    }

    pub fn contains(&self, (row, col): (usize, usize)) -> bool {
        row < self.rows() && col < self.columns()
    }

    /// `sel` with the focus brought inside the grid, whole rows across every column; `None` when
    /// the anchor is outside it.
    pub fn fit(&self, sel: Selection) -> Option<Selection> {
        let anchor = if sel.whole { (sel.anchor.0, 0) } else { sel.anchor };
        if !self.contains(anchor) {
            return None;
        }
        let last = self.columns().saturating_sub(1);
        let focus = (sel.focus.0.min(self.rows().saturating_sub(1)), if sel.whole { last } else { sel.focus.1.min(last) });
        Some(Selection { anchor, focus, whole: sel.whole })
    }

    /// The stored selection as far as it still fits the grid.
    pub fn selection(&self) -> Option<Selection> {
        self.selection.and_then(|sel| self.fit(sel))
    }

    /// The text of grid cell `(row, col)`: its staged value, else the loaded one. `None` is NULL
    /// (also for a cell the row does not have, and a new row's unset cell).
    pub fn cell(&self, row: usize, col: usize) -> Option<&'a str> {
        if let Some(staged) = self.edits.and_then(|e| e.shown(self.data, row, col)) {
            return staged;
        }
        self.held(row)?.get(col)?.as_deref()
    }

    /// The cells held for grid row `row`: through the order, when the rows are sorted.
    fn held(&self, row: usize) -> Option<&'a [Cell]> {
        let at = received_row(self.order.map(|order| &**order), row)?;
        self.data.rows.get(at).map(Vec::as_slice)
    }

    /// The view of row `row`: its object as Copy as JSON writes it, cut at the cap.
    fn row_view(&self, row: usize) -> ValueView {
        let cells = Rect { grid: self, top: row, left: 0, rows: 1, columns: self.columns() };
        let (object, len) = row_json(&self.data.columns, &cells, 0, VALUE_VIEW_MAX_BYTES);
        ValueView::of_row(self.first.saturating_add(row as u64), object, len)
    }
}

/// A rectangle of a grid, read cell by cell: what a copy renders.
pub(crate) struct Rect<'g, 'a> {
    pub grid: &'g Grid<'a>,
    pub top: usize,
    pub left: usize,
    pub rows: usize,
    pub columns: usize,
}

impl Cells for Rect<'_, '_> {
    fn rows(&self) -> usize {
        self.rows
    }
    fn width(&self, _: usize) -> usize {
        self.columns
    }
    fn cell(&self, row: usize, col: usize) -> Option<&str> {
        if row >= self.rows || col >= self.columns {
            return None;
        }
        self.grid.cell(self.top.saturating_add(row), self.left.saturating_add(col))
    }
    fn each<E>(&self, row: usize, mut f: impl FnMut(Option<&str>) -> Result<(), E>) -> Result<(), E> {
        if row >= self.rows {
            return Ok(());
        }
        if self.grid.edits.is_some() {
            // A table tab's page: staged values come first, cell by cell.
            for col in 0..self.columns {
                f(self.cell(row, col))?;
            }
            return Ok(());
        }
        // A query's result: the row's cells straight from the data, NULL past a short row's end.
        let cells = self.grid.held(self.top.saturating_add(row)).and_then(|r| r.get(self.left..)).unwrap_or_default();
        let cells = cells.get(..self.columns).unwrap_or(cells);
        for v in cells {
            f(v.as_deref())?;
        }
        for _ in cells.len()..self.columns {
            f(None)?;
        }
        Ok(())
    }
}

/// What a view shows: the focus cell, or the focus row of a whole-row selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Cell(usize, usize),
    Row(usize),
}

/// What the cached view was built for.
struct Entry {
    tab: TabId,
    shown: Shown,
    /// Held, so its address cannot be reused by other data while the entry lives.
    data: Arc<ResultSet>,
    /// The order the data's rows were in, held as the data is: a sort is a new list, so the row
    /// at the focus may be another.
    order: Option<Arc<[u32]>>,
    revision: u64,
    view: Arc<ValueView>,
}

/// The last value view built by a session (see the module docs).
#[derive(Default)]
pub(crate) struct Cache {
    slot: Mutex<Option<Entry>>,
    built: AtomicUsize,
}

impl Cache {
    /// The view of the focus cell of `tab`, or of its focus row when whole rows are selected;
    /// built only when something it depends on changed.
    pub fn get(&self, tab: &Tab) -> Option<Arc<ValueView>> {
        let grid = Grid::of(tab)?;
        let sel = grid.selection()?;
        let (row, col) = sel.focus;
        let shown = if sel.whole { Shown::Row(row) } else { Shown::Cell(row, col) };
        let revision = grid.edits.map_or(0, Edits::revision);
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = slot.as_ref()
            && e.tab == grid.tab
            && e.shown == shown
            && e.revision == revision
            && Arc::ptr_eq(&e.data, grid.data)
            && same_order(e.order.as_ref(), grid.order)
        {
            return Some(Arc::clone(&e.view));
        }
        let view = match shown {
            Shown::Row(row) => grid.row_view(row),
            Shown::Cell(row, col) => ValueView::new(grid.data.columns.get(col)?, grid.cell(row, col)),
        };
        let view = Arc::new(view);
        self.built.fetch_add(1, Ordering::Relaxed);
        *slot = Some(Entry { tab: grid.tab, shown, data: Arc::clone(grid.data), order: grid.order.cloned(), revision, view: Arc::clone(&view) });
        Some(view)
    }

    /// How many views were built, for tests of the caching.
    pub fn built(&self) -> usize {
        self.built.load(Ordering::Relaxed)
    }
}

/// Both as received, or the very same list.
fn same_order(a: Option<&Arc<[u32]>>, b: Option<&Arc<[u32]>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// How much of a long text `inspect()` carries.
pub const INSPECT_TEXT_BYTES: usize = 4 * 1024;

/// `inspect()`'s `value`: the metadata and at most the first 4 KiB of the text. Built field by
/// field, so the whole text (up to [`VALUE_VIEW_MAX_BYTES`]) is never copied or escaped.
pub(crate) fn summary(v: &ValueView) -> Json {
    let text = cut(&v.text, INSPECT_TEXT_BYTES);
    let kind = kind_id(&v.kind, serde_json::value::Serializer).unwrap_or(Json::Null);
    serde_json::json!({
        "column": v.column,
        "row": v.row,
        "typeLabel": v.type_label,
        "kind": kind,
        "kindLabel": v.kind_label,
        "text": text,
        "pretty": v.pretty,
        "formatted": v.formatted,
        "truncated": v.truncated,
        "isNull": v.is_null,
        "bytes": v.byte_len,
        "textCut": text.len() < v.text.len(),
    })
}

/// `inspect()`'s `selection` ([`Selection::to_json`]) as far as it fits the grid, or null.
pub(crate) fn selection_json(tab: Option<&Tab>) -> Json {
    tab.and_then(Grid::of).and_then(|g| g.selection()).map_or(Json::Null, Selection::to_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{QueryTab, StatementOutcome};
    use dbee_core::TypeClass;

    fn col(name: &str, base: &str) -> Column {
        let class = match base {
            "int4" => TypeClass::Number,
            "json" | "jsonb" => TypeClass::Json,
            "text" => TypeClass::LongText,
            _ => TypeClass::Other,
        };
        Column { name: name.into(), base_type: base.into(), type_label: base.into(), class }
    }

    #[test]
    fn cut_stays_on_a_character_boundary() {
        assert_eq!(cut("aé", 2), "a");
        assert_eq!(cut("aé", 3), "aé");
        assert_eq!(cut("🐙🐙", 5), "🐙");
        assert_eq!(cut("abc", 0), "");
        assert_eq!(cut("abc", usize::MAX), "abc");
    }

    #[test]
    fn views_of_odd_values() {
        let v = ValueView::new(&col("j", "jsonb"), Some("{bad"));
        assert_eq!((v.kind, v.kind_label.as_str(), v.pretty, v.text.as_str()), (JsonKind::Json, "jsonb", false, "{bad"));
        let v = ValueView::new(&col("n", "int4"), Some("42"));
        assert_eq!((v.kind_label.as_str(), v.pretty, v.truncated, v.byte_len), ("", false, false, 2));
        // A JSON column's label is its type as the engine names it, `json` when it names none.
        assert_eq!(ValueView::new(&col("j", "json"), Some("[1]")).kind_label, "json");
        let unnamed = Column { type_label: String::new(), ..col("j", "jsonb") };
        assert_eq!(ValueView::new(&unnamed, Some("[1]")).kind_label, "json");
        let nested = ValueView::new(&col("j", "jsonb"), Some(r#""{\"a\":1}""#));
        assert_eq!((nested.kind, nested.kind_label.as_str()), (JsonKind::JsonStringContainingJson, "JSON string containing JSON"));
        let long = "é".repeat(VALUE_VIEW_MAX_BYTES / 2 + 1);
        let v = ValueView::new(&col("t", "text"), Some(&long));
        assert!(v.truncated && v.text.len() <= VALUE_VIEW_MAX_BYTES && v.byte_len == long.len());
    }

    /// `formatted` is set only when text the database doesn't call JSON was pretty-printed: a
    /// text column holding JSON, or a JSON string holding JSON.
    #[test]
    fn formatted_says_when_text_was_turned_into_json() {
        let v = ValueView::new(&col("t", "text"), Some(r#"{"a":1}"#));
        assert!(v.pretty && v.formatted, "text holding JSON");
        let v = ValueView::new(&col("j", "jsonb"), Some(r#"{"a":1}"#));
        assert!(v.pretty && !v.formatted, "a JSON column's own JSON");
        let v = ValueView::new(&col("j", "jsonb"), Some(r#""{\"a\":1}""#));
        assert!(v.pretty && v.formatted, "a JSON string holding JSON");
        let v = ValueView::new(&col("t", "text"), Some("plain"));
        assert!(!v.formatted);
        let long = format!("[{}1]", "1,".repeat(10_000_000));
        let v = ValueView::new(&col("t", "text"), Some(&long));
        assert!(v.truncated && !v.formatted, "a cut value isn't formatted");
        let object = r#"{"a": "{\"b\":1}"}"#;
        let v = ValueView::of_row(3, object.into(), object.len());
        assert!(v.pretty && !v.formatted, "a row");
        assert_eq!(serde_json::to_value(ValueView::new(&col("t", "text"), Some("[1]"))).unwrap()["formatted"], true);
    }

    #[test]
    fn summary_is_the_view_with_its_text_cut() {
        let long = format!("{}é{}", "a".repeat(INSPECT_TEXT_BYTES - 1), "b".repeat(VALUE_VIEW_MAX_BYTES));
        let v = ValueView::new(&col("t", "text"), Some(&long));
        let s = summary(&v);
        let expected = serde_json::json!({
            "column": "t", "typeLabel": "text", "kind": "notJson", "kindLabel": "", "text": "a".repeat(INSPECT_TEXT_BYTES - 1), "row": null,
            "pretty": false, "formatted": false, "truncated": true, "isNull": false, "bytes": long.len(), "textCut": true,
        });
        assert_eq!(s, expected);
        let v = ValueView::new(&col("j", "jsonb"), Some(r#"{"a":1}"#));
        let expected = serde_json::json!({
            "column": "j", "typeLabel": "jsonb", "kind": "json", "kindLabel": "jsonb", "text": v.text, "row": null,
            "pretty": true, "formatted": false, "truncated": false, "isNull": false, "bytes": 7, "textCut": false,
        });
        assert_eq!(summary(&v), expected);
        let v = ValueView::new(&col("n", "int4"), None);
        assert_eq!(summary(&v)["isNull"], true);
        assert_eq!(summary(&v)["text"], "");
        // The same keys as the view itself, with `byteLen` named `bytes`, plus `textCut`.
        let mut keys: Vec<String> = serde_json::to_value(&v).unwrap().as_object().unwrap().keys().cloned().collect();
        keys.retain(|k| k != "byteLen");
        keys.extend(["bytes".to_string(), "textCut".to_string()]);
        keys.sort();
        let mut got: Vec<String> = summary(&v).as_object().unwrap().keys().cloned().collect();
        got.sort();
        assert_eq!(got, keys);
    }

    /// A query tab showing `rows` of `columns`, with `selection`.
    fn query_tab(columns: Vec<Column>, rows: Vec<Vec<Option<String>>>, selection: Selection) -> Tab {
        let rs = ResultSet { columns, rows_seen: rows.len() as u64, rows, complete: true, rows_affected: None, database_after: Default::default() };
        let mut q = QueryTab::new(TabId(1), "q".into(), String::new());
        q.results.push(StatementOutcome::new(0..0, Arc::new(rs), 0));
        q.selection = Some(selection);
        Tab::Query(q)
    }

    /// A row's view is its object as Copy as JSON writes it, pretty when it fits; over the cap it
    /// is the start of that object, as a long value is the start of its text. A row selection
    /// stored with other columns spans the grid's.
    #[test]
    fn a_row_view_is_its_object_cut_as_a_long_value_is() {
        let columns = vec![col("a", "text"), col("b", "jsonb")];
        let whole = |row: usize| Selection { anchor: (row, 0), focus: (row, 50), whole: true };
        let tab = query_tab(columns.clone(), vec![vec![Some("x".into())], vec![Some("y".into()), Some("[1]".into())]], whole(1));
        let grid = Grid::of(&tab).unwrap();
        assert_eq!(grid.selection(), Some(Selection { anchor: (1, 0), focus: (1, 1), whole: true }));
        let v = Cache::default().get(&tab).unwrap();
        assert_eq!(v.text, "{\n  \"a\": \"y\",\n  \"b\": [\n    1\n  ]\n}");
        assert_eq!((v.row, v.title(), v.pretty, v.truncated, v.kind, v.byte_len), (Some(2), "Row 2".into(), true, false, JsonKind::Json, 20));
        // A ragged row: NULL past its end.
        let tab = query_tab(columns.clone(), vec![vec![Some("x".into())]], whole(0));
        assert_eq!(Cache::default().get(&tab).unwrap().text, "{\n  \"a\": \"x\",\n  \"b\": null\n}");
        // Over the cap: the start, cut on a character boundary, and the whole length.
        let half = "é".repeat(VALUE_VIEW_MAX_BYTES / 4 + 1);
        let tab = query_tab(vec![col("a", "text"), col("b", "text")], vec![vec![Some(half.clone()), Some(half.clone())]], whole(0));
        let v = Cache::default().get(&tab).unwrap();
        let object = format!("{{\"a\": \"{half}\", \"b\": \"{half}\"}}");
        assert!(v.truncated && !v.pretty && v.kind == JsonKind::Json, "{:?}", (v.truncated, v.pretty, v.kind));
        assert_eq!(v.byte_len, object.len());
        assert!(v.text.len() <= VALUE_VIEW_MAX_BYTES && v.text.len() + 2 > VALUE_VIEW_MAX_BYTES && object.starts_with(&v.text));
        assert_eq!(summary(&v)["row"], 1);
    }

    /// A sorted result is read through its order by everything here: a cell, a copy's rows
    /// ([`Rect::each`]), a row's view and the cached value view, which a new order makes anew.
    #[test]
    fn a_sorted_result_reads_through_its_order() {
        let rows = vec![vec![Some("b".into()), Some("1".into())], vec![Some("c".into())], vec![Some("a".into()), Some("3".into())]];
        let cell = Selection { anchor: (0, 0), focus: (0, 0), whole: false };
        let mut tab = query_tab(vec![col("t", "text"), col("n", "int4")], rows, cell);
        let reorder = |tab: &mut Tab, order: Option<Vec<u32>>| {
            let Tab::Query(q) = tab else { panic!("a query tab") };
            q.results[0].order = order.map(Arc::from);
        };
        let select = |tab: &mut Tab, sel: Selection| {
            let Tab::Query(q) = tab else { panic!("a query tab") };
            q.selection = Some(sel);
        };
        /// Every row of the grid as a copy reads it, by `each` and by `cell`.
        fn read(tab: &Tab) -> Vec<Vec<Option<String>>> {
            let grid = Grid::of(tab).unwrap();
            let rect = Rect { grid: &grid, top: 0, left: 0, rows: grid.rows(), columns: grid.columns() };
            let by_each: Vec<Vec<Option<String>>> = (0..rect.rows)
                .map(|row| {
                    let mut cells = Vec::new();
                    rect.each(row, |v| {
                        cells.push(v.map(str::to_owned));
                        Ok::<(), ()>(())
                    })
                    .unwrap();
                    cells
                })
                .collect();
            let by_cell: Vec<Vec<Option<String>>> =
                (0..rect.rows).map(|row| (0..rect.columns).map(|c| Cells::cell(&rect, row, c).map(str::to_owned)).collect()).collect();
            assert_eq!(by_each, by_cell);
            by_each
        }
        let s = |v: &str| Some(v.to_owned());
        let cache = Cache::default();
        assert_eq!(read(&tab), [[s("b"), s("1")], [s("c"), None], [s("a"), s("3")]]);
        assert_eq!(cache.get(&tab).unwrap().text, "b");
        // Sorted: the same grid position is another row, for the cell, the copy and the view.
        reorder(&mut tab, Some(vec![2, 0, 1]));
        assert_eq!(read(&tab), [[s("a"), s("3")], [s("b"), s("1")], [s("c"), None]]);
        assert_eq!(Grid::of(&tab).unwrap().cell(2, 0), Some("c"));
        assert_eq!(cache.get(&tab).unwrap().text, "a", "a new order is a new view");
        assert_eq!(cache.built(), 2);
        cache.get(&tab);
        assert_eq!(cache.built(), 2, "the same order: the view kept");
        // The same order as a new list (the same sort asked again) is built again; as received too.
        reorder(&mut tab, Some(vec![2, 0, 1]));
        assert_eq!((cache.get(&tab).unwrap().text.as_str(), cache.built()), ("a", 3));
        reorder(&mut tab, None);
        assert_eq!((cache.get(&tab).unwrap().text.as_str(), cache.built()), ("b", 4));
        // A whole row: the object of the row now there, numbered by its place in the grid.
        reorder(&mut tab, Some(vec![1, 2, 0]));
        select(&mut tab, Selection { anchor: (2, 0), focus: (2, 1), whole: true });
        let v = cache.get(&tab).unwrap();
        assert_eq!((v.text.as_str(), v.row), ("{\n  \"t\": \"b\",\n  \"n\": 1\n}", Some(3)));
        // A part of the rows from the second column on, as a copy of a selection takes it.
        let grid = Grid::of(&tab).unwrap();
        let rect = Rect { grid: &grid, top: 1, left: 1, rows: 2, columns: 1 };
        assert_eq!((rect.cell(0, 0), rect.cell(1, 0), rect.cell(2, 0)), (Some("3"), Some("1"), None));
        // An order that names a row the result does not have, or holds fewer rows than it, reads
        // as NULL there: never a panic.
        reorder(&mut tab, Some(vec![7, u32::MAX, 0]));
        assert_eq!(read(&tab), [[None, None], [None, None], [s("b"), s("1")]]);
        reorder(&mut tab, Some(vec![2]));
        assert_eq!(read(&tab), [[s("a"), s("3")], [None, None], [None, None]]);
        select(&mut tab, Selection { anchor: (1, 0), focus: (1, 0), whole: false });
        assert!(cache.get(&tab).unwrap().is_null);
    }

    #[test]
    fn ragged_rows_and_stale_selections_do_not_panic() {
        let rs = ResultSet {
            columns: vec![col("a", "text"), col("b", "text")],
            rows: vec![vec![Some("1".into())], vec![]],
            rows_seen: 2,
            complete: true,
            rows_affected: None,
            database_after: Default::default(),
        };
        let mut q = QueryTab::new(TabId(1), "q".into(), String::new());
        q.results.push(StatementOutcome::new(0..0, Arc::new(rs), 0));
        q.selection = Some(Selection { anchor: (0, 0), focus: (50, 50), whole: false });
        let tab = Tab::Query(q);
        let g = Grid::of(&tab).expect("a grid");
        assert_eq!((g.cell(0, 0), g.cell(0, 1), g.cell(1, 0), g.cell(9, 9)), (Some("1"), None, None, None));
        assert_eq!(g.selection().map(|s| s.focus), Some((1, 1)));
        let cache = Cache::default();
        assert!(cache.get(&tab).is_some_and(|v| v.is_null || v.text == "1"));
        let mut gone = tab.clone();
        if let Tab::Query(q) = &mut gone {
            q.selection = Some(Selection { anchor: (7, 0), focus: (0, 0), whole: false });
        }
        assert!(cache.get(&gone).is_none());
    }
}

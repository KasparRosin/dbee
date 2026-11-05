//! The grid the selection addresses, and the value view of its focus cell.
//!
//! **The grid.** On a table tab it is the loaded page followed by the new rows staged on it (grid
//! rows `0..data.rows.len()` are loaded, the rest `row - len` into `Edits::new_rows`); on a query
//! tab it is the shown result. A cell's text is its staged value when it has one, else the loaded
//! one; a row shorter than the columns, or a new row's unset cell, reads as NULL. Nothing here
//! indexes: a selection that no longer fits the grid is clamped or dropped, never a panic.
//!
//! **The value view** is what the value panel shows for the focus cell: the text pretty-printed
//! when it is JSON, cut to [`VALUE_VIEW_MAX_BYTES`]. Classifying and pretty-printing walk the
//! whole value (up to 20 MB), and the UI asks for the view every frame, so the session keeps the
//! last one ([`Cache`]) and builds a new one only when the tab, the cell, the data or the staged
//! values change.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use dbee_core::json::{self, JsonKind, MAX_PRETTY_BYTES};
use dbee_core::{Column, ResultSet};
use serde::{Serialize, Serializer};

use crate::edits::Edits;
use crate::state::{Selection, Tab};
use crate::{Json, TabId};

/// The most text a value view holds: the cap `json::pretty` works up to.
pub const VALUE_VIEW_MAX_BYTES: usize = MAX_PRETTY_BYTES;

/// The value panel's content for one cell.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueView {
    /// The column's name.
    pub column: String,
    /// The column's type as the panel names it (`character varying(200)`).
    pub type_label: String,
    #[serde(serialize_with = "kind_id")]
    pub kind: JsonKind,
    /// A JSON column's type as the engine names it (`jsonb`; `json` when it names none), `text
    /// containing JSON`, `JSON string containing JSON`, or `""`.
    pub kind_label: String,
    /// Pretty when the value is JSON and under the cap (for a `JsonStringContainingJson`, the
    /// decoded inner document pretty-printed), else the raw text cut to the cap on a character
    /// boundary. Empty for NULL.
    pub text: String,
    /// `text` was pretty-printed.
    pub pretty: bool,
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
            type_label: column.type_label.clone(),
            kind: JsonKind::NotJson,
            kind_label: String::new(),
            text: String::new(),
            pretty: false,
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
            view.text = pretty.unwrap_or_else(|| raw.to_owned());
        }
        view.kind_label = match view.kind {
            JsonKind::Json if !column.type_label.is_empty() => column.type_label.clone(),
            kind => kind.label().to_owned(),
        };
        view
    }
}

/// The grid of the active tab, with its selection.
pub(crate) struct Grid<'a> {
    pub tab: TabId,
    pub data: &'a Arc<ResultSet>,
    /// A table tab's staged edits.
    pub edits: Option<&'a Edits>,
    pub selection: Option<Selection>,
}

impl<'a> Grid<'a> {
    /// The grid `tab` shows; `None` while a table tab has no page or a query tab no result.
    pub fn of(tab: &'a Tab) -> Option<Grid<'a>> {
        match tab {
            Tab::Table(t) => Some(Grid { tab: t.id, data: t.data.as_ref()?, edits: Some(&*t.edits), selection: t.selection }),
            Tab::Query(q) => Some(Grid { tab: q.id, data: &q.shown_result()?.result, edits: None, selection: q.selection }),
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

    /// `sel` with the focus brought inside the grid; `None` when the anchor is outside it.
    pub fn fit(&self, sel: Selection) -> Option<Selection> {
        if !self.contains(sel.anchor) {
            return None;
        }
        let focus = (sel.focus.0.min(self.rows().saturating_sub(1)), sel.focus.1.min(self.columns().saturating_sub(1)));
        Some(Selection { anchor: sel.anchor, focus })
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
        self.data.rows.get(row)?.get(col)?.as_deref()
    }
}

/// What the cached view was built for.
struct Entry {
    tab: TabId,
    cell: (usize, usize),
    /// Held, so its address cannot be reused by other data while the entry lives.
    data: Arc<ResultSet>,
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
    /// The view of the focus cell of `tab`; built only when something it depends on changed.
    pub fn get(&self, tab: &Tab) -> Option<Arc<ValueView>> {
        let grid = Grid::of(tab)?;
        let (row, col) = grid.selection()?.focus;
        let revision = grid.edits.map_or(0, Edits::revision);
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = slot.as_ref()
            && e.tab == grid.tab
            && e.cell == (row, col)
            && e.revision == revision
            && Arc::ptr_eq(&e.data, grid.data)
        {
            return Some(Arc::clone(&e.view));
        }
        let column = grid.data.columns.get(col)?;
        let view = Arc::new(ValueView::new(column, grid.cell(row, col)));
        self.built.fetch_add(1, Ordering::Relaxed);
        *slot = Some(Entry { tab: grid.tab, cell: (row, col), data: Arc::clone(grid.data), revision, view: Arc::clone(&view) });
        Some(view)
    }

    /// How many views were built, for tests of the caching.
    pub fn built(&self) -> usize {
        self.built.load(Ordering::Relaxed)
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
        "typeLabel": v.type_label,
        "kind": kind,
        "kindLabel": v.kind_label,
        "text": text,
        "pretty": v.pretty,
        "truncated": v.truncated,
        "isNull": v.is_null,
        "bytes": v.byte_len,
        "textCut": text.len() < v.text.len(),
    })
}

/// `inspect()`'s `selection`: `{anchor: [row, col], focus: [row, col]}` or null.
pub(crate) fn selection_json(tab: Option<&Tab>) -> Json {
    match tab.and_then(Grid::of).and_then(|g| g.selection()) {
        Some(sel) => serde_json::json!({ "anchor": [sel.anchor.0, sel.anchor.1], "focus": [sel.focus.0, sel.focus.1] }),
        None => Json::Null,
    }
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
        assert_eq!(cut("🐝🐝", 5), "🐝");
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

    #[test]
    fn summary_is_the_view_with_its_text_cut() {
        let long = format!("{}é{}", "a".repeat(INSPECT_TEXT_BYTES - 1), "b".repeat(VALUE_VIEW_MAX_BYTES));
        let v = ValueView::new(&col("t", "text"), Some(&long));
        let s = summary(&v);
        let expected = serde_json::json!({
            "column": "t", "typeLabel": "text", "kind": "notJson", "kindLabel": "", "text": "a".repeat(INSPECT_TEXT_BYTES - 1),
            "pretty": false, "truncated": true, "isNull": false, "bytes": long.len(), "textCut": true,
        });
        assert_eq!(s, expected);
        let v = ValueView::new(&col("j", "jsonb"), Some(r#"{"a":1}"#));
        let expected = serde_json::json!({
            "column": "j", "typeLabel": "jsonb", "kind": "json", "kindLabel": "jsonb", "text": v.text,
            "pretty": true, "truncated": false, "isNull": false, "bytes": 7, "textCut": false,
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
        q.results.push(StatementOutcome { range: 0..0, result: Arc::new(rs) });
        q.selection = Some(Selection { anchor: (0, 0), focus: (50, 50) });
        let tab = Tab::Query(q);
        let g = Grid::of(&tab).expect("a grid");
        assert_eq!((g.cell(0, 0), g.cell(0, 1), g.cell(1, 0), g.cell(9, 9)), (Some("1"), None, None, None));
        assert_eq!(g.selection().map(|s| s.focus), Some((1, 1)));
        let cache = Cache::default();
        assert!(cache.get(&tab).is_some_and(|v| v.is_null || v.text == "1"));
        let mut gone = tab.clone();
        if let Tab::Query(q) = &mut gone {
            q.selection = Some(Selection { anchor: (7, 0), focus: (0, 0) });
        }
        assert!(cache.get(&gone).is_none());
    }
}

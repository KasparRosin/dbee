//! Staged edits of a table tab: changed cells of loaded rows, new rows and rows to delete, kept
//! until Apply writes them in one transaction (or Reset drops them), with undo and redo per tab.
//!
//! **Grid rows.** Row `r` below the page's row count is loaded row `r`; from there on it is new
//! row `r - rows`: new rows show after the loaded ones. Columns are the page's, by index.
//!
//! **Staging.** A value equal to the loaded one unstages the cell (NULL equals only NULL, and `''`
//! is not NULL). Marking a loaded row for deletion drops the changes staged on it, and a marked
//! row takes no more; deleting a new row takes it out, and the new rows after it move up. A
//! change that changes nothing is no change at all, and no undo step.
//!
//! **Undo and redo** walk snapshots of the staged edits: every change pushes the edits as they
//! were and drops what could be redone. A reset can be undone too. Writes are not undone: the
//! engine starts a new history after a successful Apply and whenever the tab shows another page,
//! since snapshots name rows of the page they were made on. The history keeps at most
//! [`UNDO_STEPS`] steps and about [`UNDO_BYTES`]; past either, the oldest steps go.
//!
//! **`failed`** is how the last Apply failed ([`Failed`]): the grid row of the change that failed,
//! its column when that change wrote one column, the message and the error's code (`cancelled`
//! for an Apply cancelled before its commit). It stays while the user fixes things,
//! moves up with the rows when a new row before it is deleted, and goes with Reset, with the next
//! Apply and when nothing is staged any more.
//!
//! **An unknown outcome** (the connection was lost, or replaced, while an Apply ran: nobody knows
//! what it wrote) also leaves a doubt, [`Edits::outcome_unknown`]: applying again could write the
//! same changes twice, so the engine asks first. The doubt outlives `failed`: Reset, undo and redo
//! keep it (an undo can bring back the very edits that may be written), and only the next Apply
//! that goes ahead ([`Edits::apply_started`]) or a new page (which starts new edits) ends it.
//!
//! **Statements** ([`Edits::statements`]): one `UPDATE` per changed loaded row setting only its
//! staged columns, one `INSERT` per new row with only its staged columns (the others get their
//! defaults), one `DELETE` per marked row, in that order. A loaded row is found by the table's
//! row key with the values the row was loaded with, so editing the key works. Page columns are
//! matched to the catalog by name: a catalog refreshed since the page loaded still fits, or the
//! statements are refused.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dbee_core::{ColumnInfo, ResultSet, TableInfo};
use dbee_sql::Dialect;
use dbee_sql::SqlError;
use dbee_sql::edit::{NewValue, delete_sql, insert_sql, update_sql};

/// Undo steps kept per tab.
pub const UNDO_STEPS: usize = 500;

/// About how much the undo history of a tab holds (staged text plus a little per entry).
pub const UNDO_BYTES: usize = 64 * 1024 * 1024;

/// `failed`'s message when an Apply ended without the tab knowing what it wrote: its connection
/// was lost (or replaced by a reconnect) while it ran.
pub const UNKNOWN_OUTCOME: &str =
    "connection lost during Apply — the outcome is unknown. Reset and reload to check what was written, or Apply again to retry";

/// What a snapshot is counted for each value, new row and mark, beyond the text.
const ENTRY_BYTES: usize = 64;

/// How the last Apply failed (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    /// The grid row of the change that failed (or where a cancelled Apply stopped).
    pub row: usize,
    /// Its column, when that change wrote one column.
    pub column: Option<usize>,
    pub message: String,
    /// The error's code: `server`, `rowNotFound`, `readOnly`, `cancelled`, `connectionLost` (the
    /// outcome is unknown), ….
    pub code: &'static str,
}

/// The staged edits of one table tab (see the module docs). Undo and redo stacks are private.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edits {
    /// Changed cells of loaded rows, by `(row, column)`.
    pub cells: BTreeMap<(usize, usize), NewValue>,
    /// New rows, each with its staged values by column.
    pub new_rows: Vec<BTreeMap<usize, NewValue>>,
    /// Loaded rows to delete.
    pub deleted: BTreeSet<usize>,
    /// The last Apply's failure.
    pub failed: Option<Failed>,
    /// An Apply of these edits ended with an unknown outcome, and none went ahead since.
    unknown: bool,
    /// Changes whenever the staged values do (see [`Edits::revision`]).
    revision: Revision,
    undo: VecDeque<Arc<Staged>>,
    redo: Vec<Arc<Staged>>,
    /// What `undo` and `redo` hold, counted by [`Staged::bytes`].
    history_bytes: usize,
}

/// A stamp for the staged values: a number never given twice in the process, so a cache can tell
/// that they changed without comparing them. Always equal to another, so it takes no part in
/// comparing [`Edits`].
#[derive(Clone, Copy, Debug, Default)]
struct Revision(u64);

impl PartialEq for Revision {
    fn eq(&self, _: &Revision) -> bool {
        true
    }
}

impl Revision {
    fn bump(&mut self) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        self.0 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
}

/// One snapshot of the staged edits, shared between clones of the history.
#[derive(Clone, Debug, Default, PartialEq)]
struct Staged {
    cells: BTreeMap<(usize, usize), NewValue>,
    new_rows: Vec<BTreeMap<usize, NewValue>>,
    deleted: BTreeSet<usize>,
    /// What it counts for in the history.
    bytes: usize,
}

impl Staged {
    fn new(cells: BTreeMap<(usize, usize), NewValue>, new_rows: Vec<BTreeMap<usize, NewValue>>, deleted: BTreeSet<usize>) -> Staged {
        let value = |v: &NewValue| match v {
            NewValue::Null => ENTRY_BYTES,
            NewValue::Text(t) => ENTRY_BYTES.saturating_add(t.len()),
        };
        let values = |m: &BTreeMap<usize, NewValue>| m.values().fold(ENTRY_BYTES, |n, v| n.saturating_add(value(v)));
        let bytes =
            cells.values().map(value).chain(new_rows.iter().map(values)).fold(deleted.len().saturating_mul(ENTRY_BYTES), usize::saturating_add);
        Staged { cells, new_rows, deleted, bytes }
    }
}

/// The text of a staged value, `None` for NULL.
fn text(v: &NewValue) -> Option<&str> {
    match v {
        NewValue::Null => None,
        NewValue::Text(t) => Some(t),
    }
}

impl Edits {
    /// Staged changes: changed cells, new rows and rows to delete (a new row counts once, however
    /// many of its cells are set).
    pub fn count(&self) -> usize {
        self.cells.len().saturating_add(self.new_rows.len()).saturating_add(self.deleted.len())
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.new_rows.is_empty() && self.deleted.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// A stamp that is never the same for two different sets of staged values of the same page:
    /// every stage, add, delete, reset, undo and redo that changes them gives a new one (a few
    /// that change nothing visible may too). `0` for edits never changed. A cache of something
    /// derived from a shown value keys on it.
    pub fn revision(&self) -> u64 {
        self.revision.0
    }

    /// Rows in the grid: the loaded ones, then the new ones.
    pub fn grid_rows(&self, data: &ResultSet) -> usize {
        data.rows.len().saturating_add(self.new_rows.len())
    }

    /// The staged value of grid cell `(row, col)` when there is one: `Some(None)` for NULL.
    /// `None` means the loaded value shows, or for a new row its default.
    pub fn shown(&self, data: &ResultSet, row: usize, col: usize) -> Option<Option<&str>> {
        let value = match row.checked_sub(data.rows.len()) {
            None => self.cells.get(&(row, col)),
            Some(new) => self.new_rows.get(new)?.get(&col),
        };
        value.map(text)
    }

    /// The statements that write the edits in `dialect`, each with its grid row: updates, then
    /// inserts, then deletes. `NoKey` when the table has no row key or a loaded row lacks its key
    /// value, `UnknownColumn` when a staged column or a key column is not in both the page and
    /// `info`.
    pub fn statements(&self, dialect: Dialect, info: &TableInfo, data: &ResultSet) -> Result<Vec<(usize, String)>, SqlError> {
        if self.is_empty() {
            return Ok(Vec::new());
        }
        let key = info.row_key().ok_or(SqlError::NoKey)?;
        let column = |col: usize| -> Result<&ColumnInfo, SqlError> {
            let name = &data.columns.get(col).ok_or_else(|| SqlError::UnknownColumn(format!("#{col}")))?.name;
            info.column(name).ok_or_else(|| SqlError::UnknownColumn(name.clone()))
        };
        let key_of = |row: usize| -> Result<Vec<(&ColumnInfo, &str)>, SqlError> {
            let values = data.rows.get(row).ok_or(SqlError::NoKey)?;
            key.iter()
                .map(|k| {
                    let at = data.columns.iter().position(|c| c.name == k.name).ok_or_else(|| SqlError::UnknownColumn(k.name.clone()))?;
                    match values.get(at) {
                        Some(Some(v)) => Ok((*k, v.as_str())),
                        _ => Err(SqlError::NoKey),
                    }
                })
                .collect()
        };
        let mut out = Vec::with_capacity(self.count());
        let mut changed: BTreeMap<usize, Vec<(&ColumnInfo, &NewValue)>> = BTreeMap::new();
        for (&(row, col), v) in &self.cells {
            if !self.deleted.contains(&row) {
                changed.entry(row).or_default().push((column(col)?, v));
            }
        }
        for (row, changes) in changed {
            out.push((row, update_sql(dialect, info, &key_of(row)?, &changes)?));
        }
        for (i, values) in self.new_rows.iter().enumerate() {
            let values = values.iter().map(|(&col, v)| Ok((column(col)?, v))).collect::<Result<Vec<_>, SqlError>>()?;
            out.push((data.rows.len().saturating_add(i), insert_sql(dialect, info, &values)?));
        }
        for &row in &self.deleted {
            out.push((row, delete_sql(dialect, info, &key_of(row)?)?));
        }
        Ok(out)
    }

    /// The one column the change on grid `row` writes, when it writes exactly one (`loaded` is
    /// the page's row count).
    pub(crate) fn only_column(&self, loaded: usize, row: usize) -> Option<usize> {
        let mut columns: Vec<usize> = match row.checked_sub(loaded) {
            None if self.deleted.contains(&row) => return None,
            None => self.cells.range((row, 0)..=(row, usize::MAX)).map(|(&(_, c), _)| c).collect(),
            Some(new) => self.new_rows.get(new)?.keys().copied().collect(),
        };
        if columns.len() == 1 { columns.pop() } else { None }
    }

    /// The grid row of the first statement, when anything is staged.
    pub(crate) fn first_row(&self, loaded: usize) -> Option<usize> {
        let updated = self.cells.keys().map(|&(row, _)| row).find(|row| !self.deleted.contains(row));
        let inserted = (!self.new_rows.is_empty()).then_some(loaded);
        updated.or(inserted).or_else(|| self.deleted.first().copied())
    }

    /// The Apply of these edits ended and nobody knows what it wrote: `failed` says so, at the
    /// first change, and the outcome is doubted (when anything is staged).
    pub(crate) fn mark_unknown(&mut self, loaded: usize) {
        match self.first_row(loaded) {
            Some(row) => self.unknown_at(row),
            None => self.failed = None,
        }
    }

    /// [`Edits::mark_unknown`] at grid row `row`.
    pub(crate) fn unknown_at(&mut self, row: usize) {
        self.failed = Some(Failed { row, column: None, message: UNKNOWN_OUTCOME.to_owned(), code: "connectionLost" });
        self.unknown = true;
    }

    /// An Apply of these edits ended with an unknown outcome and none went ahead since, so applying
    /// again could write them twice (see the module docs).
    pub fn outcome_unknown(&self) -> bool {
        self.unknown
    }

    /// An Apply of these edits goes ahead: the last one's failure and doubt are over.
    pub(crate) fn apply_started(&mut self) {
        self.failed = None;
        self.unknown = false;
    }

    /// Stages `value` for grid cell `(row, col)`; `true` when that changed the edits. `Err` names
    /// what is wrong with the cell or the value (outside the grid, a row marked for deletion, a
    /// column Postgres computes, a NUL character). `column` is the cell's column in the catalog,
    /// when it is there.
    pub(crate) fn stage(&mut self, data: &ResultSet, row: usize, col: usize, value: NewValue, column: Option<&ColumnInfo>) -> Result<bool, String> {
        if col >= data.columns.len() {
            return Err(format!("the page has no column {col}"));
        }
        if let Some(why) = column.and_then(ColumnInfo::not_editable) {
            return Err(why.to_owned());
        }
        if matches!(&value, NewValue::Text(t) if t.contains('\0')) {
            return Err(SqlError::Nul.to_string());
        }
        match row.checked_sub(data.rows.len()) {
            None => {
                if self.deleted.contains(&row) {
                    return Err(format!("row {row} is marked for deletion"));
                }
                let loaded = data.rows.get(row).and_then(|r| r.get(col)).ok_or_else(|| format!("row {row} is shorter than the page's columns"))?;
                let unstage = text(&value) == loaded.as_deref();
                if unstage {
                    if !self.cells.contains_key(&(row, col)) {
                        return Ok(false);
                    }
                    self.record();
                    self.cells.remove(&(row, col));
                } else {
                    if self.cells.get(&(row, col)) == Some(&value) {
                        return Ok(false);
                    }
                    self.record();
                    self.cells.insert((row, col), value);
                }
            }
            Some(new) => {
                let current = self.new_rows.get(new).ok_or_else(|| format!("row {row} is not in the grid"))?;
                if current.get(&col) == Some(&value) {
                    return Ok(false);
                }
                self.record();
                if let Some(values) = self.new_rows.get_mut(new) {
                    values.insert(col, value);
                }
            }
        }
        self.settle();
        Ok(true)
    }

    /// Adds an empty new row; returns its grid row.
    pub(crate) fn add_row(&mut self, data: &ResultSet) -> usize {
        self.record();
        self.new_rows.push(BTreeMap::new());
        self.grid_rows(data).saturating_sub(1)
    }

    /// Deletes grid `rows`: new rows go, loaded rows are marked (and lose their staged cells).
    /// `true` when that changed the edits; `Err` when a row is not in the grid (then nothing
    /// changes).
    pub(crate) fn delete_rows(&mut self, data: &ResultSet, rows: &[usize]) -> Result<bool, String> {
        let loaded = data.rows.len();
        let grid = self.grid_rows(data);
        if let Some(row) = rows.iter().find(|&&r| r >= grid) {
            return Err(format!("row {row} is not in the grid"));
        }
        let mark: BTreeSet<usize> = rows.iter().copied().filter(|&r| r < loaded && !self.deleted.contains(&r)).collect();
        let gone: BTreeSet<usize> = rows.iter().filter_map(|&r| r.checked_sub(loaded)).collect();
        if mark.is_empty() && gone.is_empty() {
            return Ok(false);
        }
        self.record();
        for &row in &mark {
            self.cells.retain(|&(r, _), _| r != row);
        }
        self.deleted.extend(mark);
        // From the last, so the indexes still to take out do not move.
        for &new in gone.iter().rev() {
            if new < self.new_rows.len() {
                self.new_rows.remove(new);
            }
        }
        // A failed new row that went takes its mark along; one after deleted rows moves up.
        if let Some(new) = self.failed.as_ref().and_then(|f| f.row.checked_sub(loaded)) {
            if gone.contains(&new) {
                self.failed = None;
            } else if let Some(f) = &mut self.failed {
                f.row = f.row.saturating_sub(gone.range(..new).count());
            }
        }
        self.settle();
        Ok(true)
    }

    /// Drops every staged change (an undo step); `false` when nothing was staged.
    pub(crate) fn reset(&mut self) -> bool {
        if self.is_empty() {
            return false;
        }
        self.record();
        self.cells.clear();
        self.new_rows.clear();
        self.deleted.clear();
        self.settle();
        true
    }

    /// Back to the edits before the last change; `false` when there is none.
    pub(crate) fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else { return false };
        self.revision.bump();
        let current = self.take_current();
        self.history_bytes = self.history_bytes.saturating_sub(previous.bytes).saturating_add(current.bytes);
        self.redo.push(Arc::new(current));
        self.restore(previous);
        true
    }

    /// Forward again to the edits the last undo left; `false` when there is none.
    pub(crate) fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.revision.bump();
        let current = self.take_current();
        self.history_bytes = self.history_bytes.saturating_sub(next.bytes).saturating_add(current.bytes);
        self.undo.push_back(Arc::new(current));
        self.restore(next);
        true
    }

    /// Before a change: the edits as they are become an undo step, and nothing can be redone.
    fn record(&mut self) {
        self.revision.bump();
        let snapshot = Staged::new(self.cells.clone(), self.new_rows.clone(), self.deleted.clone());
        self.history_bytes = self.history_bytes.saturating_add(snapshot.bytes);
        self.undo.push_back(Arc::new(snapshot));
        for gone in self.redo.drain(..) {
            self.history_bytes = self.history_bytes.saturating_sub(gone.bytes);
        }
        while self.undo.len() > UNDO_STEPS || self.history_bytes > UNDO_BYTES {
            let Some(oldest) = self.undo.pop_front() else { break };
            self.history_bytes = self.history_bytes.saturating_sub(oldest.bytes);
        }
    }

    fn take_current(&mut self) -> Staged {
        Staged::new(std::mem::take(&mut self.cells), std::mem::take(&mut self.new_rows), std::mem::take(&mut self.deleted))
    }

    fn restore(&mut self, snapshot: Arc<Staged>) {
        let s = Arc::try_unwrap(snapshot).unwrap_or_else(|shared| (*shared).clone());
        self.cells = s.cells;
        self.new_rows = s.new_rows;
        self.deleted = s.deleted;
        self.settle();
    }

    /// Nothing staged: nothing failed either.
    fn settle(&mut self) {
        if self.is_empty() {
            self.failed = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dbee_core::{Column, ColumnInfo, RelKind, ResultSet, TableInfo, TableRef, TypeClass};
    use dbee_sql::SqlError;
    use dbee_sql::edit::NewValue;

    use super::*;

    fn col(name: &str, type_sql: &str, base: &str, category: char) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_sql: type_sql.into(),
            base_type: base.into(),
            class: match category {
                'N' => TypeClass::Number,
                'S' => TypeClass::Text,
                _ => TypeClass::Other,
            },
            not_null: name == "id",
            has_default: false,
            generated: Default::default(),
            hidden: false,
        }
    }

    fn info() -> TableInfo {
        TableInfo {
            table: TableRef { schema: "s".into(), name: "t".into() },
            kind: RelKind::Table,
            columns: vec![col("id", "integer", "int4", 'N'), col("name", "text", "text", 'S'), col("total", "numeric(12,2)", "numeric", 'N')],
            primary_key: vec!["id".into()],
            unique_keys: vec![],
            foreign_keys: vec![],
            estimated_rows: None,
            no_edit_reason: None,
        }
    }

    /// Two loaded rows: `(1, 'a', 1.00)` and `(2, NULL, 2.00)`.
    fn data() -> ResultSet {
        let cell = |v: &str| Some(v.to_owned());
        ResultSet {
            columns: info().columns.iter().map(ColumnInfo::column).collect(),
            rows: vec![vec![cell("1"), cell("a"), cell("1.00")], vec![cell("2"), None, cell("2.00")]],
            rows_seen: 2,
            complete: true,
            rows_affected: None,
        }
    }

    fn text(v: &str) -> NewValue {
        NewValue::Text(v.into())
    }

    #[test]
    fn staging_and_unstaging() {
        let d = data();
        let mut e = Edits::default();
        assert!(e.is_empty() && !e.can_undo() && !e.can_redo());
        assert_eq!(e.stage(&d, 0, 1, text("b"), None), Ok(true));
        assert_eq!(e.count(), 1);
        assert_eq!(e.stage(&d, 0, 1, text("b"), None), Ok(false), "the same value again changes nothing");
        // The loaded value unstages the cell: text for text, NULL for NULL.
        assert_eq!(e.stage(&d, 0, 1, text("a"), None), Ok(true));
        assert!(e.is_empty());
        assert_eq!(e.stage(&d, 1, 1, NewValue::Null, None), Ok(false));
        assert_eq!(e.stage(&d, 1, 1, text(""), None), Ok(true), "an empty string is not NULL");
        assert_eq!(e.cells.get(&(1, 1)), Some(&text("")));
        // Out of the grid, or on a column the page does not have.
        assert!(e.stage(&d, 2, 0, text("x"), None).is_err());
        assert!(e.stage(&d, 0, 3, text("x"), None).is_err());
        assert!(e.stage(&d, 0, 1, text("nul\0"), None).is_err());
        assert_eq!(e.count(), 1);
    }

    #[test]
    fn shown_gives_staged_values_only() {
        let d = data();
        let mut e = Edits::default();
        e.stage(&d, 0, 1, text("b"), None).unwrap();
        e.stage(&d, 0, 2, NewValue::Null, None).unwrap();
        let row = e.add_row(&d);
        assert_eq!(row, 2);
        e.stage(&d, row, 1, text("new"), None).unwrap();
        assert_eq!(e.shown(&d, 0, 1), Some(Some("b")));
        assert_eq!(e.shown(&d, 0, 2), Some(None));
        assert_eq!(e.shown(&d, 0, 0), None);
        assert_eq!(e.shown(&d, 2, 1), Some(Some("new")));
        assert_eq!(e.shown(&d, 2, 0), None, "an unstaged cell of a new row gets the default");
        assert_eq!(e.shown(&d, 9, 9), None);
        assert_eq!(e.grid_rows(&d), 3);
    }

    #[test]
    fn statements_update_then_insert_then_delete() {
        let (i, d) = (info(), data());
        let mut e = Edits::default();
        let new = e.add_row(&d);
        e.stage(&d, new, 0, text("9"), None).unwrap();
        e.stage(&d, new, 2, NewValue::Null, None).unwrap();
        e.add_row(&d);
        e.delete_rows(&d, &[1]).unwrap();
        // Editing the key: the row is found by the key it was loaded with.
        e.stage(&d, 0, 0, text("10"), None).unwrap();
        e.stage(&d, 0, 1, text("it's"), None).unwrap();
        assert_eq!(e.count(), 5);
        let s = e.statements(Dialect::Postgres, &i, &d).unwrap();
        assert_eq!(
            s,
            [
                (
                    0,
                    r#"UPDATE "s"."t" SET "id" = CAST('10' AS integer), "name" = CAST('it''s' AS text) WHERE "id" = CAST('1' AS integer)"#.to_owned()
                ),
                (2, r#"INSERT INTO "s"."t" ("id", "total") VALUES (CAST('9' AS integer), NULL)"#.to_owned()),
                (3, r#"INSERT INTO "s"."t" DEFAULT VALUES"#.to_owned()),
                (1, r#"DELETE FROM "s"."t" WHERE "id" = CAST('2' AS integer)"#.to_owned()),
            ]
        );
        assert_eq!((e.only_column(2, 0), e.only_column(2, 2), e.only_column(2, 3), e.only_column(2, 1)), (None, None, None, None));
        let mut one = Edits::default();
        one.stage(&d, 1, 2, text("3"), None).unwrap();
        assert_eq!(one.only_column(2, 1), Some(2));
        assert_eq!(Edits::default().statements(Dialect::Postgres, &i, &d), Ok(vec![]));
        // The same edits in another engine's SQL.
        let mysql = e.statements(Dialect::MySql, &i, &d).unwrap();
        assert!(mysql.iter().all(|(_, sql)| sql.contains("`s`.`t`")), "{mysql:?}");
        assert_eq!(mysql.get(2).map(|(_, sql)| sql.as_str()), Some("INSERT INTO `s`.`t` () VALUES ()"));
    }

    #[test]
    fn statements_need_a_key_from_the_loaded_row() {
        let d = data();
        let mut e = Edits::default();
        e.stage(&d, 0, 1, text("b"), None).unwrap();
        let mut keyless = info();
        keyless.primary_key.clear();
        assert_eq!(e.statements(Dialect::Postgres, &keyless, &d), Err(SqlError::NoKey));
        // A key value the page does not have, or a NULL one, cannot address the row.
        let mut null_key = d.clone();
        null_key.rows[0][0] = None;
        assert_eq!(e.statements(Dialect::Postgres, &info(), &null_key), Err(SqlError::NoKey));
        // A column the catalog no longer has.
        let mut renamed = info();
        renamed.columns[1].name = "label".into();
        assert_eq!(e.statements(Dialect::Postgres, &renamed, &d), Err(SqlError::UnknownColumn("name".into())));
    }

    #[test]
    fn deleting_rows() {
        let d = data();
        let mut e = Edits::default();
        let (a, b) = (e.add_row(&d), e.add_row(&d));
        e.stage(&d, b, 1, text("second"), None).unwrap();
        e.stage(&d, 0, 1, text("x"), None).unwrap();
        // A new row goes; the rows after it move up. A loaded row is marked, and its changes go.
        assert_eq!(e.delete_rows(&d, &[a, 0, 0]), Ok(true));
        assert_eq!(e.new_rows, vec![BTreeMap::from([(1, text("second"))])]);
        assert_eq!(e.shown(&d, 2, 1), Some(Some("second")));
        assert!(e.cells.is_empty());
        assert_eq!(e.deleted.iter().copied().collect::<Vec<_>>(), [0]);
        assert_eq!(e.count(), 2);
        assert_eq!(e.delete_rows(&d, &[0]), Ok(false), "already marked");
        assert!(e.stage(&d, 0, 1, text("y"), None).is_err(), "a row marked for deletion is not edited");
        assert!(e.delete_rows(&d, &[3]).is_err());
        // Several new rows at once, given in any order.
        let mut e = Edits::default();
        for _ in 0..4 {
            e.add_row(&d);
        }
        e.stage(&d, 4, 1, text("kept"), None).unwrap();
        assert_eq!(e.delete_rows(&d, &[5, 2, 3]), Ok(true));
        assert_eq!(e.new_rows, vec![BTreeMap::from([(1, text("kept"))])]);
    }

    #[test]
    fn undo_redo_walk_the_snapshots() {
        let d = data();
        let mut e = Edits::default();
        e.stage(&d, 0, 1, text("1"), None).unwrap();
        e.add_row(&d);
        e.delete_rows(&d, &[1]).unwrap();
        assert_eq!(e.count(), 3);
        assert!(e.undo());
        assert_eq!(e.count(), 2);
        assert!(e.undo() && e.undo());
        assert!(e.is_empty() && !e.undo());
        assert!(e.redo() && e.redo() && e.redo() && !e.redo());
        assert_eq!(e.count(), 3);
        // A change drops what could be redone; a reset can be undone.
        e.undo();
        e.stage(&d, 1, 2, text("5"), None).unwrap();
        assert!(!e.can_redo());
        assert!(e.reset());
        assert!(e.is_empty() && !e.reset());
        assert!(e.undo());
        assert_eq!(e.count(), 3);
        // A change that changes nothing is no undo step.
        let mut e = Edits::default();
        e.stage(&d, 0, 1, text("a"), None).unwrap();
        assert!(!e.can_undo());
    }

    #[test]
    fn failed_is_kept_until_the_edits_are_gone() {
        let d = data();
        let mut e = Edits::default();
        e.stage(&d, 0, 1, text("1"), None).unwrap();
        e.stage(&d, 1, 1, text("2"), None).unwrap();
        e.failed = Some(Failed { row: 1, column: Some(1), message: "bad".into(), code: "server" });
        e.stage(&d, 0, 2, text("3"), None).unwrap();
        assert!(e.failed.is_some(), "fixing another cell keeps the mark");
        e.reset();
        assert_eq!(e.failed, None);
        // The unknown outcome marks the first change.
        e.undo();
        e.mark_unknown(d.rows.len());
        assert_eq!(e.failed, Some(Failed { row: 0, column: None, message: UNKNOWN_OUTCOME.to_owned(), code: "connectionLost" }));
        while e.undo() {}
        assert_eq!(e.failed, None, "nothing staged, nothing failed");
        e.mark_unknown(d.rows.len());
        assert_eq!(e.failed, None);
        // A new row before the failed one moves it up when it goes.
        let mut e = Edits::default();
        let (a, b) = (e.add_row(&d), e.add_row(&d));
        e.failed = Some(Failed { row: b, column: None, message: "bad".into(), code: "server" });
        e.delete_rows(&d, &[a]).unwrap();
        assert_eq!(e.failed.as_ref().map(|f| f.row), Some(a));
        e.delete_rows(&d, &[a]).unwrap();
        assert_eq!(e.failed, None);
    }

    #[test]
    fn an_unknown_outcome_is_doubted_until_the_next_apply() {
        let d = data();
        let mut e = Edits::default();
        e.mark_unknown(d.rows.len());
        assert!(!e.outcome_unknown(), "nothing staged, nothing was being written");
        e.stage(&d, 0, 1, text("1"), None).unwrap();
        e.mark_unknown(d.rows.len());
        assert!(e.outcome_unknown());
        // The mark goes with a reset; the doubt stays, also for the edits an undo brings back.
        e.reset();
        assert_eq!((e.failed.clone(), e.outcome_unknown()), (None, true));
        e.undo();
        assert!(e.outcome_unknown());
        e.stage(&d, 1, 1, text("2"), None).unwrap();
        assert!(e.outcome_unknown());
        // An Apply that goes ahead clears it.
        e.apply_started();
        assert_eq!((e.failed.clone(), e.outcome_unknown()), (None, false));
        assert_eq!(e.count(), 2);
        assert!(!Edits::default().outcome_unknown());
    }

    #[test]
    fn history_is_capped() {
        let d = data();
        let mut e = Edits::default();
        for i in 0..UNDO_STEPS + 10 {
            e.stage(&d, 0, 1, text(&i.to_string()), None).unwrap();
        }
        let mut undos = 0;
        while e.undo() {
            undos += 1;
        }
        assert_eq!(undos, UNDO_STEPS);
        // Huge values: the oldest snapshots go first.
        let mut e = Edits::default();
        let big = "x".repeat(UNDO_BYTES / 4);
        for i in 0..8 {
            e.stage(&d, 0, 1, text(&format!("{i}{big}")), None).unwrap();
        }
        let mut undos = 0;
        while e.undo() {
            undos += 1;
        }
        assert!((1..8).contains(&undos), "{undos}");
        assert!(!e.is_empty(), "the oldest steps were dropped");
    }

    #[test]
    fn untyped_pages_stage_too() {
        // `shown` works on any page, also one with columns of no known type.
        let d = ResultSet { columns: vec![Column::untyped("x")], rows: vec![vec![None]], ..ResultSet::default() };
        let mut e = Edits::default();
        e.stage(&d, 0, 0, text("1"), None).unwrap();
        assert_eq!(e.shown(&d, 0, 0), Some(Some("1")));
    }
}

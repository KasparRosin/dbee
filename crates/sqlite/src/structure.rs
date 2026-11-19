//! A table's indexes, triggers, column defaults and foreign key actions, read when asked:
//! `pragma_index_list` and `pragma_index_xinfo` for each index, `sqlite_schema`'s statements for
//! what the pragmas leave out, `pragma_table_xinfo` for the defaults and `pragma_foreign_key_list`
//! for the actions.
//!
//! A default is `dflt_value` as written (`'new'`, `CURRENT_TIMESTAMP`); a generated column's is
//! `generated`, its expression not read (SQLite keeps it only in the `CREATE TABLE` statement).
//!
//! The pragmas give an index's columns, order and uniqueness, and whether it is partial, but not
//! the text of an expression part or of the condition; SQLite keeps those only in the index's
//! `CREATE INDEX` statement, which [`index_statement`] reads with the SQL lexer. A trigger's
//! timing and event are likewise read from its `CREATE TRIGGER` statement ([`trigger_head`]);
//! SQLite's triggers are always `FOR EACH ROW`. Text that cannot be read that way leaves the
//! expression, condition or timing out (`None`), never an error.
//!
//! An `INTEGER PRIMARY KEY` is the rowid and has no index, so such a table lists none for its key.
//! Only `main` is read, as for the catalog.

use std::ops::Range;

use dbee_core::{ColumnDefault, ForeignKeyActions, IndexPart, TableRef, TableStructure, TriggerEvent, TriggerTiming};
use dbee_driver::DbError;
use dbee_driver::catalog::{number, owned, text};
use dbee_driver::structure::{self, IndexRow, TriggerRow};
use dbee_sql::{Dialect, TokenKind, lexer};
use rusqlite::ffi;

use crate::catalog::{SCHEMA, flag, literal, nullable, query};
use crate::connect::{Running, Session, SqliteConnection};
use crate::errors;

impl SqliteConnection {
    /// The indexes, triggers, defaults and foreign key actions of `table`; none for a schema other
    /// than `main`.
    pub async fn table_structure(&self, table: &TableRef) -> Result<TableStructure, DbError> {
        if table.schema != SCHEMA {
            return Ok(TableStructure::default());
        }
        let name = table.name.clone();
        let _turn = self.lock().await?;
        self.blocking(move |s, running| read(s, running, &name)).await
    }
}

fn read(s: &mut Session, running: &Running, table: &str) -> Result<TableStructure, DbError> {
    let quoted = literal(table)?;
    let mut indexes = Vec::new();
    let list = query(s, running, &format!("SELECT name, \"unique\", origin, partial FROM pragma_index_list({quoted}, 'main')"))?;
    for row in &list {
        let name = owned(row, 0, "index name")?;
        let index = literal(&name)?;
        let statement = query(s, running, &format!("SELECT sql FROM main.sqlite_schema WHERE type = 'index' AND name = {index}"))?;
        let statement = statement.first().and_then(|row| row.first()).and_then(|c| c.as_deref()).and_then(index_statement);
        let parts =
            query(s, running, &format!("SELECT seqno, cid, name, \"desc\" FROM pragma_index_xinfo({index}, 'main') WHERE key ORDER BY seqno"))?;
        let texts = statement.as_ref().map(|(texts, _)| texts).filter(|texts| texts.len() == parts.len());
        let predicate = if flag(row, 3, "partial index")? { statement.as_ref().and_then(|(_, p)| p.clone()) } else { None };
        for (i, part) in parts.iter().enumerate() {
            let cid: i64 = number(part, 1, "index column id")?;
            let column = nullable(part, 2, "index column")?.map(str::to_owned);
            let expression = if cid == -2 { texts.and_then(|t| t.get(i)).cloned() } else { None };
            indexes.push(IndexRow {
                name: name.clone(),
                unique: flag(row, 1, "unique index")?,
                primary: text(row, 2, "index origin")? == "pk",
                method: None,
                predicate: predicate.clone(),
                position: number(part, 0, "index position")?,
                part: IndexPart { column, expression, descending: flag(part, 3, "descending")? },
            });
        }
    }
    let mut triggers = Vec::new();
    let sql = format!("SELECT name, sql FROM main.sqlite_schema WHERE type = 'trigger' AND lower(tbl_name) = lower({quoted})");
    for row in query(s, running, &sql)? {
        let definition = nullable(&row, 1, "trigger statement")?.unwrap_or_default().to_owned();
        let (timing, events) = trigger_head(&definition).map_or((None, Vec::new()), |(t, e)| (Some(t), e.into_iter().collect()));
        triggers.push(TriggerRow { name: owned(&row, 0, "trigger name")?, timing, events, for_each_row: Some(true), definition });
    }
    let built = structure::build(indexes, triggers);
    Ok(structure::with_keys(built, defaults(s, running, &quoted)?, actions(s, running, &quoted)?))
}

/// Each column's default: `generated` for a generated column (`hidden` 2 or 3), else `dflt_value`.
/// A virtual table's hidden columns (`hidden` 1) are left out.
fn defaults(s: &mut Session, running: &Running, quoted: &str) -> Result<Vec<ColumnDefault>, DbError> {
    let rows = match query(s, running, &format!("SELECT name, dflt_value, hidden FROM pragma_table_xinfo({quoted}, 'main') ORDER BY cid")) {
        Ok(rows) => rows,
        // A view whose statement no longer reads (a table it names was dropped): no columns, as
        // the catalog reads it.
        Err(e) if errors::is_code(&e, ffi::SQLITE_ERROR) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for row in rows {
        let hidden: u32 = number(&row, 2, "hidden")?;
        let default = match hidden {
            1 => continue,
            2 | 3 => Some("generated".to_owned()),
            _ => nullable(&row, 1, "column default")?.map(str::to_owned),
        };
        out.push(ColumnDefault { column: owned(&row, 0, "column name")?, default });
    }
    Ok(out)
}

/// Each foreign key's actions, by its columns in key order and the table it references (named as
/// the statement spells it; the engine matches the catalog's key ignoring ASCII case).
fn actions(s: &mut Session, running: &Running, quoted: &str) -> Result<Vec<ForeignKeyActions>, DbError> {
    let sql = format!("SELECT id, \"table\", \"from\", on_delete, on_update FROM pragma_foreign_key_list({quoted}, 'main') ORDER BY id, seq");
    let mut out: Vec<(i64, ForeignKeyActions)> = Vec::new();
    for row in query(s, running, &sql)? {
        let id: i64 = number(&row, 0, "foreign key id")?;
        let column = owned(&row, 2, "foreign key column")?;
        match out.last_mut() {
            Some((last, key)) if *last == id => key.columns.push(column),
            _ => out.push((
                id,
                ForeignKeyActions {
                    columns: vec![column],
                    target: TableRef { schema: SCHEMA.into(), name: owned(&row, 1, "referenced table")? },
                    on_delete: nullable(&row, 3, "on delete")?.and_then(structure::fk_action),
                    on_update: nullable(&row, 4, "on update")?.and_then(structure::fk_action),
                },
            )),
        }
    }
    Ok(out.into_iter().map(|(_, key)| key).collect())
}

/// The tokens of `sql` that are not whitespace or comments, with their text.
fn significant(sql: &str) -> Vec<(TokenKind, Range<usize>, &str)> {
    lexer::tokenize(Dialect::Sqlite, sql)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .filter_map(|t| sql.get(t.range.clone()).map(|s| (t.kind, t.range, s)))
        .collect()
}

/// A token that is the word `w`, unquoted, in any ASCII case.
fn is_word(token: Option<&(TokenKind, Range<usize>, &str)>, w: &str) -> bool {
    token.is_some_and(|(kind, _, s)| matches!(kind, TokenKind::Keyword | TokenKind::Ident) && s.eq_ignore_ascii_case(w))
}

fn is(token: Option<&(TokenKind, Range<usize>, &str)>, punct: &str) -> bool {
    token.is_some_and(|(_, _, s)| *s == punct)
}

/// A `CREATE INDEX` statement's parts as written (an `ASC` or `DESC` after one left out) and its
/// `WHERE` condition; `None` when the statement does not read as one.
fn index_statement(sql: &str) -> Option<(Vec<String>, Option<String>)> {
    let tokens = significant(sql);
    let on = tokens.iter().position(|t| is_word(Some(t), "ON"))?;
    let open = on + tokens.get(on..)?.iter().position(|t| is(Some(t), "("))?;
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = open.checked_add(1)?;
    let mut close = None;
    for (i, t) in tokens.iter().enumerate().skip(open) {
        match t.2 {
            "(" => depth = depth.checked_add(1)?,
            ")" | "," if depth == 1 => {
                let mut end = i;
                if end > start && (is_word(tokens.get(end - 1), "ASC") || is_word(tokens.get(end - 1), "DESC")) {
                    end -= 1;
                }
                if end <= start {
                    return None; // an empty part
                }
                let (first, last) = (tokens.get(start)?, tokens.get(end - 1)?);
                parts.push(sql.get(first.1.start..last.1.end)?.to_owned());
                start = i + 1;
                if t.2 == ")" {
                    close = Some(i);
                    break;
                }
            }
            ")" => depth = depth.checked_sub(1)?,
            _ => {}
        }
    }
    let close = close?;
    let predicate = match tokens.get(close + 1) {
        w if is_word(w, "WHERE") => {
            let from = w?.1.end;
            let mut until = sql.len();
            if let Some(last) = tokens.last().filter(|t| t.2 == ";") {
                until = last.1.start;
            }
            Some(sql.get(from..until)?.trim().to_owned()).filter(|p| !p.is_empty())
        }
        _ => None,
    };
    Some((parts, predicate))
}

/// A `CREATE TRIGGER` statement's timing (`BEFORE` when it names none, as SQLite reads it) and
/// event; `None` when the statement does not read as one.
fn trigger_head(sql: &str) -> Option<(TriggerTiming, Option<TriggerEvent>)> {
    let tokens = significant(sql);
    let mut i = tokens.iter().position(|t| is_word(Some(t), "TRIGGER"))? + 1;
    if is_word(tokens.get(i), "IF") {
        i += 3; // IF NOT EXISTS
    }
    i += 1; // the name
    if is(tokens.get(i), ".") {
        i += 2; // schema . name
    }
    let timing = if is_word(tokens.get(i), "BEFORE") {
        i += 1;
        TriggerTiming::Before
    } else if is_word(tokens.get(i), "AFTER") {
        i += 1;
        TriggerTiming::After
    } else if is_word(tokens.get(i), "INSTEAD") && is_word(tokens.get(i + 1), "OF") {
        i += 2;
        TriggerTiming::InsteadOf
    } else {
        TriggerTiming::Before
    };
    let event = tokens.get(i).filter(|t| matches!(t.0, TokenKind::Keyword | TokenKind::Ident)).and_then(|t| structure::event(t.2));
    Some((timing, event.filter(|e| *e != TriggerEvent::Truncate)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(sql: &str) -> Option<(Vec<String>, Option<String>)> {
        index_statement(sql)
    }

    #[test]
    fn index_statements_read_their_parts_and_condition() {
        let got =
            parts("CREATE UNIQUE INDEX \"i (x)\" ON \"t(\" (lower(a, ','), b DESC, \"c\" COLLATE nocase ASC) WHERE a > 0 AND (b, c) IS NOT NULL;");
        let want = (vec!["lower(a, ',')".to_string(), "b".into(), "\"c\" COLLATE nocase".into()], Some("a > 0 AND (b, c) IS NOT NULL".to_string()));
        assert_eq!(got, Some(want));
        assert_eq!(parts("create index i on t(a) -- note"), Some((vec!["a".to_string()], None)));
        assert_eq!(parts("CREATE INDEX i ON t /* ( */ (a)\nwhere\n b"), Some((vec!["a".to_string()], Some("b".to_string()))));
    }

    #[test]
    fn unreadable_index_statements_give_none() {
        for sql in ["", "CREATE INDEX i", "CREATE INDEX i ON t (a", "CREATE INDEX i ON t ((a)", ")))", "ON ( , ) )"] {
            let got = parts(sql);
            assert!(got.is_none() || got.as_ref().is_some_and(|(p, _)| p.iter().all(|p| !p.is_empty())), "{sql:?} {got:?}");
        }
        assert_eq!(parts("CREATE INDEX i ON t ()"), None);
    }

    #[test]
    fn trigger_heads_read_timing_and_event() {
        use TriggerEvent::*;
        use TriggerTiming::*;
        let cases = [
            ("CREATE TRIGGER t AFTER INSERT ON x BEGIN SELECT 1; END", Some((After, Some(Insert)))),
            ("create temp trigger if not exists main.\"before\" instead of update of a on v begin select 1; end", Some((InsteadOf, Some(Update)))),
            ("CREATE TRIGGER [after] DELETE ON x BEGIN SELECT 1; END", Some((Before, Some(Delete)))),
            ("CREATE TRIGGER before BEFORE /* c */ UPDATE ON x BEGIN SELECT 1; END", Some((Before, Some(Update)))),
            ("CREATE TABLE x (a)", None),
            ("TRIGGER", Some((Before, None))),
        ];
        for (sql, want) in cases {
            assert_eq!(trigger_head(sql), want, "{sql}");
        }
    }
}

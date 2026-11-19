//! Building a [`TableStructure`] from a driver's rows: one row per index part, one per trigger
//! (or per trigger and event), put together the same way for every engine ([`build`]); then each
//! column's default and each foreign key's actions ([`with_keys`]).
//!
//! Server text is cut where it is kept: an index expression, predicate or column default to
//! [`MAX_EXPRESSION`] bytes, a trigger's definition to [`MAX_DEFINITION`] ([`kept_text`]). The queries run within the
//! catalog's budget ([`crate::CatalogBudget`]), and a cut result is an error, never a partial list.

use std::collections::HashMap;

use dbee_core::{ColumnDefault, FkAction, ForeignKeyActions, IndexInfo, IndexPart, TableStructure, TriggerEvent, TriggerInfo, TriggerTiming};

use crate::kept_text;

/// The most of an index expression or predicate dbee keeps.
pub const MAX_EXPRESSION: usize = 8 * 1024;

/// The most of a trigger's definition dbee keeps.
pub const MAX_DEFINITION: usize = 64 * 1024;

/// Part `position` of index `name` (any numbers that grow in index order). The index's own facts
/// are taken from its first row.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexRow {
    pub name: String,
    pub unique: bool,
    pub primary: bool,
    pub method: Option<String>,
    pub predicate: Option<String>,
    pub position: u32,
    pub part: IndexPart,
}

/// Trigger `name`, or one of its events: rows that share a name make one trigger, its events
/// merged, its other facts from the first row.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerRow {
    pub name: String,
    pub timing: Option<TriggerTiming>,
    pub events: Vec<TriggerEvent>,
    pub for_each_row: Option<bool>,
    pub definition: String,
}

/// The structure: indexes with their parts in position order, the primary key's first and then by
/// name; triggers by name, each one's events in the order insert, update, delete, truncate.
pub fn build(indexes: Vec<IndexRow>, triggers: Vec<TriggerRow>) -> TableStructure {
    let mut built: Vec<(IndexInfo, Vec<(u32, IndexPart)>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for row in indexes {
        let part = IndexPart { expression: row.part.expression.map(|e| kept_text(&e, MAX_EXPRESSION)), ..row.part };
        match at.get(&row.name).and_then(|&i| built.get_mut(i)) {
            Some((_, parts)) => parts.push((row.position, part)),
            None => {
                at.insert(row.name.clone(), built.len());
                let info = IndexInfo {
                    name: row.name,
                    parts: Vec::new(),
                    unique: row.unique,
                    primary: row.primary,
                    method: row.method,
                    predicate: row.predicate.map(|p| kept_text(&p, MAX_EXPRESSION)),
                };
                built.push((info, vec![(row.position, part)]));
            }
        }
    }
    let mut indexes: Vec<IndexInfo> = built
        .into_iter()
        .map(|(mut info, mut parts)| {
            parts.sort_by_key(|(position, _)| *position);
            info.parts = parts.into_iter().map(|(_, part)| part).collect();
            info
        })
        .collect();
    indexes.sort_by(|a, b| b.primary.cmp(&a.primary).then_with(|| a.name.cmp(&b.name)));

    let mut merged: Vec<TriggerInfo> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for row in triggers {
        match at.get(&row.name).and_then(|&i| merged.get_mut(i)) {
            Some(t) => t.events.extend(row.events),
            None => {
                at.insert(row.name.clone(), merged.len());
                merged.push(TriggerInfo {
                    name: row.name,
                    timing: row.timing,
                    events: row.events,
                    for_each_row: row.for_each_row,
                    definition: kept_text(&row.definition, MAX_DEFINITION),
                });
            }
        }
    }
    for t in &mut merged {
        t.events.sort();
        t.events.dedup();
    }
    merged.sort_by(|a, b| a.name.cmp(&b.name));
    TableStructure { indexes, triggers: merged, defaults: Vec::new(), actions: Vec::new() }
}

/// `structure` with each column's default, cut to [`MAX_EXPRESSION`], and each foreign key's
/// actions.
pub fn with_keys(structure: TableStructure, defaults: Vec<ColumnDefault>, actions: Vec<ForeignKeyActions>) -> TableStructure {
    let defaults = defaults.into_iter().map(|d| ColumnDefault { default: d.default.map(|t| kept_text(&t, MAX_EXPRESSION)), ..d }).collect();
    TableStructure { defaults, actions, ..structure }
}

/// A foreign key action as SQL spells it (`NO ACTION`, `set null`); `None` for another word.
pub fn fk_action(word: &str) -> Option<FkAction> {
    match word.trim().to_ascii_uppercase().as_str() {
        "NO ACTION" => Some(FkAction::NoAction),
        "RESTRICT" => Some(FkAction::Restrict),
        "CASCADE" => Some(FkAction::Cascade),
        "SET NULL" => Some(FkAction::SetNull),
        "SET DEFAULT" => Some(FkAction::SetDefault),
        _ => None,
    }
}

/// An event as SQL spells it (`INSERT`, `update`); `None` for another word.
pub fn event(word: &str) -> Option<TriggerEvent> {
    match word.to_ascii_uppercase().as_str() {
        "INSERT" => Some(TriggerEvent::Insert),
        "UPDATE" => Some(TriggerEvent::Update),
        "DELETE" => Some(TriggerEvent::Delete),
        "TRUNCATE" => Some(TriggerEvent::Truncate),
        _ => None,
    }
}

/// A timing as SQL spells it (`BEFORE`, `after`, `INSTEAD OF`); `None` for another word.
pub fn timing(word: &str) -> Option<TriggerTiming> {
    match word.to_ascii_uppercase().as_str() {
        "BEFORE" => Some(TriggerTiming::Before),
        "AFTER" => Some(TriggerTiming::After),
        "INSTEAD OF" | "INSTEAD" => Some(TriggerTiming::InsteadOf),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(column: &str) -> IndexPart {
        IndexPart { column: Some(column.into()), expression: None, descending: false }
    }

    fn index(name: &str, primary: bool, position: u32, part: IndexPart) -> IndexRow {
        IndexRow { name: name.into(), unique: primary, primary, method: None, predicate: None, position, part }
    }

    fn trigger(name: &str, events: &[TriggerEvent], definition: &str) -> TriggerRow {
        TriggerRow {
            name: name.into(),
            timing: Some(TriggerTiming::After),
            events: events.to_vec(),
            for_each_row: Some(true),
            definition: definition.into(),
        }
    }

    #[test]
    fn index_parts_grouped_in_position_order_primary_first() {
        let s = build(
            vec![
                index("b_idx", false, 2, part("y")),
                index("a_idx", false, 1, part("z")),
                index("pk", true, 2, part("second")),
                index("b_idx", false, 1, part("x")),
                index("pk", true, 1, part("first")),
            ],
            Vec::new(),
        );
        let names: Vec<&str> = s.indexes.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["pk", "a_idx", "b_idx"]);
        let cols = |i: &IndexInfo| i.parts.iter().map(|p| p.column.clone().unwrap()).collect::<Vec<_>>();
        assert_eq!(cols(&s.indexes[0]), ["first", "second"]);
        assert_eq!(cols(&s.indexes[2]), ["x", "y"]);
        assert!(s.indexes[0].primary && s.indexes[0].unique);
    }

    #[test]
    fn trigger_events_merge_by_name_in_event_order() {
        use TriggerEvent::*;
        let s = build(Vec::new(), vec![trigger("t2", &[Delete], "d"), trigger("t1", &[Update], "first"), trigger("t1", &[Insert, Update], "second")]);
        let got: Vec<(&str, &[TriggerEvent], &str)> =
            s.triggers.iter().map(|t| (t.name.as_str(), t.events.as_slice(), t.definition.as_str())).collect();
        assert_eq!(got, [("t1", &[Insert, Update][..], "first"), ("t2", &[Delete][..], "d")]);
    }

    /// Server text is kept to its cap, cut at a character boundary.
    #[test]
    fn long_server_text_is_cut() {
        let long = "é".repeat(MAX_DEFINITION);
        let mut row = index("i", false, 1, IndexPart { column: None, expression: Some(long.clone()), descending: false });
        row.predicate = Some(long.clone());
        let s = build(vec![row], vec![trigger("t", &[], &long)]);
        let kept = |t: &str| t.len() <= MAX_DEFINITION + 3 && t.ends_with('…');
        assert!(kept(&s.triggers[0].definition));
        let e = s.indexes[0].parts[0].expression.as_deref().unwrap();
        assert!(e.len() <= MAX_EXPRESSION + 3 && e.ends_with('…'));
        assert!(s.indexes[0].predicate.as_deref().is_some_and(|p| p.len() <= MAX_EXPRESSION + 3));
    }

    #[test]
    fn fk_actions_spell_as_sql() {
        assert_eq!(fk_action("SET NULL"), Some(FkAction::SetNull));
        assert_eq!(fk_action("no action"), Some(FkAction::NoAction));
        assert_eq!(fk_action("Set Default"), Some(FkAction::SetDefault));
        assert_eq!(fk_action("RESTRICT"), Some(FkAction::Restrict));
        assert_eq!(fk_action("cascade"), Some(FkAction::Cascade));
        assert_eq!(fk_action("bogus"), None);
    }

    #[test]
    fn defaults_are_cut_and_kept_as_written() {
        let default = |column: &str, text: &str| ColumnDefault { column: column.into(), default: Some(text.into()) };
        let long = "x".repeat(9 * 1024);
        let s = with_keys(
            TableStructure::default(),
            vec![default("big", &long), default("status", "'Pending'::text"), ColumnDefault { column: "n".into(), default: None }],
            Vec::new(),
        );
        let big = s.defaults[0].default.as_deref().unwrap();
        assert!(big.len() <= MAX_EXPRESSION + 3 && big.ends_with('…'), "{}", big.len());
        assert!(big.len() >= MAX_EXPRESSION - 4);
        assert_eq!(s.defaults[1].default.as_deref(), Some("'Pending'::text"));
        assert_eq!(s.defaults[2].default, None);
    }

    #[test]
    fn words_as_sql_spells_them() {
        assert_eq!(event("insert"), Some(TriggerEvent::Insert));
        assert_eq!(event("TRUNCATE"), Some(TriggerEvent::Truncate));
        assert_eq!(event("SELECT"), None);
        assert_eq!(timing("instead of"), Some(TriggerTiming::InsteadOf));
        assert_eq!(timing("After"), Some(TriggerTiming::After));
        assert_eq!(timing(""), None);
    }
}

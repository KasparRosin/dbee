//! Tab commands: new query tab, activate, list, set the query text, close.

use std::collections::HashSet;

use serde_json::json;

use super::{CommandSpec, active_query, always, bad, has_tabs, p};
use crate::{EngineError, Json, Need, Outcome, QueryTab, Session, Tab, TabId};

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: "tab.newQuery",
        label: "New Query Tab",
        menu: &["File"],
        shortcut: Some("Ctrl+T"),
        params: "{text?, schema?: the schema the tree has selected}",
        enabled: always,
        run: new_query,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "tab.close",
        label: "Close Tab",
        menu: &["File"],
        shortcut: Some("Ctrl+W"),
        params: "{tab?, force?}",
        enabled: has_tabs,
        run: close,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "tab.activate",
        label: "Activate Tab",
        menu: &[],
        shortcut: None,
        params: "{tab}",
        enabled: has_tabs,
        run: activate,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "tab.list",
        label: "List Tabs",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: always,
        run: list,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: "query.setText",
        label: "Set Query Text",
        menu: &[],
        shortcut: None,
        params: "{text}",
        enabled: active_query,
        run: set_text,
        journal: true,
        undoable: false,
    },
];

/// The prefix of query tab titles: `Query 1`, `Query 2`, …
const QUERY_TITLE: &str = "Query ";

/// `{text?, schema?}` → `{tab}`: a new query tab, made active, titled with the lowest free
/// number. On an engine with `USE` its database is `schema` (the tree's selected schema or table's
/// schema), else the settings' database, else none.
fn new_query(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let text = p::opt_str(params, "text")?.unwrap_or_default().to_string();
    let database = new_tab_database(s, p::opt_str(params, "schema")?)?;
    let used: HashSet<u64> = s
        .state
        .tabs
        .iter()
        .filter_map(|t| match t {
            Tab::Query(q) => q.title.strip_prefix(QUERY_TITLE).and_then(|n| n.parse().ok()),
            Tab::Table(_) => None,
        })
        .collect();
    // At most `used.len()` numbers are taken, so a free one is found within `used.len() + 1`.
    let number = (1..=u64::MAX).find(|n| !used.contains(n)).unwrap_or(u64::MAX);
    let id = s.state.take_tab_id();
    s.state.tabs.push(Tab::Query(QueryTab { database, ..QueryTab::new(id, format!("{QUERY_TITLE}{number}"), text) }));
    s.state.active = Some(id);
    Ok(Outcome::Done(json!({ "tab": id })))
}

/// The database a new query tab runs in (see [`new_query`]): none off an engine with `USE`.
fn new_tab_database(s: &Session, schema: Option<&str>) -> Result<Option<String>, EngineError> {
    let Some(c) = s.state.connected.as_ref().filter(|c| c.dialect.has_use_database()) else { return Ok(None) };
    let Some(name) = schema.filter(|n| !n.is_empty()).or(Some(c.config.database.as_str()).filter(|d| !d.is_empty())) else { return Ok(None) };
    c.dialect.use_database(name).map_err(|e| bad(format!("`schema`: {e}")))?;
    Ok(Some(name.to_owned()))
}

/// `{tab}`: makes an open tab the active one.
fn activate(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = existing(s, p::tab_id(params, "tab")?)?;
    s.state.active = Some(id);
    Ok(Outcome::Done(json!({ "tab": id })))
}

/// `{}` → `{tabs: [{id, kind, title}], active}`.
fn list(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let tabs: Vec<Json> = s.state.tabs.iter().map(Tab::summary).collect();
    Ok(Outcome::Done(json!({ "tabs": tabs, "active": s.state.active })))
}

/// `{text}`: replaces the active query tab's text.
fn set_text(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let text = p::str(params, "text")?.to_string();
    let active = s.state.active;
    match active.and_then(|id| s.state.tab_mut(id)) {
        Some(Tab::Query(q)) => {
            q.text = text;
            Ok(Outcome::Done(json!({ "tab": q.id })))
        }
        // `enabled` checked it; kept for a state changed since.
        _ => Err(EngineError::Disabled { id: String::new(), why: "no query tab is active".into() }),
    }
}

/// `{tab?, force?}` → `{closed, active}`: closes the tab (default: the active one). A query tab
/// with text, or a table tab with staged edits, asks first unless `force`
/// (`Needs::Confirm { what: "queryText" | "stagedEdits" }`). A query run of the tab is cancelled;
/// any other job still running for the tab goes on (an Apply too). Either way its result is
/// ignored. Closing the active tab activates the one to its right, else the one to its left.
fn close(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let force = p::bool_or(params, "force", false)?;
    let id = match p::opt_tab_id(params, "tab")? {
        Some(id) => existing(s, id)?,
        None => s.state.active_tab().map(Tab::id).ok_or_else(|| bad("no tab is active; name one with `tab`"))?,
    };
    let at = s.state.tabs.iter().position(|t| t.id() == id).ok_or_else(|| bad(format!("no tab {}", id.0)))?;
    if !force {
        match s.state.tabs.get(at) {
            Some(Tab::Query(q)) if !q.text.trim().is_empty() => {
                return Err(EngineError::Needs {
                    need: Need::Confirm { what: "queryText".into() },
                    message: format!("Close {}? Its query text will be lost.", q.title),
                });
            }
            Some(Tab::Table(t)) if t.has_staged_edits() => {
                return Err(EngineError::Needs {
                    need: Need::Confirm { what: "stagedEdits".into() },
                    message: format!("Close {}? Its {} will be lost.", t.table.display(), staged_changes(t.edits.count())),
                });
            }
            _ => {}
        }
    }
    if let Tab::Query(q) = s.state.tabs.remove(at)
        && let Some(job) = q.running
    {
        super::query::stop_run(s, job);
    }
    if s.state.active == Some(id) {
        s.state.active = s.state.tabs.get(at).or_else(|| at.checked_sub(1).and_then(|i| s.state.tabs.get(i))).map(Tab::id);
    }
    Ok(Outcome::Done(json!({ "closed": id, "active": s.state.active })))
}

/// `1 staged change`, `3 staged changes`.
pub(crate) fn staged_changes(n: usize) -> String {
    if n == 1 { "1 staged change".into() } else { format!("{n} staged changes") }
}

/// `id` when that tab is open, else `BadParams`.
fn existing(s: &Session, id: TabId) -> Result<TabId, EngineError> {
    s.state.tab(id).map(Tab::id).ok_or_else(|| bad(format!("no tab {} is open", id.0)))
}

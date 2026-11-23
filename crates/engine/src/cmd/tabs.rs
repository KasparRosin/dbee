//! Tab commands: new query tab, activate, list, set the query text, close.

use std::collections::HashSet;

use serde_json::json;

use super::query::{self, End};
use super::{CommandSpec, active_query, always, bad, connection_param, has_tabs, p};
use crate::{ConnectionId, EndAbout, EngineError, Json, Need, Outcome, QueryTab, Session, Tab, TabId, TransactionState, Which};

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: "tab.newQuery",
        label: "New Query Tab",
        menu: &["File"],
        shortcut: Some("Ctrl+T"),
        params: "{text?, schema?: the schema the tree has selected, connection?}",
        enabled: always,
        run: new_query,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: CLOSE,
        label: "Close Tab",
        menu: &["File"],
        shortcut: Some("Ctrl+W"),
        params: "{tab?, force?, end?: \"commit\" or \"rollback\"}",
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

const CLOSE: &str = "tab.close";

/// The prefix of query tab titles: `Query 1`, `Query 2`, …
const QUERY_TITLE: &str = "Query ";

/// `{text?, schema?, connection?}` → `{tab}`: a new query tab on the connection named, else the
/// active tab's (none while none is open), made active, titled with the lowest free number. On an
/// engine with `USE` its database is `schema` (the tree's selected schema or table's schema), else
/// the settings' database, else none.
fn new_query(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let text = p::opt_str(params, "text")?.unwrap_or_default().to_string();
    let connection = connection_param(s, params)?;
    let database = new_tab_database(s, connection, p::opt_str(params, "schema")?)?;
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
    s.state.tabs.push(Tab::Query(QueryTab { connection, database, ..QueryTab::new(id, format!("{QUERY_TITLE}{number}"), text) }));
    s.state.active = Some(id);
    Ok(Outcome::Done(json!({ "tab": id })))
}

/// The database a new query tab on `connection` runs in (see [`new_query`]): none off an engine
/// with `USE`.
fn new_tab_database(s: &Session, connection: Option<ConnectionId>, schema: Option<&str>) -> Result<Option<String>, EngineError> {
    let Some(c) = connection.and_then(|id| s.state.connection(Which::Id(id))).filter(|c| c.dialect.has_use_database()) else { return Ok(None) };
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

/// `{}` → `{tabs: [{id, kind, title, connection}], active}`.
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

/// `{tab?, force?, end?}` → `{closed, active}`: closes the tab (default: the active one). Unless
/// `force` or `end`, it asks first:
/// - the query tab whose run opened its connection's transaction, while it is open or failed:
///   `Needs::EndTransaction` ([`query::end_question`]), which comes before the question below and
///   carries its sentence in `lost`;
/// - a query tab with text, or a table tab with staged edits: `Needs::Confirm`, `what` being
///   `queryText` or `stagedEdits`.
///
/// With `end` (`"commit"` or `"rollback"`) it is a job that ends the query session's transaction
/// first, and closes the tab unless that fails ([`query::end_then_close`]). A query run of the tab
/// is cancelled; any other job still running for the tab goes on (an Apply too). Either way its
/// result is ignored. Closing the active tab activates the one to its right, else the one to its
/// left.
fn close(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let force = p::bool_or(params, "force", false)?;
    let end = End::param(params)?;
    let id = match p::opt_tab_id(params, "tab")? {
        Some(id) => existing(s, id)?,
        None => s.state.active_tab().map(Tab::id).ok_or_else(|| bad("no tab is active; name one with `tab`"))?,
    };
    let tab = s.state.tab(id).ok_or_else(|| bad(format!("no tab {}", id.0)))?;
    if let Some(end) = end {
        let connection = tab.connection();
        return query::end_then_close(s, connection, end, CLOSE, move |s| Some(close_now(s, id)));
    }
    if !force {
        let question = format!("Close {}?", tab.title());
        let lost = lost(tab);
        if let Tab::Query(q) = tab
            && let Some(c) = q.connection.and_then(|c| s.state.connection(Which::Id(c)))
            && let Some(t) = c.transaction()
            && t.tab == Some(id)
            && matches!(t.state, TransactionState::Open | TransactionState::Failed)
        {
            return Err(query::end_question(s, c.id, t.state, EndAbout::Tab, q.title.clone(), &question, lost.map(|(_, l)| l)));
        }
        if let Some((what, lost)) = lost {
            return Err(EngineError::Needs { need: Need::Confirm { what: what.into() }, message: format!("{question} {lost}") });
        }
    }
    Ok(Outcome::Done(close_now(s, id)))
}

/// What closing `tab` loses: the `what` of its `Needs::Confirm`, and the sentence its question
/// says it in after asking ("Its query text will be lost.").
fn lost(tab: &Tab) -> Option<(&'static str, String)> {
    match tab {
        Tab::Query(q) if !q.text.trim().is_empty() => Some(("queryText", "Its query text will be lost.".to_owned())),
        Tab::Table(t) if t.has_staged_edits() => Some(("stagedEdits", format!("Its {} will be lost.", staged_changes(t.edits.count())))),
        _ => None,
    }
}

/// Closes tab `id` when it is still open (see [`close`]): `{closed, active}`.
fn close_now(s: &mut Session, id: TabId) -> Json {
    let Some(at) = s.state.tabs.iter().position(|t| t.id() == id) else { return json!({ "closed": id, "active": s.state.active }) };
    if let Tab::Query(q) = s.state.tabs.remove(at)
        && let Some(connection) = q.connection
    {
        if let Some(job) = q.running {
            super::query::stop_run(s, connection, job);
        }
        if let Some(c) = s.state.connection_mut(Which::Id(connection)) {
            c.tab_closed(id);
        }
    }
    if s.state.active == Some(id) {
        s.state.active = s.state.tabs.get(at).or_else(|| at.checked_sub(1).and_then(|i| s.state.tabs.get(i))).map(Tab::id);
    }
    json!({ "closed": id, "active": s.state.active })
}

/// `1 staged change`, `3 staged changes`.
pub(crate) fn staged_changes(n: usize) -> String {
    if n == 1 { "1 staged change".into() } else { format!("{n} staged changes") }
}

/// `id` when that tab is open, else `BadParams`.
fn existing(s: &Session, id: TabId) -> Result<TabId, EngineError> {
    s.state.tab(id).map(Tab::id).ok_or_else(|| bad(format!("no tab {} is open", id.0)))
}

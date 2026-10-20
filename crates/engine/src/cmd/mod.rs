//! The command registry. Each command module has a `SPECS` table; [`GROUPS`] lists the tables.
//!
//! A command is an id (`tab.newQuery`), a label and menu path for the UI, the shortcut the menu
//! shows, a params doc, an `enabled` check and a `run`. Ids follow Beekeeper Studio's menus plus
//! primitives. Commands call each other's functions, never [`Session::execute`], so the journal
//! records only what the caller asked for.
//!
//! **Stale job results.** A job's `Apply` first checks that what it worked for still exists: its
//! tab is open (and still runs this job), and the connection it used is still the session's
//! (`Session::is_current_browse`, or `Arc::ptr_eq` on the query connection). When not, because
//! the tab closed or the connection was closed or replaced while the job ran, it changes nothing
//! and returns `Ok(`[`ignored()`]`)`: never an error, and never a `Needs` prompt, since nobody is
//! waiting for that answer any more. The check comes before the job's own error is mapped.

pub mod connection;
pub mod schema;
pub mod tabs;

use serde::Serialize;

use crate::{EngineError, Json, Outcome, Session};

/// One command.
pub struct CommandSpec {
    pub id: &'static str,
    /// The menu item's text.
    pub label: &'static str,
    /// The menus the item sits in, e.g. `["File"]`; empty when it is not in a menu.
    pub menu: &'static [&'static str],
    /// The shortcut text the menu shows (`Ctrl+T`); the UI's shortcut table binds it.
    pub shortcut: Option<&'static str>,
    /// The params, e.g. `{tab?, force?}`.
    pub params: &'static str,
    /// `Err(why)` when it cannot run now.
    pub enabled: fn(&Session) -> Result<(), String>,
    pub run: fn(&mut Session, &Json) -> Result<Outcome, EngineError>,
    /// Recorded in the journal when it succeeds (queries that change nothing are not).
    pub journal: bool,
    /// Changes something the per-tab undo covers (staged edits).
    pub undoable: bool,
}

/// A command as the UI and the control channel list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    pub params: &'static str,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

impl CommandInfo {
    pub(crate) fn new(spec: &CommandSpec, enabled: Result<(), String>) -> CommandInfo {
        CommandInfo {
            id: spec.id,
            label: spec.label,
            menu: spec.menu,
            shortcut: spec.shortcut,
            params: spec.params,
            enabled: enabled.is_ok(),
            disabled_reason: enabled.err(),
        }
    }
}

/// Every command table, in menu order. A new command module adds its `SPECS` here.
const GROUPS: &[&[CommandSpec]] = &[tabs::SPECS, connection::SPECS, schema::SPECS];

#[cfg(test)]
fn test_specs() -> &'static [CommandSpec] {
    crate::tests::COMMANDS
}

#[cfg(not(test))]
fn test_specs() -> &'static [CommandSpec] {
    &[]
}

/// Every registered command.
pub(crate) fn specs() -> impl Iterator<Item = &'static CommandSpec> {
    GROUPS.iter().flat_map(|g| g.iter()).chain(test_specs())
}

pub(crate) fn find(id: &str) -> Option<&'static CommandSpec> {
    specs().find(|c| c.id == id)
}

/// A param that holds a secret: `password`, `passphrase`, or a key ending in `Password` or
/// `Passphrase` (`sshPassword`, `sshPassphrase`).
pub fn is_secret_param(key: &str) -> bool {
    key == "password" || key == "passphrase" || key.ends_with("Password") || key.ends_with("Passphrase")
}

/// `params` as the journal keeps it: every secret param is left out, at any depth. Replaying
/// the entry runs the command without it (the secret store or the user supplies it again).
pub fn journal_params(params: &Json) -> Json {
    let mut kept = params.clone();
    // Iterative, so deep params cannot overflow the stack.
    let mut stack: Vec<&mut Json> = vec![&mut kept];
    while let Some(v) = stack.pop() {
        match v {
            Json::Object(map) => {
                map.retain(|k, _| !is_secret_param(k));
                stack.extend(map.values_mut());
            }
            Json::Array(items) => stack.extend(items.iter_mut()),
            _ => {}
        }
    }
    kept
}

/// `enabled` for commands that can always run.
pub fn always(_: &Session) -> Result<(), String> {
    Ok(())
}

/// At least one tab is open.
pub fn has_tabs(s: &Session) -> Result<(), String> {
    if s.state.tabs.is_empty() { Err("no tab is open".into()) } else { Ok(()) }
}

/// The result of a job whose tab or connection went away while it ran: `{"ignored": true}`.
pub(crate) fn ignored() -> Json {
    serde_json::json!({ "ignored": true })
}

/// Why a command that needs a connection cannot run on the connection screen.
pub const NOT_CONNECTED: &str = "not connected";

/// Why a command that needs the database cannot run after the connection broke.
pub const LOST: &str = "connection lost — reconnect";

/// A connection is open, lost or not.
pub fn connected(s: &Session) -> Result<(), String> {
    if s.state.connected.is_some() { Ok(()) } else { Err(NOT_CONNECTED.into()) }
}

/// No connection is open (one connection per window).
pub fn not_connected(s: &Session) -> Result<(), String> {
    if s.state.connected.is_some() { Err("already connected; disconnect first".into()) } else { Ok(()) }
}

/// A connection is open and not lost: for every command that needs the database.
pub fn needs_db(s: &Session) -> Result<(), String> {
    match &s.state.connected {
        None => Err(NOT_CONNECTED.into()),
        Some(c) if c.lost => Err(LOST.into()),
        Some(_) => Ok(()),
    }
}

/// The active tab is a query tab.
pub fn active_query(s: &Session) -> Result<(), String> {
    match s.state.active_tab() {
        Some(crate::Tab::Query(_)) => Ok(()),
        _ => Err("no query tab is active".into()),
    }
}

/// Reading params. Each helper returns `BadParams` naming the key; `execute` fills in the
/// command id. A missing key and `null` are the same.
pub mod p {
    use crate::{EngineError, Json, TabId};

    fn bad(key: &str, what: &str) -> EngineError {
        EngineError::BadParams { id: String::new(), msg: format!("`{key}` {what}") }
    }

    fn get<'a>(p: &'a Json, key: &str) -> Option<&'a Json> {
        p.get(key).filter(|v| !v.is_null())
    }

    fn whole(v: &Json, key: &str) -> Result<u64, EngineError> {
        v.as_u64().ok_or_else(|| bad(key, "must be a whole number, 0 or more"))
    }

    /// A required string.
    pub fn str<'a>(p: &'a Json, key: &str) -> Result<&'a str, EngineError> {
        opt_str(p, key)?.ok_or_else(|| bad(key, "is required (a string)"))
    }

    pub fn opt_str<'a>(p: &'a Json, key: &str) -> Result<Option<&'a str>, EngineError> {
        match get(p, key) {
            None => Ok(None),
            Some(Json::String(s)) => Ok(Some(s)),
            Some(_) => Err(bad(key, "must be a string")),
        }
    }

    /// A required whole number.
    pub fn usize(p: &Json, key: &str) -> Result<usize, EngineError> {
        opt_usize(p, key)?.ok_or_else(|| bad(key, "is required (a whole number)"))
    }

    pub fn opt_usize(p: &Json, key: &str) -> Result<Option<usize>, EngineError> {
        get(p, key).map(|v| whole(v, key).and_then(|n| usize::try_from(n).map_err(|_| bad(key, "is too large")))).transpose()
    }

    pub fn bool_or(p: &Json, key: &str, default: bool) -> Result<bool, EngineError> {
        match get(p, key) {
            None => Ok(default),
            Some(v) => v.as_bool().ok_or_else(|| bad(key, "must be true or false")),
        }
    }

    /// A required tab id. Whether the tab exists is the command's check.
    pub fn tab_id(p: &Json, key: &str) -> Result<TabId, EngineError> {
        opt_tab_id(p, key)?.ok_or_else(|| bad(key, "is required (a tab id)"))
    }

    pub fn opt_tab_id(p: &Json, key: &str) -> Result<Option<TabId>, EngineError> {
        get(p, key).map(|v| whole(v, key).map(TabId)).transpose()
    }
}

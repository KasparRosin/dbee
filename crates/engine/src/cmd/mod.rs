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
//! waiting for that answer any more. The check comes before the job's own error is mapped. The
//! one exception is a query run whose tab closed: what it did to the query connection is kept
//! (see [`query`]).

pub mod connection;
pub mod edit;
pub mod query;
pub mod schema;
pub mod selection;
pub mod table;
pub mod tabs;

use serde::Serialize;

use crate::{ConnectionId, EngineError, Json, Outcome, Session, Which};

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
const GROUPS: &[&[CommandSpec]] = &[tabs::SPECS, connection::SPECS, schema::SPECS, table::SPECS, edit::SPECS, query::SPECS, selection::SPECS];

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

/// A `BadParams` error; [`Session::execute`] fills in the command id.
pub(crate) fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { id: String::new(), msg: msg.into() }
}

/// A `Disabled` error saying why; [`Session::execute`] fills in the command id.
pub(crate) fn disabled(why: impl Into<String>) -> EngineError {
    EngineError::Disabled { id: String::new(), why: why.into() }
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

/// How many connections may be open at once.
pub(crate) const MAX_OPEN: usize = 1;

/// Why another connection cannot open while [`MAX_OPEN`] are.
pub(crate) const ALREADY_CONNECTED: &str = "already connected; disconnect first";

/// A connection is open, lost or not.
pub fn connected(s: &Session) -> Result<(), String> {
    if s.state.connection(Which::Active).is_some() { Ok(()) } else { Err(NOT_CONNECTED.into()) }
}

/// Another connection may open: fewer than [`MAX_OPEN`] are.
pub fn not_connected(s: &Session) -> Result<(), String> {
    if s.state.connections.len() >= MAX_OPEN { Err(ALREADY_CONNECTED.into()) } else { Ok(()) }
}

/// A connection is open and not lost: for every command that needs the database.
pub fn needs_db(s: &Session) -> Result<(), String> {
    match s.state.connection(Which::Active) {
        None => Err(NOT_CONNECTED.into()),
        Some(c) if c.lost => Err(LOST.into()),
        Some(_) => Ok(()),
    }
}

/// The open connection a command's `connection` param names, else the active tab's
/// ([`Which::Active`]); `None` when none is named and none is open. An id that is not open is
/// `BadParams`.
pub(crate) fn connection_param(s: &Session, params: &Json) -> Result<Option<ConnectionId>, EngineError> {
    let which = match p::opt_connection_id(params, "connection")? {
        Some(id) if s.state.connection(Which::Id(id)).is_none() => return Err(bad(format!("no connection {} is open", id.0))),
        Some(id) => Which::Id(id),
        None => Which::Active,
    };
    Ok(s.state.connection(which).map(|c| c.id))
}

/// [`connection_param`] for a command that needs a connection: `Disabled` when none is open.
pub(crate) fn needed_connection(s: &Session, params: &Json) -> Result<ConnectionId, EngineError> {
    connection_param(s, params)?.ok_or_else(|| disabled(NOT_CONNECTED))
}

/// Why a query tab command cannot run when the active tab is not a query tab.
pub const NO_QUERY_TAB: &str = "no query tab is active";

/// The active tab is a query tab.
pub fn active_query(s: &Session) -> Result<(), String> {
    match s.state.active_tab() {
        Some(crate::Tab::Query(_)) => Ok(()),
        _ => Err(NO_QUERY_TAB.into()),
    }
}

/// Reading params. Each helper returns `BadParams` naming the key; `execute` fills in the
/// command id. A missing key and `null` are the same.
pub mod p {
    use crate::{ConnectionId, EngineError, Json, TabId};

    fn bad_key(key: &str, what: &str) -> EngineError {
        EngineError::BadParams { id: String::new(), msg: format!("`{key}` {what}") }
    }

    fn get<'a>(p: &'a Json, key: &str) -> Option<&'a Json> {
        p.get(key).filter(|v| !v.is_null())
    }

    fn whole(v: &Json, key: &str) -> Result<u64, EngineError> {
        v.as_u64().ok_or_else(|| bad_key(key, "must be a whole number, 0 or more"))
    }

    /// A required string.
    pub fn str<'a>(p: &'a Json, key: &str) -> Result<&'a str, EngineError> {
        opt_str(p, key)?.ok_or_else(|| bad_key(key, "is required (a string)"))
    }

    pub fn opt_str<'a>(p: &'a Json, key: &str) -> Result<Option<&'a str>, EngineError> {
        match get(p, key) {
            None => Ok(None),
            Some(Json::String(s)) => Ok(Some(s)),
            Some(_) => Err(bad_key(key, "must be a string")),
        }
    }

    /// A required whole number.
    pub fn usize(p: &Json, key: &str) -> Result<usize, EngineError> {
        opt_usize(p, key)?.ok_or_else(|| bad_key(key, "is required (a whole number)"))
    }

    pub fn opt_usize(p: &Json, key: &str) -> Result<Option<usize>, EngineError> {
        get(p, key).map(|v| whole(v, key).and_then(|n| usize::try_from(n).map_err(|_| bad_key(key, "is too large")))).transpose()
    }

    pub fn bool_or(p: &Json, key: &str, default: bool) -> Result<bool, EngineError> {
        match get(p, key) {
            None => Ok(default),
            Some(v) => v.as_bool().ok_or_else(|| bad_key(key, "must be true or false")),
        }
    }

    /// A required tab id. Whether the tab exists is the command's check.
    pub fn tab_id(p: &Json, key: &str) -> Result<TabId, EngineError> {
        opt_tab_id(p, key)?.ok_or_else(|| bad_key(key, "is required (a tab id)"))
    }

    pub fn opt_tab_id(p: &Json, key: &str) -> Result<Option<TabId>, EngineError> {
        get(p, key).map(|v| whole(v, key).map(TabId)).transpose()
    }

    /// An open connection's id. Whether it is open is the caller's check
    /// ([`connection_param`](super::connection_param)).
    pub fn opt_connection_id(p: &Json, key: &str) -> Result<Option<ConnectionId>, EngineError> {
        get(p, key).map(|v| whole(v, key).map(ConnectionId)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::{bad, disabled};
    use crate::EngineError;

    #[test]
    fn bad_and_disabled_leave_the_id_for_execute() {
        assert_eq!(bad("`x` is required"), EngineError::BadParams { id: String::new(), msg: "`x` is required".into() });
        assert_eq!(bad(format!("no tab {}", 3)), EngineError::BadParams { id: String::new(), msg: "no tab 3".into() });
        assert_eq!(disabled("not connected"), EngineError::Disabled { id: String::new(), why: "not connected".into() });
        assert_eq!(disabled(String::from("busy")), EngineError::Disabled { id: String::new(), why: "busy".into() });
    }
}

//! `schema.inspect` and `quickSearch.find`: read the session's catalog and saved connections,
//! no database work.
//!
//! The catalog is the one loaded on connect and by `schema.refresh`, of the connection the
//! `connection` param names, else of the active tab's. Both commands are read-only.
//! `schema.inspect` needs a connection that is not lost; `quickSearch.find` runs without one too,
//! and then finds saved connections only.

use serde_json::json;

use super::{CommandSpec, always, bad, connection_param, needs_db, p};
use crate::fuzzy::{self, Found};
use crate::{EngineError, Json, Outcome, Session, Which};

const INSPECT: &str = "schema.inspect";
const FIND: &str = "quickSearch.find";

/// Results of `quickSearch.find` when `limit` is not given.
const DEFAULT_LIMIT: usize = 20;

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: INSPECT,
        label: "Inspect Schema",
        menu: &[],
        shortcut: None,
        params: "{connection?, schema?, table?}",
        enabled: needs_db,
        run: inspect,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: FIND,
        label: "Quick Search",
        menu: &[],
        shortcut: None,
        params: "{query, limit?, connection?, grouped?}",
        enabled: always,
        run: quick_find,
        journal: false,
        undoable: false,
    },
];

/// `{}` → the catalog; `{schema}` → that schema; `{schema, table}` → that `TableInfo`.
fn inspect(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let schema = p::opt_str(params, "schema")?;
    let table = p::opt_str(params, "table")?;
    let Some(c) = connection_param(s, params)?.and_then(|id| s.state.connection(Which::Id(id))) else { return Err(bad("not connected")) };
    let catalog = &c.catalog;
    let value = match (schema, table) {
        (None, None) => serde_json::to_value(&**catalog),
        (None, Some(_)) => return Err(bad("`table` needs `schema`")),
        (Some(sch), t) => {
            let info = catalog.schemas.iter().find(|x| x.name == sch).ok_or_else(|| bad(format!("no schema `{sch}`")))?;
            match t {
                None => serde_json::to_value(info),
                Some(t) => {
                    let info = info.tables.iter().find(|x| x.table.name == t).ok_or_else(|| bad(format!("no table `{sch}.{t}`")))?;
                    serde_json::to_value(info)
                }
            }
        }
    };
    value.map(Outcome::Done).map_err(|e| bad(format!("cannot serialize: {e}")))
}

/// `{query, limit?, grouped?}` → the hits, best first: `{hit: "table", schema, table, kind}` for the open
/// connection's tables (none while not connected or lost), `kind` being the relation's, and
/// `{hit: "connection", id, name, dbKind, kindLabel, color}` for the saved connections but the
/// open one, `name` being the display name. With `grouped: true` the tables come first, best
/// first, at most `limit`, then the connections, best first, at most `limit`.
fn quick_find(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let query = p::str(params, "query")?;
    let limit = p::opt_usize(params, "limit")?.unwrap_or(DEFAULT_LIMIT);
    let grouped = p::bool_or(params, "grouped", false)?;
    let open = connection_param(s, params)?.and_then(|id| s.state.connection(Which::Id(id)));
    let catalog = open.filter(|c| !c.lost).map(|c| &*c.catalog);
    let (saved, open) = (&s.state.store.saved, open.map(|c| c.config.id.as_str()));
    let found = if grouped { fuzzy::search_grouped(catalog, saved, open, query, limit) } else { fuzzy::search(catalog, saved, open, query, limit) };
    let hits: Vec<Json> = found
        .into_iter()
        .map(|found| match found {
            Found::Table(h) => json!({ "hit": "table", "schema": h.table.schema, "table": h.table.name, "kind": h.kind }),
            Found::Connection(h) => {
                let info = h.config.kind.info();
                json!({ "hit": "connection", "id": h.config.id, "name": h.name, "dbKind": info.id, "kindLabel": info.label, "color": h.config.color })
            }
        })
        .collect();
    Ok(Outcome::Done(Json::Array(hits)))
}

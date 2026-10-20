//! `schema.inspect` and `quickSearch.find`: read the session's catalog, no database work.
//!
//! The catalog is the one loaded on connect and by `schema.refresh`; both commands are
//! read-only and need a connection that is not lost.

use serde_json::json;

use super::{CommandSpec, needs_db, p};
use crate::{EngineError, Json, Outcome, Session, fuzzy};

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
        params: "{schema?, table?}",
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
        params: "{query, limit?}",
        enabled: needs_db,
        run: quick_find,
        journal: false,
        undoable: false,
    },
];

fn bad(msg: String) -> EngineError {
    EngineError::BadParams { id: String::new(), msg }
}

/// `{}` → the catalog; `{schema}` → that schema; `{schema, table}` → that `TableInfo`.
fn inspect(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let schema = p::opt_str(params, "schema")?;
    let table = p::opt_str(params, "table")?;
    let Some(c) = s.state.connected.as_ref() else { return Err(bad("not connected".into())) };
    let catalog = &c.catalog;
    let value = match (schema, table) {
        (None, None) => serde_json::to_value(&**catalog),
        (None, Some(_)) => return Err(bad("`table` needs `schema`".into())),
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

/// `{query, limit?}` → `[{schema, table, kind}]`, best first.
fn quick_find(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let query = p::str(params, "query")?;
    let limit = p::opt_usize(params, "limit")?.unwrap_or(DEFAULT_LIMIT);
    let Some(c) = s.state.connected.as_ref() else { return Err(bad("not connected".into())) };
    let hits: Vec<Json> = fuzzy::find(&c.catalog, query, limit)
        .into_iter()
        .map(|h| json!({ "schema": h.table.schema, "table": h.table.name, "kind": h.kind }))
        .collect();
    Ok(Outcome::Done(Json::Array(hits)))
}

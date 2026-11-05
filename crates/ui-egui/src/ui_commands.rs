//! UI-only commands: what only the window can do. They are listed with the engine's commands
//! (`engine.commands`, the menus) and run the same way, through [`DbeeApp::run`] or
//! `engine.execute`; they are not journaled.
//!
//! - `ui.quickSearch` (Ctrl+P): opens the quick search over the workspace → `{open: true}`.
//! - `ui.toggleValuePanel`: hides or shows the value panel → `{visible}`.
//! - `clipboard.copy {format}` (Ctrl+C `tsv`, Ctrl+Shift+C `tsvHeader`, the grid's menu): runs
//!   `copy.selection`, puts its text on the clipboard and shows "Copied n rows" →
//!   `{rows, columns, bytes}` (`bytes`: the length of the text). The text itself is not in the
//!   result: it can be large, and it is cell values. A copy whose text would pass the limit
//!   (`COPY_MAX_BYTES`) is `Disabled` with the engine's reason, which a shortcut or the menu shows
//!   as a notice.

use dbee_engine::{CommandInfo, EngineError, Json, Outcome, cmd};
use serde_json::json;

use crate::DbeeApp;

/// One UI-only command; the same fields as the engine's `CommandSpec`.
pub struct UiCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    pub params: &'static str,
    pub enabled: fn(&DbeeApp) -> Result<(), String>,
    run: fn(&mut DbeeApp, Json) -> Result<Outcome, EngineError>,
}

const COPY: &str = "clipboard.copy";

pub const UI_COMMANDS: &[UiCommand] = &[
    UiCommand {
        id: "ui.quickSearch",
        label: "Quick Search",
        menu: &["File"],
        shortcut: Some("Ctrl+P"),
        params: "{} -> {open}",
        enabled: |app| match &app.view.dialog {
            Some(_) => Err("a dialog is open; close it first".into()),
            None => cmd::needs_db(app.session()),
        },
        run: quick_search,
    },
    UiCommand {
        id: "ui.toggleValuePanel",
        label: "Toggle Value Panel",
        menu: &["View"],
        shortcut: None,
        params: "{} -> {visible}",
        enabled: |_| Ok(()),
        run: toggle_value_panel,
    },
    UiCommand {
        id: COPY,
        label: "Copy",
        menu: &["Edit"],
        shortcut: Some("Ctrl+C"),
        params: "{format: \"tsv\" | \"tsvHeader\" | \"csv\" | \"json\" | \"markdown\"} -> {rows, columns, bytes}; the text goes to the clipboard",
        enabled: |_| Ok(()),
        run: copy,
    },
];

/// The UI-only command `id`.
pub fn find(id: &str) -> Option<&'static UiCommand> {
    UI_COMMANDS.iter().find(|c| c.id == id)
}

/// The UI-only commands as `engine.commands` lists them. `engine` is the engine's list, which
/// `clipboard.copy` takes its enabled state from (`copy.selection`).
pub fn infos(app: &DbeeApp, engine: &[CommandInfo]) -> Vec<CommandInfo> {
    UI_COMMANDS
        .iter()
        .map(|c| {
            let enabled = if c.id == COPY {
                match engine.iter().find(|e| e.id == "copy.selection") {
                    Some(e) if !e.enabled => Err(e.disabled_reason.clone().unwrap_or_default()),
                    _ => Ok(()),
                }
            } else {
                (c.enabled)(app)
            };
            CommandInfo {
                id: c.id,
                label: c.label,
                menu: c.menu,
                shortcut: c.shortcut,
                params: c.params,
                enabled: enabled.is_ok(),
                disabled_reason: enabled.err(),
            }
        })
        .collect()
}

/// Runs `c` as `Session::execute` runs an engine command: the enabled check, then params, which
/// must be an object (`null` counts as `{}`).
pub(crate) fn run(app: &mut DbeeApp, c: &UiCommand, params: Json) -> Result<Outcome, EngineError> {
    (c.enabled)(app).map_err(|why| EngineError::Disabled { id: c.id.to_string(), why })?;
    let params = match params {
        Json::Null => json!({}),
        p @ Json::Object(_) => p,
        _ => return Err(EngineError::BadParams { id: c.id.to_string(), msg: "params must be a JSON object".into() }),
    };
    (c.run)(app, params)
}

fn quick_search(app: &mut DbeeApp, _: Json) -> Result<Outcome, EngineError> {
    app.view.quick.show_new(app.session.state());
    Ok(Outcome::Done(json!({ "open": true })))
}

fn toggle_value_panel(app: &mut DbeeApp, _: Json) -> Result<Outcome, EngineError> {
    app.view.value_panel = !app.view.value_panel;
    Ok(Outcome::Done(json!({ "visible": app.view.value_panel })))
}

fn copy(app: &mut DbeeApp, params: Json) -> Result<Outcome, EngineError> {
    let rename = |e: EngineError| match e {
        EngineError::Disabled { why, .. } => EngineError::Disabled { id: COPY.into(), why },
        EngineError::BadParams { msg, .. } => EngineError::BadParams { id: COPY.into(), msg },
        other => other,
    };
    let done = match app.session.execute("copy.selection", params).map_err(rename)? {
        Outcome::Done(v) => v,
        Outcome::Started(_) => return Err(EngineError::Internal { id: COPY.into(), msg: "copy.selection started a job".into() }),
    };
    let Json::Object(mut done) = done else {
        return Err(EngineError::Internal { id: COPY.into(), msg: "copy.selection returned no object".into() });
    };
    let rows = done.remove("rows").unwrap_or(Json::Null);
    let bytes = match done.remove("text") {
        Some(Json::String(text)) => {
            let n = text.len();
            app.copy_text(text);
            n
        }
        _ => 0,
    };
    app.notice_again(copied_notice(rows.as_u64().unwrap_or(0)));
    Ok(Outcome::Done(json!({ "rows": rows, "columns": done.remove("columns").unwrap_or(Json::Null), "bytes": bytes })))
}

/// The notice after a grid copy: `Copied 1 row`, `Copied 1,250 rows`.
fn copied_notice(rows: u64) -> String {
    let unit = if rows == 1 { "row" } else { "rows" };
    format!("Copied {} {unit}", crate::tree::thousands(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_notice_counts_rows() {
        assert_eq!(copied_notice(1), "Copied 1 row");
        assert_eq!(copied_notice(0), "Copied 0 rows");
        assert_eq!(copied_notice(2), "Copied 2 rows");
        assert_eq!(copied_notice(10_000), "Copied 10,000 rows");
    }
}

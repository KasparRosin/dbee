//! UI-only commands: what only the window can do. They are listed with the engine's commands
//! (`engine.commands`, the menus) and run the same way, through [`DbeeApp::run`] or
//! `engine.execute`; they are not journaled.
//!
//! - `ui.quickSearch` (Ctrl+P): opens the quick search over either screen → `{open: true}`.
//! - `ui.toggleValuePanel`: hides or shows the value panel → `{visible}`.
//! - `ui.themeSystem`, `ui.themeLight`, `ui.themeDark` (View > Theme): choose how the window is
//!   coloured and keep the choice → `{choice, mode}`; `System` follows what the desktop said at
//!   startup. A choice that cannot be kept still applies, and a notice says so.
//! - `clipboard.copy {format}` (Ctrl+C `tsv`, Ctrl+Shift+C `tsvHeader`, the grid's menu): runs
//!   `copy.selection`, puts its text on the clipboard and shows "Copied n rows" →
//!   `{rows, columns, bytes}` (`bytes`: the length of the text). The text itself is not in the
//!   result: it can be large, and it is cell values. A copy whose text would pass the limit
//!   (`COPY_MAX_BYTES`) is `Disabled` with the engine's reason, which a shortcut or the menu shows
//!   as a notice.

use dbee_config::ThemeChoice;
use dbee_engine::{CommandInfo, EngineError, Json, Outcome};
use serde_json::json;

use crate::DbeeApp;
use crate::components::toast::Kind;
use crate::theme::Words;

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
            None => Ok(()),
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
        id: "ui.themeSystem",
        label: "System",
        menu: &["View", "Theme"],
        shortcut: None,
        params: "{} -> {choice, mode}",
        enabled: |_| Ok(()),
        run: |app, _| choose(app, ThemeChoice::System),
    },
    UiCommand {
        id: "ui.themeLight",
        label: "Light",
        menu: &["View", "Theme"],
        shortcut: None,
        params: "{} -> {choice, mode}",
        enabled: |_| Ok(()),
        run: |app, _| choose(app, ThemeChoice::Light),
    },
    UiCommand {
        id: "ui.themeDark",
        label: "Dark",
        menu: &["View", "Theme"],
        shortcut: None,
        params: "{} -> {choice, mode}",
        enabled: |_| Ok(()),
        run: |app, _| choose(app, ThemeChoice::Dark),
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

/// The command id of `choice`, which the menu marks.
pub(crate) fn theme_id(choice: ThemeChoice) -> &'static str {
    match choice {
        ThemeChoice::System => "ui.themeSystem",
        ThemeChoice::Light => "ui.themeLight",
        ThemeChoice::Dark => "ui.themeDark",
    }
}

fn choose(app: &mut DbeeApp, choice: ThemeChoice) -> Result<Outcome, EngineError> {
    app.choose_theme(choice);
    let mode = if app.mode() == egui::Theme::Dark { "dark" } else { "light" };
    Ok(Outcome::Done(json!({ "choice": choice.as_str(), "mode": mode })))
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
    app.notice_again(Kind::Success, copied_notice(rows.as_u64().unwrap_or(0)));
    Ok(Outcome::Done(json!({ "rows": rows, "columns": done.remove("columns").unwrap_or(Json::Null), "bytes": bytes })))
}

/// The notice after a grid copy: `Copied 1 row`, `Copied 1,250 rows`.
pub(crate) fn copied_notice(rows: u64) -> Words {
    let unit = if rows == 1 { "row" } else { "rows" };
    Words::from("Copied ").name(crate::tree::thousands(rows)).plain(format!(" {unit}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_notice_counts_rows() {
        assert_eq!(copied_notice(1).to_string(), "Copied 1 row");
        assert_eq!(copied_notice(0).to_string(), "Copied 0 rows");
        assert_eq!(copied_notice(2).to_string(), "Copied 2 rows");
        assert_eq!(copied_notice(10_000).to_string(), "Copied 10,000 rows");
    }
}

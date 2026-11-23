//! The menu bar, built from the commands' `menu` paths ([`DbeeApp::commands`]: the engine's and
//! the UI-only ones): File, Edit, View and Query in that order, then any other top menu a command
//! names, each listing its commands in registry order, a deeper path as a submenu. Edit lists its
//! commands in the Screens page's groups, a line between each: Undo, Redo | Copy, Select All |
//! Apply, Reset.
//!
//! An entry shows its label and its shortcut's keys, and is enabled as the command is now;
//! hovering a disabled entry shows why. A menu with a chosen entry (View > Theme) marks it in a
//! column on the left. Clicking an entry runs the command through [`DbeeApp::start`], as
//! a button would, so a `needs…` answer opens its dialog and an error shows as a notice. It runs
//! with the params of its shortcut's row ([`SHORTCUTS`]), so an entry does what its keys do (Copy
//! copies as tab-separated values); without a shortcut, with `{}`.
//!
//! Automation ids: `menu-<Top>` for the top menus (`menu-File`) and `menu-<command id>` for the
//! entries (`menu-tab.newQuery`), with the value `{label, shortcut, why?, checked?}`; a submenu's
//! button is tagged with its path (`menu-View-Theme`). Entries are drawn
//! (and tagged) only while their menu is open.

use dbee_engine::{CommandInfo, Json};
use egui::containers::menu::MenuBar;
use serde_json::json;

use crate::DbeeApp;
use crate::app::Origin;
use crate::automation::Tags;
use crate::components::menu::{self, Item};
use crate::components::tooltip::Hint;
use crate::shortcuts::SHORTCUTS;

/// The top menus that come first, in this order.
pub const MENUS: &[&str] = &["File", "Edit", "View", "Query"];

/// The params a menu entry runs with: those of its shortcut's row, else `{}`.
pub fn params_for(shortcut: Option<&str>) -> Json {
    shortcut.and_then(|keys| SHORTCUTS.iter().find(|(k, ..)| *k == keys)).map_or_else(|| json!({}), |(_, _, params)| params.clone())
}

/// The top menus `commands` name: [`MENUS`] first, then the others as they come.
fn tops(commands: &[CommandInfo]) -> Vec<&str> {
    let mut tops: Vec<&str> = MENUS.iter().copied().filter(|m| commands.iter().any(|c| c.menu.first() == Some(m))).collect();
    for c in commands {
        if let Some(top) = c.menu.first()
            && !tops.contains(top)
        {
            tops.push(top);
        }
    }
    tops
}

/// Draws the menu bar ([`menu::bar`]) into `ui` and runs the entry clicked, if any.
pub(crate) fn bar(app: &mut DbeeApp, ui: &mut egui::Ui) {
    // Nothing is taken, so every entry clicked runs here.
    bar_taking(app, ui, &[]);
}

/// [`bar`], except that the entries of `taken` are the caller's: each drawn enabled when its
/// reason is `None`, else off with that reason on hover, and returned when clicked instead of run
/// (the connection screen's File > Connect runs its form's Connect).
pub(crate) fn bar_taking(app: &mut DbeeApp, ui: &mut egui::Ui, taken: &[(&str, Option<&str>)]) -> Option<&'static str> {
    let mut commands = app.commands();
    for c in &mut commands {
        if let Some((_, why)) = taken.iter().find(|(id, _)| *id == c.id) {
            c.enabled = why.is_none();
            c.disabled_reason = why.map(str::to_owned);
        }
    }
    let mut chosen: Option<&'static str> = None;
    let checked = crate::ui_commands::theme_id(app.theme_choice);
    let tags = &mut app.tags;
    menu::bar(ui, |ui| {
        MenuBar::new().ui(ui, |ui| {
            // The titles stand side by side: each holds its own room around its word.
            ui.spacing_mut().item_spacing.x = 0.0;
            for top in tops(&commands) {
                let (r, _) = menu::bar_title(ui, top, |ui| entries(ui, &commands, &[top], checked, tags, &mut chosen));
                tags.tag(&format!("menu-{top}"), &r);
            }
        })
    });
    let id = chosen?;
    if taken.iter().any(|(t, _)| *t == id) {
        return Some(id);
    }
    let shortcut = commands.iter().find(|c| c.id == id).and_then(|c| c.shortcut);
    app.start(id, params_for(shortcut), Origin::Menu);
    None
}

/// One line of an open menu.
enum Row<'a> {
    Entry(&'a CommandInfo),
    /// The submenu of this name, holding the commands whose menu goes deeper.
    Submenu(&'a str),
    Separator,
}

/// The menus drawn in groups with a line between them, each group's commands in its order. A
/// command of such a menu that no group names comes last, in a group of its own.
const GROUPS: &[(&str, &[&[&str]])] =
    &[("Edit", &[&["edit.undo", "edit.redo"], &["clipboard.copy", "selection.all"], &["edit.apply", "edit.reset"]])];

/// What menu `path` lists: the commands whose menu is exactly `path`, and a submenu for each
/// deeper path, in registry order, or in the order of its [`GROUPS`].
fn rows<'a>(commands: &'a [CommandInfo], path: &[&str]) -> Vec<Row<'a>> {
    let mut rows = Vec::new();
    let mut subs: Vec<&str> = Vec::new();
    for c in commands {
        if c.menu.len() > path.len() && c.menu.starts_with(path) {
            if let Some(sub) = c.menu.get(path.len())
                && !subs.contains(sub)
            {
                subs.push(sub);
                rows.push(Row::Submenu(sub));
            }
        } else if c.menu == path {
            rows.push(Row::Entry(c));
        }
    }
    let Some((_, order)) = GROUPS.iter().find(|(top, _)| path == [*top]) else { return rows };
    let mut groups: Vec<Vec<Row<'a>>> = order
        .iter()
        .map(|ids| ids.iter().filter_map(|id| commands.iter().find(|c| c.id == *id && c.menu == path).map(Row::Entry)).collect())
        .collect();
    groups.push(rows.into_iter().filter(|row| !matches!(row, Row::Entry(c) if order.iter().any(|ids| ids.contains(&c.id)))).collect());
    let mut grouped = Vec::new();
    for group in groups.into_iter().filter(|g| !g.is_empty()) {
        if !grouped.is_empty() {
            grouped.push(Row::Separator);
        }
        grouped.extend(group);
    }
    grouped
}

/// The entries under menu `path` ([`rows`]).
fn entries(ui: &mut egui::Ui, commands: &[CommandInfo], path: &[&str], checked: &str, tags: &mut Tags, chosen: &mut Option<&'static str>) {
    // A menu holding the chosen entry is a menu of choices: each entry has the mark's column.
    let choices = commands.iter().any(|c| c.menu == path && c.id == checked);
    for row in rows(commands, path) {
        let c = match row {
            Row::Entry(c) => c,
            Row::Submenu(sub) => {
                let deeper: Vec<&str> = path.iter().copied().chain(std::iter::once(sub)).collect();
                let (r, _) = menu::submenu(ui, sub, |ui| entries(ui, commands, &deeper, checked, tags, chosen));
                tags.tag(&format!("menu-{}", deeper.join("-")), &r);
                continue;
            }
            Row::Separator => {
                menu::separator(ui);
                continue;
            }
        };
        let mut item = Item::new(c.label).choice(choices.then_some(c.id == checked));
        if let Some(keys) = c.shortcut {
            item = item.shortcut(keys);
        }
        let mut r = ui.add_enabled(c.enabled, item);
        let mut value = json!({ "label": c.label, "shortcut": c.shortcut });
        if c.id == checked {
            value["checked"] = json!(true);
        }
        if let Some(why) = &c.disabled_reason {
            r = r.hint_disabled(why.as_str());
            value["why"] = json!(why);
        }
        // A menu's first frame is a sizing pass, which reads every widget as disabled.
        tags.tag_value_as(&format!("menu-{}", c.id), &r, value, c.enabled);
        if r.clicked() {
            *chosen = Some(c.id);
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &'static str, menu: &'static [&'static str]) -> CommandInfo {
        CommandInfo { id, label: id, menu, shortcut: None, params: "{}", enabled: true, disabled_reason: None }
    }

    #[test]
    fn known_menus_come_first_then_the_others() {
        let commands = vec![info("a", &["Tools"]), info("b", &["Query"]), info("c", &["File"]), info("d", &[]), info("e", &["Help"])];
        assert_eq!(tops(&commands), ["File", "Query", "Tools", "Help"]);
    }

    /// Edit, in light and dark: Undo, Redo | Copy, Select All |
    /// Apply, Reset.
    #[test]
    fn the_edit_menu_has_the_pages_groups() {
        let home = dbee_testkit::temp_home(None, None);
        let options = dbee_engine::SessionOptions { config_dir: Some(home.home.join("config")), ..dbee_engine::SessionOptions::default() };
        let app = DbeeApp::new(dbee_engine::Session::new(options).unwrap(), None);
        let commands = app.commands();
        let edit: Vec<&str> = rows(&commands, &["Edit"])
            .iter()
            .map(|row| match row {
                Row::Entry(c) => c.id,
                Row::Submenu(sub) => sub,
                Row::Separator => "|",
            })
            .collect();
        assert_eq!(edit, ["edit.undo", "edit.redo", "|", "clipboard.copy", "selection.all", "|", "edit.apply", "edit.reset"]);
    }

    #[test]
    fn an_entry_runs_with_its_shortcut_params() {
        assert_eq!(params_for(Some("Ctrl+C")), json!({ "format": "tsv" }));
        assert_eq!(params_for(Some("Ctrl+Shift+Enter")), json!({ "current": true }));
        assert_eq!(params_for(Some("Ctrl+T")), json!({}));
        assert_eq!(params_for(None), json!({}));
        assert_eq!(params_for(Some("Hyper+Q")), json!({}));
    }
}

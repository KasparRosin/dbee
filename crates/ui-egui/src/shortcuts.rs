//! Keyboard shortcuts: [`SHORTCUTS`] binds key text to a command and its params.
//!
//! The key text is what the menus show (`CommandSpec.shortcut`, e.g. `Ctrl+Shift+Enter`), with
//! `Ctrl` meaning [`Modifiers::COMMAND`] (Cmd on macOS). A command's `shortcut` names one row
//! here; a row may add params (`Ctrl+Shift+Enter` runs `query.run` with `current`).
//!
//! The app handles them each frame before the screens draw ([`handle`]), so a bound key never
//! also reaches a widget. Matching is exact: Ctrl+Z does not fire on Ctrl+Shift+Z. While a text
//! field has keyboard focus, the keys a text field uses itself ([`TEXT_KEYS`]: copy, select all,
//! undo, redo) are left to it; the others always fire. The keyboard's Ctrl+C reaches egui as a
//! copy event, not a key, so that is matched too, with Shift held for Ctrl+Shift+C.
//!
//! An open cell editor ([`crate::editors`]) of text counts as a focused text field; the boolean
//! choice has no text, so the text keys run there (after committing it). Its Enter keys
//! ([`EDITOR_KEYS`]) are the editor's own: it takes Ctrl+Enter as commit before this step, and
//! Ctrl+Shift+Enter does nothing there. Any shortcut that runs commits the editor first, so
//! Ctrl+S writes the value being typed.
//!
//! The value panel ([`crate::value_panel`]) comes before them, unless a text field or a text
//! editor has the keyboard: with text selected in it, the copy is its own (Ctrl+C copies that
//! text and `clipboard.copy` does not run), and while it has the keyboard Ctrl+A selects its
//! text, not the grid's cells.

use std::sync::LazyLock;

use egui::{Event, Key, Modifiers};
use serde_json::{Value as Json, json};

use crate::{DbeeApp, editors, value_panel};

/// `(keys, command id, params)`.
pub static SHORTCUTS: LazyLock<Vec<(&'static str, &'static str, Json)>> = LazyLock::new(|| {
    vec![
        ("Ctrl+T", "tab.newQuery", json!({})),
        ("Ctrl+W", "tab.close", json!({})),
        ("Ctrl+Enter", "query.run", json!({})),
        // The query editor adds `cursor` (see `DbeeApp::run_shortcut`).
        ("Ctrl+Shift+Enter", "query.run", json!({ "current": true })),
        ("Ctrl+S", "edit.apply", json!({})),
        ("Ctrl+Z", "edit.undo", json!({})),
        ("Ctrl+Shift+Z", "edit.redo", json!({})),
        ("Ctrl+A", "selection.all", json!({})),
        ("Ctrl+C", "clipboard.copy", json!({ "format": "tsv" })),
        ("Ctrl+Shift+C", "clipboard.copy", json!({ "format": "tsvHeader" })),
        ("Ctrl+P", "ui.quickSearch", json!({})),
        ("Ctrl+Shift+Q", "connection.close", json!({})),
    ]
});

/// The shortcuts a focused text field keeps for itself.
pub const TEXT_KEYS: &[&str] = &["Ctrl+C", "Ctrl+Shift+C", "Ctrl+A", "Ctrl+Z", "Ctrl+Shift+Z"];

/// The shortcuts an open cell editor keeps from running: Enter is the editor's.
pub const EDITOR_KEYS: &[&str] = &["Ctrl+Enter", "Ctrl+Shift+Enter"];

/// Whether the shortcut `keys` runs its command, given that a text field has keyboard focus.
pub fn fires(keys: &str, text_focused: bool) -> bool {
    !(text_focused && TEXT_KEYS.contains(&keys))
}

/// [`fires`], with whether a cell editor is open, and whether it edits text (which counts as a
/// focused text field).
pub fn fires_while(keys: &str, text_focused: bool, editing: bool, editing_text: bool) -> bool {
    fires(keys, text_focused || editing_text) && !(editing && EDITOR_KEYS.contains(&keys))
}

/// `Ctrl+Shift+Enter` → the modifiers and the key; `None` for text that names no key.
pub fn parse(keys: &str) -> Option<(Modifiers, Key)> {
    let mut parts: Vec<&str> = keys.split('+').collect();
    let key = Key::from_name(parts.pop()?)?;
    let mut modifiers = Modifiers::NONE;
    for part in parts {
        modifiers |= match part {
            "Ctrl" => Modifiers::COMMAND,
            "Shift" => Modifiers::SHIFT,
            "Alt" => Modifiers::ALT,
            _ => return None,
        };
    }
    Some((modifiers, key))
}

/// Removes the first press of `pattern`+`key` from `events`; `true` when there was one. `held` is
/// the frame's modifiers, which a copy event (the keyboard's Ctrl+C) carries none of.
pub(crate) fn take(events: &mut Vec<Event>, pattern: Modifiers, key: Key, held: Modifiers) -> bool {
    let copy = key == Key::C && pattern.command;
    let at = events.iter().position(|e| match e {
        Event::Key { key: k, pressed: true, modifiers, .. } => *k == key && modifiers.matches_exact(pattern),
        Event::Copy => copy && held.matches_exact(pattern),
        _ => false,
    });
    match at {
        Some(i) => {
            events.remove(i);
            true
        }
        None => false,
    }
}

/// Runs the commands of this frame's shortcut presses (step 4 of the frame).
pub(crate) fn handle(app: &mut DbeeApp, ctx: &egui::Context) {
    let text_focused = ctx.text_edit_focused();
    let editor = editors::open_kind(app);
    let (editing, editing_text) = (editor.is_some(), editor.is_some_and(|k| k != editors::Kind::Bool));
    if !text_focused && !editing_text {
        value_panel::take_keys(app, ctx);
    }
    let mut fired: Vec<(&'static str, Json)> = Vec::new();
    ctx.input_mut(|i| {
        let held = i.modifiers;
        for (keys, id, params) in SHORTCUTS.iter() {
            if !fires_while(keys, text_focused, editing, editing_text) {
                continue;
            }
            let Some((pattern, key)) = parse(keys) else { continue };
            while take(&mut i.events, pattern, key, held) {
                fired.push((id, params.clone()));
            }
        }
    });
    if !fired.is_empty() {
        editors::commit(app, ctx);
    }
    for (id, params) in fired {
        app.run_shortcut(id, params);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Key, Modifiers};

    use super::*;

    #[test]
    fn key_text_parses_with_ctrl_as_command() {
        assert_eq!(parse("Ctrl+T"), Some((Modifiers::COMMAND, Key::T)));
        assert_eq!(parse("Ctrl+Shift+Enter"), Some((Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter)));
        assert_eq!(parse("Ctrl+Shift+Q"), Some((Modifiers::COMMAND | Modifiers::SHIFT, Key::Q)));
        assert_eq!(parse("Alt+F4"), Some((Modifiers::ALT, Key::F4)));
        assert_eq!(parse("F5"), Some((Modifiers::NONE, Key::F5)));
        for bad in ["", "Ctrl+", "Ctrl+Nope", "Hyper+T", "Ctrl+T+U", "+"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_row_parses_and_no_two_rows_share_keys() {
        let mut seen = std::collections::HashSet::new();
        for (keys, id, params) in SHORTCUTS.iter() {
            let parsed = parse(keys);
            assert!(parsed.is_some(), "{keys} ({id})");
            assert!(seen.insert(parsed), "{keys} is bound twice");
            assert!(params.is_object(), "{keys}");
        }
        let row = |keys: &str| SHORTCUTS.iter().find(|(k, ..)| *k == keys).map(|(_, id, p)| (*id, p.clone()));
        assert_eq!(row("Ctrl+Enter"), Some(("query.run", serde_json::json!({}))));
        assert_eq!(row("Ctrl+Shift+Enter"), Some(("query.run", serde_json::json!({ "current": true }))));
        assert_eq!(row("Ctrl+C"), Some(("clipboard.copy", serde_json::json!({ "format": "tsv" }))));
        assert_eq!(row("Ctrl+Shift+C"), Some(("clipboard.copy", serde_json::json!({ "format": "tsvHeader" }))));
        assert_eq!(row("Ctrl+P"), Some(("ui.quickSearch", serde_json::json!({}))));
    }

    #[test]
    fn text_keys_yield_to_a_focused_text_field() {
        for keys in ["Ctrl+C", "Ctrl+Shift+C", "Ctrl+A", "Ctrl+Z", "Ctrl+Shift+Z"] {
            assert!(fires(keys, false), "{keys}");
            assert!(!fires(keys, true), "{keys} is the text field's while it has focus");
        }
        for keys in ["Ctrl+Enter", "Ctrl+Shift+Enter", "Ctrl+S", "Ctrl+P", "Ctrl+T", "Ctrl+W", "Ctrl+Shift+Q"] {
            assert!(fires(keys, false) && fires(keys, true), "{keys} always fires");
        }
    }

    #[test]
    fn an_open_cell_editor_keeps_enter_and_the_text_keys() {
        for keys in ["Ctrl+Enter", "Ctrl+Shift+Enter", "Ctrl+Z", "Ctrl+Shift+Z", "Ctrl+A", "Ctrl+C"] {
            assert!(!fires_while(keys, false, true, true), "{keys} is the text editor's");
            assert!(fires_while(keys, false, false, false), "{keys} fires with no editor");
        }
        for keys in ["Ctrl+S", "Ctrl+P", "Ctrl+T", "Ctrl+W"] {
            assert!(fires_while(keys, true, true, true), "{keys} still fires (after committing the editor)");
        }
        // The boolean choice has no text: the text keys run (after committing it), Enter stays.
        for keys in ["Ctrl+Z", "Ctrl+Shift+Z", "Ctrl+C", "Ctrl+A"] {
            assert!(fires_while(keys, false, true, false), "{keys} runs over the boolean choice");
        }
        for keys in ["Ctrl+Enter", "Ctrl+Shift+Enter"] {
            assert!(!fires_while(keys, false, true, false), "{keys} is the choice's");
        }
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn matching_takes_exact_modifiers_and_consumes_the_event() {
        let ctrl = Modifiers::COMMAND | Modifiers::CTRL;
        let mut events = vec![key(Key::Z, ctrl | Modifiers::SHIFT), key(Key::T, ctrl), egui::Event::Text("t".into())];
        // Ctrl+Z does not match Ctrl+Shift+Z.
        assert!(!take(&mut events, Modifiers::COMMAND, Key::Z, Modifiers::NONE));
        assert!(take(&mut events, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z, Modifiers::NONE));
        assert!(take(&mut events, Modifiers::COMMAND, Key::T, Modifiers::NONE));
        assert_eq!(events, vec![egui::Event::Text("t".into())]);
        // A copy event from the keyboard is Ctrl+C, or Ctrl+Shift+C with Shift held.
        let mut events = vec![egui::Event::Copy];
        assert!(!take(&mut events, Modifiers::COMMAND | Modifiers::SHIFT, Key::C, ctrl));
        assert!(take(&mut events, Modifiers::COMMAND, Key::C, ctrl));
        assert!(events.is_empty());
        let mut events = vec![egui::Event::Copy];
        assert!(take(&mut events, Modifiers::COMMAND | Modifiers::SHIFT, Key::C, ctrl | Modifiers::SHIFT));
        // A released key is not a press.
        let mut events = vec![egui::Event::Key { key: Key::T, physical_key: None, pressed: false, repeat: false, modifiers: ctrl }];
        assert!(!take(&mut events, Modifiers::COMMAND, Key::T, Modifiers::NONE));
    }
}

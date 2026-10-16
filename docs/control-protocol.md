# dbee control protocol

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

**All of this is planned.** The desktop app will take a control port, so agents and tests can drive the real window:

```sh
dbee --control 7979            # or DBEE_CONTROL_PORT=7979
```

The app listens on `127.0.0.1` only. Send newline-delimited JSON; every request line gets one reply line:

```json
{"id":1,"method":"engine.execute","params":{"command":"connection.open","params":{"name":"demo"}}}
{"id":1,"ok":true,"result":{"connection":"demo"}}
```

Errors come back as `{"id":…,"ok":false,"error":"…"}`. A line that is not JSON gets an error reply and the server keeps serving.

## Methods

| Method | Params | What it does |
|---|---|---|
| `engine.execute` | `{command, params}` | Runs any engine command, including UI-only ones. |
| `engine.commands` | `{search?}` | Every command: id, label, menu path, shortcut, params doc, enabled now. |
| `engine.journal` | | Commands run so far (replayable). Passwords are left out. |
| `document.inspect` | | Session summary: connections, open tabs, selection, undo labels. |
| `ui.inspect` | | UI state: open dialog, active panel, window size, scroll. |
| `ui.click` | `{id}` or `{x, y, button?, count?, modifiers?}` | Real pointer input by automation id or window point. |
| `ui.key` | `{key, shift?, cmd?, alt?, ctrl?}` | Key or shortcut. |
| `ui.text` | `{text}` | Type into the focused field. |
| `ui.screenshot` | `{path?}` | PNG of the window (base64 when no path). |
| `ui.dialog.*` | e.g. `{name}`, `{field, value}` | Open, fill in, confirm and cancel dialogs. |
| `app.open` | `{path}` | Open a file (script, project, SQL file). |
| `app.quit` | | Quit the app. |

## Rules

- Programmatic calls never open dialogs. `engine.execute` with empty params runs with defaults or returns an error; only menu-style invocation (`ui.*`) opens a dialog.
- Params are hostile input: wrong types, missing keys and deep nesting are errors, never panics ([never crash](standards/never-crash.md)).
- Connection passwords are never echoed in replies or the journal.

# dbee control protocol

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-20 · **Change:** major (the channel exists: M1.20)

The desktop app takes a control port, so agents and tests can drive the real window:

```sh
dbee --control 7979            # or DBEE_CONTROL_PORT=7979
cargo xtask run-isolated       # the same on a throwaway home (docs/agents.md §4)
```

The app listens on `127.0.0.1` only, before the window opens: a port in use stops the start, so an agent never drives another dbee by mistake. Any local process can connect, so it is meant for agents and tests. Send newline-delimited JSON; every request line gets one reply line, in order:

```json
{"id":1,"method":"engine.execute","params":{"command":"tab.newQuery","params":{"text":"select 1"}}}
{"id":1,"ok":true,"result":{"tab":1}}
```

Errors look like `{"id":…,"ok":false,"error":"…","code":"…","data":{…}}`. `code` is stable: an engine error's code (`disabled`, `badParams`, `unknownCommand`, `needsPassword`, `connectionLost`, `server`, `busy`, `timeout`, …) with its `data` (`{command, why}` for `disabled`), or one of the channel's own: `invalidJson`, `badRequest` (not a request object, no `method`), `unknownMethod`, `badParams`, `notFound` (no such automation id), `lineTooLong`, `timeout`, `busy` (too many connections or too much queued input), `closed` (the app is closing), `screenshot`. A line that is not JSON gets an error reply and the server keeps serving; blank lines get none.

- A request line holds at most 64 MiB.
- `params` is an object (or left out). Every method takes `timeoutMs` (default 60,000, at most 600,000): how long its reply may wait. A reply that waits longer is a `timeout` error; a job it waited for goes on.
- Requests are handled in the window's frames, so the window must be shown (not minimised) for replies to come.

## Methods

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `{command, params?, wait? = true}` | Runs any command, engine or UI-only (`DbeeApp::run`). A command that starts a job is answered when the job's result is applied; with `wait: false` the reply is `{job}` at once. |
| `engine.commands` | `{search?}` | Every command: `{id, label, menu, shortcut, params, enabled, disabledReason}`. `search` keeps those whose id, label or a menu contains it, ignoring case. |
| `engine.journal` | | `[{command, params}]`: the commands run so far (replayable), without secret params. |
| `engine.wait` | | `{}` once no job runs. |
| `document.inspect` | | The session summary (`Session::inspect`): screen, connection, tabs, the active tab, selection, value view, running jobs. Never a secret. |
| `ui.inspect` | | `{screen, dialog, window: [width, height], frame, automation: [{id, rect: [x, y, width, height]}]}`. `frame` counts the frames drawn; `automation` lists the widgets of the last drawn frame, in window points. |
| `ui.click` | `{id}` or `{x, y}`, then `button?` (`left`, `right`, `middle`), `count?` (1–3), `modifiers?` (`{shift?, cmd?, alt?, ctrl?}`) | `{x, y}`, once the frame that got the click has run. `{id}` clicks the centre of that automation id. |
| `ui.key` | `{key, shift?, cmd?, alt?, ctrl?}` | `{}`, once the key was handled. `key` is egui's name (`Enter`, `Escape`, `ArrowDown`, `A`, `F2`; `esc`, `down` and the like work too). `cmd` is Cmd on macOS and Ctrl elsewhere; with it, `C` and `X` arrive as copy and cut, as from the keyboard. Type text with `ui.text`. |
| `ui.text` | `{text}` | `{}`: the text typed into the focused field. |
| `ui.screenshot` | `{path?}` | A PNG of the window: written to `path` (relative to the app's working directory, folders created) as `{path, width, height}`, or `{base64, width, height}`. |
| `app.quit` | | `{}`; the window closes. |

Planned: `ui.dialog.*` (open, fill in, confirm and cancel dialogs) and `app.open {path}`.

## Automation ids

Widgets agents drive have kebab-case ids, listed by `ui.inspect` and clicked with `ui.click {id}`. Each UI task lists its ids; today: `conn-new` (the connection screen's New connection button) and `notice` (the notice shown at the bottom right).

## Rules

- Programmatic calls never open dialogs. `engine.execute` with empty params runs with defaults or returns an error; a command that needs an answer (a password, a host key) returns a `needs…` error with the details in `data`, and is run again with the answer in its params.
- Params are hostile input: wrong types, missing keys and deep nesting are errors, never panics ([never crash](standards/never-crash.md)).
- Passwords are never echoed in replies or the journal. The log names methods and command ids only, never params, SQL or values.

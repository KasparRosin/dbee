# dbee control protocol

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-23 · **Change:** minor (the workspace's ids)

The desktop app takes a control port, so agents and tests can drive the real window:

```sh
dbee --control 7979            # or DBEE_CONTROL_PORT=7979
cargo xtask run-isolated       # the same on a throwaway home (docs/agents.md §4)
```

The app listens on `127.0.0.1` only, before the window opens: a port in use stops the start, so an agent never drives another dbee by mistake. Any local process can connect, so it is meant for agents and tests. HTTP requests are refused (an `httpRefused` reply, then the connection closes, with nothing run), so web pages cannot use the channel through the browser. Send newline-delimited JSON; every request line gets one reply line, in order:

```json
{"id":1,"method":"engine.execute","params":{"command":"tab.newQuery","params":{"text":"select 1"}}}
{"id":1,"ok":true,"result":{"tab":1}}
```

Errors look like `{"id":…,"ok":false,"error":"…","code":"…","data":{…}}`. `code` is stable: an engine error's code (`disabled`, `badParams`, `unknownCommand`, `needsPassword`, `connectionLost`, `server`, `busy`, `timeout`, …) with its `data` (`{command, why}` for `disabled`), or one of the channel's own: `invalidJson`, `badRequest` (not a request object, no `method`), `unknownMethod`, `badParams`, `notFound` (no such automation id), `lineTooLong`, `httpRefused`, `timeout`, `busy` (too many connections or too much queued input), `closed` (the app is closing), `screenshot`. A line that is not JSON gets an error reply and the server keeps serving; blank lines get none.

- A request line holds at most 64 MiB.
- `params` is an object (or left out). Every method takes `timeoutMs` (default 60,000, at most 600,000): how long its reply may wait. A reply that waits longer is a `timeout` error; a job it waited for goes on. A request still waiting to be handled when its reply timed out is not run.
- Requests are handled in the window's frames, so the window must be shown (not minimised) for replies to come.

## Methods

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `{command, params?, wait? = true}` | Runs any command, engine or UI-only (`DbeeApp::run`). A command that starts a job is answered when the job's result is applied; with `wait: false` the reply is `{job}` at once. |
| `engine.commands` | `{search?}` | Every command: `{id, label, menu, shortcut, params, enabled, disabledReason}`. `search` keeps those whose id, label or a menu contains it, ignoring case. |
| `engine.journal` | | `[{command, params}]`: the commands run so far (replayable), without secret params. |
| `engine.wait` | | `{}` once no job runs. |
| `document.inspect` | | The session summary (`Session::inspect`): screen, connection, tabs, the active tab, selection, value view, running jobs. Never a secret. |
| `ui.inspect` | | `{screen, dialog, window: [width, height], frame, automation: [{id, rect: [x, y, width, height], visible?, value?, masked?, set?, enabled?}]}`. `frame` counts the frames drawn; `dialog` names the open dialog (`fillFromUrl`, `hostKey`, `passphrase`, `password`, `sshPassword`, `confirm`, `deleteConnection`) or is `null`; `automation` lists the widgets of the last drawn frame, in window points. `rect` is the part of the widget in view; a widget scrolled out of view has `visible: false` and its whole rect. `value` is what a widget shows (a field's text, a check box's state, a choice, a message); a password, passphrase or URL field has `masked: true` and `set` (whether it is filled in) instead, never its text. A disabled widget has `enabled: false`; this holds in the first frame of a dialog or a menu too, which egui draws all disabled. |
| `ui.click` | `{id}` or `{x, y}`, then `button?` (`left`, `right`, `middle`), `count?` (1–3), `modifiers?` (`{shift?, cmd?, alt?, ctrl?}`) | `{x, y}`, once the frame that got the click has run. `{id}` clicks the centre of that automation id; a widget scrolled out of view is scrolled into view first (`notFound` when that fails within 30 frames). |
| `ui.key` | `{key, shift?, cmd?, alt?, ctrl?}` | `{}`, once the key was handled. `key` is egui's name (`Enter`, `Escape`, `ArrowDown`, `A`, `F2`; `esc`, `down` and the like work too). `cmd` is Cmd on macOS and Ctrl elsewhere; with it, `C` and `X` arrive as copy and cut, as from the keyboard. Type text with `ui.text`. |
| `ui.text` | `{text}` | `{}`: the text typed into the focused field. |
| `ui.screenshot` | `{path?}` | A PNG of the window: written to `path` (relative to the app's working directory, folders created) as `{path, width, height}`, or `{base64, width, height}`. |
| `app.quit` | | `{}`; the window closes a frame later, after the reply is written. |

Planned: `ui.dialog.*` (open, fill in, confirm and cancel dialogs) and `app.open {path}`.

## Automation ids

Widgets agents drive have kebab-case ids, listed by `ui.inspect` and clicked with `ui.click {id}`. Each UI task lists its ids; today:

- the connection screen's lists: `conn-new`, `conn-filter`, `conn-store-warning`, `conn-saved-<id>` and `conn-recent-<n>` (cards; right-click for `conn-menu-connect`, `conn-menu-edit`, `conn-menu-duplicate`, `conn-menu-delete`);
- its form: `conn-title`, `conn-url`, `conn-name`, `conn-color` (and `conn-color-<colour>`), `conn-host`, `conn-port`, `conn-user`, `conn-password`, `conn-password-show`, `conn-save-secrets`, `conn-no-keyring`, `conn-database`, `conn-ssl` (and `conn-ssl-<mode>`), `conn-ca`, `conn-ca-choose`, `conn-ca-clear`, `conn-readonly`;
- its SSH tunnel: `conn-ssh-on`, `conn-ssh-host`, `conn-ssh-port`, `conn-ssh-user`, `conn-ssh-auth` (and `conn-ssh-auth-key`, `-agent`, `-password`), `conn-ssh-key` (what the login will use), `conn-ssh-change`, `conn-ssh-auto`;
- its buttons and their message: `conn-save`, `conn-test`, `conn-connect`, then `conn-error` (something failed), `conn-result` (a test passed, a save) or `conn-note` (anything else, such as a note about a file picker);
- dialogs: `dialog-ok`, `dialog-cancel`, `dialog-text`, `dialog-url`, `dialog-error`, `dialog-fingerprint`, `dialog-secret`, `dialog-save`;
- the workspace's menu bar: `menu-File`, `menu-Edit`, `menu-View`, `menu-Query` (one per top menu the commands name) and, while a menu is open, `menu-<command id>` (`menu-tab.newQuery`) with the value `{label, shortcut, why?}`, `why` saying why a disabled entry cannot run now. An entry runs its command as a button does, with its shortcut's params;
- its schema tree: `tree-filter`, `tree-refresh`, `tree-system` (the system schemas' group), `tree-schema-<schema>`, `tree-table-<schema>.<table>` (value: its kind; a double-click opens it), `tree-expand-<schema>.<table>` (shows its columns) and `tree-column-<schema>.<table>.<column>` (value `{type, key}`, `key` being `primary`, `foreign` or null). Schema and table rows out of view are listed with `visible: false`, and a click scrolls them into view first;
- its tabs: `tab-<id>` (value: the title as shown, `orders [2]` when 2 filters are applied; a middle click closes it), `tab-close-<id>`, `tab-dirty-<id>` (the dot of a tab whose query text or staged edits closing would lose), `tab-new`, and the active query tab's editor `query-editor`;
- its status bar: `status-connection` (value: the connection's name, with `read-only` when the session is), `status-server`, `status-job` (value: what runs in the active tab, or a reconnect) with `status-cancel`, and `status-lost` with `status-reconnect` while the connection is lost;
- the quick search (Ctrl+P): `qs-input` and `qs-item-<n>` (value `schema.table`, best first, `n` from 0). ArrowDown and ArrowUp move, Enter opens, Escape closes;
- `notice` (the notice shown at the bottom right).

The file pickers (`conn-ssh-change`, `conn-ca-choose`) open the desktop's picker, which agents cannot drive: save a connection with the path instead (`connection.save`) and click its card.

## Rules

- Programmatic calls never open dialogs. `engine.execute` with empty params runs with defaults or returns an error; a command that needs an answer (a password, a host key) returns a `needs…` error with the details in `data`, and is run again with the answer in its params.
- Params are hostile input: wrong types, missing keys and deep nesting are errors, never panics ([never crash](standards/never-crash.md)).
- Passwords are never echoed in replies or the journal. The log names methods and command ids only, never params, SQL or values.

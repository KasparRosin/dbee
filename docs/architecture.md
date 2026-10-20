# Architecture

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-20 · **Change:** minor (engine core)

How dbee is built, and how it will be built. Rules for contributors are in [`AGENTS.md`](../AGENTS.md). Anything not yet built is marked **(planned)**.

## Workspace

An engine-first Cargo workspace. Layering is enforced by `cargo xtask layers`; nothing below L6 depends on egui, eframe, winit, rfd or wgpu, so the UI crate is swappable.

| Path | Status | Job |
|---|---|---|
| `crates/core` | exists | L0. Cells, result sets, the schema model (`Catalog`, `TableInfo`), grid text and the clipboard copy formats. Plain values with serde shapes; no I/O. |
| `crates/testkit` | exists | L0, a dev-dependency only. Finds the Docker test servers (`cargo xtask testdb`), the skip rule (`require_testdb!`), a `psql` helper, unique names and throwaway home directories. |
| `crates/sql` | exists | L1. Identifier and literal quoting, the SQL lexer, statement splitting. Pure text; no I/O. |
| `crates/config` | exists | L1. Connection settings, URL parsing, the saved and recent connections file, and the secret stores (OS keyring, in-memory). No passwords in the file. |
| `crates/ssh` | exists | L2. The SSH tunnel (russh with ring): host key check against `known_hosts` (unknown hosts ask once and can be learned, changed keys are refused), key, agent and password login, and a direct-tcpip channel per connection. Everything it reads from the machine comes in through `SshEnv`. |
| `crates/pg` | exists | L3. The Postgres connection (tokio-postgres): TCP or a channel through the SSH tunnel, TLS with rustls (ring) and the OS certificate verifier, `application_name=dbee`, `standard_conforming_strings = on`, read-only mode, cancel requests, and a simple-query runner that returns values as text exactly as Postgres prints them. |
| `crates/engine` | exists | L5. `Session`: the session state, the command registry (`cmd/*`, so far the tab commands), background jobs on a tokio runtime, the panic guard and the journal. |
| `crates/ui-egui` | exists, placeholder | L6. The egui UI and the control channel. |
| `crates/mcp` | exists, empty | L6. MCP server over stdio. |
| `apps/dbee` | exists | L7. Desktop binary; opens a placeholder window. |
| `apps/dbee-cli` | exists | L7. CLI; prints its version. |
| `xtask` | exists | Exempt from layering. `ci`, `assets`, `layers`, `licenses`, `provenance`, `testdb`, `version`, `stats`, `ico`, `icns`, `contributors`. `xtask/testdb/` holds the test servers' compose file, tools image and seed. |
| L1–L4 domain crates | **(planned)** | Named when the first features land, for example `sql`, `drivers`, `model`. |

## Layers

| Layer | Crates | Job |
|---|---|---|
| L0–L4 | `core`, `testkit` (L0, tests only); `sql`, `config` (L1); `ssh` (L2); `pg` (L3); domain crates **(planned)**, e.g. `drivers`, `model` | Values and types, SQL parsing and quoting, database drivers and wire protocols, the connection and schema model. No UI, no engine. |
| L5 | `engine` | Session, commands, undo, journal. Depends on L0–L4 only. |
| L6 | `ui-egui`, `mcp` | Front ends over the engine. egui and friends are allowed here and above (`ui-egui` for the UI, `mcp` for UI control), never below. |
| L7 | `apps/dbee`, `apps/dbee-cli` | Binaries that wire a front end to the engine. |
| — | `xtask` | Build tooling (exempt). |

A crate may depend only on lower layers. Register every new crate in `xtask/src/layers.rs` and in this table in the same change.

## Data flow

1. **Every action is a command.** The UI, CLI, control channel and MCP all call `app.run(id, params)`, which reaches `Session::execute`. A command (`crates/engine/src/cmd/*`, one `SPECS` table per module) has an id, label, menu path, shortcut text, params doc, `enabled` and `run`. Ids follow Beekeeper Studio's menus plus primitives (`connection.open`, `query.run`, `table.open`). UI-only commands live in `ui-egui`.
2. **Execute.** Errors carry a stable `code()` and JSON `data()`; a command that needs an answer (a password, a host key, a confirmation) returns a `needs…` error and is run again with it. `Session::execute(id, params)`:
   1. finds the command (`unknownCommand`) and checks `enabled` (`disabled`, with the reason);
   2. checks that params is a JSON object (`null` counts as `{}`); the `cmd::p` helpers check each key (`badParams` naming it);
   3. snapshots the session state, a cheap clone (large data sits behind `Arc`), and runs the command under `catch_unwind`. A panic restores the snapshot and returns `internal`, and jobs the command started are orphaned: they run to their end and their results are dropped ([never crash](standards/never-crash.md));
   4. records a successful journaled command in the **journal**.
3. **Jobs.** A command either finishes (`Outcome::Done`) or starts a job (`Outcome::Started`). Jobs are futures on the session's tokio runtime (2 worker threads named `dbee-db`). A job's future ends in an `Apply` closure; `Session::pump`, which the UI will call every frame **(planned)**, runs each `Apply` on the UI thread under the same panic guard, so the window never waits on the database. `wait`, `wait_idle` and `run_blocking` serve tests and the control channel. `notify` (the UI's repaint request) is called after each result. A job is never aborted: it runs to its end, a Postgres apply is never abandoned mid-transaction, and database work is stopped through the Postgres cancel request. A result whose tab has gone is ignored. Dropping the session shuts the runtime down without waiting (safe in async code too); jobs still running are dropped with it.
4. **Journal.** Successful journaled commands with their params, in memory and replayable; the localhost control channel will serve it **(planned)**. Secret params (`password`, `passphrase`, and keys ending in `Password` or `Passphrase`) are left out at any depth. It keeps the newest 10,000 entries at most (it drops the oldest 5,000), and consecutive `query.setText` entries replace each other. Logs carry command ids, job labels, error codes and timings only: never params, secrets, cell values or SQL text.
5. **The UI is thin.** Panels read `Session::state()` and act only through commands. `Session::inspect()` is the JSON summary agents read (`document.inspect`).
6. **Programmatic calls never open dialogs.** Only menu-style invocation opens a dialog (`UiRequest::Dialog`, collected with `take_ui_requests`).

## Agent control

All **(planned)**:

- Control channel: `dbee --control <port>`, JSON lines on `127.0.0.1`: [`control-protocol.md`](control-protocol.md).
- CLI: [`cli.md`](cli.md).
- MCP: `dbee-cli mcp`, headless or bridged to a running app: [`mcp.md`](mcp.md).

## Platforms

Desktop only: macOS, Windows, Linux and FreeBSD. There is no web build and no wasm target, because a browser cannot open database connections. See [`releasing.md`](releasing.md) for the platform matrix and [`decisions.md`](decisions.md).

## Known gaps

Most of M1. The domain crates and the engine core exist; the engine has only its tab commands so far, and no UI, query editor, result grid or schema browser exists. The ranked work list is [`gaps.md`](gaps.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-20 | minor | `crates/engine` (L5): session, command registry, jobs, panic guard, journal; data flow describes what exists |
| 2026-10-19 | minor | `crates/pg` (L3): Postgres connection with TLS, SSH tunnel, read-only mode and cancel |
| 2026-10-19 | minor | `crates/ssh` (L2): SSH tunnel with host key check, key, agent and password login |
| 2026-10-19 | minor | `crates/config` (L1): connection settings, saved list, secret stores |
| 2026-10-19 | minor | `crates/sql` (L1): quoting, lexer, statement splitting |
| 2026-10-19 | minor | `crates/core` (L0): cells, schema model, grid text, copy formats |
| 2026-10-19 | minor | `crates/testkit` (L0) and `cargo xtask testdb` |
| 2026-10-15 | major | Rewritten for dbee: workspace, layers, data flow, agent control, platforms, known gaps |

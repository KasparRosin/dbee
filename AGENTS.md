# dbee — instructions for agents

dbee is a clean-room, open-source, Rust-native desktop database client targeting Beekeeper Studio parity — and superiority (speed, openness, agent control). It runs natively on macOS, Windows, Linux and FreeBSD. There is no browser version: a browser cannot open database connections.

Shared standards live in [`docs/standards/`](docs/standards/): never crash, progress docs, licensing. Read them at the start of a session and follow them.

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next task), then the task in `plan/execution-plan.md` and the relevant `plan/architecture.md` section. Behaviour reference: `plan/beekeeper/*.md` (`01-observed-ui.md` holds measured observations of the running app).
2. Follow the autonomous operation protocol (`plan/execution-plan.md` §7). Don't stop to ask unless §7 lists the decision as the user's.

`plan/` is gitignored (local only).

## Assets and iconography — absolute rule
**No Beekeeper Studio (or any other vendor's) iconography, images, artwork, fonts, templates, themes, sample databases or help text — ever.** This is the most important rule in this repository; breaking it is a serious failure.
- Every asset in the repo is original work by dbee contributors, or public domain / CC0, or open source / Creative Commons that allows redistribution (or licensed open source by the contributor who created it).
- Every asset file has a row in [`ATTRIBUTION.md`](ATTRIBUTION.md) (author, source, licence). `cargo xtask assets` enforces it. Adding an asset without a row is a bug.
- Prefer art generated in code: the UI icon set, colour themes and sample databases are drawn or defined in code and original.
- Never copy Beekeeper Studio's icons, themes, connection-dialog artwork, sample data or wording beyond feature names. Feature *names* (menu labels, setting names) are fine.
- Screenshots of Beekeeper Studio are never committed (they live only in `plan/beekeeper/screenshots/`).

## Never crash
People trust dbee with their databases; a crash can lose an unsaved query or leave a transaction open. **This outranks feature work.** Standard: [`docs/standards/never-crash.md`](docs/standards/never-crash.md).
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`).
- **Errors are `Result<T, E>`** through the crate's error type and `?`.
- **Input-derived numbers are hostile** (server responses, result sets, files, commands, MCP/control params): `get()` instead of indexing, checked/saturating arithmetic, cap input-sized allocations (huge result sets, wide rows, large BLOBs).
- **Bound recursion** (nested JSON values, recursive schema relationships, nested queries).
- **Last-resort guard:** `Session::execute` catches escaped panics and restores the session. It's a safety net, not a licence.
- **Prove it:** every crash fix lands with a regression test. The engine's first commands bring `every_command_survives_empty_params` (fuzzes every command) and a command-sequence fuzz test against undo/redo invariants.
- Crates carry `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.

## Non-negotiables
- **Clean-room.** Beekeeper Studio is installed on the dev machine and may be *observed* black-box, started only through `plan/beekeeper/observe.sh`: run it, use its UI with synthetic databases, take screenshots (by window id) stored only under `plan/beekeeper/screenshots/`. Never read, disassemble or copy anything inside its bundle (`app.asar`, resources, plugins), its source repository or its network traffic; never commit files produced by Beekeeper Studio; never copy GPL/AGPL/LGPL code, and don't read the sources of other database GUIs (DBeaver, pgAdmin, HeidiSQL, DbGate…). Wire protocols and SQL behaviour come from public specifications (PostgreSQL and MySQL protocol documentation, the SQLite file format, MS-TDS), public documentation and observation.
- **Fonts are never committed.** The app uses system fonts and egui's defaults.
- **Test corpora** (real database files) live outside this repo, fetched pinned and sha256-verified. Never commit large binary fixtures; build test databases in code.
- **Everything is a command.** User-visible behaviour = a command in `crates/engine/src/cmd/*` (id, label, menu path, shortcut, params doc, `enabled`, `run`) + tests. Ids follow Beekeeper Studio's menus plus primitives (`connection.open`, `query.run`, `table.open`). UI-only commands live in `crates/ui-egui`. The control channel, CLI and MCP reach all of them.
- **Programmatic calls never open dialogs**: a command with empty params runs with defaults or returns an error; only menu-style invocation opens a dialog (`UiRequest::Dialog`).
- **Layering** is enforced by `cargo xtask layers`: domain crates (L0–L4) → `engine` (L5) → `ui-egui`/`mcp` (L6) → `apps/*` (L7). Nothing below L6 depends on egui/eframe/winit/rfd/wgpu. **The UI crate is swappable.** Register every new crate in `xtask/src/layers.rs` and `docs/architecture.md`.
- **The UI is thin**: panels read engine state and act through `app.run(id, params)`.
- **Rust only** (no handwritten JS/TS).
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests, assets, layers, provenance). One task id per commit (`M1.2: connection list`).

## Running and looking at the app
- `cargo run -p dbee` opens the app (a placeholder window until the first features land). `DBEE_NO_PREFS=1` keeps no log file.
- Planned, documented before it exists: `--control <port>` (JSON lines on `127.0.0.1`, `docs/control-protocol.md`), `dbee-cli mcp` (`docs/mcp.md`), the CLI commands in `docs/cli.md`.
- **For UI work, look at the result** (a screenshot of the window) and compare with `plan/beekeeper/01-observed-ui.md`.
- Parallel agents: separate `CARGO_TARGET_DIR` per agent; edit only the crates you own; delete your target dir when done.

## Roadmap
`ROADMAP.md` (committed) is the one-page summary: stage, headline numbers, dimensions, languages, progress log. It follows [`docs/standards/progress-docs.md`](docs/standards/progress-docs.md) and points at the detail in `docs/`: `target-app-parity.md` (authoritative assessment), `gaps.md` (ranked work list: pick from it), `roadmap.md` (current focus, beta gates, milestones), `architecture.md`, and the area checklists `driver-parity.md`, `file-format-parity.md`, `ui-parity.md`, `hardware-parity.md`, `localization-parity.md`. Update the affected docs (and their timestamp lines and revision history) whenever work lands.

## Contributor credits (About window)

- About ▸ Contributors/Models will be compiled into the binary from `contributors/contributors.json`
  (commit stats; generated, never hand-edit) and `contributors/people.toml` (names people chose for
  themselves). See `docs/contributors.md`.
- **Agents working for a contributor:** before you commit for them, check whether they have a
  `[people.<username>]` entry in `contributors/people.toml`. If not, ask them once whether they want
  to be credited by more than their git name: a real name, a display name, their commit addresses.
  If yes, add **only their own** entry (copy the template at the top of the file), committed as them.
  If no, change nothing.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or
  GitHub profiles). Never hand-edit `contributors.json`.
- Refresh the stats with `cargo xtask contributors`.

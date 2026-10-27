# Agents

> **Last reviewed:** 2026-10-26 · **Last updated:** 2026-10-26 · **Change:** minor (§4: the M1 day as a scripted check, with its command)

dbee is built to be driven by AI agents, and largely built by them. Part 1 covers using dbee from an agent. Part 2 covers developing dbee as an agent.

# Part 1: driving dbee

The control channel (§2) and the isolated runs for checking UI work (§4) exist. The MCP server (§1) and the command line (§3) are **(planned)**, documented first so the implementation has a contract to meet; today `dbee-cli` prints its version.

## 1. MCP server (planned)

`dbee-cli mcp` serves MCP on stdio, headless (`--demo`, a seeded in-memory SQLite database) or bridged to a running app (`--bridge HOST:PORT`). The core tools `command_list`, `command_run`, `command_batch`, `doc_inspect` and `render_preview` work in both modes. Database tools: `connection_list`, `schema_inspect`, `query_run`, `table_read`. Bridge-only: the `ui_*` tools and `screenshot`. Full reference: [`mcp.md`](mcp.md).

Typical loop: `connection_list` / `schema_inspect` to get names, `command_run` or `query_run`, `table_read` or a screenshot to look at the result, `edit.undo` if it is wrong.

## 2. Control channel

`dbee --control 7979` (or `DBEE_CONTROL_PORT=7979`) listens on `127.0.0.1` only: one JSON request per line, one JSON reply per line.

```jsonc
{"id":1,"method":"engine.commands"}
{"id":2,"method":"engine.execute","params":{"command":"connection.open","params":{"id":"<saved connection id>"}}}
{"id":3,"method":"ui.screenshot","params":{"path":"target/look/shot.png"}}
```

Replies look like `{"id":2,"ok":true,"result":{…}}` or `{"id":2,"ok":false,"error":"…","code":"…","data":{…}}`. A command that starts a job is answered when the job ends. Method table: [`control-protocol.md`](control-protocol.md).

## 3. Command line (planned)

Every command is one shell call away: `dbee-cli exec <id> k=v…`, `run <script.jsonl>`, `query --connection <name> <sql>`. Output is JSON; exit status is 0 on success, 1 when a command fails and 2 on a usage error. Reference: [`cli.md`](cli.md).

## 4. Verifying UI work

For any UI change, look at the result:

1. `cargo xtask run-isolated --port 7979` (add `--release` for a release build). It builds dbee and runs it on a throwaway home, `target/isolated/<port>/home`, with the control channel on that port, in-memory secrets, an in-memory clipboard (`DBEE_CLIPBOARD=memory`: every copy and cut, a text field's too, stays in the app, and `document.inspect` shows the start of the last as `clipboard` with its length as `clipboardBytes`) and no SSH agent on Linux and macOS (on Windows the OpenSSH agent pipe is still reachable); with the Docker test servers up, that home's `.ssh` holds their test key and host key. The run may connect only to the test servers: each server's port and the sshd's, on `127.0.0.1` and `localhost`, plus `127.0.0.1:1` for error demos (`DBEE_ALLOW_TARGETS` on the child). Without `target/testdb/env.json` only `127.0.0.1:1` is allowed, and run-isolated warns. Test, open and reconnect refuse any other target before any network I/O, with an error saying this isolated run may only connect to the test servers; through an SSH tunnel the SSH server's host and port are what count.
2. Drive the feature through the control channel (the MCP bridge is planned): commands first (`engine.execute`), then clicks and keys by automation id (`ui.click`, `ui.key`).
3. Assert on `ui.inspect` and `document.inspect`.
4. Take `ui.screenshot` (the whole window) and open the PNG; `app.quit` ends the run.

The whole M1 day is scripted the same way, in the real window with the isolated run's environment: `cargo test --workspace --test m1_day -- --ignored --nocapture` drives the M1 day (an SSH connection, Ctrl+P, filters, a JSON value, a foreign key, edits, a query copied with its header) and saves a screenshot of each step in `target/m1-day/` ([`testing.md`](testing.md) §4).

Screenshots you commit follow the asset rule in [`AGENTS.md`](../AGENTS.md): synthetic data only, and a row in `ATTRIBUTION.md`. Screenshots of Beekeeper Studio are never committed.

# Part 2: developing dbee

## 5. Orientation, in this order

1. [`AGENTS.md`](../AGENTS.md): absolute rules (assets, clean-room, never crash).
2. [`docs/standards/`](standards/): [never crash](standards/never-crash.md), [progress docs](standards/progress-docs.md), [licensing](standards/licensing.md).
3. [`ROADMAP.md`](../ROADMAP.md), then [`gaps.md`](gaps.md) (the ranked work list).
4. [`architecture.md`](architecture.md).
5. [`testing.md`](testing.md).

Maintainers also keep a local planning folder, `plan/` (gitignored): the status file, the task plan and behaviour notes. If you don't have it, the public docs above are enough to contribute; ask a maintainer for a task id.

## 6. Autonomous work loop

1. **Orient.** Pick the next task: the next one in the maintainer status file, or the top open entry in [`gaps.md`](gaps.md).
2. **Plan tests first.** Write down the acceptance test before the code.
3. **Implement and test.**
4. **Verify.** Run `cargo xtask ci`. For UI work, also take a screenshot of the window and look at it.
5. **Record.** Update the progress docs the change touches (ROADMAP.md, `docs/gaps.md`, the parity docs, with their status lines and revision history). Commit as `M<n>.<k>: …`, one task id per commit, then move on.

For parallel agents see §9.

## 7. Decision rules

Don't block on choices you can make yourself. Where Beekeeper Studio's behaviour is undocumented, choose what database clients most commonly do and record the choice in the crate README. Never inspect Beekeeper Studio's internals (clean-room rule in `AGENTS.md`).

Ask a human only about:

- licences outside the allowlist ([licensing](standards/licensing.md));
- spending money;
- publishing or pushing;
- personal data;
- destructive git operations;
- anything AGENTS.md leaves unclear ("when in doubt, leave it out").

## 8. Definition of Done

A feature is done when:

- [ ] its command is registered with label, menu path, shortcut (if any) and a params doc;
- [ ] engine tests cover it, including undo/redo and the disabled case;
- [ ] the UI is wired: menu, panel control and shortcut;
- [ ] you have driven it through the control channel and reviewed a screenshot;
- [ ] all gates pass (`cargo xtask ci`) and new assets have rows in `ATTRIBUTION.md`;
- [ ] the progress docs are updated where relevant;
- [ ] it is committed with its task id.

### Reporting progress honestly

ROADMAP.md and [`target-app-parity.md`](target-app-parity.md) keep two numbers: **feature breadth** (does it exist?) and **ready for real work** (does it hold up?). The rules are in [progress docs](standards/progress-docs.md). In short:

- say how a number was obtained: *measured* or *estimated*;
- count an approximation, a stub or a partly wired setting as such, never as done;
- record known bugs and limitations in [`gaps.md`](gaps.md), not only what works;
- benchmarks state the machine, the load average and before/after on the same build;
- don't raise a percentage without evidence a reviewer can rerun.

## 9. Parallel agents

- One git worktree and one `CARGO_TARGET_DIR` per agent; delete your target dir when done.
- Each crate has one owner at a time. Edit only the crates you own.
- A new crate is registered in `xtask/src/layers.rs` and [`architecture.md`](architecture.md) in the same change.

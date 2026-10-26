# Testing

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-26 · **Change:** minor (§4 the clipboard tests and the in-memory clipboard; §6 the copy timing test and the freeze guard)

```sh
cargo test --workspace                              # everything; oracle tests skip without their tool
cargo test -p dbee-engine                           # one crate
cargo xtask testdb up                               # start the Docker test servers (§3); ci does it when Docker answers
cargo xtask ci                                      # all gates (fmt, clippy, testdb, test, assets, layers, provenance)
DBEE_REQUIRE_ORACLES=1 cargo test --workspace       # CI: a missing oracle tool fails instead of skipping
cargo test --workspace -- --ignored                 # performance tests
```

The scaffold has no features, so most layers below are **(planned)**. They are the contract for tests as features land.

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules | identifier quoting, type mapping, value formatting, command parameter parsing |
| Property (`proptest`) **(planned)** | domain crates, engine | quote then unquote is the identity, undo/redo restores state, export then import round-trips |
| Scripted UI | `crates/ui-egui/tests/` | headless app driven over the control channel: open a connection, run a query, click by automation id |
| Golden images **(planned)** | `crates/ui-egui/tests/` | small screenshots of fixed states, compared with a tolerance |
| Oracle **(planned)** | driver and engine tests | query results checked against the `sqlite3` CLI (§2) |
| Driver **(planned)** | `crates/pg`, `crates/ssh` tests | PostgreSQL 14 and the newest major, and SSH tunnels, against the Docker test servers (§3, which exist) |
| Robustness / fuzz | engine; wire parsers **(planned)** | `every_command_survives_empty_params`, the command-sequence fuzz (`DBEE_FUZZ_SEED`, `DBEE_FUZZ_STEPS`), hostile wire input **(planned)** (§5) |
| Performance (`#[ignore]`) **(planned)** | engine, ui-egui | scrolling a million-row result, streaming a large export (§6) |

Test databases are built in code (a SQL script or the driver API run at the start of the test). Never commit database files or large binary fixtures.

## 2. Oracle tests: SQLite through the `sqlite3` CLI

For SQLite the oracle is the `sqlite3` command-line shell, an independent implementation:

- build the same database with both dbee and `sqlite3`, run the same query, compare rows, column names and declared types;
- compare schema introspection with `sqlite3`'s `.schema` and `PRAGMA table_info`;
- compare exports (CSV, JSON, SQL) by loading dbee's output back into `sqlite3`.

Oracle tests skip with a printed `SKIPPED` when `sqlite3` is not on `PATH`, unless `DBEE_REQUIRE_ORACLES=1` is set (CI sets it). State the pass criterion in the test: results are exact, and any tolerance (floating point text) is named.

## 3. Driver tests: the Docker test servers

Postgres and SSH tests run against local containers from `cargo xtask testdb` (compose project `dbee-testdb`, files in `xtask/testdb/`):

```sh
cargo xtask testdb up       # start, or reuse what runs; the seed runs once (well under a minute)
cargo xtask testdb status   # ports from the env file, and docker compose ps
cargo xtask testdb down     # remove the containers, their volumes and target/testdb
```

- **Servers.** Each listens on a random port on `127.0.0.1`.
  - `pg14` runs `postgres:14-alpine`, the oldest supported major.
  - `pgnew` runs `postgres:<PG_NEWEST_MAJOR>-alpine`, the newest released major (`xtask/src/testdb.rs`; 18 on 2026-10-19).
  - Both have database `dbee`, superuser `postgres` with the generated password, and TLS on. Their certificate is for `localhost`, `127.0.0.1`, `pg14` and `pgnew`, signed by a generated CA.
  - `sshd`: user `dbee` logs in with the test keys (ed25519, ed25519 with a passphrase, RSA 3072), user `dbeepw` only with the generated password. Forwarding reaches `pg14:5432` and `pgnew:5432` and nothing else.
- **Env file.** `up` writes `<repo>/target/testdb/env.json`, always under the repository's `target/`, whatever `CARGO_TARGET_DIR` says.
  - It holds the password, every port, the SSH users and `known_hosts` line, the key files (copied to `target/testdb/keys/`, mode 0600) with the passphrase, and the CA file.
  - `dbee_testkit::testdb()` reads it. `target/testdb/compose.env` holds the variables every `docker compose` call passes with `--env-file`.
- **Seed.** `xtask/testdb/seed.sql`, the same on every server, synthetic data only.
  - Schema `shop`: an enum, a domain and a composite type; customers (500 rows), orders (5,000) and their items and shipments with single and composite foreign keys; `all_types` with exotic types; a table without a key; a view and a materialized view; a partitioned table; `big_rows` (1,000,000 rows); and `big_cell`, one row with a 20,000,000-character text and a jsonb of about that size.
  - Schema `many`: 300 tables. Schema `odd`: names that need quoting (quotes, spaces, upper case, `Ö`, emoji).
  - Schema `scratch` is empty. The seed is read-only: a test that writes creates its own table in `scratch`, named with `dbee_testkit::unique(..)`, and drops it at the end.
- **Skip rule.** A test that needs the servers starts with `let db = dbee_testkit::require_testdb!();`.
  - When the env file is missing or a server does not answer, it prints `SKIPPED: …` and returns. With `DBEE_REQUIRE_ORACLES=1` it fails instead.
  - `cargo xtask ci` runs `testdb up` before `test` when `docker info` answers. Otherwise its summary says `skip  driver tests (no Docker)`, and with `DBEE_REQUIRE_ORACLES=1` ci fails.
- **Helpers.** `dbee_testkit::psql(server, sql)` runs `psql` inside a server's container, for set-up and for checking what dbee wrote. `dbee_testkit::temp_home(..)` gives SSH tests a throwaway home with a key and `known_hosts`.
- **Never a real server**, and never a connection string from the environment that points elsewhere. Tests never touch the real `~/.ssh`, SSH agent or keyring.

## 4. UI: control channel and screenshots

- **Scripted tests** (`crates/ui-egui/tests/control.rs`, `crates/ui-egui/tests/connections.rs`, `crates/ui-egui/tests/workspace.rs`, `crates/ui-egui/tests/table_tab.rs`, `crates/ui-egui/tests/editing.rs`, `crates/ui-egui/tests/value_panel.rs`, `crates/ui-egui/tests/query_tab.rs`, `crates/ui-egui/tests/clipboard.rs`) run `DbeeApp::ui` in an egui_kittest harness, frame by frame, and drive it only through control requests and automation ids, asserting on `ui.inspect` and `document.inspect`. Each frame first feeds the app's synthetic input (`DbeeApp::feed_input`), as eframe's raw-input hook does. Copied text is read from the frame's egui output (`OutputCommand::CopyText`), which kittest hands back and which never reaches the system clipboard.
- **Golden images (planned)** are procedural: a fixed in-code database, a fixed window size, one state per image. Update a golden image only after looking at the new picture.
- **Interactive checks** run the real app with `cargo xtask run-isolated`, drive it and take `ui.screenshot`. Such a run keeps every copy and cut in the app (`DBEE_CLIPBOARD=memory`), a text field's as well as the grid's, so it never touches the system clipboard; `document.inspect` shows the first 4 KiB of the last copy as `clipboard` and its length as `clipboardBytes`. It connects only to the Docker test servers (and `127.0.0.1:1`); `connection.test`, `connection.open` and `connection.reconnect` refuse any other target with a `config` error before any network I/O (`DBEE_ALLOW_TARGETS`, [`agents.md`](agents.md) §4). The launch time check (`cargo test -p xtask -- --ignored`) starts the release build that way and prints how long the connection screen takes. Do this for every UI change (see [`agents.md`](agents.md) §4).
- **A hidden window runs no frames on Wayland.** The control channel is served in the window's frames. On Wayland, winit holds the next redraw until the compositor answers the last frame's frame callback (egui-wgpu asks for one before every present), and KWin sends none to a window it does not show. So while the isolated window is minimised (reproduced on KDE Plasma 6, Mesa 25 RADV), and presumably while KWin paints nothing for it at all (another virtual desktop, displays asleep), every request times out with "the app did not answer in time (is the window hidden?)", and requests are answered again within one frame once the window is shown. The UI thread meanwhile idles in winit's event loop, not in wgpu's present.

## 5. Robustness and fuzz

Follow [never crash](standards/never-crash.md). Beyond a regression test per fixed crash:

- `every_command_survives_empty_params` (`crates/engine/src/tests.rs`) runs every registered command with empty params;
- the command-sequence fuzz (`crates/engine/tests/command_sequence_fuzz.rs`) runs random commands with random, often hostile, params and checks the session's invariants after every step: no `internal` error, `inspect()` serializes, the active tab is an open one, and no journal entry keeps a secret. A new command adds its param shapes to `PARAM_HINTS` there (a test fails without them). With the test servers up it also runs random edit commands on a table tab and checks the undo/redo invariant: undoing until `edit.undo` is disabled leaves no staged edit, and as many redos bring back the edits from before;

  ```sh
  cargo test -p dbee-engine --test command_sequence_fuzz                      # 300 steps, seed from the clock (printed)
  DBEE_FUZZ_SEED=42 DBEE_FUZZ_STEPS=5000 cargo test -p dbee-engine --test command_sequence_fuzz   # replay or a longer run
  ```

- wire parsers (PostgreSQL, MySQL, TDS) take truncated, oversized and random bytes and return errors, never panic **(planned)**.

## 6. Performance

Performance tests are `#[ignore]` so `cargo test` stays fast; run them with `-- --ignored`. Each states its budget (for example time to first row, rows per second, peak memory) and prints the machine, the load average and the measured value. A claim of improvement needs before/after numbers on the same build.

Today: `tree_filter_1000_tables_within_a_frame` (`crates/ui-egui/tests/workspace.rs`) draws the schema tree over a synthetic catalog of 1,000 tables and prints the median frame time with a new filter each frame and with the filter unchanged (budget 16 ms each); `grid_frame_within_budget` (`crates/ui-egui/tests/table_tab.rs`) draws the data grid over 10,000 rows of 20 columns with a 20 MB value in view, the selection moving each frame (budget 16 ms); `keystroke_frame_with_a_value_at_the_cap` (`crates/ui-egui/tests/editing.rs`) types into a cell editor holding one paragraph just under the inline cap (32 KiB), single-line and multiline, and prints the median frame time with the frame's tessellation (budget 33 ms); `value_panel_frames_within_budget` (`crates/ui-egui/tests/value_panel.rs`) shows values at the 10 MiB cap in the value panel (the seed's 20 MB text and jsonb, 20 MB of line breaks, 20 MB of two-byte characters, 20 MB of tabs and emoji, a pretty-printed 9 MiB jsonb) and prints the frame that first shows each (budget 250 ms), the frames until its JSON colours arrive, and the median frame unchanged, while scrolling, while the width changes, and while the panel's edge is dragged, with the frame that lets it go (budget 16 ms for the unchanged, scrolling and dragging ones); `keystroke_frames_in_big_scripts` (`crates/ui-egui/tests/query_tab.rs`) types into the query editor holding SQL of 32 KiB, 256 KiB and 1 MiB, as a script of short lines and as one line, and prints the median keystroke frame with its tessellation for the editor alone (beside egui's plain code editor on the same text) and for the whole window, in a plain egui context and in kittest, which always builds the AccessKit tree (budget 33 ms for the 32 KiB script in the editor alone; the others are reported); the launch time check is in §4.

`copy_frames_within_budget` (`crates/ui-egui/tests/clipboard.rs`) copies, in each format, a full 10,000-row result, 10,000 rows whose TSV is one byte under the 8 MiB copy limit, one value of control characters (six bytes each in JSON), one of line breaks (`<br>` in Markdown) and 10,000 rows of 1,664 NULLs, and prints the median `clipboard.copy` on its own and the median frame with it, copied or refused (budget 250 ms for both; frames are judged where a frame without a copy is within it, which the 1,664-column grid is not).

Two guards against a frozen window run with the normal tests: `twenty_megabyte_cells_keep_frames_under_250_ms` (`crates/ui-egui/tests/table_tab.rs`) and `no_copy_freezes_the_window` (`crates/ui-egui/tests/clipboard.rs`, a copy of a 10,000-row result or of a row whose escaping passes the limit takes less than a second, in each format).

## 7. Corpora

Real database files and large dumps live **outside the repository**. They are fetched from a pinned URL and verified against a pinned sha256 before use; a mismatch fails the test. Tests that need a corpus skip when it is absent unless `DBEE_REQUIRE_ORACLES=1`. Never commit them.

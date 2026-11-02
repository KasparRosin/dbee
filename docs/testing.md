# Testing

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** minor (§3: the MySQL and MariaDB test servers, the SQLite seed, `targets()`)

```sh
cargo xtask testdb up                                         # start the Docker test servers (§3); ci does it when Docker answers
cargo xtask ci                                                # all gates (fmt, clippy, testdb, test, assets, layers, provenance); after testdb up a skipped driver test fails
DBEE_REQUIRE_ORACLES=1 cargo xtask ci                         # the same, and without Docker ci fails instead of skipping the driver tests
cargo test --workspace                                        # every test; tests that need the servers print SKIPPED without them
cargo test --workspace --test table_tab                       # one test file (of any crate)
cargo test --workspace paging                                 # tests whose name contains `paging`
cargo test --workspace -- --ignored --nocapture --test-threads=1   # the timing tests and the M1 day (§4, §6)
cargo xtask licenses                                          # the licence check, by hand after adding a dependency
```

Keep `--workspace` on focused runs too. `-p <crate>` resolves features for that crate alone, so serde, libc, log and others get other features, and egui, wgpu and everything above them are built again as another copy. On a shared machine, `nice -n 10 cargo …` keeps the desktop responsive.

Layers still marked **(planned)** below are the contract for tests as features land.

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules | identifier quoting, type mapping, value formatting, command parameter parsing |
| Property (`proptest`) **(planned)** | domain crates, engine | quote then unquote is the identity, undo/redo restores state, export then import round-trips |
| Scripted UI | `crates/ui-egui/tests/`; `apps/dbee/tests/m1_day.rs` (the real window, by hand) | the app driven over the control channel: open a connection, run a query, click by automation id (§4) |
| Golden images **(planned)** | `crates/ui-egui/tests/` | small screenshots of fixed states, compared with a tolerance |
| Oracle **(planned)** | driver and engine tests | query results checked against the `sqlite3` CLI (§2) |
| Driver | `crates/pg`, `crates/ssh` tests | PostgreSQL 14 and the newest major, and SSH tunnels, against the Docker test servers (§3) |
| Robustness / fuzz | engine; wire parsers **(planned)** | `every_command_survives_empty_params`, the command-sequence fuzz (`DBEE_FUZZ_SEED`, `DBEE_FUZZ_STEPS`), hostile wire input **(planned)** (§5) |
| Performance (`#[ignore]`) | core, engine, ui-egui, xtask | frame times with 1,000 tables, 20 MB values, wide results and big scripts; a page of a million rows; the launch time (§6). Streaming a large export **(planned)** |

Test databases are built in code (a SQL script or the driver API run at the start of the test). Never commit database files or large binary fixtures.

## 2. Oracle tests: SQLite through the `sqlite3` CLI

For SQLite the oracle is the `sqlite3` command-line shell, an independent implementation:

- build the same database with both dbee and `sqlite3`, run the same query, compare rows, column names and declared types;
- compare schema introspection with `sqlite3`'s `.schema` and `PRAGMA table_info`;
- compare exports (CSV, JSON, SQL) by loading dbee's output back into `sqlite3`.

Oracle tests skip with a printed `SKIPPED` when `sqlite3` is not on `PATH`, unless `DBEE_REQUIRE_ORACLES=1` is set (CI sets it). State the pass criterion in the test: results are exact, and any tolerance (floating point text) is named.

## 3. Driver tests: the Docker test servers

Postgres, MySQL, MariaDB and SSH tests run against local containers from `cargo xtask testdb` (compose project `dbee-testdb`, files in `xtask/testdb/`):

```sh
cargo xtask testdb up       # start, or reuse what runs; the seeds run once (80 s from clean, measured 2026-10-29)
cargo xtask testdb status   # ports from the env file, and docker compose ps
cargo xtask testdb down     # remove the containers, their volumes and target/testdb
```

- **Servers.** Each listens on a random port on `127.0.0.1`. `SERVICES` in `xtask/src/testdb.rs` is the one list of the database servers: name, type, image, release and whether it holds the full seed. The compose file takes each image from it (`DBEE_IMAGE_<SERVICE>` in `compose.env`) and the certificate its names; adding a server of a known type is a row there and its block in `compose.yml`.
  - `pg14` runs `postgres:14-alpine`, the oldest supported major.
  - `pgnew` runs the newest released major, `postgres:18-alpine` on 2026-10-19.
  - Both have database `dbee`, superuser `postgres` with the generated password, and TLS on.
  - `mysql57` (`mysql:5.7`, built for amd64 only), `mysql80` (`mysql:8.0`) and `mysqlnew` (the newest, `mysql:26.10` on 2026-10-28) run MySQL; `mariadb1011` (`mariadb:10.11`) and `mariadbnew` (the newest, `mariadb:13.0` on 2026-10-28) run MariaDB.
  - They have user `root` with the generated password, no default database, TLS on and `max_allowed_packet` 64 MiB. They run light (`performance_schema` off, a 64 MB buffer pool, no binary log), as five of them run next to the Postgres servers: about 820 MiB together, measured. `up` checks each server's version and seed against its row, and on MySQL and MariaDB a TLS connection with the CA checked.
  - Their certificates are for `localhost`, `127.0.0.1` and every database service's name, signed by a generated CA: ECDSA for Postgres, RSA for MySQL and MariaDB (MySQL 5.7's image offers no cipher for an ECDSA key).
  - `mysql57`'s image (OpenSSL 1.0.2) has no ECDHE and no TLS 1.3, only DHE-RSA suites, so a client on rustls (dbee's) cannot agree on a cipher with it.
  - `sshd`: user `dbee` logs in with the test keys (ed25519, ed25519 with a passphrase, RSA 3072), user `dbeepw` only with the generated password. Forwarding reaches the database servers (`pg14:5432`, `pgnew:5432`, `mysql57:3306`, …, `mariadbnew:3306`) and nothing else.
- **Env file.** `up` writes `<repo>/target/testdb/env.json`, always under the repository's `target/`, whatever `CARGO_TARGET_DIR` says.
  - It holds the password; one `servers` list, each with its type, version, port, port inside the network (where the sshd reaches it as `name:port`) and whether it holds the full seed; the SSH users and `known_hosts` line; the key files (copied to `target/testdb/keys/`, mode 0600) with the passphrase; and the CA file.
  - `dbee_testkit::testdb()` reads it. `target/testdb/compose.env` holds the variables every `docker compose` call passes with `--env-file`.
- **Seed.** `xtask/testdb/seed.sql`, the same on every Postgres server, synthetic data only.
  - Schema `shop`: an enum, a domain and a composite type; customers (500 rows), orders (5,000) and their items and shipments with single and composite foreign keys; `all_types` with exotic types; a table without a key; a view and a materialized view; a partitioned table; `big_rows` (1,000,000 rows); and `big_cell`, one row with a 20,000,000-character text and a jsonb of about that size.
  - Schema `many`: 300 tables. Schema `odd`: names that need quoting (quotes, spaces, upper case, `Ö`, emoji).
  - Schema `scratch` is empty. The seed is read-only: a test that writes creates its own table in `scratch`, named with `dbee_testkit::unique(..)`, and drops it at the end.
- **MySQL seed.** `xtask/testdb/seed-mysql.sql`, the same file on every MySQL and MariaDB server: the shapes of `seed.sql` in databases `shop`, `many`, `odd` and `scratch`, with the same table names, keys, foreign keys and row counts.
  - Where MySQL lacks a Postgres shape it has the nearest one: an `ENUM` column, a plain view for the materialized view, a partitioned `shop.events` without partition tables. `odd`'s names have quotes, spaces, upper case, `é` and a backtick, but no emoji (MySQL names are BMP-only).
  - MySQL only: `shop.audit_log` (MyISAM, so dbee's grid keeps it read-only) and `shop.legacy_dates` with zero dates.
  - `mysql80` and `mariadbnew` hold `big_rows` with 1,000,000 rows and `big_cell`'s 20,000,000-character row; the other three hold 30,000 rows and no big cell, to keep `testdb up` light on the CPU (`dbee_testkit::Server::full_seed`, `TestTarget::big_rows`).
- **SQLite seed.** `xtask/testdb/seed-sqlite.sql`: the same shapes in schema `main`, with emoji in `odd`'s names, `notes_nokey` (no key) and `rowid_taken` (a column named `rowid`). No server: `dbee_testkit::sqlite_seeded(dir)` builds it once into `target/testkit/seed-<hash>.sqlite` and hands each call a fresh copy in `dir`, so a test may write to its copy. A copy is about 65 MB: keep `dir` under `target/`, as the system temp directory may be small.
- **Skip rule.** A test that needs the servers starts with `let db = dbee_testkit::require_testdb!();`.
  - When the env file is missing or a server does not answer, it prints `SKIPPED: …` and returns. With `DBEE_REQUIRE_ORACLES=1` it fails instead.
  - `cargo xtask ci` runs `testdb up` before `test` when `docker info` answers, and then runs the tests with `DBEE_REQUIRE_ORACLES=1` (it prints `$ DBEE_REQUIRE_ORACLES=1 cargo test --workspace`). So once the servers came up, a driver test that skips fails: a server that stopped answering, or an env file the tests read differently from how `testdb up` wrote it (the writer, `xtask/src/testdb.rs`, and the reader, `crates/testkit/src/lib.rs`, are kept apart on purpose).
  - Without Docker its summary says `skip  driver tests (no Docker)`, and with `DBEE_REQUIRE_ORACLES=1` ci fails.
- **Helpers.** `dbee_testkit::psql(server, sql)` and `dbee_testkit::mysql(server, sql)` run `psql` and the `mysql` (or `mariadb`) client inside a server's container, for set-up and for checking what dbee wrote. `dbee_testkit::targets()` lists every test server and one SQLite copy, for tests that run on every engine; each `TestTarget` is a kind and a server or a file, and what differs per kind in test SQL is one table, `dbee_testkit::Kind::info`. `dbee_testkit::temp_home(..)` gives SSH tests a throwaway home with a key and `known_hosts`.
- **Never a real server**, and never a connection string from the environment that points elsewhere. Tests never touch the real `~/.ssh`, SSH agent or keyring.

## 4. UI: control channel and screenshots

- **Scripted tests** (`crates/ui-egui/tests/control.rs`, `crates/ui-egui/tests/connections.rs`, `crates/ui-egui/tests/workspace.rs`, `crates/ui-egui/tests/table_tab.rs`, `crates/ui-egui/tests/editing.rs`, `crates/ui-egui/tests/value_panel.rs`, `crates/ui-egui/tests/query_tab.rs`, `crates/ui-egui/tests/clipboard.rs`) run `DbeeApp::ui` in an egui_kittest harness, frame by frame, and drive it only through control requests and automation ids, asserting on `ui.inspect` and `document.inspect`. They share one harness, `crates/ui-egui/tests/common/mod.rs` (`mod common;`, as the engine's tests share `crates/engine/tests/common/mod.rs`): a `Ui` window on a throwaway home (`Ui::fresh`, `Ui::connected`, `Ui::connected_in`, `Ui::connected_with` and a `Setup` for the size, frame step, home, secrets, connection and app), the request, click, key, text and inspect helpers, and a lock every window holds (exclusive for a test that kills dbee's backends). Each file keeps only its own helpers. Each frame first feeds the app's synthetic input (`DbeeApp::feed_input`), as eframe's raw-input hook does. Copied text is read from the frame's egui output (`OutputCommand::CopyText`), which kittest hands back and which never reaches the system clipboard.
- **The M1 day in the real window** (`apps/dbee/tests/m1_day.rs`, `#[ignore]`: it opens a window on the desktop, so `cargo xtask ci` never runs it). It starts the built `dbee` with the environment `cargo xtask run-isolated` gives (a throwaway home holding the test key and the sshd's host key, in-memory secrets and clipboard, the test servers as the only targets) and drives the M1 day through the control channel. A saved connection "Shop live" through the test sshd, Ctrl+P, three filters, a jsonb value, a foreign key, two edits written with Ctrl+S, a join copied with its header. It checks `document.inspect` after each step and psql after the edits, and saves `target/m1-day/<n>-<step>.png` to look at. The seed stays read-only: the day edits a customer and an order the test inserts, and deletes them when it ends, also when it fails. A run that is killed (Ctrl+C, a tool's timeout) cannot delete them, so each run first removes the rows earlier runs left, found by the customers' note `the M1 day test row`: their orders, then the customers. After the day it checks that the counts of customers and orders are as before. Run it by hand, with the window shown (a hidden window runs no frames on Wayland; see the last point below):

  ```sh
  cargo xtask testdb up
  cargo test --workspace --test m1_day -- --ignored --nocapture
  ```

  Without the test servers it prints `SKIPPED` (with `DBEE_REQUIRE_ORACLES=1` it fails), and without `DISPLAY` or `WAYLAND_DISPLAY` it prints `SKIPPED`.
- **Golden images (planned)** are procedural: a fixed in-code database, a fixed window size, one state per image. Update a golden image only after looking at the new picture.
- **Interactive checks** run the real app with `cargo xtask run-isolated`, drive it and take `ui.screenshot`. Such a run keeps every copy and cut in the app (`DBEE_CLIPBOARD=memory`), a text field's as well as the grid's, so it never touches the system clipboard; `document.inspect` shows the first 4 KiB of the last copy as `clipboard` and its length as `clipboardBytes`. It connects only to the Docker test servers (and `127.0.0.1:1`); `connection.test`, `connection.open` and `connection.reconnect` refuse any other target with a `config` error before any network I/O (`DBEE_ALLOW_TARGETS`, [`agents.md`](agents.md) §4). The launch time check (`cargo test --workspace connection_screen -- --ignored --nocapture`) starts the release build that way and prints how long the connection screen takes. Do this for every UI change (see [`agents.md`](agents.md) §4).
- **A hidden window runs no frames on Wayland.** The control channel is served in the window's frames. On Wayland, winit holds the next redraw until the compositor answers the last frame's frame callback (egui-wgpu asks for one before every present), and KWin sends none to a window it does not show. So while the isolated window is minimised (reproduced on KDE Plasma 6, Mesa 25 RADV), and presumably while KWin paints nothing for it at all (another virtual desktop, displays asleep), every request times out with "the app did not answer in time (is the window hidden?)", and requests are answered again within one frame once the window is shown. The UI thread meanwhile idles in winit's event loop, not in wgpu's present.

## 5. Robustness and fuzz

Follow [never crash](standards/never-crash.md). Beyond a regression test per fixed crash:

- `every_command_survives_empty_params` (`crates/engine/src/tests.rs`) runs every registered command with empty params;
- the command-sequence fuzz (`crates/engine/tests/command_sequence_fuzz.rs`) runs random commands with random, often hostile, params and checks the session's invariants after every step: no `internal` error, `inspect()` serializes, the active tab is an open one, and no journal entry keeps a secret. A new command adds its param shapes to `PARAM_HINTS` there (a test fails without them). With the test servers up it also runs random edit commands on a table tab and checks the undo/redo invariant: undoing until `edit.undo` is disabled leaves no staged edit, and as many redos bring back the edits from before;

  ```sh
  cargo test --workspace --test command_sequence_fuzz                         # 300 steps, seed from the clock (printed)
  DBEE_FUZZ_SEED=42 DBEE_FUZZ_STEPS=20000 cargo test --workspace --test command_sequence_fuzz   # replay or a longer run
  ```

- wire parsers (PostgreSQL, MySQL, TDS) take truncated, oversized and random bytes and return errors, never panic **(planned)**.

## 6. Performance

Performance tests are `#[ignore]` so `cargo test` stays fast; run them with `-- --ignored --nocapture --test-threads=1` (one at a time, so two timing tests of one file do not share the CPU). Each states its budget (for example time to first row, rows per second, peak memory) and prints the machine, the load average and the measured value. A claim of improvement needs before/after numbers on the same build. The numbers measured at the end of M1 are in [`target-app-parity.md`](target-app-parity.md), "Performance".

Today: `tree_filter_1000_tables_within_a_frame` (`crates/ui-egui/tests/workspace.rs`) draws the schema tree over a synthetic catalog of 1,000 tables and prints the median frame time with a new filter each frame and with the filter unchanged (budget 16 ms each); `grid_frame_within_budget` (`crates/ui-egui/tests/table_tab.rs`) draws the data grid over 10,000 rows of 20 columns with a 20 MB value in view, the selection moving each frame (budget 16 ms); `keystroke_frame_with_a_value_at_the_cap` (`crates/ui-egui/tests/editing.rs`) types into a cell editor holding one paragraph just under the inline cap (32 KiB), single-line and multiline, and prints the median frame time with the frame's tessellation (budget 33 ms); `value_panel_frames_within_budget` (`crates/ui-egui/tests/value_panel.rs`) shows values at the 10 MiB cap in the value panel (the seed's 20 MB text and jsonb, 20 MB of line breaks, 20 MB of two-byte characters, 20 MB of tabs and emoji, a pretty-printed 9 MiB jsonb) and prints the frame that first shows each (budget 250 ms), the frames until its JSON colours arrive, and the median frame unchanged, while scrolling, while the width changes, and while the panel's edge is dragged, with the frame that lets it go (budget 16 ms for the unchanged, scrolling and dragging ones); `keystroke_frames_in_big_scripts` (`crates/ui-egui/tests/query_tab.rs`) types into the query editor holding SQL of 32 KiB, 256 KiB and 1 MiB, as a script of short lines and as one line, and prints the median keystroke frame with its tessellation for the editor alone (beside egui's plain code editor on the same text) and for the whole window, in a plain egui context and in kittest, which always builds the AccessKit tree (budget 33 ms for the 32 KiB script in the editor alone; the others are reported); the launch time check is in §4.

`copy_frames_within_budget` (`crates/ui-egui/tests/clipboard.rs`) copies, in each format, a full 10,000-row result, 10,000 rows whose TSV is one byte under the 8 MiB copy limit, one value of control characters (six bytes each in JSON), one of line breaks (`<br>` in Markdown), one of double quotes at half the limit (doubled in CSV, escaped in JSON) and 10,000 rows of 1,664 NULLs, and prints the median `clipboard.copy` on its own and the median frame with it, copied or refused (budget 250 ms for both; frames are judged where a frame without a copy is within it, which the 1,664-column grid is not).

`wide_result_frames` (`crates/ui-egui/tests/query_tab.rs`) times a frame with a result of 50, 100, 300 and 1,664 columns over 10,000 rows (NULLs, then short texts) in view. It measures the whole window without an AccessKit tree, unchanged and scrolling, and the kittest frame. It reports and sets no budget: such a frame grows with the columns ([`gaps.md`](gaps.md)).

Elsewhere: `filter_1000_tables_within_a_frame` (`crates/core`), `quick_search_1000_tables_within_a_frame` (`crates/engine/tests/schema.rs`, budget 2 ms) and `next_page_on_a_million_rows_is_fast` (`crates/engine/tests/table.rs`).

Two guards against a frozen window run with the normal tests: `twenty_megabyte_cells_keep_frames_under_250_ms` (`crates/ui-egui/tests/table_tab.rs`) and `no_copy_freezes_the_window` (`crates/ui-egui/tests/clipboard.rs`, a copy of a 10,000-row result or of a row whose escaping passes the limit takes less than a second, in each format).

## 7. Corpora

Real database files and large dumps live **outside the repository**. They are fetched from a pinned URL and verified against a pinned sha256 before use; a mismatch fails the test. Tests that need a corpus skip when it is absent unless `DBEE_REQUIRE_ORACLES=1`. Never commit them.

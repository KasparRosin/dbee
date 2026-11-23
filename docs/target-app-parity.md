# Target app parity

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-23 · **Change:** minor (checked against the M3 tree: the Query and Data rows and "What works today" say what the cell editor, the value panel, the error band and the tracked transaction do; "Effort and calibration" keeps the dates and counts; grades unchanged) · **Target:** Beekeeper Studio 5.5.7

## Summary

dbee covers the daily workflow on PostgreSQL, MySQL, MariaDB and SQLite files: connect (a server also through an SSH tunnel), find a table, filter it, read its JSON, follow a foreign key, edit rows, run queries and copy results. It works end to end in the real window: the M1 day on Postgres (`apps/dbee/tests/m1_day.rs`) and the M2 day on MySQL through SSH, MariaDB and a SQLite file (`apps/dbee/tests/m2_day.rs`), measured. There is no file export or import yet (M4).

Overall parity with Beekeeper Studio: **~30 % (estimated)**, from the weights and dimension scores below. Breadth comes from grading Beekeeper's observed feature list row by row. Readiness stays low until dbee has been used on real databases (a day of real use on the maintainer's own databases, the last check of M1 and of M2).

## Readiness by audience

| Audience | Ready % | Agent wall-clock hours to ~95% | Work that dominates |
|---|---|---|---|
| Developer querying a local SQLite file | ~30 % (estimated): the daily workflow on a file (`dbee <file>`, a dropped file, Open with), not yet used on real files | not estimated yet | file export and import (M4), a table editor, saved queries and history |
| Backend developer on Postgres/MySQL | ~35 % (estimated): the daily workflow on Postgres, MySQL and MariaDB, not yet used on real databases | not estimated yet | file export and import (M4), a table editor, saved queries and history |
| Data analyst exporting results | ~10 % (estimated): copies of up to 8 MiB only | not estimated yet | file export (M4), results past 10,000 rows |

## What we measured against and how

Beekeeper Studio 5.5.7, observed black-box in a sandbox, using synthetic databases only. The evidence is in observation notes kept outside the repository (they hold Beekeeper Studio screenshots, which are not ours to publish). Feature breadth grades each row of the feature catalog in those notes: **1** done, **0.5** partial, **0** missing. A row is done only when it behaves as observed, on every engine dbee has. Engine rows are graded in [driver-parity.md](driver-parity.md), not here. dbee's side was measured with the test suites against the Docker test servers (Postgres 14 and 18; MySQL 5.7, 8.0 and the newest; MariaDB 10.11 and the newest; an sshd), SQLite files the tests make, and the real window (`m1_day`, `m2_day`).

## Weights

| Dimension | Weight |
|---|---|
| Features | 40 |
| Database engines | 20 |
| UI | 12 |
| Performance | 8 |
| Stability | 8 |
| File formats (import/export) | 4 |
| Localization | 4 |
| Platforms | 2 |
| Ecosystem and AI | 2 |

## Feature areas

| Area | Weight | Breadth | Ready | Hours | Evidence |
|---|---|---|---|---|---|
| Connections | 15 | 62 % (7.5 of 12 rows; estimated) | ~45 % (estimated) | not estimated yet | Done: read-only mode, test, save with name, colour and passwords, saved and recent lists with a filter, Fill from URL. Partial: no socket mode or IAM; SSL without client certificates; SSH without a jump host; no Copy URL. Missing: a sample database, team workspaces. Also on every engine: the database type's cards, the server's databases listed under the Database field, a database file opened from the command line or a drop. `crates/engine/tests/connection.rs`, `crates/ui-egui/tests/connections.rs`, `m1_day` step 1, `m2_day`. |
| Navigation | 10 | 50 % (3 of 6; estimated) | ~40 % (estimated) | not estimated yet | Done: the schema tree, quick search. Partial: the tree's filter and refresh only; tabs without reopen or restore. Missing: database switcher, several windows. Ctrl+P finds saved connections too, and switches to one. `crates/ui-egui/tests/workspace.rs`, `m1_day` step 2, `m2_day`. |
| Query | 20 | 32 % (3.5 of 11; estimated) | ~25 % (estimated) | not estimated yet | Done: run all and run current, several result sets, errors. Partial: the editor has no autocomplete. Missing: manual transactions (a transaction a script opens itself is shown as IN TRANSACTION and asked about when its tab or the connection closes, and a failed one offers ROLLBACK; there is no manual mode and no Begin or Commit control), saved queries, history, Vim keymap and wrap, find, run to file, AI shell. On MySQL and MariaDB a query tab's database chip. `crates/ui-egui/tests/query_tab.rs`, `m1_day` step 7, `m2_day`. |
| Results | 10 | 33 % (1 of 3; estimated) | ~30 % (estimated) | M4 | Done: copy as TSV (with or without header), CSV, JSON, Markdown. Missing: downloads. `crates/ui-egui/tests/clipboard.rs`. |
| Data | 25 | 59 % (6.5 of 11; estimated) | ~45 % (estimated) | not estimated yet | Done: the filter builder (any number of filters, raw `WHERE`), paging with counts, following a foreign key. Partial: the grid has no tooltips; editing has no copy as SQL, and rows have no clone; the value panel (pretty JSON, copy, a selected row as its JSON object, the draft of the cell being edited) stands in for two rows, the edit modal and the JSON row viewer, but a value past 32 KiB is read-only in it, and a row is shown there, not edited; copy has no SQL or IN list formats; columns resize only. Missing: quick filter. `crates/ui-egui/tests/table_tab.rs`, `editing.rs`, `value_panel.rs`, `m1_day` steps 3–6, `m2_day`. |
| Structure | 8 | 40 % (2 of 5; estimated) | ~30 % (estimated) | not estimated yet | Partial: a table tab's STRUCTURE shows its columns (with defaults and keys), indexes, foreign keys and the keys that reference it, and its triggers, read-only, on all four engines; a foreign key opens its table. Missing: editing the structure, a new table. `crates/ui-egui/tests/structure.rs`, `m1_day` step 2d. |
| Import/export | 7 | 0 % (0 of 3) | 0 % | M4 | Not started ([gaps.md](gaps.md) 1). |
| Tools | 3 | 0 % (0 of 2) | 0 % | not estimated yet | Backup, restore and plugins not started. |
| App | 2 | 12 % (0.5 of 4; estimated) | ~10 % (estimated) | not estimated yet | Partial: dbee's own light and dark themes, or the system's choice (View > Theme). Missing: zoom, help menus. `crates/ui-egui/tests/theme.rs`. |

Features, weighted by area: **42 %**, put at **~38 % (estimated)**.

## Dimensions

| Dimension | Weight | Score | Kind | Basis |
|---|---|---|---|---|
| Features | 40 | ~38 % | estimated | the feature areas above |
| Database engines | 20 | 20 % | measured (4 of 20 engines) | [driver-parity.md](driver-parity.md) |
| UI | 12 | ~45 % | estimated | [ui-parity.md](ui-parity.md): 24 elements graded, 3 done and 19 partial |
| Performance | 8 | ~30 % | estimated | dbee meets its own M1 targets (measured, below), but a frame passes 16.7 ms at about 100 columns in most runs. Beekeeper's timings were not measured, so there is no side-by-side. |
| Stability | 8 | ~25 % | estimated | Never-crash rules and the panic guard; the command-sequence fuzz at 20,000 steps (again on 2026-11-06, seed 1793954111563178215) and every command with empty params pass (measured). No use on real databases yet. |
| File formats (import/export) | 4 | 0 % | measured | none ([file-format-parity.md](file-format-parity.md)) |
| Localization | 4 | 0 % | measured | English only ([localization-parity.md](localization-parity.md)) |
| Platforms | 2 | ~25 % | estimated | Linux x86_64 built, packaged and run; macOS, Windows, FreeBSD and the other Linux builds built in the first test release (2026-10-29), not run except Windows ARM64 ([hardware-parity.md](hardware-parity.md)) |
| Ecosystem and AI | 2 | ~25 % | estimated | The localhost control channel drives every command and the window (`docs/control-protocol.md`); the CLI's database commands are planned; no AI features |

Weighted: 0.40 × 38 + 0.20 × 20 + 0.12 × 45 + 0.08 × 30 + 0.08 × 25 + 0.02 × 25 + 0.02 × 25 = **30.0 %**, given as **~30 % (estimated)**.

## Performance

All measured on 2026-10-26 on Linux x86_64, AMD Ryzen 7 PRO 5850U (16 threads), Debian 13, KDE Plasma (Wayland), in a dev build (the workspace's dev profile: dbee's crates at opt-level 1, dependencies at 2) unless noted. Two runs: the full `nice -n 10 cargo test --workspace -- --ignored --nocapture --test-threads=1` (17:06–17:15), then the timing tests of `clipboard`, `editing`, `query_tab`, `schema` and `table` again (17:18–17:22). The load average (1, 5 and 15 minutes) is the one each test printed. The machine is shared and was busy: the first run's early tests ran right after its own test build (load up to 53), and the launch check right after its release build. Runs differ by up to 2× even at a load of 4–6, so the table gives the range over the runs, and the numbers are upper bounds.

| What | Target | Measured | Load average | Test |
|---|---|---|---|---|
| Launch to the connection screen, release build | under 1 s | 301 ms | about 24.5 (its own release build just before) | `connection_screen_within_one_second` (xtask) |
| Entity filter over 1,000 tables, a new filter each frame | within a frame (16 ms) | 1.56 ms median (2.64 ms slowest); 0.98 ms unchanged | 5.67 / 16.70 / 16.03 | `tree_filter_1000_tables_within_a_frame` |
| Quick search (Ctrl+P) over 1,000 tables | 2 ms | 0.45–0.67 ms median (slowest 1.6–3.1 ms) | about 50, and 3.00 / 12.56 / 15.29 | `quick_search_1000_tables_within_a_frame` |
| Table filter alone over 1,000 tables | 2 ms | 0.28 ms median | about 50 | `filter_1000_tables_within_a_frame` |
| Next page of a million-row table (Postgres 14) | — | 1.06–1.09 ms median over 20 loads | about 50, and about 3 | `next_page_on_a_million_rows_is_fast` |
| A page drawn in the frame its rows arrive | same frame | yes | — | `a_page_draws_in_the_frame_its_rows_arrive` (runs in `cargo xtask ci`) |
| Grid frame, 10,000 × 20 rows with a 20 MB value in view | 16 ms | 6.44 ms median (8.06 ms slowest) | 6.72 / 17.46 / 16.26 | `grid_frame_within_budget` |
| Keystroke in a cell editor holding 32 KiB on one line | 33 ms | 18.8–24.9 ms single-line, 26.1–29.5 ms multi-line | 30.60 / 25.29 / 18.23, and 4.16 / 11.71 / 14.89 | `keystroke_frame_with_a_value_at_the_cap` |
| Keystroke in a 32 KiB script, the editor alone and the whole window | 33 ms | 1.9–2.6 ms and 2.0–5.5 ms | as above | `keystroke_frames_in_big_scripts` |
| Keystroke in a 1 MiB script of short lines, the whole window | — | 28.7–46.4 ms | as above | as above |
| Keystroke in 1 MiB on one line, the editor alone | — | 582–673 ms (slowest 2.0 s; 0.6–1.6 s in earlier runs; [gaps.md](gaps.md) 12) | as above | as above |
| Keystroke in 256 KiB and in 32 KiB on one line, the editor alone | — | 143–144 ms and 18.3–32.6 ms | as above | as above |
| Value panel, 20 MB values: first frame showing them | 250 ms | 9–173 ms | 5.67 / 16.70 / 16.03 | `value_panel_frames_within_budget` |
| Value panel, 20 MB values: frames unchanged and scrolling | 16 ms | 0.08–0.12 ms unchanged, 0.09–0.94 ms scrolling | as above | as above |
| Copying 10,000 rows (3–4.6 MB) in each format | 250 ms | `clipboard.copy` 13–59 ms; its frame 16–58 ms | 53.35 / 28.18 / 18.85, and 3.00 / 12.56 / 15.29 | `copy_frames_within_budget` |
| A frame with a wide result, 50 / 100 / 300 / 1,664 columns, the whole window without an AccessKit tree | none (a 60 Hz frame is 16.7 ms) | nothing changing 6.5–13.9 / 10.7–27.1 / 38–67 / 231–897 ms; scrolling 8.2–17.3 / 15.4–39.8 / 50–97 / 357–778 ms; in kittest, with an AccessKit tree, 376–792 ms at 1,664 columns (340–890 ms in earlier runs). Medians of NULL and short-text cells ([gaps.md](gaps.md) 5) | five runs, 7.5, 14.9, 18.3, 6.2 and 4.4 at their start (the fifth on 2026-10-27) | `wide_result_frames` |
| The whole M1 day in the real window, through the SSH tunnel | — | 10.5–12.4 s for the test; from the SSH connect to the open Postgres session 0.14–0.33 s | about 2.5–5 for the 12.4 s run; about 50 for the other | `m1_day_end_to_end` |

Every target above is met in every run, at every load. The wide-result row has none: at 100 columns its frame passed 60 Hz's 16.7 ms in four of the five runs, and at 300 in all ([gaps.md](gaps.md) 5).

### Measured again on every engine (M2)

Measured on 2026-11-05 on the same machine, in a release build (`cargo test --workspace --release … -- --ignored --nocapture --test-threads=1`), three runs between 17:00 and 17:01. **Measured on a loaded laptop:** the load average (1, 5 and 15 minutes) was 16.71 / 17.93 / 17.68 at the start and 12.38 / 16.63 / 17.25 at the end, from other work on the machine. Each number is the median of the three runs' medians, with the range of the three after it. MySQL 5.7, the newest MySQL and MariaDB 10.11 hold the light seed, 30,000 rows in `shop.big_rows`; the others a million.

| What | Target | PostgreSQL 14 | PostgreSQL 18 | MySQL 5.7 | MySQL 8.0 | MySQL 26.10 | MariaDB 10.11 | MariaDB 13.0 | SQLite | Test |
|---|---|---|---|---|---|---|---|---|---|---|
| Next page of `shop.big_rows`, median of 20 loads | 200 ms | 1.40 ms (1.29–2.11) | 1.48 ms (1.41–2.66) | 2.79 ms (2.39–8.12) | 2.88 ms (1.62–4.54) | 1.57 ms (1.42–3.86) | 1.61 ms (1.57–2.33) | 1.88 ms (1.65–3.46) | 0.43 ms (0.39–0.88) | `next_page_on_a_million_rows_is_fast` |
| A 20 MB cell's page | 10 s | 1.26 s (1.14–2.02) | 0.90 s (0.68–0.96) | — | 1.14 s (0.68–1.59) | — | — | 0.81 s (0.64–2.67) | 0.20 s (0.16–0.39) | `twenty_megabyte_cell_loads_within_ten_seconds` |
| A page drawn in the frame its rows arrive | same frame | yes | yes | yes | yes | yes | yes | yes | yes | `a_page_draws_in_the_frame_its_rows_arrive` (runs in `cargo xtask ci`, a dev build) |

Quick search (Ctrl+P) over 1,000 tables scores the session's catalog in memory, the same code whatever the engine, so it has one row: 357 µs median (235–481 µs over the three runs; slowest single search 1.7–3.7 ms), and 426 µs (307–496 µs) with 100 saved connections, against 2 ms (`quick_search_1000_tables_within_a_frame`, `quick_search_with_connections_within_budget`).

Every target is met on every engine, in every run.

The launch check, a release build to the connection screen with the window shown (`connection_screen_within_one_second`, target 1 s), took 224, 228 and 199 ms on 2026-11-06 at a load of 0.99–1.30, and the same day 948 ms right after its own release build and 347 ms the run after, at a load of 8.5–9.5. On 2026-11-05 it took 1.82 s at a load of about 24, right after its own release build (M1 measured 301 ms in that state), and later the same day it could not draw a frame, as the desktop was locked.

The M2 day itself (`m2_day_end_to_end`, a dev build: SQLite, MySQL through SSH, MariaDB and back) took 40.3 s and 39.8 s on 2026-11-05 (the load was not recorded for those two), 91.9 s in that day's `--ignored` run (load 22.14 at its start), and 37.6 s on 2026-11-06 (load 7.79 at its start, 5.70 at its end).

Beekeeper Studio's own timings were not measured.

## Platforms

Linux x86_64: built, packaged (tarball, AppImage) and started from the tarball on 2026-10-26; the M2 app's tarball built on 2026-11-06 from `df2f3ca` (sha256 `38051b3c89fde4a1213fb9a0c80297f173c9d322ea5297a4c439f4ae74f441fa`; the AppImage was not rebuilt: its tool now downloads the AppImage runtime while it builds). The rest of the matrix was built by the release pipeline in its first test release (2026-10-29, version 0.1.0, from the M1 code); only Windows ARM64 was installed and started. Regular CI builds Linux only. See [hardware-parity.md](hardware-parity.md).

## Ecosystem and AI

The control channel (`dbee --control <port>`) reaches every command and drives the real window: clicks, keys, text, screenshots and inspection. The test suites and the M1 day use it. The CLI's database commands are planned ([cli.md](cli.md)). There are no AI features in dbee (a non-goal).

## Stage

Pre-alpha. M1 and M2 are built: five of the six alpha gate rows pass, on all four engines ([roadmap.md](roadmap.md)), and file export (M4, after the M3 redesign) is the sixth. Both wait for a day of real use on the maintainer's own databases.

## Effort and calibration

M1 ran from 2026-10-19 to 2026-10-27 (measured, git): 28 planned tasks, 66 commits with the fixes that followed.

M2 ran from 2026-10-28 to 2026-11-08 (measured, git): 21 tasks, and the fixes that followed. At 2026-11-08 its branch held 92 commits of its own and one merge of `main`.

Agent hours were not logged in either, so per-area hours are not estimated yet. The first milestone that logs them gives the calibration.

## What works today

- **Engines:** PostgreSQL 14 and newer; MySQL 5.7 and newer; MariaDB 10.11 and newer; SQLite files.
- **Connecting:** saved and recent connections; the database type's cards, then its form with Fill from URL; TLS modes with a CA file; an SSH tunnel with key (found on its own), agent or password and host key checks; the server's databases listed under the Database field; read-only mode; passwords in the OS keyring; reconnect after a lost connection; a database file opened from the command line (`dbee <file>`), a drop on the window or the file manager, and saved from the status bar.
- **Browsing:** a schema tree with a filter (databases on MySQL and MariaDB); quick search (Ctrl+P) over the tables and the saved connections, switching connection; table tabs with a filter bar (13 operators, any number of filters, raw `WHERE`), sorting, paging by 100, estimated and exact counts, and foreign keys followed into a filtered tab.
- **Editing:** staged edits (an editor in the cell for every type, the value panel showing the same draft and taking new lines; set NULL, add and delete rows), undo and redo, and Apply or Ctrl+S in one transaction, all or nothing.
- **Values:** a value panel with the whole value of a cell, or a selected row as JSON, JSON pretty-printed and coloured, selectable and copyable.
- **Queries:** query tabs with syntax colours in the engine's dialect; run all, or the statement under the cursor; cancel; several results; an error band that jumps to the position, with ROLLBACK under a failed transaction; an open transaction shown in the status bar and asked about on close; on MySQL and MariaDB the database a tab runs in.
- **Copying:** values, with a header, or as CSV, JSON or Markdown, up to 8 MiB.
- **Structure:** a table tab's STRUCTURE, read-only: columns, indexes, foreign keys, the keys that reference the table, triggers.
- **Look:** M3's redesign on every screen built so far, in light and dark; nothing that waits shows before it has run 300 ms.
- **Agents:** a localhost control channel, and isolated runs (`cargo xtask run-isolated`).

## Not measured yet

Beekeeper Studio's own timings; dbee on macOS, Windows and FreeBSD; a day of real use on any engine; memory use with large results.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-23 | minor | Checked against the M3 tree. Query: what dbee does with a transaction a script opens (manual transactions still missing). Data: the value panel shows a selected row and an edited cell's draft. "What works today": the cell editor, the error band, the transaction. "Effort and calibration" keeps the dates and counts only. Grades unchanged. |
| 2026-11-20 | minor | M3's screens: Structure 40 % (2 of 5, read-only; estimated), App 12 % (light and dark themes), features ~38 %, overall ~30 % (estimated); UI ~45 % stands (24 elements, 3 done, 18 partial); what works today gains Structure and the look. |
| 2026-11-08 | minor | Platforms: the first test release built every row (2026-10-29), regular CI builds Linux only; M2 effort; import and export are M4 (M3 is the Redesign). |
| 2026-11-06 | minor | The M2 tarball as rebuilt from `df2f3ca`, with its sha256; the launch check right after its release build (948 and 347 ms at a load of 8.5–9.5). |
| 2026-11-06 | minor | The launch check measured at a low load (199–228 ms); the M2 day's runs with their loads; the 20,000-step fuzz run again. |
| 2026-11-05 | major | M2 built: MySQL, MariaDB and SQLite (4 of 20 engines, ~29 % overall, estimated); readiness for SQLite and MySQL users; M1's targets measured again on every engine, in a release build, with the load; the M2 day; effort for M2. |
| 2026-10-29 | trivial | The MCP server is no longer described until it exists. |
| 2026-10-28 | minor | Import and export is now M3; M2 is three databases (MySQL/MariaDB and SQLite). |
| 2026-10-27 | minor | Wording: the day of real use on the maintainer's own databases, M1 fixes, earlier runs instead of task ids. |
| 2026-10-27 | trivial | The observation notes are cited as kept outside the repository, not by their local paths. |
| 2026-10-27 | minor | Effort counts 59 commits for M1 (four for its last task) and five for the M1 fixes; gaps.md 13 is now 12. |
| 2026-10-27 | minor | Performance is the one source for the timings gaps.md and roadmap.md quote: wide results scrolling and in kittest, keystrokes in 256 KiB and 32 KiB on one line, the earlier 1 MiB figure added; a fifth wide-result run. |
| 2026-10-26 | major | M1 built: first assessment. Feature areas graded against the observed catalog; dimension scores; ~25 % overall (estimated); performance measured with machine and load; readiness by audience. |
| 2026-10-15 | major | Template (scaffold). |

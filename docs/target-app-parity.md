# Target app parity

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** minor (import and export are M3) · **Target:** Beekeeper Studio 5.5.7

## Summary

dbee covers the daily Postgres workflow: connect (also through an SSH tunnel), find a table, filter it, read its JSON, follow a foreign key, edit rows, run queries and copy results. It works end to end in the real window (`apps/dbee/tests/m1_day.rs`, measured). Postgres is the only engine, and there is no file export or import yet (M3). MySQL/MariaDB and SQLite come first (M2).

Overall parity with Beekeeper Studio: **~25 % (estimated)**, from the weights and dimension scores below. Breadth comes from grading Beekeeper's observed feature list row by row. Readiness stays low until dbee has been used on real databases (a day of real use on the maintainer's own databases, M1's last check).

## Readiness by audience

| Audience | Ready % | Agent wall-clock hours to ~95% | Work that dominates |
|---|---|---|---|
| Developer querying a local SQLite file | 0 % (measured: no SQLite engine) | not estimated yet | the second engine, on the planned driver interface |
| Backend developer on Postgres/MySQL | ~30 % (estimated): Postgres's daily workflow only, not yet used on real databases | not estimated yet | file export and import (M3), MySQL, structure views, saved queries and history |
| Data analyst exporting results | ~10 % (estimated): copies of up to 8 MiB only | not estimated yet | file export (M3), results past 10,000 rows |

## What we measured against and how

Beekeeper Studio 5.5.7, observed black-box in a sandbox, using synthetic databases only. The evidence is in observation notes kept outside the repository (they hold Beekeeper Studio screenshots, which are not ours to publish). Feature breadth grades each row of the feature catalog in those notes: **1** done, **0.5** partial, **0** missing. A row is done only when it behaves as observed, for Postgres. Engine rows are graded in [driver-parity.md](driver-parity.md), not here. dbee's side was measured with the test suites against the Docker test servers (Postgres 14 and 18, an sshd) and the real window (`m1_day`).

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
| Connections | 15 | 62 % (7.5 of 12 rows; estimated) | ~45 % (estimated) | not estimated yet | Done: read-only mode, test, save with name, colour and passwords, saved and recent lists with a filter, Fill from URL. Partial: no socket mode or IAM; SSL without client certificates; SSH without a jump host; no Copy URL. Missing: a sample database, team workspaces. `crates/engine/tests/connection.rs`, `crates/ui-egui/tests/connections.rs`, `m1_day` step 1. |
| Navigation | 10 | 50 % (3 of 6; estimated) | ~40 % (estimated) | not estimated yet | Done: the schema tree, quick search. Partial: the tree's filter and refresh only; tabs without reopen or restore. Missing: database switcher, several windows. `crates/ui-egui/tests/workspace.rs`, `m1_day` step 2. |
| Query | 20 | 32 % (3.5 of 11; estimated) | ~25 % (estimated) | not estimated yet | Done: run all and run current, several result sets, errors. Partial: the editor has no autocomplete. Missing: manual transactions, saved queries, history, Vim keymap and wrap, find, run to file, AI shell. `crates/ui-egui/tests/query_tab.rs`, `m1_day` step 7. |
| Results | 10 | 33 % (1 of 3; estimated) | ~30 % (estimated) | M3 | Done: copy as TSV (with or without header), CSV, JSON, Markdown. Missing: downloads. `crates/ui-egui/tests/clipboard.rs`. |
| Data | 25 | 59 % (6.5 of 11; estimated) | ~45 % (estimated) | not estimated yet | Done: the filter builder (any number of filters, raw `WHERE`), paging with counts, following a foreign key. Partial: the grid has no tooltips; editing has no copy as SQL, and rows have no clone; the value panel (pretty JSON, copy) stands in for two rows, the edit modal and the JSON row viewer, but it is read-only and shows one cell, not the row; copy has no SQL or IN list formats; columns resize only. Missing: quick filter. `crates/ui-egui/tests/table_tab.rs`, `editing.rs`, `value_panel.rs`, `m1_day` steps 3–6. |
| Structure | 8 | 0 % (0 of 5) | 0 % | not estimated yet | Not started. |
| Import/export | 7 | 0 % (0 of 3) | 0 % | M3 | Not started ([gaps.md](gaps.md) 1). |
| Tools | 3 | 0 % (0 of 2) | 0 % | not estimated yet | Backup, restore and plugins not started. |
| App | 2 | 0 % (0 of 4) | 0 % | not estimated yet | One light theme of dbee's own; no zoom or help menus. |

Features, weighted by area: **39 %**, put at **~35 % (estimated)**.

## Dimensions

| Dimension | Weight | Score | Kind | Basis |
|---|---|---|---|---|
| Features | 40 | ~35 % | estimated | the feature areas above |
| Database engines | 20 | 5 % | measured (1 of 20 engines) | [driver-parity.md](driver-parity.md) |
| UI | 12 | ~45 % | estimated | [ui-parity.md](ui-parity.md): 22 elements graded |
| Performance | 8 | ~30 % | estimated | dbee meets its own M1 targets (measured, below), but a frame passes 16.7 ms at about 100 columns in most runs. Beekeeper's timings were not measured, so there is no side-by-side. |
| Stability | 8 | ~25 % | estimated | Never-crash rules and the panic guard; the command-sequence fuzz at 20,000 steps and every command with empty params pass (measured). No use on real databases yet. |
| File formats (import/export) | 4 | 0 % | measured | none ([file-format-parity.md](file-format-parity.md)) |
| Localization | 4 | 0 % | measured | English only ([localization-parity.md](localization-parity.md)) |
| Platforms | 2 | ~25 % | estimated | Linux x86_64 built, packaged and run; macOS, Windows and FreeBSD not built in M1 ([hardware-parity.md](hardware-parity.md)) |
| Ecosystem and AI | 2 | ~25 % | estimated | The localhost control channel drives every command and the window (`docs/control-protocol.md`); MCP and the CLI's database commands are planned; no AI features |

Weighted: 0.40 × 35 + 0.20 × 5 + 0.12 × 45 + 0.08 × 30 + 0.08 × 25 + 0.02 × 25 + 0.02 × 25 = **25.8 %**, given as **~25 % (estimated)**.

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

Beekeeper Studio's own timings were not measured.

## Platforms

Linux x86_64: built, packaged (tarball, AppImage) and started from the tarball on 2026-10-26. The rest of the matrix was not built in M1. CI builds wait for the GitHub repository. See [hardware-parity.md](hardware-parity.md).

## Ecosystem and AI

The control channel (`dbee --control <port>`) reaches every command and drives the real window: clicks, keys, text, screenshots and inspection. The test suites and the M1 day use it. `dbee-cli mcp` and the CLI's database commands are planned ([mcp.md](mcp.md), [cli.md](cli.md)). There are no AI features in dbee (a non-goal).

## Stage

Pre-alpha. M1 is built: five of the six alpha gate rows pass ([roadmap.md](roadmap.md)), and file export (M3) is the sixth. M1 waits for a day of real use on the maintainer's own databases.

## Effort and calibration

M1 ran from 2026-10-19 to 2026-10-27 (measured, git): 28 planned tasks in 59 commits: 54 for the first 27 tasks (each reviewed, then fixed where the review found problems), one toolchain pin and four for the last task. The M1 review fixes followed in seven more, 66 commits in all. Agent hours were not logged, so per-area hours are not estimated yet. The first milestone that logs them gives the calibration.

## What works today

- **Connecting:** saved and recent connections; a form with Fill from URL; TLS modes with a CA file; an SSH tunnel with key (found on its own), agent or password and host key checks; read-only mode; passwords in the OS keyring; reconnect after a lost connection.
- **Browsing:** a schema tree with a filter; quick search (Ctrl+P); table tabs with a filter bar (13 operators, any number of filters, raw `WHERE`), sorting, paging by 100, estimated and exact counts, and foreign keys followed into a filtered tab.
- **Editing:** staged edits (one-line, multi-line and boolean editors, set NULL, add and delete rows), undo and redo, and Apply or Ctrl+S in one transaction, all or nothing.
- **Values:** a value panel with the whole value of a cell, JSON pretty-printed and coloured, selectable and copyable.
- **Queries:** query tabs with syntax colours; run all, or the statement under the cursor; cancel; several results; error cards that jump to the position.
- **Copying:** values, with a header, or as CSV, JSON or Markdown, up to 8 MiB.
- **Agents:** a localhost control channel, and isolated runs (`cargo xtask run-isolated`).

## Not measured yet

Beekeeper Studio's own timings; dbee on macOS, Windows and FreeBSD; a day of real use; memory use with large results.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-28 | minor | Import and export is now M3; M2 is three databases (MySQL/MariaDB and SQLite). |
| 2026-10-27 | minor | Wording: the day of real use on the maintainer's own databases, M1 review fixes, earlier runs instead of task ids. |
| 2026-10-27 | trivial | The observation notes are cited as kept outside the repository, not by their local paths. |
| 2026-10-27 | minor | Effort counts 59 commits for M1 (four for its last task) and five for the M1 review fixes; gaps.md 13 is now 12. |
| 2026-10-27 | minor | Performance is the one source for the timings gaps.md and roadmap.md quote: wide results scrolling and in kittest, keystrokes in 256 KiB and 32 KiB on one line, the earlier 1 MiB figure added; a fifth wide-result run. |
| 2026-10-26 | major | M1 built: first assessment. Feature areas graded against the observed catalog; dimension scores; ~25 % overall (estimated); performance measured with machine and load; readiness by audience. |
| 2026-10-15 | major | Template (scaffold). |

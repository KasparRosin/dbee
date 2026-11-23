# Roadmap: gates and milestones

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-23 · **Change:** trivial (wording: M1 is built, the query tab's error band; checked against the code) · **Target:** Beekeeper Studio 5.5.7

The one-page summary is [`ROADMAP.md`](../ROADMAP.md) at the repository root. This page holds the detail behind it: the current focus, the alpha and beta gates, and the milestones.

## Current focus

M1 "Daily Postgres workflow" is built, and the M1 day runs end to end in the real window. It waits for its last check: a day of real use of a Linux release build on the maintainer's own databases. Only the maintainer does that; agents never connect to those databases.

M2 "Three databases" is built: one driver interface, with Postgres behind it, and MySQL, MariaDB and SQLite on it with the whole M1 workflow. The M2 day runs end to end in the real window on all three (`apps/dbee/tests/m2_day.rs`). Like M1, it waits for a day of real use on the maintainer's own MySQL, MariaDB and SQLite databases. The Redesign (M3) is in progress: its screens are built (2026-11-20), in light and dark, and the fixes from looking at them followed up to 2026-11-23. M4 "Import and export" comes next, for all four engines.

## Alpha gate

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| Connect to Postgres, MySQL, MariaDB or SQLite (M1, M2) | yes (measured) | `m1_day` step 1 (`apps/dbee/tests/m1_day.rs`): a double-click on a saved connection reaches the workspace through the test sshd with the key found on its own, no prompt. Driver and engine tests: TLS modes, the CA file, key, agent and password SSH logins, host keys, read-only mode, lost connections (`crates/pg/tests`, `crates/ssh/tests`, `crates/engine/tests/connection.rs`), on Postgres 14 and 18. `m2_day`: MySQL 8.0 through the test sshd from the connection form (the engine cards, the Database field's list), the newest MariaDB directly, and a SQLite file opened with `dbee <file>`; the driver tests in `crates/mysql/tests` and `crates/sqlite/tests`. | 0 |
| Browse and filter tables (M1) | yes (measured) | `m1_day` steps 2, 3 and 5, and `m2_day` on each engine: Ctrl+P, three filters through the bar, a foreign key followed into a filtered tab. Paging, sort, count and odd identifiers in `crates/engine/tests/table.rs` and `crates/ui-egui/tests/table_tab.rs`; the next page of a million-row table in 1.06–1.09 ms on Postgres at the end of M1, and in 0.4–2.9 ms on every engine at the end of M2 (release build, loaded laptop; `next_page_on_a_million_rows_is_fast`; [target-app-parity.md](target-app-parity.md), "Performance"). | 0 |
| Run a query (M1) | yes (measured) | `m1_day` step 7: a join pasted and run with Ctrl+Enter. `m2_day` on each engine: three result sets, a cancelled statement, a join; on MySQL and MariaDB the query tab's database. Run current, cancel, several results, the error band and the row cap in `crates/engine/tests/query.rs` and `crates/ui-egui/tests/query_tab.rs`. | 0 |
| Edit a row (M1) | yes (measured) | `m1_day` step 6: a name fixed and a date cleared to NULL, each written with Ctrl+S and checked with psql. `m2_day` on each engine: a name fixed, a row deleted and one added, all in one Ctrl+S, checked with the engine's own client. All-or-nothing Apply with a failing change in `crates/pg/tests/apply.rs`, `crates/engine/tests/edit.rs` and `crates/ui-egui/tests/editing.rs`. | 0 |
| Copy results (M1) | yes (measured) | `m1_day` steps 4 and 7, and `m2_day` on each engine: a JSON line copied from the value panel, and a result copied with its header row (Ctrl+A, Ctrl+Shift+C). The formats in `crates/core` and `crates/ui-egui/tests/clipboard.rs`. | 0 |
| Export to a file (M4) | no | Not built ([gaps.md](gaps.md) entry 1). | not estimated yet |

## Beta gates

Not defined yet: they follow M4, from the ranked list in [gaps.md](gaps.md) and [target-app-parity.md](target-app-parity.md).

| Gate | Gap | Hours |
|---|---|---|

## Milestones

| # | Milestone | Status |
|---|---|---|
| M0 | Scaffold | done (2026-10-16) |
| M1 | Daily Postgres workflow | built (2026-10-26); waits for a day of real use on the maintainer's own databases |
| M2 | Three databases | built (2026-11-05); waits for a day of real use on the maintainer's own databases |
| M3 | Redesign | in progress: the screens are built (2026-11-20), fixes from looking at them up to 2026-11-23 |
| M4 | Import and export | not started |

The packages offer dbee for SQLite files without taking the file type over: the Linux desktop entry and AppStream data list `application/vnd.sqlite3` and `application/x-sqlite3` (the user's settings pick the default), and the Windows installer adds dbee to the Open with list of `.sqlite` and `.sqlite3`. The macOS bundle declares no document types yet ([gaps.md](gaps.md) entry 17). On every platform a file named on the command line, or dropped on the window, opens.

## Measuring

Percentages and hours come from [target-app-parity.md](target-app-parity.md) and follow [standards/progress-docs.md](standards/progress-docs.md): each number is marked measured or estimated, hours are agent wall-clock hours to about 95%, and a percentage is raised only with evidence a reviewer can rerun. Gaps are listed in [gaps.md](gaps.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-23 | trivial | Checked against the code (the day tests' steps, the test servers' versions, the milestones): still true. Wording: M1 is built; the query tab's errors are the error band. |
| 2026-11-23 | minor | M3 is in progress: its screens are built (2026-11-20), with fixes up to 2026-11-23. |
| 2026-11-08 | minor | Import and export is now M4; M3 is the Redesign. Checked against the code: still true. |
| 2026-11-05 | major | M2 "Three databases" built: MySQL, MariaDB and SQLite with the whole M1 workflow; the alpha gate's rows cover them (`m2_day`). |
| 2026-11-05 | minor | The packages offer dbee for SQLite files (Linux MIME types, Windows Open with), never as the default; macOS not yet (gaps.md entry 17). |
| 2026-10-28 | major | Milestones renumbered: M2 "Three databases" (driver interface, MySQL/MariaDB, SQLite) is in progress; "Import and export" is now M3. |
| 2026-10-27 | trivial | Titled "gates and milestones", with a line on its role next to ROADMAP.md. |
| 2026-10-27 | minor | Wording: M1's last check is a day of real use on the maintainer's own databases. |
| 2026-10-27 | minor | The packages' `.sqlite` and `.sqlite3` file associations removed until the SQLite engine exists. |
| 2026-10-27 | minor | The next page's timing quoted from target-app-parity.md (1.06–1.09 ms). |
| 2026-10-26 | major | M1 built: five of six alpha gate rows pass with evidence (`m1_day` and the driver, engine and UI tests); M1 waits for a day of real use; next an architecture step, then M2. |
| 2026-10-19 | minor | M1 "Daily Postgres workflow" defined and in progress; alpha gate rewritten for Postgres; M2 "Import and export" added. |
| 2026-10-15 | major | Template (scaffold). |

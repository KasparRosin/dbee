# Roadmap: gates and milestones

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** major (M2 is three databases; import and export are M3) · **Target:** Beekeeper Studio 5.5.7

The one-page summary is [`ROADMAP.md`](../ROADMAP.md) at the repository root. This page holds the detail behind it: the current focus, the alpha and beta gates, and the milestones.

## Current focus

M1 "Daily Postgres workflow" is built. Every task of its plan is done, and the M1 day runs end to end in the real window. It waits for its last check: a day of real use of a Linux release build on the maintainer's own databases. Only the maintainer does that; agents never connect to those databases.

In progress: M2 "Three databases". A driver interface comes first, so the engine no longer names Postgres types; then MySQL/MariaDB and SQLite get the full M1 workflow on it. M3 "Import and export" follows.

## Alpha gate

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| Connect to Postgres (M1) | yes (measured) | `m1_day` step 1 (`apps/dbee/tests/m1_day.rs`): a double-click on a saved connection reaches the workspace through the test sshd with the key found on its own, no prompt. Driver and engine tests: TLS modes, the CA file, key, agent and password SSH logins, host keys, read-only mode, lost connections (`crates/pg/tests`, `crates/ssh/tests`, `crates/engine/tests/connection.rs`), on Postgres 14 and 18. | 0 |
| Browse and filter tables (M1) | yes (measured) | `m1_day` steps 2, 3 and 5: Ctrl+P, three filters through the bar, a foreign key followed into a filtered tab. Paging, sort, count and odd identifiers in `crates/engine/tests/table.rs` and `crates/ui-egui/tests/table_tab.rs`; the next page of a million-row table in 1.06–1.09 ms (`next_page_on_a_million_rows_is_fast`; [target-app-parity.md](target-app-parity.md), "Performance"). | 0 |
| Run a query (M1) | yes (measured) | `m1_day` step 7: a join pasted and run with Ctrl+Enter. Run current, cancel, several results, error cards and the row cap in `crates/engine/tests/query.rs` and `crates/ui-egui/tests/query_tab.rs`. | 0 |
| Edit a row (M1) | yes (measured) | `m1_day` step 6: a name fixed and a date cleared to NULL, each written with Ctrl+S and checked with psql. All-or-nothing Apply with a failing change in `crates/pg/tests/apply.rs`, `crates/engine/tests/edit.rs` and `crates/ui-egui/tests/editing.rs`. | 0 |
| Copy results (M1) | yes (measured) | `m1_day` steps 4 and 7: a JSON line copied from the value panel, and a result copied with its header row (Ctrl+A, Ctrl+Shift+C). The formats in `crates/core` and `crates/ui-egui/tests/clipboard.rs`. | 0 |
| Export to a file (M3) | no | Not built ([gaps.md](gaps.md) entry 1). | not estimated yet |

## Beta gates

Not defined yet: they follow M3, from the ranked list in [gaps.md](gaps.md) and [target-app-parity.md](target-app-parity.md).

| Gate | Gap | Hours |
|---|---|---|

## Milestones

| # | Milestone | Status |
|---|---|---|
| M0 | Scaffold | done (2026-10-16) |
| M1 | Daily Postgres workflow | built (2026-10-26); waits for a day of real use on the maintainer's own databases |
| M2 | Three databases | in progress |
| M3 | Import and export | not started |

The packages declare no file types for now: the `.sqlite` and `.sqlite3` associations (Linux desktop entry and AppStream data, macOS, Windows) come back with the SQLite engine.

## Measuring

Percentages and hours come from [target-app-parity.md](target-app-parity.md) and follow [standards/progress-docs.md](standards/progress-docs.md): each number is marked measured or estimated, hours are agent wall-clock hours to about 95%, and a percentage is raised only with evidence a reviewer can rerun. Gaps are listed in [gaps.md](gaps.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-28 | major | Milestones renumbered: M2 "Three databases" (driver interface, MySQL/MariaDB, SQLite) is in progress; "Import and export" is now M3. |
| 2026-10-27 | trivial | Titled "gates and milestones", with a line on its role next to ROADMAP.md. |
| 2026-10-27 | minor | Wording: M1's last check is a day of real use on the maintainer's own databases. |
| 2026-10-27 | minor | The packages' `.sqlite` and `.sqlite3` file associations removed until the SQLite engine exists. |
| 2026-10-27 | minor | The next page's timing quoted from target-app-parity.md (1.06–1.09 ms). |
| 2026-10-26 | major | M1 built: five of six alpha gate rows pass with evidence (`m1_day` and the driver, engine and UI tests); M1 waits for a day of real use; next an architecture step, then M2. |
| 2026-10-19 | minor | M1 "Daily Postgres workflow" defined and in progress; alpha gate rewritten for Postgres; M2 "Import and export" added. |
| 2026-10-15 | major | Template (scaffold). |

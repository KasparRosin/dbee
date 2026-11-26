# dbee roadmap

**Stage: pre-alpha** · next: alpha, once file export lands (M4). M1 "Daily Postgres workflow" and M2 "Three databases" are built and wait for their last check: a day of real use on the maintainer's own databases.

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-26 · **Change:** minor (the update button's click opens the release's page, in the progress log; parity numbers unchanged) · **Target:** Beekeeper Studio 5.5.7

Why pre-alpha: five of the six alpha gate rows work end to end, measured in the real window (`apps/dbee/tests/m1_day.rs` on Postgres, `apps/dbee/tests/m2_day.rs` on MySQL, MariaDB and SQLite; both `#[ignore]`: `cargo xtask ci` skips them, and the CI workflow runs them on a virtual screen on every pull request). They are connecting (a server also through an SSH tunnel), browsing and filtering tables, running a query, editing a row and copying results. The sixth, export to a file, is M4. Detail: [`docs/target-app-parity.md`](docs/target-app-parity.md) (the assessment), [`docs/gaps.md`](docs/gaps.md) (the ranked work list), [`docs/roadmap.md`](docs/roadmap.md) (gates and milestones); every doc is listed in [`docs/README.md`](docs/README.md).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Overall parity with Beekeeper Studio | ~30 % | estimated (weighted dimensions, [target-app-parity](docs/target-app-parity.md)) |
| Alpha gate rows that pass | 5 of 6, on all four engines | measured (`m1_day`, `m2_day` and the test suites, [roadmap](docs/roadmap.md)) |
| Commands registered | 68: 59 engine, 9 UI-only | measured (the command tables, 2026-11-23, and `query.sortResult` added on 2026-11-25) |
| Tests | 1,848 `#[test]` attributes in 126,484 lines of Rust; the test harness lists 2,132 tests, the ignored ones included | measured (`cargo xtask stats --exact`, 2026-11-23) |

## By dimension

| Dimension | Parity | Remaining (h) | Doc |
|---|---|---|---|
| Features | ~38 % (estimated) | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Database engines | 4 of 20: PostgreSQL, MySQL, MariaDB, SQLite (20 %, measured) | not estimated yet | [driver-parity](docs/driver-parity.md) |
| UI | ~45 % (estimated) | not estimated yet | [ui-parity](docs/ui-parity.md) |
| Performance | M1's targets met, again on every engine (measured); ~30 % against Beekeeper (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| Stability | 20,000-step command fuzz clean (measured on 2026-11-06 with `DBEE_FUZZ_STEPS=20000`; ci runs 300 steps); ~25 % (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| File formats (import/export) | 0 % (measured) | M4 | [file-format-parity](docs/file-format-parity.md) |
| Localization | 0 % (measured) | not estimated yet | [localization-parity](docs/localization-parity.md) |
| Platforms | Linux x86_64 built, packaged and run; the rest built by the release pipeline, and only Windows ARM64 of them installed and started | — | [hardware-parity](docs/hardware-parity.md) |

Hours are not estimated yet: M1's and M2's agent hours were not logged, so there is nothing to calibrate on.

## Features

Breadth grades Beekeeper's observed feature list row by row: done 1, partial 0.5 (estimated). Ready is lower until dbee has been used on real databases.

| Area | Breadth | Ready | Hours | Doc |
|---|---|---|---|---|
| Connections | 62 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Navigation | 50 % | ~40 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Query | 32 % | ~25 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Results | 33 % | ~30 % | M4 | [target-app-parity](docs/target-app-parity.md) |
| Data | 59 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Structure | 40 % | ~30 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Import/export | 0 % | 0 % | M4 | [file-format-parity](docs/file-format-parity.md) |
| Tools, App | 5 % | ~4 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |

## Languages

| Language | UI | Docs |
|---|---|---|
| English | yes (the only one) | yes |

## Upcoming

1. The last check of M1 and M2: a day of real use of a Linux release build on the maintainer's own Postgres, MySQL, MariaDB and SQLite databases. Only the maintainer does this step; agents never connect to those databases.
2. M3 Redesign: a new look and layout for every screen, with dark mode. Its screens are built (2026-11-20), and the fixes from looking at them are in up to 2026-11-23 (the grid's header and row selection, the cell editor, STRUCTURE's columns, the view switch, selected text); its last checks and the day pictures come next.
3. M4 Import and export, for all four engines: file export and import, the last alpha gate row, and dump and restore with each engine's own tools.

## Progress log

| Date | Entry |
|---|---|
| 2026-11-26 | A click on `DOWNLOAD <version>` starts the desktop's opener with the release's page (run on Linux with a stand-in opener; a real browser, macOS and Windows were not run). In 0.3.1 and 0.3.2 the click opens nothing, so those two are updated by hand ([decisions.md](docs/decisions.md), 2026-11-26; [gaps.md](docs/gaps.md) entry 59). Releases carry a signed checksum file from 0.3.1 on. |
| 2026-11-25 | A click on a column header of a query tab's result sorts it (asc, desc, then as the server sent it), as it does in a table tab. The result is sorted in memory, among the rows it holds, by dbee's own rules, not the database's `ORDER BY` ([decisions.md](docs/decisions.md), 2026-11-25; [gaps.md](docs/gaps.md) entries 7 and 89). |
| 2026-11-23 | The update check: an installed copy asks GitHub for the newest release once a day, and when one is newer the right end of the menu bar reads `DOWNLOAD <version>`, which opens the release's page. Each package now says how it was installed. dbee installs nothing by itself yet, and releases are not signed ([gaps.md](docs/gaps.md) entry 59). |
| 2026-11-23 | M3, the fixes from looking at the screens (2026-11-22 and 2026-11-23): several whole rows are selected by a drag over the row numbers or by Shift with the arrow keys, which also move the selection, and are copied whole; every cell editor opens in its cell, one line whatever the type, while the value panel shows the same draft; a grid header cuts its words with `…`, draws long type names in their short spelling and says the whole on hover; STRUCTURE's columns take the page's spare room where their content is cut and are resized at their edges; the DATA / STRUCTURE switch stands in the table tab's bottom bar in both views; selected text has a colour of its own, stronger than a selected row's. Parity numbers unchanged. |
| 2026-11-20 | M3's screens built: the table tab's bars, the query tab (result bar, error band with ROLLBACK), the value panel inside the tab (WRAP, editing several lines), the schema tree's menu and count, the connection screen (one list, a two-column form, CONNECT saves), the quick search grouped by connection, the empty and loading states on one 300 ms clock, and a read-only STRUCTURE page; light and dark. |
| 2026-11-08 | Milestones renumbered: M3 is the Redesign, and import and export move to M4. Alpha waits for M4's file export. |
| 2026-11-08 | Bugs fixed: two dbee windows no longer lose each other's saved connections (each change re-reads the file under a lock); a newer dbee's connections file is read and never overwritten, and fields an older build does not know survive; a table tab's stopped load stops no other tab; the Postgres cancel and the MySQL KILL go to the server the connection reached; every wall-clock bound in a test is in an ignored timing twin. |
| 2026-11-06 | M2 fixes: a stopped statement's cancel is sent again until it ends and never reaches a later one; a stored password is not used with another CA file or an option that protects it less; the filter bar's `like` escapes with `\` on every engine; a file dropped while connected opens through the disconnect question, and one a saved connection names opens as saved (read-only kept); each engine's fixed facts in its own file. The engine differences that stay are in gaps.md. |
| 2026-11-05 | M2 "Three databases" built: one driver interface with Postgres behind it, and MySQL, MariaDB and SQLite with the whole M1 workflow: connect (a server also through SSH; a SQLite file from the command line or a drop), browse, filter, query, edit and copy. The Database field lists the server's databases (#1), and Ctrl+P finds saved connections and switches to one (#3). The M2 day runs end to end in the real window on all three. |
| 2026-10-28 | Milestones renumbered: M2 is "Three databases" (a driver interface, MySQL/MariaDB and SQLite), and import and export move to M3. SQLite's public-domain code is allowed. |
| 2026-10-27 | M1 fixes: a value longer than a `varchar(n)`, `char(n)` or `bit(n)` column fails the Apply instead of being written cut; a running Apply can be cancelled until it commits; generated columns are refused; notices read as sentences; the UI tests share one harness, and ci fails a skipped driver test once the servers are up. |
| 2026-10-26 | M1 "Daily Postgres workflow" built: connect (SSH tunnel, TLS, read-only), browse and filter, foreign keys, staged edits with all-or-nothing Apply, the value panel, query tabs, copying. The M1 day runs end to end in the real window. Linux tarball and AppImage built for the day of real use. |
| 2026-10-19 | M1 "Daily Postgres workflow" started: licence check (`cargo xtask licenses`) and roadmap. |
| 2026-10-15 | Scaffold: rules, docs, tooling, packaging and release pipeline in place; placeholder app. |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-26 | minor | Progress log: the update button's click opens the release's page; releases signed from 0.3.1 on. Parity numbers unchanged. |
| 2026-11-25 | minor | Progress log: a query result sorted by a header click. Commands: 68 (59 engine, 9 UI-only), `query.sortResult` added to the count of 2026-11-23; the tests were not counted again. Parity numbers unchanged. |
| 2026-11-23 | minor | Progress log: the update check and its button. Headline numbers counted again: 67 commands (58 engine, 9 UI-only), 1,848 `#[test]` attributes in 126,484 lines, 2,132 tests listed by the harness. Parity numbers unchanged. |
| 2026-11-23 | minor | Platforms: the other platforms are built by the release pipeline ([hardware-parity](docs/hardware-parity.md)); the row said they were not built yet. |
| 2026-11-23 | minor | Headline numbers counted again: 64 commands (55 engine, 9 UI-only), 1,769 `#[test]` attributes in 122,785 lines, 2,053 tests listed by the harness. The progress log and this table say what was fixed without naming reviews. Parity numbers unchanged. |
| 2026-11-23 | minor | Progress log and upcoming list to 2026-11-23 (the grid, the cell editor, STRUCTURE's columns, the view switch, selected text); the day tests run in the CI workflow; numbers unchanged. |
| 2026-11-20 | minor | M3's screens: Structure 40 %, Tools and App 5 %, features ~38 %, overall ~30 % (estimated); upcoming list and progress log. |
| 2026-11-08 | minor | M3 is the Redesign; import and export are M4; upcoming list and progress log updated. |
| 2026-11-08 | minor | Progress log: the bugs fixed on 2026-11-08. |
| 2026-11-06 | minor | Progress log: M2's fixes; test numbers measured again. |
| 2026-11-06 | minor | The 20,000-step fuzz run again and dated. |
| 2026-11-05 | major | M2 built: stage line, headline numbers (parity, engines, commands, tests), dimensions, upcoming list and progress log. |
| 2026-10-29 | trivial | Stability and alpha gate: how the fuzz and `m1_day` were run. |
| 2026-10-28 | minor | M2 is "Three databases"; import and export are M3; upcoming list and progress log updated. |
| 2026-10-27 | trivial | The detail docs say what they hold, and link the docs index. |
| 2026-10-27 | minor | Wording: the day of real use on the maintainer's own databases; M1 fixes. |
| 2026-10-27 | minor | Progress log: M1 fixes. |
| 2026-10-26 | major | M1 built: stage line, headline numbers, dimensions and feature areas assessed (measured and estimated marked), upcoming list, progress log. |
| 2026-10-19 | minor | M1 started: next line, upcoming list and progress log. |
| 2026-10-15 | major | First version (scaffold). |

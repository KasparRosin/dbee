# dbee roadmap

**Stage: pre-alpha** · next: alpha, once file export lands (M3). M1 "Daily Postgres workflow" and M2 "Three databases" are built and wait for their last check: a day of real use on the maintainer's own databases.

> **Last reviewed:** 2026-11-08 · **Last updated:** 2026-11-08 · **Change:** minor (the larger review's bugs) · **Target:** Beekeeper Studio 5.5.7

Why pre-alpha: five of the six alpha gate rows work end to end, measured in the real window (`apps/dbee/tests/m1_day.rs` on Postgres, `apps/dbee/tests/m2_day.rs` on MySQL, MariaDB and SQLite; both run by hand: `#[ignore]`, ci skips them). They are connecting (a server also through an SSH tunnel), browsing and filtering tables, running a query, editing a row and copying results. The sixth, export to a file, is M3. Detail: [`docs/target-app-parity.md`](docs/target-app-parity.md) (the assessment), [`docs/gaps.md`](docs/gaps.md) (the ranked work list), [`docs/roadmap.md`](docs/roadmap.md) (gates and milestones); every doc is listed in [`docs/README.md`](docs/README.md).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Overall parity with Beekeeper Studio | ~29 % | estimated (weighted dimensions, [target-app-parity](docs/target-app-parity.md)) |
| Alpha gate rows that pass | 5 of 6, on all four engines | measured (`m1_day`, `m2_day` and the test suites, [roadmap](docs/roadmap.md)) |
| Commands registered | 50: 47 engine, 3 UI-only | measured (the command tables) |
| Tests | 1,003 `#[test]` attributes in 66,848 lines of Rust; `cargo xtask ci` runs 1,188 | measured (`cargo xtask stats` and `cargo xtask ci`, 2026-11-06) |

## By dimension

| Dimension | Parity | Remaining (h) | Doc |
|---|---|---|---|
| Features | ~35 % (estimated) | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Database engines | 4 of 20: PostgreSQL, MySQL, MariaDB, SQLite (20 %, measured) | not estimated yet | [driver-parity](docs/driver-parity.md) |
| UI | ~45 % (estimated) | not estimated yet | [ui-parity](docs/ui-parity.md) |
| Performance | M1's targets met, again on every engine (measured); ~30 % against Beekeeper (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| Stability | 20,000-step command fuzz clean (measured on 2026-11-06 with `DBEE_FUZZ_STEPS=20000`; ci runs 300 steps); ~25 % (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| File formats (import/export) | 0 % (measured) | M3 | [file-format-parity](docs/file-format-parity.md) |
| Localization | 0 % (measured) | not estimated yet | [localization-parity](docs/localization-parity.md) |
| Platforms | Linux x86_64 built, packaged and run; the rest not built yet | — | [hardware-parity](docs/hardware-parity.md) |

Hours are not estimated yet: M1's and M2's agent hours were not logged, so there is nothing to calibrate on.

## Features

Breadth grades Beekeeper's observed feature list row by row: done 1, partial 0.5 (estimated). Ready is lower until dbee has been used on real databases.

| Area | Breadth | Ready | Hours | Doc |
|---|---|---|---|---|
| Connections | 62 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Navigation | 50 % | ~40 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Query | 32 % | ~25 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Results | 33 % | ~30 % | M3 | [target-app-parity](docs/target-app-parity.md) |
| Data | 59 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Structure | 0 % | 0 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Import/export | 0 % | 0 % | M3 | [file-format-parity](docs/file-format-parity.md) |
| Tools, App | 0 % | 0 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |

## Languages

| Language | UI | Docs |
|---|---|---|
| English | yes (the only one) | yes |

## Upcoming

1. The last check of M1 and M2: a day of real use of a Linux release build on the maintainer's own Postgres, MySQL, MariaDB and SQLite databases. Only the maintainer does this step; agents never connect to those databases.
2. M3 Import and export, for all four engines: file export and import, the last alpha gate row, and dump and restore with each engine's own tools.

## Progress log

| Date | Entry |
|---|---|
| 2026-11-08 | The larger review's bugs fixed: two dbee windows no longer lose each other's saved connections (each change re-reads the file under a lock); a newer dbee's connections file is read and never overwritten, and fields an older build does not know survive; a table tab's stopped load stops no other tab; the Postgres cancel and the MySQL KILL go to the server the connection reached; every wall-clock bound in a test is in an ignored timing twin. |
| 2026-11-06 | M2's whole-branch review fixed: a stopped statement's cancel is sent again until it ends and never reaches a later one; a stored password is not used with another CA file or an option that protects it less; the filter bar's `like` escapes with `\` on every engine; a file dropped while connected opens through the disconnect question, and one a saved connection names opens as saved (read-only kept); each engine's fixed facts in its own file. The engine differences the review found are in gaps.md. |
| 2026-11-05 | M2 "Three databases" built: one driver interface with Postgres behind it, and MySQL, MariaDB and SQLite with the whole M1 workflow: connect (a server also through SSH; a SQLite file from the command line or a drop), browse, filter, query, edit and copy. The Database field lists the server's databases (#1), and Ctrl+P finds saved connections and switches to one (#3). The M2 day runs end to end in the real window on all three. |
| 2026-10-28 | Milestones renumbered: M2 is "Three databases" (a driver interface, MySQL/MariaDB and SQLite), and import and export move to M3. SQLite's public-domain code is allowed. |
| 2026-10-27 | M1 review fixes: a value longer than a `varchar(n)`, `char(n)` or `bit(n)` column fails the Apply instead of being written cut; a running Apply can be cancelled until it commits; generated columns are refused; notices read as sentences; the UI tests share one harness, and ci fails a skipped driver test once the servers are up. |
| 2026-10-26 | M1 "Daily Postgres workflow" built: connect (SSH tunnel, TLS, read-only), browse and filter, foreign keys, staged edits with all-or-nothing Apply, the value panel, query tabs, copying. The M1 day runs end to end in the real window. Linux tarball and AppImage built for the day of real use. |
| 2026-10-19 | M1 "Daily Postgres workflow" started: licence check (`cargo xtask licenses`) and roadmap. |
| 2026-10-15 | Scaffold: rules, docs, tooling, packaging and release pipeline in place; placeholder app. |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-08 | minor | Progress log: the larger review's bugs. |
| 2026-11-06 | minor | Progress log: M2's review fixes; test numbers measured again. |
| 2026-11-06 | minor | The 20,000-step fuzz run again and dated. |
| 2026-11-05 | major | M2 built: stage line, headline numbers (parity, engines, commands, tests), dimensions, upcoming list and progress log. |
| 2026-10-29 | trivial | Stability and alpha gate: how the fuzz and `m1_day` were run. |
| 2026-10-28 | minor | M2 is "Three databases"; import and export are M3; upcoming list and progress log updated. |
| 2026-10-27 | trivial | The detail docs say what they hold, and link the docs index. |
| 2026-10-27 | minor | Wording: the day of real use on the maintainer's own databases; M1 review fixes. |
| 2026-10-27 | minor | Progress log: M1 review fixes. |
| 2026-10-26 | major | M1 built: stage line, headline numbers, dimensions and feature areas assessed (measured and estimated marked), upcoming list, progress log. |
| 2026-10-19 | minor | M1 started: next line, upcoming list and progress log. |
| 2026-10-15 | major | First version (scaffold). |

# dbee roadmap

**Stage: pre-alpha** · next: alpha, once file export lands (M2). M1 "Daily Postgres workflow" is built and waits for its last check: a day of real use on the maintainer's own databases.

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** minor (wording: the day of real use, M1 review fixes) · **Target:** Beekeeper Studio 5.5.7

Why pre-alpha: five of the six alpha gate rows work end to end, measured in the real window (`apps/dbee/tests/m1_day.rs`). They are connecting to Postgres (also through an SSH tunnel), browsing and filtering tables, running a query, editing a row and copying results. The sixth, export to a file, is M2. Postgres is the only engine. Detail: [`docs/target-app-parity.md`](docs/target-app-parity.md), [`docs/gaps.md`](docs/gaps.md), [`docs/roadmap.md`](docs/roadmap.md).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Overall parity with Beekeeper Studio | ~25 % | estimated (weighted dimensions, [target-app-parity](docs/target-app-parity.md)) |
| Alpha gate rows that pass | 5 of 6 | measured (`m1_day` and the test suites, [roadmap](docs/roadmap.md)) |
| Commands registered | 46: 43 engine, 3 UI-only | measured (the command tables) |
| Tests | 637 `#[test]` attributes in 42,079 lines | measured (`cargo xtask stats`, 2026-10-26) |

## By dimension

| Dimension | Parity | Remaining (h) | Doc |
|---|---|---|---|
| Features | ~35 % (estimated) | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Database engines | 1 of 20, Postgres (5 %, measured) | not estimated yet | [driver-parity](docs/driver-parity.md) |
| UI | ~45 % (estimated) | not estimated yet | [ui-parity](docs/ui-parity.md) |
| Performance | M1's targets met (measured); ~30 % against Beekeeper (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| Stability | 20,000-step command fuzz clean (measured); ~25 % (estimated) | — | [target-app-parity](docs/target-app-parity.md) |
| File formats (import/export) | 0 % (measured) | M2 | [file-format-parity](docs/file-format-parity.md) |
| Localization | 0 % (measured) | not estimated yet | [localization-parity](docs/localization-parity.md) |
| Platforms | Linux x86_64 built, packaged and run; the rest not built in M1 | — | [hardware-parity](docs/hardware-parity.md) |

Hours are not estimated yet: M1's agent hours were not logged, so there is nothing to calibrate on.

## Features

Breadth grades Beekeeper's observed feature list row by row: done 1, partial 0.5 (estimated). Ready is lower until dbee has been used on real databases.

| Area | Breadth | Ready | Hours | Doc |
|---|---|---|---|---|
| Connections | 62 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Navigation | 50 % | ~40 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Query | 32 % | ~25 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Results | 33 % | ~30 % | M2 | [target-app-parity](docs/target-app-parity.md) |
| Data | 59 % | ~45 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Structure | 0 % | 0 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Import/export | 0 % | 0 % | M2 | [file-format-parity](docs/file-format-parity.md) |
| Tools, App | 0 % | 0 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |

## Languages

| Language | UI | Docs |
|---|---|---|
| English | yes (the only one) | yes |

## Upcoming

1. M1's last check: a day of real use of a Linux release build on the maintainer's own databases. Only the maintainer does this step; agents never connect to those databases.
2. An architecture step: a database driver interface, so the engine no longer names Postgres types.
3. M2 Import and export: file export and import, the last alpha gate row.

## Progress log

| Date | Entry |
|---|---|
| 2026-10-27 | M1 review fixes: a value longer than a `varchar(n)`, `char(n)` or `bit(n)` column fails the Apply instead of being written cut; a running Apply can be cancelled until it commits; generated columns are refused; notices read as sentences; the UI tests share one harness, and ci fails a skipped driver test once the servers are up. |
| 2026-10-26 | M1 "Daily Postgres workflow" built: connect (SSH tunnel, TLS, read-only), browse and filter, foreign keys, staged edits with all-or-nothing Apply, the value panel, query tabs, copying. The M1 day runs end to end in the real window. Linux tarball and AppImage built for the day of real use. |
| 2026-10-19 | M1 "Daily Postgres workflow" started: licence check (`cargo xtask licenses`) and roadmap. |
| 2026-10-15 | Scaffold: rules, docs, tooling, packaging and release pipeline in place; placeholder app. |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-27 | minor | Wording: the day of real use on the maintainer's own databases; M1 review fixes. |
| 2026-10-27 | minor | Progress log: M1 review fixes. |
| 2026-10-26 | major | M1 built: stage line, headline numbers, dimensions and feature areas assessed (measured and estimated marked), upcoming list, progress log. |
| 2026-10-19 | minor | M1 started: next line, upcoming list and progress log. |
| 2026-10-15 | major | First version (scaffold). |

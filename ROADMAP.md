# dbee roadmap

**Stage: pre-alpha** · next: alpha (the first core workflow, connect to Postgres → browse → query, works end to end; M1 "Daily Postgres workflow" in progress)

> **Last reviewed:** 2026-10-19 · **Last updated:** 2026-10-19 · **Change:** minor (M1 started) · **Target:** Beekeeper Studio 5.5.7

Why pre-alpha: the repository holds the scaffold only (rules, docs, tooling, packaging, release
pipeline). No feature exists yet. Detail: [`docs/target-app-parity.md`](docs/target-app-parity.md),
[`docs/gaps.md`](docs/gaps.md), [`docs/roadmap.md`](docs/roadmap.md).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Overall parity with Beekeeper Studio | 0 % | measured |
| Commands registered | 0 | measured |
| Tests | see `cargo xtask stats` | measured |

## By dimension

| Dimension | Parity | Remaining (h) | Doc |
|---|---|---|---|
| Features | 0 % | not estimated yet | [target-app-parity](docs/target-app-parity.md) |
| Database engines | 0 % | not estimated yet | [driver-parity](docs/driver-parity.md) |
| UI | 0 % | not estimated yet | [ui-parity](docs/ui-parity.md) |
| Performance | not measured | — | [target-app-parity](docs/target-app-parity.md) |
| Stability | not measured | — | [target-app-parity](docs/target-app-parity.md) |
| File formats (import/export) | 0 % | not estimated yet | [file-format-parity](docs/file-format-parity.md) |
| Localization | 0 % | not estimated yet | [localization-parity](docs/localization-parity.md) |
| Platforms | Linux x86_64 built and launched locally; the rest unverified | — | [hardware-parity](docs/hardware-parity.md) |

## Features

| Area | Breadth | Ready | Hours | Doc |
|---|---|---|---|---|
| (filled from `plan/beekeeper/05-feature-catalog.md` at the first milestone) | | | | |

## Languages

| Language | UI | Docs |
|---|---|---|
| English | — | — |

## Upcoming

1. M1 Daily Postgres workflow (in progress): connect, browse and filter, query, edit a row, copy results. Defined in `docs/roadmap.md`.
2. M2 Import and export.

## Progress log

| Date | Entry |
|---|---|
| 2026-10-19 | M1 "Daily Postgres workflow" started: licence check (`cargo xtask licenses`) and roadmap. |
| 2026-10-15 | Scaffold: rules, docs, tooling, packaging and release pipeline in place; placeholder app. |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-19 | minor | M1 started: next line, upcoming list and progress log. |
| 2026-10-15 | major | First version (scaffold). |

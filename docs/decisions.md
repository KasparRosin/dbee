# Decisions

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

Significant decisions, newest last. Add an entry for every significant dependency, platform or process choice.

## 2026-10-15: Name `dbee`, app ID `dev.rosin.dbee`, homepage `https://dbee.rosin.dev`

- **Decision:** The product is `dbee` (lowercase everywhere), with app ID `dev.rosin.dbee` and homepage `https://dbee.rosin.dev`.
- **Why:** One stable lowercase name keeps binaries, package ids and the website consistent.

## 2026-10-15: Desktop only, no browser build

- **Decision:** dbee ships as a native desktop app. There is no web build and no wasm target.
- **Why:** Browsers cannot open database connections (raw TCP to PostgreSQL, MySQL or SQL Server).

## 2026-10-15: Full platform matrix

- **Decision:** Linux x86_64, aarch64 and riscv64; macOS universal; Windows x64, x86 and arm64; FreeBSD.
- **Why:** A database client is used on whatever machine the developer has, and a pure-Rust stack makes the wide matrix cheap.

## 2026-10-15: Signing is optional

- **Decision:** Builds ship unsigned until signing secrets exist; the workflows warn instead of failing.
- **Why:** Certificates cost money and need the owner's accounts; the pipeline must work before they exist.

## 2026-10-15: Self-contained workflows

- **Decision:** There is no shared tooling; third-party actions are pinned to commit SHAs.
- **Why:** The repository must build and release on its own, and it should be easy to audit.

## 2026-10-15: GitHub repository created when version 1 runs

- **Decision:** Until version 1 runs, there are local commits only; the repository `KasparRosin/dbee` is created then.
- **Why:** The pre-alpha scaffold has nothing to publish, and the history can be tidied before it is public.

## 2026-10-15: Rust 1.99 minimum

- **Decision:** `rust-version = "1.99"` in `[workspace.package]`, edition 2024.
- **Why:** A single recent toolchain keeps the code modern and the support matrix small.

## 2026-10-15: Licence MIT OR Apache-2.0

- **Decision:** dbee is dual-licensed `MIT OR Apache-2.0`.
- **Why:** The common permissive pair for Rust projects; contributors and users may pick either.

## 2026-10-15: Owner Kaspar Rosin, an independent project

- **Decision:** The owner and publisher is Kaspar Rosin. dbee is an independent project and not part of any app family.
- **Why:** One accountable owner for releases, signing and the licence line.

## 2026-10-15: Target app Beekeeper Studio 5.5.7, observed black-box only

- **Decision:** The parity target is Beekeeper Studio 5.5.7. It is only observed from outside, as a user would (clean room): its source, bundle, settings and traffic are never read.
- **Why:** dbee must be original work, and a black-box target keeps the licence of dbee clean.

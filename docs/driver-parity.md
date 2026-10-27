# Driver parity

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** trivial (the observation notes are cited as kept outside the repository) · **Target:** Beekeeper Studio 5.5.7

The engine list is the one observed in Beekeeper Studio's connection type list: 20 engines, 10 free and 10 paid ("*" after the name). The evidence is in observation notes kept outside the repository (they hold Beekeeper Studio screenshots, which are not ours to publish). "done" means it works end to end against the Docker test servers (measured); the limits are in [gaps.md](gaps.md).

| Engine | Beekeeper Studio | dbee connect | dbee browse | dbee query | dbee edit | Notes | Tests |
|---|---|---|---|---|---|---|---|
| PostgreSQL | free | done | done | done | done | 14 (the oldest supported) and the newest major (18) tested. Direct or through an SSH tunnel (key, agent or password; host keys checked against `known_hosts`); TLS modes with an optional CA file, verified with the OS store; read-only mode. Values shown exactly as Postgres prints them. Not supported: transaction-mode poolers, client certificates, SSH jump hosts and `~/.ssh/config` ([gaps.md](gaps.md) 2–4). Editing needs a primary key or a NOT NULL unique constraint. | `crates/pg/tests`, `crates/ssh/tests/tunnel.rs`, `crates/engine/tests`, `crates/ui-egui/tests`, `apps/dbee/tests/m1_day.rs` |
| MySQL | free | not started | not started | not started | not started | | |
| MariaDB | free | not started | not started | not started | not started | | |
| TiDB | free | not started | not started | not started | not started | | |
| SQLite | free | not started | not started | not started | not started | | |
| SQL Server | free | not started | not started | not started | not started | | |
| Amazon Redshift | free | not started | not started | not started | not started | | |
| CockroachDB | free | not started | not started | not started | not started | | |
| BigQuery | free | not started | not started | not started | not started | | |
| Redis | free (beta) | not started | not started | not started | not started | | |
| LibSQL | paid | not started | not started | not started | not started | | |
| Oracle | paid | not started | not started | not started | not started | | |
| Cassandra | paid | not started | not started | not started | not started | | |
| Firebird | paid | not started | not started | not started | not started | | |
| DuckDB | paid | not started | not started | not started | not started | | |
| ClickHouse | paid | not started | not started | not started | not started | | |
| MongoDB | paid | not started | not started | not started | not started | | |
| SQL Anywhere | paid | not started | not started | not started | not started | | |
| Trino | paid | not started | not started | not started | not started | | |
| SurrealDB | paid | not started | not started | not started | not started | | |

Engines: 1 of 20 (5 %, measured by count). Weighted by use the share is larger, as Postgres is among the most used, but no usage data backs a number.

## Work

- **Next:** a database driver interface, so the engine no longer names Postgres types (before M2). The second engine is added on it.
- **Estimates:** not estimated yet. M1's agent hours were not logged, so there is nothing to calibrate a per-engine figure on ([target-app-parity.md](target-app-parity.md), "Effort and calibration").

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-27 | trivial | The observation notes are cited as kept outside the repository, not by their local paths. |
| 2026-10-26 | major | PostgreSQL connect, browse, query and edit done (M1); the engine list checked against the observed app (TiDB, SQL Anywhere and SurrealDB added), with free and paid marked. |
| 2026-10-15 | major | Template (scaffold). |

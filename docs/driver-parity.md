# Driver parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** major (MySQL, MariaDB and SQLite done) · **Target:** Beekeeper Studio 5.5.7

The engine list is the one observed in Beekeeper Studio's connection type list: 20 engines, 10 free and 10 paid ("*" after the name). The evidence is in observation notes kept outside the repository (they hold Beekeeper Studio screenshots, which are not ours to publish). "done" means it works end to end against the Docker test servers (measured); the limits are in [gaps.md](gaps.md).

| Engine | Beekeeper Studio | dbee connect | dbee browse | dbee query | dbee edit | Notes | Tests |
|---|---|---|---|---|---|---|---|
| PostgreSQL | free | done | done | done | done | 14 (the oldest supported) and the newest major (18) tested. Direct or through an SSH tunnel (key, agent or password; host keys checked against `known_hosts`); TLS modes with an optional CA file, verified with the OS store; read-only mode. Values shown exactly as Postgres prints them. Not supported: transaction-mode poolers, client certificates, SSH jump hosts and `~/.ssh/config` ([gaps.md](gaps.md) 2–4). Editing needs a primary key or a NOT NULL unique constraint. | `crates/pg/tests`, `crates/ssh/tests/tunnel.rs`, `crates/engine/tests`, `crates/ui-egui/tests`, `apps/dbee/tests/m1_day.rs` |
| MySQL | free | done | done | done | done | 5.7 (the oldest supported), 8.0 and the newest (26.10) tested; an older server gets a warning, and dbee goes on. dbee's own client for MySQL's protocol. Direct or through an SSH tunnel; the TLS modes and verifier as for Postgres (a 5.7 server whose TLS offers no modern cipher, as its official image's does not, is reached through SSH, and the error says so); the login methods `mysql_native_password`, `caching_sha2_password` and `sha256_password`; read-only mode (`SET SESSION TRANSACTION READ ONLY`). Databases show as schemas; a connection that names one lists only it; the form lists the server's databases; a query tab's database chip (`USE`). Values as the server prints them, binary ones as `0x…`. Grid edits run under a strict `sql_mode`; a table whose storage engine has no transactions (MyISAM) is read-only in the grid. Not supported: a Unix socket ([gaps.md](gaps.md) 19), client certificates (3). | `crates/mysql/tests` (`connects_and_reports_version`, `wrong_password_is_auth_error`, `through_ssh_tunnel`, `read_only_refuses_writes`, `cancel_stops_sleep`, `killed_connection_reports_connection_lost`, `twenty_megabyte_cell_loads`, `one_failure_writes_nothing`, `myisam_table_cannot_be_edited`), `crates/engine/tests` (every engine: `hostile_filter_value_is_just_a_value`, `failed_apply_writes_nothing_and_marks_the_row`, `views_and_keyless_tables_are_read_only`, `editing_one_column_leaves_zero_date_unchanged`, `queries_and_their_cancel_go_through_the_tunnel`), `crates/ui-egui/tests`, `apps/dbee/tests/m2_day.rs` (8.0 through the SSH tunnel) |
| MariaDB | free | done | done | done | done | 10.11 (the oldest supported) and the newest (13.0) tested, on the MySQL driver (the server's version string tells them apart). As MySQL, and: its JSON columns (a `LONGTEXT` with a `json_valid` check) open in the JSON panel; its own types (`uuid`, `inet6`) show as text. | `crates/mysql/tests` (`mariadb_version_num_is_the_real_version`, `mariadb_json_is_json`, `a_mariadb_json_column_is_json`, `a_gate_cancels_an_apply_until_it_commits`), `crates/engine/tests` (every engine), `crates/ui-egui/tests`, `apps/dbee/tests/m2_day.rs` (the newest, directly) |
| TiDB | free | not started | not started | not started | not started | | |
| SQLite | free | done | done | done | done | The SQLite library compiled in (3.53). A file opened read-write or read-only, chosen in the form (Choose… or New file…), named on the command line (`dbee <file>`), dropped on the window or opened from the file manager; an unsaved file can be saved to the list. Schema `main` only; a table without a key is edited by its rowid; values by fixed rules (reals as SQLite prints them, blobs as `X'…'`); a 5 s busy timeout, so a locked file fails cleanly. Not supported: attached databases, extensions and encrypted files ([gaps.md](gaps.md) 20); a VACUUM by another program can move rowid keys (22). | `crates/sqlite/tests` (`opens_and_reports_version`, `missing_file_is_not_created`, `not_a_database_file_is_a_connect_error`, `read_only_refuses_writes`, `cancel_stops_a_long_query`, `apply_writes_all_or_nothing`, `apply_while_file_locked_fails_cleanly`, `a_table_without_a_key_is_keyed_by_a_hidden_rowid`), `crates/engine/tests` (every engine), `crates/ui-egui/tests`, `apps/dbee/tests/m2_day.rs` (opened with `dbee <file>`, then saved) |
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

Engines: 4 of 20 (20 %, measured by count): PostgreSQL, MySQL, MariaDB and SQLite. Weighted by use the share is larger, as these four are among the most used, but no usage data backs a number. CockroachDB and Amazon Redshift speak Postgres's protocol and may work through its driver, but are not tested.

## Work

- **Done (M2 "Three databases", 2026-11-05):** one driver interface (`crates/driver`), Postgres moved behind it, and MySQL/MariaDB and SQLite on it with the whole M1 workflow, in the real window on each (`apps/dbee/tests/m2_day.rs`).
- **Next:** none planned; M3 brings import and export to all four.
- **Estimates:** not estimated yet. M1's agent hours were not logged, so there is nothing to calibrate a per-engine figure on ([target-app-parity.md](target-app-parity.md), "Effort and calibration").

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-05 | major | MySQL, MariaDB and SQLite connect, browse, query and edit: done, with their tests; 4 of 20 engines (M2). |
| 2026-10-28 | minor | Work: the driver interface, MySQL/MariaDB and SQLite are M2 "Three databases". |
| 2026-10-27 | trivial | The observation notes are cited as kept outside the repository, not by their local paths. |
| 2026-10-26 | major | PostgreSQL connect, browse, query and edit done (M1); the engine list checked against the observed app (TiDB, SQL Anywhere and SurrealDB added), with free and paid marked. |
| 2026-10-15 | major | Template (scaffold). |

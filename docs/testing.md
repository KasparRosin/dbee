# Testing

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

```sh
cargo test --workspace                              # everything; oracle tests skip without their tool
cargo test -p dbee-engine                           # one crate
cargo xtask ci                                      # all gates (fmt, clippy, test, assets, layers, provenance)
DBEE_REQUIRE_ORACLES=1 cargo test --workspace       # CI: a missing oracle tool fails instead of skipping
cargo test --workspace -- --ignored                 # performance tests
```

The scaffold has no features, so most layers below are **(planned)**. They are the contract for tests as features land.

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules | identifier quoting, type mapping, value formatting, command parameter parsing |
| Property (`proptest`) **(planned)** | domain crates, engine | quote then unquote is the identity, undo/redo restores state, export then import round-trips |
| Scripted UI **(planned)** | `crates/ui-egui/tests/` | headless app driven over the control channel: open a connection, run a query, click by automation id |
| Golden images **(planned)** | `crates/ui-egui/tests/` | small screenshots of fixed states, compared with a tolerance |
| Oracle **(planned)** | driver and engine tests | query results checked against the `sqlite3` CLI (§2) |
| Driver **(planned)** | `crates/drivers` tests | PostgreSQL and MySQL against throwaway containers (§3) |
| Robustness / fuzz **(planned)** | engine, wire parsers | `every_command_survives_empty_params`, command-sequence fuzz, hostile wire input (§5) |
| Performance (`#[ignore]`) **(planned)** | engine, ui-egui | scrolling a million-row result, streaming a large export (§6) |

Test databases are built in code (a SQL script or the driver API run at the start of the test). Never commit database files or large binary fixtures.

## 2. Oracle tests: SQLite through the `sqlite3` CLI

For SQLite the oracle is the `sqlite3` command-line shell, an independent implementation:

- build the same database with both dbee and `sqlite3`, run the same query, compare rows, column names and declared types;
- compare schema introspection with `sqlite3`'s `.schema` and `PRAGMA table_info`;
- compare exports (CSV, JSON, SQL) by loading dbee's output back into `sqlite3`.

Oracle tests skip with a printed `SKIPPED` when `sqlite3` is not on `PATH`, unless `DBEE_REQUIRE_ORACLES=1` is set (CI sets it). State the pass criterion in the test: results are exact, and any tolerance (floating point text) is named.

## 3. Driver tests: throwaway containers

PostgreSQL and MySQL tests run against containers started by the test or by `cargo xtask`:

- the container is local, created for the run and removed afterwards, with a random port and a generated password;
- **never a real server**, and never a connection string from the environment that points elsewhere;
- tests skip when no container runtime is available, unless `DBEE_REQUIRE_ORACLES=1`.

## 4. UI: control channel and screenshots (planned)

- **Scripted tests** run the app headlessly (egui_kittest) and drive it only through the control channel and automation ids, asserting on `ui.inspect` and `document.inspect`.
- **Golden images** are procedural: a fixed in-code database, a fixed window size, one state per image. Update a golden image only after looking at the new picture.
- **Interactive checks** run the real app with `--control`, drive it and take `ui.screenshot`. Do this for every UI change (see [`agents.md`](agents.md) §4).

## 5. Robustness and fuzz

Follow [never crash](standards/never-crash.md). Beyond a regression test per fixed crash:

- `every_command_survives_empty_params` fuzzes every registered command **(planned)**;
- a command-sequence fuzz test applies random commands and checks undo/redo invariants **(planned)**;
- wire parsers (PostgreSQL, MySQL, TDS) take truncated, oversized and random bytes and return errors, never panic **(planned)**.

## 6. Performance

Performance tests are `#[ignore]` so `cargo test` stays fast; run them with `-- --ignored`. Each states its budget (for example time to first row, rows per second, peak memory) and prints the machine, the load average and the measured value. A claim of improvement needs before/after numbers on the same build.

## 7. Corpora

Real database files and large dumps live **outside the repository**. They are fetched from a pinned URL and verified against a pinned sha256 before use; a mismatch fails the test. Tests that need a corpus skip when it is absent unless `DBEE_REQUIRE_ORACLES=1`. Never commit them.

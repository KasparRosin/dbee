# Never crash

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-20 · **Change:** minor (the guard and the fuzz tests exist)

People trust dbee with their databases. A crash can lose an unsaved query or leave a transaction open. This outranks feature work. The short form is in [`AGENTS.md`](../../AGENTS.md); this page is the standard behind it.

## Rules

- **No panics in non-test code** (`unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`) and no `unsafe`. Why: each one turns a recoverable condition into a process abort.
- **Errors are `Result<T, E>`** through the crate's error type and `?`. Why: the caller decides what to show, and the session stays alive.
- **Input-derived numbers are hostile.** Use `get()` instead of indexing, checked or saturating arithmetic, and cap every allocation sized by input (huge result sets, wide rows, large BLOBs). Why: a server, file or agent can send any value, and an overflow or out-of-range index is a panic.
- **Bound recursion** (nested JSON values, recursive schema relationships, nested queries). Why: unbounded depth overflows the stack, which no handler can catch.
- **Last-resort guard:** `Session::execute` snapshots the session state, runs the command under `catch_unwind`, and on a panic restores the snapshot and returns an `internal` error; jobs the command started are orphaned (they run to their end and their results are dropped). `Session::pump` guards each job result the same way, and a panic inside a job's future becomes an `internal` result. Why: it is a safety net for the bug nobody foresaw, not a licence to panic. It relies on unwinding (no profile sets `panic = "abort"`).
- **Prove it:** every crash fix lands with a regression test. Why: a crash that came back once will come back again.

## Enforcement

- Every crate root carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.
- The workspace sets `unsafe_code = "forbid"` under `[workspace.lints.rust]`.
- `clippy.toml` allows `unwrap`, `expect`, `panic` and indexing inside tests only.
- `cargo xtask ci` (fmt, clippy with `-D warnings`, tests, assets, tokens, layers, provenance) must pass before every commit.

## Hostile input checklist

Before merging code that reads from outside the process, ask what happens with:

- lengths and counts read from the wire (a packet claiming 4 GiB, a column count of 65535);
- result-set sizes (millions of rows, thousands of columns) and what is buffered at once;
- BLOB and long text sizes;
- deeply nested JSON (control channel, MCP params, JSON columns);
- identifiers containing quotes, NUL bytes, emoji or combining characters;
- non-UTF-8 text from servers, and non-UTF-8 command-line arguments;
- an empty parameter object, a missing key, a wrong type.

## Proving a fix

1. Write the regression test first and watch it fail. Name it after the input (`handles_nul_in_table_name`, `rejects_row_count_larger_than_buffer`).
2. Fix the code.
3. Extend the fuzz tests in `crates/engine`:
   - `every_command_survives_empty_params` (`src/tests.rs`) runs every command with empty params.
   - `tests/command_sequence_fuzz.rs` runs random commands with random, often hostile, params (number edges, quotes, NUL, emoji, 10 kB strings, nulls, arrays, 64-deep nesting). After every step no result is `internal`, `inspect()` serializes, the active tab is an open one and no journal entry keeps a secret. A new command adds its param shapes to `PARAM_HINTS` there (a test fails without them). `DBEE_FUZZ_SEED` replays a run (the seed is printed) and `DBEE_FUZZ_STEPS` sets its length (300 by default).
   - `undoing_every_edit_then_redoing_restores_them` (`crates/engine/tests/command_sequence_fuzz.rs`) checks the undo/redo invariant on the test server.

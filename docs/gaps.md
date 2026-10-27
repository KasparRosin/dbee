# Gaps

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** minor (entry 15: what is left after values that do not fit a column's length are refused) · **Target:** Beekeeper Studio 5.5.7

Ranked by user impact; pick from the top. Each entry says what is missing or limited, the evidence a reviewer can rerun, who it hurts and an estimate. Hours are agent wall-clock hours, **estimated**. Timings are **measured**, in a dev build (the workspace's dev profile: dbee's crates at opt-level 1, dependencies at 2). They are quoted from [`target-app-parity.md`](target-app-parity.md), "Performance", which holds every measurement with its machine, runs and load averages.

Kinds: F feature, U UI, D database engine, FF file format, H hardware, L localization, P performance, S stability.

## Alpha blockers

### 1. No export or import of files (FF)

- **Missing:** exporting a table or a query result to a file (CSV, JSON, SQL) and importing one. The alpha gate's last row ([`roadmap.md`](roadmap.md)).
- **Evidence:** `engine.commands` lists no export or import command; the way out today is the clipboard, at most 8 MiB of text (entry 9).
- **Impact:** results larger than a copy, or meant for a file, cannot leave dbee. Kaspar switches fully once this lands (M2).
- **Estimate:** M2 "Import and export"; not estimated yet.

## After alpha

### 2. SSH: no jump hosts, no `~/.ssh/config`, no second factor (D)

- **Missing:** an SSH jump host (bastion), reading `Host` aliases, `ProxyJump` and identity settings from `~/.ssh/config`, and keyboard-interactive login (one-time codes).
- **Evidence:** `SshConfig` (`crates/config/src/connection.rs`) holds one host. `crates/ssh/src/lib.rs` logs in only with a key, the agent or a password (`authenticate_publickey`, `authenticate_publickey_with`, `authenticate_password`). Nothing reads `~/.ssh/config`.
- **Impact:** a server behind a bastion, or one that asks for a code, cannot be reached; aliases must be typed out by hand. Kaspar's own setup (one hop, his key) works.
- **Estimate:** 6–10 h (estimated).

### 3. No TLS client certificates (D)

- **Missing:** `sslcert`/`sslkey`: logging in to Postgres with a client certificate.
- **Evidence:** `crates/pg/src/tls.rs` builds its rustls config `with_no_client_auth()`. The connection form has a CA file only.
- **Impact:** servers that require certificate login (some managed Postgres) refuse the connection.
- **Estimate:** 3–5 h (estimated).

### 4. No transaction-mode connection poolers (D)

- **Missing:** support for a pooler in transaction mode (PgBouncer `pool_mode=transaction`, pooled ports of hosted Postgres).
- **Evidence:** out of M1's scope. dbee quotes values into SQL text, which is safe only with `standard_conforming_strings = on`. The connect sets it (and `default_transaction_read_only = on` for a read-only connection) and confirms both with a separate `SHOW` (`crates/pg/src/connect.rs`). A pooler that runs the `SHOW` in another server session can fail that check, and then the connect is refused ("read-only mode did not stick …; a transaction-mode connection pooler is not supported", or the `standard_conforming_strings` message). Later statements would still assume the same session.
- **Impact:** connect to the server's direct or session-mode port instead.
- **Estimate:** not estimated (needs a design for per-statement session state).

### 5. Wide results are slow to draw (P)

- **Limit:** a frame's cost grows with the number of columns in the result, not with the columns in view: roughly 0.1–0.2 ms a column in a dev build, up to 0.5 ms at 1,664 columns. The grid is shared by table and query tabs. Measured with `wide_result_frames` (`crates/ui-egui/tests/query_tab.rs`): the whole window without an AccessKit tree, as the app runs, over 10,000 rows; medians of NULL and short-text cells over five runs ([`target-app-parity.md`](target-app-parity.md), "Performance", with the loads):

  | Columns | Frame, nothing changing | Frame while scrolling |
  |---|---|---|
  | 50 | 6.5–13.9 ms | 8.2–17.3 ms |
  | 100 | 10.7–27.1 ms | 15.4–39.8 ms |
  | 300 | 38–67 ms | 50–97 ms |
  | 1,664 (Postgres's most) | 231–897 ms | 357–778 ms |

  kittest, which always builds an AccessKit tree, measured 1,664 columns at 376–792 ms a frame (340–890 ms in M1.27).
- **Impact:** at about 100 columns a frame no longer fits 60 Hz (in four of the five runs). At 300 columns scrolling stutters, and at 1,664 the window draws one to four frames a second. `select *` over a wide join is a common case.
- **Estimate:** 4–8 h (estimated): lay out and draw only the columns in view.

### 6. One connection per window (F)

- **Missing:** several connections at once, or several windows.
- **Evidence:** the session holds one `connected: Option<Connected>` (`crates/engine/src/state.rs`), and `connection.open` is disabled while connected (a later milestone).
- **Impact:** comparing two databases means disconnecting and reconnecting. Tabs and staged edits belong to the one connection.
- **Estimate:** not estimated (an engine change: sessions per connection).

### 7. Query results are capped at 10,000 rows (F)

- **Limit:** a query keeps at most 10,000 rows per result and 256 MiB of cell text per run, and says `First 10,000 of N rows`.
- **Evidence:** `QUERY_LIMITS` (`crates/pg/src/query.rs`); `a_result_cut_at_the_cap_says_so` (`crates/ui-egui/tests/query_tab.rs`).
- **Impact:** a larger result cannot be read through in the query tab. A table tab pages through any number of rows (100 a page; the next page of a million-row table loads in 1.06–1.09 ms, [`target-app-parity.md`](target-app-parity.md), "Performance"), and file export comes in M2.
- **Estimate:** part of M2's export; paging query results is not estimated.

### 8. Generated columns are offered for editing (S)

- **Problem:** a stored generated column, or an identity column `GENERATED ALWAYS`, can be edited in the grid. Postgres refuses the write at Apply ("column … can only be updated to DEFAULT"). Nothing is written, as Apply is all or nothing, and the error card names the column, but the edit should not be offered.
- **Evidence:** the catalog query (`COLUMNS_SQL`, `crates/pg/src/catalog.rs`) does not read `attgenerated`, and only treats `attidentity` as "has a default". `cmd::edit::can_edit` has no per-column rule. On pg14 (2026-10-26), an `UPDATE` of a generated column and of an identity `GENERATED ALWAYS` column both fail with SQLSTATE 428C9.
- **Impact:** a confusing failed Apply; no data is lost.
- **Estimate:** 2–3 h (estimated): read `attgenerated` and `attidentity`, and refuse those cells with a reason, as read-only cells are.

### 9. A copy is at most 8 MiB of text (F)

- **Limit:** one copy (Ctrl+C, Ctrl+Shift+C, the grid menu's formats) holds at most 8 MiB of text, counting everything (quotes, escapes, `NULL`, column names). A larger one copies nothing and says how many rows fit.
- **Evidence:** `COPY_MAX_BYTES` (`crates/engine/src/cmd/selection.rs`); `a_copy_over_the_cap_copies_nothing_and_says_why` (`crates/ui-egui/tests/clipboard.rs`).
- **Impact:** large selections need several copies until file export lands in M2 (entry 1).
- **Estimate:** M2.

### 10. Unique indexes without a constraint are not row keys (F)

- **Limit:** a table without a primary key can still be edited through a unique constraint on NOT NULL columns. A unique index made with `CREATE UNIQUE INDEX` is not seen, so such a table is read-only in the grid, and the note above it says why.
- **Evidence:** keys come from `pg_constraint` only (`contype` `p`, `u`, `f`: `CONSTRAINTS_SQL`, `crates/pg/src/catalog.rs`; `TableInfo::row_key`, `crates/core/src/schema.rs`). On pg14 (2026-10-26), a unique index alone leaves `pg_constraint` empty for its table.
- **Impact:** such tables are changed through SQL in a query tab.
- **Estimate:** 2–3 h (estimated): read `pg_index` (`indisunique`, no expressions, no predicate).

### 11. Inline editing stops at 32 KiB (U)

- **Limit:** a cell editor opens only for values up to 32 KiB, and a paste that would make a value longer is dropped; a notice says so. The value panel shows larger values read-only, so they are changed through SQL.
- **Evidence:** `MAX_INLINE_BYTES` (`crates/ui-egui/src/editors.rs`); `a_value_too_large_to_edit_inline_says_so`, `a_paste_that_would_pass_the_cap_is_dropped_and_says_so` (`crates/ui-egui/tests/editing.rs`). Measured: a keystroke in an editor holding 32 KiB on one line takes an 18.8–24.9 ms frame single-line and 26.1–29.5 ms multi-line, within the 33 ms budget (`keystroke_frame_with_a_value_at_the_cap`; [`target-app-parity.md`](target-app-parity.md), "Performance").
- **Impact:** large JSON documents or long texts cannot be fixed in the grid.
- **Estimate:** 6–10 h (estimated): an editor that lays out only the lines in view, as the value panel does.

### 12. The Flatpak cannot reach `~/.ssh`, the SSH agent or the keyring (H)

- **Missing:** Flatpak permissions for the user's SSH keys and `known_hosts`, the agent socket and the secret service.
- **Evidence:** `packaging/linux/flatpak/dev.rosin.dbee.yml` grants no home access, no `--socket=ssh-auth` and no secret-service bus name (left for later in M1).
- **Impact:** in the Flatpak, SSH tunnels with the user's key and saved passwords do not work. The tarball and the AppImage are expected to (not measured: key logins through the tunnel were exercised in the dev build only, by `m1_day` and `crates/ssh/tests/tunnel.rs`).
- **Estimate:** 2–4 h (estimated), plus a check on a Flatpak host.

### 13. One very long line in the query editor is slow to type in (P)

- **Limit:** egui lays out a paragraph (the text between line breaks) whole on every change, so typing in one long line costs time in proportion to its length.
- **Evidence:** `keystroke_frames_in_big_scripts` (`crates/ui-egui/tests/query_tab.rs`), medians over two runs ([`target-app-parity.md`](target-app-parity.md), "Performance", with the loads). For the editor alone:

  | Text | Keystroke frame |
  |---|---|
  | 1 MiB on one line | 582–673 ms (slowest 2.0 s); 0.6–1.6 s in M1.26's runs |
  | 256 KiB on one line | 143–144 ms |
  | 32 KiB on one line | 18.3–32.6 ms |
  | 1 MiB of short lines, the whole window | 28.7–46.4 ms |
- **Impact:** pasting minified SQL or JSON of hundreds of kilobytes on one line makes typing lag. Ordinary scripts are not affected.
- **Estimate:** 8–16 h (estimated): lay out a long line in pieces.

### 14. The value panel's scroll position loses precision past about a million rows (U)

- **Limit:** positions are f32, as egui's scrolling is, so past about 18 million points (a million rows of 18 points) a row's place is off by a point or more.
- **Evidence:** the module docs of `crates/ui-egui/src/value_panel.rs`. A value gets there only with more than about a million rows (lines, and long lines wrapped) within the 10 MiB the panel shows, so lines of about 10 bytes or fewer on average. 10 MiB of line breaks is about 10 million rows (180 million points), where f32 steps by 16 points.
- **Impact:** small: few real values have a million short lines. In the far end of such a value, rows can sit several points off and a click can land one row away.
- **Estimate:** 3–6 h (estimated): scroll by row, not by point.

### 15. Filters on `char(n)` and `bit(n)` skip their index; array filters still cut (D, P)

- **Limit:** a value written or filtered on is cast to the column's type without its length (`ColumnInfo::write_type_sql`, `crates/core/src/schema.rs`), because an explicit cast to `varchar(n)`, `char(n)` or `bit(n)` cuts (or pads) the value without an error. So an edit that does not fit fails at Apply (SQLSTATE 22001 or 22026) and a filter value that does not fit matches nothing. Two cases are left. A filter on a `char(n)` column compares as `text` and one on `bit(n)` as `bit varying`, which an index on the column does not serve, so such a filter reads the whole table. And a filter on an array of `varchar(n)`, `char(n)` or `bit(n)` keeps the column's own type, since Postgres has no operator for `varchar[] = text[]`, `char[] = text[]` or `bit[] = bit varying[]`, so an element longer than the length is still cut and can match the rows of its prefix.
- **Evidence:** `values_that_do_not_fit_the_length_are_refused_not_cut` (`crates/engine/tests/edit.rs`, both test servers); `array_filters_that_no_operator_takes_without_the_length_keep_their_type` (`crates/sql/tests/builders.rs`). On pg14 and Postgres 18 (2026-10-27), `EXPLAIN` of `c = CAST('ab' AS text)` on an indexed `char(2)` column is a sequential scan, and `c = CAST('ab' AS bpchar)` an index scan; `CAST('…' AS bpchar[])`, `CAST('…' AS "bit"[])` and `CAST('…' AS character varying[])` (no length) compare with those arrays and cut nothing.
- **Impact:** a filter on an indexed `char(n)` or `bit(n)` column of a large table is slow; a filter on an array with an over-long element can match rows it should not. Edits are not affected.
- **Estimate:** 1–2 h (estimated): cast to the types without a length that keep the column's operators (`bpchar`, `"bit"`, `character varying[]`, `bpchar[]`, `"bit"[]`).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-27 | minor | Entry 15: values that do not fit a column's length are refused, not cut; what is left (filters on `char(n)` and `bit(n)` skip their index, array filters still cut). |
| 2026-10-27 | minor | Timings quoted from target-app-parity.md, the one source; the next page 1.06–1.09 ms; a fifth wide-result run; the tarball's and AppImage's SSH key logins marked expected, not measured. |
| 2026-10-26 | major | M1 built: 14 known limits with evidence, ranked; wide results and the long-line editor measured. |
| 2026-10-15 | major | Template (scaffold). |

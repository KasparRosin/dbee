# Gaps

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** minor (entry 17: Finder cannot open a database file in dbee) · **Target:** Beekeeper Studio 5.5.7

Ranked by user impact; pick from the top. Each entry says what is missing or limited, the evidence a reviewer can rerun, who it hurts and an estimate. Hours are agent wall-clock hours, **estimated**. Timings are **measured**, in a dev build (the workspace's dev profile: dbee's crates at opt-level 1, dependencies at 2). They are quoted from [`target-app-parity.md`](target-app-parity.md), "Performance", which holds every measurement with its machine, runs and load averages.

Kinds: F feature, U UI, D database engine, FF file format, H hardware, L localization, P performance, S stability.

## Alpha blockers

### 1. No export or import of files (FF)

- **Missing:** exporting a table or a query result to a file (CSV, JSON, SQL) and importing one. The alpha gate's last row ([`roadmap.md`](roadmap.md)).
- **Evidence:** `engine.commands` lists no export or import command; the way out today is the clipboard, at most 8 MiB of text (entry 8).
- **Impact:** results larger than a copy, or meant for a file, cannot leave dbee. The maintainer switches to dbee fully once this lands (M3).
- **Estimate:** M3 "Import and export"; not estimated yet.

## After alpha

### 2. SSH: no jump hosts, no `~/.ssh/config`, no second factor (D)

- **Missing:** an SSH jump host (bastion), reading `Host` aliases, `ProxyJump` and identity settings from `~/.ssh/config`, and keyboard-interactive login (one-time codes).
- **Evidence:** `SshConfig` (`crates/config/src/connection.rs`) holds one host. `crates/ssh/src/lib.rs` logs in only with a key, the agent or a password (`authenticate_publickey`, `authenticate_publickey_with`, `authenticate_password`). Nothing reads `~/.ssh/config`.
- **Impact:** a server behind a bastion, or one that asks for a code, cannot be reached; aliases must be typed out by hand. One hop with a key works.
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

  kittest, which always builds an AccessKit tree, measured 1,664 columns at 376–792 ms a frame (340–890 ms in earlier runs).
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
- **Impact:** a larger result cannot be read through in the query tab. A table tab pages through any number of rows (100 a page; the next page of a million-row table loads in 1.06–1.09 ms, [`target-app-parity.md`](target-app-parity.md), "Performance"), and file export comes in M3.
- **Estimate:** part of M3's export; paging query results is not estimated.

### 8. A copy is at most 8 MiB of text (F)

- **Limit:** one copy (Ctrl+C, Ctrl+Shift+C, the grid menu's formats) holds at most 8 MiB of text, counting everything (quotes, escapes, `NULL`, column names). A larger one copies nothing and says how many rows fit.
- **Evidence:** `COPY_MAX_BYTES` (`crates/engine/src/cmd/selection.rs`); `a_copy_over_the_cap_copies_nothing_and_says_why` (`crates/ui-egui/tests/clipboard.rs`).
- **Impact:** large selections need several copies until file export lands in M3 (entry 1).
- **Estimate:** M3.

### 9. Unique indexes without a constraint are not row keys (F)

- **Limit:** a table without a primary key can still be edited through a unique constraint on NOT NULL columns. A unique index made with `CREATE UNIQUE INDEX` is not seen, so such a table is read-only in the grid, and the note above it says why.
- **Evidence:** keys come from `pg_constraint` only (`contype` `p`, `u`, `f`: `CONSTRAINTS_SQL`, `crates/pg/src/catalog.rs`; `TableInfo::row_key`, `crates/core/src/schema.rs`). On pg14 (2026-10-26), a unique index alone leaves `pg_constraint` empty for its table.
- **Impact:** such tables are changed through SQL in a query tab.
- **Estimate:** 2–3 h (estimated): read `pg_index` (`indisunique`, no expressions, no predicate).

### 10. Inline editing stops at 32 KiB (U)

- **Limit:** a cell editor opens only for values up to 32 KiB, and a paste that would make a value longer is dropped; a notice says so. The value panel shows larger values read-only, so they are changed through SQL.
- **Evidence:** `MAX_INLINE_BYTES` (`crates/ui-egui/src/editors.rs`); `a_value_too_large_to_edit_inline_says_so`, `a_paste_that_would_pass_the_cap_is_dropped_and_says_so` (`crates/ui-egui/tests/editing.rs`). Measured: a keystroke in an editor holding 32 KiB on one line takes an 18.8–24.9 ms frame single-line and 26.1–29.5 ms multi-line, within the 33 ms budget (`keystroke_frame_with_a_value_at_the_cap`; [`target-app-parity.md`](target-app-parity.md), "Performance").
- **Impact:** large JSON documents or long texts cannot be fixed in the grid.
- **Estimate:** 6–10 h (estimated): an editor that lays out only the lines in view, as the value panel does.

### 11. The Flatpak cannot reach `~/.ssh`, the SSH agent or the keyring (H)

- **Missing:** Flatpak permissions for the user's SSH keys and `known_hosts`, the agent socket and the secret service.
- **Evidence:** `packaging/linux/flatpak/dev.rosin.dbee.yml` grants no home access, no `--socket=ssh-auth` and no secret-service bus name (left for later in M1).
- **Impact:** in the Flatpak, SSH tunnels with the user's key and saved passwords do not work. The tarball and the AppImage are expected to (not measured: key logins through the tunnel were exercised in the dev build only, by `m1_day` and `crates/ssh/tests/tunnel.rs`).
- **Estimate:** 2–4 h (estimated), plus a check on a Flatpak host.

### 12. One very long line in the query editor is slow to type in (P)

- **Limit:** egui lays out a paragraph (the text between line breaks) whole on every change, so typing in one long line costs time in proportion to its length.
- **Evidence:** `keystroke_frames_in_big_scripts` (`crates/ui-egui/tests/query_tab.rs`), medians over two runs ([`target-app-parity.md`](target-app-parity.md), "Performance", with the loads). For the editor alone:

  | Text | Keystroke frame |
  |---|---|
  | 1 MiB on one line | 582–673 ms (slowest 2.0 s); 0.6–1.6 s in earlier runs |
  | 256 KiB on one line | 143–144 ms |
  | 32 KiB on one line | 18.3–32.6 ms |
  | 1 MiB of short lines, the whole window | 28.7–46.4 ms |
- **Impact:** pasting minified SQL or JSON of hundreds of kilobytes on one line makes typing lag. Ordinary scripts are not affected.
- **Estimate:** 8–16 h (estimated): lay out a long line in pieces.

### 13. The value panel's scroll position loses precision past about a million rows (U)

- **Limit:** positions are f32, as egui's scrolling is, so past about 18 million points (a million rows of 18 points) a row's place is off by a point or more.
- **Evidence:** the module docs of `crates/ui-egui/src/value_panel.rs`. A value gets there only with more than about a million rows (lines, and long lines wrapped) within the 10 MiB the panel shows, so lines of about 10 bytes or fewer on average. 10 MiB of line breaks is about 10 million rows (180 million points), where f32 steps by 16 points.
- **Impact:** small: few real values have a million short lines. In the far end of such a value, rows can sit several points off and a click can land one row away.
- **Estimate:** 3–6 h (estimated): scroll by row, not by point.

### 14. A failed statement leaves a query tab's transaction failed (D)

- **Limit:** a script that opens a transaction (`BEGIN`) and fails inside it leaves the query connection in that failed transaction: every later run fails with SQLSTATE 25P02 until a `ROLLBACK`. dbee does not end it, since it cannot know whether the user means to roll back to a savepoint instead. The error card says so: under a 25P02 error it adds that a ROLLBACK ends the failed transaction.
- **Evidence:** `a_failed_transaction_says_a_rollback_ends_it` (`crates/ui-egui/tests/query_tab.rs`); `ROLLBACK_NOTE` (`crates/ui-egui/src/query_tab.rs`).
- **Impact:** one more run (`ROLLBACK`) after a failed transaction. A reconnect also ends it.
- **Estimate:** 2–4 h (estimated): show the open or failed transaction in the query tab and offer Rollback.

### 15. A table without a row key pages by OFFSET alone (D)

- **Limit:** a table with neither a primary key nor a NOT NULL unique key pages with `LIMIT 101 OFFSET n` and no `ORDER BY`, so Postgres may return its rows in another order from one page to the next. Under concurrent writes (or a plan change) a page can repeat rows of another or skip some. Ordering by `ctid` would fix the order, but it sorts the whole table for every page, which a large keyless table cannot afford.
- **Evidence:** `page_sql` (`crates/sql/src/select.rs`) orders by the row key only; `page_details` (`crates/sql/tests/builders.rs`) checks that a keyless table's page has no `ORDER BY`.
- **Impact:** browsing a keyless table that changes meanwhile can show a row twice or miss one; such tables are read-only in the grid (no row to name in an `UPDATE`).
- **Estimate:** 3–5 h (estimated): page a keyless table by `ctid` ranges (a `tid` cursor), which needs no sort.

### 16. A filter row cannot match an empty string (F)

- **Limit:** a filter row whose operator takes a value and that has none is left out of the condition, so `status = ''` cannot be written as a filter row. The bar mutes such a row, its field says `No value: left out`, and the tab title does not count it. A raw `WHERE` (`status = ''`) expresses it.
- **Evidence:** `Filter::restricts` (`crates/sql/src/filter.rs`); `a_filter_without_a_value_is_muted_left_out_and_not_counted` (`crates/ui-egui/tests/table_tab.rs`).
- **Impact:** small: rows with an empty text are found through the raw condition.
- **Estimate:** 1–2 h (estimated): an "is empty" operator, or a way to mark an empty value as meant.

### 17. macOS: Finder can't open a database file in dbee (H)

- **Missing:** Finder's Open With, and a file dropped on dbee's Dock icon. macOS hands the app such a file through the open-documents event (`application:openURLs:`), not its command line, and winit, the window library under eframe, does not pass that event on. So the app bundle declares no document types: Finder would offer an Open With that starts dbee without the file.
- **Evidence:** winit 0.30.13's application delegate (`src/platform_impl/macos/app_state.rs`) answers only `applicationDidFinishLaunching:` and `applicationWillTerminate:`, and its docs (`src/platform/macos.rs`) leave `application:openURLs:` to a delegate of the app's own, which takes Objective-C class code with `unsafe` (dbee forbids it). eframe 0.36 and egui-winit 0.36 have no hook for it either. `packaging/macos/Info.plist.in` has no `CFBundleDocumentTypes`, which `dbee_is_offered_for_database_files_but_takes_no_type_over` (`apps/dbee/src/main.rs`) checks.
- **Impact:** on macOS a database file opens by dropping it on dbee's window or by naming it on the command line (`dbee <file>`, or `open -n -a dbee --args <file>`). On Linux and Windows the file manager's Open With works.
- **Estimate:** 2–4 h (estimated) once winit or eframe passes the event on; more if dbee has to carry the delegate itself.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-05 | minor | Entry 17: on macOS, Finder's Open With cannot hand dbee a database file (winit does not pass the open-documents event on), so the bundle declares no document types. |
| 2026-10-28 | minor | Import and export is now M3 (M2 is three databases). |
| 2026-10-27 | minor | Wording: no personal names or task ids. |
| 2026-10-27 | minor | Entry 14 fixed and dropped: filters compare `char(n)` as `bpchar`, `bit(n)` as `"bit"` and arrays of `varchar(n)`, `char(n)` and `bit(n)` as `character varying[]`, `bpchar[]` and `"bit"[]`, so they use the column's index and an over-long element matches nothing; 15–17 are now 14–16. |
| 2026-10-27 | minor | M1 review fixes: generated and identity `GENERATED ALWAYS` columns refused (old entry 8 dropped, 9–15 now 8–14); entries 15 (a failed transaction in a query tab), 16 (keyless paging), 17 (empty filter values). |
| 2026-10-27 | minor | Entry 15 (now 14): values that do not fit a column's length are refused, not cut; what is left (filters on `char(n)` and `bit(n)` skip their index, array filters still cut). |
| 2026-10-27 | minor | Timings quoted from target-app-parity.md, the one source; the next page 1.06–1.09 ms; a fifth wide-result run; the tarball's and AppImage's SSH key logins marked expected, not measured. |
| 2026-10-26 | major | M1 built: 14 known limits with evidence, ranked; wide results and the long-line editor measured. |
| 2026-10-15 | major | Template (scaffold). |

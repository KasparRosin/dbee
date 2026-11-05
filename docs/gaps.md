# Gaps

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** minor (entries 19–24: MySQL and SQLite limits, Ctrl+P, the cramped form; older entries say which engines they hold for) · **Target:** Beekeeper Studio 5.5.7

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

- **Missing:** logging in with a client certificate (`sslcert`/`sslkey` on Postgres, `ssl-cert`/`ssl-key` on MySQL and MariaDB).
- **Evidence:** `crates/pg/src/tls.rs` and the TLS setup the other drivers share, `crates/driver/src/tls.rs`, build their rustls configs `with_no_client_auth()`. The connection form has a CA file only.
- **Impact:** servers that require certificate login (some managed Postgres and MySQL) refuse the connection.
- **Estimate:** 3–5 h (estimated).

### 4. Postgres: no transaction-mode connection poolers (D)

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
- **Impact:** comparing two databases means switching between them (Ctrl+P finds a saved connection and switches to it, asking first about query text and staged edits). Tabs and staged edits belong to the one connection.
- **Estimate:** not estimated (an engine change: sessions per connection).

### 7. Query results are capped at 10,000 rows (F)

- **Limit:** a query keeps at most 10,000 rows per result and 256 MiB of cell text per run, and says `First 10,000 of N rows`.
- **Evidence:** `QUERY_LIMITS` (`crates/driver/src/api.rs`), the same on every engine; `a_result_cut_at_the_cap_says_so` (`crates/ui-egui/tests/query_tab.rs`).
- **Impact:** a larger result cannot be read through in the query tab. A table tab pages through any number of rows (100 a page; the next page of a million-row table loads in 1.06–1.09 ms, [`target-app-parity.md`](target-app-parity.md), "Performance"), and file export comes in M3.
- **Estimate:** part of M3's export; paging query results is not estimated.

### 8. A copy is at most 8 MiB of text (F)

- **Limit:** one copy (Ctrl+C, Ctrl+Shift+C, the grid menu's formats) holds at most 8 MiB of text, counting everything (quotes, escapes, `NULL`, column names). A larger one copies nothing and says how many rows fit.
- **Evidence:** `COPY_MAX_BYTES` (`crates/engine/src/cmd/selection.rs`); `a_copy_over_the_cap_copies_nothing_and_says_why` (`crates/ui-egui/tests/clipboard.rs`).
- **Impact:** large selections need several copies until file export lands in M3 (entry 1).
- **Estimate:** M3.

### 9. Postgres and SQLite: unique indexes without a constraint are not row keys (F)

- **Limit:** a table without a primary key can still be edited through a unique constraint on NOT NULL columns. A unique index made with `CREATE UNIQUE INDEX` is not seen, so on Postgres such a table is read-only in the grid, and the note above it says why. On SQLite such a table is edited by its rowid instead. MySQL and MariaDB keep no such difference: a unique index is their unique key.
- **Evidence:** on Postgres keys come from `pg_constraint` only (`contype` `p`, `u`, `f`: `CONSTRAINTS_SQL`, `crates/pg/src/catalog.rs`; `TableInfo::row_key`, `crates/core/src/schema.rs`). On pg14 (2026-10-26), a unique index alone leaves `pg_constraint` empty for its table. On SQLite only `UNIQUE` constraints are keys (`crates/sqlite/src/catalog.rs`, the module docs).
- **Impact:** such tables are changed through SQL in a query tab (Postgres), or by their rowid (SQLite).
- **Estimate:** 2–3 h (estimated): read `pg_index` (`indisunique`, no expressions, no predicate), and SQLite's unique indexes.

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

### 14. Postgres: a failed statement leaves a query tab's transaction failed (D)

- **Limit:** on Postgres, a script that opens a transaction (`BEGIN`) and fails inside it leaves the query connection in that failed transaction: every later run fails with SQLSTATE 25P02 until a `ROLLBACK`. dbee does not end it, since it cannot know whether the user means to roll back to a savepoint instead. The error card says so: under a 25P02 error it adds that a ROLLBACK ends the failed transaction.
- **Evidence:** `a_failed_transaction_says_a_rollback_ends_it` (`crates/ui-egui/tests/query_tab.rs`); `ROLLBACK_NOTE` (`crates/ui-egui/src/query_tab.rs`).
- **Impact:** one more run (`ROLLBACK`) after a failed transaction. A reconnect also ends it. MySQL, MariaDB and SQLite keep a transaction going after a failed statement, so there it does not arise.
- **Estimate:** 2–4 h (estimated): show the open or failed transaction in the query tab and offer Rollback.

### 15. A table without a row key pages by OFFSET alone (D)

- **Limit:** on Postgres, MySQL and MariaDB a table with neither a primary key nor a NOT NULL unique key pages with `LIMIT 101 OFFSET n` and no `ORDER BY`, so the server may return its rows in another order from one page to the next (SQLite pages such a table by its rowid). Under concurrent writes (or a plan change) a page can repeat rows of another or skip some. Ordering by `ctid` would fix the order, but it sorts the whole table for every page, which a large keyless table cannot afford.
- **Evidence:** `page_sql` (`crates/sql/src/select.rs`) orders by the row key only; `page_details` (`crates/sql/tests/builders.rs`) checks that a keyless table's page has no `ORDER BY`.
- **Impact:** browsing a keyless table that changes meanwhile can show a row twice or miss one; such tables are read-only in the grid (no row to name in an `UPDATE`).
- **Estimate:** 3–5 h (estimated) on Postgres: page a keyless table by `ctid` ranges (a `tid` cursor), which needs no sort. MySQL and MariaDB have no such row id; not estimated there.

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

### 18. Linux: Open With goes by a file's content, dbee by its name (U)

- **Limit:** the desktop entry lists dbee for `application/vnd.sqlite3` and `application/x-sqlite3`. File managers give a file that type by its content too, so they offer dbee for any SQLite file, whatever its name (`.db3`, `.gpkg`, none). dbee picks the database type by extension, so such a file is refused, with a notice naming the extensions it opens and saying that a file with another name opens through New connection. And on a system where no other app takes SQLite files and the user's MIME settings name none, the desktop has only dbee to choose, so a double-click opens SQLite files with it. dbee sets no default itself; it is the only candidate.
- **Evidence:** `packaging/linux/dev.rosin.dbee.desktop` (`MimeType=`); shared-mime-info detects `application/vnd.sqlite3` by its magic bytes as well as by `*.sqlite3`. `connection.openFile` and its refusal (`crates/engine/src/cmd/connection.rs`); `open_file_finds_the_type_by_extension_in_any_case` (`crates/engine/tests/connection.rs`), `a_dropped_file_of_no_type_dbee_opens_says_which_do` (`crates/ui-egui/tests/workspace.rs`).
- **Impact:** a SQLite file with an unusual name takes one more step: New connection, then the file. Nothing is changed or lost.
- **Estimate:** 1–2 h (estimated) if Open With should open such a file anyway: pick the type by the file's first bytes when its name names none.

### 19. MySQL and MariaDB: no Unix socket (D)

- **Missing:** connecting to MySQL or MariaDB through its socket file (`/var/run/mysqld/mysqld.sock` and the like) or a Windows named pipe. dbee connects over TCP only, directly or through the SSH tunnel.
- **Evidence:** the MySQL driver opens a TCP connection or an SSH channel (`crates/mysql/src/connect.rs`); the connection form has a host and a port, no socket path. A non-goal of M2 (drivers spec §4).
- **Impact:** a server that listens only on its socket (`skip_networking`) cannot be reached. Most servers also listen on TCP, on `127.0.0.1:3306` at least, and the SSH tunnel reaches that.
- **Estimate:** 2–4 h (estimated).

### 20. SQLite: no attached databases, no extensions, no encrypted files (D)

- **Missing:** the tables of a database attached with `ATTACH` in the tree and in table tabs; loading an extension (SpatiaLite and the like); opening an encrypted file (SQLCipher).
- **Evidence:** the SQLite catalog reads schema `main` only (`crates/sqlite/src/catalog.rs`); rusqlite is built without its `load_extension` feature (`crates/sqlite/Cargo.toml`). In a normal run `ATTACH` works in a query tab, but the attached tables are not listed; in an isolated run it is refused. Non-goals of M2 (drivers spec §4).
- **Impact:** an attached database is read through SQL in a query tab only. A table that needs an extension's functions or virtual tables cannot be read, and an encrypted file is refused as not a database.
- **Estimate:** attached databases 3–5 h (estimated); extensions and encryption not estimated.

### 21. Ctrl+P finds saved connections only (U)

- **Limit:** the quick search lists saved connections, not recent ones. With nothing typed on the workspace it lists the open connection's tables first, so the other connections show once a letter or two is typed. Letters are matched from the left, so `stag` matches "Shop staging" from the S of "Shop", not as the start of "staging", and ranks it lower than it could.
- **Evidence:** `quickSearch.find` reads the saved list only (`crates/engine/src/cmd/schema.rs`); `fuzzy::search` puts the tables first for an empty query and matches letters left to right (`crates/engine/src/fuzzy.rs`).
- **Impact:** a connection opened once from Recent must be saved to be found. Typing the start of a word of the name finds it.
- **Estimate:** 1–2 h (estimated) each: Recent in the search; a match that tries each word start.

### 22. SQLite: a row keyed by its rowid can change under a VACUUM (S)

- **Limit:** a SQLite table without a primary key or a NOT NULL unique key is edited by its hidden rowid. SQLite's `VACUUM` may renumber the rowids of a table without an `INTEGER PRIMARY KEY`. If another program vacuums the file while such a table's page is loaded, an edit staged from that page can write a different row. Apply checks that each statement changes exactly one row, which catches a row that is gone, not one that moved.
- **Evidence:** SQLite's documentation of `VACUUM`; the rowid key in `crates/sqlite/src/catalog.rs` (`a_table_without_a_key_is_keyed_by_a_hidden_rowid`, `crates/sqlite/tests/catalog.rs`); `docs/decisions.md` (2026-11-03).
- **Impact:** rare: it needs another program to vacuum the file while dbee has the table open. Refreshing the page after a VACUUM avoids it.
- **Estimate:** 2–3 h (estimated): check the file's change counter before Apply and refuse a stale page.

### 23. Linux without the desktop portal: Choose… cannot pick a file without an extension (U)

- **Limit:** where xdg-desktop-portal is missing, the file picker is zenity's. Its "All files" filter becomes `*.*`, so a database file whose name has no extension is not offered.
- **Evidence:** rfd's zenity backend turns the filter `*` into `*.*` (`rfd` 0.17, `src/backend/linux/zenity.rs`); the pickers are in `crates/ui-egui/src/picker.rs`.
- **Impact:** such a file can't be chosen in the connection form. Renaming it with `.sqlite` or `.db`, or installing the portal, works around it.
- **Estimate:** 1–2 h (estimated): no filter at all where only zenity is there.

### 24. The connection form is cramped at the smallest window (U)

- **Limit:** at the window's minimum size (640×420) the form under the title row is too narrow: the TLS mode buttons and Save, Test and Connect run past the card's edge, Test covers part of Save, and the name field shrinks to about 30 points.
- **Evidence:** the connection form's layout (`crates/ui-egui/src/conn_form.rs`), seen at 640×420. At the default size (1100×720) and above everything fits (`crates/ui-egui/tests/connections.rs`).
- **Impact:** small: only in a window made as small as it goes. A wider window shows the form whole.
- **Estimate:** part of the connection screen's redesign; not estimated.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-05 | minor | M2 done: entries 19 (no Unix socket for MySQL and MariaDB), 20 (SQLite: no attached databases, extensions or encrypted files), 21 (Ctrl+P finds saved connections only, tables first with nothing typed), 22 (SQLite rowid keys and VACUUM), 23 (zenity's "All files" picker), 24 (the cramped form at the smallest window); entries 3, 6, 7, 9, 14 and 15 say which engines they hold for. |
| 2026-11-05 | minor | Entry 18: on Linux, file managers offer dbee for any SQLite file by its content while dbee opens by extension, and with no other app for SQLite files dbee is the only candidate. |
| 2026-11-05 | minor | Entry 17: on macOS, Finder's Open With cannot hand dbee a database file (winit does not pass the open-documents event on), so the bundle declares no document types. |
| 2026-10-28 | minor | Import and export is now M3 (M2 is three databases). |
| 2026-10-27 | minor | Wording: no personal names or task ids. |
| 2026-10-27 | minor | Entry 14 fixed and dropped: filters compare `char(n)` as `bpchar`, `bit(n)` as `"bit"` and arrays of `varchar(n)`, `char(n)` and `bit(n)` as `character varying[]`, `bpchar[]` and `"bit"[]`, so they use the column's index and an over-long element matches nothing; 15–17 are now 14–16. |
| 2026-10-27 | minor | M1 review fixes: generated and identity `GENERATED ALWAYS` columns refused (old entry 8 dropped, 9–15 now 8–14); entries 15 (a failed transaction in a query tab), 16 (keyless paging), 17 (empty filter values). |
| 2026-10-27 | minor | Entry 15 (now 14): values that do not fit a column's length are refused, not cut; what is left (filters on `char(n)` and `bit(n)` skip their index, array filters still cut). |
| 2026-10-27 | minor | Timings quoted from target-app-parity.md, the one source; the next page 1.06–1.09 ms; a fifth wide-result run; the tarball's and AppImage's SSH key logins marked expected, not measured. |
| 2026-10-26 | major | M1 built: 14 known limits with evidence, ranked; wide results and the long-line editor measured. |
| 2026-10-15 | major | Template (scaffold). |

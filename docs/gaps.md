# Gaps

> **Last reviewed:** 2026-11-08 · **Last updated:** 2026-11-19 · **Change:** minor (entry 55: `table.structure` also gives each column's default and each foreign key's ON DELETE and ON UPDATE, on all four engines; before: entry 55: the engine side done: `table.structure` reads a table's indexes and triggers on all four engines; the switch and the page are left; before: entry 58 done and dropped: the cells an Apply wrote show the saved mark, then it fades; before: entry 14 done: closing the window asks too, so every way out of a transaction asks first; what stays is the shared session, a limit; before: entry 14: the window's transaction prompt, its toast and the switch that waits for it; only quit is left; before: entry 14: closing over a transaction asks how to end it in the engine; before: the M3 Phase 2 review: entries 14, 53 and 54 say what is left now that the status bar and the error band are built; entries 67 to 73 are new; entries 24, 56 and 61 gain a line)

Ranked by user impact; pick from the top. Each entry says what is missing or limited, the evidence a reviewer can rerun, who it hurts and an estimate. Hours are agent wall-clock hours, **estimated**. Timings are **measured**, in a dev build (the workspace's dev profile: dbee's crates at opt-level 1, dependencies at 2). They are quoted from [`target-app-parity.md`](target-app-parity.md), "Performance", which holds every measurement with its machine, runs and load averages.

Kinds: F feature, U UI, D database engine, FF file format, H hardware, L localization, P performance, S stability.

## Alpha blockers

### 1. No export or import of files (FF)

- **Missing:** exporting a table or a query result to a file (CSV, JSON, SQL) and importing one. The alpha gate's last row ([`roadmap.md`](roadmap.md)).
- **Evidence:** `engine.commands` lists no export or import command; the way out today is the clipboard, at most 8 MiB of text (entry 8).
- **Drawn:** the M3 screens have EXPORT and IMPORT buttons, the footer's EXPORTING and IMPORTING, and a toast's OPEN FOLDER. They start working with this entry.
- **Impact:** results larger than a copy, or meant for a file, cannot leave dbee. The maintainer switches to dbee fully once this lands (M4).
- **Estimate:** M4 "Import and export"; not estimated yet.

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
| 2026-11-19 | minor | Entry 55: the engine side done: `table.structure` gives a table's columns, keys, REFERENCED BY, indexes and triggers on all four engines, read when asked; the switch and the page are left. |
  | 50 | 6.5–13.9 ms | 8.2–17.3 ms |
  | 100 | 10.7–27.1 ms | 15.4–39.8 ms |
  | 300 | 38–67 ms | 50–97 ms |
  | 1,664 (Postgres's most) | 231–897 ms | 357–778 ms |

  kittest, which always builds an AccessKit tree, measured 1,664 columns at 376–792 ms a frame (340–890 ms in earlier runs).
- **Impact:** at about 100 columns a frame no longer fits 60 Hz (in four of the five runs). At 300 columns scrolling stutters, and at 1,664 the window draws one to four frames a second. `select *` over a wide join is a common case.
- **Estimate:** 4–8 h (estimated): lay out and draw only the columns in view.

### 6. One connection per window (F)

- **Missing:** several connections at once, or several windows.
- **Evidence:** the session holds its open connections by id (`State::connections`, `crates/engine/src/state.rs`) and each tab names its connection, but it allows one (`MAX_OPEN`, `crates/engine/src/cmd/mod.rs`): `connection.open` is disabled while one is open (a later milestone). The window shows one connection's tree.
- **Impact:** comparing two databases means switching between them (Ctrl+P finds a saved connection and switches to it, asking first about query text and staged edits). Tabs and staged edits belong to the one connection.
- **Estimate:** not estimated (the window's tree, status bar and tabs per connection, and the engine's checks that still judge the active tab's connection).

### 7. Query results are capped at 10,000 rows (F)

- **Limit:** a query keeps at most 10,000 rows per result and 256 MiB of cell text per run, and says `First 10,000 of N rows`.
- **Evidence:** `QUERY_LIMITS` (`crates/driver/src/api.rs`), the same on every engine; `a_result_cut_at_the_cap_says_so` (`crates/ui-egui/tests/query_tab.rs`).
- **Impact:** a larger result cannot be read through in the query tab. A table tab pages through any number of rows (100 a page; the next page of a million-row table loads in 1.06–1.09 ms, [`target-app-parity.md`](target-app-parity.md), "Performance"), and file export comes in M4.
- **Estimate:** part of M4's export; paging query results is not estimated.

### 8. A copy is at most 8 MiB of text (F)

- **Limit:** one copy (Ctrl+C, Ctrl+Shift+C, the grid menu's formats) holds at most 8 MiB of text, counting everything (quotes, escapes, `NULL`, column names). A larger one copies nothing and says how many rows fit.
- **Evidence:** `COPY_MAX_BYTES` (`crates/engine/src/cmd/selection.rs`); `a_copy_over_the_cap_copies_nothing_and_says_why` (`crates/ui-egui/tests/clipboard.rs`).
- **Impact:** large selections need several copies until file export lands in M4 (entry 1).
- **Estimate:** M4.

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

### 11. The Flatpak cannot reach the keyring (H)

- **Missing:** a Flatpak permission for the secret service (the keyring). The manifest now grants read-only `~/.ssh` and the SSH agent socket; it grants no secret-service bus name.
- **Evidence:** `packaging/linux/flatpak/dev.rosin.dbee.yml` (`--filesystem=~/.ssh:ro`, `--socket=ssh-auth`; no `--talk-name=org.freedesktop.secrets`). Not built or run here (unverified).
- **Impact:** in the Flatpak, saved passwords do not work. SSH tunnels with the user's key and the agent should; a Flatpak host has not checked it. The tarball and the AppImage are expected to work (not measured: key logins through the tunnel were exercised in the dev build only, by `m1_day` and `crates/ssh/tests/tunnel.rs`).
- **Estimate:** 1–2 h (estimated), plus a check on a Flatpak host.
- **Also:** `~/.ssh` is read-only there, so a host key the user trusts is saved nowhere ("trusted for this connection only") and every connect asks again, which trains Trust on every connection. The fix: learn into a `known_hosts` of dbee's own in the app's data folder and read both files (about 1 h, estimated). The security review's lane C, C-6.

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

### 14. Query tabs share one session, and its transaction outlives a tab (D)

- **Limit:** every query tab of a connection runs on one query connection, so they share one session: its transaction, its `SET`s and its temporary tables (one session per tab is not planned for M3). A transaction a tab opened (`BEGIN`) stays open when the tab closes without ending it (`force`), and another tab's `COMMIT` or `ROLLBACK` ends it. On MySQL and MariaDB a statement that commits on its own (`CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `LOCK TABLES` and others) in any tab commits it, without a word. On Postgres a script that fails inside a transaction leaves the session in that failed transaction: every later run, in any tab, fails with SQLSTATE 25P02 until a `ROLLBACK`. dbee does not end it, since it cannot know whether the user means to roll back to a savepoint instead; under a 25P02 error the band adds that a ROLLBACK ends it. MySQL, MariaDB and SQLite keep a transaction going after a failed statement, so there that part does not arise.
- **Done (engine):** the connection's `transaction` in `document.inspect` says whether its query session has one open (`open`, Postgres's `failed`, or `unknown`) and which tab's run opened it, as each driver reports after every run: Postgres's ReadyForQuery status (read off the stream, which tokio-postgres keeps to itself), MySQL's and MariaDB's `SERVER_STATUS_IN_TRANS` (a `COM_PING` after an error), SQLite's autocommit flag; never from the SQL. A reconnect or a lost query connection reads none. `query.commit` and `query.rollback` end it from any of the connection's query tabs. Closing the tab that opened it, while it is open or failed, or the connection while it has one, asks how to end it (`needsTransactionEnd`, which carries the sentence about query text or staged edits the close would also lose, so it is the only question); `tab.close` and `connection.close` with `end: "commit"` or `"rollback"` end it, then close, and close nothing when the end fails. While a run goes in it, only a rollback ends it, which stops the run first.
- **Done (window):** the status bar says IN TRANSACTION while the query session has a transaction open or failed (a running job and a lost connection come before it there). Closing the owner tab, or closing or switching the connection (Ctrl+Shift+Q, Ctrl+P, a dropped file), asks that question in the transaction prompt: COMMIT, ROLLBACK or CANCEL, or only ROLLBACK and CANCEL for a failed transaction or one a run still goes in, with the lost-work sentence in the same prompt. Its outcome is a toast (`Committed on Shop`, `Rolled back on Shop`, or `The commit failed: …` with nothing closed), and a switch waits for the end before it opens the other connection. Closing the window asks too, about each connection in turn: the transaction prompt, or `Quit anyway?` for a query or an Apply still running, query text or staged edits; CANCEL or a failed end keeps the window open, and with nothing to lose it closes at once (`app.quit` never asks).
- **Evidence:** the query connection (`Connected::query`, `crates/engine/src/state.rs`) is one per connection; `Connected::transaction` and `Transaction` (`crates/engine/src/state.rs`), `Connection::transaction` (`crates/driver/src/api.rs`), `crates/pg/src/tap.rs`; `end_then_close` and `end_question` (`crates/engine/src/cmd/query.rs`); the tests of `crates/engine/tests/transaction.rs`; `Dialog::EndTransaction`, `ended` and `quit_close` (`crates/ui-egui/src/dialogs.rs`), `switch_pending` (`crates/ui-egui/src/connections.rs`), `close_requested` and `quit_pending` (`crates/ui-egui/src/app.rs`), `the_transaction_prompt_matches_the_page` and `the_quit_words_say_what_still_runs_first` (`crates/ui-egui/src/dialogs.rs`) and `commit_from_the_prompt_closes_the_tab_and_toasts`, `a_lost_connection_at_commit_toasts_and_keeps_the_tab`, `a_switch_waits_for_the_end_then_switches`, `quitting_with_a_transaction_asks`, `quit_stops_at_the_first_cancel` and `quitting_with_a_running_query_asks` (`crates/ui-egui/tests/workspace.rs`); `the_transaction_state_is_read_on_every_way_there` (`crates/pg/tests/connect.rs`), `the_transaction_state_follows_the_servers_status` (`crates/mysql/tests/query.rs`) and `the_transaction_state_follows_autocommit` (`crates/sqlite/tests/query.rs`); `a_failed_transaction_says_a_rollback_ends_it` (`crates/ui-egui/tests/query_tab.rs`); `ROLLBACK_NOTE` (`crates/ui-egui/src/query_tab.rs`); `activity` (`crates/ui-egui/src/workspace.rs`) for IN TRANSACTION.
- **Impact:** a tab closed with `force` (from the control channel) leaves its transaction open, and an uncommitted change made in it can be committed later by another tab, by its `COMMIT` or, on MySQL and MariaDB, by its DDL. A reconnect ends the session and its transaction (rolled back).
- **Estimate:** the M3 work is done; what stays is the shared session, and one session per tab is not planned for M3; not estimated.

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
- **Evidence:** the MySQL driver opens a TCP connection or an SSH channel (`crates/mysql/src/connect.rs`); the connection form has a host and a port, no socket path. A non-goal of M2.
- **Impact:** a server that listens only on its socket (`skip_networking`) cannot be reached. Most servers also listen on TCP, on `127.0.0.1:3306` at least, and the SSH tunnel reaches that.
- **Estimate:** 2–4 h (estimated).

### 20. SQLite: no attached databases, no extensions, no encrypted files (D)

- **Missing:** the tables of a database attached with `ATTACH` in the tree and in table tabs; loading an extension (SpatiaLite and the like); opening an encrypted file (SQLCipher).
- **Evidence:** the SQLite catalog reads schema `main` only (`crates/sqlite/src/catalog.rs`); rusqlite is built without its `load_extension` feature (`crates/sqlite/Cargo.toml`). In a normal run `ATTACH` works in a query tab, but the attached tables are not listed; in an isolated run it is refused. Non-goals of M2.
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

- **Limit:** at the window's minimum size (640×420) the form under the title row is too narrow: the TLS mode buttons and Save, Test and Connect run past the card's edge, Test covers part of Save, and the colour swatches beside the name field (216 points wide since M3) leave the field about 18 points, narrower than its label, so `NAME` runs into `COLOUR`.
- **Evidence:** the connection form's layout (`crates/ui-egui/src/conn_form.rs`), seen at 640×420. At the default size (1100×720) and above everything fits (`crates/ui-egui/tests/connections.rs`).
- **Impact:** small: only in a window made as small as it goes. A wider window shows the form whole.
- **Estimate:** part of the connection screens (M3 Phase 3); not estimated.

### 25. MySQL and MariaDB: a `CALL` shows only its last result set (D)

- **Limit:** a stored procedure that returns several result sets shows only the last one with rows; the ones before it are read and dropped, with no sign.
- **Evidence:** `crates/mysql/src/result.rs` keeps the last row-returning result; `several_result_sets_keep_the_last` (`crates/mysql/tests/query.rs`).
- **Impact:** a procedure that reports in steps shows its last step only. Calling the statements one by one shows each.
- **Estimate:** 3–5 h (estimated): one statement giving several results in the query tab's selector.

### 26. MySQL and MariaDB: the warning count is not shown (D)

- **Limit:** the server's warning count in its OK packet is dropped. On a server without a strict `sql_mode`, a query tab's `UPDATE` that cuts a value shows "1 row affected" and nothing else (grid edits run under a strict `sql_mode`, so there the value is refused).
- **Evidence:** the OK packet's warnings are not read into the result (`crates/mysql/src/result.rs`); `ResultSet` has no field for them (`crates/core`).
- **Impact:** a truncation or a conversion in a query tab goes unseen. `SHOW WARNINGS` right after the statement lists them.
- **Estimate:** 1–2 h (estimated): the count beside "rows affected", with a link that runs `SHOW WARNINGS`.

### 27. Read-only guards against mistakes, not against what runs inside one statement (D)

- **Limit:** on a read-only connection dbee refuses, before sending, the statements that can turn read-only off or write in spite of it (a `DO` block, a MySQL or MariaDB block of several statements, `CALL`, `SET … READ WRITE`, MariaDB's `SET STATEMENT`, `ALTER SYSTEM`, `COPY … TO`, `INTO OUTFILE`, `SET GLOBAL`, SQLite's `ATTACH` and `VACUUM INTO`, and the others the decisions of 2026-11-08 list), and checks the mode after each statement (Postgres reads it back; MySQL and MariaDB take the server's own report). What a function does inside one statement stays out of a client's reach: one that commits through another connection (Postgres's `dblink`), one that turns read-only off and writes before its statement ends, and the built-ins that run SQL text (`query_to_xml`, `query_to_xml_and_xmlschema`, `cursor_to_xml` and the like; measured: `query_to_xml('SELECT lo_from_bytea(…)', …)` made a large object on Postgres 14). On MySQL and MariaDB a function may also turn session tracking off (`SET SESSION session_track_system_variables = ''` in a function: measured on all five servers), after which a later function's change of read-only mode goes unreported, and so does a later statement whose text slips past dbee's check; dbee sets the tracking up again only after it found such a change. A function that changes `sql_mode` or the character set is out of reach too, as one that turns read-only off is: the guard reads text as the session did at connect. Statements that write no table data run too: Postgres's `VACUUM FULL` and `CHECKPOINT`, and `COMMIT PREPARED` and MySQL's `XA COMMIT` of a transaction another session prepared (not tried: the test servers allow no prepared transactions).
- **Evidence:** `read_only_holds_on_every_engine`, `read_only_postgres_refuses_what_turns_it_off_and_reads_it_back`, `read_only_mysql_refuses_what_turns_it_off_and_reads_it_back` and `read_only_sqlite_attaches_nothing_and_keeps_query_only` (`crates/engine/tests/query.rs`), `the_servers_report_catches_what_the_text_check_misses` (`crates/mysql/src/guard.rs`), `a_raw_filter_on_a_read_only_connection_writes_nothing` (`crates/engine/tests/table.rs`); the checks (`crates/sql/src/refusal.rs`, `crates/pg/src/guard.rs`, `crates/mysql/src/guard.rs`, the authorizer in `crates/sqlite/src/connect.rs`).
- **Impact:** a pasted script or a migration file is stopped; someone who means to write with the connection's role can. A database user without write grants is the lock, as the form's Read-only says.
- **Estimate:** none: the lock is a role on the server.

### 28. A server's idle timeout ends the session, and dbee then asks to reconnect (D)

- **Limit:** after the server's idle timeout (MySQL's and MariaDB's `wait_timeout`, often short on hosted servers), the next action finds the browse connection gone and asks for a reconnect; a query tab's session (its transaction, `SET`s and temporary tables) is gone with it. Postgres servers rarely end idle sessions, so there it rarely shows.
- **Evidence:** `a_timed_out_connection_reports_connection_lost` (`crates/mysql/tests/query.rs`); the lost-connection state (`crates/engine/src/cmd/connection.rs`).
- **Impact:** one more click (Reconnect) after a pause; nothing is lost but an open transaction, which the server already rolled back.
- **Estimate:** 1–2 h (estimated): a ping or an automatic reconnect of an idle browse connection.

### 29. MySQL and MariaDB: some login methods are refused (D)

- **Missing:** the login methods `client_ed25519` and PARSEC (MariaDB) and `mysql_clear_password` (used with LDAP and PAM). A server that asks for one is refused with an error naming it, before the password is used.
- **Evidence:** `auth_switch_to_an_unknown_method_is_auth_error` (`crates/mysql/tests/connect.rs`); the login methods dbee answers (`crates/mysql/src/auth.rs`).
- **Impact:** an account set up with one of them cannot log in; `mysql_native_password`, `caching_sha2_password` and `sha256_password` work.
- **Estimate:** 2–4 h (estimated) each; `mysql_clear_password` only over TLS.

### 30. MySQL and MariaDB: `lower_case_table_names` 1 and 2 are untested (D)

- **Limit:** the test servers run with `lower_case_table_names = 0`, the Linux default. On 1 (Windows) and 2 (macOS) the server folds or compares names by case differently, which the catalog's matching of tables, columns and keys has not been tried against.
- **Evidence:** every server in `xtask/testdb/compose.yml` runs with the default; the catalog joins by name (`crates/mysql/src/catalog.rs`) and logs the rows it could not place (`dropped()`).
- **Impact:** unknown on such servers: a table whose keys are not found would be read-only in the grid.
- **Estimate:** 2–3 h (estimated): a test server with each setting.

### 31. MariaDB: system-versioned tables with hidden period columns are read-only (D)

- **Limit:** a system-versioned table whose `row_start` and `row_end` are implicit has a primary key that names `row_end`, a column `information_schema.COLUMNS` does not list, so the grid finds no row key and keeps the table read-only. With explicit (INVISIBLE) period columns the table is edited as any other.
- **Evidence:** the row key needs every key column in the catalog (`crates/mysql/src/catalog.rs`); MariaDB's `KEY_COLUMN_USAGE` names `row_end` in such a table's key while `COLUMNS` leaves it out (measured on the MariaDB test servers).
- **Impact:** such a table is changed through SQL in a query tab.
- **Estimate:** 1–2 h (estimated): key such a table by its visible key columns.

### 32. The catalog is not read in one snapshot (D)

- **Limit:** on Postgres, MySQL and MariaDB the catalog is read in several queries, outside one snapshot, so DDL that runs meanwhile on the server can leave the tree with a table whose columns or keys are from before it.
- **Evidence:** the catalog loads (`crates/pg/src/catalog.rs`, `crates/mysql/src/catalog.rs`) run their queries one after another in auto-commit. On Postgres one snapshot needs care: a failed query inside it would leave the browse session in an aborted transaction.
- **Impact:** rare: it needs DDL during the load. Refresh (`schema.refresh`) reads it again.
- **Estimate:** 1–2 h (estimated).

### 33. MySQL and MariaDB: connections close without `COM_QUIT` (D)

- **Limit:** dbee drops a MySQL or MariaDB connection by closing its socket, without the protocol's goodbye. With MariaDB's default `log_warnings = 2` (and MySQL 5.7's notes) the server writes an "Aborted connection" line to its error log for each Test, database list, disconnect and reconnect.
- **Evidence:** the connection is dropped without `quit()` (`crates/mysql/src/connect.rs`).
- **Impact:** noise in the server's error log; nothing is lost.
- **Estimate:** 1 h (estimated): a `Connection::close()` (a no-op by default) that sends `COM_QUIT`.

### 34. SQLite: `ilike` and `like` ignore case for A–Z only (D)

- **Limit:** SQLite's `LIKE` and `LOWER()` fold the letters A to Z only, so `like` and `ilike` match `é` and `É` as different letters. The filter bar's operators say so in their note.
- **Evidence:** the SQLite dialect's operator notes (`crates/sql/src/dialect/sqlite.rs`); SQLite's documentation of `LIKE` and `lower()`.
- **Impact:** a case-insensitive filter on text outside ASCII misses rows that differ in case.
- **Estimate:** 1–2 h (estimated): a Unicode case fold registered as a function on the connection.

### 35. A catalog that takes more than 60 s cannot be opened (P)

- **Limit:** the catalog load is one step bounded at 60 s (`CATALOG_TIMEOUT`, `crates/engine/src/cmd/connection.rs`), and dbee loads every table, column and key before the tree shows. On a server whose catalog is very large the load can take longer, and the connection fails with a message that names the 60 s.
- **Evidence:** the bound and its message (`crates/engine/src/cmd/connection.rs`, tested there). No test uses a catalog that large: the seeds hold 300 tables, and the driver tests load them in under 2 s.
- **Impact:** such a server cannot be opened at all.
- **Estimate:** not estimated: a light load of the table list and details per table, which the Structure view needs too.

### 36. SQLite: a file a file manager hands dbee opens read-write, and a value may be 1 GB (D)

- **Limit:** a file opened from a file manager (not a saved connection) opens read-write, so SQLite applies a hot journal or a WAL beside it, as it always does. A value may be as large as SQLite's own limit, 1 GB, which SQLite holds in memory before dbee's result cap leaves it out.
- **Evidence:** `connection.openFile` and the open (`crates/sqlite/src/connect.rs`). The security review of 2026-11-08 suggested opening such a file read-only until the user chooses otherwise, and `SQLITE_LIMIT_LENGTH` near the 256 MiB result cap.
- **Impact:** low: a hostile file needs a bug in SQLite itself to gain anything, and files open with SQLite's hardening for untrusted files. With the length limit, a page holding a larger value would fail to load instead of leaving the value out.
- **Estimate:** 1–2 h (estimated).

### 37. Isolated runs: a parent folder swapped in during a SQLite open is followed (S)

- **Limit:** an isolated run opens the path its check resolved with `SQLITE_OPEN_NOFOLLOW`, so SQLite refuses a link anywhere in it. Between SQLite's own look at the path and its `open()`, a parent folder swapped for a link is still followed. On Windows `NOFOLLOW` does nothing: only SQLite's Unix VFS reads it, so there a link swapped in after the check is followed.
- **Evidence:** `a_link_swapped_in_after_the_check_never_opens_a_file_outside` (`crates/sqlite/tests/connect.rs`) swaps the file itself; nothing swaps a folder.
- **Impact:** it needs a process of the same user racing the open within that window; an isolated run guards against mistakes, not against such a process.
- **Estimate:** 2–4 h (estimated): open the folder with `O_DIRECTORY | O_NOFOLLOW` and check it again after the open.

### 38. MySQL and MariaDB: read-only rests on the server's report, which an error leaves out (D)

- **Limit:** on a read-only MySQL or MariaDB connection the server reports, in each statement's own OK packet, a change of the read-only variable and of the transaction's characteristics (session tracking), so a statement that turns read-only off is caught right after it ran, however its text was spelled, also through `PREPARE` and `EXECUTE`, `EXECUTE IMMEDIATE`, `CALL` and a function. Not covered by the report: an error packet carries none, and MariaDB drops a variable's change made before its error, so after an error dbee reads the variable before the next statement that is not a diagnostic one (one more round trip after each error); MariaDB's `SET STATEMENT … FOR` changes the mode for its one statement before any report, so it is refused from the raw text, as are a block of several statements and a `SET` with a name outside plain ASCII; and a function can turn the tracking off (entry 27); when dbee catches a change, it sets the tracking up again. A read-only session keeps `sql_mode` and the character set as they were at connect (a statement that holds `SET` beside `sql_mode`, or beside a word that changes the character set, is refused from its raw text), and the check reads `"…"` as that session does, as a name under `ANSI_QUOTES`: under `sjis` or `gbk`, and under `ANSI_QUOTES`, the server had read a MariaDB block's `;`s as code where dbee saw them inside a string, and the block wrote (measured on both MariaDB servers, security review fix rounds 4 and 5). The raw rules find `SET` with any mark but a letter, `_` or `$` around it, also right after a version comment's number (`/*!40101SET …*/`): before, that hid a name that MySQL 8.0 and later or MariaDB 13 folds to `sql_mode` or `session_track_system_variables`, which changed the mode or turned the tracking off, and on MariaDB 13 a block then wrote (measured, fix round 5). They refuse more than they need: a `SET` inside a string or a comment counts. `SET CHAR SET` is refused as `SET CHARACTER SET` is, since the servers read `CHAR` as `CHARACTER`; and a line comment ends at a line feed only, as on the servers: before, a bare carriage return ended it for dbee, so a `'` after it hid a block's `;`s, a lookalike name or a `SET` of the read-only variable, and a block or a plain `INSERT` then wrote (measured on all five servers, final re-review). Refused on a read-only connection though harmless (measured on all five; each message says how to respell it or to turn Read-only off): `SHOW CHARACTER SET WHERE Charset LIKE …`; a `SELECT` with two `CAST(… CHARACTER SET …)`s, or one beside the word `names`; the words in strings or as names (`… WHERE 'set' <> 'names'`, ``SELECT 1 AS `set`, 'names' AS n``, `SET @names = …`, `SET @charset = …`, `SET @t1set = 1, @x = 'names'`); mysqldump's `SET @saved_cs_client = @@character_set_client` and a script's `SET @old_sql_mode = @@sql_mode`; `SET NAMES utf8mb4` and `SET CHARACTER SET utf8mb4`, by design; `SELECT 'it\'s; fine'`; ``CAST(… CHARACTER SET …) AS `Größe` `` and `SET @a = "Zürich"`. A whole dump file stops at its first line, as it would at its first `DROP TABLE`.
- **Evidence:** measured on all five servers (security review of 2026-11-08, fix round 2): `SET @@transaction_read_only = 0` and `START TRANSACTION READ WRITE` reported as characteristics `SET TRANSACTION READ WRITE;` and `START TRANSACTION READ WRITE;`, a function's or procedure's `SET SESSION … = 0` as the variable `OFF`, also behind `PREPARE`, `EXECUTE` and `EXECUTE IMMEDIATE`; after `SELECT f(), (SELECT 1 UNION SELECT 2)` MySQL reported the change with the next OK packet and MariaDB never did, while `SELECT @@SESSION.…` read `0` on both. `the_servers_report_catches_what_the_text_check_misses` (`crates/mysql/src/guard.rs`) sends each form the reviews found with the text check skipped; `read_only_mysql_keeps_the_character_set_utf8`, `read_only_mysql_keeps_sql_mode`, `read_only_mysql_reads_double_quotes_as_the_session_does`, `read_only_mysql_finds_set_after_a_version_number`, `read_only_mysql_ends_a_line_comment_where_the_server_does` and `read_only_mysql_sets_the_tracking_up_again_after_a_caught_change` (`crates/engine/tests/query.rs`); `a_line_comment_ends_where_each_engine_ends_it` (`crates/sql/src/lexer.rs`).
- **Impact:** a round trip after each error on a read-only connection, and the harmless statements above refused; after a function turned the tracking off, a statement that slips past the check goes unreported (entry 27).
- **Estimate:** none: the read after an error closes what the report leaves open. The security review's lane B is closed: a further crafted form that gets past the read-only check is listed in this entry, not taken into another round of fixes.

### 39. Postgres: a filter under another `client_encoding` finds other rows, and a trigger can write a value as other text (D)

- **Limit:** a function the session calls (through a view, say) can change `client_encoding`. Apply checks it before its statements and again before its `COMMIT`, and sets it back, but a page or a count does not: a filter value above ASCII then reaches the server as other text and finds other rows, or none. Nothing is written. Nor does Apply see a trigger that changes it for one row and sets it back before `COMMIT`: that row's value is written as other text (`Ã©` for `é`), never as other SQL.
- **Evidence:** the checks are in Apply only (`crates/pg/src/apply.rs`); `an_edit_after_a_view_changed_client_encoding_writes_nothing_hidden` and `an_apply_whose_trigger_changes_client_encoding_writes_nothing` (`crates/engine/tests/edit.rs`) cover Apply.
- **Impact:** a wrong page after such a view, until the next Apply or a reconnect sets the encoding back; wrong text in a row whose table has such a trigger.
- **Estimate:** 1 h (estimated): the same check before each page and count, at one round trip each.

### 40. MySQL and MariaDB: after `SET NAMES` to another character set, text shows wrongly (D)

- **Limit:** dbee sends every statement in UTF-8 and reads every result as UTF-8. On a read-write connection a `SET NAMES`, `SET CHARACTER SET` or `SET` of `character_set_client` to another character set runs, and from then on the query tabs' session reads dbee's text in that one (and, after `SET NAMES` or `SET CHARACTER SET`, sends its results in it): stored text outside ASCII shows wrongly, and text typed into a statement is stored as other text. A read-only connection refuses such a `SET`, to `utf8mb4` too (entry 38).
- **Evidence:** measured on all five servers (security review fix round 4): after `SET NAMES latin1` a stored `Zürich` showed as `Z�rich`, and an `INSERT` of `Größe` stored `GrÃ¶ÃŸe`, which showed as typed until `SET NAMES utf8mb4` and as `GrÃ¶ÃŸe` after it.
- **Impact:** wrong text in a query tab's results, and in rows it writes, until `SET NAMES utf8mb4` or a reconnect.
- **Estimate:** 1 h (estimated): refuse such a `SET` on every connection, or read results in the character set the session reports.

### 41. Through an SSH tunnel, each cancel names the host again (D)

- **Limit:** a direct connection's cancel goes to the address the connection reached. Through the tunnel, each cancel opens a new channel to the host *name*, which the SSH server resolves again: when the name leads to another server by then (a failed-over primary, round-robin DNS), MySQL logs in there with the user's password and sends `KILL QUERY <id>`, which can stop another session's statement there, and Postgres sends the cancel key there.
- **Evidence:** code reading (security review, lane A, L2): `crates/mysql/src/cancel.rs` and `crates/pg/src/connect.rs` open the cancel's channel with `Tunnel::connect(host, port)`; the SSH server's resolver cannot be steered from a test.
- **Impact:** a cancel that stops nothing, or a statement of another session stopped, while a name moves between servers.
- **Estimate:** 2 h (estimated): on MySQL and MariaDB, check `@@server_uuid` (MariaDB: `@@server_id` and `@@hostname`), read at connect, on the KILL connection before the KILL; Postgres has no identity to check.

### 42. A server can keep a statement running forever (S)

- **Limit:** a server that answers a statement with result after result (MySQL's `SERVER_MORE_RESULTS_EXISTS`, Postgres alike), or never ends it, keeps the run going: queries have no read timeout, and a cancel's `KILL QUERY` goes to that same server, which can take it and go on. The run's task and socket live until the server stops; the rows past the caps are read and not kept, so memory stays bounded.
- **Evidence:** security review, lane A, L3: a stand-in that sent OK packets with more results for ever took the KILL with an OK, and the statement still ran 3.5 s later.
- **Impact:** a hostile or broken server holds a query tab's run until the connection closes; nothing else is lost.
- **Estimate:** 2 h (estimated): after a delivered cancel, a grace period (10 s, say), then the connection is marked lost and its socket dropped.

### 43. MySQL fetches the server's RSA key without a word under `disable` or through a tunnel (D)

- **Limit:** with SSL mode `disable`, or through the SSH tunnel, a `caching_sha2_password` or `sha256_password` login takes the server's RSA key without a warning (2026-10-31). A man in the middle of a `disable` link, or between the SSH server and the database, can hand over a key of his own and decrypt the password, not only read the session. MySQL's own client refuses this unless `--get-server-public-key` is given. `allowPublicKey` stays the user's own choice.
- **Evidence:** security review, lane A, L5; the repository's `the_servers_key_when_allowed_comes_with_a_warning` connects under `disable` with no warning.
- **Impact:** the password, on a link the user chose to leave plain or that is plain past the SSH server.
- **Estimate:** 1 h (estimated), not decided: a warning under `disable`, as under `allowPublicKey`, or a refusal unless `allowPublicKey` is on.

### 44. The password is held as a plain `String` (S)

- **Limit:** an open connection keeps the database password in memory for the connection's life, as a plain `String`: MySQL's KILL route for its cancel's login (`crates/mysql/src/cancel.rs`), and tokio-postgres's `Config` on Postgres; the engine keeps it for reconnects too. Nothing clears it when it is dropped.
- **Evidence:** code reading (security review, lane A, L6).
- **Impact:** theoretical: whoever can read dbee's memory can open the keyring as well.
- **Estimate:** 1 h (estimated): a wrapper that zeroes the password when it is dropped.

### 45. tokio-postgres keeps every ParameterStatus a server sends (S)

- **Limit:** every ParameterStatus message, at login or later, goes into a map in tokio-postgres's connection for the connection's life, uncut and with no bound on their number. dbee's watch reads the login's messages but does not hold them to a size, and the map is not dbee's code. The memory is about what the server sends.
- **Evidence:** code reading (lane A review 2, Minor 6): tokio-postgres 0.7.18, `connection.rs:124-127`.
- **Impact:** a hostile server makes dbee hold what it sends, with no cap, on an open connection.
- **Estimate:** 2 h (estimated): a cap in the watch while it reads the login, or a limit in tokio-postgres upstream.

### 46. SSH: `/etc/ssh/ssh_known_hosts` is not read, and names are written unhashed (S)

- **Missing:** dbee reads `~/.ssh/known_hosts` only. A key an administrator recorded in the system file, or a `@revoked` line there, does not count; a user with `HashKnownHosts yes` finds the host names dbee learns in the clear.
- **Evidence:** `crates/ssh/src/known_hosts.rs` (`entries_for` reads the one file `SshEnv` names).
- **Also:** for a server on another port, OpenSSH falls back to a line with no port and accepts a matching key; dbee looks for `[host]:port` only, and does not read `!` exclusions the way a fall-back would need. It asks, then learns the key, which is the safe side.
- **Impact:** a system-wide revocation is not honoured, and a host dbee has not learned asks, though the system file has it. The security review's lane C, C-6.
- **Estimate:** 1 h (estimated).

### 47. Windows Credential Manager entries roam (S)

- **Missing:** dbee stores secrets with the persistence `windows-native-keyring-store` (with `keyring-core`) picks on Windows, which can roam with the user's profile ("Enterprise"). A password may then reach other machines of the same domain account.
- **Evidence:** the security review's lane C, C-7 (read from the crate's code; no Windows machine here). It goes with the decision on packaging.
- **Impact:** Windows users on a domain profile.
- **Estimate:** 1 h (estimated), plus a Windows host.

### 48. macOS ad-hoc signing, and channels an SSH server opens (S)

- **Missing:** unsigned macOS releases are ad-hoc signed, so the keychain's access rule for an item dbee made is the binary's code hash: every update asks again, and users learn to click "Always Allow", for an impostor too. Separately, russh accepts by default the channels an SSH server opens (agent forwarding, forwarded ports, X11, sessions); dbee drops them, so nothing reaches the agent or a local port, but a hostile server can open them without limit.
- **Evidence:** `.github/workflows/release.yml`, `packaging/macos/package.sh` (ad-hoc); `crates/ssh/src/lib.rs` (`Client` overrides no `server_channel_open_*`). Neither is proven to cost anything (no Mac here). The security review's lane C, C-10.
- **Impact:** macOS users on unsigned releases; a hostile SSH server.
- **Estimate:** 1 h (estimated) for the channels, a Developer ID signature as `docs/releasing.md` plans.

### 49. The control channel answers only while the window draws (S)

- **Missing:** a request is handled in a frame of the window, so a minimised or hidden window stalls a client until it is shown again (its reply times out, `docs/control-protocol.md`).
- **Evidence:** `docs/control-protocol.md`, "Requests are handled in the window's frames"; issue #10's note.
- **Impact:** an agent driving a dbee on a machine where the window is minimised or on another desktop that does not draw.
- **Estimate:** 3 h (estimated): serve the channel from a thread that asks the event loop to wake, or run the engine's commands off the draw loop.

### 50. The control channel: a client with no token can keep all 16 places (S)

- **Missing:** a connection that has proved nothing still holds one of the 16 places until its 10 seconds are up, so a client with no token that reconnects in a loop can keep all of them, and a client with the right token gets `busy`. Each refusal also logs a line, which could reach the log's 16 MiB cap over time (not measured).
- **Evidence:** `crates/ui-egui/src/control_server.rs` (`accept`); the security review's lane C fix round, R1 M6.
- **Impact:** an annoyance with no secret in reach, and better than before the token, when a silent connection kept its place for good.
- **Estimate:** 1 h (estimated): count the connections that proved themselves and those that did not apart.

### 51. Two small test gaps around the control channel (S)

- **Missing:** nothing pins that a start without `--control` opens no port and writes no file; and `a_client_that_proved_itself_may_take_its_time` gives the token 1 second to arrive, which could fail on a very loaded machine (it has not yet).
- **Evidence:** `apps/dbee/tests/control_access.rs`; `crates/ui-egui/src/control_server.rs`. The security review's lane C fix round, R1 M9.
- **Impact:** a regression there would go unseen, or a test would flake.
- **Estimate:** 0.5 h (estimated).

### 52. The log's end-to-end test is not built (S)

- **Missing:** an ignored test that runs SQL carrying a marker on each engine with `RUST_LOG=trace` and checks that the log holds no marker or password (the security review's lane C, C-9). Reading every log call with arguments in dbee's crates found none that carries a param, a secret, SQL or server text.
- **Evidence:** no such test in the tree; `target/sec-wt/C/sec_c_log_probe.py` is the probe it would follow.
- **Impact:** a new log call could leak without a test saying so.
- **Estimate:** 1.5 h (estimated).

### 53. No ROLLBACK button after a failed transaction (F)

- **Done (engine):** `query.rollback` (and `query.commit`) end the query session's transaction, a Postgres one that failed (25P02) too, and return the new state; with none open they do nothing and say so.
- **Left:** a ROLLBACK button that runs it. After a failed statement in a Postgres transaction (25P02), the error band only has a note saying a rollback ends it; the band as drawn has no button, and the Query tab screen (M3 Phase 3) adds ROLLBACK.
- **Evidence:** `ROLLBACK_NOTE` (`crates/ui-egui/src/query_tab.rs`); `a_failed_postgres_transaction_reads_failed_and_rollback_ends_it` (`crates/engine/tests/transaction.rs`).
- **Impact:** until the button is there, the user types `ROLLBACK` (or calls `query.rollback`) before that session can run anything else; the band's note says so.
- **Estimate:** part of the Query tab screen (M3 Phase 3); not estimated.

### 54. A run dbee cancelled shows a band where the result bar should say CANCELLED (U)

- **Done (engine):** only dbee's own cancel reads as cancelled. The drivers report Postgres's 57014 and MySQL's and MariaDB's 1317 as the server's error with its code and words, marked `interrupted`; the engine makes it `cancelled` only for a statement it sent a cancel for (a run, a page load, a count), or an Apply whose gate it cancelled. A `statement_timeout`, or a `pg_cancel_backend` or `KILL QUERY` from another session, fails as a `server` error, which the error band shows with the server's code and words; a query tab's error says `meaning: "interrupted"`. SQLite's interrupt comes only from dbee and stays cancelled.
- **Done (window):** the query and table tabs show an error as the M3 error band (M3 Phase 2).
- **Left:** a run that dbee's own cancel stopped still shows a band, in neutral colours; the design shows no band then, and the result bar says CANCELLED. It goes with the Query tab screen (M3 Phase 3). When a Postgres timeout lands while dbee's own cancel request is on its way, both have the same code and the run reads as cancelled: the user asked for the stop.
- **Evidence:** `Meaning::Interrupted` (`crates/driver/src/error.rs`), `stoppable` (`crates/engine/src/jobs.rs`); the tests `a_statement_timeout_reads_as_an_error`, `a_cancel_from_another_session_reads_as_an_error` and `a_cancel_after_the_end_changes_nothing` (`crates/engine/tests/query.rs`) and `a_table_read_stopped_by_the_server_reads_as_an_error` (`crates/engine/tests/table.rs`).
- **Impact:** small: a cancelled run says so in a band, where the design says it in the result bar.
- **Estimate:** part of the Query tab screen (M3 Phase 3); not estimated.

### 55. No structure view (F)

- **Done (engine):** `table.structure` gives everything the page shows, on PostgreSQL, MySQL, MariaDB and SQLite: the catalog's COLUMNS and FOREIGN KEYS, REFERENCED BY worked out from every table's foreign keys in the catalog, and the INDEXES (parts in order, column or expression, descending, unique, primary, method, a partial index's condition) and TRIGGERS (timing, events, row or statement, definition) read from the database when asked, not at connect, with each column's default and each foreign key's ON DELETE and ON UPDATE. Each driver reads its own catalog (Postgres `pg_index` and `pg_trigger`, MySQL and MariaDB `information_schema.STATISTICS` and `TRIGGERS`, SQLite's pragmas and the `CREATE` statements in `sqlite_schema`); server text is cut where kept (8 KiB for an expression or a condition, 64 KiB for a definition).
- **Left:** the table tab's DATA / STRUCTURE switch and its read-only page (the M3 table tab draws both), which run `table.structure`. Known limits: SQLite's `INTEGER PRIMARY KEY` has no index to list; Postgres's `INCLUDE` columns are left out; MySQL and MariaDB give a trigger's body, not its whole statement; MySQL before 8.0.13 and MariaDB have no expression parts; MySQL gives a string default bare and MariaDB as SQL, so dbee quotes MySQL's (`'pending'`) and the two read alike; a foreign key that names no action reads `restrict` on MySQL 5.7 and MariaDB, as they report it; SQLite's generated columns read `generated` as their default, their expression not read (SQLite keeps it only in the `CREATE TABLE` statement).
- **Evidence:** `docs/ui-parity.md` ("Structure and new table": None) for the page; for the engine, `crates/driver/src/structure.rs`, the drivers' `structure.rs`, `table.structure` (`crates/engine/src/cmd/table.rs`) and the tests `crates/{pg,mysql,sqlite}/tests/structure.rs` and `structure_of_a_table` (`crates/engine/tests/table.rs`).
- **Impact:** until the page lands, seeing a table's indexes or triggers in the window means querying the system catalogs by hand; agents can call `table.structure`.
- **Estimate:** 3-5 h (estimated) for the page, with the M3 structure view screen.

### 56. Cancelling a connection attempt: the engine is done, the button is not (F)

- **Done (engine):** `connection.cancel` stops the attempt of an open, an openFile, a reconnect or a test wherever it is (connecting, the SSH tunnel's handshake or login, the database's login, the catalog's load): the socket it was using closes, then the tunnel, and the attempt ends `cancelled`. A cancelled reconnect leaves the connection as it was, with its tabs. With no attempt running it does nothing and says so.
- **Left:** the CANCEL button the M3 connection screen draws beside `CONNECTING`, which runs it; it goes with the connection screens (M3 Phase 3). Until then the form only relabels its button `Connecting…`.
- **Evidence:** `connection.cancel` (`crates/engine/src/cmd/connection.rs`), `Unfinished` (`crates/ssh/src/lib.rs`); the tests `a_connect_to_a_silent_host_can_be_cancelled`, `a_connect_through_the_tunnel_can_be_cancelled`, `a_test_can_be_cancelled`, `a_reconnect_can_be_cancelled`, `cancel_with_nothing_running_is_harmless`, `a_cancelled_attempt_leaves_no_connection` and `a_postgres_connect_cancelled_after_the_login_leaves_no_socket` (`crates/engine/tests/connection.rs`) and `a_dropped_open_closes_its_socket` (`crates/ssh/tests/tunnel.rs`); `crates/ui-egui/src/conn_form.rs` for the form.
- **Impact:** until the button is drawn, a host that doesn't answer keeps the user waiting until the connect limit passes (15 s a step); an agent can call `connection.cancel`.
- **Estimate:** part of the connection screens (M3 Phase 3); not estimated.

### 57. Saved connections aren't ordered by last use (F)

- **Missing:** a stored last-used time, so that the M3 sidebar's one flat list can show the last used first.
- **Evidence:** no such field in `crates/config/src/connection.rs`.
- **Impact:** with many saved connections, the one you use most can be far down the list.
- **Estimate:** 1 h (estimated). Left out of M3 at the start (the maintainer's call, 2026-11-09).

### 59. No update check or update button (F)

- **Missing:** an update check and a button at the right end of the menu bar. With no update out it shows nothing. When one is out it reads `UPDATE 0.4.0 → 0.5.0`, and a click starts the update; the system asks for the password if it needs root (pkexec). While it runs it reads `UPDATING 42%`, then `RESTART TO UPDATE`, or `UPDATE FAILED` with the reason on hover and RETRY. A copy installed by a package manager or Flatpak shows `DOWNLOAD 0.5.0`, which opens the download page. Decided by the maintainer on 2026-11-10; the design draws it after the M3 component groups.
- **Evidence:** no update code in the tree; the version shows only on the connection screen (`crates/ui-egui/src/connections.rs`).
- **Impact:** a user learns of a new version only by looking on GitHub.
- **Estimate:** 8 h (estimated), once drawn: the check is a new network request to GitHub, the download and install need a security review, and each package format updates in its own way.

### 60. The connection form's result strip uses a tint that is no token (U)

- **Limit:** the strip under the form's buttons is filled with its meaning colour, `danger` or `success`, at 8 %, which is no token. Its text reads on it (light `success` at 4.56:1 is the lowest), but the message can be selected, and selected text sits on `selection`: `success` on `selection` in light is 4.13:1, a pair foundations keeps off screen. The strip is provisional until the design draws it.
- **Evidence:** `gamma_multiply(0.08)` in the result strip (`crates/ui-egui/src/conn_form.rs`); the contrast figures are reviewer 1's, from the M3 Phase 1 review.
- **Impact:** small: a user who selects the message of a passed test, in light, reads it at 4.13:1.
- **Estimate:** part of the toast or error band component (M3 Phase 2), once the strip is drawn; not estimated.

### 61. At the smallest window, three texts are cut and the filter's APPLY is out of view (U)

- **Limit:** at 640×420 the paging bar's page label (`PAGE 1`) runs about 29 points past its room, the query toolbar's `RUN CURRENT` is cut and overlaps the row count (`1 ROW`), and the form's title (`New connection`) is cut short at about 101 points. All three were cut before M3's fonts too (`Page 1`, `Run current` over `1 row`); see also entry 24.
- **Limit:** at 640×420 with the value panel closed, the table tab's filter row runs about 97 points past the work area: APPLY is out of view and `+` is cut. It ran about 49 points over before M3; the row's 28-point squares, APPLY's 62 and the 12 inset add the rest. Enter in a value field still applies.
- **Evidence:** reviewer 1's size probe in the M3 Phase 1 review, run on the tree before and after Phase 1; `crates/ui-egui/src/paging.rs`, `crates/ui-egui/src/query_tab.rs` (the toolbar), `crates/ui-egui/src/conn_form.rs` (the title); for the filter row, `ui.inspect` in the M3 Phase 2 review (`filter-apply` at x 655 in a 640-wide window, `visible: false`) and `crates/ui-egui/src/filter_bar.rs`. At 1100×720 nothing is cut.
- **Impact:** small: only in a window made as small as it goes.
- **Estimate:** part of the bars' and the query and table tab screens (M3 Phase 3); not estimated.

### 62. View > Theme can't be reached before connecting (U)

- **Limit:** the menu bar is on the workspace only, so the connection screen has no View menu and no way to choose the theme. A user whose desktop says nothing, or who wants another mode than the desktop's, gets the choice only after the first connect; from then on it holds on the connection screen too, and it is kept between runs.
- **Evidence:** `crates/ui-egui/src/menu.rs` is drawn by the workspace (`crates/ui-egui/src/workspace.rs`) only; `ui.themeDark` and the other two work on the connection screen over the control channel.
- **Impact:** small: only before the first connect, and only when the desktop's setting isn't the one wanted.
- **Estimate:** part of the Shell screen (M3 Phase 3); not estimated.

### 63. A long column type may be cut in the grid header (U)

- **Limit:** a column's first width counts the type at 0.85 of a cell character, from when types were drawn smaller. Since M3 a type is drawn in the cells' own font, so a type longer than its column's name and values can end elided in the header until the column is widened.
- **Evidence:** `initial_widths` in `crates/ui-egui/src/grid.rs` (`chars(source.column_type(col)) * CHAR_W * 0.85`); types are drawn in `Text::Data`.
- **Impact:** small: long types such as `timestamp with time zone` or a MySQL `enum(…)` show cut until the column is dragged wider. The full type is in the value panel.
- **Estimate:** part of the Grid screen (M3 Phase 3); 0.5 h (estimated).

### 64. The tokens check can misread a test helper with a comma in its type (S)

- **Limit:** `cargo xtask tokens` skips the item after a `#[cfg(test)]`, and finds that item's end at its first comma or brace. A test helper whose signature has a comma inside angle brackets, such as `Result<u8, String>`, ends early, so its body is read as drawing code.
- **Evidence:** `item_end` in `xtask/src/tokens.rs`; confirmed with a probe in the M3 Phase 1 fix check. Nothing in the tree trips it today.
- **Impact:** small: it fails loudly, flagging a colour in test code, never quietly letting one through in drawing code.
- **Estimate:** 0.5 h (estimated).

### 65. No Help menu or About (F)

- **Missing:** a Help menu with About, which shows dbee's version once the update button (59) no longer does. Decided with 59 on 2026-11-10; not drawn yet.
- **Evidence:** the menu bar has File, Edit, View and Query (`MENUS`, `crates/ui-egui/src/menu.rs`); the version shows only on the connection screen.
- **Impact:** with 59 in place, the version can't be seen from the workspace.
- **Estimate:** 1 h (estimated), once drawn.

### 66. Names inside the engine's messages are drawn in `body`, not `data` (U)

- **Limit:** the design draws the names, hosts, paths and numbers in a dialog's or a toast's text in `data` mono, as written. dbee marks those it puts into its own sentences: the password dialogs' user and host, the passphrase dialog's key path, the host key dialog's host and key type, Delete connection's name, the requested dialog's name, the copied row count, the theme notice's file and folder, and the sizes in the two notices about values too large to edit inline. A message from the engine arrives as one string, so the names in it stay in `body`: the tab named in Close anyway ("Close Query 1? Its query text will be lost."), the engine's own notices (a secret not kept, a connection's warning), every error shown as a notice (a table, a column, a file), the reason after "The password was not saved:", and the URL dialog's error line.
- **Evidence:** `Need::Confirm` carries `what` and one message (`crates/engine/src/lib.rs`), which `crates/engine/src/cmd/tabs.rs` and `connection.rs` build with the tab's title in it; `UiRequest::Notice` is one `String`; `notice_text` in `crates/ui-egui/src/app.rs` reads an error as its text. The marked sentences use `theme::Words` (`crates/ui-egui/src/theme/text.rs`).
- **Impact:** small: every word is there, but the name doesn't stand out, which matters most in Close anyway, a prompt that loses work.
- **Estimate:** 2–3 h (estimated): the engine gives a message's names as parts (a confirm naming its tab, a notice its file) and the window marks them; with the wording pass (spec §7).

### 67. Some cut names and types have no hover (U)

- **Limit:** three places older than M3 cut a long name or type with `…` and show it whole nowhere in the window: the value panel's column name and type above the value, a tree column's name and type (`timestamp with time …`; only a foreign key's row has a hover), and the grid header's column name (entry 63 covers its type). Since M3 Phase 2 a cut connection name, a cut file path and a long job in the status bar read whole on hover.
- **Evidence:** the facts' name and type in `crates/ui-egui/src/components/value.rs`; `TreeRow::paint` (`crates/ui-egui/src/components/rows.rs`) and the column row's hover in `crates/ui-egui/src/tree.rs`; `Header` (`crates/ui-egui/src/components/grid_cell.rs`). Seen in the M3 Phase 2 review; cut without hover before M3 too.
- **Impact:** small: a long column name or type is read in full only by widening its pane or column, or through `ui.inspect`.
- **Estimate:** 1 h (estimated); with the screens that hold them in M3 Phase 3: the query and table tab screens (the value panel, the grid) and the sidebar screen (the tree).

### 68. At the smallest window with the value panel open, two controls draw past their own edges (U)

- **Limit:** at 640×420 with the value panel open, as it is by default, the work area is narrow enough that the filter row's column dropdown draws its chevron onto the raw SQL toggle beside it, and the error band's position link (`Line 3, column 8 →`) lands left of the band, over the pane's edge, while the message wraps one or two letters a line. Closing the panel or widening the window shows both whole.
- **Evidence:** `Dropdown::show` places the chevron from the control's right edge with no floor (`crates/ui-egui/src/components/dropdown.rs`); the band's `link_room` is not limited to its head's width (`crates/ui-egui/src/components/error_band.rs`). Seen in the M3 Phase 2 review at 640×420, in both modes.
- **Impact:** small: only in a window made as small as it goes, with the panel open.
- **Estimate:** part of the query and table tab screens (M3 Phase 3), where the panes give way before the work area does; each control keeping within its rect is about 0.5 h (estimated).

### 69. The tokens check misses a token at an opacity, and `theme::mask` (S)

- **Limit:** `cargo xtask tokens` flags a colour literal or a font size outside `theme/`, but a token turned to an opacity there (`gamma_multiply`, `linear_multiply`) passes, and so does `theme::mask`, which is pure white and works anywhere. The colours made so today are the page's own: the form's result strip (entry 60), the 70 % `on_ink` shortcut on buttons and in the result bar, a toast action's edge and the table icon's band.
- **Evidence:** `xtask/src/tokens.rs`; `gamma_multiply` in `crates/ui-egui/src/conn_form.rs`, `components/button.rs`, `components/results.rs`, `components/toast.rs` and `components/icon.rs`. A probe file in the M3 Phase 2 review held three such lines, and only `Color32::from_rgb` was flagged.
- **Impact:** none seen today; a colour that is no token could pass the check unnoticed later.
- **Estimate:** 1 h (estimated): name the opacities in `theme/`, flag `gamma_multiply(` and `linear_multiply(` outside it, and let only the logo's drawing call `theme::mask`. Not tied to a screen.

### 70. Postgres: a server that stops answering after a result holds the run for up to 15 s (D)

- **Limit:** after a run, a COMMIT or a ROLLBACK, dbee reads the session's transaction state from the ReadyForQuery that ends the server's answer. If the server stops answering right after the result, so that ReadyForQuery never comes, the run keeps its spinner and CANCEL up to 15 s longer, and the state then reads `unknown`; Cancel doesn't shorten the wait. tokio-postgres ends every batch with a Sync, so a server that answers normally never gets here.
- **Evidence:** `SETTLE_LIMIT` and `state` in `crates/pg/src/tap.rs`, awaited outside the run's stop in `crates/engine/src/cmd/query.rs` (the run, and COMMIT and ROLLBACK).
- **Impact:** small: only with a server or a proxy that hangs in the middle of an answer.
- **Estimate:** 1 h (estimated), in the engine: race the wait against the run's stop, or read the state without waiting once the run is stopped. Not tied to a screen.

### 71. MySQL and MariaDB: IN TRANSACTION stays after the server dropped the session (D)

- **Limit:** MySQL and MariaDB say whether a transaction is open only in the packets they send, so when the server ends the session (its `wait_timeout`, a restart) the status bar goes on saying IN TRANSACTION, though the server rolled the transaction back. The next run or COMMIT finds out; a COMMIT then fails with the general lost-connection words, not the "nothing was committed" a connection already known to be lost gets. Postgres notices at once, through its connection task.
- **Evidence:** `SERVER_STATUS_IN_TRANS` read from each packet (`crates/mysql/src/connect.rs`, `crates/mysql/src/result.rs`); `Connected::transaction` (`crates/engine/src/state.rs`) hides the state only once the connection knows it is closed; `COMMIT_SESSION_LOST` (`crates/engine/src/cmd/query.rs`) covers only that case.
- **Impact:** small but misleading: the footer shows a transaction that is gone, and a COMMIT's error doesn't say that nothing was committed.
- **Estimate:** 1 h (estimated) for the COMMIT's words, in the engine; whether to ping the server while a transaction is open goes with the Shell screen (M3 Phase 3).

### 72. A toast's close and action can't be reached with the keyboard (U)

- **Limit:** a toast's close (×) and its action button answer the pointer only; Tab never reaches them. Nothing is lost today: a toast goes after 4 s, and before M3 it had no close.
- **Evidence:** `Sense::CLICK` on both in `crates/ui-egui/src/components/toast.rs`.
- **Impact:** once toasts carry actions, a keyboard user can't take them.
- **Estimate:** part of the Shell screen (M3 Phase 3), which uses toast actions: a shortcut, or focus when the toast shows; not estimated.

### 73. The control channel: a click sent right after one that moves things lands in the old place (S)

- **Limit:** `ui.click {id}` clicks the middle of the place the id had in the last frame drawn. Sent right after a click that changes the layout (a tree row opened), before a frame has drawn the new layout, it lands where the widget was: in the M3 Phase 2 review a click on `customers` selected `order_items`.
- **Evidence:** `click` in `crates/ui-egui/src/control.rs`; `ui.click` in `docs/control-protocol.md`.
- **Impact:** an agent can act on the wrong row; an `ui.inspect` between the two clicks avoids it.
- **Estimate:** 1 h (estimated): draw a frame before a click by id when the input before it changed the layout, or find the id again then. Not tied to a screen.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | minor | Entry 58 done and dropped: the cells a successful Apply wrote show the `saved` mark at full for 1 s, then it fades out over 0.5 s; the engine finds them on the reloaded page by their rows' keys (`saved` in `document.inspect`), and editing the cell, another Apply, another page load, a reconnect, a reset, undo or redo takes the mark off at once. The entries after it keep their numbers. |
| 2026-11-13 | minor | Entry 14 done: closing the window asks too (the transaction prompt, or `Quit anyway?` for running work, query text or staged edits), each connection in turn, and closes at once with nothing to lose; what stays is the shared session, a limit, since one session per tab is not planned for M3. |
| 2026-11-13 | minor | Entry 14: the window asks in the transaction prompt (COMMIT, ROLLBACK or CANCEL) when the owner tab closes or the connection closes or changes, toasts the outcome, and a switch waits for the end; only quit is left. |
| 2026-11-13 | minor | Entry 14: closing the tab that opened a transaction, or its connection, asks how to end it (`needsTransactionEnd`), and `end` on `tab.close` and `connection.close` ends it before closing; the window's prompt is left. |
| 2026-11-13 | minor | The M3 Phase 2 review: entries 14, 53 and 54 say what is left now that the status bar shows IN TRANSACTION and the error band is built; new entries 67 (cut names and types without a hover), 68 (two controls past their edges at the smallest window), 69 (the tokens check misses a token at an opacity), 70 (Postgres: a silent server holds a finished run up to 15 s), 71 (MySQL and MariaDB: a stale IN TRANSACTION), 72 (a toast can't be reached with the keyboard) and 73 (a click by id right after a layout change); entry 24's name field, entry 61's filter row, and entry 56's test of a cancelled Postgres connect after the login. |
| 2026-11-11 | minor | Entry 66: dbee draws the names in its own dialog and toast sentences in `data`; names inside the engine's messages stay in `body` until the engine gives them as parts. |
| 2026-11-11 | minor | Entry 56: `connection.cancel` stops the attempt of an open, an openFile, a reconnect or a test, closing what it had opened, the SSH tunnel included; the CANCEL button beside `CONNECTING` is left for the connection screens. |
| 2026-11-10 | minor | Entry 59 decided (the update button's states and where it sits) and entry 65 (no Help menu or About). |
| 2026-11-10 | minor | Entries 14 and 53: the connection's `transaction` says whether the query session has one open, as each driver reports it, and which tab's run opened it; `query.commit` and `query.rollback` end it. The footer mark and the prompt on close are left for the Shell screen, the ROLLBACK button for the Query tab screen. |
| 2026-11-10 | minor | Entry 54: the engine reads only its own cancel as cancelled, and a statement the server stopped by itself is a server error marked `interrupted`; the M3 error band for it is left for the Query tab screen. |
| 2026-11-10 | minor | The check of the M3 Phase 1 fixes: entries 63 (a long column type may be cut in the grid header) and 64 (the tokens check can misread a test helper with a comma in its type). |
| 2026-11-10 | minor | The M3 Phase 1 review: entries 60 (the connection form's result strip uses a tint that is no token), 61 (three texts cut at the smallest window, older than M3) and 62 (View > Theme can't be reached before connecting). |
| 2026-11-09 | minor | Entry 60 fixed and dropped: the prompt after an unknown Apply reads "Apply again?" with an "Apply again" button, and "Close anyway" stays for the other prompts. |
| 2026-11-09 | minor | Entries 53 to 60: what the M3 design draws that the code can't do yet (rollback, a cancel dbee didn't send, the structure view, cancelling a connect, last-used order, the saved cell's mark, the update button), and the "Close anyway" prompt that applies again. Entry 1 names what M3 draws for it. |
| 2026-11-08 | minor | The security review's lane C fix round: entries 50 (a client with no token can keep the 16 places), 51 (two small control channel test gaps) and 52 (the log's end-to-end test); entry 46 says OpenSSH's fall-back to a line with no port and `!` exclusions are not done; entry 47 names the right crate. |
| 2026-11-08 | minor | The security review's lane C, job 3: entries 46 (`/etc/ssh/ssh_known_hosts` is not read), 47 (Windows Credential Manager entries roam), 48 (macOS ad-hoc signing, channels an SSH server opens) and 49 (the control channel answers only while the window draws); entry 11 says the Flatpak keeps no trusted host key. |
| 2026-11-08 | minor | The security review's lane A, fix round 1: entry 45 (tokio-postgres keeps every ParameterStatus a server sends). |
| 2026-11-08 | minor | The security review's lane B, last two fixes: entry 38 says `SET CHAR SET` is refused and a line comment ends at a line feed only on MySQL, MariaDB and SQLite, lists the harmless statements read-only refuses, and says lane B is closed. |
| 2026-11-08 | minor | The security review's lane A: entries 41 (a tunnelled cancel names the host again), 42 (a server can keep a statement running forever), 43 (MySQL takes the server's RSA key without a word under `disable` or a tunnel) and 44 (the password held as a plain `String`). |
| 2026-11-08 | minor | The security review's fix round 5, adjudicated fix: entry 38 says the raw rules find `SET` also after a version comment's number, which had hidden a lookalike name, while `_` and `$` stay part of a name (`SELECT @@character_set_client` runs). |
| 2026-11-08 | minor | The security review's fix round 5: entry 38 says a read-only session keeps `sql_mode` and the character set as at connect and the guard reads `"…"` as the session does, and that a version comment glued to `SET` still hides a lookalike name; 27 puts a function that changes `sql_mode` or the character set out of reach; 40 says read-only refuses `SET NAMES utf8mb4` too. |
| 2026-11-08 | minor | The security review's fix round 4: entry 38 says a read-only connection keeps the character set at utf8mb4 and sets the tracking up again after a caught change, and that under `ANSI_QUOTES` a MariaDB block can still slip past the check and write; 27 adds a statement that slips past the check after a function turned the tracking off; 40 (another character set on a read-write connection). |
| 2026-11-08 | minor | The security review's fix round 3: entry 38 says a multi-byte character set or `ANSI_QUOTES` can move the guard's message after the statement, while the server's report still stops the write, and that blocks and names outside ASCII are refused; 39 adds a trigger that sets `client_encoding` back before `COMMIT`; 27 names blocks of several statements. |
| 2026-11-08 | minor | The security review's fix round 2: entry 38 now says what MySQL's and MariaDB's read-only rests on (the server's report through session tracking) and what an error leaves out; 27 adds a function that turns the tracking off; 39 notes Apply's second check of `client_encoding`, before `COMMIT`. |
| 2026-11-08 | minor | The security review's fix round 1: entry 27 refuses more (`ALTER SYSTEM`, `COPY … TO`, `INTO OUTFILE`, `SET GLOBAL`) and names the built-ins that run SQL text and the statements that write no table data; 37 says `NOFOLLOW` does nothing on Windows; 38 (MySQL and MariaDB: read-only is read back with a statement of dbee's own, where session tracking would do) and 39 (Postgres: a filter under another `client_encoding`). |
| 2026-11-08 | minor | The security review's lane B: entry 27 now says what read-only refuses and reads back, and what runs inside one statement out of its reach; entries 36 (SQLite: a file from the file manager opens read-write, values up to 1 GB) and 37 (isolated runs: a parent folder swapped in during a SQLite open). |
| 2026-11-08 | minor | Entry 6: the engine holds its open connections by id and each tab names its connection; one may be open. |
| 2026-11-08 | minor | Entry 11 now says the Flatpak reads `~/.ssh` and the agent and lacks only the keyring; entry 35 (a catalog that takes more than 60 s cannot be opened); import and export are M4 (M3 is the Redesign). |
| 2026-11-08 | trivial | Entries 19 and 20 no longer point to the private M2 plan. |
| 2026-11-06 | minor | M2's final review: entry 14 now says query tabs share one session, whose transaction outlives a tab (and MySQL's DDL commits it); entries 25–34: MySQL's `CALL` results and warning count, read-only undone in a query tab, idle timeouts, refused login methods, `lower_case_table_names`, MariaDB's system-versioned tables, the catalog read without one snapshot, no `COM_QUIT`, SQLite's case folding. |
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
| 2026-11-08 | trivial | Entry 14 is planned for M3, with the redesign. |

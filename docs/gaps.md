# Gaps

> **Last reviewed:** 2026-11-08 · **Last updated:** 2026-11-08 · **Change:** minor (entry 38: a read-only session keeps `sql_mode` and the character set, the guard reads `"…"` as the session does, and a version comment glued to `SET` still hides a lookalike name; 27: a function that changes `sql_mode` or the character set; 40: `SET NAMES utf8mb4` refused on read-only)

Ranked by user impact; pick from the top. Each entry says what is missing or limited, the evidence a reviewer can rerun, who it hurts and an estimate. Hours are agent wall-clock hours, **estimated**. Timings are **measured**, in a dev build (the workspace's dev profile: dbee's crates at opt-level 1, dependencies at 2). They are quoted from [`target-app-parity.md`](target-app-parity.md), "Performance", which holds every measurement with its machine, runs and load averages.

Kinds: F feature, U UI, D database engine, FF file format, H hardware, L localization, P performance, S stability.

## Alpha blockers

### 1. No export or import of files (FF)

- **Missing:** exporting a table or a query result to a file (CSV, JSON, SQL) and importing one. The alpha gate's last row ([`roadmap.md`](roadmap.md)).
- **Evidence:** `engine.commands` lists no export or import command; the way out today is the clipboard, at most 8 MiB of text (entry 8).
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

- **Limit:** every query tab of a connection runs on one query connection, so they share one session: its transaction, its `SET`s and its temporary tables. A transaction a tab opened (`BEGIN`) stays open when the tab closes, and another tab's `COMMIT` or `ROLLBACK` ends it. On MySQL and MariaDB a statement that commits on its own (`CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `LOCK TABLES` and others) in any tab commits it, without a word. On Postgres a script that fails inside a transaction leaves the session in that failed transaction: every later run, in any tab, fails with SQLSTATE 25P02 until a `ROLLBACK`. dbee does not end it, since it cannot know whether the user means to roll back to a savepoint instead; under a 25P02 error the card adds that a ROLLBACK ends it. MySQL, MariaDB and SQLite keep a transaction going after a failed statement, so there that part does not arise.
- **Evidence:** the query connection (`Connected::query`, `crates/engine/src/state.rs`) is one per connection, and closing a tab ends no transaction (`crates/engine/src/cmd/tabs.rs`); `a_failed_transaction_says_a_rollback_ends_it` (`crates/ui-egui/tests/query_tab.rs`); `ROLLBACK_NOTE` (`crates/ui-egui/src/query_tab.rs`).
- **Impact:** an uncommitted change made in a tab that was closed can be committed later by another tab, by its `COMMIT` or, on MySQL and MariaDB, by its DDL. A reconnect ends the session and its transaction (rolled back).
- **Estimate:** 2–4 h (estimated): each driver reports after a run whether a transaction is open (Postgres's ReadyForQuery status, MySQL's `SERVER_STATUS_IN_TRANS`, SQLite's autocommit flag), the query tab shows it, and closing the tab or switching connections asks whether to commit or roll back. Waits for the maintainer's call.

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

- **Limit:** at the window's minimum size (640×420) the form under the title row is too narrow: the TLS mode buttons and Save, Test and Connect run past the card's edge, Test covers part of Save, and the name field shrinks to about 30 points.
- **Evidence:** the connection form's layout (`crates/ui-egui/src/conn_form.rs`), seen at 640×420. At the default size (1100×720) and above everything fits (`crates/ui-egui/tests/connections.rs`).
- **Impact:** small: only in a window made as small as it goes. A wider window shows the form whole.
- **Estimate:** part of the connection screen's redesign; not estimated.

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

- **Limit:** on a read-only MySQL or MariaDB connection the server reports, in each statement's own OK packet, a change of the read-only variable and of the transaction's characteristics (session tracking), so a statement that turns read-only off is caught right after it ran, however its text was spelled, also through `PREPARE` and `EXECUTE`, `EXECUTE IMMEDIATE`, `CALL` and a function. Not covered by the report: an error packet carries none, and MariaDB drops a variable's change made before its error, so after an error dbee reads the variable before the next statement that is not a diagnostic one (one more round trip after each error); MariaDB's `SET STATEMENT … FOR` changes the mode for its one statement before any report, so it is refused from the raw text, as are a block of several statements and a `SET` with a name outside plain ASCII; and a function can turn the tracking off (entry 27); when dbee catches a change, it sets the tracking up again. A read-only session keeps `sql_mode` and the character set as they were at connect (a statement that holds `SET` beside `sql_mode`, or beside a word that changes the character set, is refused from its raw text), and the check reads `"…"` as that session does, as a name under `ANSI_QUOTES`: under `sjis` or `gbk`, and under `ANSI_QUOTES`, the server had read a MariaDB block's `;`s as code where dbee saw them inside a string, and the block wrote (measured on both MariaDB servers, security review fix rounds 4 and 5). Still open: a version comment's number glued to `SET` (`/*!40101SET …*/`) hides it from the check for names outside ASCII, so a name that MySQL 8.0 and later or MariaDB 13 folds to `sql_mode` or `session_track_system_variables` changes the mode or turns the tracking off; on MariaDB 13 a block then wrote (measured, fix round 5).
- **Evidence:** measured on all five servers (security review of 2026-11-08, fix round 2): `SET @@transaction_read_only = 0` and `START TRANSACTION READ WRITE` reported as characteristics `SET TRANSACTION READ WRITE;` and `START TRANSACTION READ WRITE;`, a function's or procedure's `SET SESSION … = 0` as the variable `OFF`, also behind `PREPARE`, `EXECUTE` and `EXECUTE IMMEDIATE`; after `SELECT f(), (SELECT 1 UNION SELECT 2)` MySQL reported the change with the next OK packet and MariaDB never did, while `SELECT @@SESSION.…` read `0` on both. `the_servers_report_catches_what_the_text_check_misses` (`crates/mysql/src/guard.rs`) sends each form the reviews found with the text check skipped; `read_only_mysql_keeps_the_character_set_utf8`, `read_only_mysql_keeps_sql_mode`, `read_only_mysql_reads_double_quotes_as_the_session_does` and `read_only_mysql_sets_the_tracking_up_again_after_a_caught_change` (`crates/engine/tests/query.rs`).
- **Impact:** a round trip after each error on a read-only connection. On MariaDB 13, a lookalike name behind a glued version comment and a block after it write on a read-only connection; so can a statement that slips past the check after a function turned the tracking off (entry 27).
- **Estimate:** none for the read after an error, which closes what the report leaves open; under 1 h (estimated) to find `SET` after a version comment's number in the check for names outside ASCII, as the `sql_mode` check does.

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

## Revision history

| Date | Change | Summary |
|---|---|---|
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

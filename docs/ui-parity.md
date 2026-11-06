# UI parity

> **Last reviewed:** 2026-11-06 · **Last updated:** 2026-11-06 · **Change:** minor (the grid's keyboard) · **Target:** Beekeeper Studio 5.5.7

Graded by behaviour against what was observed in Beekeeper Studio, not by whether a control exists. The evidence is in observation notes kept outside the repository (they hold Beekeeper Studio screenshots, which are not ours to publish). Status: **done** (does what the observed element does, on every engine dbee has: PostgreSQL, MySQL, MariaDB and SQLite), **partial** (some of it), **not started**. dbee's styling and wording are its own. Every element below that dbee has was driven through the control channel and checked on screenshots (`apps/dbee/tests/m1_day.rs`, `apps/dbee/tests/m2_day.rs`, `crates/ui-egui/tests/`).

| Element | Beekeeper Studio | dbee | Status |
|---|---|---|---|
| Window frame and menus | Own title bar; File, Edit, View, Tools, Help menus | Native frame. A menu bar built from the commands' menu paths (File, Edit, View, Query), with shortcuts; a disabled entry says why | partial: no Tools or Help menu |
| Connection lists | Saved and recent lists with a filter, a colour bar, an engine pill; a right-click menu | Saved and recent lists with a filter and the colour bar, the database type's name under each connection's name; Connect, Edit, Duplicate, Delete on right-click; double-click connects | partial: no Copy URL |
| Connection form | Engine type, auth method, socket mode, host and port, user and password, database, SSL with CA, client cert and key, SSH tunnel with jump host and keepalive, read-only, name, colour, save passwords, Fill from URL, Test, Connect, Save | The database type first (PostgreSQL, MySQL, MariaDB or SQLite, as cards; a URL picks it too), then its form. Servers: host and port, user and password (show), database (typed, or chosen from the server's list under the field), SSL modes with a CA file, read-only, the type's options, SSH tunnel (host, port, user; key found on its own, agent or password). SQLite: the file (choose one, or a new one), read-only. Every type: name, seven colours or none, Test, Connect, Save; save passwords for the servers; Fill from URL, except on a SQLite connection opened from the list | partial: no socket mode, IAM, client certificate, jump host or keepalive |
| Connect prompts | Asks for what is missing | Dialogs for an unknown host key, a key passphrase, the SSH and database passwords, confirmations | done |
| Schema tree | Database switcher; schemas, tables, views, columns with types; filter, refresh, expand and collapse all, pin, hide | Schemas, or databases on MySQL and MariaDB (the system ones grouped), the default one first and open; tables, views and materialized views with counts, columns with type and key; filter and refresh | partial: no database switcher, pin, hide or expand-all |
| Quick search | Ctrl+P overlay, `schema.table`, match in bold | Ctrl+P overlay, best match first, arrows and Enter, within a frame on 1,000 tables and 100 saved connections. It finds the saved connections too (with their colour and type; on the connection screen only them; by any word of their names), and choosing one switches to it through the disconnect questions. The list scrolls when it doesn't fit the window | done |
| Tabs | Icons per kind, a dot for unsaved changes, close, +, reopen closed, restored on reconnect | Table and query tabs with icons, the applied filter count in the title, a dot for unsaved text or staged edits, close (also middle-click), + | partial: no reopen closed; tabs stay through a reconnect but are not restored on the next connect |
| Status bar | Connection, per-tab counts and timings | Connection name in its colour (read-only marked), the server's product and version (MariaDB as MariaDB), the running job with Cancel, a lost connection with Reconnect, the value panel switch | partial: counts live in the tab's own bars |
| Filter bar | Column, operator, value; + adds; raw SQL toggle; 2 filters free | The same, 13 operators (a hover says what one does differently on the engine, such as MySQL's collation), any number of filters, raw `WHERE` toggle, Apply or Enter | done |
| Data grid | Type labels, key icons, sort, NULL and empty told apart, line-break glyph, tooltips, column resize modes, hide and reset columns, column filter | Type labels, primary and foreign key icons, sort by header, NULL and empty told apart, line-break glyph, a selection rectangle, column resize, the foreign key link | partial: no hidden columns, column filter or cell tooltips; slow past about 100 columns ([gaps.md](gaps.md) 5) |
| Grid keyboard | Not observed | Click and Shift+click select; Enter opens the selected cell's editor, and in it Tab and Shift+Tab go to the next and previous cell; Ctrl+A and Ctrl+C on the grid | partial: no arrow keys to move the selection, and a selected cell at the edge of the view is not scrolled into view |
| Paging bar | Page box, row count on demand, refresh, + row, Reset, Apply with a count | First, previous, page, next, last; the estimate then the exact count; refresh; + Row; the changes, Reset, Apply; gives room up in order when narrow | partial: no page box to type a page into |
| Inline editing | Editor in the cell, staged colours, Apply and Reset, add, clone, delete rows, set NULL | One-line, multi-line (text, json, jsonb, xml) and true/false/NULL editors; Enter, Tab, Escape; staged, new, deleted and failed colours; add and delete rows; set NULL; Ctrl+S | partial: no clone row, no copy of changes as SQL; 32 KiB per cell ([gaps.md](gaps.md) 10) |
| Grid right-click menu | Set NULL, copy as TSV, JSON, Markdown, SQL, IN list, column name, quick filter, clone, delete | Set NULL, copy (values, with header, CSV, JSON, Markdown), delete row | partial |
| Value side panel | JSON row viewer (paid) | The selected cell's whole value, JSON pretty-printed and coloured, select and copy; up to 10 MiB shown | partial: the cell, not the row; read-only |
| Edit in a modal | Text, JSON, HTML; Minify & Apply | None | not started |
| Query editor | Line numbers, syntax colours, autocomplete, find and replace, Vim keymap, wrap | Line numbers, syntax colours in the engine's own SQL, Ctrl+Enter (all), Ctrl+Shift+Enter (the statement under the cursor), Tab indents, a draggable split; on MySQL and MariaDB the database a tab runs in, opened on the tree's selection and chosen from a list | partial: no autocomplete, find, Vim keymap or wrap |
| Query results | Result selector, row count, affected rows, timing, error card, download | Result selector, row count with the cap notice, rows affected, error card that jumps to the position, cancel | partial: no timing or download |
| Transactions | Auto commit or manual (Begin, Commit, Rollback) | Auto commit only | not started |
| Saved queries and history | Side panels | None | not started |
| Structure and new table | Columns, indexes, relations, triggers; table editor | None | not started |
| Notifications | Toasts with actions; export progress | Notices at the bottom right ("Copied 13 rows", why something cannot run) | partial |
| Themes | System, light, dark, Solarized | One light theme of dbee's own | partial: dark theme later |

By count: 3 of 23 elements done and 16 partial, which at half weight is 48 % (estimated). Screens not observed yet (settings, plugins, backup and restore) are not in the list, so the UI dimension is put at **~45 % (estimated)**.

## Work

From [gaps.md](gaps.md): wide results first (5), then the editing limits (9, 10). Not estimated as a whole yet.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-06 | minor | Grid keyboard (partial): no arrow keys, and a cell at the view's edge is not scrolled into view; 3 of 23 done, 16 partial, 48 % by count; ~45 % stands. |
| 2026-11-05 | minor | Graded on every engine: the M2 day drives the same elements on MySQL, MariaDB and SQLite (`m2_day`); grades unchanged. |
| 2026-11-05 | minor | Quick search: Ctrl+P finds saved connections (by any word of their names) and switches to one, also on the connection screen; the list scrolls when it doesn't fit the window (M2.19, issue #3); grades unchanged. |
| 2026-11-05 | minor | Connection form: the Database field lists the server's databases to choose from (M2.18, issue #1); grades unchanged. |
| 2026-11-05 | minor | The workspace on every engine (M2.16): the engine's word for schemas, the default schema first, a query tab's database on MySQL and MariaDB, operator notes, the product in the status bar; grades unchanged. |
| 2026-11-05 | minor | Connection form and lists: every database type (M2.15); grades unchanged. |
| 2026-10-27 | trivial | The observation notes are cited as kept outside the repository, not by their local paths. |
| 2026-10-27 | minor | gaps.md references renumbered (the generated-columns entry is fixed and gone): the editing limits are 9 and 10. |
| 2026-10-27 | minor | Connection lists (no Copy URL) and the paging bar (no page box) regraded partial: 3 done, 15 partial, 48 % by count; ~45 % stands. |
| 2026-10-26 | major | M1 built: 22 elements graded against the observed app; ~45 % (estimated). |
| 2026-10-15 | major | Template (scaffold). |

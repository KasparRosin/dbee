# Decisions

> **Last reviewed:** 2026-11-08 · **Last updated:** 2026-11-08 · **Change:** minor (the security review's fix round 1: dbee's own Postgres operators, dollar-quoted values, executable comments, read-only's further refusals, trusted_schema back on)

Significant decisions, newest last. Add an entry for every significant dependency, platform or process choice.

## 2026-10-15: Name `dbee`, app ID `dev.rosin.dbee`, homepage `https://dbee.rosin.dev`

- **Decision:** The product is `dbee` (lowercase everywhere), with app ID `dev.rosin.dbee` and homepage `https://dbee.rosin.dev`.
- **Why:** One stable lowercase name keeps binaries, package ids and the website consistent.

## 2026-10-15: Desktop only, no browser build

- **Decision:** dbee ships as a native desktop app. There is no web build and no wasm target.
- **Why:** Browsers cannot open database connections (raw TCP to PostgreSQL, MySQL or SQL Server).

## 2026-10-15: Full platform matrix

- **Decision:** Linux x86_64, aarch64 and riscv64; macOS universal; Windows x64, x86 and arm64; FreeBSD.
- **Why:** A database client is used on whatever machine the developer has, and a pure-Rust stack makes the wide matrix cheap.

## 2026-10-15: Signing is optional

- **Decision:** Builds ship unsigned until signing secrets exist; the workflows warn instead of failing.
- **Why:** Certificates cost money and need the owner's accounts; the pipeline must work before they exist.

## 2026-10-15: Self-contained workflows

- **Decision:** The workflows use no shared tooling from outside this repository; third-party actions are pinned to commit SHAs.
- **Why:** The repository must build and release on its own, and it should be easy to audit.

## 2026-10-15: GitHub repository created when version 1 runs

- **Decision:** Until version 1 runs, there are local commits only; the repository `KasparRosin/dbee` is created then.
- **Why:** The pre-alpha scaffold has nothing to publish, and the history can be tidied before it is public.
- **Superseded:** 2026-10-27, by "GitHub repository after M1" below.

## 2026-10-15: Rust 1.99 minimum

- **Decision:** `rust-version = "1.99"` in `[workspace.package]`, edition 2024.
- **Why:** A single recent toolchain keeps the code modern and the support matrix small.

## 2026-10-15: Licence MIT OR Apache-2.0

- **Decision:** dbee is dual-licensed `MIT OR Apache-2.0`.
- **Why:** The common permissive pair for Rust projects; contributors and users may pick either.

## 2026-10-15: Owner Kaspar Rosin, an independent project

- **Decision:** The owner and publisher is Kaspar Rosin. dbee is an independent project and not part of any app family.
- **Why:** One accountable owner for releases, signing and the licence line.

## 2026-10-15: Target app Beekeeper Studio 5.5.7, observed black-box only

- **Decision:** The parity target is Beekeeper Studio 5.5.7. It is only observed from outside, as a user would (clean room): its source, bundle, settings and traffic are never read.
- **Why:** dbee must be original work, and a black-box target keeps the licence of dbee clean.

## 2026-10-19: Docker test servers, Postgres 14 + newest

- **Decision:** Driver and SSH tests run against local Docker containers that `cargo xtask testdb up` starts (compose project `dbee-testdb`): Postgres 14, the oldest supported major, and the newest released major (the `pgnew` row of `SERVICES` in `xtask/src/testdb.rs`, 18 on 2026-10-19 because `postgres:19-alpine` was not yet published), both with TLS and the same synthetic seed, plus an SSH server for tunnels. Ports are random on `127.0.0.1`, the password and key passphrase are generated, and `target/testdb/env.json` tells the tests where everything is. Without Docker the tests print `SKIPPED`; `DBEE_REQUIRE_ORACLES=1` makes that a failure.
- **Why:** Testing the oldest and newest majors catches catalog and type differences across the supported range, and a fixed seed gives every test the same odd identifiers, exotic types, a million rows and a 20 MB cell. Containers keep tests away from real servers, `~/.ssh` and the SSH agent, and one compose project is easy to start, inspect and remove.

## 2026-10-19: Secrets in the OS keyring via keyring-core, pure-Rust Secret Service

- **Decision:** Passwords and key passphrases are stored through `keyring-core` 1 with one store per platform: `zbus-secret-service-keyring-store` (feature `crypto-rust`) on Linux and FreeBSD, `apple-native-keyring-store` (`keychain`) on macOS, `windows-native-keyring-store` on Windows. The connections file never holds a password. Tests and agent runs use an in-memory store.
- **Why:** The zbus store with pure-Rust crypto needs neither `libdbus-sys` nor OpenSSL, which the licence and build rules forbid. Without a Secret Service the store reports an error and the UI offers to continue without saving.

## 2026-10-19: russh with ring for SSH tunnels

- **Decision:** SSH tunnels use `russh` 0.64 in-process, with default features off and `ring`, `rsa` and `flate2` on, over `tokio`. Host keys are checked against `~/.ssh/known_hosts` with russh's `check_known_hosts_path`: an unknown host is refused with its SHA256 fingerprint until the user accepts it, and is then learned; a changed key is refused and cannot be overridden. Logins use the agent, a key file (with its passphrase) or a password. The `rsa` crate carries advisory RUSTSEC-2023-0071 (the Marvin timing side channel); it matters little for a client that signs with its own key locally, and nothing else in dbee uses RSA decryption.
- **Why:** A library tunnel needs no `ssh` binary on the machine, works the same on every platform (including the Windows agent pipe), and reports each failure as a typed error the UI can turn into a prompt (host key, passphrase, password). `ring` keeps `aws-lc-sys` and `aws-lc-rs`, which the build rules forbid, out of the tree.

## 2026-10-19: tokio-postgres over the simple-query text protocol (values exactly as printed, no prepared statements for data)

- **Decision:** `dbee-pg` talks to Postgres with `tokio-postgres` 0.7 on the shared `tokio` runtime. Data travels only over the simple-query protocol, so every value arrives as the text Postgres prints for it (`1.50`, `2024-01-02 03:04:05`, `{"a": 2, "b": 1}`), and dbee shows that text unchanged. No prepared statement carries data: values dbee writes or filters on go into the SQL text as quoted literals with an explicit cast, which is safe because every session runs `SET standard_conforming_strings = on` and tokio-postgres always starts with `client_encoding=UTF8`. Connections through an SSH tunnel use `Config::connect_raw` on a tunnel channel; cancel requests go the same way. Server notices are not logged, since they can quote data.
- **Why:** The binary protocol would need a decoder per type and would still re-format values (numeric scale, time zones, intervals, exotic and user-defined types); the text protocol is exact for every type, including ones dbee does not know. tokio-postgres is pure Rust, MIT/Apache, maintained, and lets the caller supply the stream and the TLS layer.

## 2026-10-19: rustls with ring and the OS certificate verifier

- **Decision:** TLS uses `rustls` 0.23 (default features off; `ring`, `std`, `tls12`) through `tokio-postgres-rustls` 0.14 (`ring`). The modes follow libpq: `disable`; `prefer` and `require` without verification (`prefer` falls back to plain when the server has no TLS or the handshake fails; `require` checks the chain when a CA file is set); `verify-ca` checks the chain against the CA file only, and is refused without a CA file; `verify-full` checks the chain and the configured host name, also through a tunnel. The roots are the CA file's certificates when one is set, otherwise (`verify-full` only) the operating system's through `rustls-platform-verifier` 0.7. `verify-ca` against the system roots is refused on every platform, as libpq refuses it: a chain check alone would accept any publicly trusted certificate for any host. `tokio-postgres` is pinned exactly (`=0.7.18`) because dbee-pg classifies its errors by their texts.
- **Why:** `ring` keeps `aws-lc-sys` and `aws-lc-rs` out of the build (the build rules forbid them), and the platform verifier uses the trust store and policies the user already manages instead of a bundled root list (`webpki-roots` is forbidden too).

## 2026-10-20: egui_extras for the grid, egui_kittest for UI tests

- **Decision:** `crates/ui-egui` takes `egui_extras` 0.36 (default features off: no image loaders, no syntax highlighting) for its table layout, and `egui_kittest` 0.36 as a dev-dependency, which runs `DbeeApp::ui` frame by frame without a window. Both are from the egui project, versioned with egui, MIT OR Apache-2.0.
- **Why:** The result grid needs virtual scrolling over thousands of rows with resizable columns, which `egui_extras`' table gives; writing our own would be a second grid to maintain. kittest drives the real UI code in tests (frames, input, accessibility queries) with no display, so the UI tasks get tests that run in `cargo xtask ci`.

## 2026-10-23: rfd file pickers through the XDG desktop portal, no GTK

- **Decision:** `crates/ui-egui` opens file pickers (the SSH key's "Change…", the CA file's "Choose…") with `rfd` 0.17, default features off and `xdg-portal` and `wayland` on (MIT). On Linux and the BSDs that asks the XDG desktop portal over D-Bus, through the system's `libdbus-1` loaded at run time (no `libdbus-sys` crate), and falls back to `zenity` when there is no portal; macOS and Windows use their native pickers. The picker runs on a thread of its own and sends the chosen path back to the UI thread, so the window keeps drawing while it is open on Linux, the BSDs and Windows; on macOS rfd shows the panel modally on the main thread, so the window waits while it is open, as it does for any macOS open panel. When no picker can open, the form says so under its buttons instead of waiting. Tests and agents never open it: they set a key path by saving a connection.
- **Why:** rfd's `gtk3` backend would bring GTK, which the build rules forbid, and a picker written for egui would be a second file browser to maintain. The portal gives the desktop's own picker, also inside sandboxes such as Flatpak.

## 2026-10-26: The toolchain is pinned in `rust-toolchain.toml`

- **Decision:** `rust-toolchain.toml` names an exact release (`1.99.1`, the minimal profile with clippy and rustfmt), and it is bumped by hand. `rust-version = "1.99"` stays the minimum.
- **Why:** A new compiler invalidates every cached build. With `stable`, an update of the machine's toolchain rebuilt the whole dependency tree in the middle of the work; a pinned release changes only when the repo says so, in a commit of its own.

## 2026-10-27: GitHub repository after M1; changes land through merge requests

- **Decision:** The private repository `KasparRosin/dbee` is created once M1 is built, before version 1, and `main` is pushed to it after the history tidy. From then on changes land on `main` through merge requests, and CI runs on GitHub.
- **Why:** M1 does real work and is ready for review on GitHub; GitHub's runners build the macOS, Windows and BSD releases the local machine cannot. Regular CI builds Linux only.

## 2026-10-28: M2 is three databases, before import and export

- **Decision:** M2 is "Three databases": a driver interface, then MySQL/MariaDB and SQLite with the full M1 workflow. Import and export move to M3 (M4 since 2026-11-08).
- **Why:** The engine still names Postgres types, and export and import should be written once against the interface, not once per engine. Two more databases also widen who can use dbee daily.

## 2026-10-28: SQLite's public-domain code is allowed

- **Decision:** The licence allowlist gains SQLite's public-domain dedication, for the SQLite library compiled in by `libsqlite3-sys` (through `rusqlite` with `bundled`). The crates themselves are MIT.
- **Why:** SQLite is public domain and has no licence text to put on the list. Linking it in keeps dbee one binary with no system library to find.

## 2026-10-30: A driver interface: traits in dbee-driver, drivers at L4, the engine names none

- **Decision:** `dbee-driver` (L3) holds the interface every database driver implements: the `Driver`, `Connection` and `Cancel` traits, one error type (`DbError`), `ServerInfo`, `Target`, the caps, the apply gate, the shared TLS setup and the `CatalogBuilder`. The drivers (`dbee-pg` now; MySQL/MariaDB and SQLite next) sit at L4, and `dbee-drivers` (L4) is their one list, which only the apps and test code use. `cargo xtask layers` keeps the engine and the UIs from taking a driver crate except as a dev-dependency.
- **Families:** A driver serves a family of database types: the MySQL driver serves MySQL and MariaDB, and the Postgres driver can later serve Postgres-like servers. It names the types it opens, and each connection says which SQL dialect its server speaks.
- **Why:** Each engine plugs in the same way, and shared logic (TLS, the apply gate, catalog building) exists once. Adding an engine is one driver crate, one dialect file and one registry line. More exactly, as "Adding an engine" in `architecture.md` lists it: one driver crate, one dialect file, one kind file, one registry line, the enum variants and the layers line.

## 2026-10-30: dbee's own MySQL client on mysql_common

- **Decision:** `dbee-mysql` speaks MySQL's client/server protocol itself. `mysql_common` 0.38 (MIT OR Apache-2.0, default features off: no zlib, no derive macros) gives the packet structs, the login scrambles and RSA; the I/O is dbee's own: TCP or a channel through the SSH tunnel, upgraded to TLS by `tokio-rustls` 0.26 (default features off; `ring`, `tls12` and `logging`; already in the tree through `tokio-postgres-rustls`) with the same rustls verifier as Postgres. Values travel over the text protocol (`COM_QUERY`), as the server prints them. MariaDB is a flavor of the same driver, told by `@@version`, not by the connection's type.
- **Logins:** `mysql_native_password`, `caching_sha2_password` (the fast path; the full path over TLS, or with the password encrypted with the server's RSA key) and `sha256_password` (the same rule). The password is never sent in clear without TLS; any other method (`mysql_clear_password` among them) ends the login before the password is used. Without TLS the RSA exchange keeps the password from a listener, not from a man in the middle, who can hand over a key of their own; TLS or the SSH tunnel protect against that.
- **Hostile servers:** every length, count and offset a server sends is checked, and every `mysql_common` parser dbee feeds server bytes to is fuzzed. Two of them panic on hostile input (the greeting parser on a challenge length of 128 to 135, the RSA key's DER parser on a length past its data), so dbee checks those bytes against the documented layouts before handing them over.
- **Why (checked 2026-10-28):** `mysql_async` 0.37's TLS needs `webpki-roots` (`rustls-tls`) or OpenSSL (`native-tls-tls`), both forbidden, and takes no stream of dbee's, so no tunnel channel; `sqlx` opens its own socket (a loopback forward) and checks certificates against the OS's roots, not through the OS verifier. `mysql_common` has no networking of its own.

## 2026-10-30: MySQL 5.7's TLS is out of rustls's reach; plain direct and SSH work

- **Decision:** The official `mysql:5.7` image offers TLS only with DHE-RSA ciphers on TLS 1.2, which rustls never speaks. dbee tells this from the TLS failure itself (the server's handshake alert), not from the version: `require`, `verify-ca` and `verify-full` fail with a TLS error that says the server offers only ciphers dbee does not support and points to an SSH tunnel, and `prefer` connects without TLS, as libpq's `prefer` does after a failed handshake. Plain direct connections and connections through the SSH tunnel work.
- **Why:** No new TLS stack (OpenSSL and aws-lc stay out) and no 5.7-only code path. The tunnel encrypts the link instead.

## 2026-10-31: MySQL's server key only without encryption or when allowed; per-type connection options

- **Decision:** On a link without TLS, `caching_sha2_password` and `sha256_password` protect the password with an RSA key the server sends, which dbee cannot check. dbee fetches that key when the user chose no encryption (`disable`) or the SSH tunnel encrypts the link, with no warning. When the user wanted encryption and the link ended up plain on a direct connection (`prefer`, with TLS failed or not offered, which an attacker on the link can force), the login is refused before the password is used, unless the connection's `allowPublicKey` option is on; then it goes ahead with a warning on the connection. `require` and the `verify-*` modes always have TLS. dbee follows MySQL's own clients in refusing the key on a link the user wanted encrypted, but allows it when the user chose no encryption.
- **Options:** A database type's own settings are on/off options: `ConnectionConfig.options` (by id, left out of the file when empty, keys a build does not know kept) and `KindInfo.options` (id and label). MySQL lists `allowPublicKey`, and MariaDB inherits it through its row; Postgres and SQLite list none. The connection form shows them from M2.15.
- **Why:** Whoever sits on a plain link can hand dbee a key of their own and read the password. Where the user asked for no encryption, or the tunnel gives it, there is nothing to protect against; where the user wanted encryption, that choice must not be undone silently. One options map keeps engine-only settings off the shared struct.

## 2026-11-01: dbee reads MySQL's column definitions itself; MariaDB's JSON from its extended metadata

- **Decision:** `dbee-mysql` parses every column definition itself, from MySQL's and MariaDB's documented layout, on both flavors, instead of with `mysql_common`'s `Column`: one code path, every length checked against the packet, fuzzed, and held to `mysql_common`'s reading of the same definitions on every test server. From a MariaDB server that offers it, dbee asks for `MARIADB_CLIENT_EXTENDED_METADATA` and reads the extra field only when it was agreed; its format `json` makes a column JSON, and its other keys are skipped.
- **Classes:** A column's class comes from its type, flags, character set and length: `TINYINT(1)` is true/false (every server sends the declared width), strings and blobs in the `binary` character set hold bytes, and so do `BIT`, geometries and vectors, whose values are shown as `0x` and hex rather than as text they would not survive. A type dbee does not know (one a newer server adds) is shown as text, and the queries that touch it go on working.
- **Why:** MariaDB has no JSON type: without the extended metadata a JSON column looks exactly like a `LONGTEXT` with a binary collation, and `mysql_common`'s parser cannot read the definitions once the metadata is on.

## 2026-11-03: rusqlite with the bundled SQLite

- **Decision:** `dbee-sqlite` is built on `rusqlite` 0.40 (MIT) with `bundled`, which compiles SQLite (public domain) into dbee through `libsqlite3-sys` (MIT), and with `column_decltype`, which gives a result column's declared type. Every SQLite call runs on a blocking thread, one operation at a time on each connection; an apply runs one blocking call per statement, so its gate is read between them.
- **Facts measured (SQLite 3.53.2), which the driver follows:**
  - SQLite ends a statement at a NUL outside a string literal and runs the part before it (`DELETE FROM t<NUL> WHERE id = 1` deleted every row); one inside a literal is an error. The driver refuses statement text holding a NUL before SQLite sees it, and the dialect's rule stays that statements cannot hold one.
  - `sqlite3_interrupt` sent while no statement runs has no effect on the next one. dbee sends it only while a call of its own runs, so a cancel never reaches a later call.
  - `CAST(x AS TEXT)` of a real is SQLite's own form, neither the shortest nor a fixed number of digits (`5e-324` prints `4.9406564584124654e-324`), so reals become text through SQLite itself.
  - rusqlite panics on a result column name or declared type that is not UTF-8, which a crafted file's schema can hold. Before it reads them, the driver checks the schemas' names and types and refuses such a result.
  - The bundled build reads `file:` names as URIs even without the URI flag, so the driver opens a file by its absolute path.
  - `PRAGMA quick_check(1)` reads the whole file (about 150 ms for 65 MB). Opening reads `sqlite_schema` instead, which still refuses a file that is not a database or whose schema is damaged; damage elsewhere shows when it is read. A path that is no regular file is refused before it is opened (a FIFO would wait for a writer), the open is bounded by the 15 s connect timeout, and the file is closed on a blocking thread, since closing can write (a WAL checkpoint, a rollback).
  - SQLite ends a transaction itself when an interrupt stops a write in it, and may when a write fails with `SQLITE_FULL`, `IOERR`, `BUSY` or `NOMEM`; a `BEGIN IMMEDIATE` that fails (a locked or a read-only file) starts none. The apply session therefore has its own rollback step (by default it sends `ROLLBACK`): SQLite's reads in the same call whether a transaction is open, and when none is, there is nothing to undo, as on the other engines, and no lost connection.
- **Why:** SQLite in the binary means no system library to find, and the same SQLite on every platform. rusqlite is the established binding; dbee uses it as a library.

## 2026-11-03: SQLite tables without a key are edited by their rowid; isolated runs keep SQLite to their own folders

- **Decision:** An ordinary SQLite table (not a view, a virtual table or a `WITHOUT ROWID` table, which always has a primary key) with no primary key and no NOT NULL unique key is keyed by its rowid. The catalog lists the rowid as a hidden leading column, named by the first of `rowid`, `_rowid_` and `oid` that no column takes (ignoring ASCII case, as SQLite does), and the page reads it beside the visible columns; the grid, sort, filters, copies and the value view never show it, and an insert never writes it. A table whose columns take all three names, and a virtual table, take no edits and say why.
- **Known limit:** SQLite's `VACUUM` may change the rowids of a table without an `INTEGER PRIMARY KEY` (SQLite's documentation of `VACUUM` says so). An edit staged before another process vacuums the file could then find a different row; Apply's one-row rule only catches a row that is gone. Not handled; it is on the security review's list.
- **Decision:** An isolated run (`cargo xtask run-isolated`) lists its folders in `DBEE_ALLOW_DIRS`: its throwaway home and `target/` (with `DBEE_ALLOW_TARGETS` set and `DBEE_ALLOW_DIRS` not, no folder). The folders go to the driver in `Target::allowed_dirs`. A file driver checks its file inside its bounded open, with `dbee_driver::allowed_file` (links followed, `..` gone; a file not there yet by its folder), and opens the path that check resolved, on both connections; a file outside is refused with the `config` code, and nothing is opened or made. The engine checks only what needs no filesystem read: a server's target, and the SSH server any config names; a database file takes no SSH tunnel at all. While folders are set, SQLite's driver installs an authorizer (the `hooks` feature of `rusqlite`) that refuses every `ATTACH`, and `PRAGMA temp_store_directory` and `data_store_directory` with a value (compared by name), with the hint "This isolated run lets SQL open no other file, so ATTACH and VACUUM are refused." Normal runs keep `ATTACH` and `VACUUM INTO`.
- **Facts measured (SQLite 3.53.2):**
  - `VACUUM INTO '<path>'` reaches the authorizer as an `ATTACH` of that path, and a plain `VACUUM` as an `ATTACH` of `''` (its temporary database). Refusing `ATTACH` refuses all three, whatever the name, a `file:` URI and `:memory:` included, with `SQLITE_AUTH` (23); no file is made. dbee's message adds a hint that the run lets SQL open no other file.
  - `SQLITE_LIMIT_ATTACHED` = 0 also refuses all three, as "too many attached databases - max 0"; the authorizer was chosen for its clear code and because it is SQLite's documented way to keep SQL from what it may not do.
  - `ATTACH` opens with the main file's flags, so on a file dbee opened without leave to make one it opens only a file that is there.
  - `load_extension()` is off: "not authorized". The modules compiled in are `dbstat`, `fts3`, `fts4`, `fts5` and `rtree`, which read only the databases on the connection.
  - `PRAGMA temp_store_directory` and `data_store_directory` move SQLite's own files to another folder, process-wide; the authorizer sees them as pragmas with their value.
- **Why:** `rowid` counts as a real key within an edit session (Kaspar's call, 2026-10-28). The isolation rule must hold on every path into a file, and parsing paths is where it would leak, so the driver refuses the statements, not paths.

## 2026-11-05: The connection screen picks the database type first; no engine logos for now

- **Decision:** A new connection starts with one card per database type (`DbKind::ALL`), then the form for the chosen type, with the type and Change engine on its title row (the redesign of 2026-10-31, in the app's present look, its engine row joined to the title row so the form fits the app's first window). A type this build has no driver for shows "Upcoming" and cannot be chosen. The form's rows come from the type's settings and options in `KindInfo`, so the UI names no type. A URL's scheme picks the type. A connection opened from the saved list or Recent keeps its type, and its Fill from URL takes that type's URLs only; one saved during the edit, or a Duplicate, can still change type. A config holds only what its type's settings use. After New file…, every `connection.test` and `connection.open` sends `createFile: true` until a connect succeeds; the engine refuses it for a read-only connection.
- **Logos:** AGENTS.md allows each engine's own logo where its owner's guidelines do, and the redesign shows one on each card. Read on 2026-11-05, none of the owners' pages grants this use. PostgreSQL's fair use covers the names only; it asks that the elephant not be changed or shown with other logos, and other uses need the board's approval. Oracle's allows two badges, "Powered by MySQL" and "Includes MySQL", as links to mysql.com on websites; every other MySQL logo needs a signed agreement. MariaDB's pages limit the Foundation's logo to referring to the Foundation, and grant no other use. SQLite publishes no guideline; asked on its forum, its author said the logo is a registered trademark and that its use depends on how it is used. SQLite's card shows its name too, so the four cards match. A logo can follow once its owner allows it.
- **Why:** The type decides which fields make sense, so it comes first; a connection opened from the list keeps its own (the redesign's E2), while one still being made can be corrected. A logo used against its owner's guidelines is a trademark problem dbee doesn't need, and the name says the same.

## 2026-11-06: MySQL values with a backslash or a NUL are written as hex

- **Decision:** A value dbee writes into MySQL or MariaDB SQL text (an edit, a filter, a key) that holds a `\` or a NUL is written `_utf8mb4 X'…'`, the uppercase hex of its UTF-8 bytes; every other value is `'…'` with `'` doubled.
- **Why:** whether `\` escapes depends on the session's `sql_mode` (`NO_BACKSLASH_ESCAPES`), so a quoted value with one would mean different text on different sessions. Hex reads the same in every mode, and the introducer keeps it a character string, so `LOWER`, `LIKE` and collations treat it as `'…'`.

## 2026-11-06: A NUL outside a string literal is refused on MySQL

- **Decision:** On MySQL and MariaDB a query tab's statement holding a NUL character outside a string literal is refused before it is sent; one inside a literal is sent (it is a character there).
- **Why:** measured on all five test servers, a NUL outside a literal is a syntax error when text follows it, but ends the statement there when only spaces follow, and is ignored inside a `/* */` comment. The server must run what the editor shows, so every such NUL is refused, as on SQLite, which cuts a statement at one (`DELETE FROM t<NUL> WHERE id = 1` deleted every row).

## 2026-11-06: The query connection's database comes from MySQL's session tracking

- **Decision:** The engine keeps the database the query connection is in: the settings' when it opens, each `USE` it sends, and where the server says each statement left it, from MySQL's session-state tracking (`SESSION_TRACK_SCHEMA` in its OK packets). dbee sends no statement of its own to ask. After a run whose text names `session_track` the driver trusts the reports no more on that connection, and the engine sends a tab's `USE` before every run.
- **Why:** the tab's database chip must say where a run goes. A `SELECT DATABASE()` after each run would clear the warnings a user reads next with `SHOW WARNINGS`; a statement that turns the tracking off would leave the reports silent, and a run in the wrong database is worse than a `USE` too many.

## 2026-11-06: Connections-file entries dbee cannot read are kept and written back

- **Decision:** The connections file's saved and recent entries are read one by one. An entry this build cannot read (a database type it does not know, say) is kept as it is and written back on every save, and a warning gives their count; a file that is not JSON at all is moved aside as `connections.json.bad`.
- **Why:** an older dbee must never delete the entries a newer one wrote.

## 2026-11-06: The MySQL browse connection runs in autocommit

- **Decision:** The browse connection's session setup also runs `SET SESSION autocommit = 1`; the query connection keeps the server's mode.
- **Why:** on a server whose default is autocommit off, page reads would share one long `REPEATABLE READ` snapshot (stale pages) and hold metadata locks that block other sessions' DDL.

## 2026-11-06: `LIKE` escapes with a backslash on every engine

- **Decision:** The filter bar's `like`, `not like`, `ilike` and `not ilike` take `\` as the escape on every engine, as Postgres's `LIKE` does by default: MySQL's and MariaDB's patterns end in `ESCAPE _utf8mb4 X'5C'`, SQLite's in `ESCAPE '\'`, Postgres's in nothing. Each is a field of the engine's dialect `Rules`.
- **Why:** measured on the seven test servers and SQLite, `50\%` found "50%" on Postgres, MySQL 5.7 and MariaDB, but nothing on MySQL 8.0 and later (no escape under `NO_BACKSLASH_ESCAPES`) and SQLite (no escape unless one is named). The hex form reads the same whether or not `NO_BACKSLASH_ESCAPES` is set, where `'\'` would not.

## 2026-11-06: Foreign keys are on for every SQLite connection

- **Decision:** dbee runs `PRAGMA foreign_keys = ON` when it opens a SQLite file, so `ON DELETE CASCADE` and the other foreign key actions run, and an edit that breaks a foreign key fails. The setting belongs to the connection, not to the file.
- **Why:** the schema's foreign keys mean what they say, and other database tools do the same. A program that keeps foreign keys off against the same file sees cascades it did not expect after dbee deletes a row.

## 2026-11-06: A cancel is sent again until the statement ends, and never reaches a later one

- **Decision:** the job that runs a statement sends the cancel itself, again after 1 s and then every 2 s while the statement has not ended, and waits for each request to arrive before it goes on. Each driver starts no statement while a request is on its way.
- **Why:** a cancel that reaches the server before the statement does is lost. A cancel that lands on the next statement would stop work the user did not stop.

## 2026-11-06: A file opens as it was saved

- **Decision:** a database file that a saved connection names (links followed, `.` and `..` gone) opens with that connection's name, colour and read-only setting, and a file dropped on the window while connected goes through the questions of closing the open connection.
- **Why:** opening a file that was saved as read-only must not make it writable.

## 2026-11-08: Each change to the connections file re-reads it under a lock

- **Decision:** A save, a delete, a duplicate and the recent list's touch at each open read `connections.json` again, apply only that change to what it holds, write it atomically (temp file, then rename) and keep the result as the session's copy, all under an advisory lock on `connections.json.lock` beside it (std's `File::lock`, no dependency), held for the read and the write only. A lock another dbee keeps for 5 s is an error the user sees, and nothing changes; so is a lock the file system refuses. A file over 4 MiB is not read: the store keeps its changes in memory, as for an unreadable file. The connection screen reads the file again after a disconnect.
- **Why:** one dbee per window (and the file manager starts one per file) each wrote its whole list from memory, so a second dbee's open wrote back an older list over the first one's save. The atomic write prevents a torn file, not a lost update.

## 2026-11-08: The connections file's version, and fields a build does not know

- **Decision:** The file says its `version` (1). Adding a field is free; renaming one, or changing what it means, moves `version` on; a build never writes a file whose version is newer than its own. The version is read before the rest, so a newer file is never moved aside as damaged, whatever its shape: the build reads what it can of it and keeps its changes in memory, with the warning "A newer dbee wrote the connections file, so changes here won't be saved." Fields a build does not know (at the top, in an entry, its tunnel, a recent entry) are kept and written back, and never reach the UI or a command's reply; the tunnel's login (`ssh.auth`) is not covered, so a field a build does not know there is dropped at a save. A command's `config` with a key the build does not know is refused (`badParams`, naming it, `ssh.auth.<key>` included), so a script's `"hots"` is not saved quietly.
- **Why:** builds of different ages share one file (a Flatpak and a package, dotfiles on two machines, a downgrade), and what the first public build does to a newer file it does for good.

## 2026-11-08: The Redesign is M3, import and export is M4

- **Decision:** the milestone after M2 is M3, the Redesign. Import and export moves from M3 to M4, and the alpha gate's last row (file export) lands in M4.
- **Why:** the owner's order of work: the redesign renames and moves many things that export and import would otherwise have to name twice.

## 2026-11-08: dbee's own Postgres SQL names `pg_catalog`'s functions and operators

- **Decision:** each catalog query (relations, columns, keys, the database list) runs as `SET LOCAL search_path TO pg_catalog, pg_temp; <query>` in one message, so the setting ends with that query's implicit transaction, and the queries name `pg_catalog`'s functions and tables themselves. dbee's filters, counts and saved changes name `pg_catalog`'s operators, in parentheses (`("code" OPERATOR(pg_catalog.=) CAST('a' AS pg_catalog.text))`; `in` as such equalities joined by `OR`; the patterns as `~~`, `!~~`, `~~*` and `!~~*`), where the column's operators are built in: its base type (after domains) is in `pg_catalog`, or is an array, an enum, a range or a composite (`ColumnInfo::builtin_operators`, which the catalog reads). A type that brings its own operators (an extension's: `citext`, PostGIS) keeps the plain operator, which its schema holds. Counts are `pg_catalog.count(*)`, and the casts to text name `pg_catalog.text` (also `bpchar` and `"bit"` in filters). A type's label leaves out a schema the session's search path finds (read once at connect, `current_schemas(false)`), as Postgres shows it, while `type_sql` names it for SQL. A search path that lists another schema before `pg_catalog` gets a warning at connect: the type names in casts (`jsonb`, `uuid`) are still found through it.
- **Why:** Postgres takes an operator or a function whose argument types match exactly over one that needs a coercion or is polymorphic, whatever the search path's order (the pattern of CVE-2018-1058), so one someone made in `public` runs as whoever filters or saves. Measured on both Postgres test servers with the default search path: a planted `public.unnest(smallint[])` and `name !~~ name` ran during dbee's catalog read, and planted `varchar = varchar`, `varchar = text`, `varchar <> text`, `varchar < text`, `integer[] = integer[]` and an enum's `=` ran during its filters, counts and saves; none runs now, and the primary key's index still serves each comparison (`EXPLAIN` with sequential scans off).

## 2026-11-08: MySQL's executable comments are code

- **Decision:** on MySQL and MariaDB the lexer reads `/*! … */` and `/*M! … */` (with a version or without) as code: the text inside is coloured and split as SQL, and the marks stay in the statement dbee sends, also at its end. A `*/` closes one only while one is open (`SELECT 6*/*x;SELECT 7;*/2` is one statement, as on the server), and a `;` inside an open one ends no statement.
- **What still differs from a server, measured on all five:** a `/*M! … */` is a plain comment on MySQL, and a comment whose version is above the server's is skipped by it (MariaDB also skips `/*!` with a MySQL 5.7 to 9.x version), while dbee colours and splits both as code for every server. Both differ in the safe direction: dbee shows more than the server runs, or the statement fails; it never shows as a comment what the server runs. The read-only guard does not rely on the colours: it reads each statement as its server will (2026-11-08, read-only).
- **Why:** the server runs what such a comment holds. dbee coloured it as a comment, and cut a trailing one off before sending, while `DELETE … WHERE /*! 1 = 1 OR */ id = 2` deleted every row (measured on all five servers). The server runs what the editor shows (2026-11-06).

## 2026-11-08: Postgres values with a backslash are dollar-quoted; Apply checks client_encoding

- **Decision:** a value dbee writes into Postgres SQL that holds a `\` is written as a dollar quote whose tag occurs neither in the value nor across its end (`$v$a\b$v$`); every other value stays `'…'` with `'` doubled. Before the statements of an Apply, dbee reads `client_encoding`; when a function changed it, dbee sets it back to UTF8, writes nothing and asks to apply again. A statement that turns `standard_conforming_strings` off (`SET`, `RESET`, `set_config`) is refused on every connection, and before a statement whose `'…'` strings hold a backslash dbee checks it is still on; when it is not, dbee turns it on again and does not run the statement.
- **Why:** what `'…\…'` means depends on `standard_conforming_strings`, and what `E'…\\…'` means on `client_encoding` and `backslash_quote` (under SJIS the byte `\` can be the second half of a character), all of which a function the session calls, through a view say, can change. A key holding `\'` ended its literal early and the rest of it ran as SQL (measured on both servers, first with the setting, then with `E'…'` under SJIS and `backslash_quote` on). A dollar quote carries no escape, so it reads the same under all three; but under another `client_encoding` the server reads every byte above ASCII as other text, which is why Apply checks it.

## 2026-11-08: Read-only refuses what could turn it off, and reads it back

- **Decision:** on a connection set read-only, the driver refuses, before sending, the statements that can turn read-only off or write in spite of it. On Postgres: `DO`, `CALL`, `SET` of a read-only setting to anything but on, `RESET` of one, a `SET` with `READ WRITE`, `BEGIN` or `START TRANSACTION … READ WRITE`, `RESET ALL`, `DISCARD ALL`, `COMMIT` or `ROLLBACK … AND CHAIN`, `set_config` of a read-only setting, the large-object functions that write (Postgres 14 runs them in a read-only transaction), `ALTER SYSTEM`, and `COPY … TO` a file or a program. On MySQL and MariaDB: `CALL`, `PREPARE`, `EXECUTE`, `SET STATEMENT`, a `SET` of the read-only variable to anything but on or with `READ WRITE`, `START TRANSACTION … READ WRITE`, `AND CHAIN`, `INTO OUTFILE` and `INTO DUMPFILE`, and `SET GLOBAL` and `SET PERSIST`; the guard reads each statement as its server will, so a version comment the server skips hides no word from it. Names spelled with Unicode escapes (`U&"…"`, `UESCAPE` too) are read as the names they spell. On SQLite the authorizer refuses `ATTACH` (and with it `VACUUM INTO`) and setting `query_only`, and its hint names the case that applies. After each statement the driver reads the mode back (Postgres both settings with `SHOW`, which takes no snapshot; MySQL and MariaDB the session variable) and, when it changed, rolls back, turns read-only on again and says so; when that fails, it closes the connection. A read the transaction's failure keeps from running is made before the next statement; so is MySQL's after a statement that left warnings, since reading a variable clears what `SHOW WARNINGS` lists. A read before a statement that a cancel stopped stops the statement too, and on MySQL a read the server refused does. The connection form shows, while Read-only is ticked, that it guards against mistakes and that a database user without write rights is the lock.
- **Why:** issue #8 and the security review: a pasted script (`pg_dumpall`'s starts with `SET default_transaction_read_only = off`), a `DO` block that commits, `START TRANSACTION READ WRITE` and MariaDB's `SET STATEMENT` all wrote on a read-only connection, some leaving the session looking read-only, and a version comment the server skipped hid them from the first check. What a function does inside one statement stays out of any client's reach (gaps entry 27): read-only is not a security boundary.

## 2026-11-08: SQLite files open with SQLite's own hardening

- **Decision:** every SQLite file opens with `SQLITE_DBCONFIG_DEFENSIVE` on and `cell_size_check` on; an isolated run's file opens with `SQLITE_OPEN_NOFOLLOW` on the path its check resolved. `trusted_schema` stays on, SQLite's default.
- **What changes in a query tab:** `PRAGMA writable_schema = ON` no longer lets SQL write the schema (measured), and `journal_mode = OFF` and `schema_version = N` stop working, as SQLite documents for `DEFENSIVE`.
- **Why `trusted_schema` stays on:** off, it keeps a file's views, triggers and defaults from the functions and virtual tables SQLite does not mark harmless. dbee registers no SQL function and no virtual-table module, so that protects nothing today, while views over FTS's `MATCH`, `snippet()` and `bm25()` or a pragma table function failed with "unsafe use of …", in table tabs with no way around (measured with the bundled SQLite 3.53). Turn it off the day dbee registers a function or a module that has effects.
- **Why:** SQLite's advice for files from sources one does not know, which any file a file manager hands dbee can be. `NOFOLLOW` closes the race in which a link swapped in after the check opened a file outside the allowed folders (measured: won within the first tries before).

## 2026-11-08: The connections file is private

- **Decision:** where the system has file modes, the folder of `connections.json` is made `0700`, and the file, its lock and a damaged file moved aside `0600`, whatever the umask; a folder and files made before are tightened at the next write.
- **Why:** the file holds no passwords, but it lists every saved server's host, user, database, SSH host and key path, which other local users could read (`664` under a umask of `002`, measured).

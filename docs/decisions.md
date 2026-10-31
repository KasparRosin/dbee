# Decisions

> **Last reviewed:** 2026-10-31 · **Last updated:** 2026-10-31 · **Change:** minor (MySQL's server key only without encryption or when allowed; per-type connection options)

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

- **Decision:** Driver and SSH tests run against local Docker containers that `cargo xtask testdb up` starts (compose project `dbee-testdb`): Postgres 14, the oldest supported major, and the newest released major (`PG_NEWEST_MAJOR` in `xtask/src/testdb.rs`, 18 on 2026-10-19 because `postgres:19-alpine` was not yet published), both with TLS and the same synthetic seed, plus an SSH server for tunnels. Ports are random on `127.0.0.1`, the password and key passphrase are generated, and `target/testdb/env.json` tells the tests where everything is. Without Docker the tests print `SKIPPED`; `DBEE_REQUIRE_ORACLES=1` makes that a failure.
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
- **Why:** M1 does real work and is ready for review on GitHub; CI on GitHub covers the macOS, Windows and BSD builds the local machine cannot run.

## 2026-10-28: M2 is three databases, before import and export

- **Decision:** M2 is "Three databases": a driver interface, then MySQL/MariaDB and SQLite with the full M1 workflow. Import and export move to M3.
- **Why:** The engine still names Postgres types, and export and import should be written once against the interface, not once per engine. Two more databases also widen who can use dbee daily.

## 2026-10-28: SQLite's public-domain code is allowed

- **Decision:** The licence allowlist gains SQLite's public-domain dedication, for the SQLite library compiled in by `libsqlite3-sys` (through `rusqlite` with `bundled`). The crates themselves are MIT.
- **Why:** SQLite is public domain and has no licence text to put on the list. Linking it in keeps dbee one binary with no system library to find.

## 2026-10-30: A driver interface: traits in dbee-driver, drivers at L4, the engine names none

- **Decision:** `dbee-driver` (L3) holds the interface every database driver implements: the `Driver`, `Connection` and `Cancel` traits, one error type (`DbError`), `ServerInfo`, `Target`, the caps, the apply gate, the shared TLS setup and the `CatalogBuilder`. The drivers (`dbee-pg` now; MySQL/MariaDB and SQLite next) sit at L4, and `dbee-drivers` (L4) is their one list, which only the apps and test code use. `cargo xtask layers` keeps the engine and the UIs from taking a driver crate except as a dev-dependency.
- **Families:** A driver serves a family of database types: the MySQL driver serves MySQL and MariaDB, and the Postgres driver can later serve Postgres-like servers. It names the types it opens, and each connection says which SQL dialect its server speaks.
- **Why:** Each engine plugs in the same way, and shared logic (TLS, the apply gate, catalog building) exists once. Adding an engine is one driver crate, one dialect file and one registry line.

## 2026-10-30: dbee's own MySQL client on mysql_common

- **Decision:** `dbee-mysql` speaks MySQL's client/server protocol itself. `mysql_common` 0.38 (MIT OR Apache-2.0, default features off: no zlib, no derive macros) gives the packet structs, the login scrambles and RSA; the I/O is dbee's own: TCP or a channel through the SSH tunnel, upgraded to TLS by `tokio-rustls` 0.26 (default features off; `ring`, `tls12` and `logging`; already in the tree through `tokio-postgres-rustls`) with the same rustls verifier as Postgres. Values travel over the text protocol (`COM_QUERY`), as the server prints them. MariaDB is a flavor of the same driver, told by `@@version`, not by the connection's type.
- **Logins:** `mysql_native_password`, `caching_sha2_password` (the fast path; the full path over TLS, or with the password encrypted with the server's RSA key) and `sha256_password` (the same rule). The password is never sent in clear without TLS; any other method (`mysql_clear_password` among them) ends the login before the password is used. Without TLS the RSA exchange keeps the password from a listener, not from a man in the middle, who can hand over a key of their own; TLS or the SSH tunnel protect against that.
- **Hostile servers:** every length, count and offset a server sends is checked, and every `mysql_common` parser dbee feeds server bytes to is fuzzed. Two of them panic on hostile input (the greeting parser on a challenge length of 128 to 135, the RSA key's DER parser on a length past its data), so dbee checks those bytes against the documented layouts before handing them over.
- **Why (checked 2026-10-28):** `mysql_async` 0.37's TLS needs `webpki-roots` (`rustls-tls`) or OpenSSL (`native-tls-tls`), both forbidden, and takes no stream of dbee's, so no tunnel channel; `sqlx` opens its own socket (a loopback forward) and checks certificates against the OS's roots, not through the OS verifier. `mysql_common` has no networking of its own.

## 2026-10-30: MySQL 5.7's TLS is out of rustls's reach; plain direct and SSH work

- **Decision:** The official `mysql:5.7` image offers TLS only with DHE-RSA ciphers on TLS 1.2, which rustls never speaks. dbee tells this from the TLS failure itself (the server's handshake alert), not from the version: `require`, `verify-ca` and `verify-full` fail with a TLS error that says the server offers only ciphers dbee does not support and points to an SSH tunnel, and `prefer` connects without TLS, as libpq's `prefer` does after a failed handshake. Plain direct connections and connections through the SSH tunnel work.
- **Why:** No new TLS stack (OpenSSL and aws-lc stay out) and no 5.7-only code path (spec amendment of 2026-10-29). The tunnel encrypts the link instead.

## 2026-10-31: MySQL's server key only without encryption or when allowed; per-type connection options

- **Decision:** On a link without TLS, `caching_sha2_password` and `sha256_password` protect the password with an RSA key the server sends, which dbee cannot check. dbee fetches that key when the user chose no encryption (`disable`) or the SSH tunnel encrypts the link, with no warning. When the user wanted encryption and the link ended up plain on a direct connection (`prefer`, with TLS failed or not offered, which an attacker on the link can force), the login is refused before the password is used, unless the connection's `allowPublicKey` option is on; then it goes ahead with a warning on the connection. `require` and the `verify-*` modes always have TLS. dbee follows MySQL's own clients in refusing the key on a link the user wanted encrypted, but allows it when the user chose no encryption.
- **Options:** A database type's own settings are on/off options: `ConnectionConfig.options` (by id, left out of the file when empty, keys a build does not know kept) and `KindInfo.options` (id and label). MySQL lists `allowPublicKey`, and MariaDB inherits it through its row; Postgres and SQLite list none. The connection form shows them from M2.15.
- **Why:** Whoever sits on a plain link can hand dbee a key of their own and read the password. Where the user asked for no encryption, or the tunnel gives it, there is nothing to protect against; where the user wanted encryption, that choice must not be undone silently. One options map keeps engine-only settings off the shared struct.

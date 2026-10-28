# Decisions

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-28 · **Change:** minor (the GitHub repository comes after M1)

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

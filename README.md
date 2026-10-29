<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/banner-dark.svg">
    <img src="assets/readme/banner-light.svg" alt="dbee">
  </picture>
</p>

<p align="center">A cross-platform desktop database client, written in Rust.</p>

<!-- Later, once CI runs on GitHub: one centred line of 2–3 badges here (CI, licence, latest release). -->

> [!NOTE]
> **Pre-alpha.** The first milestone, M1, is the daily Postgres workflow, and it is built: connect (also through an SSH tunnel), browse, filter and edit tables, run queries and copy results.\
> Postgres is the only database so far, and file export and import come next (M2).\
> Linux x86_64 is tested; the other platforms are defined, untested yet.

<p align="center">
  <img src="assets/readme/screenshot-filters.png" alt="dbee with a table of orders filtered by three conditions: the schema tree on the left, the grid under its filter bar in the middle, the value panel on the right">
</p>

## What works today

- **Connecting:** saved and recent connections; a form with Fill from URL; TLS modes with a CA file; an SSH tunnel with a key (found on its own), the agent or a password, and host key checks; read-only mode; passwords in the OS keyring; reconnecting after a lost connection.
- **Browsing:** a schema tree with a filter; quick search (Ctrl+P); table tabs with a filter bar (13 operators, any number of filters, raw `WHERE`), sorting, paging, estimated and exact counts, and foreign keys followed into a filtered tab.
- **Editing:** staged edits (one-line, multi-line and boolean editors, set NULL, add and delete rows), undo and redo, and Apply (Ctrl+S) in one transaction, all or nothing.
- **Values:** a value panel with the whole value of a cell, JSON pretty-printed and coloured.
- **Queries:** query tabs with syntax colours; run all, or the statement under the cursor; cancel; several results; error cards that jump to the error's position.
- **Copying:** values, with a header, or as CSV, JSON or Markdown.
- **Agents:** a localhost control channel that runs every command and drives the window.

Known gaps and limits are ranked in [docs/gaps.md](docs/gaps.md).

## Why dbee

- **Native and fast.** One Rust codebase, with no Electron and no browser engine.
- **Open source.** Dual-licensed MIT or Apache-2.0, written clean-room from public specifications and black-box observation.
- **Scriptable.** Every action is a command. The control channel reaches all of them today; a command-line tool is planned.

## Platforms

dbee is built for Linux (x86_64, aarch64, riscv64), macOS (universal), Windows (x64, x86, arm64) and FreeBSD (x86_64). Linux x86_64 is tested. The other platforms build in the release pipeline but nobody has installed and started them yet (Windows ARM64 excepted) ([docs/hardware-parity.md](docs/hardware-parity.md)).

## Download

v0.1.0 on the Releases page is an unsigned test pre-release of the release pipeline ([docs/releasing.md](docs/releasing.md)). Building dbee from source is still the main way to run it.

## Build from source

You need:

- Rust through [rustup](https://rustup.rs). `rust-toolchain.toml` picks the pinned toolchain.
- A C compiler (Debian and Ubuntu: `build-essential`).
- On Linux, no `-dev` packages: dbee loads the window and graphics libraries (Wayland or X11, xkbcommon, Vulkan or OpenGL) from the desktop at run time.
- Optional: Docker, for the test servers that the Postgres and SSH tests run against. Without them those tests print `SKIPPED`.

```sh
cargo run -p dbee                       # the desktop app
cargo run -p dbee-cli -- --version      # the command-line tool (version and help so far)
cargo xtask testdb up                   # optional: start the Docker test servers
cargo xtask ci                          # fmt, clippy, tests, assets, layering, provenance
```

The app writes its log to `<settings dir>/logs/dbee.log`, where the settings dir is `~/.config/dbee` on Linux and BSD, `~/Library/Application Support/dbee` on macOS and `%APPDATA%\dbee` on Windows.

## For agents

`dbee --control <port>` serves a JSON-lines control channel on `127.0.0.1`. It runs every command and drives the window: clicks, keys, inspection and screenshots ([docs/control-protocol.md](docs/control-protocol.md)). `cargo xtask run-isolated` starts the app with it on a throwaway home. The command-line tool's commands are planned and documented first ([docs/cli.md](docs/cli.md)). Agents working on dbee start with [AGENTS.md](AGENTS.md).

## Documentation

- [ROADMAP.md](ROADMAP.md): stage, headline numbers and progress log.
- [docs/gaps.md](docs/gaps.md): known gaps and limits, ranked.
- [docs/architecture.md](docs/architecture.md): an engine-first Cargo workspace whose layering is enforced, so the UI crate can be swapped.
- [docs/testing.md](docs/testing.md): the tests and the Docker test servers.
- [docs/agents.md](docs/agents.md): driving dbee from an agent, and developing it as one.
- [docs/README.md](docs/README.md): every doc.

## Security

Please report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License and credits

dbee is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Copyright (c) 2026 Kaspar Rosin and the dbee contributors.

Every asset in the repository is listed with its author, source and licence in [ATTRIBUTION.md](ATTRIBUTION.md). The repository holds no font files; the app embeds egui's default fonts (see ATTRIBUTION.md).

dbee is inspired by Beekeeper Studio.

<sub>Beekeeper Studio is a trademark of its owner. dbee is an independent project and is not affiliated with or endorsed by it.</sub>

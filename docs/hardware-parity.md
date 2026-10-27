# Hardware parity

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** minor (Work filled in; the build machine named as such) · **Target:** Beekeeper Studio 5.5.7

| Platform | Build | Installs | Runs | Notes |
|---|---|---|---|---|
| Linux x86_64 | tar.gz and AppImage of the M1 app built locally (2026-10-26); deb and rpm last built for M0 (2026-10-16), as nfpm was not installed on the build machine | not tried (needs sudo) | `dbee --version` from the tarball; the release build's window (connection screen in 301 ms at load about 24.5, `connection_screen_within_one_second`); the dev build through the whole M1 day (`m1_day`) | 2026-10-26, Debian 13, KDE Plasma (Wayland) |
| Linux aarch64 | defined, unverified | defined, unverified | defined, unverified | |
| Linux riscv64 | defined, unverified | defined, unverified | defined, unverified | |
| macOS universal | defined, unverified | defined, unverified | defined, unverified | |
| Windows x64 | defined, unverified | defined, unverified | defined, unverified | |
| Windows x86 | defined, unverified | defined, unverified | defined, unverified | |
| Windows arm64 | defined, unverified | defined, unverified | defined, unverified | |
| FreeBSD x86_64 | defined, unverified | defined, unverified | defined, unverified | |

## Work

The release pipeline builds every row ([releasing.md](releasing.md)), but it has never run: it waits for the GitHub repository. Its first run, then installing and starting each build, fills in this table. Not estimated yet.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-27 | minor | Work: the release pipeline's first run fills in the table; the scaffold placeholder replaced. |
| 2026-10-26 | minor | Linux x86_64: the M1 app's tarball and AppImage built and run. History put newest first. |
| 2026-10-16 | minor | Linux x86_64 verified locally. |
| 2026-10-15 | major | Template (scaffold). |

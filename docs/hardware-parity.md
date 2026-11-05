# Hardware parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** minor (Linux x86_64: the M2 app's tarball) · **Target:** Beekeeper Studio 5.5.7

| Platform | Build | Installs | Runs | Notes |
|---|---|---|---|---|
| Linux x86_64 | tar.gz of the M2 app built locally (2026-11-05); the AppImage last built for M1 (2026-10-27), as its tool now downloads the AppImage runtime while it builds, which a local build does not do; deb and rpm last built for M0 (2026-10-16), as nfpm was not installed on the build machine | not tried (needs sudo) | `dbee-cli --version` from the staged package; the release build's window (`connection_screen_within_one_second`, [target-app-parity.md](target-app-parity.md), "Performance"); the dev build through the whole M1 day (`m1_day`) and the M2 day on MySQL, MariaDB and SQLite (`m2_day`) | 2026-11-05, Debian 13, KDE Plasma (Wayland) |
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
| 2026-11-05 | minor | Linux x86_64: the M2 app's tarball built and the M2 day run; the AppImage not rebuilt (its tool downloads its runtime). |
| 2026-10-27 | minor | Work: the release pipeline's first run fills in the table; the scaffold placeholder replaced. |
| 2026-10-26 | minor | Linux x86_64: the M1 app's tarball and AppImage built and run. History put newest first. |
| 2026-10-16 | minor | Linux x86_64 verified locally. |
| 2026-10-15 | major | Template (scaffold). |

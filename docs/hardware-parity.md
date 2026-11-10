# Hardware parity

> **Last reviewed:** 2026-11-06 · **Last updated:** 2026-11-06 · **Change:** minor (Linux x86_64: the M2 app's tarball rebuilt) · **Target:** Beekeeper Studio 5.5.7

| Platform | Build | Installs | Runs | Notes |
|---|---|---|---|---|
| Linux x86_64 | tar.gz of the M2 app built locally (2026-11-06, from `df2f3ca`, sha256 `38051b3c89fde4a1213fb9a0c80297f173c9d322ea5297a4c439f4ae74f441fa`); the AppImage last built for M1 (2026-10-27), as its tool now downloads the AppImage runtime while it builds, which a local build does not do; deb and rpm last built for M0 (2026-10-16), as nfpm was not installed on the build machine | not tried (needs sudo) | `dbee-cli --version` from the staged package; the release build's window (`connection_screen_within_one_second`, [target-app-parity.md](target-app-parity.md), "Performance"); the dev build through the whole M1 day (`m1_day`) and the M2 day on MySQL, MariaDB and SQLite (`m2_day`) | 2026-11-06, Debian 13, KDE Plasma (Wayland) |
| Linux aarch64 | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |
| Linux riscv64 | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |
| macOS universal | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |
| Windows x64 | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |
| Windows x86 | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |
| Windows arm64 | built in the test release (2026-10-29) | installed on ARM64 hardware (2026-10-29) | started on ARM64 hardware (2026-10-29) | the windows-arm64 workflow |
| FreeBSD x86_64 | built in the test release (2026-10-29) | defined, unverified | defined, unverified | |

## Work

The release pipeline has run ([releasing.md](releasing.md)). Every row built in its second test run (2026-10-29, version 0.1.0). Windows ARM64 also installed and started on ARM64 hardware. Installing and starting the other builds is still to do. Not estimated yet.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-06 | minor | Linux x86_64: the M2 app's tarball rebuilt from `df2f3ca`, with the fixes after the M2 day in, and its sha256. |
| 2026-11-05 | minor | Linux x86_64: the M2 app's tarball built and the M2 day run; the AppImage not rebuilt (its tool downloads its runtime). |
| 2026-10-29 | minor | The pipeline ran: every row built (test release 0.1.0); Windows ARM64 installed and started on ARM64. |
| 2026-10-27 | minor | Work: the release pipeline's first run fills in the table; the scaffold placeholder replaced. |
| 2026-10-26 | minor | Linux x86_64: the M1 app's tarball and AppImage built and run. History put newest first. |
| 2026-10-16 | minor | Linux x86_64 verified locally. |
| 2026-10-15 | major | Template (scaffold). |

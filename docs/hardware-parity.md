# Hardware parity

> **Last reviewed:** 2026-10-29 · **Last updated:** 2026-10-29 · **Change:** minor (every row built in the first test release; Windows ARM64 installed and started) · **Target:** Beekeeper Studio 5.5.7

| Platform | Build | Installs | Runs | Notes |
|---|---|---|---|---|
| Linux x86_64 | tar.gz and AppImage of the M1 app built locally (2026-10-26); deb and rpm last built for M0 (2026-10-16), as nfpm was not installed on the build machine | not tried (needs sudo) | `dbee --version` from the tarball; the release build's window (connection screen in 301 ms at load about 24.5, `connection_screen_within_one_second`); the dev build through the whole M1 day (`m1_day`) | 2026-10-26, Debian 13, KDE Plasma (Wayland) |
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
| 2026-10-29 | minor | The pipeline ran: every row built (test release 0.1.0); Windows ARM64 installed and started on ARM64. |
| 2026-10-27 | minor | Work: the release pipeline's first run fills in the table; the scaffold placeholder replaced. |
| 2026-10-26 | minor | Linux x86_64: the M1 app's tarball and AppImage built and run. History put newest first. |
| 2026-10-16 | minor | Linux x86_64 verified locally. |
| 2026-10-15 | major | Template (scaffold). |

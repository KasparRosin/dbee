# Installing dbee

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-26 · **Change:** minor (updates: in 0.3.1 and 0.3.2 the button's click does nothing)

Downloads are on the [Releases page](https://github.com/KasparRosin/dbee/releases). Each file is named `dbee-<version>-<platform>-<arch>.<ext>`; pick the one for your system below.

dbee's builds aren't signed (see [releasing.md](releasing.md), "Signing"). macOS and Windows warn the first time you open one; the steps below get past that once. Linux and FreeBSD don't warn.

## macOS (11 or later, Intel and Apple silicon)

1. Download `dbee-<version>-macos-universal.dmg`.
2. In Terminal, unblock it and open it:

   ```sh
   xattr -d com.apple.quarantine ~/Downloads/dbee-*-macos-universal.dmg
   open ~/Downloads/dbee-*-macos-universal.dmg
   ```

If macOS blocks the app when you open it:

```sh
xattr -dr com.apple.quarantine /Applications/dbee.app
```

Without Terminal: after macOS refuses the file, open System Settings → Privacy & Security and press **Open Anyway**.

The command-line tool is `dbee-cli-<version>-macos-universal.zip`.

## Windows (x64, x86 and arm64)

- **Installer:** run `dbee-<version>-windows-<arch>.msi`.
- **Portable:** unpack `dbee-<version>-windows-<arch>-portable.zip` anywhere and run `dbee.exe`.

If SmartScreen says "Windows protected your PC", choose **More info**, then **Run anyway**.

## Linux (x86_64 and aarch64; riscv64 as `.deb`, `.rpm` and `.tar.gz`)

| File | Install |
|---|---|
| `.flatpak` | `flatpak install --user dbee-<version>-linux-<arch>.flatpak`; Flatpak fetches its runtime from Flathub if it's missing |
| `.AppImage` | `chmod +x dbee-<version>-linux-<arch>.AppImage`, then run it |
| `.deb` | `sudo apt install ./dbee-<version>-linux-<arch>.deb` |
| `.rpm` | `sudo dnf install ./dbee-<version>-linux-<arch>.rpm` |
| `.tar.gz` | unpack it and run `bin/dbee`; `share/` holds the desktop entry and icons |

The Flatpak can't reach the keyring yet ([gaps.md](gaps.md), 11).

## FreeBSD (x86_64)

Unpack `dbee-<version>-freebsd-x86_64.tar.gz` and run `bin/dbee`.

## Updates

dbee looks for a newer release by itself: a few seconds after it starts, and once a day while it is open, it asks GitHub for the newest release's checksum file. When a newer release is out, the right end of the menu bar reads `DOWNLOAD <version>`; a click opens that release's page in the browser, and you install it as you did the first time. In 0.3.1 and 0.3.2 the click does nothing: open the [Releases page](https://github.com/KasparRosin/dbee/releases) yourself. dbee does not install anything by itself. With no newer release nothing is shown there.

What the check sends, and how to turn it off: [SECURITY.md](../SECURITY.md#what-dbee-sends-by-itself).

## Checking a download

Every file is listed in the release's `SHA256SUMS.txt`. In the folder with both:

- Linux: `sha256sum --ignore-missing -c SHA256SUMS.txt`
- FreeBSD: `sha256 dbee-<version>-freebsd-x86_64.tar.gz`, then compare with the line in `SHA256SUMS.txt`
- macOS: `shasum -a 256 --ignore-missing -c SHA256SUMS.txt`
- Windows (PowerShell): `Get-FileHash dbee-<version>-windows-x64.msi`, then compare with the line in `SHA256SUMS.txt`

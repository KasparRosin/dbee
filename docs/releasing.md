# Releasing dbee

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-23 · **Change:** minor (Linux riscv64 is built as `.deb`, `.rpm` and `.tar.gz`, as the release workflow does)

Every push to the `release` branch runs `.github/workflows/release.yml`. The workflow builds installers for macOS, Windows, Linux and FreeBSD and creates or updates a **draft** GitHub Release named `v<version>`, the tag alone. Nobody sees a draft until the owner publishes it. The workflows use no shared tooling from outside this repository; third-party actions are pinned to commit SHAs.

**Status:** the pipeline has run twice by hand; the test release `v0.1.0` is published as an unsigned pre-release. User-facing names say **dbee**; files, binaries and ids stay lowercase (`dbee-<version>-<platform>-<arch>.<ext>`, app id `dev.rosin.dbee`).

## Cutting a release

1. **Bump the version** on `main`. It lives only in `[workspace.package] version` in the root `Cargo.toml`:

   ```sh
   cargo xtask version                 # prints the current version
   cargo xtask version set 0.1.0       # or 0.0.1-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit `Cargo.toml` and `Cargo.lock`.
2. **Push `main` to `release`:** `git push origin main:release`. The workflow starts by itself.
3. **Wait for the draft.** Notarization and the FreeBSD VM are the slow parts. The Releases page then has a draft `v<version>`, tagged the same on the pushed commit, with every artifact and `SHA256SUMS.txt`.
4. **Check it.** Download an installer or two and read the job summaries. A `::warning::` there means a signing secret was missing and that artifact is unsigned.
5. **Write the notes and publish** the draft in the GitHub UI. Publishing creates the tag. From 0.2.0 on, releases are full releases, not pre-releases; a version containing `-` (such as `0.0.1-rc.1`) would still be marked as one. The notes link to [installing.md](installing.md) for opening an unsigned build, instead of repeating the steps; links into the repository point at the release's commit, since its tag only exists once it's published.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets. After publishing, the workflow refuses to touch that version again, so bump first.

**Test runs:** *Actions → Release → Run workflow* runs the pipeline by hand. The optional `version` input overrides `Cargo.toml` for that run only. Run from any branch other than `release`, it is a dry run: the jobs that need the `release` environment are refused and no release is drafted.

## What gets built

`<v>` is the version. Every file is listed in `SHA256SUMS.txt`.

| Platform | Assets |
|---|---|
| macOS 11+ (universal) | `dbee-<v>-macos-universal.dmg`, `dbee-cli-<v>-macos-universal.zip` |
| Windows x64 | `dbee-<v>-windows-x64.msi`, `dbee-<v>-windows-x64-portable.zip` |
| Windows arm64 | `dbee-<v>-windows-arm64.msi`, `dbee-<v>-windows-arm64-portable.zip` |
| Windows x86 | `dbee-<v>-windows-x86.msi`, `dbee-<v>-windows-x86-portable.zip` |
| Linux x86_64 | `dbee-<v>-linux-x86_64.{AppImage,AppImage.zsync,flatpak,deb,rpm,tar.gz}` |
| Linux aarch64 | `dbee-<v>-linux-aarch64.{AppImage,AppImage.zsync,flatpak,deb,rpm,tar.gz}` |
| Linux riscv64 | `dbee-<v>-linux-riscv64.{deb,rpm,tar.gz}` (cross-compiled) |
| FreeBSD x86_64 | `dbee-<v>-freebsd-x86_64.tar.gz` (built in a FreeBSD VM by the `freebsd` job of `release.yml`) |
| All | `SHA256SUMS.txt` |

There is no web build: dbee is desktop only ([decisions](decisions.md)). Packaging scripts live in `packaging/{macos,windows,linux,freebsd}`; `packaging/icons.sh` regenerates icons from `assets/app-icon/`.

## Signing (optional)

Signing is optional. Without the secrets below, builds ship unsigned (macOS ad-hoc signed, not notarized) and the job prints a `::warning::`. The secrets live in the repository's `release` environment, which only the `release` branch can deploy to; never store them as repository or organisation secrets.

| Secret | Enables |
|---|---|
| `APPLE_CERTIFICATE` | macOS: single-line base64 `.p12` Developer ID Application certificate |
| `APPLE_CERTIFICATE_PASSWORD` | macOS: password for that `.p12` |
| `KEYCHAIN_PASSWORD` | macOS, optional: password for the temporary CI keychain (generated when unset) |
| `APPLE_ID` | macOS notarization: the Apple ID used by `notarytool` |
| `APPLE_PASSWORD` | macOS notarization: app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | macOS: the Apple developer team id |
| `WINDOWS_CERTIFICATE` | Windows: base64 `.pfx` code-signing certificate |
| `WINDOWS_CERTIFICATE_PASSWORD` | Windows: password for that `.pfx` |
| `AZURE_TENANT_ID` | Windows, Azure Trusted Signing: tenant of the service principal |
| `AZURE_CLIENT_ID` | Windows, Azure Trusted Signing: service principal id |
| `AZURE_CLIENT_SECRET` | Windows, Azure Trusted Signing: service principal secret |
| `AZURE_SIGNING_ENDPOINT` | Windows, Azure Trusted Signing: regional endpoint URL |
| `AZURE_SIGNING_ACCOUNT` | Windows, Azure Trusted Signing: signing account name |
| `AZURE_CERT_PROFILE` | Windows, Azure Trusted Signing: certificate profile name |

Windows signing uses the `.pfx` pair if both are set, otherwise Azure Trusted Signing if all six Azure secrets are set, otherwise it leaves files unsigned. `GITHUB_TOKEN` creates the release; only the final job gets `contents: write`.

## Checks

- `.github/workflows/packaging-lint.yml` (on changes to `packaging/`, the workflows or the icons; no build, no secrets): actionlint, shellcheck, PowerShell parse, xmllint, `desktop-file-validate`, `appstreamcli validate`, Flatpak manifest checks.
- `.github/workflows/windows-arm64.yml` (on `release` pushes): installs, runs and uninstalls the arm64 MSI on a Windows 11 ARM64 runner.
- `.github/workflows/freebsd.yml`: FreeBSD build and tests.
- `.github/workflows/ci.yml`: the everyday gates. Locally: `cargo xtask ci`.

Only the `release` branch builds installers; no build workflow runs on `main` or pull requests.

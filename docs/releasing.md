# Releasing dbee

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-25 · **Change:** minor (the release key: the checksum file's signature, its secret and how to make or replace the key)

Every push to the `release` branch runs `.github/workflows/release.yml`. The workflow builds installers for macOS, Windows, Linux and FreeBSD and creates or updates a **draft** GitHub Release named `v<version>`, the tag alone. Nobody sees a draft until the owner publishes it. The workflows use no shared tooling from outside this repository; third-party actions are pinned to commit SHAs.

**Status:** three releases are published, all unsigned: `v0.1.0`, a test of the pipeline, as a pre-release, `v0.2.0` as the first full release, and `v0.3.0`. The release key is pinned since 2026-11-25, and the next release is the first signed one. User-facing names say **dbee**; files, binaries and ids stay lowercase (`dbee-<version>-<platform>-<arch>.<ext>`, app id `dev.rosin.dbee`).

## Cutting a release

1. **Bump the version** on `main`. It lives only in `[workspace.package] version` in the root `Cargo.toml`:

   ```sh
   cargo xtask version                 # prints the current version
   cargo xtask version set 0.1.0       # or 0.0.1-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit `Cargo.toml` and `Cargo.lock`.
2. **Push `main` to `release`:** `git push origin main:release`. The workflow starts by itself.
3. **Wait for the draft.** Notarization and the FreeBSD VM are the slow parts. The Releases page then has a draft `v<version>`, tagged the same on the pushed commit, with every artifact, `SHA256SUMS.txt` and `SHA256SUMS.txt.sig`.
4. **Check it.** Download an installer or two and read the job summaries. A `::warning::` there means a signing secret was missing and that artifact is unsigned. A release without a valid `SHA256SUMS.txt.sig` is not drafted at all: the last job fails.
5. **Write the notes and publish** the draft in the GitHub UI. Publishing creates the tag. From 0.2.0 on, releases are full releases, not pre-releases; a version containing `-` (such as `0.0.1-rc.1`) would still be marked as one. The notes link to [installing.md](installing.md) for opening an unsigned build, instead of repeating the steps; links into the repository point at the release's commit, since its tag only exists once it's published.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets. After publishing, the workflow refuses to touch that version again, so bump first.

**Test runs:** *Actions → Release → Run workflow* runs the pipeline by hand. The optional `version` input overrides `Cargo.toml` for that run only. Run from any branch other than `release`, it is a dry run: the jobs that need the `release` environment are refused and no release is drafted.

## What gets built

`<v>` is the version. Every file but `SHA256SUMS.txt.sig` is listed in `SHA256SUMS.txt`, which is also what a running dbee reads to learn of a newer release: its file names must keep the form `dbee(-cli)?-<v>-<platform>-<arch>[-portable].<ext>`, all of one version, or the copies in use stop seeing new releases.

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
| All | `SHA256SUMS.txt` and its signature, `SHA256SUMS.txt.sig` |

There is no web build: dbee is desktop only ([decisions](decisions.md)). Packaging scripts live in `packaging/{macos,windows,linux,freebsd}`; `packaging/icons.sh` regenerates icons from `assets/app-icon/`.

## Signing (optional)

Signing is optional, except `RELEASE_SIGNING_KEY`, which is required now that a key is pinned. Without the other secrets below, builds ship unsigned (macOS ad-hoc signed, not notarized) and the job prints a `::warning::`. The secrets live in the repository's `release` environment, which only the `release` branch can deploy to; never store them as repository or organisation secrets.

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
| `RELEASE_SIGNING_KEY` | All: the release key, an ed25519 private key in PEM, which signs `SHA256SUMS.txt`. Required once a key is pinned (below) |

Windows signing uses the `.pfx` pair if both are set, otherwise Azure Trusted Signing if all six Azure secrets are set, otherwise it leaves files unsigned. `GITHUB_TOKEN` creates the release; only the final job gets `contents: write`.

### The release key

The last job signs `SHA256SUMS.txt` with `RELEASE_SIGNING_KEY` into `SHA256SUMS.txt.sig`: ed25519, the 64 raw bytes. A running dbee will install a release itself only when that signature is by a public key compiled into it (`PINNED_KEYS` in `crates/update/src/signature.rs`; [decisions](decisions.md), 2026-11-24). No copy does so yet: the install step is not built and the app does not fetch the signature ([gaps](gaps.md), entry 59). The same job then checks the files as the app will (`cargo run -p dbee-update --example check_release -- artifacts <version>`): the checksum file must read as a release's file list of that version, and once a key is pinned, a release without a signature by it is not drafted. With no key pinned it only warns. The check knows the keys this release pins, not the keys of the copies in use, so it cannot tell that a key was replaced too early (below).

The key was made on 2026-11-25 and its public half is in `PINNED_KEYS`; no release is signed with it yet, so the first release from here is the signing step's first run. The steps below are for a fork, or for a new key. Before making one, check in the repository's settings that the `release` environment deploys from the `release` branch only. That rule, not the branch's protection alone, keeps a workflow on another branch from reading the key.

To make the key, with openssl 3 and `gh`:

1. Make the key outside the repository and set the secret. The key file is the only copy besides the secret, which GitHub never shows again: keep it somewhere safe or delete it.

   ```sh
   key="$(mktemp -d)/release-key.pem"
   (umask 077; openssl genpkey -algorithm ed25519 -out "$key")
   gh secret set RELEASE_SIGNING_KEY --env release <"$key"
   ```

2. Print the public key, the last 32 bytes of its DER form (44 bytes in all for ed25519; another kind of key prints 32 bytes that are no key), put them into `PINNED_KEYS` as a `[u8; 32]` and commit. The releases built from then on carry the key and are signed with it.

   ```sh
   openssl pkey -in "$key" -pubout -outform DER | tail -c 32 | od -An -v -tx1
   ```

To replace a key, pin the next one a release before it signs: make the next key and add its public key to `PINNED_KEYS` beside the old one, release (signed with the old key, carrying both), and only then set the secret to the next key. Every copy from that release on installs what the next key signs. An older copy that skipped it reads only the newest release, refuses the next key's signature and offers the download instead, so keep signing with the old key for as long as older copies should update themselves.

When a key is lost or no longer trusted, make a new key, set the secret to it and put its public key into `PINNED_KEYS` in the commit that is released next, in place of the old key when that one is no longer trusted. Until the new key is pinned, the release job refuses to draft. The copies in use know only the old key: they refuse a release signed with the new one and offer the download instead, until they are updated by hand to a release that carries the new key. Dropping the old key from the list protects only the copies built afterwards. A fork that releases pins its own key in place of this one.

Whoever can push to the `release` branch, or run the workflow on it by hand, can have a release signed, and whoever can change what runs in the `release` environment can also copy the key out. A copied key cannot be called back: the copies in use accept what it signs until each is updated, by hand, to a release that no longer pins it. Keep that branch protected, and consider required reviewers on the `release` environment.

## Checks

- `.github/workflows/packaging-lint.yml` (on changes to `packaging/`, the workflows or the icons; no build, no secrets): actionlint, shellcheck, PowerShell parse, xmllint, `desktop-file-validate`, `appstreamcli validate`, Flatpak manifest checks, and `packaging/check-install-kind.sh`: each package format writes its one-word install-kind marker, which tells the app how it was installed ([decisions](decisions.md)), and the AppImage, the Flatpak and the macOS app carry none. The packaging scripts read the marker back from the `.deb`, the `.rpm`, the tarballs and the portable zip they build.
- `.github/workflows/windows-arm64.yml` (on `release` pushes): installs, runs and uninstalls the arm64 MSI on a Windows 11 ARM64 runner.
- `.github/workflows/freebsd.yml`: FreeBSD build and tests.
- `.github/workflows/ci.yml`: the everyday gates. Locally: `cargo xtask ci`.

Only the `release` branch builds installers; no build workflow runs on `main` or pull requests.

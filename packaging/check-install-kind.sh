#!/usr/bin/env bash
# Checks, without building anything, that each package format writes its one-word install-kind
# marker and that the formats known another way write none. The app reads the marker to learn how
# it was installed (crates/update/src/install.rs): `share/dbee/install-kind` beside `bin/` on
# Linux and FreeBSD, `install-kind` beside dbee.exe on Windows.
#
#   .deb `deb`, .rpm `rpm`      packaging/linux/nfpm.yaml, one entry per packager
#   .tar.gz `tar`               packaging/linux/package.sh and packaging/freebsd/package.sh, in the
#                               tarball's own tree, never in the tree every Linux format shares
#   .msi `msi`, zip `portable`  packaging/windows/package.ps1 and dbee.wxs
#   AppImage, Flatpak, macOS    none: known by $APPIMAGE, /.flatpak-info and the app bundle's id
#
# The packaging scripts read the marker back from each package they build. Needs python3 with
# PyYAML. Run by .github/actions/packaging-lint.
# shellcheck disable=SC2016  # the texts looked for are the scripts' own lines, `$` and all
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "error: $*" >&2; exit 1; }
# has <file> <fixed text>: the file holds that text.
has() { grep -qF -- "$2" "$1" || fail "$1 should hold: $2"; }
# lacks <fixed text> <file>...: none of the files holds that text.
lacks() { local text="$1"; shift; if grep -nF -- "$text" "$@"; then fail "the lines above should not be there"; fi; }

python3 - <<'PY'
import sys, yaml
contents = yaml.safe_load(open("packaging/linux/nfpm.yaml"))["contents"]
markers = {c.get("packager"): c["src"] for c in contents if c.get("dst") == "/usr/share/dbee/install-kind"}
want = {"deb": "${MARKERS}/deb", "rpm": "${MARKERS}/rpm"}
if markers != want:
    sys.exit(f"error: packaging/linux/nfpm.yaml should ship one install-kind marker per packager, {want}, but has {markers}")
if any("install-kind" in c["src"] and not c.get("packager") for c in contents):
    sys.exit("error: packaging/linux/nfpm.yaml has an install-kind marker for every packager")
print("ok: nfpm.yaml ships `deb` in the .deb and `rpm` in the .rpm")
PY

linux=packaging/linux/package.sh
has "$linux" 'for kind in deb rpm tar; do printf '"'"'%s\n'"'"' "$kind" >"$MARKERS/$kind"; done'
has "$linux" 'install -Dm644 "$MARKERS/tar" "$WORK/tar/$BASENAME/share/dbee/install-kind"'
has "$linux" 'check_marker tar '
has "$linux" 'check_marker deb '
has "$linux" 'check_marker rpm '
has "$linux" 'if [ -n "$(find "$APPDIR" -name install-kind)" ]; then'
lacks '$STAGE/usr/share/dbee' "$linux"
echo "ok: the Linux tarball gets \`tar\` in its own tree, the shared tree and the AppImage none"

has packaging/linux/flatpak-bundle.sh 'rm -rf "$WORK/stage/share/dbee"'
lacks 'install-kind' packaging/linux/flatpak/*.yml
lacks 'share/dbee' packaging/linux/flatpak/*.yml
echo "ok: the Flatpak carries none"

has packaging/freebsd/package.sh 'printf '"'"'tar\n'"'"' >"$TREE/share/dbee/install-kind"'
echo "ok: the FreeBSD tarball gets \`tar\`"

has packaging/windows/package.ps1 "Set-Content -NoNewline -Encoding ascii -Path (Join-Path \$Stage 'install-kind') -Value 'msi'"
has packaging/windows/package.ps1 "Set-Content -NoNewline -Encoding ascii -Path (Join-Path \$Portable 'install-kind') -Value 'portable'"
has packaging/windows/dbee.wxs '<File Id="InstallKindMarker" Source="$(var.BinDir)\install-kind" KeyPath="yes" />'
echo "ok: the MSI gets \`msi\` and the portable zip \`portable\`, beside dbee.exe"

if grep -rnF -- 'install-kind' packaging/macos; then fail "the macOS app is known by its bundle id and carries no marker"; fi
echo "ok: the macOS app carries none"

# The words the packaging writes are the words the app reads.
for word in deb rpm tar msi portable; do
  has crates/update/src/install.rs "(\"$word\", Os::"
done
has crates/update/src/install.rs 'pub const MARKER_FILE: &str = "install-kind";'
echo "ok: the app reads the same words"

#!/usr/bin/env bash
# Build and package dbee for Linux (<arch> is x86_64 or aarch64, or riscv64 when cross-compiling):
#
#   $DIST/dbee-<version>-linux-<arch>.AppImage  any distro with glibc >= the build host's
#   $DIST/dbee-<version>-linux-<arch>.AppImage.zsync  delta updates (needs zsyncmake)
#   $DIST/dbee-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/dbee-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/dbee-<version>-linux-<arch>.tar.gz    plain FHS-style tree (bin/, share/)
#
# The .deb, the .rpm and the .tar.gz each carry a one-word marker, share/dbee/install-kind (`deb`,
# `rpm`, `tar`), which tells the app how it was installed, so its update button knows what it
# can do. The staged tree every format shares holds none: the AppImage must not carry one, and
# the Flatpak bundle is built from the tarball without it (flatpak-bundle.sh).
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar"]
#
# Needs: cargo; nfpm for deb/rpm (https://nfpm.goreleaser.com); appimagetool for the AppImage
# (downloaded into $CARGO_TARGET_DIR if missing). Build on an old distro (CI: Ubuntu 22.04,
# glibc 2.35) so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli,
# zsyncmake (the zsync package) for the AppImage's .zsync.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=dev.rosin.dbee

SKIP_BUILD=0
FORMATS="appimage deb rpm tar"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  riscv64) DEB_ARCH=riscv64 ;;
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
BASENAME="dbee-$VERSION-linux-$ARCH"

echo "==> dbee $VERSION for Linux $ARCH ($FORMATS)"

if [ "$SKIP_BUILD" = 0 ]; then
  if [ -n "${CROSS_TARGET:-}" ]; then
    (cd "$ROOT" && cargo build --release --locked -p dbee -p dbee-cli --target "$CROSS_TARGET")
  else
    (cd "$ROOT" && cargo build --release --locked -p dbee -p dbee-cli)
  fi
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/dbee" "$STAGE/usr/bin/dbee"
install -Dm755 "$BIN/dbee-cli" "$STAGE/usr/bin/dbee-cli"
"${CROSS_COMPILE:-}strip" "$STAGE/usr/bin/dbee" "$STAGE/usr/bin/dbee-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$DBEE_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/dbee"
copy_docs "$STAGE/usr/share/doc/dbee"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
# The metainfo uses AppStream 1.0 tags (<developer>, the vcs-browser url) that older validators,
# such as Ubuntu 22.04's 0.15, reject; packaging-lint checks it with a current one on every change.
if command -v appstreamcli >/dev/null; then
  as_version="$(appstreamcli --version | grep -oE '[0-9]+(\.[0-9]+)+' | head -n 1)"
  if [ "${as_version%%.*}" -ge 1 ] 2>/dev/null; then
    appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
  else
    echo "skipping the metainfo check: AppStream ${as_version:-?} is older than 1.0"
  fi
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# The install-kind markers, one file per word, outside the staged tree.
MARKERS="$WORK/install-kind"
mkdir -p "$MARKERS"
for kind in deb rpm tar; do printf '%s\n' "$kind" >"$MARKERS/$kind"; done

# check_marker <word> <what was read from the package>: the package carries exactly that word.
check_marker() {
  if [ "$2" = "$1" ]; then
    echo "ok: the install-kind marker is '$1'"
  else
    echo "error: the install-kind marker should be '$1', but is '$2'" >&2
    exit 1
  fi
}

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  # Into the tarball's own tree only, never into $STAGE.
  install -Dm644 "$MARKERS/tar" "$WORK/tar/$BASENAME/share/dbee/install-kind"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
  check_marker tar "$(tar -xzOf "$DIST/$BASENAME.tar.gz" "$BASENAME/share/dbee/install-kind")"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  sed -e "s|\${STAGE}|$STAGE|g" -e "s|\${MARKERS}|$MARKERS|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
  # Each package carries its own marker, read back from the file that ships, where this machine
  # has the tool to open it.
  if has deb; then
    if command -v dpkg-deb >/dev/null; then
      check_marker deb "$(dpkg-deb --fsys-tarfile "$DIST/$BASENAME.deb" | tar -xO ./usr/share/dbee/install-kind)"
    else
      warn "dpkg-deb not found, so the .deb's install-kind marker was not checked"
    fi
  fi
  if has rpm; then
    if command -v rpm2cpio >/dev/null && command -v cpio >/dev/null; then
      check_marker rpm "$(rpm2cpio "$DIST/$BASENAME.rpm" | cpio --quiet -i --to-stdout '*usr/share/dbee/install-kind')"
    else
      warn "rpm2cpio or cpio not found, so the .rpm's install-kind marker was not checked"
    fi
  fi
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/dbee.AppDir"
  cp -R "$STAGE" "$APPDIR"
  # An AppImage is known by $APPIMAGE; a marker in it would name another format.
  if [ -n "$(find "$APPDIR" -name install-kind)" ]; then
    echo "error: the AppImage's tree carries an install-kind marker" >&2
    exit 1
  fi
  mv "$APPDIR/usr/share/doc" "$WORK/doc-unused"
  ln -s usr/bin/dbee "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$ARCH.AppImage"
    if [ ! -x "$TOOL" ]; then
      curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
      chmod +x "$TOOL"
    fi
  fi
  # Absolute, because appimagetool runs in $DIST below (CARGO_TARGET_DIR or APPIMAGETOOL may be
  # relative, e.g. target/local).
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  # A .zsync left by an earlier run would hide a missing zsyncmake and describe another file.
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate, AppImageLauncher and the like read it from the file and
  # fetch only the blocks that changed in a newer release, through the .zsync published next to
  # each AppImage on GitHub Releases. `latest` is the newest published release that is not a
  # pre-release. A fork's builds point at its own releases through GITHUB_REPOSITORY.
  REPO="${GITHUB_REPOSITORY:-KasparRosin/dbee}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|dbee-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either. With zsyncmake on the host (CI installs the zsync
  # package) appimagetool also writes the .zsync, into its working directory, hence the cd.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [ -s "$OUT.zsync" ]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

if [ -z "${CROSS_TARGET:-}" ]; then
  "$STAGE/usr/bin/dbee-cli" --version
elif [ -n "${EMULATOR:-}" ]; then
  "$EMULATOR" "$STAGE/usr/bin/dbee-cli" --version
fi
echo "==> done"
ls -lh "$DIST"

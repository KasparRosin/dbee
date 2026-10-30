#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/dbee.svg. Sizes of 64 px and up are rendered from the
# SVG; every size below 64 is the 64 px render resized down, so there is one drawing at all sizes.
#
# Needs: resvg (apt install resvg / cargo install resvg) and ImageMagick (apt install imagemagick).
# The outputs are committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/dbee.svg"
ID="dev.rosin.dbee"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v magick >/dev/null || { echo "error: magick not found (apt install imagemagick)" >&2; exit 1; }
command -v resvg >/dev/null || { echo "error: resvg not found (apt install resvg / cargo install resvg)" >&2; exit 1; }

# The artwork is a full-bleed 512 tile (rx=112). Windows and Linux use it as is. macOS icons sit on
# Apple's grid: an 824/1024 body with a transparent margin, so widen the viewBox to 512*1024/824.
mac() {
  sed 's/viewBox="0 0 512 512"/viewBox="-62.136 -62.136 636.272 636.272"/' "$1" >"$2"
  grep -q 'viewBox="-62.136' "$2" || { echo "error: expected viewBox=\"0 0 512 512\" in $1" >&2; exit 1; }
}
MAC="$TMP/mac.svg"
mac "$SVG" "$MAC"

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }
# Sizes of 64 px and up come from the SVG; smaller ones are the 64 px render resized down. $1 SVG, $2 size, $3 output.
icon() {
  if [ "$2" -ge 64 ]; then render "$1" "$2" "$3"; else
    render "$1" 64 "$TMP/base-64.png"
    magick "$TMP/base-64.png" -filter Lanczos -resize "${2}x${2}" "$3"
  fi
}

render "$SVG" 1024 "$DIR/dbee-1024.png"
# Runtime window/Dock icon on macOS (for apps/dbee to embed); other platforms use hicolor 256.
render "$MAC" 512 "$DIR/dbee-macos-512.png"

# Linux hicolor theme.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  icon "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$SVG" "$DIR/hicolor/scalable/apps/$ID.svg"

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  icon "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/dbee.ico" "${ICO_PNGS[@]}")

# macOS .icns, on any OS (from the macOS-grid variant, like the runtime icon).
ICNS_PNGS=()
for s in 16 32 64 128 256 512 1024; do
  icon "$MAC" "$s" "$TMP/icns-$s.png"
  ICNS_PNGS+=("$TMP/icns-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- icns "$DIR/dbee.icns" "${ICNS_PNGS[@]}")
echo "icons written to $DIR"

#!/usr/bin/env bash
# Generate local build icon assets from the single replaceable source PNG.
#
# Source of truth:
#   crates/fleet-host/icons/icon.png
#
# Generated and ignored:
#   crates/fleet-host/icons/32x32.png
#   crates/fleet-host/icons/128x128.png
#   crates/fleet-host/icons/Fleet.icns
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:-$HERE/icons/icon.png}"
OUT="$HERE/icons"
STRICT=0

if [ "${1:-}" = "--strict" ]; then
  STRICT=1
  SRC="${2:-$HERE/icons/icon.png}"
fi

note() { printf '[icons] %s\n' "$*" >&2; }
soft_fail() {
  if [ "$STRICT" -eq 1 ]; then
    note "error: $*"
    exit 1
  fi
  note "warning: $*"
  exit 0
}

[ -f "$SRC" ] || soft_fail "source PNG not found: $SRC"
command -v sips >/dev/null || soft_fail "macOS sips not found; keeping existing derived icons"

mkdir -p "$OUT"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# This both validates that the file is a readable image and normalizes whatever
# PNG flavor the user dropped in. Alpha is allowed but not required.
NORMALIZED="$TMP/icon.png"
sips -s format png "$SRC" --out "$NORMALIZED" >/dev/null 2>&1 \
  || soft_fail "source is not a readable PNG: $SRC"

BASE="$TMP/icon-512.png"
sips -z 512 512 "$NORMALIZED" --out "$BASE" >/dev/null 2>&1 \
  || soft_fail "failed to normalize source to 512x512"

sips -z 32 32 "$BASE" --out "$OUT/32x32.png" >/dev/null 2>&1 \
  || soft_fail "failed to write $OUT/32x32.png"
sips -z 128 128 "$BASE" --out "$OUT/128x128.png" >/dev/null 2>&1 \
  || soft_fail "failed to write $OUT/128x128.png"

sips -s format icns "$BASE" --out "$OUT/Fleet.icns" >/dev/null 2>&1 \
  || soft_fail "failed to write $OUT/Fleet.icns"

note "refreshed derived icons from $SRC"

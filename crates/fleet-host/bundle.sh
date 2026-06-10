#!/usr/bin/env bash
# Assemble a minimal macOS .app bundle around the built fleet-host binary, so the
# window runs under LaunchServices and PERSISTS (a bare binary launched from a
# CLI dies when its launching shell/session goes away; an .app does not).
#
# Usage:  ./bundle.sh [debug|release]   (default: release)
# Result: ./Fleet.app  →  launch with `open ./Fleet.app`
#         (custom hub:  open ./Fleet.app --args ws://host:port)
#
# Lightweight, nix-friendly path (plain cargo + file copies, no tauri-cli). For a
# signed, distributable .app/.dmg, `cargo tauri build` is the production route.
set -eo pipefail

PROFILE="${1:-release}"
HERE="$(cd "$(dirname "$0")" && pwd)"
APP="$HERE/Fleet.app"
BIN="$HERE/target/$PROFILE/fleet-host"
BRIDGE_VSIX="$HERE/../../packages/fleet-bridge/fleet-bridge-0.2.0.vsix"

echo "building fleet-host ($PROFILE)..."
if [ "$PROFILE" = "release" ]; then
  ( cd "$HERE" && cargo build --release )
else
  ( cd "$HERE" && cargo build )
fi
[ -x "$BIN" ] || { echo "binary not found: $BIN"; exit 1; }

echo "assembling $APP..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/fleet-host"
if [ -f "$BRIDGE_VSIX" ]; then
  cp "$BRIDGE_VSIX" "$APP/Contents/Resources/fleet-bridge.vsix"
else
  echo "warning: fleet-bridge VSIX not found at $BRIDGE_VSIX"
fi

# Convert the source PNG to local build icons. This is deliberately driven from
# icons/icon.png so replacing that one file is enough.
ICON_PRESENT=0
"$HERE/scripts/refresh-icons.sh" --strict
if [ -f "$HERE/icons/Fleet.icns" ]; then
  cp "$HERE/icons/Fleet.icns" "$APP/Contents/Resources/Fleet.icns"
  ICON_PRESENT=1
fi

{
  echo '<?xml version="1.0" encoding="UTF-8"?>'
  echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
  echo '<plist version="1.0">'
  echo '<dict>'
  echo '  <key>CFBundleName</key><string>Fleet</string>'
  echo '  <key>CFBundleDisplayName</key><string>Fleet</string>'
  echo '  <key>CFBundleExecutable</key><string>fleet-host</string>'
  echo '  <key>CFBundleIdentifier</key><string>dev.fleet.host</string>'
  echo '  <key>CFBundlePackageType</key><string>APPL</string>'
  echo '  <key>CFBundleShortVersionString</key><string>0.1.0</string>'
  echo '  <key>CFBundleVersion</key><string>1</string>'
  echo '  <key>NSHighResolutionCapable</key><true/>'
  echo '  <key>LSMinimumSystemVersion</key><string>10.15</string>'
  if [ "$ICON_PRESENT" -eq 1 ]; then
    echo '  <key>CFBundleIconFile</key><string>Fleet</string>'
    echo '  <key>CFBundleIconName</key><string>Fleet</string>'
  fi
  echo '</dict>'
  echo '</plist>'
} > "$APP/Contents/Info.plist"

touch "$APP"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [ -x "$LSREGISTER" ]; then
  "$LSREGISTER" -f "$APP" >/dev/null 2>&1 || true
fi

echo "done → $APP"
echo "launch:  open '$APP'        (persists across terminals; close from its window)"
echo "custom:  open '$APP' --args ws://127.0.0.1:51777"

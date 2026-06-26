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
ROOT="$(cd "$HERE/../.." && pwd)"
APP="$HERE/Fleet.app"
BIN="$HERE/target/$PROFILE/fleet-host"
REPORTER_BIN="$ROOT/target/$PROFILE/fleet-reporter"
BRIDGE_VSIX="$HERE/../../packages/fleet-bridge/fleet-bridge-0.2.0.vsix"
BRIDGE_PACKAGE="$HERE/../../packages/fleet-bridge/package-vsix.sh"
BUILD_VERSION="$(date -u +%Y%m%d%H%M%S)"

# Convert the source PNG to local build icons before compiling. The Rust host
# embeds icons/128x128.png at compile time for the native window/app icon.
"$HERE/scripts/refresh-icons.sh" --strict

echo "building fleet-host ($PROFILE)..."
if [ "$PROFILE" = "release" ]; then
  ( cd "$HERE" && cargo build --release )
  ( cd "$ROOT" && cargo build --release -p fleet-reporter )
else
  ( cd "$HERE" && cargo build )
  ( cd "$ROOT" && cargo build -p fleet-reporter )
fi
[ -x "$BIN" ] || { echo "binary not found: $BIN"; exit 1; }

if [ -x "$BRIDGE_PACKAGE" ]; then
  "$BRIDGE_PACKAGE"
else
  echo "warning: fleet-bridge packer not found at $BRIDGE_PACKAGE"
fi

echo "assembling $APP..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/fleet-host"
if [ -x "$REPORTER_BIN" ]; then
  cp "$REPORTER_BIN" "$APP/Contents/MacOS/fleet-reporter"
else
  echo "warning: fleet-reporter binary not found at $REPORTER_BIN"
fi
if [ -f "$BRIDGE_VSIX" ]; then
  cp "$BRIDGE_VSIX" "$APP/Contents/Resources/fleet-bridge.vsix"
else
  echo "warning: fleet-bridge VSIX not found at $BRIDGE_VSIX"
fi

# Copy the generated .icns into the app bundle. This is deliberately driven from
# icons/icon.png so replacing that one file is enough.
ICON_PRESENT=0
if [ -f "$HERE/icons/Fleet.icns" ]; then
  cp "$HERE/icons/Fleet.icns" "$APP/Contents/Resources/Fleet.icns"
  sips -g pixelWidth -g pixelHeight "$APP/Contents/Resources/Fleet.icns" >/dev/null
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
  echo '  <key>CFBundleShortVersionString</key><string>0.1.2</string>'
  echo "  <key>CFBundleVersion</key><string>$BUILD_VERSION</string>"
  echo '  <key>NSHighResolutionCapable</key><true/>'
  echo '  <key>LSMinimumSystemVersion</key><string>10.15</string>'
  if [ "$ICON_PRESENT" -eq 1 ]; then
    echo '  <key>CFBundleIconFile</key><string>Fleet.icns</string>'
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

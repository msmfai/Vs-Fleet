#!/usr/bin/env bash
# Build Linux release bundles inside a rust:1 container (invoked by docker run
# with the repo mounted at /src). Mirrors the release.yml Linux lane: stage the
# fleet-reporter sidecar + bridge VSIX, tauri build deb/rpm (+appimage,
# best-effort under docker), run the launch smoke under Xvfb, then copy
# artifacts to /src/dist-local/.
set -euo pipefail

PLATFORM_LABEL="${1:?usage: local-linux-release.sh <platform-label>}"

apt-get update -qq >/dev/null
apt-get install -y -qq libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev xvfb imagemagick curl ca-certificates file >/dev/null
curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null 2>&1
apt-get install -y -qq nodejs >/dev/null
echo "node $(node --version)"

cd /src
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
echo "building for $TRIPLE ($PLATFORM_LABEL)"

# Container-local target dirs: do not thrash the host's macOS build caches.
export CARGO_TARGET_DIR=/tmp/ct-root
cargo build --release -p fleet-reporter 2>&1 | tail -1
mkdir -p crates/fleet-host/binaries crates/fleet-host/resources
cp "/tmp/ct-root/release/fleet-reporter" "crates/fleet-host/binaries/fleet-reporter-$TRIPLE"
cp packages/fleet-bridge/fleet-bridge-0.2.0.vsix crates/fleet-host/resources/fleet-bridge.vsix

cd crates/fleet-host
export CARGO_TARGET_DIR=/tmp/ct-host
export APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1
npx --yes @tauri-apps/cli@^2 build --config tauri.release.conf.json --bundles deb,rpm 2>&1 | tail -3
npx --yes @tauri-apps/cli@^2 build --config tauri.release.conf.json --bundles appimage 2>&1 | tail -3 \
  || echo "WARN: appimage bundling failed under docker; shipping deb/rpm only"

cd /src
xvfb-run -a node scripts/release-smoke.mjs --bin /tmp/ct-host/release/fleet-host --out "/tmp/smoke-$PLATFORM_LABEL"
cp "/tmp/smoke-$PLATFORM_LABEL/smoke-report.json" "/src/dist-local/smoke-report-$PLATFORM_LABEL.json" || true

mkdir -p /src/dist-local
find /tmp/ct-host/release/bundle -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) | while read -r f; do
  base="$(basename "$f")"
  cp "$f" "/src/dist-local/${base%.*}-$PLATFORM_LABEL.${base##*.}"
done
ls -la /src/dist-local/
echo "DONE $PLATFORM_LABEL"

#!/usr/bin/env bash
# Fleet environment entrypoint: phone home to the Hub on the host, then serve the
# editor. The host is the container's default-route gateway (Apple Containers /
# Docker bridge) unless FLEET_HOST_ADDR overrides it.
set -euo pipefail

HOST="${FLEET_HOST_ADDR:-$(ip route 2>/dev/null | awk '/default/ {print $3; exit}')}"
HOST="${HOST:-192.168.64.1}"
ID="${FLEET_SERVER_ID:-env-$(hostname)}"

export FLEET_HUB_URL="ws://${HOST}:${FLEET_HUB_PORT:-51777}"
export FLEET_BRIDGE_URL="ws://${HOST}:${FLEET_BRIDGE_PORT:-51778}"
export FLEET_SERVER_ID="$ID"
export FLEET_SERVER_LABEL="${FLEET_SERVER_LABEL:-$ID}"
export FLEET_SESSION_TITLE="$ID"   # so the Hub session is titled by env id
export FLEET_SERVER_URL="${FLEET_SERVER_URL:-}"   # Fleet fills the host-reachable URL
export FLEET_REPORTER_SOCKET="${FLEET_REPORTER_SOCKET:-/tmp/fleet-reporter.sock}"

echo "[fleet-env] id=$ID host=$HOST hub=$FLEET_HUB_URL bridge=$FLEET_BRIDGE_URL"

# Phone home: the reporter registers this env as a session and listens on the
# socket for the claude lifecycle hooks (see /etc/fleet/hooks.json).
rm -f "$FLEET_REPORTER_SOCKET"
fleet-reporter --serve \
  --ws "$FLEET_HUB_URL" \
  --socket "$FLEET_REPORTER_SOCKET" \
  --session-id "$ID" &

WORKSPACE="${FLEET_WORKSPACE:-/home/coder/project}"
mkdir -p "$WORKSPACE"

# Serve the editor. The fleet-bridge extension dials FLEET_BRIDGE_URL on startup
# (command forwarding); the `claude` shell function (~/.bashrc) adds the hooks.
exec code-server \
  --auth none \
  --disable-telemetry \
  --disable-update-check \
  --bind-addr 0.0.0.0:8080 \
  "$WORKSPACE"

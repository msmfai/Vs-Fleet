#!/usr/bin/env bash
# Headless integration test: launch a FLEET of fleet-env containers in parallel,
# assert each phones home to the Hub + serves its editor, and collect diagnostics
# from all of them. No GUI — the containers self-report (Docker/colima runtime).
#
#   containers/fleet-env/test.sh [N]      # N = number of environments (default 3)
#
# Requires: the `fleet-env:latest` image (docker build) + a running docker daemon
# (colima start) + the workspace binaries (target/debug/{fleet-hub,fleet-cli}).
#
# Networking (colima): containers run in the Lima VM, so host→container uses
# published ports (-p), and container→host is host.docker.internal. The Hub binds
# 0.0.0.0 so the containers can reach it.
set -uo pipefail

N="${1:-3}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HUB="$ROOT/target/debug/fleet-hub"
CLI="$ROOT/target/debug/fleet"   # fleet-cli's binary is named `fleet`
OUT="${FLEET_TEST_OUT:-/tmp/fleet-test}"
BASE_PORT=8090
NAMES=(); for i in $(seq 1 "$N"); do NAMES+=("fleet-test-$i"); done
rm -rf "$OUT"; mkdir -p "$OUT"

say() { printf '\033[1;36m[test]\033[0m %s\n' "$*"; }
cleanup() {
  say "cleanup: removing ${#NAMES[@]} containers + hub"
  for n in "${NAMES[@]}"; do docker rm -f "$n" >/dev/null 2>&1; done
  [ -n "${HUB_PID:-}" ] && kill "$HUB_PID" 2>/dev/null
}
trap cleanup EXIT

# 1) Fresh, ephemeral Hub bound on 0.0.0.0 so containers reach it.
say "starting Hub on 0.0.0.0:51777 (ephemeral)"
pkill -f 'target/debug/fleet-hub' 2>/dev/null; sleep 2
FLEET_WS_ADDR=0.0.0.0 RUST_LOG=warn "$HUB" >"$OUT/hub.log" 2>&1 & HUB_PID=$!
for _ in $(seq 1 20); do nc -z 127.0.0.1 51777 2>/dev/null && break; sleep 0.5; done
nc -z 127.0.0.1 51777 2>/dev/null || { say "ERROR: Hub not listening on 51777 (lock held?)"; exit 1; }

# 2) Launch N environment containers in parallel — each phones home on its own.
# Wait only for the launch jobs (a bare `wait` would also block on the Hub).
say "launching $N fleet-env containers in parallel"
pids=()
for i in $(seq 1 "$N"); do
  docker rm -f "fleet-test-$i" >/dev/null 2>&1
  docker run -d --name "fleet-test-$i" \
    -e "FLEET_SERVER_ID=env-$i" \
    -e "FLEET_HOST_ADDR=host.docker.internal" \
    -p "$((BASE_PORT+i)):8080" \
    fleet-env:latest >/dev/null 2>&1 &
  pids+=($!)
done
wait "${pids[@]}"

# 3) Assert phone-home: poll the Hub until all N sessions appear (or time out).
say "waiting for $N environments to phone home…"
PHONED=0; SNAP=""
for _ in $(seq 1 60); do
  SNAP="$("$CLI" ls --once 2>/dev/null)"
  PHONED=$(printf '%s\n' "$SNAP" | grep -cE 'env-[0-9]+')
  [ "$PHONED" -ge "$N" ] && break
  sleep 1
done
printf '%s\n' "$SNAP" > "$OUT/hub-inbox.txt"
say "phoned home: $PHONED / $N  (snapshot → $OUT/hub-inbox.txt)"

# 4) Per-container: editor reachability (published port) + diagnostics.
PASS=0
for i in $(seq 1 "$N"); do
  n="fleet-test-$i"; port="$((BASE_PORT+i))"
  # code-server's HTTP comes up a few seconds after the reporter phones home — retry.
  code=000
  for _ in $(seq 1 25); do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:$port/" 2>/dev/null)
    { [ "$code" = "302" ] || [ "$code" = "200" ]; } && break
    sleep 1
  done
  docker logs "$n" > "$OUT/$n.log" 2>&1
  phoned=$(printf '%s' "$SNAP" | grep -q "env-$i" && echo yes || echo no)
  ok="no"; { [ "$code" = "302" ] || [ "$code" = "200" ]; } && [ "$phoned" = "yes" ] && { ok="yes"; PASS=$((PASS+1)); }
  say "  $n  port=$port  http=$code  phoned=$phoned  → $ok"
done

say "RESULT: $PASS / $N environments healthy (phoned home + editor reachable)"
say "diagnostics in $OUT/  (hub.log, hub-inbox.txt, fleet-test-N.log)"
[ "$PASS" -ge "$N" ]

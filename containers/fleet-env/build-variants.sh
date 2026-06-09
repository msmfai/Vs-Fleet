#!/usr/bin/env bash
# Build the Fleet language-variant images (Track G, §7 of eval/PLAN.md):
#   fleet-env-python:latest  fleet-env-node:latest  fleet-env-rust:latest
#
# Each variant is `FROM fleet-env:latest` + a language toolchain + a matching
# Open-VSX extension. The base image MUST exist first (the variants inherit it),
# so this builds the base unless --skip-base is passed (or it's already present).
#
# §8 gotcha: build with **Docker** (+ colima), never Apple `container build` — its
# context-snapshot truncation persists a corrupt ref across builder deletes. Run
# `colima start --cpu 4 --memory 8` first if colima isn't up.
#
#   ./containers/fleet-env/build-variants.sh                 # base + all variants
#   ./containers/fleet-env/build-variants.sh python rust     # base (if needed) + these
#   ./containers/fleet-env/build-variants.sh --skip-base node
#
# Run from the repo root (the base Containerfile needs the repo as build context).
set -euo pipefail

# ── locate repo root (this script lives at <root>/containers/fleet-env/) ────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_DIR="$SCRIPT_DIR"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

DOCKER="${DOCKER:-docker}"
BASE_IMAGE="fleet-env:latest"
SKIP_BASE=0
WANT=()

for arg in "$@"; do
  case "$arg" in
    --skip-base) SKIP_BASE=1 ;;
    python|node|rust) WANT+=("$arg") ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \{0,1\}//' ; exit 0 ;;
    *) echo "build-variants: unknown arg '$arg' (want: python|node|rust|--skip-base)" >&2; exit 2 ;;
  esac
done
# default: all three variants
if [ "${#WANT[@]}" -eq 0 ]; then WANT=(python node rust); fi

echo "[build-variants] repo-root=$REPO_ROOT  variants=${WANT[*]}"

# ── 1. base image (variants are FROM it) ────────────────────────────────────────
if [ "$SKIP_BASE" -eq 0 ]; then
  if "$DOCKER" image inspect "$BASE_IMAGE" >/dev/null 2>&1; then
    echo "[build-variants] base $BASE_IMAGE already present — skipping (pass --skip-base to silence, or 'docker rmi' to force rebuild)"
  else
    echo "[build-variants] building base $BASE_IMAGE …"
    "$DOCKER" build -t "$BASE_IMAGE" \
      -f "$ENV_DIR/Containerfile" "$REPO_ROOT"
  fi
else
  echo "[build-variants] --skip-base: assuming $BASE_IMAGE exists"
fi

if ! "$DOCKER" image inspect "$BASE_IMAGE" >/dev/null 2>&1; then
  echo "[build-variants] ERROR: base $BASE_IMAGE missing — build it first (drop --skip-base)" >&2
  exit 1
fi

# ── 2. each language variant ────────────────────────────────────────────────────
# Variant build context is the env dir (the Containerfile.<lang> only needs FROM the
# base + network installs — no repo files), which keeps the context tiny.
for lang in "${WANT[@]}"; do
  tag="fleet-env-${lang}:latest"
  containerfile="$ENV_DIR/Containerfile.${lang}"
  if [ ! -f "$containerfile" ]; then
    echo "[build-variants] ERROR: $containerfile not found" >&2; exit 1
  fi
  echo "[build-variants] building $tag from $containerfile …"
  "$DOCKER" build -t "$tag" -f "$containerfile" "$ENV_DIR"
  echo "[build-variants] built $tag"
done

echo "[build-variants] done: ${WANT[*]/#/fleet-env-}  (-> *:latest)"

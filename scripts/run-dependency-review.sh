#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: dependency review must run inside a git worktree" >&2
  exit 2
fi

out="${1:--}"
tmp="${TMPDIR:-/tmp}/fleet-dependency-review-$(git -C "$root" rev-parse --short=12 HEAD)"
mkdir -p "$tmp"

artifact_pattern='(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/'

run_logged() {
  local label=$1
  local logfile=$2
  shift 2
  echo "review: $label"
  if ! "$@" >"$tmp/$logfile" 2>&1; then
    echo "FAIL: $label failed; see $tmp/$logfile" >&2
    sed -n '1,160p' "$tmp/$logfile" >&2
    exit 1
  fi
}

run_logged "cargo tree" cargo-tree.txt \
  cargo tree --workspace --all-features

run_logged "cargo metadata --locked" fleet-cargo-metadata.json \
  cargo metadata --format-version 1 --locked

run_logged "fleet-host cargo metadata --locked" fleet-host-cargo-metadata.json \
  bash -c 'cd "$1/crates/fleet-host" && cargo metadata --format-version 1 --locked' bash "$root"

run_logged "lockfile policy" lockfile-policy.txt \
  "$root/scripts/check-lockfile-policy.sh"

run_logged "fleet-bridge npm audit" fleet-bridge-npm-audit.txt \
  bash -c 'cd "$1/packages/fleet-bridge" && npm ci && npm audit --audit-level=moderate' bash "$root"

run_logged "extension npm audit" extension-npm-audit.txt \
  bash -c 'cd "$1/packages/extension" && npm ci && npm audit --audit-level=moderate' bash "$root"

echo "review: generated artifact check"
tracked_artifacts="$(git -C "$root" ls-files | rg "$artifact_pattern" || true)"
if [ -n "$tracked_artifacts" ]; then
  echo "FAIL: generated artifacts are tracked:" >&2
  printf '%s\n' "$tracked_artifacts" >&2
  exit 1
fi

commit="$(git -C "$root" rev-parse HEAD)"
reviewed_date="$(date -u +%Y-%m-%d)"
report="$(
  cat <<EOF
# Dependency Review

Dependency review status: PASS

This report records the dependency review checks for the current commit. Keep
the command output or release notes with the release, but do not commit a
repo-local evidence file.

Commit: \`$commit\`
Reviewed date: \`$reviewed_date\`

## Command Results

cargo tree: \`pass\`
cargo metadata --locked: \`pass\`
fleet-host cargo metadata --locked: \`pass\`
lockfile policy: \`pass\`
fleet-bridge npm audit: \`pass\`
extension npm audit: \`pass\`
generated artifact check: \`pass\`
Accepted findings: \`none\`
EOF
)"

if [ "$out" = "-" ]; then
  printf '%s\n' "$report"
  echo "Command logs: $tmp" >&2
else
  if [ -f "$out" ] && [ "${FLEET_DEPENDENCY_REVIEW_FORCE:-0}" != "1" ]; then
    echo "FAIL: dependency review report already exists: $out" >&2
    echo "Set FLEET_DEPENDENCY_REVIEW_FORCE=1 to overwrite." >&2
    exit 1
  fi
  mkdir -p "$(dirname "$out")"
  printf '%s\n' "$report" >"$out"
  echo "Dependency review report written to $out"
  echo "Command logs: $tmp"
fi

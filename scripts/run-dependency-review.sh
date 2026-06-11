#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: dependency review must run inside a git worktree" >&2
  exit 2
fi

out="${1:-$root/docs/release/DEPENDENCY_REVIEW_EVIDENCE.md}"
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

mkdir -p "$(dirname "$out")"
cat >"$out" <<EOF
# Dependency Review Evidence

Dependency review status: PASS

This file records dependency review evidence for the exact commit that will
become the first public GitHub alpha. Do not mark the owner decision record
\`APPROVED\` until this file is concrete and
\`scripts/check-dependency-review-decision.sh\` passes.

Commit: \`$commit\`
Reviewed date: \`$reviewed_date\`

## Command Evidence

Use this section if the owner decision record chooses to run the dependency
review commands.

cargo tree: \`pass\`
cargo metadata --locked: \`pass\`
fleet-host cargo metadata --locked: \`pass\`
lockfile policy: \`pass\`
fleet-bridge npm audit: \`pass\`
extension npm audit: \`pass\`
generated artifact check: \`pass\`
Accepted findings: \`none\`

## Skipped Review Evidence

Use this section only if the owner explicitly accepts publishing the first
source alpha without dependency review.

Accepted risk: \`not used\`

## Other Evidence

Use this section only if the owner records a concrete \`Other\` dependency review
decision.

Dependency review evidence path: \`not used\`
EOF

echo "Dependency review evidence written to $out"
echo "Command logs: $tmp"

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

draft="$TMPDIR/OWNER_DECISION_RECORD.md"
stdout="$TMPDIR/stdout.md"

if ! "$ROOT/scripts/draft-owner-decisions.sh" example vs-fleet >"$stdout"; then
  echo "FAIL: expected stdout draft generation to pass" >&2
  exit 1
fi

for pattern in \
  '^Decision record status: PENDING$' \
  '^- \[x\] MIT OR Apache-2.0 dual license\.$' \
  '^- \[x\] Publish a cleaned/squashed history for the first public branch\.$' \
  '^\| GitHub org/user \| example \|$' \
  '^\| GitHub repo name \| vs-fleet \|$' \
  '^- \[x\] Source-only alpha\.' \
  '^- \[x\] `Fleet` name and current icon are alpha placeholders\.$' \
  '^- \[x\] Alpha pre-release tags only\.' \
  '^- \[x\] Open public issues only for scoped bug reports and alpha feedback;' \
  '^- \[x\] Single-maintainer alpha\.' \
  '^- \[x\] Allow AI-assisted contributions if the contributor certifies human review,'
do
  if ! rg -q "$pattern" "$stdout"; then
    echo "FAIL: stdout draft missing expected pattern: $pattern" >&2
    cat "$stdout" >&2
    exit 1
  fi
done

if ! "$ROOT/scripts/draft-owner-decisions.sh" example vs-fleet "$draft" >"$TMPDIR/write.out"; then
  echo "FAIL: expected file draft generation to pass" >&2
  cat "$TMPDIR/write.out" >&2
  exit 1
fi

if [ ! -f "$draft" ]; then
  echo "FAIL: draft file was not written" >&2
  exit 1
fi

if "$ROOT/scripts/draft-owner-decisions.sh" example vs-fleet "$draft" >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: existing draft should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! FLEET_OWNER_DRAFT_FORCE=1 "$ROOT/scripts/draft-owner-decisions.sh" example renamed "$draft" >"$TMPDIR/force.out"; then
  echo "FAIL: forced draft overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

if ! rg -q '^\| GitHub repo name \| renamed \|$' "$draft"; then
  echo "FAIL: forced draft overwrite did not update the repo name" >&2
  cat "$draft" >&2
  exit 1
fi

if "$ROOT/scripts/public-alpha-decision-packet.sh" "$draft" >"$TMPDIR/packet-pending.out" 2>&1; then
  echo "FAIL: generated draft must remain blocked until the owner approves it" >&2
  cat "$TMPDIR/packet-pending.out" >&2
  exit 1
fi

if ! rg -q 'Decision record status: not APPROVED' "$TMPDIR/packet-pending.out"; then
  echo "FAIL: packet should explain that the generated draft is not approved" >&2
  cat "$TMPDIR/packet-pending.out" >&2
  exit 1
fi

approved="$TMPDIR/approved.md"
cp "$draft" "$approved"
perl -0pi -e 's/^Decision record status: PENDING$/Decision record status: APPROVED/m' "$approved"

if ! "$ROOT/scripts/check-owner-decisions.sh" "$approved" >"$TMPDIR/check-approved.out" 2>&1; then
  echo "FAIL: approved generated draft should satisfy owner choice completeness" >&2
  cat "$TMPDIR/check-approved.out" >&2
  exit 1
fi

if "$ROOT/scripts/draft-owner-decisions.sh" 'bad`owner' repo >"$TMPDIR/bad.out" 2>&1; then
  echo "FAIL: unsafe owner text should be rejected" >&2
  cat "$TMPDIR/bad.out" >&2
  exit 1
fi

echo "Owner decision draft tests passed."

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"
cp "$ROOT/scripts/release-evidence-status.sh" "$repo/scripts/release-evidence-status.sh"
chmod +x "$repo/scripts/release-evidence-status.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

write_complete() {
  cat >"$repo/docs/release/PUBLIC_BRANCH_EVIDENCE.md" <<'EOF'
# Public Branch Evidence
Public branch evidence status: PASS
Source commit: `0123456789abcdef0123456789abcdef01234567`
Public branch: `public-alpha`
EOF
  cat >"$repo/docs/release/PUBLIC_CI_EVIDENCE.md" <<'EOF'
# Public CI Evidence
Public CI evidence status: PASS
Commit: `0123456789abcdef0123456789abcdef01234567`
EOF
  cat >"$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md" <<'EOF'
# GitHub Publication Evidence
GitHub publication evidence status: PASS
Commit: `0123456789abcdef0123456789abcdef01234567`
EOF
  cat >"$repo/docs/release/DEPENDENCY_REVIEW_EVIDENCE.md" <<'EOF'
# Dependency Review Evidence
Dependency review status: PASS
Commit: `0123456789abcdef0123456789abcdef01234567`
EOF
}

write_complete
git -C "$repo" add .
git -C "$repo" commit -q -m "complete evidence"

output="$TMPDIR/status.out"
if ! (cd "$repo" && ./scripts/release-evidence-status.sh) >"$output" 2>&1; then
  echo "FAIL: complete evidence should pass" >&2
  cat "$output" >&2
  exit 1
fi

for expected in \
  "Public branch evidence: PASS" \
  "Public CI evidence: PASS" \
  "GitHub publication evidence: PASS" \
  "Dependency review evidence: PASS" \
  "Release evidence status: COMPLETE"
do
  if ! rg -Fq "$expected" "$output"; then
    echo "FAIL: missing expected status output: $expected" >&2
    cat "$output" >&2
    exit 1
  fi
done

perl -0pi -e 's/Public CI evidence status: PASS/Public CI evidence status: PENDING\nCI workflow run: TODO/' \
  "$repo/docs/release/PUBLIC_CI_EVIDENCE.md"
if (cd "$repo" && ./scripts/release-evidence-status.sh) >"$TMPDIR/pending.out" 2>&1; then
  echo "FAIL: pending evidence should fail" >&2
  cat "$TMPDIR/pending.out" >&2
  exit 1
fi
if ! rg -q 'Public CI evidence: PENDING, placeholders remain|Release evidence status: BLOCKED' "$TMPDIR/pending.out"; then
  echo "FAIL: pending output should identify placeholder evidence" >&2
  cat "$TMPDIR/pending.out" >&2
  exit 1
fi

write_complete
rm "$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md"
if (cd "$repo" && ./scripts/release-evidence-status.sh) >"$TMPDIR/missing.out" 2>&1; then
  echo "FAIL: missing evidence should fail" >&2
  cat "$TMPDIR/missing.out" >&2
  exit 1
fi
if ! rg -q 'GitHub publication evidence: MISSING' "$TMPDIR/missing.out"; then
  echo "FAIL: missing output should identify missing evidence file" >&2
  cat "$TMPDIR/missing.out" >&2
  exit 1
fi

write_complete
perl -ni -e 'print unless /^Dependency review status:/' "$repo/docs/release/DEPENDENCY_REVIEW_EVIDENCE.md"
if (cd "$repo" && ./scripts/release-evidence-status.sh) >"$TMPDIR/missing-status.out" 2>&1; then
  echo "FAIL: missing status evidence should fail" >&2
  cat "$TMPDIR/missing-status.out" >&2
  exit 1
fi
if ! rg -q 'Dependency review evidence: MISSING STATUS' "$TMPDIR/missing-status.out"; then
  echo "FAIL: missing-status output should identify the bad evidence file" >&2
  cat "$TMPDIR/missing-status.out" >&2
  exit 1
fi

echo "Release evidence status tests passed."

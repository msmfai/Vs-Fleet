#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"
cp "$ROOT/scripts/check-public-release-branch.sh" "$repo/scripts/check-public-release-branch.sh"
chmod +x "$repo/scripts/check-public-release-branch.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "source fixture"
source_commit="$(git -C "$repo" rev-parse HEAD)"
public_commit="$(git -C "$repo" commit-tree HEAD^{tree} -m "clean public snapshot")"
git -C "$repo" branch public-alpha "$public_commit"

cat >"$repo/scripts/history-release-check.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = "docs/release/OWNER_DECISION_RECORD.md" ]
[ "$2" = "public-alpha" ]
echo "history ok"
EOF

cat >"$repo/scripts/check-public-branch-evidence.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail
[ "\$1" = "docs/release/OWNER_DECISION_RECORD.md" ]
[ "\$2" = "docs/release/PUBLIC_BRANCH_EVIDENCE.md" ]
[ "\$3" = "$source_commit" ]
echo "evidence ok"
EOF

cat >"$repo/scripts/secret-release-check.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = "public-alpha" ]
echo "secret ok"
EOF

cat >"$repo/scripts/release-check.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "${FLEET_RELEASE_HISTORY_REF:-}" = "public-alpha" ]
echo "release ok"
EOF

chmod +x "$repo/scripts/"*.sh

output="$TMPDIR/check-public-release-branch.out"
if ! (cd "$repo" && ./scripts/check-public-release-branch.sh public-alpha HEAD) >"$output" 2>&1; then
  echo "FAIL: expected public release branch check to pass" >&2
  cat "$output" >&2
  exit 1
fi

for expected in \
  "==> history release check" \
  "history ok" \
  "==> public branch evidence check" \
  "evidence ok" \
  "==> secret release check" \
  "secret ok" \
  "==> aggregate release check" \
  "release ok" \
  "Public release branch check passed."
do
  if ! rg -Fq "$expected" "$output"; then
    echo "FAIL: missing expected verifier output: $expected" >&2
    cat "$output" >&2
    exit 1
  fi
done

if (cd "$repo" && ./scripts/check-public-release-branch.sh public-alpha) >"$TMPDIR/missing.out" 2>&1; then
  echo "FAIL: missing source ref should be rejected" >&2
  cat "$TMPDIR/missing.out" >&2
  exit 1
fi

cat >"$repo/scripts/secret-release-check.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "secret failed"
exit 1
EOF
chmod +x "$repo/scripts/secret-release-check.sh"

if (cd "$repo" && ./scripts/check-public-release-branch.sh public-alpha HEAD) >"$TMPDIR/fail.out" 2>&1; then
  echo "FAIL: verifier should fail when an underlying gate fails" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

if ! rg -q 'secret failed|Public release branch check failed' "$TMPDIR/fail.out"; then
  echo "FAIL: verifier failure output should include underlying failure and summary" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

echo "Public release branch verifier tests passed."

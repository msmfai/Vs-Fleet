#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"
cp "$ROOT/scripts/generate-public-ci-evidence.sh" "$repo/scripts/generate-public-ci-evidence.sh"
cp "$ROOT/scripts/check-ci-evidence-decision.sh" "$repo/scripts/check-ci-evidence-decision.sh"
chmod +x "$repo/scripts/"*.sh

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

cat >"$repo/docs/release/OWNER_DECISION_RECORD.md" <<'EOF'
# Owner Decision Record

Decision record status: APPROVED

## Required Before Public GitHub Visibility

### 9. Public CI Evidence

- [x] Require GitHub Actions green on the exact branch/commit before public
  visibility.
- [ ] Accept local check output only for the first publish.
- [ ] Other: `TODO`

### 10. Privacy And Telemetry Posture
EOF

cat >"$repo/docs/release/PUBLIC_CI_EVIDENCE.md" <<'EOF'
# Public CI Evidence

Public CI evidence status: PENDING
Commit: `TODO`
Branch: `TODO`
CI workflow run: `TODO`
Release Readiness workflow run: `TODO`
EOF

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add .
git -C "$repo" commit -q -m "checked source"
source_commit="$(git -C "$repo" rev-parse HEAD)"

ci_url="https://github.com/example/fleet/actions/runs/123456788"
readiness_url="https://github.com/example/fleet/actions/runs/123456789"
evidence="$repo/docs/release/PUBLIC_CI_EVIDENCE.md"

if ! (cd "$repo" && ./scripts/generate-public-ci-evidence.sh public-alpha "$ci_url" "$readiness_url" HEAD "$evidence") >"$TMPDIR/generate.out" 2>&1; then
  echo "FAIL: expected public CI evidence generation to pass" >&2
  cat "$TMPDIR/generate.out" >&2
  exit 1
fi

for pattern in \
  '^Public CI evidence status: PASS$' \
  "^Commit: \`$source_commit\`$" \
  '^Release-control evidence file: `docs/release/PUBLIC_CI_EVIDENCE.md`$' \
  '^Branch: `public-alpha`$' \
  "^CI workflow run: \`$ci_url\`$" \
  "^Release Readiness workflow run: \`$readiness_url\`$" \
  '^Local check transcript: `not used`$' \
  '^CI evidence path: `not used`$'
do
  if ! rg -q "$pattern" "$evidence"; then
    echo "FAIL: generated CI evidence missing pattern: $pattern" >&2
    cat "$evidence" >&2
    exit 1
  fi
done

if ! (cd "$repo" && ./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md "$evidence" "$source_commit") >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: generated public CI evidence should pass checker" >&2
  cat "$TMPDIR/check.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-public-ci-evidence.sh public-alpha "$ci_url" "$readiness_url" HEAD "$evidence") >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: concrete CI evidence should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! rg -q 'FLEET_PUBLIC_CI_EVIDENCE_FORCE=1' "$TMPDIR/overwrite.out"; then
  echo "FAIL: overwrite rejection should explain the force override" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! (cd "$repo" && FLEET_PUBLIC_CI_EVIDENCE_FORCE=1 ./scripts/generate-public-ci-evidence.sh public-alpha "$ci_url" "$readiness_url" HEAD "$evidence") >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced CI evidence overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

git -C "$repo" add docs/release/PUBLIC_CI_EVIDENCE.md
git -C "$repo" commit -q -m "record CI evidence"
evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && ./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md "$evidence" "$evidence_commit") >"$TMPDIR/check-evidence-commit.out" 2>&1; then
  echo "FAIL: CI evidence commit should pass when only the CI evidence file differs" >&2
  cat "$TMPDIR/check-evidence-commit.out" >&2
  exit 1
fi

printf 'unexpected public payload drift\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "drift public payload"
drift_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && ./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md "$evidence" "$drift_commit") >"$TMPDIR/check-drift.out" 2>&1; then
  echo "FAIL: CI evidence check should reject source payload drift outside the evidence file" >&2
  cat "$TMPDIR/check-drift.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-public-ci-evidence.sh public-alpha "https://gitlab.com/example/fleet/-/pipelines/123" "$readiness_url" HEAD -) >"$TMPDIR/bad-url.out" 2>&1; then
  echo "FAIL: non-GitHub CI run URL should be rejected" >&2
  cat "$TMPDIR/bad-url.out" >&2
  exit 1
fi

echo "Public CI evidence generator tests passed."

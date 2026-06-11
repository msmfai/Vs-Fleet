#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
fakebin="$TMPDIR/bin"
evidence="$TMPDIR/DEPENDENCY_REVIEW_EVIDENCE.md"
output="$TMPDIR/dependency-review.out"

mkdir -p \
  "$repo/scripts" \
  "$repo/crates/fleet-host" \
  "$repo/packages/fleet-bridge" \
  "$repo/packages/extension" \
  "$fakebin"

cp "$ROOT/scripts/run-dependency-review.sh" "$repo/scripts/run-dependency-review.sh"
cp "$ROOT/scripts/check-lockfile-policy.sh" "$repo/scripts/check-lockfile-policy.sh"
chmod +x "$repo/scripts/run-dependency-review.sh" "$repo/scripts/check-lockfile-policy.sh"

cat >"$fakebin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  tree)
    printf 'fleet fake dependency tree\n'
    ;;
  metadata)
    printf '{"packages":[],"resolve":null}\n'
    ;;
  *)
    echo "unexpected cargo command: $*" >&2
    exit 9
    ;;
esac
EOF

cat >"$fakebin/npm" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  ci)
    printf 'fake npm ci ok\n'
    ;;
  audit)
    printf 'fake npm audit ok\n'
    ;;
  *)
    echo "unexpected npm command: $*" >&2
    exit 9
    ;;
esac
EOF
chmod +x "$fakebin/cargo" "$fakebin/npm"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# root cargo lock\n' >"$repo/Cargo.lock"
printf '# host cargo lock\n' >"$repo/crates/fleet-host/Cargo.lock"
printf 'lockfileVersion: "9.0"\n' >"$repo/pnpm-lock.yaml"
printf '{"lockfileVersion":3}\n' >"$repo/packages/fleet-bridge/package-lock.json"
printf '{"lockfileVersion":3}\n' >"$repo/packages/extension/package-lock.json"
git -C "$repo" add .
git -C "$repo" commit -q -m "fixture"

if ! PATH="$fakebin:$PATH" TMPDIR="$TMPDIR/logs" "$repo/scripts/run-dependency-review.sh" "$evidence" >"$output" 2>&1; then
  echo "FAIL: expected dependency review runner to pass" >&2
  cat "$output" >&2
  exit 1
fi

commit="$(git -C "$repo" rev-parse HEAD)"
for pattern in \
  '^Dependency review status: PASS$' \
  "^Commit: \`$commit\`$" \
  '^cargo tree: `pass`$' \
  '^cargo metadata --locked: `pass`$' \
  '^fleet-host cargo metadata --locked: `pass`$' \
  '^lockfile policy: `pass`$' \
  '^fleet-bridge npm audit: `pass`$' \
  '^extension npm audit: `pass`$' \
  '^generated artifact check: `pass`$' \
  '^Release-control evidence file: `../.*DEPENDENCY_REVIEW_EVIDENCE.md`$|^Release-control evidence file: `not tracked in this worktree`$' \
  '^Accepted findings: `none`$'
do
  if ! rg -q "$pattern" "$evidence"; then
    echo "FAIL: generated evidence missing pattern: $pattern" >&2
    cat "$evidence" >&2
    exit 1
  fi
done

if PATH="$fakebin:$PATH" TMPDIR="$TMPDIR/logs" "$repo/scripts/run-dependency-review.sh" "$evidence" >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: concrete dependency review evidence should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! rg -q 'FLEET_DEPENDENCY_REVIEW_FORCE=1' "$TMPDIR/overwrite.out"; then
  echo "FAIL: overwrite rejection should explain the force override" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! PATH="$fakebin:$PATH" TMPDIR="$TMPDIR/logs" FLEET_DEPENDENCY_REVIEW_FORCE=1 "$repo/scripts/run-dependency-review.sh" "$evidence" >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced dependency review evidence overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

mkdir -p "$repo/packages/extension/out"
printf 'generated\n' >"$repo/packages/extension/out/generated.js"
git -C "$repo" add packages/extension/out/generated.js
git -C "$repo" commit -q -m "tracked generated artifact"

if PATH="$fakebin:$PATH" TMPDIR="$TMPDIR/logs" FLEET_DEPENDENCY_REVIEW_FORCE=1 "$repo/scripts/run-dependency-review.sh" "$evidence" >"$output" 2>&1; then
  echo "FAIL: expected dependency review runner to reject tracked generated artifacts" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q 'generated artifacts are tracked' "$output"; then
  echo "FAIL: expected generated artifact failure output" >&2
  cat "$output" >&2
  exit 1
fi

echo "Dependency review runner tests passed."

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
output="$TMPDIR/prepare-public-branch.out"
mkdir -p "$repo/scripts"
cp "$ROOT/scripts/prepare-public-branch.sh" "$repo/scripts/prepare-public-branch.sh"
cp "$ROOT/scripts/history-release-check.sh" "$repo/scripts/history-release-check.sh"
chmod +x "$repo/scripts/prepare-public-branch.sh" "$repo/scripts/history-release-check.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "clean start"

mkdir -p "$repo/artifacts"
fixture_path="/""Users/private/local"
printf '{"path":"%s"}\n' "$fixture_path" >"$repo/artifacts/raw.json"
git -C "$repo" add artifacts/raw.json
git -C "$repo" commit -q -m "add private artifact"

rm "$repo/artifacts/raw.json"
rmdir "$repo/artifacts"
printf 'public tree\n' >"$repo/README.md"
git -C "$repo" add -A
git -C "$repo" commit -q -m "remove private artifact"
source_commit="$(git -C "$repo" rev-parse HEAD)"
source_tree="$(git -C "$repo" rev-parse HEAD^{tree})"

if ! (cd "$repo" && ./scripts/prepare-public-branch.sh public-alpha) >"$output" 2>&1; then
  echo "FAIL: expected public branch preparation to pass" >&2
  cat "$output" >&2
  exit 1
fi

public_commit="$(git -C "$repo" rev-parse public-alpha)"
public_tree="$(git -C "$repo" rev-parse public-alpha^{tree})"

if [ "$public_tree" != "$source_tree" ]; then
  echo "FAIL: public branch tree does not match source tree" >&2
  exit 1
fi

if [ "$(git -C "$repo" rev-list --count public-alpha)" != "1" ]; then
  echo "FAIL: public branch should contain exactly one commit" >&2
  exit 1
fi

if [ "$(git -C "$repo" rev-list --parents -n1 public-alpha | wc -w | tr -d ' ')" != "1" ]; then
  echo "FAIL: public branch root commit should have no parents" >&2
  git -C "$repo" rev-list --parents -n1 public-alpha >&2
  exit 1
fi

if ! git -C "$repo" log -1 --format=%B public-alpha | rg -q "Source snapshot: $source_commit"; then
  echo "FAIL: public branch commit message must record source commit" >&2
  git -C "$repo" log -1 --format=%B public-alpha >&2
  exit 1
fi

if ! rg -q 'FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh' "$output"; then
  echo "FAIL: helper output must show the ref-scoped aggregate release check" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q './scripts/generate-public-branch-evidence.sh public-alpha' "$output"; then
  echo "FAIL: helper output must show public branch evidence generation" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q './scripts/secret-release-check.sh public-alpha' "$output"; then
  echo "FAIL: helper output must show the ref-scoped secret release check" >&2
  cat "$output" >&2
  exit 1
fi

if ! (cd "$repo" && ./scripts/history-release-check.sh missing-owner.md public-alpha) >"$TMPDIR/history-public.out" 2>&1; then
  echo "FAIL: public branch history should be clean" >&2
  cat "$TMPDIR/history-public.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/history-release-check.sh missing-owner.md) >"$TMPDIR/history-all.out" 2>&1; then
  echo "FAIL: full private repo history should still fail" >&2
  cat "$TMPDIR/history-all.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/prepare-public-branch.sh public-alpha) >"$output" 2>&1; then
  echo "FAIL: existing public branch should be rejected" >&2
  cat "$output" >&2
  exit 1
fi

echo "Public branch preparation tests passed."

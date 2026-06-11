#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
output="$TMPDIR/secret-release-check.out"
mkdir -p "$repo/scripts"
cp "$ROOT/scripts/secret-release-check.sh" "$repo/scripts/secret-release-check.sh"
chmod +x "$repo/scripts/secret-release-check.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

cat >"$repo/README.md" <<'EOF'
# Clean Fixture
EOF
git -C "$repo" add .
git -C "$repo" commit -q -m "clean fixture"

expect_pass() {
  local label=$1
  if ! (cd "$repo" && ./scripts/secret-release-check.sh) >"$output" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_fail_redacted() {
  local label=$1
  local secret=$2

  if (cd "$repo" && ./scripts/secret-release-check.sh) >"$output" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$output" >&2
    exit 1
  fi

  if rg -Fq "$secret" "$output"; then
    echo "FAIL: secret release check printed the matched secret for $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_pass "clean tracked tree and history"

aws_key="AKIAABCDEFGHIJKLMNOP"
printf 'AWS_ACCESS_KEY_ID=%s\n' "$aws_key" >"$repo/current.env"
git -C "$repo" add current.env
git -C "$repo" commit -q -m "add current secret"
expect_fail_redacted "current tracked AWS key" "$aws_key"

rm "$repo/current.env"
git -C "$repo" add -u
git -C "$repo" commit -q -m "remove current secret"
expect_fail_redacted "removed AWS key remains in history" "$aws_key"

history_repo="$TMPDIR/history-repo"
mkdir -p "$history_repo/scripts"
cp "$ROOT/scripts/secret-release-check.sh" "$history_repo/scripts/secret-release-check.sh"
chmod +x "$history_repo/scripts/secret-release-check.sh"
git -C "$history_repo" init -q
git -C "$history_repo" config user.email "release-test@example.invalid"
git -C "$history_repo" config user.name "Fleet Release Test"

openai_key="sk-proj-abcdefghijklmnopqrstuvwxyz123456"
cat >"$history_repo/deleted.env" <<EOF
OPENAI_API_KEY=$openai_key
EOF
git -C "$history_repo" add .
git -C "$history_repo" commit -q -m "add deleted secret"
rm "$history_repo/deleted.env"
git -C "$history_repo" add -u
git -C "$history_repo" commit -q -m "remove deleted secret"

repo="$history_repo"
expect_fail_redacted "deleted OpenAI key remains in history" "$openai_key"

echo "Secret release gate tests passed."

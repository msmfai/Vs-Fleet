#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

template="$TMPDIR/OWNER_DECISION_REPLY_TEMPLATE.md"
cp "$ROOT/docs/release/OWNER_DECISION_REPLY_TEMPLATE.md" "$template"

if ! "$ROOT/scripts/check-owner-reply-template.sh" "$template" >"$TMPDIR/pass.out" 2>&1; then
  echo "FAIL: expected owner reply template to pass" >&2
  cat "$TMPDIR/pass.out" >&2
  exit 1
fi

missing_acceptance="$TMPDIR/missing-acceptance.md"
cp "$template" "$missing_acceptance"
perl -0pi -e 's/I accept the recommended source-only alpha defaults/I accept some defaults/' "$missing_acceptance"
if "$ROOT/scripts/check-owner-reply-template.sh" "$missing_acceptance" >"$TMPDIR/fail.out" 2>&1; then
  echo "FAIL: missing acceptance statement should fail" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

missing_namespace="$TMPDIR/missing-namespace.md"
cp "$template" "$missing_namespace"
perl -0pi -e 's/^  Open VSX publisher:.*\n//m' "$missing_namespace"
if "$ROOT/scripts/check-owner-reply-template.sh" "$missing_namespace" >"$TMPDIR/fail2.out" 2>&1; then
  echo "FAIL: missing namespace field should fail" >&2
  cat "$TMPDIR/fail2.out" >&2
  exit 1
fi

todo_template="$TMPDIR/todo.md"
cp "$template" "$todo_template"
printf '\nTODO: later\n' >>"$todo_template"
if "$ROOT/scripts/check-owner-reply-template.sh" "$todo_template" >"$TMPDIR/fail3.out" 2>&1; then
  echo "FAIL: TODO placeholders should fail" >&2
  cat "$TMPDIR/fail3.out" >&2
  exit 1
fi

echo "Owner decision reply template tests passed."

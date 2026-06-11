#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/secret-release-check.sh [scan-ref]

Scan tracked release content and git history for credential-looking material.
If scan-ref is omitted, all refs are scanned. Pass a prepared public branch
name to validate only the history intended for public release.
EOF
}

scan_ref="${1:---all}"

if [ "$scan_ref" = "-h" ] || [ "$scan_ref" = "--help" ]; then
  usage
  exit 2
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: secret release check must run inside a git worktree"
  exit 2
fi

cd "$(git rev-parse --show-toplevel)"

if [ "$scan_ref" != "--all" ]; then
  git rev-parse --verify "$scan_ref^{commit}" >/dev/null
fi

hits="$tmpdir/hits"
revs="$tmpdir/revs"
touch "$hits"

pathspec_excludes=(
  ':(exclude)scripts/secret-release-check.sh'
  ':(exclude)scripts/test-secret-release-check.sh'
)

record_current_hits() {
  local name=$1
  local pattern=$2

  while IFS=: read -r path line _content; do
    if [ -n "${path:-}" ] && [ -n "${line:-}" ]; then
      printf 'current:%s:%s:%s\n' "$path" "$line" "$name" >>"$hits"
    fi
  done < <(git grep -I -n -E "$pattern" -- . "${pathspec_excludes[@]}" 2>/dev/null || true)
}

record_history_hits() {
  local name=$1
  local pattern=$2

  while IFS= read -r rev; do
    while IFS=: read -r hit_rev path line _content; do
      if [ -n "${hit_rev:-}" ] && [ -n "${path:-}" ] && [ -n "${line:-}" ]; then
        printf 'history:%s:%s:%s:%s\n' "${hit_rev:0:12}" "$path" "$line" "$name" >>"$hits"
      fi
    done < <(git grep -I -n -E "$pattern" "$rev" -- . "${pathspec_excludes[@]}" 2>/dev/null || true)
  done <"$revs"
}

scan_pattern() {
  local name=$1
  local pattern=$2
  record_current_hits "$name" "$pattern"
  record_history_hits "$name" "$pattern"
}

if [ "$scan_ref" = "--all" ]; then
  git rev-list --all >"$revs"
else
  git rev-list "$scan_ref" >"$revs"
fi

scan_pattern "private-key" '-----BEGIN[[:space:]]+([A-Z0-9]+[[:space:]]+)?PRIVATE[[:space:]]+KEY-----'
scan_pattern "aws-access-key" '(^|[^A-Z0-9])(AKIA|ASIA)[0-9A-Z]{16}([^A-Z0-9]|$)'
scan_pattern "github-token" '(^|[^A-Za-z0-9_-])gh[pousr]_[A-Za-z0-9_]{20,}'
scan_pattern "openai-token" '(^|[^A-Za-z0-9_-])sk-(proj-)?[A-Za-z0-9_-]{20,}'
scan_pattern "slack-token" '(^|[^A-Za-z0-9_-])xox[baprs]-[A-Za-z0-9-]{20,}'
scan_pattern "stripe-live-secret" '(^|[^A-Za-z0-9_-])sk_live_[A-Za-z0-9]{20,}'
scan_pattern "npm-token" '(^|[^A-Za-z0-9_-])npm_[A-Za-z0-9]{20,}'

if [ -s "$hits" ]; then
  echo "FAIL: tracked tree or git history contains credential-looking material"
  echo "Redacted findings follow as scope:commit-or-current:path:line:pattern."
  sort -u "$hits" | sed -n '1,80p'
  echo
  echo "Remove the secret from the tracked tree and rewrite/squash public history before publishing."
  exit 1
fi

echo "Secret release check passed."

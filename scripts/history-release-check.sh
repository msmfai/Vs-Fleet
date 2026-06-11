#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
fail=0
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

history_accepted=0
if [ -f "$owner_record" ] &&
  rg -q '^Decision record status: APPROVED$' "$owner_record" &&
  rg -q '^- \[x\] Publish the current branch history and accept that old commits may contain' "$owner_record"; then
  history_accepted=1
fi

local_path_pattern='/Users/[^[:space:]"'"'"'<>]+|/private/tmp/|/private/var/folders/[[:alnum:]]{2}/|/var/folders/[[:alnum:]]{2}/|C:\\Users\\[^[:space:]"/]+'

git rev-list --all >"$tmpdir/revs"

content_hits="$tmpdir/content-hits"
while IFS= read -r rev; do
  git grep -I -n -E "$local_path_pattern" "$rev" -- . >>"$content_hits" 2>/dev/null || true
done <"$tmpdir/revs"

if [ -s "$content_hits" ]; then
  rg -v '/Users/(dev|example)([^[:alnum:]_]|$)|local_path_pattern=|/Users/\$\{USER\}' \
    "$content_hits" >"$tmpdir/content-real-hits" || true
  mv "$tmpdir/content-real-hits" "$content_hits"
fi

if [ -s "$content_hits" ]; then
  echo "FAIL: git history contains local absolute paths"
  sed -n '1,40p' "$content_hits"
  fail=1
fi

object_hits="$tmpdir/object-hits"
git rev-list --objects --all |
  rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/|(^|/)fleet-host\.log$|(^|/)host-keepalive-[^/]*\.json$|(^|/)artifacts/.+\.(png|jpg|jpeg|webp|gif|json|log|txt)$' \
    >"$object_hits" || true

if [ -s "$object_hits" ]; then
  echo "FAIL: git history contains generated outputs, local logs, or raw artifacts"
  sed -n '1,40p' "$object_hits"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  if [ "$history_accepted" -eq 1 ]; then
    echo
    echo "History findings are explicitly accepted by the approved owner decision record."
    echo "History release check passed with accepted findings."
    exit 0
  fi

  echo
  echo "History release check failed."
  echo "Clean/squash the first public branch, or approve the owner record choice that accepts current history exposure."
  exit 1
fi

echo "History release check passed."

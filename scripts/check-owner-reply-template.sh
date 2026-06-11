#!/usr/bin/env bash
set -euo pipefail

template="${1:-docs/release/OWNER_DECISION_REPLY_TEMPLATE.md}"

if [ ! -f "$template" ]; then
  echo "FAIL: missing owner decision reply template: $template"
  exit 1
fi

require_text() {
  local pattern=$1
  local description=$2
  if ! rg -qi "$pattern" "$template"; then
    echo "FAIL: $template must contain $description"
    exit 1
  fi
}

require_text '^# Owner Decision Reply Template$' "title"
require_text 'not an approval record by itself' "non-approval warning"
require_text 'OWNER_DECISION_RECORD\.md' "owner decision record handoff"
require_text 'I accept the recommended source-only alpha defaults' "compact acceptance statement"
require_text 'cleaned first public history' "clean history default"
require_text 'local macOS-only scope' "local macOS alpha scope"
require_text 'user-provided VS Code' "editor server boundary"
require_text 'source-only distribution' "source-only distribution default"
require_text 'DCO/no CLA for alpha' "contribution posture"
require_text 'best-effort support' "support boundary"
require_text 'no stable compatibility promise' "compatibility boundary"
require_text 'provisional Fleet name/no trademark claim' "name collision posture"
require_text 'no telemetry by default' "privacy posture"
require_text 'read-only/no-secret workflows' "workflow supply-chain posture"

for field in \
  'GitHub org/user:' \
  'GitHub repo name:' \
  'Product name:' \
  'Rust crate prefix:' \
  'npm package names:' \
  'VS Code Marketplace publisher:' \
  'Open VSX publisher:' \
  'macOS bundle id:' \
  'Security reporting:' \
  'Emergency removal owner for publication evidence:' \
  'CI evidence:'
do
  require_text "$field" "reply field $field"
done

require_text 'Namespace answers must be concrete' "concrete namespace warning"
require_text 'Source-only alpha still defers crates\.io, npm, VS Code Marketplace' \
  "package publication deferral warning"
require_text './scripts/draft-owner-decisions\.sh <github-owner> <github-repo>' \
  "owner draft command"
require_text 'keep the status `PENDING` until the evidence files are concrete' \
  "pending-until-evidence warning"

if rg -q 'TODO|TBD' "$template"; then
  echo "FAIL: $template must not use TODO/TBD placeholders; use angle-bracket placeholders"
  exit 1
fi

echo "Owner decision reply template check passed."

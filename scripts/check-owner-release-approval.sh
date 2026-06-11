#!/usr/bin/env bash
set -euo pipefail

sheet="${1:-docs/release/OWNER_RELEASE_APPROVAL.md}"

if [ ! -f "$sheet" ]; then
  echo "FAIL: missing owner release approval sheet: $sheet"
  exit 1
fi

require_text() {
  local pattern=$1
  local description=$2
  if ! rg -qi "$pattern" "$sheet"; then
    echo "FAIL: $sheet must contain $description"
    exit 1
  fi
}

require_text '^Approval status:[[:space:]]*PENDING$' "pending approval status"
require_text '^## Honest Readiness Judgment$' "honest readiness judgment section"
require_text 'too rough for a broad open-source launch' "explicit roughness warning"
require_text 'narrow source-only alpha' "source-only alpha qualification"
require_text 'Clean public history' "clean public history constraint"
require_text 'Provisional name' "provisional name constraint"
require_text 'Best-effort support only' "best-effort support constraint"
require_text 'Local privacy boundary' "local privacy constraint"

for decision in \
  'License' \
  'Public history' \
  'Namespace' \
  'Alpha scope' \
  'Editor server boundary' \
  'Distribution' \
  'Security reporting' \
  'Contributions' \
  'CI checks' \
  'Privacy' \
  'Dependency review' \
  'Support' \
  'Branding' \
  'Versioning' \
  'Community intake' \
  'Release custody' \
  'AI contributions' \
  'Platform' \
  'Roadmap' \
  'Name collision' \
  'Local data' \
  'Workflow supply chain'
do
  require_text "^[|][[:space:]]*${decision}[[:space:]]*[|]" "recommended answer for $decision"
done

require_text 'OWNER_DECISION_RECORD\.md.*APPROVED' "owner decision approval requirement"
require_text 'check-public-release-branch\.sh <public-branch> <source-ref-sha>' \
  "public branch verifier requirement"
require_text 'GitHub CI and Release Readiness pass' "public CI requirement"
require_text 'GitHub repository settings match' "GitHub publication settings requirement"
require_text 'Dependency review has been run' "dependency review requirement"
require_text './scripts/check-public-release-branch\.sh <public-branch> <source-ref-sha>' \
  "public release branch verifier command"
if rg -q 'FLEET_RELEASE_HISTORY_REF' "$sheet"; then
  echo "FAIL: $sheet must use check-public-release-branch.sh for cleaned-history approval evidence, not FLEET_RELEASE_HISTORY_REF"
  exit 1
fi
require_text './scripts/draft-owner-decisions\.sh <github-owner> <github-repo>' \
  "owner decision draft command"
require_text 'OWNER_DECISION_REPLY_TEMPLATE\.md' "owner decision reply template reference"
require_text 'namespace, security, emergency-removal, and CI check values' \
  "owner reply template value scope"

echo "Owner release approval sheet check passed."

#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -d "$root" ]; then
  echo "FAIL: missing repository root: $root"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

workflow_block="$(
  sed -n '/^### 22\. GitHub Actions Supply-Chain Posture$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$workflow_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: GitHub Actions supply-chain posture decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$workflow_block" | rg '^- \[x\] ' | head -n1)"

require_file() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file"
    exit 1
  fi
}

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  require_file "$file"
  if ! rg -qi "$pattern" "$root/$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

reject_text() {
  local file=$1
  local pattern=$2
  local description=$3
  require_file "$file"
  if rg -ni "$pattern" "$root/$file"; then
    echo "FAIL: $file must not contain $description"
    exit 1
  fi
}

reject_placeholder_file() {
  local file=$1
  require_file "$file"
  if rg -ni 'TODO|TBD|PLACEHOLDER|pending owner decision' "$root/$file"; then
    echo "FAIL: $file still contains placeholder workflow supply-chain text"
    exit 1
  fi
}

workflow_files=(
  ".github/workflows/ci.yml"
  ".github/workflows/release-readiness.yml"
)

check_read_only_workflows() {
  local workflow
  for workflow in "${workflow_files[@]}"; do
    require_text "$workflow" '^permissions:$' "top-level permissions block"
    require_text "$workflow" '^[[:space:]]+contents:[[:space:]]*read$' \
      "read-only contents permission"
    reject_text "$workflow" 'secrets\.|secrets\[' "repository secret references"
    reject_text "$workflow" 'contents:[[:space:]]*write|packages:[[:space:]]*write|id-token:[[:space:]]*write|actions:[[:space:]]*write' \
      "write-capable workflow permissions"
    reject_text "$workflow" 'cargo publish|npm publish|pnpm publish|vsce publish|ovsx publish|docker push|gh release|git push|create-release|upload-release-asset' \
      "package, release, container, or tag publishing commands"
  done
}

check_policy_docs() {
  reject_placeholder_file "docs/release/WORKFLOW_SUPPLY_CHAIN.md"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" '^# GitHub Actions Supply-Chain Posture$' \
    "workflow supply-chain title"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'Tagged third-party GitHub Actions are accepted for source alpha' \
    "tagged Action acceptance"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" '`?GITHUB_TOKEN`? permissions are read-only: `?contents: read`?' \
    "read-only GITHUB_TOKEN policy"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'must not reference repository secrets' \
    "no workflow secrets policy"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'must not publish packages, create releases, upload release assets' \
    "no release asset publishing policy"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'or push tags' \
    "no tag publishing policy"
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'full commit SHA' \
    "future full-SHA pinning note"
  require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'GitHub Actions supply-chain posture' \
    "public decision table workflow supply-chain row"
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" 'approved supply-chain posture: read-only' \
    "publication runbook supply-chain posture checkpoint"
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" '`?GITHUB_TOKEN`? permissions, no repository secrets' \
    "publication runbook read-only token checkpoint"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" '^## Workflow Supply Chain$' \
    "release-notes Workflow Supply Chain section"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'read-only `?GITHUB_TOKEN`? permissions' \
    "release-notes read-only token policy"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'do not use repository secrets or publishing' \
    "release-notes no secrets"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'credentials' \
    "release-notes no publishing credentials"
}

check_full_sha_pinning() {
  check_read_only_workflows
  local workflow
  local bad_uses
  for workflow in "${workflow_files[@]}"; do
    bad_uses="$(
      rg -n 'uses:[[:space:]]*[^[:space:]]+@' "$root/$workflow" |
        rg -v 'uses:[[:space:]]*[^[:space:]]+@[0-9a-f]{40}([[:space:]]|$)' || true
    )"
    if [ -n "$bad_uses" ]; then
      echo "FAIL: $workflow contains third-party Actions not pinned by full commit SHA"
      printf '%s\n' "$bad_uses"
      exit 1
    fi
  done
  require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" 'Full SHA pinning: required' \
    "full SHA pinning requirement"
}

case "$checked" in
  "- [x] Tagged third-party GitHub Actions are accepted for source alpha,"*)
    check_read_only_workflows
    check_policy_docs
    ;;
  "- [x] Require every third-party GitHub Action to be pinned by full commit SHA"*)
    check_full_sha_pinning
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other workflow supply-chain decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/WORKFLOW_SUPPLY_CHAIN.md"
    require_text "docs/release/WORKFLOW_SUPPLY_CHAIN.md" '^Owner decision:' \
      "a concrete owner decision line"
    ;;
  *)
    echo "FAIL: unsupported GitHub Actions supply-chain posture decision: $checked"
    exit 1
    ;;
esac

echo "Workflow supply-chain decision check passed."

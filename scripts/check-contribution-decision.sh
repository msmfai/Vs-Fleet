#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
contributing_file="${2:-CONTRIBUTING.md}"
pr_template="${3:-.github/PULL_REQUEST_TEMPLATE.md}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$contributing_file" ]; then
  echo "FAIL: missing contribution guide: $contributing_file"
  exit 1
fi

if [ ! -f "$pr_template" ]; then
  echo "FAIL: missing pull request template: $pr_template"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

contribution_block="$(
  sed -n '/^### 7\. Contribution Intake$/,/^### 8\. Public CI Evidence$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$contribution_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: contribution intake decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$contribution_block" | rg '^- \[x\] ' | head -n1)"

reject_provisional_contribution_text() {
  local provisional='not ready for broad external contributions|should wait unless|may be deferred|deferred or closed|must be finalized before accepting|Recommended policy once|until the project license|until the project license and contribution policy'
  local failed=0
  for file in "$contributing_file" "$pr_template"; do
    if rg -ni "$provisional" "$file"; then
      failed=1
    fi
  done
  if [ "$failed" -ne 0 ]; then
    echo "FAIL: contribution docs still contain provisional pre-decision language"
    exit 1
  fi
}

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  if ! rg -qi "$pattern" "$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

case "$checked" in
  "- [x] Accept small focused PRs under the chosen project license using the PR"*)
    reject_provisional_contribution_text
    require_text "$contributing_file" 'contributions are licensed under the same license as the project' \
      "same-license contribution policy"
    require_text "$pr_template" 'certify|certification' \
      "a contributor certification checkbox"
    require_text "$pr_template" 'project license|same license as the project' \
      "project-license contribution certification"
    ;;
  "- [x] Require DCO sign-off.")
    reject_provisional_contribution_text
    require_text "$contributing_file" 'Developer Certificate of Origin|DCO' \
      "DCO policy"
    require_text "$contributing_file" 'Signed-off-by' \
      "Signed-off-by instructions"
    require_text "$pr_template" 'Developer Certificate of Origin|DCO' \
      "DCO certification"
    require_text "$pr_template" 'Signed-off-by' \
      "Signed-off-by certification"
    ;;
  "- [x] Keep code PRs closed; accept issues and docs feedback only.")
    reject_provisional_contribution_text
    require_text "$contributing_file" 'code (pull requests|PRs) (are )?closed|code contributions are closed|not accepting code (pull requests|PRs)' \
      "explicit code-PR-closed policy"
    require_text "$pr_template" 'code (pull requests|PRs) (are )?closed|code contributions are not accepted|docs feedback only' \
      "code-PR-closed notice"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other contribution decision must contain a concrete value"
      exit 1
    fi
    reject_provisional_contribution_text
    require_text "$contributing_file" '^Contribution intake policy:' \
      "a concrete 'Contribution intake policy:' line"
    require_text "$pr_template" '^Contribution policy:' \
      "a concrete 'Contribution policy:' line"
    ;;
  *)
    echo "FAIL: unsupported contribution intake decision: $checked"
    exit 1
    ;;
esac

echo "Contribution decision check passed."

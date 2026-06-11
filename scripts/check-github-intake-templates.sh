#!/usr/bin/env bash
set -euo pipefail

bug="${1:-.github/ISSUE_TEMPLATE/bug_report.yml}"
feedback="${2:-.github/ISSUE_TEMPLATE/alpha_feedback.yml}"
config="${3:-.github/ISSUE_TEMPLATE/config.yml}"
pr="${4:-.github/PULL_REQUEST_TEMPLATE.md}"

require_file() {
  local file=$1
  if [ ! -f "$file" ]; then
    echo "FAIL: missing GitHub intake template: $file"
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

require_file "$bug"
require_file "$feedback"
require_file "$config"
require_file "$pr"

require_text "$config" '^blank_issues_enabled:[[:space:]]*false$' \
  "blank issues disabled for alpha triage"

require_text "$bug" 'labels:[[:space:]]*\["bug",[[:space:]]*"alpha"\]' \
  "bug and alpha labels"
require_text "$bug" 'scrub workspace paths, local URLs, logs, screenshots, and command lines' \
  "privacy scrub warning"
require_text "$bug" 'Do not report vulnerabilities or exploit details in public issues; use SECURITY\.md' \
  "public vulnerability-reporting warning"
require_text "$bug" 'local macOS Fleet host, local code serve-web sessions, reporter, bridge, or CLI' \
  "supported local alpha scope checkbox"
require_text "$bug" 'remote/container deployment and binary distribution are experimental' \
  "unsupported remote/binary scope warning"
require_text "$bug" 'id:[[:space:]]*version' "commit/build field"
require_text "$bug" 'id:[[:space:]]*steps' "reproduction steps field"
require_text "$bug" 'id:[[:space:]]*environment' "environment field"
require_text "$bug" 'required:[[:space:]]*true' "required fields"

require_text "$feedback" 'labels:[[:space:]]*\["alpha-feedback"\]' \
  "alpha-feedback label"
require_text "$feedback" 'Scrub private local details before posting' \
  "privacy scrub warning"
require_text "$feedback" 'Do not report vulnerabilities or exploit details in public issues; use SECURITY\.md' \
  "public vulnerability-reporting warning"
require_text "$feedback" 'Security/privacy expectations' \
  "security/privacy feedback topic"
require_text "$feedback" 'Release packaging' "release packaging feedback topic"
require_text "$feedback" 'id:[[:space:]]*context' "feedback context field"
require_text "$feedback" 'id:[[:space:]]*feedback' "feedback body field"

require_text "$pr" 'focused alpha contributions under the project license with DCO' \
  "approved alpha contribution boundary"
require_text "$pr" 'Broad or speculative code changes are triaged narrowly during alpha' \
  "broad-change alpha triage boundary"
require_text "$pr" 'did not add generated build output, raw logs, private screenshots, or machine-specific paths' \
  "artifact/privacy contribution checklist"
require_text "$pr" 'Test Evidence' "test evidence section"
require_text "$pr" 'Contribution Licensing' "contribution licensing section"
require_text "$pr" 'licensing and DCO requirements' "contribution policy acknowledgement"

echo "GitHub intake template check passed."

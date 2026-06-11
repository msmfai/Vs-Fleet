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

privacy_block="$(
  sed -n '/^### 9\. Privacy And Telemetry Posture$/,/^### 10\. Dependency Review Evidence$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$privacy_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: privacy/telemetry decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$privacy_block" | rg '^- \[x\] ' | head -n1)"

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

reject_placeholder_file() {
  local file=$1
  require_file "$file"
  if rg -ni 'TODO|TBD|PLACEHOLDER' "$root/$file"; then
    echo "FAIL: $file still contains placeholder privacy text"
    exit 1
  fi
}

check_no_telemetry_default() {
  require_text "README.md" 'no intended telemetry by default' "no-telemetry-by-default statement"
  require_text "README.md" 'workspace paths' "workspace-path logging disclosure"
  require_text "README.md" 'local URLs' "local URL logging disclosure"
  require_text "README.md" 'session labels' "session label logging disclosure"
  require_text "README.md" 'process command lines' "process command-line logging disclosure"
  require_text "README.md" 'editor state' "editor-state logging disclosure"
  require_text "README.md" 'Scrub logs and review artifacts before sharing' "scrub-before-sharing warning"

  require_text "SECURITY.md" 'workspace paths' "workspace-path security disclosure"
  require_text "SECURITY.md" 'local URLs' "local URL security disclosure"
  require_text "SECURITY.md" 'session labels' "session label security disclosure"
  require_text "SECURITY.md" 'command-line metadata' "command-line metadata security disclosure"
  require_text "SECURITY.md" 'scrubbed before sharing publicly' "public-sharing scrub warning"

  require_text "docs/ARCHITECTURE.md" 'no intended telemetry by default' \
    "architecture no-telemetry statement"
  require_text ".github/ISSUE_TEMPLATE/bug_report.yml" 'scrub workspace paths, local URLs, logs, screenshots, and command lines' \
    "bug-report privacy warning"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'no intended telemetry by default' \
    "release-notes no-telemetry statement"
}

case "$checked" in
  "- [x] No telemetry by default. Local logs and artifacts may contain workspace"*)
    check_no_telemetry_default
    ;;
  "- [x] Add an explicit telemetry or remote reporting disclosure before public"*)
    reject_placeholder_file "PRIVACY.md"
    require_text "PRIVACY.md" '^Telemetry:' "a concrete Telemetry line"
    require_text "PRIVACY.md" '^Remote reporting:' "a concrete Remote reporting line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other privacy decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/PRIVACY_POSTURE.md"
    require_text "docs/release/PRIVACY_POSTURE.md" '^Privacy posture:' \
      "a concrete 'Privacy posture:' line"
    ;;
  *)
    echo "FAIL: unsupported privacy/telemetry decision: $checked"
    exit 1
    ;;
esac

echo "Privacy decision check passed."

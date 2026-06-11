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

local_data_block="$(
  sed -n '/^### 21\. Local Data And Uninstall Policy$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$local_data_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: local data and uninstall policy decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$local_data_block" | rg '^- \[x\] ' | head -n1)"

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
  if rg -ni 'TODO|TBD|PLACEHOLDER|pending owner decision' "$root/$file"; then
    echo "FAIL: $file still contains placeholder local-data text"
    exit 1
  fi
}

check_manual_cleanup_docs() {
  reject_placeholder_file "docs/LOCAL_DATA_AND_UNINSTALL.md"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" '^# Local Data And Uninstall$' \
    "local data and uninstall title"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" '`?~/.fleet/run`?' \
    "~/.fleet/run runtime path"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" '`?~/.fleet/mux`?' \
    "~/.fleet/mux runtime path"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'FLEET_RUNTIME_DIR' \
    "FLEET_RUNTIME_DIR override"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'FLEET_MUX_DIR' \
    "FLEET_MUX_DIR override"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'rm -rf ~/.fleet/run ~/.fleet/mux' \
    "manual cleanup command"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'Quitting Fleet must not kill external servers' \
    "external-session ownership boundary"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'Closing a Fleet-spawned server from the Fleet UI is the explicit action' \
    "Fleet-spawned close action"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'does not' \
    "cleanup non-goal statement"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'remove the user'"'"'s VS Code installation or repositories outside the Fleet runtime' \
    "cleanup non-goal scope"

  require_text "README.md" 'docs/LOCAL_DATA_AND_UNINSTALL\.md' \
    "local data and uninstall link"
  require_text "README.md" '`?~/.fleet/run`?.*`?~/.fleet/mux`?' \
    "README local runtime paths"
  require_text "docs/QUICKSTART.md" '^## Cleanup$' \
    "Quickstart Cleanup section"
  require_text "docs/QUICKSTART.md" 'rm -rf ~/.fleet/run ~/.fleet/mux' \
    "Quickstart manual cleanup command"
  require_text "docs/QUICKSTART.md" 'Close any Fleet-spawned servers from the Fleet UI' \
    "Quickstart close-spawned-server warning"
  require_text "docs/QUICKSTART.md" 'FLEET_RUNTIME_DIR.*FLEET_MUX_DIR|FLEET_MUX_DIR.*FLEET_RUNTIME_DIR' \
    "Quickstart cleanup override warning"
  require_text "docs/ARCHITECTURE.md" '^## Local Data And Cleanup$' \
    "Architecture Local Data And Cleanup section"
  require_text "docs/ARCHITECTURE.md" 'Quitting Fleet does not promise to delete spawned editor userdata or logs' \
    "Architecture cleanup boundary"
  require_text "docs/ARCHITECTURE.md" 'must not kill externally registered sessions' \
    "Architecture external-session ownership boundary"
  require_text "SECURITY.md" 'Source-alpha runtime files live under `?~/.fleet/run`? and `?~/.fleet/mux`?' \
    "Security local runtime paths"
  require_text "SECURITY.md" 'docs/LOCAL_DATA_AND_UNINSTALL\.md' \
    "Security cleanup reference"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" '^## Local Data And Cleanup$' \
    "release-notes Local Data And Cleanup section"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'rm -rf ~/.fleet/run ~/.fleet/mux' \
    "release-notes cleanup command"
  require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'Local data and uninstall policy' \
    "public decision table local-data row"
}

check_automated_cleanup() {
  require_file "docs/LOCAL_DATA_AND_UNINSTALL.md"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" '^Automated cleanup command:' \
    "a concrete automated cleanup command line"
  require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" 'rm -rf ~/.fleet/run ~/.fleet/mux' \
    "manual fallback cleanup command"
  if ! rg -q 'cleanup|uninstall' "$root/scripts" "$root/crates" 2>/dev/null; then
    echo "FAIL: automated cleanup decision requires a cleanup/uninstall implementation in scripts or crates"
    exit 1
  fi
}

case "$checked" in
  "- [x] Document local data locations and manual cleanup for source alpha."*)
    check_manual_cleanup_docs
    ;;
  "- [x] Add an automated cleanup or uninstall command before public visibility.")
    check_automated_cleanup
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other local-data decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/LOCAL_DATA_AND_UNINSTALL.md"
    require_text "docs/LOCAL_DATA_AND_UNINSTALL.md" '^Owner decision:' \
      "a concrete owner decision line"
    ;;
  *)
    echo "FAIL: unsupported local data and uninstall policy decision: $checked"
    exit 1
    ;;
esac

echo "Local data decision check passed."

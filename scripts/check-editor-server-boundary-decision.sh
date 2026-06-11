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

boundary_block="$(
  sed -n '/^### 5\. Editor Server Licensing Boundary$/,/^### 6\. Distribution Scope$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$boundary_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: editor server boundary decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$boundary_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder editor-server boundary text"
    exit 1
  fi
}

check_user_provided_vscode_only() {
  require_text "README.md" "user'?s local.*code serve-web|local.*code serve-web.*install" \
    "user-provided local code serve-web boundary"
  require_text "README.md" 'download, bundle' "no editor-server bundling boundary"
  require_text "README.md" 'redistribute' "no editor-server redistribution boundary"
  require_text "README.md" 'VS Code Server' "VS Code Server boundary"
  require_text "README.md" 'Microsoft Marketplace|Marketplace extensions' \
    "Marketplace non-redistribution boundary"

  require_text "docs/QUICKSTART.md" "user'?s local.*code serve-web" \
    "user-provided local code serve-web quickstart boundary"
  require_text "docs/QUICKSTART.md" 'download, bundle' "quickstart no editor-server bundling boundary"
  require_text "docs/QUICKSTART.md" 'redistribute' "quickstart no editor-server redistribution boundary"
  require_text "docs/QUICKSTART.md" 'VS Code' "quickstart VS Code Server boundary"
  require_text "docs/QUICKSTART.md" 'Microsoft Marketplace|Marketplace extensions' \
    "quickstart Marketplace non-redistribution boundary"

  require_text "docs/ARCHITECTURE.md" 'user-provided VS Code|user.*local.*code serve-web' \
    "architecture user-provided VS Code boundary"
  require_text "docs/ARCHITECTURE.md" 'download, bundle' "architecture no editor-server bundling boundary"
  require_text "docs/ARCHITECTURE.md" 'redistribute' "architecture no editor-server redistribution boundary"
  require_text "docs/ARCHITECTURE.md" 'VS Code Server' "architecture VS Code Server boundary"
  require_text "docs/ARCHITECTURE.md" 'Microsoft Marketplace|Marketplace extensions' \
    "architecture Marketplace non-redistribution boundary"

  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" "user'?s local.*code serve-web" \
    "release-notes user-provided code serve-web boundary"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'redistribute' \
    "release-notes no editor-server redistribution boundary"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'VS Code Server' \
    "release-notes VS Code Server boundary"
}

case "$checked" in
  "- [x] User-provided VS Code only. Fleet may launch the user's local"*)
    check_user_provided_vscode_only
    ;;
  "- [x] OSS server only. Supported workflows use \`code-server\` or"*)
    reject_placeholder_file "docs/release/EDITOR_SERVER_BOUNDARY.md"
    require_text "docs/release/EDITOR_SERVER_BOUNDARY.md" '^Editor server boundary:' \
      "a concrete 'Editor server boundary:' line"
    require_text "docs/release/EDITOR_SERVER_BOUNDARY.md" 'code-server|openvscode-server' \
      "OSS editor server choice"
    require_text "docs/release/EDITOR_SERVER_BOUNDARY.md" 'Open VSX' \
      "Open VSX marketplace boundary"
    require_text "docs/release/EDITOR_SERVER_BOUNDARY.md" 'no Microsoft VS Code Server|not Microsoft' \
      "Microsoft server exclusion"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other editor server boundary decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/EDITOR_SERVER_BOUNDARY.md"
    require_text "docs/release/EDITOR_SERVER_BOUNDARY.md" '^Editor server boundary:' \
      "a concrete 'Editor server boundary:' line"
    ;;
  *)
    echo "FAIL: unsupported editor server boundary decision: $checked"
    exit 1
    ;;
esac

echo "Editor server boundary decision check passed."

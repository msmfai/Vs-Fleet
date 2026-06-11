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

platform_block="$(
  sed -n '/^### 18\. Supported Platform And Toolchain$/,/^### 19\. Public Roadmap And Non-Goals$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$platform_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: supported platform and toolchain decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$platform_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder platform support text"
    exit 1
  fi
}

check_local_macos_toolchain() {
  require_text "docs/QUICKSTART.md" 'macOS' "macOS prerequisite"
  require_text "docs/QUICKSTART.md" 'Rust 1\.78 or newer' "Rust 1.78+ prerequisite"
  require_text "docs/QUICKSTART.md" 'Node\.js 20 and npm' "Node.js 20/npm prerequisite"
  require_text "docs/QUICKSTART.md" 'Visual Studio Code with the `?code`? CLI available' \
    "VS Code code CLI prerequisite"
  require_text "docs/QUICKSTART.md" 'Git' "Git prerequisite"
  require_text "README.md" 'macOS Tauri Fleet host' "README macOS host scope"
  require_text "README.md" 'local `?code serve-web`? sessions' "README local code serve-web scope"
  require_text "SUPPORT.md" 'Source builds and local macOS dogfooding' \
    "support local macOS source-build boundary"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" '^## Supported Platform And Toolchain$' \
    "release-notes platform/toolchain section"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'macOS source build only' \
    "release-notes macOS source-build support"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Rust 1\.78 or newer' \
    "release-notes Rust 1.78+ support"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Node\.js 20 and npm' \
    "release-notes Node.js 20/npm support"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'user-provided VS Code `?code`? CLI' \
    "release-notes user-provided VS Code code CLI support"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Linux, Windows, and remote/container workflows are not supported alpha platforms' \
    "release-notes unsupported platform boundary"
}

case "$checked" in
  "- [x] macOS source alpha only. Supported toolchain: Rust 1.78 or newer,"*)
    check_local_macos_toolchain
    ;;
  "- [x] Publish a broader OS/toolchain support matrix before public alpha.")
    reject_placeholder_file "docs/release/PLATFORM_SUPPORT.md"
    require_text "docs/release/PLATFORM_SUPPORT.md" '^Supported platforms:' \
      "a concrete Supported platforms line"
    require_text "docs/release/PLATFORM_SUPPORT.md" '^Supported toolchains:' \
      "a concrete Supported toolchains line"
    require_text "docs/release/PLATFORM_SUPPORT.md" '^Unsupported platforms:' \
      "a concrete Unsupported platforms line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other platform support decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/PLATFORM_SUPPORT.md"
    require_text "docs/release/PLATFORM_SUPPORT.md" '^Supported platforms:' \
      "a concrete Supported platforms line"
    ;;
  *)
    echo "FAIL: unsupported platform support decision: $checked"
    exit 1
    ;;
esac

echo "Platform support decision check passed."

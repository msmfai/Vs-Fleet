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

distribution_block="$(
  sed -n '/^### 4\. Distribution Scope$/,/^### 5\. Security Reporting Channel$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$distribution_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: distribution scope decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$distribution_block" | rg '^- \[x\] ' | head -n1)"

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

check_cargo_publish_false() {
  local file=$1
  require_file "$file"
  if ! rg -q '^publish[[:space:]]*=[[:space:]]*false$' "$root/$file"; then
    echo "FAIL: $file must keep publish = false for source-only alpha"
    exit 1
  fi
}

check_npm_private_true() {
  local file=$1
  require_file "$file"
  if ! rg -q '"private"[[:space:]]*:[[:space:]]*true' "$root/$file"; then
    echo "FAIL: $file must keep \"private\": true for source-only alpha"
    exit 1
  fi
}

check_no_generated_release_outputs() {
  local hits=""
  if [ -d "$root/.git" ]; then
    hits="$(
      cd "$root"
      git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/' || true
    )"
  else
    hits="$(
      cd "$root"
      find . -type f | sed 's#^\./##' | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/' || true
    )"
  fi
  if [ -n "$hits" ]; then
    echo "FAIL: generated dependency/build outputs are present in release scope"
    printf '%s\n' "$hits" | sed -n '1,40p'
    exit 1
  fi
}

section_block() {
  local section=$1
  local start
  start="$(rg -n "^${section}$" "$owner_record" | cut -d: -f1 | head -n1 || true)"
  if [ -z "$start" ]; then
    return 1
  fi

  local next
  next="$(
    tail -n +"$((start + 1))" "$owner_record" |
      rg -n '^### ' |
      cut -d: -f1 |
      head -n1 || true
  )"
  if [ -n "$next" ]; then
    sed -n "${start},$((start + next - 1))p" "$owner_record"
  else
    sed -n "${start},\$p" "$owner_record"
  fi
}

require_binary_sections_decided() {
  for section in \
    "### 9. macOS Signing and Notarization" \
    "### 10. Update Channel" \
    "### 11. Branding Stability"
  do
    local block
    if ! block="$(section_block "$section")"; then
      echo "FAIL: owner decision record missing binary distribution section: $section"
      exit 1
    fi
    local count
    count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
    count="${count:-0}"
    if [ "$count" -ne 1 ]; then
      echo "FAIL: $section must have exactly one checked choice for public app-bundle distribution; found $count"
      exit 1
    fi
  done
}

check_source_only() {
  check_cargo_publish_false "crates/fleet-cli/Cargo.toml"
  check_cargo_publish_false "crates/fleet-e2e/Cargo.toml"
  check_cargo_publish_false "crates/fleet-host-core/Cargo.toml"
  check_cargo_publish_false "crates/fleet-host/Cargo.toml"
  check_cargo_publish_false "crates/fleet-hub/Cargo.toml"
  check_cargo_publish_false "crates/fleet-protocol/Cargo.toml"
  check_cargo_publish_false "crates/fleet-reporter/Cargo.toml"
  check_npm_private_true "packages/fleet-bridge/package.json"
  check_npm_private_true "packages/extension/package.json"
  check_no_generated_release_outputs
  require_text "docs/release/RELEASE_PROCESS.md" 'source-only public alpha' "source-only release process"
  require_text "docs/QUICKSTART.md" 'source-only alpha path' "source-only quickstart wording"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Package publication: `?\[none for source-only alpha' \
    "source-only package-publication release note field"
}

check_binary_process() {
  require_binary_sections_decided
  require_file "docs/release/BINARY_RELEASE_PROCESS.md"
  require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'Gatekeeper|unsigned|notarization|Developer ID' \
    "binary trust/signing guidance"
  require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'checksum|sha256' "checksum guidance"
  require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'rollback|upgrade' "upgrade or rollback guidance"
}

case "$checked" in
  "- [x] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code"*)
    check_source_only
    ;;
  "- [x] Source plus unsigned macOS app bundle.")
    check_binary_process
    require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'unsigned' "unsigned binary distribution warning"
    ;;
  "- [x] Source plus signed/notarized macOS app bundle.")
    check_binary_process
    require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'Developer ID' "Developer ID signing guidance"
    require_text "docs/release/BINARY_RELEASE_PROCESS.md" 'notarization|notarized' "notarization guidance"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other distribution decision must contain a concrete value"
      exit 1
    fi
    require_text "docs/release/DISTRIBUTION_SCOPE.md" '^Distribution scope:' \
      "a concrete 'Distribution scope:' line"
    ;;
  *)
    echo "FAIL: unsupported distribution scope decision: $checked"
    exit 1
    ;;
esac

echo "Distribution decision check passed."

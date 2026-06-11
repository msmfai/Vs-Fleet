#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"
release_notes="${3:-docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -d "$root" ]; then
  echo "FAIL: missing repository root: $root"
  exit 1
fi

if [ ! -f "$root/$release_notes" ]; then
  echo "FAIL: missing release notes/template file: $release_notes"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

branding_block="$(
  sed -n '/^### 13\. Branding Stability$/,/^### 14\. Versioning And Compatibility$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$branding_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: branding stability decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$branding_block" | rg '^- \[x\] ' | head -n1)"
expected_branding=""
branding_choice=""
case "$checked" in
  "- [x] \`Fleet\` name and current icon are alpha placeholders.")
    expected_branding="alpha placeholders"
    branding_choice="placeholders"
    ;;
  "- [x] \`Fleet\` name is stable, icon may change.")
    expected_branding="Fleet name stable, icon may change"
    branding_choice="name-stable"
    ;;
  "- [x] Name and icon are stable.")
    expected_branding="name and icon stable"
    branding_choice="stable"
    ;;
  "- [x] Other: "*)
    expected_branding="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$expected_branding" ] || [ "$expected_branding" = "TODO" ]; then
      echo "FAIL: checked Other branding decision must contain a concrete value"
      exit 1
    fi
    branding_choice="other"
    ;;
  *)
    echo "FAIL: unsupported branding stability decision: $checked"
    exit 1
    ;;
esac

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
  if ! rg -q "$pattern" "$root/$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

require_file "crates/fleet-host/icons/icon.png"
if [ ! -s "$root/crates/fleet-host/icons/icon.png" ]; then
  echo "FAIL: crates/fleet-host/icons/icon.png is empty"
  exit 1
fi

require_text "crates/fleet-host/build.rs" 'rerun-if-changed=icons/icon\.png' \
  "a rebuild trigger for the replaceable source icon"
require_text "crates/fleet-host/bundle.sh" 'refresh-icons\.sh.*--strict' \
  "strict icon refresh before app bundling"
require_text "crates/fleet-host/bundle.sh" 'replacing that one file is enough' \
  "the source-icon replacement contract"
require_text "crates/fleet-host/tauri.conf.json" '"icons/128x128\.png"' \
  "generated PNG icon in the Tauri bundle config"
require_text "crates/fleet-host/tauri.conf.json" '"icons/Fleet\.icns"' \
  "generated ICNS icon in the Tauri bundle config"
require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'Branding stability' \
  "the public branding-stability decision prompt"
require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'release notes checker requires the decision to be stated' \
  "the release-notes branding follow-up"

branding_line="$(sed -n 's/^- Branding:[[:space:]]*//p' "$root/$release_notes" | head -n1)"
if [ -z "$branding_line" ]; then
  echo "FAIL: $release_notes must contain a Branding field"
  exit 1
fi

branding_value="$(printf '%s\n' "$branding_line" | sed 's/^`//; s/`$//; s/^\[//; s/\]$//')"
if printf '%s\n' "$branding_value" | rg -q '\|'; then
  for option in \
    "alpha placeholders" \
    "Fleet name stable, icon may change" \
    "name and icon stable"
  do
    if ! printf '%s\n' "$branding_value" | rg -F -q "$option"; then
      echo "FAIL: $release_notes Branding template must include \"$option\""
      exit 1
    fi
  done
else
  branding_matches=0
  case "$branding_choice" in
    placeholders)
      if [ "$branding_value" = "alpha placeholders" ] || \
        [ "$branding_value" = "Fleet name and current icon are alpha placeholders" ]; then
        branding_matches=1
      fi
      ;;
    name-stable)
      if [ "$branding_value" = "Fleet name stable, icon may change" ] || \
        [ "$branding_value" = "Fleet name is stable, icon may change" ]; then
        branding_matches=1
      fi
      ;;
    stable|other)
      if [ "$branding_value" = "$expected_branding" ]; then
        branding_matches=1
      fi
      ;;
  esac
fi

if [ "${branding_matches:-1}" -ne 1 ]; then
  echo "FAIL: $release_notes Branding is \"$branding_value\", expected \"$expected_branding\" from owner decision"
  exit 1
fi

echo "Branding decision check passed."

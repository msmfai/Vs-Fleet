#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/apply-license-decision.sh [owner-record] [repo-root] [license-file]

Apply the approved owner license decision to release metadata:
  - root Cargo.toml workspace.package.license
  - crates/fleet-host/Cargo.toml package.license
  - packages/fleet-bridge/package.json and package-lock.json root package
  - packages/extension/package.json and package-lock.json root package

The script does not invent legal license text. If license-file is provided it is
copied to <repo-root>/LICENSE. Otherwise <repo-root>/LICENSE must already exist
and be nonempty.
EOF
}

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"
license_file="${3:-}"

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 2
fi

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record" >&2
  exit 1
fi

if [ ! -d "$root" ]; then
  echo "FAIL: repo root does not exist: $root" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: jq is required to update npm manifests and lockfiles" >&2
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved" >&2
  exit 1
fi

license_block="$(
  sed -n '/^### 1\. License$/,/^### 2\. Public History$/p' "$owner_record"
)"
checked_count="$(printf '%s\n' "$license_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: license decision must have exactly one checked choice; found $checked_count" >&2
  exit 1
fi

checked="$(printf '%s\n' "$license_block" | rg '^- \[x\] ' | head -n1)"
case "$checked" in
  "- [x] MIT OR Apache-2.0 dual license.") license_expr="MIT OR Apache-2.0" ;;
  "- [x] MIT only.") license_expr="MIT" ;;
  "- [x] Apache-2.0 only.") license_expr="Apache-2.0" ;;
  "- [x] AGPL-3.0-only.") license_expr="AGPL-3.0-only" ;;
  "- [x] Other: "*)
    license_expr="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$license_expr" ] || [ "$license_expr" = "TODO" ]; then
      echo "FAIL: checked Other license decision must contain a concrete SPDX expression" >&2
      exit 1
    fi
    ;;
  *)
    echo "FAIL: unsupported license decision: $checked" >&2
    exit 1
    ;;
esac

if [ -n "$license_file" ]; then
  if [ ! -s "$license_file" ]; then
    echo "FAIL: license file is missing or empty: $license_file" >&2
    exit 1
  fi
  cp "$license_file" "$root/LICENSE"
elif [ ! -s "$root/LICENSE" ]; then
  echo "FAIL: $root/LICENSE is missing or empty; pass a license-file to copy" >&2
  exit 1
fi

require_file() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file" >&2
    exit 1
  fi
}

update_cargo_license() {
  local file=$1
  require_file "$file"
  LICENSE_EXPR="$license_expr" perl -0pi -e '
    my $license = $ENV{LICENSE_EXPR};
    if (s/^license\s*=\s*"[^"]*"/license = "$license"/m == 0) {
      die "missing license field\n";
    }
  ' "$root/$file"
}

update_json_license() {
  local file=$1
  require_file "$file"
  local tmp="$root/$file.$$"
  jq --arg license "$license_expr" '.license = $license' "$root/$file" >"$tmp"
  mv "$tmp" "$root/$file"
}

update_lock_root_license() {
  local file=$1
  require_file "$file"
  local tmp="$root/$file.$$"
  jq --arg license "$license_expr" '.packages[""].license = $license' "$root/$file" >"$tmp"
  mv "$tmp" "$root/$file"
}

update_cargo_license "Cargo.toml"
update_cargo_license "crates/fleet-host/Cargo.toml"
update_json_license "packages/fleet-bridge/package.json"
update_json_license "packages/extension/package.json"
update_lock_root_license "packages/fleet-bridge/package-lock.json"
update_lock_root_license "packages/extension/package-lock.json"

echo "Applied license metadata: $license_expr"
echo "Root LICENSE: $root/LICENSE"

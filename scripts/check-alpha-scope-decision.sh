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

scope_block="$(
  sed -n '/^### 4\. Alpha Scope$/,/^### 5\. Editor Server Licensing Boundary$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$scope_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: alpha scope decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$scope_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder alpha-scope text"
    exit 1
  fi
}

check_local_macos_scope() {
  require_text "README.md" 'macOS Tauri Fleet host' "macOS host alpha scope"
  require_text "README.md" 'local `?code serve-web`? sessions' "local code serve-web alpha scope"
  require_text "README.md" 'Fleet bridge' "Fleet bridge alpha scope"
  require_text "README.md" 'reporter' "Fleet reporter alpha scope"
  require_text "README.md" 'Remote/container deployment' "README remote/container scope heading"
  require_text "README.md" 'supported alpha path|not release-ready' \
    "README remote/container non-support boundary"

  require_text "docs/QUICKSTART.md" 'local macOS' "local macOS quickstart scope"
  require_text "docs/QUICKSTART.md" 'local `?code serve-web`?' "local code serve-web quickstart scope"
  require_text "docs/QUICKSTART.md" 'Remote, SSH, and container' \
    "quickstart remote/SSH/container path list"
  require_text "docs/QUICKSTART.md" 'alpha commitments' \
    "quickstart remote/SSH/container non-support boundary"

  require_text "docs/ARCHITECTURE.md" '^## Supported Alpha Surface' "supported alpha surface section"
  require_text "docs/ARCHITECTURE.md" 'macOS Fleet host' "architecture macOS host scope"
  require_text "docs/ARCHITECTURE.md" 'Local `?code serve-web`? sessions' "architecture local code serve-web scope"
  require_text "docs/ARCHITECTURE.md" 'Fleet bridge extension' "architecture bridge scope"
  require_text "docs/ARCHITECTURE.md" 'Fleet reporter process' "architecture reporter scope"
  require_text "docs/ARCHITECTURE.md" 'Embedded local Hub' "architecture embedded Hub scope"
  require_text "docs/ARCHITECTURE.md" 'Remote, SSH, Docker/container, visual probe, and eval harness paths' \
    "architecture remote/container/eval path list"
  require_text "docs/ARCHITECTURE.md" 'public support commitments' \
    "architecture remote/container/eval non-support boundary"
  require_text "containers/fleet-env/eval/README.md" '^## Public alpha support boundary' \
    "eval harness public alpha support boundary section"
  require_text "containers/fleet-env/eval/README.md" 'development and release-verification infrastructure' \
    "eval harness development-infrastructure classification"
  require_text "containers/fleet-env/eval/README.md" 'not supported public alpha user paths' \
    "eval harness non-support boundary"
  require_text "containers/fleet-env/eval/README.md" 'local macOS Fleet host' \
    "eval harness local macOS host pointer"
  require_text "containers/fleet-env/eval/README.md" 'user-provided `?code serve-web`?' \
    "eval harness user-provided code serve-web pointer"

  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'local macOS source builds' \
    "release-notes local macOS scope"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'local `?code serve-web`? sessions' \
    "release-notes local code serve-web scope"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'container/remote deployment as a supported user path' \
    "release-notes remote/container non-support boundary"
}

case "$checked" in
  "- [x] Local macOS Fleet host plus local \`code serve-web\` sessions, Fleet bridge,"*)
    check_local_macos_scope
    ;;
  "- [x] Broaden public alpha scope to include remote, SSH, Docker/container,"*)
    reject_placeholder_file "docs/release/ALPHA_SCOPE.md"
    require_text "docs/release/ALPHA_SCOPE.md" '^Alpha scope:' \
      "a concrete 'Alpha scope:' line"
    require_text "docs/release/ALPHA_SCOPE.md" 'remote|SSH|Docker|container|eval' \
      "the broadened supported workflow names"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other alpha scope decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/ALPHA_SCOPE.md"
    require_text "docs/release/ALPHA_SCOPE.md" '^Alpha scope:' \
      "a concrete 'Alpha scope:' line"
    ;;
  *)
    echo "FAIL: unsupported alpha scope decision: $checked"
    exit 1
    ;;
esac

echo "Alpha scope decision check passed."

#!/usr/bin/env bash
set -euo pipefail

assessment="${1:-docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md}"

fail=0

require_file() {
  local file=$1
  if [ ! -f "$file" ]; then
    echo "FAIL: missing public alpha readiness assessment: $file"
    exit 1
  fi
}

require_text() {
  local pattern=$1
  local description=$2
  if ! rg -qi "$pattern" "$assessment"; then
    echo "FAIL: readiness assessment must state $description"
    fail=1
  fi
}

require_file "$assessment"

if rg -n 'TODO|TBD|PLACEHOLDER|\[[^]]*(known alpha limitation|one-line change|full commit SHA|YYYY-MM-DD)[^]]*\]' "$assessment"; then
  echo "FAIL: readiness assessment contains unresolved placeholders"
  fail=1
fi

require_text '^Current verdict:[[:space:]]*GATED FOR PUBLIC SOURCE ALPHA\.' \
  "the current gated source-alpha verdict"
require_text 'credible for a source-only public alpha after the release gates pass' \
  "that source alpha is acceptable only after gates pass"
require_text 'general end-user product' \
  "that the project is not ready as a general product"
require_text 'public macOS binary' \
  "that public macOS binaries are not ready"
require_text 'package-index release' \
  "that package-index publication is not ready"
require_text 'supported remote/container platform' \
  "that remote/container support is not ready"
require_text 'source-only public alpha' \
  "source-only alpha scope"
require_text 'technical review and dogfooding' \
  "the intended alpha audience"
require_text 'stable APIs|stable API' \
  "stable API non-commitment"
require_text 'binary install support|download the binary|public macOS binary' \
  "binary distribution non-commitment"
require_text '"Fleet supports remote machines, containers, or SSH workflows\."' \
  "remote/container non-commitment"
require_text 'production support|response SLAs|stable release lines' \
  "support non-commitment"
require_text 'redistributes Microsoft' \
  "Microsoft editor-server redistribution boundary"
require_text 'owner decision record is not approved|OWNER_DECISION_RECORD\.md' \
  "owner decision gating"
require_text 'public namespace table' \
  "namespace gating"
require_text 'branch history contains local paths|cleaned public branch' \
  "history cleanup gating"
require_text 'Public branch evidence|public branch evidence' \
  "public branch evidence gating"
require_text 'public CI evidence' \
  "public CI evidence gating"
require_text 'GitHub publication evidence' \
  "GitHub publication evidence gating"
require_text 'dependency review evidence' \
  "dependency review evidence gating"
require_text 'source-only alpha' \
  "required source-only disclosure"
require_text 'macOS local source build' \
  "required supported platform disclosure"
require_text 'user-provided VS Code.*code serve-web|code serve-web' \
  "required editor-server disclosure"
require_text 'no telemetry by default' \
  "required telemetry disclosure"
require_text '~/.fleet/run.*~/.fleet/mux|~/.fleet/mux.*~/.fleet/run' \
  "required local data disclosure"
require_text 'best-effort support' \
  "required support disclosure"
require_text 'no stable API, protocol, state-file, or upgrade compatibility' \
  "required compatibility disclosure"
require_text 'provisional `?Fleet`? name' \
  "required naming disclosure"
require_text 'public-alpha-decision-packet\.sh' \
  "owner packet decision rule"
require_text 'release-evidence-status\.sh' \
  "release evidence decision rule"
require_text 'check-public-release-branch\.sh' \
  "clean public branch verifier decision rule"
require_text 'release-check\.sh' \
  "current-history release gate fallback"

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Public alpha readiness assessment check passed."

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

intent="$TMPDIR/LICENSE_INTENT.md"
dco="$TMPDIR/DCO.md"
contributing="$TMPDIR/CONTRIBUTING.md"
pr="$TMPDIR/PULL_REQUEST_TEMPLATE.md"

write_valid() {
  cat >"$intent" <<'EOF'
# License Intent

Fleet should ship as MIT OR Apache-2.0. LICENSE-MIT and LICENSE-APACHE are
tracked, and manifests use SPDX metadata. Developer Certificate of Origin (DCO)
sign-off is used; it does not assign copyright and does not give the maintainer
relicensing rights over contributor code. Revisit a Contributor License
Agreement (CLA) before commercial relicensing. Released versions remain
available under their release license. Keep reusable library/API crates
permissive. AGPL-3.0-only is a contingency for a future hosted control plane or
hosted-reseller trigger.
EOF
  cat >"$dco" <<'EOF'
# DCO

Signed-off-by: Your Name <your.email@example.com>
Use git commit -s. You certify the right to submit under the project license.
EOF
  cat >"$contributing" <<'EOF'
# Contributing

Developer Certificate of Origin (DCO) sign-off is required. Add Signed-off-by
to every commit. No Contributor License Agreement (no CLA) is required for
alpha.
EOF
  cat >"$pr" <<'EOF'
# Pull Request

- [ ] I agree to the Developer Certificate of Origin (DCO) and included
  Signed-off-by on every code commit.
EOF
}

expect_pass() {
  if ! "$ROOT/scripts/check-license-intent.sh" "$intent" "$dco" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected valid license intent policy to pass" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  if "$ROOT/scripts/check-license-intent.sh" "$intent" "$dco" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

write_valid
expect_pass

perl -0pi -e 's/does not assign copyright/assigns copyright/' "$intent"
expect_fail "DCO copyright limitation is required"

write_valid
perl -0pi -e 's/Signed-off-by/Signed by/' "$pr"
expect_fail "PR template must require Signed-off-by"

echo "License intent check tests passed."

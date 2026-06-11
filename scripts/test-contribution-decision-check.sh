#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 6. Contribution Intake

- [$([ "$checked" = "accept" ] && echo x || echo ' ')] Accept small focused PRs under the chosen project license using the PR
  template certification.
- [$([ "$checked" = "dco" ] && echo x || echo ' ')] Require DCO sign-off.
- [$([ "$checked" = "closed" ] && echo x || echo ' ')] Keep code PRs closed; accept issues and docs feedback only.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Maintainer invitation only\`

### 7. Public CI Evidence
EOF
}

write_docs() {
  local contributing=$1
  local pr=$2
  local contributing_body=$3
  local pr_body=$4
  printf '# Contributing\n\n%s\n' "$contributing_body" >"$contributing"
  printf '# Pull Request\n\n%s\n' "$pr_body" >"$pr"
}

expect_pass() {
  local label=$1
  local owner=$2
  local contributing=$3
  local pr=$4
  if ! "$ROOT/scripts/check-contribution-decision.sh" "$owner" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local contributing=$3
  local pr=$4
  if "$ROOT/scripts/check-contribution-decision.sh" "$owner" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_accept="$TMPDIR/owner-accept.md"
contributing_accept="$TMPDIR/contributing-accept.md"
pr_accept="$TMPDIR/pr-accept.md"
write_owner_record "$owner_accept" APPROVED accept
write_docs "$contributing_accept" "$pr_accept" \
  "Contributions are licensed under the same license as the project." \
  "- [ ] I certify that I can license this contribution under the project license."
expect_pass "same-license PR certification is accepted" "$owner_accept" "$contributing_accept" "$pr_accept"

pr_missing_cert="$TMPDIR/pr-missing-cert.md"
write_docs "$contributing_accept" "$pr_missing_cert" \
  "Contributions are licensed under the same license as the project." \
  "- [ ] I ran tests."
expect_fail "accepted PRs require license certification" "$owner_accept" "$contributing_accept" "$pr_missing_cert"

contributing_provisional="$TMPDIR/contributing-provisional.md"
write_docs "$contributing_provisional" "$pr_accept" \
  "This section must be finalized before accepting outside code contributions." \
  "- [ ] I certify that I can license this contribution under the project license."
expect_fail "provisional contribution guide is rejected" "$owner_accept" "$contributing_provisional" "$pr_accept"

owner_dco="$TMPDIR/owner-dco.md"
contributing_dco="$TMPDIR/contributing-dco.md"
pr_dco="$TMPDIR/pr-dco.md"
write_owner_record "$owner_dco" APPROVED dco
write_docs "$contributing_dco" "$pr_dco" \
  "Developer Certificate of Origin (DCO) is required. Add a Signed-off-by line to every commit." \
  "- [ ] I agree to the DCO and included Signed-off-by on every commit."
expect_pass "DCO policy is accepted" "$owner_dco" "$contributing_dco" "$pr_dco"

owner_closed="$TMPDIR/owner-closed.md"
contributing_closed="$TMPDIR/contributing-closed.md"
pr_closed="$TMPDIR/pr-closed.md"
write_owner_record "$owner_closed" APPROVED closed
write_docs "$contributing_closed" "$pr_closed" \
  "Code PRs are closed for this alpha; use issues and docs feedback only." \
  "Code contributions are not accepted for this alpha; docs feedback only."
expect_pass "code-PR-closed policy is accepted" "$owner_closed" "$contributing_closed" "$pr_closed"

owner_other="$TMPDIR/owner-other.md"
contributing_other="$TMPDIR/contributing-other.md"
pr_other="$TMPDIR/pr-other.md"
write_owner_record "$owner_other" APPROVED other
write_docs "$contributing_other" "$pr_other" \
  "Contribution intake policy: maintainer invitation only." \
  "Contribution policy: maintainer invitation only."
expect_pass "concrete Other policy is accepted" "$owner_other" "$contributing_other" "$pr_other"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING accept
expect_fail "pending owner record is rejected" "$owner_pending" "$contributing_accept" "$pr_accept"

echo "Contribution decision check tests passed."

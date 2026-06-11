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

### 17. AI-Assisted Contribution Provenance

- [$([ "$checked" = "allow" ] && echo x || echo ' ')] Allow AI-assisted contributions if the contributor certifies human review, right to submit, and no private prompts, logs, or generated artifacts.
- [$([ "$checked" = "approval" ] && echo x || echo ' ')] Require maintainer approval before accepting AI-generated code or model-generated patches.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`No AI-generated code during alpha\`

## Required Before Binary Distribution
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
  if ! "$ROOT/scripts/check-ai-contribution-decision.sh" "$owner" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
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
  if "$ROOT/scripts/check-ai-contribution-decision.sh" "$owner" "$contributing" "$pr" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_allow="$TMPDIR/owner-allow.md"
contributing_allow="$TMPDIR/contributing-allow.md"
pr_allow="$TMPDIR/pr-allow.md"
write_owner_record "$owner_allow" APPROVED allow
write_docs "$contributing_allow" "$pr_allow" \
  "AI-assisted contributions are allowed when you reviewed and understand the change, have the right to submit it, include no private prompts, private model transcripts, private logs, workspace paths, generated build outputs, raw logs, or machine-specific paths." \
  "- [ ] AI-assisted contribution: I reviewed and understand the change and included no private prompts, private model transcripts, private logs, or workspace paths."
expect_pass "AI-assisted contributions with certification are accepted" "$owner_allow" "$contributing_allow" "$pr_allow"

missing_private_boundary="$TMPDIR/missing-private-boundary.md"
write_docs "$missing_private_boundary" "$pr_allow" \
  "AI-assisted contributions are allowed when you reviewed and understand the change and have the right to submit it." \
  "- [ ] AI-assisted contribution: I reviewed and understand the change and included no private prompts, private model transcripts, private logs, or workspace paths."
expect_fail "AI-assisted policy requires private prompt/log boundary" "$owner_allow" "$missing_private_boundary" "$pr_allow"

owner_approval="$TMPDIR/owner-approval.md"
contributing_approval="$TMPDIR/contributing-approval.md"
pr_approval="$TMPDIR/pr-approval.md"
write_owner_record "$owner_approval" APPROVED approval
write_docs "$contributing_approval" "$pr_approval" \
  "AI-generated code and model-generated patches require explicit maintainer approval before submission." \
  "- [ ] This AI-generated or model-generated patch had explicit maintainer approval."
expect_pass "maintainer approval policy is accepted" "$owner_approval" "$contributing_approval" "$pr_approval"

owner_other="$TMPDIR/owner-other.md"
contributing_other="$TMPDIR/contributing-other.md"
pr_other="$TMPDIR/pr-other.md"
write_owner_record "$owner_other" APPROVED other
write_docs "$contributing_other" "$pr_other" \
  "AI contribution policy: no AI-generated code during public alpha." \
  "AI contribution policy: no AI-generated code during public alpha."
expect_pass "concrete Other AI policy is accepted" "$owner_other" "$contributing_other" "$pr_other"

placeholder="$TMPDIR/placeholder.md"
write_docs "$placeholder" "$pr_other" \
  "AI policy pending." \
  "AI contribution policy: no AI-generated code during public alpha."
expect_fail "placeholder AI policy is rejected" "$owner_allow" "$placeholder" "$pr_other"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING allow
expect_fail "pending owner record is rejected" "$owner_pending" "$contributing_allow" "$pr_allow"

echo "AI contribution decision check tests passed."

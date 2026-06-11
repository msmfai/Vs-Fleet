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

### 15. Community Intake And Moderation

- [$([ "$checked" = "limited" ] && echo x || echo ' ')] Open public issues only for scoped bug reports and alpha feedback; keep blank issues disabled and keep discussions off unless explicitly enabled later.
- [$([ "$checked" = "closed" ] && echo x || echo ' ')] Keep public issues and discussions closed during alpha; collect feedback privately or by invite only.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private preview feedback only\`

## Required Before Binary Distribution
EOF
}

write_limited_docs() {
  local root=$1
  mkdir -p "$root/.github/ISSUE_TEMPLATE" "$root/docs/release"
  cat >"$root/.github/ISSUE_TEMPLATE/config.yml" <<'EOF'
blank_issues_enabled: false
EOF
  cat >"$root/.github/ISSUE_TEMPLATE/bug_report.yml" <<'EOF'
name: Bug report
description: Report a reproducible problem in the supported local alpha path.
body:
  - type: markdown
    attributes:
      value: Do not report vulnerabilities or exploit details in public issues; use SECURITY.md.
  - type: checkboxes
    id: scope
    attributes:
      options:
        - label: This is about the local macOS Fleet host, local code serve-web sessions, reporter, bridge, or CLI.
EOF
  cat >"$root/.github/ISSUE_TEMPLATE/alpha_feedback.yml" <<'EOF'
name: Alpha feedback
body:
  - type: markdown
    attributes:
      value: Use this for product/readiness feedback rather than a specific reproducible bug.
  - type: dropdown
    id: topic
    attributes:
      options:
        - Security/privacy expectations
EOF
  cat >"$root/CODE_OF_CONDUCT.md" <<'EOF'
# Code of Conduct

Fleet is not yet open for broad public contribution, but public discussion still needs clear expectations.
Do not post private data, credentials, unredacted logs, local paths, screenshots containing private information, or exploit details in public issues.
Maintainers may edit, hide, lock, or remove issues, pull requests, comments, or accounts from project spaces.
EOF
  cat >"$root/docs/release/GITHUB_PUBLICATION_RUNBOOK.md" <<'EOF'
# GitHub Publication Runbook

- Review Discussions before public visibility.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-community-intake-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-community-intake-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_limited="$TMPDIR/owner-limited.md"
limited_root="$TMPDIR/limited-root"
write_owner_record "$owner_limited" APPROVED limited
write_limited_docs "$limited_root"
expect_pass "limited public issue intake is accepted" "$owner_limited" "$limited_root"

missing_moderation="$TMPDIR/missing-moderation"
write_limited_docs "$missing_moderation"
printf '# Code of Conduct\n' >"$missing_moderation/CODE_OF_CONDUCT.md"
expect_fail "missing moderation enforcement is rejected" "$owner_limited" "$missing_moderation"

owner_closed="$TMPDIR/owner-closed.md"
closed_root="$TMPDIR/closed-root"
write_owner_record "$owner_closed" APPROVED closed
write_limited_docs "$closed_root"
cat >"$closed_root/docs/release/COMMUNITY_INTAKE.md" <<'EOF'
# Community Intake

Community intake commitment: public issues and discussions remain closed during alpha.
Feedback channel: private invite-only feedback.
Moderation policy: maintainers may remove unsafe or off-scope content from project spaces.
EOF
expect_pass "closed public intake policy is accepted" "$owner_closed" "$closed_root"

closed_placeholder="$TMPDIR/closed-placeholder"
write_limited_docs "$closed_placeholder"
cat >"$closed_placeholder/docs/release/COMMUNITY_INTAKE.md" <<'EOF'
Community intake commitment: TODO
Feedback channel: TODO
Moderation policy: TODO
EOF
expect_fail "placeholder closed intake policy is rejected" "$owner_closed" "$closed_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_limited_docs "$other_root"
printf 'Community intake commitment: invite-only research cohort\n' >"$other_root/docs/release/COMMUNITY_INTAKE.md"
expect_pass "concrete Other community policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING limited
expect_fail "pending owner record is rejected" "$owner_pending" "$limited_root"

echo "Community intake decision check tests passed."

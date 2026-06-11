#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_templates() {
  local dir=$1
  mkdir -p "$dir/.github/ISSUE_TEMPLATE"

  cat >"$dir/.github/ISSUE_TEMPLATE/config.yml" <<'EOF'
blank_issues_enabled: false
EOF

  cat >"$dir/.github/ISSUE_TEMPLATE/bug_report.yml" <<'EOF'
name: Bug report
description: Report a reproducible problem in the supported local alpha path.
title: "[bug]: "
labels: ["bug", "alpha"]
body:
  - type: markdown
    attributes:
      value: |
        Fleet is pre-release alpha software. Please scrub workspace paths, local URLs, logs, screenshots, and command lines before posting.
        Do not report vulnerabilities or exploit details in public issues; use SECURITY.md.
  - type: checkboxes
    id: scope
    attributes:
      label: Alpha scope
      options:
        - label: This is about the local macOS Fleet host, local code serve-web sessions, reporter, bridge, or CLI.
          required: true
        - label: I understand remote/container deployment and binary distribution are experimental unless explicitly documented.
          required: true
  - type: input
    id: version
    validations:
      required: true
  - type: textarea
    id: steps
    validations:
      required: true
  - type: textarea
    id: environment
    validations:
      required: true
EOF

  cat >"$dir/.github/ISSUE_TEMPLATE/alpha_feedback.yml" <<'EOF'
name: Alpha feedback
description: Share feedback on rough edges, release blockers, or expected alpha behavior.
title: "[alpha feedback]: "
labels: ["alpha-feedback"]
body:
  - type: markdown
    attributes:
      value: |
        Use this for product/readiness feedback rather than a specific reproducible bug. Scrub private local details before posting.
        Do not report vulnerabilities or exploit details in public issues; use SECURITY.md.
  - type: dropdown
    id: topic
    attributes:
      label: Topic
      options:
        - Security/privacy expectations
        - Release packaging
  - type: textarea
    id: context
    validations:
      required: true
  - type: textarea
    id: feedback
    validations:
      required: true
EOF

  cat >"$dir/.github/PULL_REQUEST_TEMPLATE.md" <<'EOF'
# Pull Request

Fleet accepts focused alpha contributions under the project license with DCO
sign-off. Broad or speculative code changes are triaged narrowly during alpha.

## Checks

- [ ] I did not add generated build output, raw logs, private screenshots, or machine-specific paths.

## Test Evidence

Commands run:

## Contribution Licensing

- [ ] I understand broad alpha contributions are triaged narrowly even when they satisfy the licensing and DCO requirements.
EOF
}

expect_pass() {
  local label=$1
  local dir=$2
  if ! "$ROOT/scripts/check-github-intake-templates.sh" \
    "$dir/.github/ISSUE_TEMPLATE/bug_report.yml" \
    "$dir/.github/ISSUE_TEMPLATE/alpha_feedback.yml" \
    "$dir/.github/ISSUE_TEMPLATE/config.yml" \
    "$dir/.github/PULL_REQUEST_TEMPLATE.md" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local dir=$2
  if "$ROOT/scripts/check-github-intake-templates.sh" \
    "$dir/.github/ISSUE_TEMPLATE/bug_report.yml" \
    "$dir/.github/ISSUE_TEMPLATE/alpha_feedback.yml" \
    "$dir/.github/ISSUE_TEMPLATE/config.yml" \
    "$dir/.github/PULL_REQUEST_TEMPLATE.md" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

valid="$TMPDIR/valid"
write_templates "$valid"
expect_pass "valid intake templates are accepted" "$valid"

blank_issues="$TMPDIR/blank-issues"
write_templates "$blank_issues"
printf 'blank_issues_enabled: true\n' >"$blank_issues/.github/ISSUE_TEMPLATE/config.yml"
expect_fail "blank issues must stay disabled" "$blank_issues"

public_vuln="$TMPDIR/public-vuln"
write_templates "$public_vuln"
perl -0pi -e 's/        Do not report vulnerabilities or exploit details in public issues; use SECURITY\.md\.\n//' \
  "$public_vuln/.github/ISSUE_TEMPLATE/bug_report.yml"
expect_fail "bug report must redirect vulnerability details" "$public_vuln"

scope_missing="$TMPDIR/scope-missing"
write_templates "$scope_missing"
perl -0pi -e 's/local macOS Fleet host, local code serve-web sessions, reporter, bridge, or CLI/something else/' \
  "$scope_missing/.github/ISSUE_TEMPLATE/bug_report.yml"
expect_fail "bug report must keep supported alpha scope" "$scope_missing"

pr_artifacts="$TMPDIR/pr-artifacts"
write_templates "$pr_artifacts"
perl -0pi -e 's/- \[ \] I did not add generated build output, raw logs, private screenshots, or machine-specific paths\.\n//' \
  "$pr_artifacts/.github/PULL_REQUEST_TEMPLATE.md"
expect_fail "PR template must keep artifact/privacy checklist" "$pr_artifacts"

echo "GitHub intake template check tests passed."

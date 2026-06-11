#!/usr/bin/env bash
set -euo pipefail

file="${1:-docs/release/OWNER_DECISION_RECORD.md}"

if [ ! -f "$file" ]; then
  echo "Public alpha owner decision packet"
  echo
  echo "Release readiness: BLOCKED"
  echo "Missing owner decision record: $file"
  exit 1
fi

required_sections=$(
  cat <<'EOF'
### 1. License
### 2. Public History
### 4. Alpha Scope
### 5. Editor Server Licensing Boundary
### 6. Distribution Scope
### 7. Security Reporting Channel
### 8. Contribution Intake
### 9. Public CI Evidence
### 10. Privacy And Telemetry Posture
### 11. Dependency Review Evidence
### 12. Support Commitment
### 13. Branding Stability
### 14. Versioning And Compatibility
### 15. Community Intake And Moderation
### 16. Release Custody And Maintainer Authority
### 17. AI-Assisted Contribution Provenance
### 18. Supported Platform And Toolchain
### 19. Public Roadmap And Non-Goals
EOF
)

section_block() {
  local source_block=$1
  local section=$2
  local section_line
  section_line="$(printf '%s\n' "$source_block" | rg -n "^${section}$" | cut -d: -f1 | head -n1 || true)"
  if [ -z "$section_line" ]; then
    return 1
  fi

  local next_section_line
  next_section_line="$(
    printf '%s\n' "$source_block" |
      tail -n +"$((section_line + 1))" |
      rg -n '^### ' |
      cut -d: -f1 |
      head -n1 || true
  )"

  if [ -n "$next_section_line" ]; then
    printf '%s\n' "$source_block" | sed -n "${section_line},$((section_line + next_section_line - 1))p"
  else
    printf '%s\n' "$source_block" | sed -n "${section_line},\$p"
  fi
}

required_start="$(rg -n '^## Required Before Public GitHub Visibility$' "$file" | cut -d: -f1 | head -n1 || true)"
required_end="$(rg -n '^## Required Before Binary Distribution$' "$file" | cut -d: -f1 | head -n1 || true)"

echo "Public alpha owner decision packet"
echo "Record: $file"
echo

fail=0
if rg -q '^Decision record status: APPROVED$' "$file"; then
  echo "Decision record status: APPROVED"
else
  echo "Decision record status: not APPROVED"
  fail=1
fi

if [ -z "$required_start" ] || [ -z "$required_end" ] || [ "$required_end" -le "$required_start" ]; then
  echo
  echo "Release readiness: BLOCKED"
  echo "Owner decision record required section boundaries are missing."
  exit 1
fi

required_block="$(sed -n "$((required_start + 1)),$((required_end - 1))p" "$file")"
binary_block="$(sed -n "$((required_end + 1)),\$p" "$file")"

echo
echo "Required decisions needing owner action:"
missing_any=0
while IFS= read -r section; do
  [ -n "$section" ] || continue
  if ! block="$(section_block "$required_block" "$section")"; then
    echo "- ${section#'### '}: missing section"
    missing_any=1
    fail=1
    continue
  fi

  checked_count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
  checked_count="${checked_count:-0}"
  if [ "$checked_count" -ne 1 ]; then
    echo "- ${section#'### '}: choose exactly one option (currently $checked_count checked)"
    missing_any=1
    fail=1
  fi
done <<<"$required_sections"

namespace_block="$(printf '%s\n' "$required_block" | sed -n '/^### 3\. Public Namespace$/,/^### 4\. Alpha Scope$/p')"
namespace_todos="$(printf '%s\n' "$namespace_block" | rg '`TODO`' || true)"
if [ -n "$namespace_todos" ]; then
  echo "- 3. Public Namespace: replace every TODO placeholder"
  printf '%s\n' "$namespace_todos" | sed 's/^/  /'
  missing_any=1
  fail=1
fi

other_todos="$(rg -n '^- \[x\] Other: `TODO`' "$file" || true)"
if [ -n "$other_todos" ]; then
  echo "- Checked Other choices must contain concrete text, not TODO"
  printf '%s\n' "$other_todos" | sed 's/^/  /'
  missing_any=1
  fail=1
fi

distribution_block="$(section_block "$required_block" "### 6. Distribution Scope" || true)"
if printf '%s\n' "$distribution_block" | rg -q '^- \[x\] Source plus|^- \[x\] Other:'; then
  for section in "### 20. macOS Signing and Notarization" "### 21. Update Channel"; do
    if ! block="$(section_block "$binary_block" "$section")"; then
      echo "- ${section#'### '}: missing section required for public binary distribution"
      missing_any=1
      fail=1
      continue
    fi

    checked_count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
    checked_count="${checked_count:-0}"
    if [ "$checked_count" -ne 1 ]; then
      echo "- ${section#'### '}: choose exactly one option because binary distribution is selected"
      missing_any=1
      fail=1
    fi
  done
fi

if [ "$missing_any" -eq 0 ]; then
  echo "- none"
fi

echo
echo "Recommended alpha defaults to review:"
echo "- License: MIT OR Apache-2.0, unless you deliberately want network copyleft."
echo "- History: cleaned/squashed first public branch."
echo "- Scope: local macOS host plus local code serve-web workflow only."
echo "- Distribution: source-only alpha."
echo "- CI: GitHub Actions green on the exact public commit."
echo "- Support: best-effort alpha support only."
echo "- Versioning: alpha pre-release tags only; no stable compatibility promise."
echo "- Community intake: scoped public bug and alpha-feedback issues only; no blank issues."
echo "- Release custody: single-maintainer alpha; no package publishing credentials."
echo "- AI contributions: human-reviewed, rights-certified, no private prompts/logs/artifacts."
echo "- Platform: macOS source alpha only with Rust 1.78+, Node.js 20/npm, Git, and VS Code code CLI."
echo "- Roadmap: no public roadmap commitments during alpha; issues/labels/milestones are triage only."

echo
echo "Mechanical next commands after recording choices:"
if printf '%s\n' "$required_block" | rg -q '^- \[x\] Publish a cleaned/squashed history for the first public branch\.'; then
  echo "  ./scripts/prepare-public-branch.sh public-alpha HEAD"
  echo "  ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha"
  echo "  FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh"
elif printf '%s\n' "$required_block" | rg -q '^- \[x\] Publish the current branch history and accept that old commits may contain'; then
  echo "  ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md"
  echo "  ./scripts/release-check.sh"
else
  echo "  # Choose Public History before selecting the release-check command."
  echo "  # Cleaned history:"
  echo "  ./scripts/prepare-public-branch.sh public-alpha HEAD"
  echo "  FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh"
  echo "  # Current history:"
  echo "  ./scripts/release-check.sh"
fi
echo "  ./scripts/run-dependency-review.sh"
echo "  ./scripts/check-versioning-decision.sh docs/release/OWNER_DECISION_RECORD.md ."
echo "  ./scripts/check-community-intake-decision.sh docs/release/OWNER_DECISION_RECORD.md ."
echo "  ./scripts/check-release-custody-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/GITHUB_PUBLICATION_EVIDENCE.md ."
echo "  ./scripts/check-ai-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md"
echo "  ./scripts/check-platform-support-decision.sh docs/release/OWNER_DECISION_RECORD.md ."
echo "  ./scripts/check-roadmap-decision.sh docs/release/OWNER_DECISION_RECORD.md ."
echo '  ./scripts/check-github-publication-evidence.sh docs/release/OWNER_DECISION_RECORD.md docs/release/GITHUB_PUBLICATION_EVIDENCE.md "$(git rev-parse HEAD)"'

echo
if [ "$fail" -eq 0 ]; then
  echo "Release readiness: OWNER DECISIONS COMPLETE"
else
  echo "Release readiness: BLOCKED"
fi

exit "$fail"

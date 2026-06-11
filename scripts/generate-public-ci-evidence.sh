#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/generate-public-ci-evidence.sh <branch> <ci-run-url> <release-readiness-run-url> [source-ref] [output-file|-]

Write PUBLIC_CI_EVIDENCE.md content for GitHub Actions checks on the exact
commit intended for the first public alpha.

source-ref defaults to HEAD. output-file defaults to
docs/release/PUBLIC_CI_EVIDENCE.md. If output-file is "-", evidence is printed
to stdout. Existing concrete evidence is not overwritten unless
FLEET_PUBLIC_CI_EVIDENCE_FORCE=1 is set.
EOF
}

branch="${1:-}"
ci_run="${2:-}"
release_run="${3:-}"
source_ref="${4:-HEAD}"
output="${5:-docs/release/PUBLIC_CI_EVIDENCE.md}"

if [ -z "$branch" ] || [ -z "$ci_run" ] || [ -z "$release_run" ] ||
  [ "$branch" = "-h" ] || [ "$branch" = "--help" ]; then
  usage
  exit 2
fi

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: public CI evidence generation must run inside a git worktree" >&2
  exit 2
fi

if ! printf '%s\n' "$branch" | rg -q '^[A-Za-z0-9._/-]+$'; then
  echo "FAIL: branch name is not concrete: $branch" >&2
  exit 1
fi

github_run_pattern='^https://github\.com/[^/]+/[^/]+/actions/runs/[0-9]+$'
for pair in "CI workflow run:$ci_run" "Release Readiness workflow run:$release_run"; do
  label="${pair%%:*}"
  value="${pair#*:}"
  if ! printf '%s\n' "$value" | rg -q "$github_run_pattern"; then
    echo "FAIL: $label must be a GitHub Actions run URL: $value" >&2
    exit 1
  fi
done

if [ "$output" != "-" ] && [ -f "$output" ] &&
  ! rg -q 'Public CI evidence status: PENDING|TODO|TBD|PLACEHOLDER|not yet run' "$output" &&
  [ "${FLEET_PUBLIC_CI_EVIDENCE_FORCE:-0}" != "1" ]; then
  echo "FAIL: public CI evidence already looks concrete: $output" >&2
  echo "Set FLEET_PUBLIC_CI_EVIDENCE_FORCE=1 to overwrite reviewed evidence." >&2
  exit 1
fi

source_commit="$(git -C "$root" rev-parse --verify "$source_ref^{commit}")"
release_control_path=""
if [ "$output" != "-" ]; then
  physical_root="$(cd "$root" && pwd -P)"
  case "$output" in
    "$root"/*) release_control_path="${output#"$root/"}" ;;
    /*)
      if [ -d "$(dirname "$output")" ]; then
        physical_out="$(cd "$(dirname "$output")" && pwd -P)/$(basename "$output")"
        case "$physical_out" in
          "$physical_root"/*) release_control_path="${physical_out#"$physical_root/"}" ;;
          *) release_control_path="" ;;
        esac
      fi
      ;;
    *) release_control_path="$output" ;;
  esac
fi

evidence="$(
  cat <<EOF
# Public CI Evidence

Public CI evidence status: PASS

This file records the exact check evidence for the commit that will become the
first public GitHub alpha. Do not mark the owner decision record \`APPROVED\`
until this file is concrete and \`scripts/check-ci-evidence-decision.sh\` passes.

This is release-control evidence. It may be updated after the checked commit is
selected; the verifier compares the checked commit to the release-prep commit
while allowing only the known release-control evidence files under
\`docs/release/*_EVIDENCE.md\` to differ.

## GitHub Actions Evidence

Commit: \`$source_commit\`
Release-control evidence file: \`${release_control_path:-not tracked in this worktree}\`
Branch: \`$branch\`
CI workflow run: \`$ci_run\`
Release Readiness workflow run: \`$release_run\`

## Local-Only Evidence

Local check transcript: \`not used\`

## Other Evidence

CI evidence path: \`not used\`
EOF
)"

if [ "$output" = "-" ]; then
  printf '%s\n' "$evidence"
else
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$evidence" >"$output"
  echo "Wrote public CI evidence: $output"
fi

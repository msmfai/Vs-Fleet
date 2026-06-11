#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/generate-public-branch-evidence.sh <public-branch> <source-ref> [output-file|-]

Write PUBLIC_BRANCH_EVIDENCE.md content for a prepared clean public branch.
The script verifies that:
  - source-ref resolves to a commit
  - public-branch resolves to a single root commit
  - public-branch's tree matches source-ref's tree
  - history-release-check passes for public-branch

The output file is release-control evidence. If it is written into the worktree,
it may differ from the prepared public branch because a commit cannot contain
evidence that names its own future commit hash.

If output-file is omitted or "-", the evidence is printed to stdout. Existing
files are not overwritten unless FLEET_PUBLIC_BRANCH_EVIDENCE_FORCE=1 is set.
EOF
}

public_branch="${1:-}"
source_ref="${2:-}"
output="${3:--}"

if [ -z "$public_branch" ] || [ -z "$source_ref" ] ||
  [ "$public_branch" = "-h" ] || [ "$public_branch" = "--help" ]; then
  usage
  exit 2
fi

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: public branch evidence generation must run inside a git worktree" >&2
  exit 2
fi

if [ "$output" != "-" ] && [ -e "$output" ] &&
  [ "${FLEET_PUBLIC_BRANCH_EVIDENCE_FORCE:-0}" != "1" ]; then
  echo "FAIL: output file already exists: $output" >&2
  echo "Set FLEET_PUBLIC_BRANCH_EVIDENCE_FORCE=1 to overwrite." >&2
  exit 1
fi

source_commit="$(git -C "$root" rev-parse --verify "$source_ref^{commit}")"
public_root="$(git -C "$root" rev-parse --verify "$public_branch^{commit}")"
output_rel=""
release_control_line=""
if [ "$output" != "-" ]; then
  physical_root="$(cd "$root" && pwd -P)"
  case "$output" in
    "$root"/*) output_rel="${output#"$root/"}" ;;
    /*)
      if [ -d "$(dirname "$output")" ]; then
        physical_output="$(cd "$(dirname "$output")" && pwd -P)/$(basename "$output")"
        case "$physical_output" in
          "$physical_root"/*) output_rel="${physical_output#"$physical_root/"}" ;;
          *) output_rel="" ;;
        esac
      fi
      ;;
    *) output_rel="$output" ;;
  esac
  if [ -n "$output_rel" ]; then
    release_control_line="Release-control evidence file: \`$output_rel\`"
  fi
fi

if [ "$(git -C "$root" rev-list --count "$public_branch")" != "1" ]; then
  echo "FAIL: public branch must contain exactly one commit: $public_branch" >&2
  exit 1
fi

if [ "$(git -C "$root" rev-list --parents -n1 "$public_branch" | wc -w | tr -d ' ')" != "1" ]; then
  echo "FAIL: public branch root commit must have no parents: $public_branch" >&2
  exit 1
fi

source_tree="$(git -C "$root" rev-parse "$source_commit^{tree}")"
public_tree="$(git -C "$root" rev-parse "$public_root^{tree}")"
if [ "$source_tree" != "$public_tree" ]; then
  diff_names="$(git -C "$root" diff --name-only "$source_commit" "$public_root")"
  diff_names="$(printf '%s\n' "$diff_names" | awk -v allowed="$output_rel" 'NF && $0 != allowed { print }')"
  if [ -z "$output_rel" ] || [ -n "$diff_names" ]; then
    echo "FAIL: public branch tree does not match source commit tree outside release-control evidence" >&2
    exit 1
  fi
fi

if ! "$root/scripts/history-release-check.sh" "$root/docs/release/OWNER_DECISION_RECORD.md" "$public_branch" >/dev/null; then
  echo "FAIL: history-release-check did not pass for public branch: $public_branch" >&2
  exit 1
fi

evidence="$(
  cat <<EOF
# Public Branch Evidence

Public branch evidence status: PASS

This file records the clean-history branch evidence for the first public GitHub
alpha. Use it when the owner decision record chooses a cleaned/squashed first
public branch. Do not mark the owner decision record \`APPROVED\` until this file
is concrete and \`scripts/check-public-branch-evidence.sh\` passes.

Source commit: \`$source_commit\`
Public branch: \`$public_branch\`
Public root commit: \`$public_root\`
$release_control_line
History check command: \`./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md $public_branch\`
History check result: \`PASS\`

## Required Facts

Single root commit: \`yes\`
Public tree matches source commit tree: \`yes\`
Public branch contains no prior private history: \`yes\`
EOF
)"

if [ "$output" = "-" ]; then
  printf '%s\n' "$evidence"
else
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$evidence" >"$output"
  echo "Wrote public branch evidence: $output"
fi

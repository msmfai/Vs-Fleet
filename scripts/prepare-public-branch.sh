#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/prepare-public-branch.sh <new-branch> [source-ref]

Create a new single-commit branch from the tree at source-ref, default HEAD.
This does not rewrite the current branch or include source-ref's parent history.
EOF
}

branch="${1:-}"
source_ref="${2:-HEAD}"

if [ -z "$branch" ] || [ "$branch" = "-h" ] || [ "$branch" = "--help" ]; then
  usage
  exit 2
fi

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: public branch preparation must run inside a git worktree" >&2
  exit 2
fi

if ! git -C "$root" check-ref-format --branch "$branch" >/dev/null; then
  echo "FAIL: invalid branch name: $branch" >&2
  exit 1
fi

if git -C "$root" show-ref --verify --quiet "refs/heads/$branch"; then
  echo "FAIL: branch already exists: $branch" >&2
  exit 1
fi

source_commit="$(git -C "$root" rev-parse --verify "$source_ref^{commit}")"
tree="$(git -C "$root" rev-parse --verify "$source_commit^{tree}")"
short_source="$(git -C "$root" rev-parse --short=12 "$source_commit")"
message="${FLEET_PUBLIC_BRANCH_MESSAGE:-Initial public alpha source snapshot}"

commit="$(
  printf '%s\n\nSource snapshot: %s\n' "$message" "$source_commit" |
    git -C "$root" commit-tree "$tree"
)"

git -C "$root" branch "$branch" "$commit"

cat <<EOF
Created clean public branch: $branch
Source commit: $source_commit
Public root commit: $commit

Next checks from the release-prep branch:
  ./scripts/generate-public-branch-evidence.sh $branch $source_commit docs/release/PUBLIC_BRANCH_EVIDENCE.md
  ./scripts/check-public-release-branch.sh $branch $source_commit

Push only after release-check passes:
  git push origin $branch

The new branch is a single commit containing the tree from $short_source; it
does not contain the source branch's earlier commits.
EOF

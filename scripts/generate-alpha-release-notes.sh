#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/generate-alpha-release-notes.sh <version> <source-ref> [output-file|-] [change=...] [rough-edge=...]

Generate checked GitHub pre-release notes for the first source alpha from the
approved owner decision record and release evidence files.

The generator refuses to run unless:
  - OWNER_DECISION_RECORD.md is approved and complete
  - public branch, public CI, GitHub publication, and dependency evidence pass
  - the generated notes pass scripts/check-release-notes.sh

output-file defaults to "-". Existing files are not overwritten unless
FLEET_ALPHA_RELEASE_NOTES_FORCE=1 is set. FLEET_RELEASE_DATE may be set to
override the generated Date field for reproducible tests.
EOF
}

version="${1:-}"
source_ref="${2:-}"
output="${3:--}"

if [ -z "$version" ] || [ -z "$source_ref" ] ||
  [ "$version" = "-h" ] || [ "$version" = "--help" ]; then
  usage
  exit 2
fi

shift_count=3
if [ "$#" -lt 3 ]; then
  shift_count="$#"
fi
shift "$shift_count"

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: alpha release notes generation must run inside a git worktree" >&2
  exit 2
fi

owner_record="docs/release/OWNER_DECISION_RECORD.md"
public_branch_evidence="docs/release/PUBLIC_BRANCH_EVIDENCE.md"
public_ci_evidence="docs/release/PUBLIC_CI_EVIDENCE.md"
github_publication_evidence="docs/release/GITHUB_PUBLICATION_EVIDENCE.md"
dependency_evidence="docs/release/DEPENDENCY_REVIEW_EVIDENCE.md"

source_commit="$(git -C "$root" rev-parse --verify "$source_ref^{commit}")"

if ! printf '%s\n' "$version" | rg -q '^v[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+$'; then
  echo "FAIL: version must look like v0.1.0-alpha.1: $version" >&2
  exit 1
fi

release_date="${FLEET_RELEASE_DATE:-$(date +%F)}"
if ! printf '%s\n' "$release_date" | rg -q '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'; then
  echo "FAIL: FLEET_RELEASE_DATE must use YYYY-MM-DD: $release_date" >&2
  exit 1
fi

changes=()
rough_edges=(
  "Source-only alpha; no public app bundle, package registry, marketplace, or container image is published."
  "macOS local source builds are the only supported public alpha path."
  "Remote, SSH, Docker/container, binary install, stable API, and production support workflows are non-goals for this alpha."
)

for arg in "$@"; do
  case "$arg" in
    change=*) changes+=("${arg#change=}") ;;
    rough-edge=*) rough_edges+=("${arg#rough-edge=}") ;;
    *)
      echo "FAIL: unsupported release-notes argument: $arg" >&2
      exit 2
      ;;
  esac
done

if [ "${#changes[@]}" -eq 0 ]; then
  changes=(
    "Prepared the repository for a source-only public alpha with explicit owner gates."
    "Added checked release evidence for public history and dependency review."
    "Documented alpha support, privacy, local data, namespace, and distribution boundaries."
  )
fi

reject_placeholder() {
  local label=$1
  local value=$2
  if [ -z "$value" ] || printf '%s\n' "$value" | rg -qi 'TODO|TBD|PLACEHOLDER|PENDING|not yet'; then
    echo "FAIL: $label is not concrete: $value" >&2
    exit 1
  fi
}

field_value() {
  local file=$1
  local label=$2
  local line
  line="$(rg -i "^${label}:" "$root/$file" | head -n1 || true)"
  if [ -z "$line" ]; then
    return 1
  fi
  local value="${line#*:}"
  value="$(printf '%s' "$value" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//')"
  printf '%s\n' "$value"
}

checked_choice() {
  local start=$1
  local end=$2
  sed -n "/^### ${start}\\./,/^### ${end}\\./p" "$root/$owner_record" |
    rg '^- \[x\] ' | head -n1 | sed 's/^- \[x\] //'
}

run_gate() {
  local label=$1
  shift
  if ! "$@"; then
    echo "FAIL: $label did not pass; release notes were not generated" >&2
    exit 1
  fi
}

run_gate "owner decision record" \
  "$root/scripts/check-owner-decisions.sh" "$root/$owner_record"
run_gate "public branch evidence" \
  "$root/scripts/check-public-branch-evidence.sh" "$root/$owner_record" "$root/$public_branch_evidence" "$source_commit"
run_gate "public CI evidence" \
  "$root/scripts/check-ci-evidence-decision.sh" "$root/$owner_record" "$root/$public_ci_evidence" "$source_commit"
run_gate "GitHub publication evidence" \
  "$root/scripts/check-github-publication-evidence.sh" "$root/$owner_record" "$root/$github_publication_evidence" "$source_commit"
run_gate "dependency review evidence" \
  "$root/scripts/check-dependency-review-decision.sh" "$root/$owner_record" "$root/$dependency_evidence" "$source_commit"

license_choice="$(checked_choice 1 2)"
distribution_choice="$(checked_choice 6 7)"
branding_choice="$(checked_choice 13 14)"
history_choice="$(checked_choice 2 3)"

case "$license_choice" in
  "MIT OR Apache-2.0 dual license.") project_license="MIT OR Apache-2.0" ;;
  "MIT only.") project_license="MIT" ;;
  "Apache-2.0 only.") project_license="Apache-2.0" ;;
  "AGPL-3.0-only.") project_license="AGPL-3.0-only" ;;
  Other:*) project_license="${license_choice#Other: }" ;;
  *) echo "FAIL: unsupported license choice: $license_choice" >&2; exit 1 ;;
esac

case "$distribution_choice" in
  Source-only\ alpha.*) distribution="source-only"; package_publication="none for source-only alpha" ;;
  *)
    echo "FAIL: first alpha release notes generator only supports the source-only distribution decision" >&2
    echo "Distribution choice: $distribution_choice" >&2
    exit 1
    ;;
esac

case "$branding_choice" in
  "\`Fleet\` name and current icon are alpha placeholders.") branding="Fleet name and icon are alpha placeholders" ;;
  "\`Fleet\` name is stable, icon may change.") branding="Fleet name is stable, icon may change" ;;
  "Name and icon are stable.") branding="Name and icon are stable" ;;
  Other:*) branding="${branding_choice#Other: }" ;;
  *) echo "FAIL: unsupported branding choice: $branding_choice" >&2; exit 1 ;;
esac

case "$history_choice" in
  Publish\ a\ cleaned/squashed\ history*) history_audit="cleaned public history" ;;
  Publish\ the\ current\ branch\ history*) history_audit="current history exposure approved in OWNER_DECISION_RECORD.md" ;;
  *) echo "FAIL: unsupported public history choice: $history_choice" >&2; exit 1 ;;
esac

ci_run="$(field_value "$public_ci_evidence" "CI workflow run")"
release_run="$(field_value "$public_ci_evidence" "Release Readiness workflow run")"
dep_date="$(field_value "$dependency_evidence" "Reviewed date")"
accepted_findings="$(field_value "$dependency_evidence" "Accepted findings")"
security_channel="$(field_value "$github_publication_evidence" "Security reporting channel available")"
public_root="$(field_value "$public_branch_evidence" "Public root commit")"

for pair in \
  "CI workflow run:$ci_run" \
  "Release Readiness workflow run:$release_run" \
  "dependency review date:$dep_date" \
  "accepted findings:$accepted_findings" \
  "security reporting channel:$security_channel" \
  "public root commit:$public_root"
do
  reject_placeholder "${pair%%:*}" "${pair#*:}"
done

if [ "$output" != "-" ] && [ -f "$output" ] &&
  [ "${FLEET_ALPHA_RELEASE_NOTES_FORCE:-0}" != "1" ]; then
  echo "FAIL: release notes already exist: $output" >&2
  echo "Set FLEET_ALPHA_RELEASE_NOTES_FORCE=1 to overwrite." >&2
  exit 1
fi

changes_md=""
for change in "${changes[@]}"; do
  reject_placeholder "change" "$change"
  changes_md="${changes_md}- ${change}
"
done

rough_edges_md=""
for edge in "${rough_edges[@]}"; do
  reject_placeholder "rough edge" "$edge"
  rough_edges_md="${rough_edges_md}- ${edge}
"
done

notes="$(
  cat <<EOF
# Fleet $version

## Release

- Version: $version
- Commit: $public_root
- Date: $release_date
- Distribution: $distribution
- Branding: $branding
- Owner decision record: docs/release/OWNER_DECISION_RECORD.md at this commit

## Alpha Scope

This alpha is intended for:

- local macOS source builds,
- local \`code serve-web\` sessions,
- Fleet host, hub, reporter, CLI, and bridge development/testing.

Not supported as a public alpha commitment:

- signed or notarized macOS binaries,
- crates.io, npm, VS Code Marketplace, or Open VSX publication,
- container/remote deployment as a supported user path,
- production support, stable APIs, or backwards-compatible state formats.

## Supported Platform And Toolchain

- macOS source build only.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- Git.
- user-provided VS Code \`code\` CLI.
- Linux, Windows, and remote/container workflows are not supported alpha
  platforms.

## Roadmap And Non-Goals

- No public roadmap commitments are made during alpha.
- Issues, labels, and milestones are triage hints, not delivery promises.
- Remote/container workflows, binary packages, stable APIs, and production
  support remain non-goals unless a later owner decision approves them.

## Naming And Trademark Posture

- \`Fleet\` is a provisional source-alpha working name.
- This release makes no trademark claim to the \`Fleet\` name.
- Stable package or binary publication under Fleet namespaces is deferred until
  the owner completes the public name decision.

## Local Data And Cleanup

- Runtime data lives under \`~/.fleet/run\` and \`~/.fleet/mux\` unless
  \`FLEET_RUNTIME_DIR\` or \`FLEET_MUX_DIR\` is set.
- Manual cleanup after closing Fleet-spawned servers:
  \`rm -rf ~/.fleet/run ~/.fleet/mux\`.
- Quitting Fleet does not promise to delete spawned editor userdata or logs, and
  externally registered sessions are not owned by the host.

## Workflow Supply Chain

- Source-alpha GitHub Actions use read-only \`GITHUB_TOKEN\` permissions:
  \`contents: read\`.
- Release-critical workflows use no repository secrets or publishing
  credentials.
- Tagged third-party Actions are accepted for source alpha; full SHA pinning is
  deferred until binaries, package publishing, or stricter maintainer policy.

## What Changed

$changes_md
## Verification

- GitHub CI on exact commit: $ci_run
- Release readiness workflow: $release_run
- Rust workspace checks: passed in Release Readiness workflow.
- Fleet host checks: passed in Release Readiness workflow.
- JavaScript/package checks: passed in Release Readiness workflow.
- Lockfile policy: passed.
- Dependency review: completed, accepted findings: $accepted_findings.
- Documentation link audit: passed.
- Public tree size audit: passed.
- History exposure audit: $history_audit; public root commit $public_root.
- Secret exposure audit: passed.
- Release hygiene gate: passed on the public release branch.

## Dependency And License Review

- Project license: $project_license
- Third-party dependency review date: $dep_date
- Accepted advisory/license findings: $accepted_findings
- Package publication: $package_publication

## Security And Privacy Notes

- Editor server boundary: user's local \`code serve-web\` only; Fleet does not
  redistribute Microsoft's VS Code Server.
- Fleet is local-first and has no intended telemetry by default.
- Logs and artifacts can contain workspace paths, local URLs, session labels,
  process command lines, and editor state.
- Vulnerability reporting path: $security_channel

## Known Rough Edges

$rough_edges_md
## Upgrade And Rollback

- No stable upgrade path is promised during alpha.
- No auto-update channel is enabled unless explicitly approved in the owner
  decision record.
- To roll back, check out the previous tag and rebuild from source.
EOF
)"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
printf '%s\n' "$notes" >"$tmp"
"$root/scripts/check-release-notes.sh" "$tmp" "$public_root" >/dev/null

if [ "$output" = "-" ]; then
  printf '%s\n' "$notes"
else
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$notes" >"$output"
  echo "Wrote alpha release notes: $output"
fi

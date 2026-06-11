#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/generate-github-publication-evidence.sh <repo-url> <default-branch> <source-ref> <emergency-removal-owner> [output-file|-] [field=value...]

Write GITHUB_PUBLICATION_EVIDENCE.md content for the exact repository settings
reviewed before the first public GitHub alpha.

repo-url must be a GitHub repository URL. source-ref defaults are not guessed:
pass the commit or ref reviewed for publication. output-file defaults to
docs/release/GITHUB_PUBLICATION_EVIDENCE.md. If output-file is "-", evidence is
printed to stdout. Existing concrete evidence is not overwritten unless
FLEET_GITHUB_PUBLICATION_EVIDENCE_FORCE=1 is set.

Supported field=value overrides:
  visibility-reviewed=yes
  repository-name-matches-namespace=yes
  issues-setting="enabled per support commitment"
  discussions-setting=disabled
  wiki-setting=disabled
  releases-setting="source tags and release notes only"
  packages-setting="not used for source-only alpha"
  github-actions-setting=enabled
  security-reporting-channel="GitHub Private Vulnerability Reporting enabled"
  secret-scanning=enabled
  dependabot-alerts=enabled
  default-branch-protection=enabled
  required-source-checks="CI source checks"
  required-release-checks="Release Readiness"
  linear-history-policy=preferred
  signed-commit-policy="not required"
  release-authority="single maintainer repository owner"
  tag-protection="owner-approved deferred: enable tag protection before first public tag if GitHub plan supports it"
  release-artifact-custody="source tags and release notes only"
  package-publishing-credentials="none for source-only alpha"
EOF
}

repo_url="${1:-}"
default_branch="${2:-}"
source_ref="${3:-}"
emergency_removal_owner="${4:-}"
output="${5:-docs/release/GITHUB_PUBLICATION_EVIDENCE.md}"

if [ -z "$repo_url" ] || [ -z "$default_branch" ] || [ -z "$source_ref" ] ||
  [ -z "$emergency_removal_owner" ] ||
  [ "$repo_url" = "-h" ] || [ "$repo_url" = "--help" ]; then
  usage
  exit 2
fi

shift_count=5
if [ "$#" -lt 5 ]; then
  shift_count="$#"
fi
shift "$shift_count"

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: GitHub publication evidence generation must run inside a git worktree" >&2
  exit 2
fi

visibility_reviewed="yes"
repository_name_matches_namespace="yes"
issues_setting="enabled per support commitment"
discussions_setting="disabled"
wiki_setting="disabled"
releases_setting="source tags and release notes only"
packages_setting="not used for source-only alpha"
github_actions_setting="enabled"
security_reporting_channel="GitHub Private Vulnerability Reporting enabled"
secret_scanning="enabled"
dependabot_alerts="enabled"
default_branch_protection="enabled"
required_source_checks="CI source checks"
required_release_checks="Release Readiness"
linear_history_policy="preferred"
signed_commit_policy="not required"
release_authority="single maintainer repository owner"
tag_protection="owner-approved deferred: enable tag protection before first public tag if GitHub plan supports it"
release_artifact_custody="source tags and release notes only"
package_publishing_credentials="none for source-only alpha"

set_override() {
  local key=$1
  local value=$2
  case "$key" in
    visibility-reviewed) visibility_reviewed="$value" ;;
    repository-name-matches-namespace) repository_name_matches_namespace="$value" ;;
    issues-setting) issues_setting="$value" ;;
    discussions-setting) discussions_setting="$value" ;;
    wiki-setting) wiki_setting="$value" ;;
    releases-setting) releases_setting="$value" ;;
    packages-setting) packages_setting="$value" ;;
    github-actions-setting) github_actions_setting="$value" ;;
    security-reporting-channel) security_reporting_channel="$value" ;;
    secret-scanning) secret_scanning="$value" ;;
    dependabot-alerts) dependabot_alerts="$value" ;;
    default-branch-protection) default_branch_protection="$value" ;;
    required-source-checks) required_source_checks="$value" ;;
    required-release-checks) required_release_checks="$value" ;;
    linear-history-policy) linear_history_policy="$value" ;;
    signed-commit-policy) signed_commit_policy="$value" ;;
    release-authority) release_authority="$value" ;;
    tag-protection) tag_protection="$value" ;;
    release-artifact-custody) release_artifact_custody="$value" ;;
    package-publishing-credentials) package_publishing_credentials="$value" ;;
    *)
      echo "FAIL: unsupported publication evidence field override: $key" >&2
      exit 2
      ;;
  esac
}

for override in "$@"; do
  case "$override" in
    *=*) set_override "${override%%=*}" "${override#*=}" ;;
    *)
      echo "FAIL: field override must use field=value syntax: $override" >&2
      exit 2
      ;;
  esac
done

if ! printf '%s\n' "$repo_url" | rg -q '^https://github\.com/[^/[:space:]]+/[^/[:space:]]+$'; then
  echo "FAIL: repository URL must be a concrete GitHub repo URL: $repo_url" >&2
  exit 1
fi

if ! printf '%s\n' "$default_branch" | rg -q '^[A-Za-z0-9._/-]+$'; then
  echo "FAIL: default branch name is not concrete: $default_branch" >&2
  exit 1
fi

reject_placeholder_value() {
  local label=$1
  local value=$2
  if [ -z "$value" ] || printf '%s\n' "$value" | rg -qi 'TODO|TBD|PLACEHOLDER|PENDING|not yet reviewed|not yet configured'; then
    echo "FAIL: $label must be concrete: $value" >&2
    exit 1
  fi
}

for pair in \
  "emergency removal owner:$emergency_removal_owner" \
  "visibility consequences reviewed:$visibility_reviewed" \
  "repository name matches namespace:$repository_name_matches_namespace" \
  "issues setting:$issues_setting" \
  "discussions setting:$discussions_setting" \
  "wiki setting:$wiki_setting" \
  "releases setting:$releases_setting" \
  "packages setting:$packages_setting" \
  "GitHub Actions setting:$github_actions_setting" \
  "security reporting channel:$security_reporting_channel" \
  "secret scanning:$secret_scanning" \
  "Dependabot alerts:$dependabot_alerts" \
  "default branch protection:$default_branch_protection" \
  "required source checks:$required_source_checks" \
  "required release checks:$required_release_checks" \
  "linear history policy:$linear_history_policy" \
  "signed commit policy:$signed_commit_policy" \
  "release authority:$release_authority" \
  "tag protection:$tag_protection" \
  "release artifact custody:$release_artifact_custody" \
  "package publishing credentials:$package_publishing_credentials"
do
  reject_placeholder_value "${pair%%:*}" "${pair#*:}"
done

if [ "$output" != "-" ] && [ -f "$output" ] &&
  ! rg -q 'GitHub publication evidence status: PENDING|TODO|TBD|PLACEHOLDER|not yet reviewed|not yet configured' "$output" &&
  [ "${FLEET_GITHUB_PUBLICATION_EVIDENCE_FORCE:-0}" != "1" ]; then
  echo "FAIL: GitHub publication evidence already looks concrete: $output" >&2
  echo "Set FLEET_GITHUB_PUBLICATION_EVIDENCE_FORCE=1 to overwrite reviewed evidence." >&2
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
# GitHub Publication Evidence

GitHub publication evidence status: PASS

This file records the GitHub repository settings review for the exact commit
that will become the first public GitHub alpha. Do not mark the owner decision
record \`APPROVED\` until this file is concrete and
\`scripts/check-github-publication-evidence.sh\` passes.

This is release-control evidence. It may be updated after the reviewed commit is
selected; the verifier compares the reviewed commit to the release-prep commit
while allowing only the known release-control evidence files under
\`docs/release/*_EVIDENCE.md\` to differ.

Commit: \`$source_commit\`
Release-control evidence file: \`${release_control_path:-not tracked in this worktree}\`
Repository: \`$repo_url\`
Default branch: \`$default_branch\`

## Visibility And Repository Settings

Visibility consequences reviewed: \`$visibility_reviewed\`
Repository name matches namespace: \`$repository_name_matches_namespace\`
Issues setting: \`$issues_setting\`
Discussions setting: \`$discussions_setting\`
Wiki setting: \`$wiki_setting\`
Releases setting: \`$releases_setting\`
Packages setting: \`$packages_setting\`
GitHub Actions setting: \`$github_actions_setting\`

## Security Settings

Security reporting channel available: \`$security_reporting_channel\`
Secret scanning or accepted unavailable reason: \`$secret_scanning\`
Dependabot alerts or accepted unavailable reason: \`$dependabot_alerts\`

## Branch Protection

Default branch protection: \`$default_branch_protection\`
Required source checks: \`$required_source_checks\`
Required release checks: \`$required_release_checks\`
Linear history policy: \`$linear_history_policy\`
Signed commit policy: \`$signed_commit_policy\`

## Release Custody

Release authority: \`$release_authority\`
Tag protection or accepted unavailable reason: \`$tag_protection\`
Release artifact custody: \`$release_artifact_custody\`
Package publishing credentials: \`$package_publishing_credentials\`
Emergency removal owner: \`$emergency_removal_owner\`
EOF
)"

if [ "$output" = "-" ]; then
  printf '%s\n' "$evidence"
else
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$evidence" >"$output"
  echo "Wrote GitHub publication evidence: $output"
fi

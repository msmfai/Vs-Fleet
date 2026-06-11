#!/usr/bin/env bash
set -euo pipefail

file="${1:-}"
expected_commit="${2:-}"

if [ -z "$file" ]; then
  echo "usage: $0 <release-notes.md> [expected-commit]"
  exit 2
fi

if [ ! -f "$file" ]; then
  echo "FAIL: missing release notes file: $file"
  exit 1
fi

fail=0

for heading in \
  "## Release" \
  "## Alpha Scope" \
  "## What Changed" \
  "## Verification" \
  "## Dependency And License Review" \
  "## Security And Privacy Notes" \
  "## Known Rough Edges" \
  "## Upgrade And Rollback"
do
  if ! rg -q "^${heading}$" "$file"; then
    echo "FAIL: release notes missing required section: $heading"
    fail=1
  fi
done

if rg -n '`?\[[^]]*(TODO|YYYY|full commit SHA|one-line change|commands/results|workflow URL|chosen license|known alpha limitation|accepted exception|owner-approved|private contact|approved binary scope)[^]]*\]`?' "$file"; then
  echo "FAIL: release notes still contain bracketed template placeholders"
  fail=1
fi

if rg -n '\[(source-only|none|passed) \|' "$file"; then
  echo "FAIL: release notes still contain unresolved choice lists"
  fail=1
fi

if rg -n '\[.*\|.*\]' "$file"; then
  echo "FAIL: release notes still contain unresolved bracketed alternatives"
  fail=1
fi

if rg -n 'accepted exception|owner-approved skip|owner-approved current history exposure' "$file"; then
  echo "FAIL: release notes mention accepted exceptions; replace with the exact approved decision and evidence"
  fail=1
fi

if ! rg -q '^- Secret exposure audit:' "$file"; then
  echo "FAIL: release notes must record the secret exposure audit result"
  fail=1
fi

if ! rg -q '^- Documentation link audit:' "$file"; then
  echo "FAIL: release notes must record the documentation link audit result"
  fail=1
fi

if ! rg -q '^- Public tree size audit:' "$file"; then
  echo "FAIL: release notes must record the public tree size audit result"
  fail=1
fi

field_value() {
  local label=$1
  local line
  line="$(rg "^- ${label}:" "$file" | head -n1 || true)"
  if [ -z "$line" ]; then
    return 1
  fi
  local value="${line#*:}"
  value="$(printf '%s' "$value" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//')"
  printf '%s\n' "$value"
}

require_value() {
  local label=$1
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: release notes missing ${label} field" >&2
    fail=1
    return 1
  fi
  if [ -z "$value" ]; then
    echo "FAIL: release notes ${label} field is empty" >&2
    fail=1
    return 1
  fi
  printf '%s\n' "$value"
}

version="$(require_value "Version" || true)"
if [ -n "${version:-}" ] && ! printf '%s\n' "$version" | rg -q '^v[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+$'; then
  echo "FAIL: release notes Version must look like v0.1.0-alpha.1"
  fail=1
fi

commit="$(require_value "Commit" || true)"
if [ -n "${commit:-}" ]; then
  if ! printf '%s\n' "$commit" | rg -q '^[0-9a-f]{40}$'; then
    echo "FAIL: release notes Commit must be a full 40-character lowercase git SHA"
    fail=1
  elif [ -n "$expected_commit" ] && [ "$commit" != "$expected_commit" ]; then
    echo "FAIL: release notes Commit $commit does not match expected commit $expected_commit"
    fail=1
  fi
fi

date="$(require_value "Date" || true)"
if [ -n "${date:-}" ] && ! printf '%s\n' "$date" | rg -q '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'; then
  echo "FAIL: release notes Date must use YYYY-MM-DD"
  fail=1
fi

distribution="$(require_value "Distribution" || true)"
if [ -n "${distribution:-}" ] && printf '%s\n' "$distribution" | rg -q '\[|\]|\|'; then
  echo "FAIL: release notes Distribution must be a concrete value"
  fail=1
fi

branding="$(require_value "Branding" || true)"
if [ -n "${branding:-}" ] && printf '%s\n' "$branding" | rg -q '\[|\]|\|'; then
  echo "FAIL: release notes Branding must be a concrete value"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Release notes check passed."

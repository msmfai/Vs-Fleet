#!/usr/bin/env bash
set -euo pipefail

file="${1:-}"

if [ -z "$file" ]; then
  echo "usage: $0 <release-notes.md>"
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

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Release notes check passed."

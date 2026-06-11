#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/docs/reference"

cat >"$repo/README.md" <<'EOF'
# Link Fixture

[Quickstart](docs/QUICKSTART.md)
[Reference](/docs/reference/REFERENCE.md)
[External](https://example.invalid/path)
[Section](#local-section)

```md
[Ignored missing fixture](docs/MISSING_FROM_CODE_BLOCK.md)
```
EOF

cat >"$repo/docs/QUICKSTART.md" <<'EOF'
# Quickstart

[Root](../README.md)
EOF

cat >"$repo/docs/reference/REFERENCE.md" <<'EOF'
# Reference

[Quickstart](../QUICKSTART.md)
EOF

expect_pass() {
  local label=$1
  shift
  if ! (cd "$repo" && "$ROOT/scripts/check-doc-links.sh" "$@") >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  shift
  if (cd "$repo" && "$ROOT/scripts/check-doc-links.sh" "$@") >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_pass "valid relative, root-relative, external, and anchor links" \
  README.md docs/QUICKSTART.md docs/reference/REFERENCE.md

cat >>"$repo/README.md" <<'EOF'

[Missing](docs/NOPE.md)
EOF

expect_fail "broken local links are rejected" README.md

echo "Documentation link check tests passed."

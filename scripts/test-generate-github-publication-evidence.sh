#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"
cp "$ROOT/scripts/generate-github-publication-evidence.sh" "$repo/scripts/generate-github-publication-evidence.sh"
cp "$ROOT/scripts/check-github-publication-evidence.sh" "$repo/scripts/check-github-publication-evidence.sh"
cp "$ROOT/scripts/check-release-custody-decision.sh" "$repo/scripts/check-release-custody-decision.sh"
chmod +x "$repo/scripts/"*.sh

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

cat >"$repo/docs/release/OWNER_DECISION_RECORD.md" <<'EOF'
# Owner Decision Record

Decision record status: APPROVED

## Required Before Public GitHub Visibility

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | smfmarin |
| GitHub repo name | vs-fleet |
| Product name | Fleet |
| Rust crate prefix | fleet-* |
| npm package names | fleet-extension, fleet-bridge |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

### 4. Alpha Scope

### 16. Release Custody And Maintainer Authority

- [x] Single-maintainer alpha. Only the repository owner or named release owner may push source tags or create GitHub releases.
- [ ] Multi-maintainer governance before public alpha.
- [ ] Other: `TODO`

### 17. AI-Assisted Contribution Provenance
EOF

cat >"$repo/docs/release/GITHUB_PUBLICATION_RUNBOOK.md" <<'EOF'
# GitHub Publication Runbook

## Release Custody

Only the approved release authority may push source tags or create GitHub releases.
EOF

cat >"$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md" <<'EOF'
# GitHub Publication Evidence

GitHub publication evidence status: PENDING
Commit: `TODO`
Repository: `TODO`
Default branch: `TODO`
EOF

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add .
git -C "$repo" commit -q -m "reviewed publication source"
source_commit="$(git -C "$repo" rev-parse HEAD)"

evidence="$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md"
repo_url="https://github.com/smfmarin/vs-fleet"

if ! (cd "$repo" && ./scripts/generate-github-publication-evidence.sh \
  "$repo_url" public-alpha HEAD smfmarin "$evidence") >"$TMPDIR/generate.out" 2>&1; then
  echo "FAIL: expected GitHub publication evidence generation to pass" >&2
  cat "$TMPDIR/generate.out" >&2
  exit 1
fi

for pattern in \
  '^GitHub publication evidence status: PASS$' \
  "^Commit: \`$source_commit\`$" \
  '^Release-control evidence file: `docs/release/GITHUB_PUBLICATION_EVIDENCE.md`$' \
  "^Repository: \`$repo_url\`$" \
  '^Default branch: `public-alpha`$' \
  '^Visibility consequences reviewed: `yes`$' \
  '^Repository name matches namespace: `yes`$' \
  '^Issues setting: `enabled per support commitment`$' \
  '^Discussions setting: `disabled`$' \
  '^Wiki setting: `disabled`$' \
  '^Releases setting: `source tags and release notes only`$' \
  '^Packages setting: `not used for source-only alpha`$' \
  '^GitHub Actions setting: `enabled`$' \
  '^Security reporting channel available: `GitHub Private Vulnerability Reporting enabled`$' \
  '^Secret scanning or accepted unavailable reason: `enabled`$' \
  '^Dependabot alerts or accepted unavailable reason: `enabled`$' \
  '^Default branch protection: `enabled`$' \
  '^Required source checks: `CI source checks`$' \
  '^Required release checks: `Release Readiness`$' \
  '^Linear history policy: `preferred`$' \
  '^Signed commit policy: `not required`$' \
  '^Release authority: `single maintainer repository owner`$' \
  '^Release artifact custody: `source tags and release notes only`$' \
  '^Package publishing credentials: `none for source-only alpha`$' \
  '^Emergency removal owner: `smfmarin`$'
do
  if ! rg -q "$pattern" "$evidence"; then
    echo "FAIL: generated GitHub publication evidence missing pattern: $pattern" >&2
    cat "$evidence" >&2
    exit 1
  fi
done

if ! (cd "$repo" && ./scripts/check-github-publication-evidence.sh \
  docs/release/OWNER_DECISION_RECORD.md "$evidence" "$source_commit") >"$TMPDIR/check-publication.out" 2>&1; then
  echo "FAIL: generated GitHub publication evidence should pass publication checker" >&2
  cat "$TMPDIR/check-publication.out" >&2
  exit 1
fi

if ! (cd "$repo" && ./scripts/check-release-custody-decision.sh \
  docs/release/OWNER_DECISION_RECORD.md "$evidence" .) >"$TMPDIR/check-custody.out" 2>&1; then
  echo "FAIL: generated GitHub publication evidence should pass release custody checker" >&2
  cat "$TMPDIR/check-custody.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-github-publication-evidence.sh \
  "$repo_url" public-alpha HEAD smfmarin "$evidence") >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: concrete GitHub publication evidence should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! rg -q 'FLEET_GITHUB_PUBLICATION_EVIDENCE_FORCE=1' "$TMPDIR/overwrite.out"; then
  echo "FAIL: overwrite rejection should explain the force override" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! (cd "$repo" && FLEET_GITHUB_PUBLICATION_EVIDENCE_FORCE=1 \
  ./scripts/generate-github-publication-evidence.sh "$repo_url" public-alpha HEAD smfmarin "$evidence" \
  default-branch-protection="owner-approved deferred: branch protection will be enabled immediately after visibility flip") >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced GitHub publication evidence overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

git -C "$repo" add docs/release/GITHUB_PUBLICATION_EVIDENCE.md
git -C "$repo" commit -q -m "record GitHub publication evidence"
evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && ./scripts/check-github-publication-evidence.sh \
  docs/release/OWNER_DECISION_RECORD.md "$evidence" "$evidence_commit") >"$TMPDIR/check-evidence-commit.out" 2>&1; then
  echo "FAIL: publication evidence commit should pass when only the publication evidence file differs" >&2
  cat "$TMPDIR/check-evidence-commit.out" >&2
  exit 1
fi

printf 'unexpected publication payload drift\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "drift publication payload"
drift_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && ./scripts/check-github-publication-evidence.sh \
  docs/release/OWNER_DECISION_RECORD.md "$evidence" "$drift_commit") >"$TMPDIR/check-drift.out" 2>&1; then
  echo "FAIL: publication evidence check should reject payload drift outside the evidence file" >&2
  cat "$TMPDIR/check-drift.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-github-publication-evidence.sh \
  "https://gitlab.com/smfmarin/vs-fleet" public-alpha HEAD smfmarin -) >"$TMPDIR/bad-repo.out" 2>&1; then
  echo "FAIL: non-GitHub publication repository URL should be rejected" >&2
  cat "$TMPDIR/bad-repo.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-github-publication-evidence.sh \
  "$repo_url" public-alpha HEAD TODO -) >"$TMPDIR/placeholder.out" 2>&1; then
  echo "FAIL: placeholder emergency removal owner should be rejected" >&2
  cat "$TMPDIR/placeholder.out" >&2
  exit 1
fi

echo "GitHub publication evidence generator tests passed."

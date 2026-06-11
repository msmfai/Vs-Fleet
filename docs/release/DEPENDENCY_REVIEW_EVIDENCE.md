# Dependency Review Evidence

Dependency review status: PASS

This file records dependency review evidence for the exact commit that will
become the first public GitHub alpha. Do not mark the owner decision record
`APPROVED` until this file is concrete and
`scripts/check-dependency-review-decision.sh` passes.

Commit: `6273c350eba357a5170e97eac088be057983f299`
Reviewed date: `2026-06-11`
Release-control evidence file: `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md`

## Command Evidence

Use this section if the owner decision record chooses to run the dependency
review commands.

cargo tree: `pass`
cargo metadata --locked: `pass`
fleet-host cargo metadata --locked: `pass`
lockfile policy: `pass`
fleet-bridge npm audit: `pass`
extension npm audit: `pass`
generated artifact check: `pass`
Accepted findings: `none`

## Skipped Review Evidence

Use this section only if the owner explicitly accepts publishing the first
source alpha without dependency review.

Accepted risk: `not used`

## Other Evidence

Use this section only if the owner records a concrete `Other` dependency review
decision.

Dependency review evidence path: `not used`

# Public Branch Evidence

Public branch evidence status: PASS

This file records the clean-history branch evidence for the first public GitHub
alpha. Use it when the owner decision record chooses a cleaned/squashed first
public branch. Do not mark the owner decision record `APPROVED` until this file
is concrete and `scripts/check-public-branch-evidence.sh` passes.

Source commit: `e090b2e87292672201d679d88ec6a4da3cb93e21`
Public branch: `public-alpha`
Public root commit: `b1a16d420c0bbbefd9982106c5153a5613d7d3f9`
Release-control evidence file: `docs/release/PUBLIC_BRANCH_EVIDENCE.md`
History check command: `./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha`
History check result: `PASS`

## Required Facts

Single root commit: `yes`
Public tree matches source commit tree: `yes`
Public branch contains no prior private history: `yes`

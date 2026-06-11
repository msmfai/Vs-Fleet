# Public Branch Evidence

Public branch evidence status: PASS

This file records the clean-history branch evidence for the first public GitHub
alpha. Use it when the owner decision record chooses a cleaned/squashed first
public branch. Do not mark the owner decision record `APPROVED` until this file
is concrete and `scripts/check-public-branch-evidence.sh` passes.

Source commit: `07fb91d4afb1c69350209c72f2342699712d8d7c`
Public branch: `public-alpha`
Public root commit: `9c08bb3f5c6790a4793b9de867c6f5e235694ef2`
Release-control evidence file: `docs/release/PUBLIC_BRANCH_EVIDENCE.md`
History check command: `./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha`
History check result: `PASS`

## Required Facts

Single root commit: `yes`
Public tree matches source commit tree: `yes`
Public branch contains no prior private history: `yes`

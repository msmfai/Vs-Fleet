# Public Branch Evidence

Public branch evidence status: PASS

This file records the clean-history branch evidence for the first public GitHub
alpha. Use it when the owner decision record chooses a cleaned/squashed first
public branch. Do not mark the owner decision record `APPROVED` until this file
is concrete and `scripts/check-public-branch-evidence.sh` passes.

Source commit: `fdf79ff487b8225bd5577e79f60c554bfdd1964b`
Public branch: `public-alpha`
Public root commit: `da9394b7ffde2651f9fa84c65d04e9a22a05d7bd`
Release-control evidence file: `docs/release/PUBLIC_BRANCH_EVIDENCE.md`
History check command: `./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha`
History check result: `PASS`

## Required Facts

Single root commit: `yes`
Public tree matches source commit tree: `yes`
Public branch contains no prior private history: `yes`

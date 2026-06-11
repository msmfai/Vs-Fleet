# Security Policy

Fleet is experimental software. It runs local services, observes editor and
agent state, and writes logs that can contain workspace paths, local URLs,
session labels, and command-line metadata.

## Supported versions

Only the current default development branch is considered for security fixes.
There are no stable release lines yet.

## Reporting a vulnerability

GitHub Private Vulnerability Reporting is enabled for this repository. Do not
publish exploit details in public issues.

## Current Security Posture

- Fleet is designed as local-first software.
- Local-only development services should bind to loopback unless explicitly
  configured otherwise.
- Runtime files live under `~/.fleet/run` and `~/.fleet/mux` unless
  `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` is set.
- Non-loopback transport, remote deployment, public binaries, and auto-update
  flows are not considered production-ready security surfaces yet.
- Logs and screenshots should be scrubbed before sharing publicly.
- Manual cleanup is documented in `docs/LOCAL_DATA_AND_UNINSTALL.md`.

## Disclosure expectations

This project does not currently offer paid support, bug bounties, or response
SLAs. Reports will be handled best-effort.

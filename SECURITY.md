# Security Policy

Fleet is pre-release alpha software. It runs local services, observes editor and
agent state, and writes logs that can contain workspace paths, local URLs,
session labels, and command-line metadata.

## Supported versions

Only the current default development branch is considered for security fixes
during alpha. There are no stable release lines yet.

## Reporting a vulnerability

Use GitHub Private Vulnerability Reporting for this repository once it is
enabled. If private reporting is not enabled yet, do not publish a public issue
with exploit details; contact the maintainer out of band and ask for a private
reporting channel first.

## Current alpha security posture

- Fleet is designed as local-first software.
- Local-only development services should bind to loopback unless explicitly
  configured otherwise.
- Non-loopback transport, remote deployment, public binaries, and auto-update
  flows are not considered production-ready security surfaces yet.
- Logs and review artifacts should be scrubbed before sharing publicly.

## Disclosure expectations

This project does not currently offer paid support, bug bounties, or response
SLAs. Reports will be handled best-effort during alpha.

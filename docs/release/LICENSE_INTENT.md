# License Intent

This note records the intended source-alpha licensing posture. It is governance
context, not a substitute for the actual `LICENSE` file and manifest metadata.

## Current Decision

Fleet should ship the source alpha as `MIT OR Apache-2.0` once the reviewed
license text is added and all Rust/npm manifests and lockfiles are updated.
Until that happens, `UNLICENSED` metadata remains a public-release blocker.

The permissive license is the default for Fleet's reusable library and protocol
surfaces. The goal is to let people inspect, fork, package, and embed the local
developer-tool pieces without forcing an early business-model decision.

## Contribution Posture

Outside code contributions require Developer Certificate of Origin (DCO)
sign-off. A DCO keeps provenance clear and low-friction, but it does not assign
copyright and it does not give the maintainer unilateral relicensing rights over
contributor-owned code.

No Contributor License Agreement (CLA) is required for source alpha. Revisit
that before accepting contributor code if the project later needs paid
commercial exceptions, proprietary relicensing, or other rights that a DCO does
not provide.

## Forward Relicensing

The current maintainer can relicense future maintainer-owned versions, but
released versions remain available under the license that shipped with those
versions. Do not rely on a future license change to revoke rights already
granted on a public release.

Keep the reusable library/API crates permissive by default. Treat
`AGPL-3.0-only` plus a commercial exception as a contingency only for the CLI,
a future hosted control plane, or another non-library service component, and
only after a concrete monetization or hosted-reseller trigger exists.

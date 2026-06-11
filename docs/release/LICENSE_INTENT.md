# License Intent

This note records the intended source-alpha licensing posture. It is governance
context, not a substitute for the actual `LICENSE` file and manifest metadata.

## Current Decision

Fleet should ship the source alpha as `MIT OR Apache-2.0`. This is the alpha
decision, not a placeholder for an imminent copyleft switch. The reviewed
license text is tracked in root `LICENSE` with full texts under `docs/legal/`, and
the Rust/npm manifests and lockfiles use the same SPDX expression.

The permissive license is the default for Fleet's reusable library and protocol
surfaces. The goal is to let people inspect, fork, package, and embed the local
developer-tool pieces without forcing an early business-model decision.

Keep those reusable library/API crates permissive by default. A later copyleft
or commercial-exception path should not tax the library surfaces Fleet wants
other developer tools to embed.

## Monetization Caveat

Do not treat copyleft as a monetization plan for the source alpha. Fleet's
initial user is expected to run a local developer tool internally, and internal
use of a local tool does not create the distribution or hosted-service pressure
that GPL-family license strategies rely on. A paid model should come from a
concrete product surface, such as support, enterprise features, hosted
coordination, or commercial terms for a real proprietary embedder.

The practical triggers are narrow:

- A proprietary embedder who wants commercial terms for a shipped product can be
  handled with support or a negotiated commercial license while the public
  library crates stay permissive.
- A real hosted reseller or hosted control-plane business can justify revisiting
  `AGPL-3.0-only` plus a commercial exception for future CLI, host, or service
  components.
- A vague hope that copyleft will create revenue is not enough reason to change
  the alpha license.

## Contribution Posture

Outside code contributions require Developer Certificate of Origin (DCO)
sign-off. A DCO keeps provenance clear and low-friction, but it does not assign
copyright and it does not give the maintainer unilateral relicensing rights over
contributor-owned code.

No Contributor License Agreement (CLA) is required for source alpha. Revisit
that before accepting contributor code if the project later needs paid
commercial exceptions, proprietary relicensing, or other rights that a DCO does
not provide. If that becomes a real goal, decide the CLA posture before taking
the relevant outside PRs rather than trying to re-paper contributors later.

## Forward Relicensing

The current maintainer can relicense future maintainer-owned versions, but
released versions remain available under the license that shipped with those
versions. Do not rely on a future license change to revoke rights already
granted on a public release.

Keep the reusable library/API crates permissive by default. Treat
`AGPL-3.0-only` plus a commercial exception as a contingency only for the CLI,
a future hosted control plane, or another non-library service component, and
only after a concrete monetization or hosted-reseller trigger exists.

Before any future relicense, confirm that the maintainer owns or has the needed
license-back rights for all affected code, leave already released versions under
their published license, and publish the paid exception path at the same time as
any copyleft release.

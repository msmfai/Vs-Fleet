//! Fleet Reporter library.
//!
//! This module contains two things:
//!
//! 1. **[`transition`]** — a pure, sync, zero-async scripted-transition
//!    generator that produces the expected sequence of [`ClientMessage`] deltas
//!    for a fake `working → waiting(approval) → working → dead` lifecycle.
//!    This is the test fixture for §4.3 two-face consistency (PLAN S4) and is
//!    heavily unit-tested so the fake's behavior is deterministic and provable.
//!
//! 2. **[`fake`]** — the async outbound connector that opens a WS (or
//!    `cfg(unix)` unix-socket) connection to the Hub and drives the scripted
//!    transition sequence with configurable inter-step delays.
//!
//! # Design choices honored here
//! - **D6 — JSON wire format**: all deltas are [`fleet_hub::wire::ClientMessage`]
//!   serialized as JSON text frames.
//! - **D7 — WebSocket everywhere + unix fast path on `cfg(unix)`**: the
//!   `FakeReporter` accepts either a WS URL (`ws://…`) or, on unix, a socket
//!   path and connects accordingly.
//! - **D9 — `done` kept distinct from `idle`**: the transition sequence ends
//!   in `Dead`, never conflating it with `Idle` or `Done`.
//! - **Invariant 5 — confidence honesty**: the fake always reports
//!   `confidence: high` for the `waiting` step (it's a scripted authoritative
//!   signal, not a heuristic inference).

#![forbid(unsafe_code)]

pub mod fake;
pub mod transition;

// Public surface used by tests and the binary.
pub use fake::{FakeReporter, FakeReporterConfig, Transport};
pub use transition::{ScriptedStep, TransitionScript};

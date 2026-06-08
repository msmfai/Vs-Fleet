//! Fleet Reporter library.
//!
//! ## The real reporter framework (S5 / node REPCORE)
//!
//! The core of this crate is the **real reporter framework**: an outbound
//! connection to the Hub that **registers** a session, **assigns fleet
//! run-ids**, sends **heartbeats**, **buffers deltas while disconnected**, and
//! **reconnects with backoff**, reconciling on reconnect rather than declaring a
//! run `dead` prematurely. It is composed of small, individually-tested pieces:
//!
//! - **[`backoff`]** — a pure, deterministic exponential-backoff policy.
//! - **[`buffer`]** — a bounded, FIFO, monotonically-`seq`-stamped outbound
//!   delta buffer (the basis for the S6 `(durable_id, seq)` invariants).
//! - **[`liveness`]** — the "dead-decision" state machine: a run is `dead`
//!   **only** on a confirmed exit or a heartbeat timeout, never on a dropped Hub
//!   link.
//! - **[`transport`]** — the [`transport::Connector`]/[`transport::Connection`]
//!   seam, with WebSocket (always) and `cfg(unix)` unix-socket fast-path
//!   implementations (D7), plus a deterministic in-memory transport
//!   ([`testkit`]) for tests.
//! - **[`reporter`]** — the [`reporter::Reporter`] driver tying it together,
//!   with a pure [`reporter::ReporterCore`] for exhaustive unit testing.
//!
//! ## The fake reporter (S4)
//!
//! - **[`transition`]** — a pure, sync scripted-transition generator producing
//!   the `working → waiting(approval) → working → dead` lifecycle used by the
//!   §4.3 two-face-consistency fixtures.
//! - **[`fake`]** — the async driver that plays the scripted sequence over a real
//!   WS (or `cfg(unix)` unix-socket) connection.
//!
//! # Design choices honored here
//! - **D4 — custom durable identity, no broker**: the reporter assigns its own
//!   run-ids and registers by durable session id.
//! - **D6 — JSON wire format**: all deltas are [`fleet_hub::wire::ClientMessage`]
//!   serialized as JSON text frames.
//! - **D7 — WebSocket everywhere + unix fast path on `cfg(unix)`**.
//! - **D9 — `done` kept distinct from `idle`**.
//! - **Invariant 5 — confidence honesty**: the framework forwards the caller's
//!   confidence verbatim and never upgrades `inferred` to `high`.

#![forbid(unsafe_code)]

pub mod backoff;
pub mod buffer;
pub mod fake;
pub mod identity;
pub mod liveness;
pub mod reporter;
pub mod testkit;
pub mod transition;
pub mod transport;

// Public surface used by tests and the binary.
pub use backoff::Backoff;
pub use buffer::{Delta, DeltaBuffer};
pub use fake::{FakeReporter, FakeReporterConfig, Transport};
pub use identity::{DurableId, IdentityLedger, RunIdentity, Stamp};
pub use liveness::{Liveness, LivenessTracker};
pub use reporter::{Reporter, ReporterCommand, ReporterConfig, ReporterCore, ReporterHandle};
pub use transition::{ScriptedStep, TransitionScript};
pub use transport::{Connection, Connector, TransportError, WsConnector};

#[cfg(unix)]
pub use transport::UnixConnector;

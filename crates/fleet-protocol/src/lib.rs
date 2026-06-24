//! Fleet wire protocol — the product (engineering spec §4.3, §7).
//!
//! This crate is the single source of truth for everything that crosses the
//! Hub↔reporter↔face boundary: the [`Session`]/[`AgentRun`] objects, the
//! [`State`]/[`Urgency`]/[`Confidence`] enums, the Hub→face [`Event`]s, and the
//! face→Hub [`Command`]s. It is transport-agnostic (WebSocket or unix socket)
//! and JSON-encoded.
//!
//! Locked design choices honored here:
//! - **D6 — JSON wire format**: every type is `serde` JSON round-trippable.
//! - **D9 — `done` kept distinct from `idle`**: [`State::Done`] is its own
//!   variant with its own wire token.
//! - **§7.6 versioning**: every object/envelope carries `schema_version`
//!   ([`SCHEMA_VERSION`]); **faces tolerate unknown fields** — the structs do
//!   not `deny_unknown_fields` and capture extras in a `flatten`ed map.
//! - **Invariant 5 — confidence honesty**: [`Confidence`] is a required field
//!   on every run.
//! - **D13 — licensing-clean / separable**: this crate has no Hub/reporter
//!   dependency and can be split out for OSS later.
//!
//! ## Examples
//!
//! Round-trip a snapshot event:
//! ```
//! use fleet_protocol::{Event, SCHEMA_VERSION};
//! let e = Event::snapshot(vec![]);
//! let json = serde_json::to_string(&e).unwrap();
//! let back: Event = serde_json::from_str(&json).unwrap();
//! assert_eq!(e, back);
//! assert!(json.contains("fleet.snapshot"));
//! let _ = SCHEMA_VERSION;
//! ```
//!
//! Build a command:
//! ```
//! use fleet_protocol::{Command, Target};
//! let c = Command::focus(Target::run("run-123"));
//! assert_eq!(c.command_name(), "focus");
//! ```

#![forbid(unsafe_code)]
// Enables `#[coverage(off)]` under cargo-llvm-cov's nightly run (which sets
// cfg(coverage_nightly)). A no-op on stable.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod commands;
pub mod events;
pub mod objects;
pub mod paths;
pub mod rollup;
pub mod state;

#[cfg(feature = "schema")]
pub mod schema;

/// The on-wire schema version. Carried on every object and envelope (§7.6).
/// Bump on any breaking change; the Hub advertises supported versions on
/// connect.
pub const SCHEMA_VERSION: u32 = 1;

// Compile-time assertion: schema version must be nonzero.
const _: () = assert!(SCHEMA_VERSION > 0);

// ---- Flat re-exports so consumers can `use fleet_protocol::Session;` etc. ----
pub use commands::{Command, Target};
pub use events::Event;
pub use objects::{
    AgentKind, AgentRun, Editor, EditorKind, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session,
};
pub use paths::{default_reporter_socket, REPORTER_SOCKET_ENV};
pub use state::{Confidence, State, Urgency};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_is_one() {
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn reexports_are_constructible() {
        let loc = Location {
            kind: LocationKind::Local,
            label: "l".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        };
        let srv = Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        };
        let mut s = Session::new("s", "t", loc, srv, State::Idle, "2026-06-08T00:00:00Z");
        s.runs.push(AgentRun::new(
            "r",
            AgentKind::ClaudeCode,
            "sid",
            "/",
            State::Working,
            Confidence::Inferred,
            "2026-06-08T00:00:00Z",
        ));
        let ev = Event::session_added(s);
        let cmd = Command::mute("s");
        // Both serialize without panicking.
        serde_json::to_string(&ev).unwrap();
        serde_json::to_string(&cmd).unwrap();
    }
}

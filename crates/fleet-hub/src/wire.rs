//! Hub-internal inbound wire messages (Hub←client).
//!
//! The frozen [`fleet_protocol`] crate (G0) defines the Hub→face direction —
//! [`fleet_protocol::Event`] — and the face→Hub [`fleet_protocol::Command`]s.
//! It does **not** yet define the *inbound envelope* a client uses to open a
//! subscription or the *reporter deltas* that mutate Hub state, because those
//! are still being shaped by the REPCORE/IDENTITY nodes (S5/S6). Until that
//! contract is frozen upstream, the Hub owns this minimal inbound vocabulary
//! locally so S2 can stand up end-to-end.
//!
//! Design choices (consistent with the protocol crate, PLAN D6/§7.6):
//! - **JSON, internally tagged** on a `type` discriminator — human-debuggable.
//! - **`schema_version` tolerated, never required** on inbound: an older client
//!   may omit it; a newer one may add fields. We do not `deny_unknown_fields`.
//! - Reporter deltas carry **whole objects** for upserts (full-object deltas are
//!   acceptable per README §7.4) and **ids only** for removals — mirroring the
//!   outbound [`fleet_protocol::Event`] shape so the two stay symmetric.

use fleet_protocol::{AgentRun, Command, Session};
use serde::{Deserialize, Serialize};

/// A message a connected client sends to the Hub.
///
/// Two client roles share this envelope:
/// - **Faces** send [`ClientMessage::Subscribe`] (then receive a snapshot and a
///   live delta stream) and [`ClientMessage::Command`] (mute/solo/focus/…).
/// - **Reporters** send the `*Upsert` / `*Remove` deltas that mutate Hub state.
///
/// Internally tagged on `type`. Unknown `type` values are a hard deserialize
/// error by design — the Hub never silently drops an instruction it cannot
/// interpret (the same stance the protocol crate takes on unknown event types).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Open a subscription. The Hub replies with a `fleet.snapshot` and then
    /// streams deltas. Carries no payload in S2 (no filters yet).
    #[serde(rename = "subscribe")]
    Subscribe,

    /// A face→Hub command (mute/solo/focus/dismiss/set_tags). Flattened so the
    /// command's own `command` discriminator and fields sit alongside `type`.
    #[serde(rename = "command")]
    Command {
        #[serde(flatten)]
        command: Command,
    },

    /// Reporter delta: a session was added or changed. Full object (§7.4).
    #[serde(rename = "session.upsert")]
    SessionUpsert { session: Session },

    /// Reporter delta: a session was removed.
    #[serde(rename = "session.remove")]
    SessionRemove { session_id: String },

    /// Reporter delta: a run within a session was added or changed.
    #[serde(rename = "run.upsert")]
    RunUpsert { session_id: String, run: AgentRun },

    /// Reporter delta: a run was removed from a session.
    #[serde(rename = "run.remove")]
    RunRemove { session_id: String, run_id: String },
}

impl ClientMessage {
    /// The wire `type` discriminator of this message.
    pub fn type_name(&self) -> &'static str {
        match self {
            ClientMessage::Subscribe => "subscribe",
            ClientMessage::Command { .. } => "command",
            ClientMessage::SessionUpsert { .. } => "session.upsert",
            ClientMessage::SessionRemove { .. } => "session.remove",
            ClientMessage::RunUpsert { .. } => "run.upsert",
            ClientMessage::RunRemove { .. } => "run.remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, Confidence, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind,
        State,
    };

    fn sample_session(id: &str) -> Session {
        Session::new(
            id,
            "t",
            Location {
                kind: LocationKind::Local,
                label: "l".into(),
                glyph: LocationGlyph::Laptop,
                attach_hint: None,
                extra: Extra::new(),
            },
            Server {
                kind: ServerKind::Local,
                version: None,
                extra: Extra::new(),
            },
            State::Idle,
            "2026-06-08T00:00:00Z",
        )
    }

    fn sample_run(id: &str) -> AgentRun {
        AgentRun::new(
            id,
            AgentKind::Codex,
            "native",
            "/",
            State::Working,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        )
    }

    #[test]
    fn subscribe_wire_shape() {
        let m = ClientMessage::Subscribe;
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["type"], "subscribe");
        let back: ClientMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn command_is_flattened() {
        let m = ClientMessage::Command {
            command: Command::mute("s1"),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["type"], "command");
        // The inner command discriminator + fields sit alongside `type`.
        assert_eq!(v["command"], "mute");
        assert_eq!(v["session_id"], "s1");
        let back: ClientMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn session_and_run_deltas_round_trip() {
        for m in [
            ClientMessage::SessionUpsert {
                session: sample_session("s1"),
            },
            ClientMessage::SessionRemove {
                session_id: "s1".into(),
            },
            ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run: sample_run("r1"),
            },
            ClientMessage::RunRemove {
                session_id: "s1".into(),
                run_id: "r1".into(),
            },
        ] {
            let v = serde_json::to_value(&m).unwrap();
            assert_eq!(v["type"], m.type_name());
            let back: ClientMessage = serde_json::from_value(v).unwrap();
            assert_eq!(back, m);
        }
    }

    #[test]
    fn unknown_type_is_rejected() {
        let raw = serde_json::json!({ "type": "totally.unknown", "x": 1 });
        let r: Result<ClientMessage, _> = serde_json::from_value(raw);
        assert!(r.is_err(), "unknown message types must not silently parse");
    }

    #[test]
    fn unknown_fields_on_known_type_tolerated() {
        // A newer client adds an envelope field; the Hub still parses.
        let raw = serde_json::json!({ "type": "subscribe", "future_filter": ["x"] });
        let m: ClientMessage = serde_json::from_value(raw).unwrap();
        assert_eq!(m, ClientMessage::Subscribe);
    }
}

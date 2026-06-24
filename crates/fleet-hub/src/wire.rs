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
//! Design choices (consistent with the protocol crate, the design/§7.6):
//! - **JSON, internally tagged** on a `type` discriminator — human-debuggable.
//! - **`schema_version` tolerated, never required** on inbound: an older client
//!   may omit it; a newer one may add fields. We do not `deny_unknown_fields`.
//! - Reporter deltas carry **whole objects** for upserts (full-object deltas are
//!   acceptable per README §7.4) and **ids only** for removals — mirroring the
//!   outbound [`fleet_protocol::Event`] shape so the two stay symmetric.

use fleet_protocol::{AgentRun, Command, Session};
use serde::{Deserialize, Serialize};

/// The durable-identity stamp a reporter attaches to a run delta (S6).
///
/// `durable_id` is the run's fixed identity anchored on its native agent id
/// (D4 — Codex `thread.id` / Claude `session_id`). `epoch` distinguishes a
/// *reconnect* (same epoch, reclaim + continue the `seq` series) from a
/// *fresh-start* (bumped epoch, wipe the prior series). `seq` is the monotonic
/// per-run sequence number. Together `(durable_id, epoch, seq)` is what
/// [`crate::reclaim::ReclaimTable`] gates on for idempotent, ordered apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqStamp {
    /// The run's fixed durable id (native-agent-anchored).
    pub durable_id: String,
    /// Run-instance epoch: bumped on a clean fresh-start, kept on reconnect.
    #[serde(default)]
    pub epoch: u64,
    /// Monotonic per-run sequence number.
    pub seq: u64,
}

impl SeqStamp {
    /// Construct a stamp.
    pub fn new(durable_id: impl Into<String>, epoch: u64, seq: u64) -> Self {
        SeqStamp {
            durable_id: durable_id.into(),
            epoch,
            seq,
        }
    }
}

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
    ///
    /// **Durable identity (S6).** A run upsert MAY carry a [`SeqStamp`] — the
    /// run's durable id, epoch, and monotonic per-run `seq` — so the Hub can
    /// apply it idempotently and in `seq` order ([`crate::reclaim`]). The field
    /// is optional and `#[serde(default)]`: an older reporter (S5) that omits it
    /// is treated as un-gated (always applied, preserving prior behavior), while
    /// an S6 reporter stamps every delta. This keeps the inbound vocabulary
    /// backward-compatible while the contract is still being shaped.
    #[serde(rename = "run.upsert")]
    RunUpsert {
        session_id: String,
        run: AgentRun,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stamp: Option<SeqStamp>,
    },

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
    fn type_name_matches_wire_tag_for_every_variant() {
        // Exhaustively pins `type_name()` to the serialized `type` discriminator
        // for all variants, including Subscribe and Command (the others are also
        // checked in the round-trip test).
        assert_eq!(ClientMessage::Subscribe.type_name(), "subscribe");
        assert_eq!(
            ClientMessage::Command {
                command: Command::mute("s1")
            }
            .type_name(),
            "command"
        );
        assert_eq!(
            ClientMessage::SessionUpsert {
                session: sample_session("s1")
            }
            .type_name(),
            "session.upsert"
        );
        assert_eq!(
            ClientMessage::SessionRemove {
                session_id: "s1".into()
            }
            .type_name(),
            "session.remove"
        );
        assert_eq!(
            ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run: sample_run("r1"),
                stamp: None,
            }
            .type_name(),
            "run.upsert"
        );
        assert_eq!(
            ClientMessage::RunRemove {
                session_id: "s1".into(),
                run_id: "r1".into(),
            }
            .type_name(),
            "run.remove"
        );
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
                stamp: None,
            },
            ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run: sample_run("r1"),
                stamp: Some(SeqStamp::new("native", 0, 7)),
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
    fn run_upsert_omits_stamp_when_absent() {
        // S5-compatibility: a run.upsert without a stamp serializes WITHOUT the
        // `stamp` key, and an S5 wire frame (no stamp) parses to `stamp: None`.
        let m = ClientMessage::RunUpsert {
            session_id: "s1".into(),
            run: sample_run("r1"),
            stamp: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert!(
            v.get("stamp").is_none(),
            "absent stamp must not be on the wire"
        );
        // An S5 frame (no stamp field at all) still deserializes.
        let raw = serde_json::json!({
            "type": "run.upsert",
            "session_id": "s1",
            "run": serde_json::to_value(sample_run("r1")).unwrap(),
        });
        let back: ClientMessage = serde_json::from_value(raw).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn run_upsert_carries_stamp_when_present() {
        let m = ClientMessage::RunUpsert {
            session_id: "s1".into(),
            run: sample_run("r1"),
            stamp: Some(SeqStamp::new("native-d", 2, 11)),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["stamp"]["durable_id"], "native-d");
        assert_eq!(v["stamp"]["epoch"], 2);
        assert_eq!(v["stamp"]["seq"], 11);
        let back: ClientMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn seq_stamp_epoch_defaults_to_zero() {
        // An S6 reporter that omits epoch (reconnect-default) parses to epoch 0.
        let raw = serde_json::json!({ "durable_id": "d", "seq": 3 });
        let s: SeqStamp = serde_json::from_value(raw).unwrap();
        assert_eq!(s.epoch, 0);
        assert_eq!(s.seq, 3);
    }

    #[test]
    fn unknown_fields_on_known_type_tolerated() {
        // A newer client adds an envelope field; the Hub still parses.
        let raw = serde_json::json!({ "type": "subscribe", "future_filter": ["x"] });
        let m: ClientMessage = serde_json::from_value(raw).unwrap();
        assert_eq!(m, ClientMessage::Subscribe);
    }
}

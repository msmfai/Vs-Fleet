//! Forward-compatibility tests (PLAN invariant 2: "faces tolerate unknown
//! fields"). A newer Hub may add fields to any object or envelope; an older
//! face must deserialize it, preserve the unknowns, and re-emit them.

use fleet_protocol::commands::Command;
use fleet_protocol::events::Event;
use fleet_protocol::objects::{AgentRun, Session};

/// Unknown fields on a nested object (Location/Editor/Server) are tolerated and
/// preserved through a round trip.
#[test]
fn unknown_fields_at_every_object_level() {
    let raw = serde_json::json!({
        "schema_version": 1,
        "session_id": "s1",
        "title": "repo @ main",
        "location": {
            "kind": "remote", "label": "hetzner", "glyph": "remote",
            "attach_hint": "ssh box", "future_loc_field": 42
        },
        "editor": { "kind": "cursor", "focus_hint": "code -r", "future_ed_field": true },
        "server": { "kind": "code-server", "version": "4.9", "future_srv_field": "x" },
        "runs": [{
            "schema_version": 1,
            "run_id": "r1", "agent_kind": "claude-code", "native_id": "sid",
            "cwd": "/work", "state": "waiting", "urgency": "approval",
            "confidence": "inferred", "updated_at": "2026-06-08T00:00:00Z",
            "future_run_field": [1, 2, 3]
        }],
        "rollup_state": "waiting", "rollup_urgency": "approval",
        "muted": false, "soloed": false, "unread": true,
        "tags": ["x"], "updated_at": "2026-06-08T00:00:00Z",
        "future_session_field": {"deep": {"nested": "ok"}}
    });

    let session: Session = serde_json::from_value(raw).expect("tolerates unknowns");
    // Captured at each level.
    assert_eq!(
        session.extra["future_session_field"]["deep"]["nested"],
        "ok"
    );
    assert_eq!(session.location.extra["future_loc_field"], 42);
    assert_eq!(
        session.editor.as_ref().unwrap().extra["future_ed_field"],
        true
    );
    assert_eq!(session.server.extra["future_srv_field"], "x");
    assert_eq!(session.runs[0].extra["future_run_field"][2], 3);

    // Re-emitted on serialize.
    let out = serde_json::to_value(&session).unwrap();
    assert_eq!(out["future_session_field"]["deep"]["nested"], "ok");
    assert_eq!(out["location"]["future_loc_field"], 42);
    assert_eq!(out["runs"][0]["future_run_field"][2], 3);
}

/// Unknown envelope fields on events/commands are tolerated.
#[test]
fn unknown_fields_on_envelopes() {
    let raw_event = serde_json::json!({
        "type": "session.removed",
        "schema_version": 1,
        "session_id": "s1",
        "future_envelope_field": "ignored-but-ok"
    });
    let e: Event = serde_json::from_value(raw_event).expect("event tolerates unknowns");
    matches!(e, Event::SessionRemoved { .. });

    let raw_cmd = serde_json::json!({
        "command": "mute",
        "schema_version": 1,
        "session_id": "s1",
        "future_cmd_field": 99
    });
    let c: Command = serde_json::from_value(raw_cmd).expect("command tolerates unknowns");
    matches!(c, Command::Mute { .. });
}

/// A run with all optional fields omitted deserializes (minimal wire form), and
/// re-serializing omits the `None` fields (skip_serializing_if).
#[test]
fn minimal_run_form_and_omitted_optionals() {
    let raw = serde_json::json!({
        "schema_version": 1,
        "run_id": "r", "agent_kind": "codex", "native_id": "t",
        "cwd": "/", "state": "idle", "confidence": "high",
        "updated_at": "2026-06-08T00:00:00Z"
    });
    let run: AgentRun = serde_json::from_value(raw).unwrap();
    assert!(run.urgency.is_none());
    assert!(run.last_message.is_none());
    assert!(run.waiting_since.is_none());

    let out = serde_json::to_value(&run).unwrap();
    for omitted in ["urgency", "last_message", "waiting_since", "diff_summary"] {
        assert!(out.get(omitted).is_none(), "{omitted} should be omitted");
    }
}

/// An unknown *enum token* (e.g. a future state) must NOT be silently accepted —
/// the schema is closed on enums even while open on object fields. This protects
/// confidence-honesty: a face never guesses an unknown state's meaning.
#[test]
fn unknown_enum_token_is_rejected() {
    let raw = serde_json::json!({
        "schema_version": 1,
        "run_id": "r", "agent_kind": "codex", "native_id": "t",
        "cwd": "/", "state": "teleporting", "confidence": "high",
        "updated_at": "2026-06-08T00:00:00Z"
    });
    let res: Result<AgentRun, _> = serde_json::from_value(raw);
    assert!(res.is_err(), "unknown state token must be rejected");
}

/// An unknown *event type* is rejected (a face should surface, not drop, an
/// event it cannot interpret).
#[test]
fn unknown_event_type_is_rejected() {
    let raw = serde_json::json!({ "type": "session.teleported", "schema_version": 1 });
    let res: Result<Event, _> = serde_json::from_value(raw);
    assert!(res.is_err());
}

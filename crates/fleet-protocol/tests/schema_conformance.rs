//! JSON Schema conformance (test criterion: JSON-Schema conformance BOTH
//! directions; emit a JSON Schema artifact and validate against it).
//!
//! Direction 1 (encode→validate): values produced by our serializers validate
//! against the generated schema.
//! Direction 2 (validate→decode): documents the schema accepts also deserialize
//! into the Rust types (and documents it rejects also fail to deserialize).
//!
//! Plus a drift guard: the committed artifact on disk must equal freshly
//! generated output, so a type change without regeneration fails CI.

#![cfg(feature = "schema")]

use fleet_protocol::commands::{Command, Target};
use fleet_protocol::events::Event;
use fleet_protocol::objects::{
    AgentKind, AgentRun, Editor, EditorKind, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session,
};
use fleet_protocol::schema;
use fleet_protocol::state::{Confidence, State, Urgency};

/// Compile a schemars `RootSchema` into a jsonschema validator.
fn compile(root: &schemars::schema::RootSchema) -> jsonschema::JSONSchema {
    let value = serde_json::to_value(root).expect("schema to value");
    jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&value)
        .expect("schema compiles")
}

fn assert_valid(validator: &jsonschema::JSONSchema, instance: &serde_json::Value, ctx: &str) {
    if let Err(errors) = validator.validate(instance) {
        let msgs: Vec<String> = errors.map(|e| format!("  - {e}")).collect();
        panic!(
            "instance failed schema validation ({ctx}):\n{}\ninstance: {}",
            msgs.join("\n"),
            serde_json::to_string_pretty(instance).unwrap()
        );
    }
}

fn full_run() -> AgentRun {
    let mut r = AgentRun::new(
        "run-1",
        AgentKind::ClaudeCode,
        "session-abc",
        "/work/repo",
        State::Waiting,
        Confidence::Inferred,
        "2026-06-08T00:00:00Z",
    );
    r.urgency = Some(Urgency::Approval);
    r.last_message = Some("Can I run `rm -rf build`?".into());
    r.waiting_since = Some("2026-06-08T00:00:01Z".into());
    r.diff_summary = Some(serde_json::json!({"files_changed": 2}));
    r
}

fn full_session() -> Session {
    let location = Location {
        kind: LocationKind::Docker,
        label: "container-7".into(),
        glyph: LocationGlyph::Docker,
        attach_hint: Some("docker exec".into()),
        extra: Extra::new(),
    };
    let editor = Editor {
        kind: Some(EditorKind::Vscode),
        focus_hint: Some("code -r /work/repo".into()),
        extra: Extra::new(),
    };
    let server = Server {
        kind: ServerKind::CodeServer,
        version: Some("4.91".into()),
        extra: Extra::new(),
    };
    let mut s = Session::new(
        "sess-1",
        "repo @ main",
        location,
        server,
        State::Waiting,
        "2026-06-08T00:00:00Z",
    );
    s.editor = Some(editor);
    s.rollup_urgency = Some(Urgency::Approval);
    s.unread = true;
    s.tags = vec!["urgent".into()];
    s.runs = vec![full_run()];
    s
}

#[test]
fn agentrun_validates_against_schema() {
    let v = compile(&schema::run_schema());
    // Full and minimal forms both validate.
    assert_valid(&v, &serde_json::to_value(full_run()).unwrap(), "full run");
    let minimal = AgentRun::new(
        "r",
        AgentKind::Codex,
        "t",
        "/",
        State::Idle,
        Confidence::High,
        "2026-06-08T00:00:00Z",
    );
    assert_valid(&v, &serde_json::to_value(minimal).unwrap(), "minimal run");
}

#[test]
fn session_validates_against_schema() {
    let v = compile(&schema::session_schema());
    assert_valid(
        &v,
        &serde_json::to_value(full_session()).unwrap(),
        "full session",
    );
}

#[test]
fn every_event_validates_against_schema() {
    let v = compile(&schema::event_schema());
    let s = full_session();
    let r = full_run();
    let events = vec![
        Event::snapshot(vec![s.clone()]),
        Event::session_added(s.clone()),
        Event::session_updated(s.clone()),
        Event::session_removed("s"),
        Event::run_added("s", r.clone()),
        Event::run_updated("s", r.clone()),
        Event::run_removed("s", "r"),
    ];
    for e in events {
        assert_valid(&v, &serde_json::to_value(&e).unwrap(), e.type_name());
    }
}

#[test]
fn every_command_validates_against_schema() {
    let v = compile(&schema::command_schema());
    let cmds = vec![
        Command::focus(Target::run("r")),
        Command::focus(Target::session("s")),
        Command::mute("s"),
        Command::unmute("s"),
        Command::solo("s"),
        Command::dismiss(Target::session("s")),
        Command::set_tags("s", vec!["a".into(), "b".into()]),
    ];
    for c in cmds {
        assert_valid(&v, &serde_json::to_value(&c).unwrap(), c.command_name());
    }
}

/// Direction 2: a document that validates against the schema also deserializes
/// into the Rust type; one that violates the schema's enum also fails to decode.
#[test]
fn schema_acceptance_matches_decode() {
    let v = compile(&schema::run_schema());

    let good = serde_json::json!({
        "schema_version": 1, "run_id": "r", "agent_kind": "codex",
        "native_id": "t", "cwd": "/", "state": "done", "confidence": "high",
        "updated_at": "2026-06-08T00:00:00Z"
    });
    assert_valid(&v, &good, "good run");
    assert!(serde_json::from_value::<AgentRun>(good).is_ok());

    // Bad enum token: schema rejects AND decode rejects (consistency).
    let bad = serde_json::json!({
        "schema_version": 1, "run_id": "r", "agent_kind": "codex",
        "native_id": "t", "cwd": "/", "state": "zzz", "confidence": "high",
        "updated_at": "2026-06-08T00:00:00Z"
    });
    assert!(v.validate(&bad).is_err(), "schema should reject bad state");
    assert!(serde_json::from_value::<AgentRun>(bad).is_err());
}

/// `done` and `idle` are BOTH present and DISTINCT in the schema enum (D9).
#[test]
fn schema_keeps_done_distinct_from_idle() {
    let root = schema::run_schema();
    let value = serde_json::to_value(&root).unwrap();
    let text = serde_json::to_string(&value).unwrap();
    assert!(text.contains("\"done\""), "schema must list `done`");
    assert!(text.contains("\"idle\""), "schema must list `idle`");
}

/// Drift guard: committed artifact == freshly generated output.
#[test]
fn committed_schema_artifact_is_current() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/schema/fleet-protocol.schema.json"
    );
    let on_disk = std::fs::read_to_string(path)
        .expect("schema artifact missing — run `cargo run -p fleet-protocol --bin gen-schema`");
    let fresh = schema::combined_schema_json();
    assert_eq!(
        on_disk, fresh,
        "committed schema is stale — regenerate with `cargo run -p fleet-protocol --bin gen-schema`"
    );
}

//! JSON Schema emission for the wire protocol (behind the `schema` feature).
//!
//! The committed artifact (`schema/fleet-protocol.schema.json`) is generated
//! from these functions and validated against real serialized values by the
//! conformance test. The generator binary `gen-schema` writes the file; a test
//! asserts the file on disk is byte-identical to freshly generated output so it
//! can never drift unnoticed.

use schemars::{schema_for, Schema};

use crate::commands::Command;
use crate::events::Event;
use crate::objects::{AgentRun, Session};

/// JSON Schema for the [`Event`] envelope (Hub→face).
pub fn event_schema() -> Schema {
    schema_for!(Event)
}

/// JSON Schema for the [`Command`] envelope (face→Hub).
pub fn command_schema() -> Schema {
    schema_for!(Command)
}

/// JSON Schema for a bare [`Session`] object.
pub fn session_schema() -> Schema {
    schema_for!(Session)
}

/// JSON Schema for a bare [`AgentRun`] object.
pub fn run_schema() -> Schema {
    schema_for!(AgentRun)
}

/// The single combined schema document committed as the artifact. It nests the
/// four top-level schemas under `definitions`-style keys so one file fully
/// describes the wire surface.
pub fn combined_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "fleet-protocol",
        "description": "Fleet wire protocol — Hub<->face events and commands plus the Session/AgentRun objects they carry.",
        "schemaVersion": crate::SCHEMA_VERSION,
        "schemas": {
            "Event": event_schema(),
            "Command": command_schema(),
            "Session": session_schema(),
            "AgentRun": run_schema(),
        }
    })
}

/// Pretty-printed, newline-terminated combined schema — exactly what the
/// generator writes and the drift test compares against.
pub fn combined_schema_json() -> String {
    let mut s =
        serde_json::to_string_pretty(&combined_schema()).expect("combined schema serializes");
    s.push('\n');
    s
}

//! The two product objects — [`Session`] and [`AgentRun`] — plus their nested
//! descriptors (README §7.1, §7.2).
//!
//! **Forward-compat (invariant 2):** none of these structs use
//! `deny_unknown_fields`. A `#[serde(flatten)]` `extra` map captures any
//! unrecognized keys so a newer Hub can add fields and an older face will
//! deserialize, preserve, and re-emit them rather than erroring. This is
//! exercised directly in the test suite.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::state::{Confidence, State, Urgency};
use crate::SCHEMA_VERSION;

/// Catch-all for unknown fields. A `BTreeMap` (not `HashMap`) so re-serialized
/// output is deterministic — important for the round-trip property tests and
/// for any golden-file comparisons.
pub type Extra = BTreeMap<String, serde_json::Value>;

/// Skip predicate for optional opaque JSON fields (`diff_summary`, `policy`).
///
/// `Option<serde_json::Value>` has a wire ambiguity: both `None` and
/// `Some(Value::Null)` mean "no value", but plain serde would emit the latter
/// as an explicit `null` and then read it back as `None` — breaking round-trip
/// identity. Treating `Some(Null)` as absent makes serialize/deserialize a true
/// inverse: absence on the wire ⇒ `None` ⇒ absence on the wire.
fn is_none_or_json_null(v: &Option<serde_json::Value>) -> bool {
    matches!(v, None | Some(serde_json::Value::Null))
}

/// Where the session physically lives (README §7.1 `location`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum LocationKind {
    Local,
    Docker,
    Remote,
}

/// Glyph hint for the location (README §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum LocationGlyph {
    Laptop,
    Docker,
    Remote,
}

/// README §7.1 `location` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Location {
    pub kind: LocationKind,
    pub label: String,
    pub glyph: LocationGlyph,
    /// Hint for attaching to a remote/docker locale; `null` for local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_hint: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The editor flavor bound to a session (README §7.1 `editor.kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum EditorKind {
    Vscode,
    Cursor,
    Windsurf,
}

/// README §7.1 `editor` object. The whole object is optional on a session
/// (`editor: null` is legal); within it, `kind` may itself be `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Editor {
    /// `null` when the editor flavor is unknown (README writes `...|null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<EditorKind>,
    /// CLI args / URI used to focus this window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_hint: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The server backing the session (README §7.1 `server.kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ServerKind {
    CodeServer,
    OpenvscodeServer,
    DesktopRemote,
    Local,
}

/// README §7.1 `server` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Server {
    pub kind: ServerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The agent flavor of a run (README §7.2 `agent_kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Other,
}

/// A single agent run within a session (README §7.2).
///
/// A [`Session`] has one or more of these. `schema_version` is carried on the
/// object per §7.6.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct AgentRun {
    /// Versioning (§7.6) — present on every object.
    pub schema_version: u32,
    pub run_id: String,
    pub agent_kind: AgentKind,
    /// Durable identity anchor: Claude `session_id` / Codex `threadId` (§7.5).
    pub native_id: String,
    pub cwd: String,
    pub state: State,
    /// `null` ⇒ no urgency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<Urgency>,
    /// Short preview of the agent's output / its question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    /// ISO-8601 timestamp the run entered `waiting`; `null` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<String>,
    /// Confidence honesty (invariant 5): `inferred` when `waiting` came from a
    /// heuristic, not an authoritative signal.
    pub confidence: Confidence,
    /// v1.5+: `{files_changed, insertions, deletions}`. Opaque for now.
    #[serde(default, skip_serializing_if = "is_none_or_json_null")]
    pub diff_summary: Option<serde_json::Value>,
    /// ISO-8601.
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: Extra,
}

impl AgentRun {
    /// Construct a minimal run at the current schema version. Optional fields
    /// default to `None`/empty; callers set what they have.
    pub fn new(
        run_id: impl Into<String>,
        agent_kind: AgentKind,
        native_id: impl Into<String>,
        cwd: impl Into<String>,
        state: State,
        confidence: Confidence,
        updated_at: impl Into<String>,
    ) -> Self {
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            agent_kind,
            native_id: native_id.into(),
            cwd: cwd.into(),
            state,
            urgency: None,
            last_message: None,
            waiting_since: None,
            confidence,
            diff_summary: None,
            updated_at: updated_at.into(),
            extra: Extra::new(),
        }
    }
}

/// A tracked VS Code-Server environment (README §7.1).
///
/// `rollup_state` / `rollup_urgency` are the most-urgent state/urgency across
/// the session's runs (computed by the Hub merge engine; see [`crate::rollup`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Session {
    /// Versioning (§7.6) — present on every object.
    pub schema_version: u32,
    /// Stable durable session id.
    pub session_id: String,
    pub title: String,
    pub location: Location,
    /// `null` when no editor is bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<Editor>,
    pub server: Server,
    pub runs: Vec<AgentRun>,
    /// Worst/most-urgent state across `runs`.
    pub rollup_state: State,
    /// Most-urgent urgency across `runs`; `null` ⇒ none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollup_urgency: Option<Urgency>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub soloed: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Opaque policy blob (§17, not modeled in v1).
    #[serde(default, skip_serializing_if = "is_none_or_json_null")]
    pub policy: Option<serde_json::Value>,
    /// ISO-8601.
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Session {
    /// Construct a minimal session at the current schema version with no runs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: impl Into<String>,
        title: impl Into<String>,
        location: Location,
        server: Server,
        rollup_state: State,
        updated_at: impl Into<String>,
    ) -> Self {
        Session {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            title: title.into(),
            location,
            editor: None,
            server,
            runs: Vec::new(),
            rollup_state,
            rollup_urgency: None,
            muted: false,
            soloed: false,
            unread: false,
            tags: Vec::new(),
            policy: None,
            updated_at: updated_at.into(),
            extra: Extra::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_location() -> Location {
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        }
    }

    fn sample_server() -> Server {
        Server {
            kind: ServerKind::Local,
            version: Some("1.0".into()),
            extra: Extra::new(),
        }
    }

    #[test]
    fn agentrun_carries_schema_version() {
        let r = AgentRun::new(
            "run-1",
            AgentKind::Codex,
            "thread-1",
            "/tmp",
            State::Working,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        // urgency omitted (None) ⇒ key absent.
        assert!(v.get("urgency").is_none());
    }

    #[test]
    fn session_carries_schema_version() {
        let s = Session::new(
            "sess-1",
            "repo @ main",
            sample_location(),
            sample_server(),
            State::Idle,
            "2026-06-08T00:00:00Z",
        );
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn forward_compat_unknown_fields_preserved() {
        // A newer Hub emits an AgentRun with a field this build doesn't know.
        let raw = serde_json::json!({
            "schema_version": 1,
            "run_id": "run-9",
            "agent_kind": "codex",
            "native_id": "thread-9",
            "cwd": "/work",
            "state": "waiting",
            "urgency": "approval",
            "confidence": "high",
            "updated_at": "2026-06-08T00:00:00Z",
            "future_field": {"nested": [1, 2, 3]},
            "another_unknown": "keepme"
        });
        let run: AgentRun = serde_json::from_value(raw.clone()).unwrap();
        // Tolerated, captured, and round-tripped back out.
        assert_eq!(run.extra.get("future_field").unwrap()["nested"][2], 3);
        let back = serde_json::to_value(&run).unwrap();
        assert_eq!(back["future_field"]["nested"][2], 3);
        assert_eq!(back["another_unknown"], "keepme");
    }

    #[test]
    fn editor_null_and_kind_null_legal() {
        // editor: null on the session.
        let raw = serde_json::json!({
            "schema_version": 1,
            "session_id": "s",
            "title": "t",
            "location": {"kind":"local","label":"l","glyph":"laptop"},
            "editor": null,
            "server": {"kind":"local"},
            "runs": [],
            "rollup_state": "idle",
            "updated_at": "2026-06-08T00:00:00Z"
        });
        let s: Session = serde_json::from_value(raw).unwrap();
        assert!(s.editor.is_none());
        assert!(s.rollup_urgency.is_none());
    }
}

//! Hub→face events (README §7.4).
//!
//! Wire shape: an internally-tagged object with a `type` discriminator whose
//! value is the dotted event name (`fleet.snapshot`, `session.added`, …) and a
//! `schema_version` carried on the envelope per §7.6. Deltas are preferred but
//! full objects are acceptable (§7.4), which is why the `*.added`/`*.updated`
//! variants carry whole objects.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::objects::{AgentRun, Session};
use crate::SCHEMA_VERSION;

/// An event pushed from the Hub to a face.
///
/// The `type` tag uses the README's dotted names. `#[serde(tag = "type")]`
/// makes them internally tagged so the discriminator and payload share one
/// JSON object. We do NOT `deny_unknown_fields` here: faces must tolerate
/// envelope fields a newer Hub adds (invariant 2). Unknown variants, however,
/// are a hard deserialize error by design — a face that sees an event `type`
/// it cannot interpret should surface that rather than silently drop state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "type")]
pub enum Event {
    /// Full list of sessions, sent on subscribe (README §7.4).
    #[serde(rename = "fleet.snapshot")]
    Snapshot {
        schema_version: u32,
        sessions: Vec<Session>,
    },
    /// A session newly appeared.
    #[serde(rename = "session.added")]
    SessionAdded {
        schema_version: u32,
        session: Session,
    },
    /// A session changed (full object acceptable per §7.4).
    #[serde(rename = "session.updated")]
    SessionUpdated {
        schema_version: u32,
        session: Session,
    },
    /// A session was removed; only its id is needed.
    #[serde(rename = "session.removed")]
    SessionRemoved {
        schema_version: u32,
        session_id: String,
    },
    /// A run newly appeared within a session.
    #[serde(rename = "run.added")]
    RunAdded {
        schema_version: u32,
        session_id: String,
        run: AgentRun,
    },
    /// A run changed.
    #[serde(rename = "run.updated")]
    RunUpdated {
        schema_version: u32,
        session_id: String,
        run: AgentRun,
    },
    /// A run was removed; ids only.
    #[serde(rename = "run.removed")]
    RunRemoved {
        schema_version: u32,
        session_id: String,
        run_id: String,
    },
}

impl Event {
    /// The dotted wire name of this event's `type` discriminator.
    pub fn type_name(&self) -> &'static str {
        match self {
            Event::Snapshot { .. } => "fleet.snapshot",
            Event::SessionAdded { .. } => "session.added",
            Event::SessionUpdated { .. } => "session.updated",
            Event::SessionRemoved { .. } => "session.removed",
            Event::RunAdded { .. } => "run.added",
            Event::RunUpdated { .. } => "run.updated",
            Event::RunRemoved { .. } => "run.removed",
        }
    }

    /// Convenience constructors that stamp the current [`SCHEMA_VERSION`].
    pub fn snapshot(sessions: Vec<Session>) -> Self {
        Event::Snapshot {
            schema_version: SCHEMA_VERSION,
            sessions,
        }
    }
    pub fn session_added(session: Session) -> Self {
        Event::SessionAdded {
            schema_version: SCHEMA_VERSION,
            session,
        }
    }
    pub fn session_updated(session: Session) -> Self {
        Event::SessionUpdated {
            schema_version: SCHEMA_VERSION,
            session,
        }
    }
    pub fn session_removed(session_id: impl Into<String>) -> Self {
        Event::SessionRemoved {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
        }
    }
    pub fn run_added(session_id: impl Into<String>, run: AgentRun) -> Self {
        Event::RunAdded {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            run,
        }
    }
    pub fn run_updated(session_id: impl Into<String>, run: AgentRun) -> Self {
        Event::RunUpdated {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            run,
        }
    }
    pub fn run_removed(session_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Event::RunRemoved {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            run_id: run_id.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_wire_shape() {
        let e = Event::snapshot(vec![]);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "fleet.snapshot");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert!(v["sessions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn removed_variants_are_id_only() {
        let e = Event::run_removed("s1", "r1");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "run.removed");
        assert_eq!(v["session_id"], "s1");
        assert_eq!(v["run_id"], "r1");
        assert!(v.get("run").is_none());
    }

    #[test]
    fn type_name_matches_tag() {
        let e = Event::session_removed("x");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], e.type_name());
    }
}

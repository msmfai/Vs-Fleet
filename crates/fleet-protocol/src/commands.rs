//! Face→Hub commands (README §7.4).
//!
//! The node's required set: `focus`, `mute`, `unmute`, `solo`, `dismiss`,
//! `set_tags`. (`deploy`/`launch_run` are v1.5+ and intentionally out of scope
//! here.) Wire shape mirrors [`crate::events::Event`]: an internally-tagged
//! object with a `command` discriminator and a `schema_version` per §7.6.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::SCHEMA_VERSION;

/// A target that a command addresses. `focus`/`dismiss` may address either a
/// whole session or a single run (README writes `focus(session_id|run_id)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Target {
    Session { session_id: String },
    Run { run_id: String },
}

impl Target {
    pub fn session(id: impl Into<String>) -> Self {
        Target::Session {
            session_id: id.into(),
        }
    }
    pub fn run(id: impl Into<String>) -> Self {
        Target::Run { run_id: id.into() }
    }
}

/// A command sent from a face to the Hub.
///
/// Internally tagged on `command`. Like events, the envelope tolerates unknown
/// fields a newer face adds; unknown *commands* are a deserialize error so the
/// Hub never silently ignores an instruction it cannot honor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "command")]
pub enum Command {
    /// Focus the editor window for a session or run (§12.2).
    #[serde(rename = "focus")]
    Focus { schema_version: u32, target: Target },
    /// Mute a session's pings (state still shown) (§15.4).
    #[serde(rename = "mute")]
    Mute {
        schema_version: u32,
        session_id: String,
    },
    /// Unmute a session.
    #[serde(rename = "unmute")]
    Unmute {
        schema_version: u32,
        session_id: String,
    },
    /// Solo a session: mute all others (§15.4).
    #[serde(rename = "solo")]
    Solo {
        schema_version: u32,
        session_id: String,
    },
    /// Dismiss a (typically `dead`/stale) session or run (§7.3, §15.2).
    #[serde(rename = "dismiss")]
    Dismiss { schema_version: u32, target: Target },
    /// Replace the tag set on a session.
    #[serde(rename = "set_tags")]
    SetTags {
        schema_version: u32,
        session_id: String,
        tags: Vec<String>,
    },
}

impl Command {
    /// The wire name of this command's `command` discriminator.
    pub fn command_name(&self) -> &'static str {
        match self {
            Command::Focus { .. } => "focus",
            Command::Mute { .. } => "mute",
            Command::Unmute { .. } => "unmute",
            Command::Solo { .. } => "solo",
            Command::Dismiss { .. } => "dismiss",
            Command::SetTags { .. } => "set_tags",
        }
    }

    pub fn focus(target: Target) -> Self {
        Command::Focus {
            schema_version: SCHEMA_VERSION,
            target,
        }
    }
    pub fn mute(session_id: impl Into<String>) -> Self {
        Command::Mute {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
        }
    }
    pub fn unmute(session_id: impl Into<String>) -> Self {
        Command::Unmute {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
        }
    }
    pub fn solo(session_id: impl Into<String>) -> Self {
        Command::Solo {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
        }
    }
    pub fn dismiss(target: Target) -> Self {
        Command::Dismiss {
            schema_version: SCHEMA_VERSION,
            target,
        }
    }
    pub fn set_tags(session_id: impl Into<String>, tags: Vec<String>) -> Self {
        Command::SetTags {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_targets_run_or_session() {
        let by_run = Command::focus(Target::run("r1"));
        let v = serde_json::to_value(&by_run).unwrap();
        assert_eq!(v["command"], "focus");
        assert_eq!(v["target"]["type"], "run");
        assert_eq!(v["target"]["run_id"], "r1");

        let by_sess = Command::focus(Target::session("s1"));
        let v = serde_json::to_value(&by_sess).unwrap();
        assert_eq!(v["target"]["type"], "session");
        assert_eq!(v["target"]["session_id"], "s1");
    }

    #[test]
    fn set_tags_round_trip() {
        let c = Command::set_tags("s1", vec!["a".into(), "b".into()]);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["command"], "set_tags");
        assert_eq!(v["tags"][1], "b");
        let back: Command = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn command_name_matches_tag() {
        for c in [
            Command::mute("s"),
            Command::unmute("s"),
            Command::solo("s"),
            Command::dismiss(Target::session("s")),
        ] {
            let v = serde_json::to_value(&c).unwrap();
            assert_eq!(v["command"], c.command_name());
        }
    }
}

//! Core state / urgency / confidence enums (README §7.2, §7.3; PLAN D9).
//!
//! Wire encoding: lowercase, kebab-cased strings (e.g. `idle-done`). Every
//! variant is exhaustively round-trip tested in the crate test suite.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// Per-run lifecycle state (README §7.3).
///
/// **D9 (locked): `Done` is KEPT DISTINCT from `Idle`.** `idle` means the run is
/// alive and waiting for the next prompt; `done` means the agent reported its
/// task complete. They must never be collapsed on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Actively producing output / running a tool.
    Working,
    /// Blocked on the user (approval or question). The only state that pings.
    Waiting,
    /// Alive, turn finished, awaiting the next prompt.
    Idle,
    /// Agent reported its task complete. Kept distinct from `Idle` (D9).
    Done,
    /// The agent raised an error.
    Error,
    /// Process exited or the reporter is gone past the timeout (§7.3).
    Dead,
}

impl State {
    /// All variants, for exhaustive testing and iteration.
    pub const ALL: [State; 6] = [
        State::Working,
        State::Waiting,
        State::Idle,
        State::Done,
        State::Error,
        State::Dead,
    ];

    /// `true` only for `Waiting` — the single state that pings (§7.3).
    pub fn pings(self) -> bool {
        matches!(self, State::Waiting)
    }
}

/// Why a run/session is demanding attention (README §7.2).
///
/// `null` on the wire (absence of urgency) maps to [`Urgency::None`]; it is
/// serialized as the JSON string `"null"` is NOT used — instead the field that
/// holds urgency is `Option<Urgency>` and `None` ⇒ JSON `null`. The explicit
/// `None` variant here exists so callers can name "no urgency" without an
/// `Option` wrapper when a non-optional urgency is required by a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Urgency {
    /// Agent is asking for an approval (run a command, edit a file, …).
    Approval,
    /// Agent asked the user a question.
    Question,
    /// Idle/done attention tier (the quiet tier; §15.5).
    IdleDone,
    /// Explicitly no urgency. Serializes to the string `"null"`.
    ///
    /// Note: the README writes `rollup_urgency: "...|null"`. We model the
    /// *absence* of urgency on optional fields as JSON `null` via `Option`, and
    /// also provide this named variant so a required urgency can express "none"
    /// without ambiguity. Its wire token is the literal string `null`.
    #[serde(rename = "null")]
    None,
}

impl Urgency {
    /// All variants, for exhaustive testing.
    pub const ALL: [Urgency; 4] = [
        Urgency::Approval,
        Urgency::Question,
        Urgency::IdleDone,
        Urgency::None,
    ];
}

/// How trustworthy a `waiting` (or any heuristic) signal is (README §7.2, §8).
///
/// **Invariant 5 (confidence honesty):** a value of `High` must only ever be
/// set from an authoritative signal; heuristic inference is always `Inferred`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// Derived from an authoritative channel (e.g. Codex `requestApproval`,
    /// Claude `PermissionRequest` in Use-Terminal mode).
    High,
    /// Derived from a heuristic (e.g. `PreToolUse`-without-`Stop` debounce).
    Inferred,
}

impl Confidence {
    /// All variants, for exhaustive testing.
    pub const ALL: [Confidence; 2] = [Confidence::High, Confidence::Inferred];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_wire_tokens() {
        assert_eq!(serde_json::to_value(State::Done).unwrap(), "done");
        assert_eq!(serde_json::to_value(State::Idle).unwrap(), "idle");
        // D9: done and idle are distinct tokens.
        assert_ne!(
            serde_json::to_value(State::Done).unwrap(),
            serde_json::to_value(State::Idle).unwrap()
        );
    }

    #[test]
    fn urgency_wire_tokens() {
        assert_eq!(
            serde_json::to_value(Urgency::IdleDone).unwrap(),
            "idle-done"
        );
        assert_eq!(serde_json::to_value(Urgency::None).unwrap(), "null");
        assert_eq!(serde_json::to_value(Urgency::Approval).unwrap(), "approval");
    }

    #[test]
    fn confidence_wire_tokens() {
        assert_eq!(serde_json::to_value(Confidence::High).unwrap(), "high");
        assert_eq!(
            serde_json::to_value(Confidence::Inferred).unwrap(),
            "inferred"
        );
    }

    #[test]
    fn only_waiting_pings() {
        for s in State::ALL {
            assert_eq!(s.pings(), s == State::Waiting);
        }
    }
}

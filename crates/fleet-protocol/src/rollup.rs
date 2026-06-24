//! Rollup helpers: compute a session's `rollup_state` / `rollup_urgency` as the
//! "worst / most-urgent across runs" (README §7.1, §7.3).
//!
//! This lives in the protocol crate so every face and the Hub agree on the
//! ordering (property tests `rollup == most-urgent across runs`).

use crate::objects::AgentRun;
use crate::state::{State, Urgency};

/// Attention priority of a [`State`] for rollup — higher = more urgent, so it
/// wins the rollup. `Waiting` is the most-urgent (it pings); `Dead` is the
/// least. The exact ordering is a contract the Hub and faces share.
fn state_rank(s: State) -> u8 {
    match s {
        State::Waiting => 5,
        State::Error => 4,
        State::Working => 3,
        State::Done => 2,
        State::Idle => 1,
        State::Dead => 0,
    }
}

/// Attention priority of an [`Urgency`] for rollup — higher wins.
fn urgency_rank(u: Urgency) -> u8 {
    match u {
        Urgency::Approval => 3,
        Urgency::Question => 2,
        Urgency::IdleDone => 1,
        Urgency::None => 0,
    }
}

/// The most-urgent state across `runs`. Returns `None` for an empty slice (a
/// session with no runs has no rollup state to compute; callers decide the
/// default).
pub fn rollup_state(runs: &[AgentRun]) -> Option<State> {
    runs.iter().map(|r| r.state).max_by_key(|&s| state_rank(s))
}

/// The most-urgent urgency across `runs`. A run with `urgency: None`
/// contributes [`Urgency::None`]. Returns `None` only for an empty slice.
pub fn rollup_urgency(runs: &[AgentRun]) -> Option<Urgency> {
    runs.iter()
        .map(|r| r.urgency.unwrap_or(Urgency::None))
        .max_by_key(|&u| urgency_rank(u))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::AgentKind;
    use crate::state::Confidence;

    fn run(state: State, urgency: Option<Urgency>) -> AgentRun {
        let mut r = AgentRun::new(
            "r",
            AgentKind::Codex,
            "n",
            "/",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        r.urgency = urgency;
        r
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(rollup_state(&[]), None);
        assert_eq!(rollup_urgency(&[]), None);
    }

    #[test]
    fn waiting_beats_working() {
        let runs = vec![run(State::Working, None), run(State::Waiting, None)];
        assert_eq!(rollup_state(&runs), Some(State::Waiting));
    }

    #[test]
    fn done_distinct_and_ranks_above_idle() {
        let runs = vec![run(State::Idle, None), run(State::Done, None)];
        assert_eq!(rollup_state(&runs), Some(State::Done));
    }

    #[test]
    fn approval_is_most_urgent() {
        let runs = vec![
            run(State::Waiting, Some(Urgency::Question)),
            run(State::Waiting, Some(Urgency::Approval)),
        ];
        assert_eq!(rollup_urgency(&runs), Some(Urgency::Approval));
    }

    // Exercise every `state_rank` arm so the worst-across-runs contract is
    // pinned for all six states, not just the headline Waiting/Done cases.
    #[test]
    fn state_rank_total_order_across_all_variants() {
        use State::*;
        // Descending urgency: each earlier state must win the rollup over each
        // later one when both appear in the same session.
        let order = [Waiting, Error, Working, Done, Idle, Dead];
        for (i, &hi) in order.iter().enumerate() {
            for &lo in &order[i + 1..] {
                let runs = vec![run(lo, None), run(hi, None)];
                assert_eq!(
                    rollup_state(&runs),
                    Some(hi),
                    "{hi:?} must outrank {lo:?} in the rollup"
                );
            }
        }
        // A lone Dead/Error run still rolls up to itself (lowest arms reached).
        assert_eq!(rollup_state(&[run(Dead, None)]), Some(Dead));
        assert_eq!(rollup_state(&[run(Error, None)]), Some(Error));
    }

    // Exercise every `urgency_rank` arm, including IdleDone and the explicit
    // None contributed by a run with no urgency.
    #[test]
    fn urgency_rank_total_order_across_all_variants() {
        use Urgency::*;
        let order = [Approval, Question, IdleDone, None];
        for (i, &hi) in order.iter().enumerate() {
            for &lo in &order[i + 1..] {
                let runs = vec![run(State::Waiting, Some(lo)), run(State::Waiting, Some(hi))];
                assert_eq!(
                    rollup_urgency(&runs),
                    Some(hi),
                    "{hi:?} must outrank {lo:?} in the rollup"
                );
            }
        }
        // A run with `urgency: None` contributes Urgency::None (unwrap_or path).
        // `use Urgency::*` shadows `Option::None` here, so qualify the absent one.
        assert_eq!(
            rollup_urgency(&[run(State::Idle, Option::None)]),
            Some(Urgency::None)
        );
        assert_eq!(
            rollup_urgency(&[run(State::Done, Some(Urgency::IdleDone))]),
            Some(Urgency::IdleDone)
        );
    }
}

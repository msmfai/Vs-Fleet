//! Tab sort ordering — slice S20 (node `UISORT`).
//!
//! S20 layers the `(unread, urgency, age)` ordering on top of the inbox's
//! default insertion order (PLAN S20: "a waiting tab rises to top by (unread,
//! urgency, age) with a ticking waiting-age timer"). The view-model already
//! exposes everything this needs on [`crate::SessionTab`] (`unread`, `urgency`,
//! `waiting_since`), so this slice fills *this file alone* — it never touches
//! the reducer in `view.rs` or any sibling seam.
//!
//! ## Sort contract
//!
//! Tabs are sorted **stable-descending** by the key `(unread, urgency_rank,
//! waiting_age_ms)` computed against a caller-supplied "now" timestamp (an
//! ISO-8601 string or `None` for "now is unknown / age = 0").
//!
//! - **`unread` first:** a tab with an unread notification always precedes one
//!   without (ties broken by the next criterion).
//! - **`urgency` second:** higher urgency precedes lower (Approval > Question >
//!   IdleDone > absent). Non-waiting tabs have urgency `None`, which sorts below
//!   all waiting-urgency tiers.
//! - **`waiting_age` third:** among ties on unread + urgency, the tab that has
//!   been waiting *longest* (largest age) precedes younger tabs — longest wait
//!   rises to the top.
//!
//! Tabs whose sort key equals another are kept in their original relative order
//! (the sort is stable).
//!
//! ## Waiting-age timer
//!
//! Age is computed by the caller injecting the current instant as an ISO-8601
//! string. The view-model itself stays a pure function — it never reads a clock
//! — so the sort is deterministic for any given `now`.
//!
//! [`sort_tabs`] is the main entry point. The caller may store the sorted slice
//! as a `Vec<&SessionTab>` and re-sort on every clock tick without re-reducing
//! the model.

use crate::SessionTab;
use fleet_protocol::Urgency;

/// Numeric priority for sort comparison (higher = more urgent = sorts first).
///
/// Matches the rollup ordering in `fleet_protocol::rollup::urgency_rank` but
/// expressed as a public, comparable value so tests can assert the contract.
///
/// `None` urgency (absent field on the tab) → rank 0, sorts last.
pub fn urgency_sort_rank(u: Option<Urgency>) -> u8 {
    match u {
        Some(Urgency::Approval) => 4,
        Some(Urgency::Question) => 3,
        Some(Urgency::IdleDone) => 2,
        Some(Urgency::None) => 1,
        None => 0,
    }
}

/// Compute the age in seconds of a `waiting_since` ISO-8601 stamp relative
/// to `now_iso` (also ISO-8601).
///
/// Returns `0` if either stamp is absent, malformed, or if `now` does not
/// strictly exceed `waiting_since` (clock skew / same instant). This makes
/// the comparison degenerate gracefully rather than panicking or mis-sorting.
///
/// **Note:** to keep `fleet-host-core` free of heavyweight date-time crates
/// (and therefore keep compile times and dependency count low), the age is
/// computed by parsing both timestamps to whole seconds since the Unix epoch
/// and subtracting. The parser is intentionally minimal — just enough for
/// age comparisons on UTC Z-suffix stamps.
pub fn waiting_age_secs(waiting_since: Option<&str>, now: Option<&str>) -> u64 {
    let (since_str, now_str) = match (waiting_since, now) {
        (Some(s), Some(n)) => (s, n),
        _ => return 0,
    };
    // parse_iso8601_secs returns None on parse failure.
    let since_secs = match parse_iso8601_secs(since_str) {
        Some(s) => s,
        None => return 0,
    };
    let now_secs = match parse_iso8601_secs(now_str) {
        Some(n) => n,
        None => return 0,
    };
    // If timestamps are non-advancing (clock skew / same instant), degrade to 0.
    if now_secs <= since_secs {
        return 0;
    }
    now_secs - since_secs
}

/// Minimal ISO-8601 parser that returns whole seconds since the UNIX epoch
/// (UTC). Accepts the subset `YYYY-MM-DDTHH:MM:SSZ` (with or without
/// fractional seconds). Returns `None` for any format it cannot parse.
///
/// This is intentionally minimal — just enough for age comparisons.
fn parse_iso8601_secs(s: &str) -> Option<u64> {
    // Strip trailing 'Z' (we treat everything as UTC).
    let s = s.trim_end_matches('Z');
    // Also strip fractional seconds if present: "...T10:00:00.123" → drop ".123"
    let s = if let Some(pos) = s.rfind('.') {
        &s[..pos]
    } else {
        s
    };
    // Expected format: "YYYY-MM-DDTHH:MM:SS" (exactly 19 chars after stripping).
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    // Validate separator bytes before parsing digits.
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_digits_opt(&bytes[0..4])?;
    let month = parse_digits_opt(&bytes[5..7])?;
    let day = parse_digits_opt(&bytes[8..10])?;
    let hour = parse_digits_opt(&bytes[11..13])?;
    let min = parse_digits_opt(&bytes[14..16])?;
    let sec = parse_digits_opt(&bytes[17..19])?;

    // Days since epoch (rough, good enough for age differences in the same year).
    let y = year.saturating_sub(1970);
    // Approximate leap-year count (good to ~±1 day, irrelevant for our purpose).
    let leap_days = y / 4;
    let days_per_month: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let month_idx = (month.saturating_sub(1) as usize).min(11);
    let year_days: u64 = y * 365 + leap_days;
    let month_days: u64 = days_per_month[..month_idx].iter().sum();
    let total_days = year_days + month_days + day.saturating_sub(1);

    Some(total_days * 86400 + hour * 3600 + min * 60 + sec)
}

/// Parse ASCII decimal digits into a `u64`. Returns `Some(n)` on success,
/// `None` if any byte is not an ASCII digit.
fn parse_digits_opt(bytes: &[u8]) -> Option<u64> {
    let mut n: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + (b - b'0') as u64;
    }
    Some(n)
}

/// The sort key for a single [`SessionTab`] given the current time.
///
/// Larger key = sorts earlier (more urgent). The components are:
/// - `unread` (bool, 1 bit) — unread beats read.
/// - `urgency_rank` (`u8`) — higher urgency beats lower.
/// - `waiting_age_secs` (`u64`) — older wait beats younger wait.
///
/// Returned as a tuple so test code can assert the full ordering contract.
pub fn sort_key(tab: &SessionTab, now: Option<&str>) -> (bool, u8, u64) {
    let age = waiting_age_secs(tab.waiting_since.as_deref(), now);
    (tab.unread, urgency_sort_rank(tab.urgency), age)
}

/// Sort a slice of [`SessionTab`]s in-place by `(unread, urgency, age)`
/// descending (most-urgent first).
///
/// The sort is **stable**: tabs that compare equal on all three criteria keep
/// their original relative order.
///
/// `now` is the current instant as an ISO-8601 string (e.g.
/// `"2026-06-08T12:34:56Z"`), used to compute waiting ages. Pass `None` if the
/// clock is unavailable — all ages will be treated as 0 (urgency + unread still
/// govern order).
pub fn sort_tabs(tabs: &mut [SessionTab], now: Option<&str>) {
    tabs.sort_by(|a, b| {
        let ka = sort_key(a, now);
        let kb = sort_key(b, now);
        // Descending: larger key first.
        kb.cmp(&ka)
    });
}

/// Sort a slice of [`SessionTab`] references in-place (useful when the caller
/// holds a borrowed slice rather than ownership).
pub fn sort_tab_refs(tabs: &mut Vec<&SessionTab>, now: Option<&str>) {
    tabs.sort_by(|a, b| {
        let ka = sort_key(a, now);
        let kb = sort_key(b, now);
        kb.cmp(&ka)
    });
}

// ── Unit tests (exhaustive, pure-function, no clock dependency) ──────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionTab, TabState};
    use fleet_protocol::{Confidence, LocationGlyph, Urgency};

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn tab(
        id: &str,
        state: TabState,
        unread: bool,
        urgency: Option<Urgency>,
        waiting_since: Option<&str>,
    ) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: crate::AgentIcon::None,
            title: id.into(),
            state,
            urgency,
            confidence: if state == TabState::Waiting {
                Some(Confidence::High)
            } else {
                None
            },
            waiting_since: waiting_since.map(String::from),
            muted: false,
            soloed: false,
            unread,
            run_count: 0,
        }
    }

    fn waiting(
        id: &str,
        unread: bool,
        urgency: Option<Urgency>,
        waiting_since: Option<&str>,
    ) -> SessionTab {
        tab(id, TabState::Waiting, unread, urgency, waiting_since)
    }

    fn idle(id: &str) -> SessionTab {
        tab(id, TabState::Idle, false, None, None)
    }

    fn working(id: &str) -> SessionTab {
        tab(id, TabState::Working, false, None, None)
    }

    const NOW: &str = "2026-06-08T12:00:00Z";
    const T_OLD: &str = "2026-06-08T10:00:00Z"; // 7200 s ago
    const T_MID: &str = "2026-06-08T11:00:00Z"; // 3600 s ago
    const T_NEW: &str = "2026-06-08T11:59:00Z"; // 60 s ago

    // ── urgency_sort_rank ─────────────────────────────────────────────────────

    #[test]
    fn approval_ranks_highest() {
        assert!(
            urgency_sort_rank(Some(Urgency::Approval)) > urgency_sort_rank(Some(Urgency::Question))
        );
        assert!(
            urgency_sort_rank(Some(Urgency::Question)) > urgency_sort_rank(Some(Urgency::IdleDone))
        );
        assert!(
            urgency_sort_rank(Some(Urgency::IdleDone)) > urgency_sort_rank(Some(Urgency::None))
        );
        assert!(urgency_sort_rank(Some(Urgency::None)) > urgency_sort_rank(None));
    }

    #[test]
    fn none_urgency_ranks_lowest() {
        assert_eq!(urgency_sort_rank(None), 0);
    }

    #[test]
    fn all_urgency_variants_have_unique_ranks() {
        let ranks: Vec<_> = [
            urgency_sort_rank(Some(Urgency::Approval)),
            urgency_sort_rank(Some(Urgency::Question)),
            urgency_sort_rank(Some(Urgency::IdleDone)),
            urgency_sort_rank(Some(Urgency::None)),
            urgency_sort_rank(None),
        ]
        .to_vec();
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ranks.len(), "all ranks must be distinct");
    }

    // ── waiting_age_secs ──────────────────────────────────────────────────────

    #[test]
    fn age_of_two_hours() {
        let age = waiting_age_secs(Some(T_OLD), Some(NOW));
        assert_eq!(age, 7200, "2 hours = 7200 s");
    }

    #[test]
    fn age_of_one_hour() {
        let age = waiting_age_secs(Some(T_MID), Some(NOW));
        assert_eq!(age, 3600, "1 hour = 3600 s");
    }

    #[test]
    fn age_of_sixty_seconds() {
        let age = waiting_age_secs(Some(T_NEW), Some(NOW));
        assert_eq!(age, 60);
    }

    #[test]
    fn age_zero_when_now_unknown() {
        assert_eq!(waiting_age_secs(Some(T_OLD), None), 0);
    }

    #[test]
    fn age_zero_when_since_unknown() {
        assert_eq!(waiting_age_secs(None, Some(NOW)), 0);
    }

    #[test]
    fn age_zero_when_both_unknown() {
        assert_eq!(waiting_age_secs(None, None), 0);
    }

    #[test]
    fn age_zero_on_clock_skew_now_before_since() {
        // now < since → clock skew → degrade to 0.
        assert_eq!(waiting_age_secs(Some(NOW), Some(T_OLD)), 0);
    }

    #[test]
    fn age_zero_same_instant() {
        assert_eq!(waiting_age_secs(Some(NOW), Some(NOW)), 0);
    }

    #[test]
    fn age_with_fractional_seconds_in_stamp() {
        // Fractional seconds are stripped; result should still be ~60 s.
        let since = "2026-06-08T11:59:00.500Z";
        let age = waiting_age_secs(Some(since), Some(NOW));
        // After stripping fraction: same as T_NEW = 60 s.
        assert_eq!(age, 60);
    }

    // ── sort_key ──────────────────────────────────────────────────────────────

    #[test]
    fn sort_key_unread_dominates() {
        let with_unread = waiting("a", true, Some(Urgency::IdleDone), Some(T_NEW));
        let no_unread = waiting("b", false, Some(Urgency::Approval), Some(T_OLD));
        let ka = sort_key(&with_unread, Some(NOW));
        let kb = sort_key(&no_unread, Some(NOW));
        assert!(ka > kb, "unread must beat higher urgency + older age");
    }

    #[test]
    fn sort_key_urgency_second() {
        let approval = waiting("a", false, Some(Urgency::Approval), Some(T_NEW));
        let question = waiting("b", false, Some(Urgency::Question), Some(T_OLD));
        let ka = sort_key(&approval, Some(NOW));
        let kb = sort_key(&question, Some(NOW));
        assert!(
            ka > kb,
            "approval urgency must beat question urgency even with older age"
        );
    }

    #[test]
    fn sort_key_age_third() {
        let older = waiting("a", false, Some(Urgency::Approval), Some(T_OLD));
        let newer = waiting("b", false, Some(Urgency::Approval), Some(T_NEW));
        let ka = sort_key(&older, Some(NOW));
        let kb = sort_key(&newer, Some(NOW));
        assert!(
            ka > kb,
            "older waiting_since must beat newer at same urgency"
        );
    }

    #[test]
    fn sort_key_without_clock_age_is_zero() {
        let a = waiting("a", false, Some(Urgency::Approval), Some(T_OLD));
        let b = waiting("b", false, Some(Urgency::Approval), Some(T_NEW));
        let ka = sort_key(&a, None);
        let kb = sort_key(&b, None);
        // With no clock, ages are both 0 → keys are equal.
        assert_eq!(ka, kb);
    }

    // ── sort_tabs — core ordering invariants ──────────────────────────────────

    #[test]
    fn waiting_sorts_before_non_waiting() {
        let mut tabs = vec![
            idle("idle"),
            waiting("w", false, Some(Urgency::Approval), Some(T_MID)),
            working("work"),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "w", "waiting must be first");
    }

    #[test]
    fn unread_sorts_before_same_urgency_no_unread() {
        let mut tabs = vec![
            waiting("no-unread", false, Some(Urgency::Approval), Some(T_OLD)),
            waiting("unread", true, Some(Urgency::Approval), Some(T_NEW)),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "unread");
        assert_eq!(tabs[1].session_id, "no-unread");
    }

    #[test]
    fn approval_before_question_before_idle_done() {
        let mut tabs = vec![
            waiting("q", false, Some(Urgency::Question), Some(T_OLD)),
            waiting("i", false, Some(Urgency::IdleDone), Some(T_OLD)),
            waiting("a", false, Some(Urgency::Approval), Some(T_OLD)),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "a");
        assert_eq!(tabs[1].session_id, "q");
        assert_eq!(tabs[2].session_id, "i");
    }

    #[test]
    fn older_wait_before_newer_at_same_urgency() {
        let mut tabs = vec![
            waiting("new", false, Some(Urgency::Approval), Some(T_NEW)),
            waiting("old", false, Some(Urgency::Approval), Some(T_OLD)),
            waiting("mid", false, Some(Urgency::Approval), Some(T_MID)),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "old");
        assert_eq!(tabs[1].session_id, "mid");
        assert_eq!(tabs[2].session_id, "new");
    }

    #[test]
    fn unread_beats_urgency_and_age() {
        // Unread + idle-done urgency + young age must beat read + approval + old age.
        let mut tabs = vec![
            waiting("high-old", false, Some(Urgency::Approval), Some(T_OLD)),
            waiting("unread-new", true, Some(Urgency::IdleDone), Some(T_NEW)),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "unread-new");
        assert_eq!(tabs[1].session_id, "high-old");
    }

    #[test]
    fn all_non_waiting_retain_relative_order() {
        // Non-waiting tabs with no urgency/unread all have the same key → stable.
        let mut tabs = vec![idle("a"), idle("b"), idle("c"), working("d")];
        sort_tabs(&mut tabs, Some(NOW));
        // All have key (false, 0, 0) → stable sort keeps original order.
        let ids: Vec<_> = tabs.iter().map(|t| t.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn mixed_states_full_ordering() {
        // Build a mixed inbox and assert the full expected order.
        let mut tabs = vec![
            idle("idle1"),
            waiting(
                "w-approval-old-unread",
                true,
                Some(Urgency::Approval),
                Some(T_OLD),
            ),
            working("work1"),
            waiting(
                "w-approval-new",
                false,
                Some(Urgency::Approval),
                Some(T_NEW),
            ),
            waiting("w-question", false, Some(Urgency::Question), Some(T_MID)),
            waiting(
                "w-approval-old",
                false,
                Some(Urgency::Approval),
                Some(T_OLD),
            ),
            idle("idle2"),
        ];
        sort_tabs(&mut tabs, Some(NOW));
        // Expected order:
        // 1. unread + approval + old  (highest: unread=true beats everything)
        // 2. no-unread + approval + old  (no-unread but same urgency + oldest)
        // 3. no-unread + approval + new  (same urgency, younger)
        // 4. no-unread + question + mid  (lower urgency)
        // 5–7. idle1, work1, idle2  (non-waiting, original relative order)
        assert_eq!(tabs[0].session_id, "w-approval-old-unread");
        assert_eq!(tabs[1].session_id, "w-approval-old");
        assert_eq!(tabs[2].session_id, "w-approval-new");
        assert_eq!(tabs[3].session_id, "w-question");
        // Non-waiting retain stable original order: idle1, work1, idle2.
        assert_eq!(tabs[4].session_id, "idle1");
        assert_eq!(tabs[5].session_id, "work1");
        assert_eq!(tabs[6].session_id, "idle2");
    }

    #[test]
    fn sort_is_stable_on_equal_keys() {
        // Tabs with identical sort keys keep their original relative order.
        let mut tabs: Vec<SessionTab> = (0..5)
            .map(|i| waiting(&format!("w{i}"), false, Some(Urgency::Question), None))
            .collect();
        sort_tabs(&mut tabs, None); // no clock → all ages = 0 → all keys equal
        let ids: Vec<_> = tabs.iter().map(|t| t.session_id.as_str()).collect();
        assert_eq!(ids, vec!["w0", "w1", "w2", "w3", "w4"]);
    }

    #[test]
    fn no_clock_urgency_still_governs() {
        // Without a clock, urgency is still the tiebreaker.
        let mut tabs = vec![
            waiting("q", false, Some(Urgency::Question), Some(T_MID)),
            waiting("a", false, Some(Urgency::Approval), Some(T_NEW)),
        ];
        sort_tabs(&mut tabs, None);
        assert_eq!(tabs[0].session_id, "a");
        assert_eq!(tabs[1].session_id, "q");
    }

    #[test]
    fn empty_tabs_does_not_panic() {
        let mut tabs: Vec<SessionTab> = vec![];
        sort_tabs(&mut tabs, Some(NOW)); // must not panic
        assert!(tabs.is_empty());
    }

    #[test]
    fn single_tab_unchanged() {
        let mut tabs = vec![waiting("w", false, Some(Urgency::Approval), Some(T_MID))];
        sort_tabs(&mut tabs, Some(NOW));
        assert_eq!(tabs[0].session_id, "w");
    }

    // ── sort_tab_refs ─────────────────────────────────────────────────────────

    #[test]
    fn sort_tab_refs_matches_sort_tabs() {
        let owned = [
            idle("a"),
            waiting("b", true, Some(Urgency::Approval), Some(T_OLD)),
            working("c"),
        ];
        let mut by_ref: Vec<&SessionTab> = owned.iter().collect();
        sort_tab_refs(&mut by_ref, Some(NOW));
        assert_eq!(by_ref[0].session_id, "b");
        // The original `owned` vec is unmodified (refs only moved).
        assert_eq!(owned[0].session_id, "a");
    }

    // ── exhaustive ordering across all state × urgency × age combos ──────────

    #[test]
    fn unread_always_beats_non_unread_for_all_urgency_pairs() {
        for u_with_unread in [
            None,
            Some(Urgency::None),
            Some(Urgency::IdleDone),
            Some(Urgency::Question),
            Some(Urgency::Approval),
        ] {
            for u_without in [
                None,
                Some(Urgency::None),
                Some(Urgency::IdleDone),
                Some(Urgency::Question),
                Some(Urgency::Approval),
            ] {
                let with_unread = tab("u", TabState::Waiting, true, u_with_unread, Some(T_NEW));
                let without_unread = tab("n", TabState::Waiting, false, u_without, Some(T_OLD));
                let ku = sort_key(&with_unread, Some(NOW));
                let kn = sort_key(&without_unread, Some(NOW));
                assert!(
                    ku > kn,
                    "unread (urgency={u_with_unread:?}) must beat non-unread (urgency={u_without:?})"
                );
            }
        }
    }

    #[test]
    fn urgency_order_is_total_and_transitive() {
        // For every pair (a, b) of distinct urgencies, verify that the pair with
        // higher rank always sorts first.
        let urgencies = [
            None,
            Some(Urgency::None),
            Some(Urgency::IdleDone),
            Some(Urgency::Question),
            Some(Urgency::Approval),
        ];
        for (i, &ua) in urgencies.iter().enumerate() {
            for (j, &ub) in urgencies.iter().enumerate() {
                let ra = urgency_sort_rank(ua);
                let rb = urgency_sort_rank(ub);
                match ra.cmp(&rb) {
                    std::cmp::Ordering::Greater => {
                        // ua should sort before ub at equal unread and age.
                        let ta = tab("a", TabState::Waiting, false, ua, None);
                        let tb = tab("b", TabState::Waiting, false, ub, None);
                        assert!(
                            sort_key(&ta, None) > sort_key(&tb, None),
                            "urgency[{i}]={ua:?} (rank {ra}) must beat urgency[{j}]={ub:?} (rank {rb})"
                        );
                    }
                    std::cmp::Ordering::Less => {
                        let ta = tab("a", TabState::Waiting, false, ua, None);
                        let tb = tab("b", TabState::Waiting, false, ub, None);
                        assert!(
                            sort_key(&ta, None) < sort_key(&tb, None),
                            "urgency[{i}]={ua:?} (rank {ra}) must lose to urgency[{j}]={ub:?} (rank {rb})"
                        );
                    }
                    std::cmp::Ordering::Equal => {
                        // Same rank → same key component.
                        let ta = tab("a", TabState::Waiting, false, ua, None);
                        let tb = tab("b", TabState::Waiting, false, ub, None);
                        assert_eq!(
                            sort_key(&ta, None).1,
                            sort_key(&tb, None).1,
                            "equal urgency ranks must have equal sort-key component"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn older_age_always_beats_younger_at_equal_urgency_and_unread() {
        let ages = [T_OLD, T_MID, T_NEW];
        for i in 0..ages.len() {
            for j in (i + 1)..ages.len() {
                // ages[i] is older than ages[j] (earlier timestamp)
                let older = tab(
                    "old",
                    TabState::Waiting,
                    false,
                    Some(Urgency::Approval),
                    Some(ages[i]),
                );
                let newer = tab(
                    "new",
                    TabState::Waiting,
                    false,
                    Some(Urgency::Approval),
                    Some(ages[j]),
                );
                let ko = sort_key(&older, Some(NOW));
                let kn = sort_key(&newer, Some(NOW));
                assert!(ko > kn, "{} (older) must beat {} (newer)", ages[i], ages[j]);
            }
        }
    }

    // ── parse_iso8601_secs edge cases ─────────────────────────────────────────

    #[test]
    fn malformed_stamp_returns_zero_age() {
        // A malformed stamp must produce 0 age, not a panic.
        assert_eq!(waiting_age_secs(Some("not-a-date"), Some(NOW)), 0);
        assert_eq!(waiting_age_secs(Some(""), Some(NOW)), 0);
        assert_eq!(waiting_age_secs(Some(NOW), Some("bad")), 0);
    }

    #[test]
    fn age_across_day_boundary() {
        // 2026-06-07T23:00:00Z to 2026-06-08T01:00:00Z = 2 hours = 7200 s
        let since = "2026-06-07T23:00:00Z";
        let now = "2026-06-08T01:00:00Z";
        assert_eq!(waiting_age_secs(Some(since), Some(now)), 7200);
    }
}

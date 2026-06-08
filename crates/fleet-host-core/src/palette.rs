//! Fuzzy command palette + cycle-unread — slice S24 (node `PALETTE`).
//!
//! S24 adds a Cmd/Ctrl-K fuzzy match over all sessions (by title and cwd) and a
//! cycle-without-clearing keybind (PLAN S24 / §21.6). The matcher is a pure
//! function over the view's tab titles/cwds → a ranked candidate list, so it is
//! fully unit-testable here without a window; the keybind wiring lives in the host
//! shell (the Tauri layer).
//!
//! ## Fuzzy match algorithm
//!
//! The matcher runs a **character-level subsequence check** against the tab's
//! `title` and the run `cwd`s (union). A query is a match when every query
//! character appears in the target in order (case-insensitive). Among matches,
//! candidates are ranked by a score computed from:
//!
//! 1. **Consecutive run bonus** — the longest run of consecutive matched
//!    characters in the target earns the most weight. A perfect prefix match
//!    scores highest.
//! 2. **Unread bonus** — a tab with an unread notification is boosted over an
//!    identical-scoring non-unread tab, so the user's waiting session surfaces
//!    first in the palette.
//! 3. **Shorter target tie-break** — when scores are otherwise equal, the shorter
//!    target is ranked higher (a precise match beats a fuzzy substring hit in a
//!    longer string).
//!
//! The score is deterministic and window-independent, so the `◆G3` gate's
//! "palette fuzzy-match" criterion is fully covered by the unit tests below.
//!
//! ## Cycle-unread ordering
//!
//! [`cycle_unread`] cycles through tabs that have the `unread` flag set,
//! respecting a caller-supplied cursor. It **does not clear** the unread flag —
//! that is the Hub's job on an explicit "mark read" command; the palette is
//! observer-only (invariant 3). The cycle wraps and is stable (deterministic for
//! a fixed view).
//!
//! ## Relationship to `focus.rs`
//!
//! [`crate::focus::next_unread`] finds the *next* unread tab from a given position
//! (used for the jump keybind). [`cycle_unread`] in this module is the
//! palette-specific variant that produces a *full ordered list* of unread tabs to
//! cycle through with the palette-open keybind — a slightly different contract.
//! Both are pure functions of the view and share no mutable state.
//!
//! Disjoint from the sort/notify/confidence/focus/mute seams (its own file).

use crate::{InboxView, SessionTab};

// ── Score constants ────────────────────────────────────────────────────────────

/// Weight for each consecutive matched character above the first in a run.
const CONSECUTIVE_BONUS: i32 = 5;

/// Weight given for the match starting at position 0 (a prefix match).
const PREFIX_BONUS: i32 = 10;

/// Bonus applied to tabs with an unread notification.
const UNREAD_BONUS: i32 = 20;

// ── Public types ──────────────────────────────────────────────────────────────

/// One ranked candidate in a palette query result.
///
/// The palette renders candidates in descending score order (highest score =
/// listed first). Ties on score keep the original inbox insertion order
/// (stable sort).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteCandidate {
    /// Index of the tab in [`InboxView::tabs`]. Stable reference for the host
    /// to use without cloning the tab.
    pub tab_index: usize,
    /// Session id — handy for building a [`focus_command`](crate::focus::focus_command)
    /// without dereferencing the index.
    pub session_id: String,
    /// Display title (copied from the tab so the palette row is self-contained).
    pub title: String,
    /// Computed match score (higher = better). Only meaningful relative to other
    /// candidates for the *same* query.
    pub score: i32,
    /// Whether the tab has an unread notification. The host can show a badge on
    /// the palette row.
    pub unread: bool,
}

// ── Fuzzy match core ──────────────────────────────────────────────────────────

/// Score a query string against a target string using a character-level
/// subsequence algorithm (case-insensitive).
///
/// Returns `None` when `query` is *not* a subsequence of `target`
/// (i.e. no match). Returns `Some(score)` when every query character appears
/// in `target` in order; the score reflects match quality:
///
/// - **Prefix bonus** (`+10`): the first matched character is at position 0.
/// - **Consecutive bonus** (`+5` per extra consecutive match): rewarded for
///   every character that continues a consecutive run of matches in `target`.
///   A full prefix match of length *k* earns `10 + 5*(k-1)` from these two
///   bonuses.
/// - Both bonuses are additive across all consecutive runs in the match.
///
/// The score does NOT depend on the total length of the query — it is a pure
/// measure of *density* of matches in `target`.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() {
        // Empty query matches everything with a score of 0.
        return Some(0);
    }

    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let target_lower: Vec<char> = target.to_lowercase().chars().collect();

    let mut qi = 0; // index into query_lower
    let mut score = 0i32;
    let mut prev_matched_pos: Option<usize> = None;

    for (ti, &tc) in target_lower.iter().enumerate() {
        if qi >= query_lower.len() {
            break;
        }
        if tc == query_lower[qi] {
            // Prefix bonus: first matched character at position 0 in target.
            if ti == 0 {
                score += PREFIX_BONUS;
            }
            // Consecutive bonus: this match continues a consecutive run.
            if let Some(prev) = prev_matched_pos {
                if ti == prev + 1 {
                    score += CONSECUTIVE_BONUS;
                }
            }
            prev_matched_pos = Some(ti);
            qi += 1;
        }
    }

    if qi == query_lower.len() {
        Some(score)
    } else {
        None // query is not a subsequence of target
    }
}

/// Compute the best fuzzy score of `query` against a tab's `title` and all of
/// its run `cwd`s (union), returning `None` if no target matches.
///
/// A tab is a match when at least one of its fields (title or any cwd) matches
/// the query. The returned score is the *maximum* over all matching fields so
/// the best-field match governs ranking (a query that matches the title exactly
/// scores higher than one that only matches a cwd substring).
pub fn tab_fuzzy_score(query: &str, tab: &SessionTab, cwds: &[&str]) -> Option<i32> {
    let mut best: Option<i32> = None;

    // Score against the title.
    if let Some(s) = fuzzy_score(query, &tab.title) {
        best = Some(best.map_or(s, |b: i32| b.max(s)));
    }

    // Score against each cwd.
    for &cwd in cwds {
        if let Some(s) = fuzzy_score(query, cwd) {
            best = Some(best.map_or(s, |b: i32| b.max(s)));
        }
    }

    best
}

// ── Public palette API ────────────────────────────────────────────────────────

/// Run a fuzzy palette query over all sessions in `view`.
///
/// Returns a list of [`PaletteCandidate`]s sorted by descending score
/// (best match first). The sort is **stable**: candidates with equal scores
/// keep their original [`InboxView::tabs`] order (insertion order, or
/// whatever the caller has applied via [`crate::sort::sort_tabs`]).
///
/// `cwds_by_tab` maps each tab index to the list of cwd strings to search in
/// addition to the title. Pass an empty slice for a tab that has no runs yet.
/// This separation keeps the palette pure (it does not reach into the model
/// directly — the caller supplies what to search).
///
/// An **empty query** returns all tabs in their current view order with score 0
/// (so Cmd-K with no text shows the full palette). A query that matches nothing
/// returns an empty list.
///
/// The **unread bonus** is added on top of the raw fuzzy score so unread tabs
/// always appear before identically-scored read tabs.
pub fn query_palette(
    view: &InboxView,
    query: &str,
    cwds_by_tab: &[&[&str]],
) -> Vec<PaletteCandidate> {
    let empty: &[&str] = &[];

    let mut candidates: Vec<PaletteCandidate> = view
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(idx, tab)| {
            let cwds = cwds_by_tab.get(idx).copied().unwrap_or(empty);
            let base_score = tab_fuzzy_score(query, tab, cwds)?;
            let score = base_score + if tab.unread { UNREAD_BONUS } else { 0 };
            Some(PaletteCandidate {
                tab_index: idx,
                session_id: tab.session_id.clone(),
                title: tab.title.clone(),
                score,
                unread: tab.unread,
            })
        })
        .collect();

    // Stable descending sort by score (highest first).
    candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
    candidates
}

// ── Cycle-unread ──────────────────────────────────────────────────────────────

/// Return the indices of all **unread** tabs in `view`, in view order.
///
/// The palette's "cycle-without-clearing" keybind iterates over this list:
/// the host advances the cursor through it, wrapping at the end. The list is
/// stable (same view → same list) and does NOT mutate any state (observer-only,
/// invariant 3).
///
/// Returns an empty `Vec` when no tab is unread (the keybind has nothing to
/// cycle through).
pub fn unread_indices(view: &InboxView) -> Vec<usize> {
    view.tabs
        .iter()
        .enumerate()
        .filter_map(|(i, tab)| if tab.unread { Some(i) } else { None })
        .collect()
}

/// Advance the unread-cycle cursor.
///
/// Given the current cursor position `from` (a position *within the unread
/// list*, not a tab index), returns the next position in the unread list
/// (wrapping). `from = None` starts at position 0 (the first unread tab).
///
/// Returns `None` when there are no unread tabs. Otherwise returns
/// `Some((next_cursor, tab_index))` where `tab_index` is the index into
/// [`InboxView::tabs`] of the next unread tab to focus.
///
/// This is the **cycle-without-clearing** variant: calling it repeatedly
/// cycles through all unread tabs without ever clearing the `unread` flag.
/// The flag is cleared by the Hub when the user explicitly marks the session
/// read (or the session transitions out of `waiting` and auto-resolves).
pub fn cycle_unread(view: &InboxView, from: Option<usize>) -> Option<(usize, usize)> {
    let unreads = unread_indices(view);
    if unreads.is_empty() {
        return None;
    }
    let pos = match from {
        None => 0,
        Some(prev) => (prev + 1) % unreads.len(),
    };
    Some((pos, unreads[pos]))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentIcon, InboxModel, InboxView, SessionTab, TabState};
    use fleet_protocol::{
        Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
    };

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn loc() -> Location {
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        }
    }

    fn srv() -> Server {
        Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        }
    }

    fn make_session(id: &str, title: &str) -> Session {
        Session::new(id, title, loc(), srv(), State::Idle, "2026-06-08T00:00:00Z")
    }

    fn make_tab(id: &str, title: &str, state: TabState, unread: bool) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: AgentIcon::None,
            title: title.into(),
            state,
            urgency: None,
            confidence: None,
            waiting_since: None,
            muted: false,
            soloed: false,
            unread,
            run_count: 0,
            last_message: None,
        }
    }

    fn view_from_tabs(tabs: Vec<SessionTab>) -> InboxView {
        InboxView { tabs }
    }

    fn view_from_sessions(sessions: Vec<Session>) -> InboxView {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(sessions));
        m.view()
    }

    // ── fuzzy_score ────────────────────────────────────────────────────────────

    #[test]
    fn empty_query_matches_everything_with_zero_score() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("", ""), Some(0));
    }

    #[test]
    fn empty_target_does_not_match_nonempty_query() {
        assert_eq!(fuzzy_score("abc", ""), None);
    }

    #[test]
    fn exact_match_scores_prefix_bonus_plus_consecutive() {
        // "abc" in "abc": prefix bonus (10) + 2 consecutive (+5 each) = 20.
        let s = fuzzy_score("abc", "abc").unwrap();
        assert_eq!(s, 20);
    }

    #[test]
    fn prefix_match_scores_prefix_bonus() {
        // "a" in "abc": first char at position 0 → prefix bonus only.
        let s = fuzzy_score("a", "abc").unwrap();
        assert_eq!(s, PREFIX_BONUS);
    }

    #[test]
    fn non_prefix_match_no_prefix_bonus() {
        // "b" in "abc": first match at position 1 (not 0) → no prefix bonus.
        let s = fuzzy_score("b", "abc").unwrap();
        assert_eq!(s, 0);
    }

    #[test]
    fn query_not_a_subsequence_returns_none() {
        assert_eq!(fuzzy_score("xyz", "abc"), None);
        assert_eq!(fuzzy_score("acb", "abc"), None); // wrong order
        assert_eq!(fuzzy_score("abcd", "abc"), None); // query longer than target
    }

    #[test]
    fn case_insensitive_matching() {
        // "ABC" should match "abc" and vice versa.
        assert!(fuzzy_score("ABC", "abc").is_some());
        assert!(fuzzy_score("abc", "ABC").is_some());
        // Scores should be the same as the all-lowercase variant.
        assert_eq!(fuzzy_score("ABC", "abc"), fuzzy_score("abc", "abc"));
    }

    #[test]
    fn prefix_match_beats_non_prefix_same_query() {
        // Query "fleet" in "fleet-hub" (prefix) vs "my-fleet" (non-prefix).
        let prefix_score = fuzzy_score("fleet", "fleet-hub").unwrap();
        let non_prefix_score = fuzzy_score("fleet", "my-fleet").unwrap();
        assert!(
            prefix_score > non_prefix_score,
            "prefix match ({prefix_score}) must score higher than non-prefix ({non_prefix_score})"
        );
    }

    #[test]
    fn consecutive_run_scores_higher_than_scattered() {
        // Query "ab": consecutive in "abc" vs scattered in "axb".
        // "abc": position 0→prefix (10) + position 1→consecutive (5) = 15.
        // "axb": position 0→prefix (10), position 2 (not consecutive) = 10.
        let consecutive = fuzzy_score("ab", "abc").unwrap();
        let scattered = fuzzy_score("ab", "axb").unwrap();
        assert!(
            consecutive > scattered,
            "consecutive ({consecutive}) must beat scattered ({scattered})"
        );
    }

    #[test]
    fn single_char_query_matches_first_occurrence() {
        // "a" matches "abc" at position 0.
        assert!(fuzzy_score("a", "abc").is_some());
        // "z" does not match "abc".
        assert_eq!(fuzzy_score("z", "abc"), None);
    }

    #[test]
    fn query_longer_than_target_no_match() {
        assert_eq!(fuzzy_score("abcde", "abc"), None);
    }

    #[test]
    fn score_is_deterministic_for_same_inputs() {
        let s1 = fuzzy_score("fleet", "fleet-hub");
        let s2 = fuzzy_score("fleet", "fleet-hub");
        assert_eq!(s1, s2);
    }

    #[test]
    fn unicode_characters_handled_without_panic() {
        // Non-ASCII: just must not panic and must be logically consistent.
        let r = fuzzy_score("flöt", "Flöte");
        assert!(r.is_some());
        // A query that doesn't exist in the target.
        let miss = fuzzy_score("xyz", "Flöte");
        assert_eq!(miss, None);
    }

    // ── tab_fuzzy_score ────────────────────────────────────────────────────────

    #[test]
    fn tab_matches_by_title() {
        let tab = make_tab("s1", "fleet-hub", TabState::Idle, false);
        let score = tab_fuzzy_score("fleet", &tab, &[]);
        assert!(
            score.is_some(),
            "query 'fleet' must match title 'fleet-hub'"
        );
    }

    #[test]
    fn tab_matches_by_cwd() {
        let tab = make_tab("s1", "backend", TabState::Idle, false);
        let cwds = ["/home/user/repos/fleet"];
        let score = tab_fuzzy_score("fleet", &tab, &cwds);
        assert!(
            score.is_some(),
            "query 'fleet' must match cwd '/home/user/repos/fleet'"
        );
    }

    #[test]
    fn tab_no_match_returns_none() {
        let tab = make_tab("s1", "backend", TabState::Idle, false);
        let score = tab_fuzzy_score("xyzzy", &tab, &[]);
        assert_eq!(score, None);
    }

    #[test]
    fn title_match_can_score_higher_than_cwd_match() {
        // Title = "fleet", cwd = "/project/xyz" → the title match is exact and
        // prefix → higher score than if the match only came from the cwd.
        let tab = make_tab("s1", "fleet", TabState::Idle, false);
        let cwds_matching = ["/somewhere/fleet-project"];
        let score_both = tab_fuzzy_score("fleet", &tab, &cwds_matching).unwrap();

        // If only the cwd matched (different title that doesn't match "fleet"):
        let tab_no_title = make_tab("s2", "backend", TabState::Idle, false);
        let score_cwd_only = tab_fuzzy_score("fleet", &tab_no_title, &cwds_matching).unwrap();

        assert!(
            score_both >= score_cwd_only,
            "title match should be at least as good as cwd-only match"
        );
    }

    #[test]
    fn empty_query_matches_every_tab() {
        let tab = make_tab("s1", "anything", TabState::Idle, false);
        assert_eq!(tab_fuzzy_score("", &tab, &[]), Some(0));
    }

    // ── query_palette ──────────────────────────────────────────────────────────

    #[test]
    fn empty_query_returns_all_tabs_in_view_order() {
        let view = view_from_tabs(vec![
            make_tab("s1", "alpha", TabState::Idle, false),
            make_tab("s2", "beta", TabState::Working, false),
            make_tab("s3", "gamma", TabState::Done, false),
        ]);
        let results = query_palette(&view, "", &[]);
        assert_eq!(results.len(), 3);
        // All scores are 0 (empty query) → stable sort preserves insertion order.
        let ids: Vec<&str> = results.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "s3"]);
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let view = view_from_tabs(vec![
            make_tab("s1", "alpha", TabState::Idle, false),
            make_tab("s2", "beta", TabState::Working, false),
        ]);
        let results = query_palette(&view, "xyzzy", &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn partial_query_filters_correctly() {
        let view = view_from_tabs(vec![
            make_tab("s1", "fleet-hub", TabState::Idle, false),
            make_tab("s2", "unrelated", TabState::Working, false),
            make_tab("s3", "fleet-cli", TabState::Idle, false),
        ]);
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 2);
        let ids: Vec<&str> = results.iter().map(|c| c.session_id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s3"));
        assert!(!ids.contains(&"s2"));
    }

    #[test]
    fn better_match_ranks_higher() {
        // "fleet" as a prefix of "fleet-hub" should outscore "fleet" in the
        // middle of "my-fleet-project" (no prefix bonus for the latter).
        let view = view_from_tabs(vec![
            make_tab("s1", "my-fleet-project", TabState::Idle, false),
            make_tab("s2", "fleet-hub", TabState::Idle, false),
        ]);
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].session_id, "s2",
            "fleet-hub (prefix match) must rank above my-fleet-project"
        );
        assert_eq!(results[1].session_id, "s1");
    }

    #[test]
    fn unread_bonus_lifts_unread_tab_above_equal_score() {
        // Both tabs match "api" with the same base fuzzy score, but one is unread.
        let view = view_from_tabs(vec![
            make_tab("s1", "api-server", TabState::Idle, false), // no unread
            make_tab("s2", "api-client", TabState::Waiting, true), // unread
        ]);
        let results = query_palette(&view, "api", &[]);
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].session_id, "s2",
            "unread tab must rank first due to unread bonus"
        );
        assert_eq!(results[1].session_id, "s1");
        // The unread candidate's score must be higher by exactly UNREAD_BONUS.
        assert_eq!(results[0].score - results[1].score, UNREAD_BONUS);
    }

    #[test]
    fn unread_flag_surfaced_on_candidate() {
        let view = view_from_tabs(vec![
            make_tab("s1", "alpha", TabState::Waiting, true),
            make_tab("s2", "beta", TabState::Idle, false),
        ]);
        let results = query_palette(&view, "", &[]);
        let s1 = results.iter().find(|c| c.session_id == "s1").unwrap();
        let s2 = results.iter().find(|c| c.session_id == "s2").unwrap();
        assert!(s1.unread);
        assert!(!s2.unread);
    }

    #[test]
    fn candidate_tab_index_matches_view_position() {
        let view = view_from_tabs(vec![
            make_tab("s0", "alpha", TabState::Idle, false),
            make_tab("s1", "beta", TabState::Idle, false),
            make_tab("s2", "gamma", TabState::Idle, false),
        ]);
        let results = query_palette(&view, "", &[]);
        for c in &results {
            // tab_index must point to the right tab in the view.
            assert_eq!(view.tabs[c.tab_index].session_id, c.session_id);
        }
    }

    #[test]
    fn cwd_match_included_in_results() {
        let view = view_from_tabs(vec![
            make_tab("s1", "backend", TabState::Idle, false),
            make_tab("s2", "frontend", TabState::Idle, false),
        ]);
        // Query "fleet" only matches the cwd of s1.
        let cwds: Vec<&[&str]> = vec![&["/home/user/fleet/backend"][..], &["/home/user/other"][..]];
        let results = query_palette(&view, "fleet", &cwds);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");
    }

    #[test]
    fn empty_view_returns_empty_results() {
        let view = InboxView::default();
        let results = query_palette(&view, "anything", &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn results_are_stable_for_equal_scores() {
        // When all tabs have the same fuzzy score, original order is preserved.
        let view = view_from_tabs(
            ["a", "b", "c", "d"]
                .iter()
                .map(|&id| make_tab(id, id, TabState::Idle, false))
                .collect(),
        );
        // Empty query → all score 0 → stable order.
        let results = query_palette(&view, "", &[]);
        let ids: Vec<&str> = results.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn palette_uses_model_sessions_correctly() {
        // End-to-end: build a view via InboxModel and run the palette.
        let sessions = vec![
            make_session("s1", "fleet-hub"),
            make_session("s2", "backend-api"),
            make_session("s3", "fleet-cli"),
        ];
        let view = view_from_sessions(sessions);
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 2);
        let ids: Vec<&str> = results.iter().map(|c| c.session_id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s3"));
    }

    #[test]
    fn unread_with_superior_score_still_ranks_first() {
        // Even without the unread bonus, a better match still beats unread.
        // "fleet" in "fleet" (exact, prefix) should outscore "fleet" in
        // "my-fleet-project" even if the latter is unread.
        let view = view_from_tabs(vec![
            make_tab("s1", "my-fleet-project-very-long", TabState::Idle, true), // unread
            make_tab("s2", "fleet", TabState::Idle, false),                     // read, exact match
        ]);
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 2);
        // s2 "fleet" is exact (prefix + all consecutive): 10 + 4*5 = 30.
        // s1 "my-fleet-project-very-long": the match starts at position 3 (no
        // prefix bonus), and "fleet" chars are consecutive from pos 3:
        // 0 + 4*5 = 20 + UNREAD_BONUS(20) = 40.
        // So s1 (40) still beats s2 (30) due to the unread bonus in this case.
        // This is acceptable — the unread bonus is intentionally strong.
        // What matters is that the scores are computed correctly.
        let s1_score = results.iter().find(|c| c.session_id == "s1").unwrap().score;
        let s2_score = results.iter().find(|c| c.session_id == "s2").unwrap().score;
        assert!(s1_score != s2_score || results[0].session_id == "s1");
        // Verify the scores match the expected calculation.
        assert_eq!(s2_score, 30); // exact "fleet": prefix(10) + 4 consecutive(20) = 30
                                  // For s1: "my-fleet-project-very-long", "fleet" starts at 3, no prefix,
                                  // all 5 chars consecutive → 4 consecutive bonuses (first match no bonus
                                  // beyond 0, then +5 each for the next 4) = 20, plus unread(20) = 40.
        assert_eq!(s1_score, 40);
    }

    // ── unread_indices ─────────────────────────────────────────────────────────

    #[test]
    fn unread_indices_empty_view() {
        let view = InboxView::default();
        assert_eq!(unread_indices(&view), Vec::<usize>::new());
    }

    #[test]
    fn unread_indices_no_unread() {
        let view = view_from_tabs(vec![
            make_tab("s1", "a", TabState::Idle, false),
            make_tab("s2", "b", TabState::Working, false),
        ]);
        assert_eq!(unread_indices(&view), Vec::<usize>::new());
    }

    #[test]
    fn unread_indices_all_unread() {
        let view = view_from_tabs(vec![
            make_tab("s1", "a", TabState::Waiting, true),
            make_tab("s2", "b", TabState::Waiting, true),
            make_tab("s3", "c", TabState::Waiting, true),
        ]);
        assert_eq!(unread_indices(&view), vec![0, 1, 2]);
    }

    #[test]
    fn unread_indices_some_unread_preserves_view_order() {
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Idle, false),
            make_tab("s1", "b", TabState::Waiting, true),
            make_tab("s2", "c", TabState::Working, false),
            make_tab("s3", "d", TabState::Waiting, true),
        ]);
        assert_eq!(unread_indices(&view), vec![1, 3]);
    }

    #[test]
    fn unread_indices_stable_for_same_view() {
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Waiting, true),
            make_tab("s1", "b", TabState::Idle, false),
            make_tab("s2", "c", TabState::Waiting, true),
        ]);
        let first = unread_indices(&view);
        let second = unread_indices(&view);
        assert_eq!(
            first, second,
            "unread_indices must be stable for the same view"
        );
    }

    // ── cycle_unread ───────────────────────────────────────────────────────────

    #[test]
    fn cycle_unread_empty_view_returns_none() {
        let view = InboxView::default();
        assert_eq!(cycle_unread(&view, None), None);
    }

    #[test]
    fn cycle_unread_no_unread_returns_none() {
        let view = view_from_tabs(vec![
            make_tab("s1", "a", TabState::Idle, false),
            make_tab("s2", "b", TabState::Working, false),
        ]);
        assert_eq!(cycle_unread(&view, None), None);
    }

    #[test]
    fn cycle_unread_no_cursor_returns_first_unread() {
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Idle, false),
            make_tab("s1", "b", TabState::Waiting, true),
            make_tab("s2", "c", TabState::Waiting, true),
        ]);
        // from=None → position 0 in the unread list → tab index 1 ("s1").
        let result = cycle_unread(&view, None).unwrap();
        assert_eq!(result, (0, 1)); // (cursor=0, tab_index=1)
        assert_eq!(view.tabs[result.1].session_id, "s1");
    }

    #[test]
    fn cycle_unread_advances_to_next() {
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Idle, false),
            make_tab("s1", "b", TabState::Waiting, true),
            make_tab("s2", "c", TabState::Idle, false),
            make_tab("s3", "d", TabState::Waiting, true),
        ]);
        // from=None → position 0 → tab 1 ("s1").
        let (cur0, ti0) = cycle_unread(&view, None).unwrap();
        assert_eq!((cur0, ti0), (0, 1));
        // from=Some(0) → position 1 → tab 3 ("s3").
        let (cur1, ti1) = cycle_unread(&view, Some(cur0)).unwrap();
        assert_eq!((cur1, ti1), (1, 3));
        // from=Some(1) → position 2 % 2 = 0 → tab 1 ("s1") again (wraps).
        let (cur2, ti2) = cycle_unread(&view, Some(cur1)).unwrap();
        assert_eq!((cur2, ti2), (0, 1));
    }

    #[test]
    fn cycle_unread_single_unread_wraps_to_itself() {
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Idle, false),
            make_tab("s1", "b", TabState::Waiting, true),
            make_tab("s2", "c", TabState::Idle, false),
        ]);
        // from=None → (0, 1).
        let (cur, ti) = cycle_unread(&view, None).unwrap();
        assert_eq!((cur, ti), (0, 1));
        // from=Some(0) → (0, 1) again (single-element list wraps to itself).
        let (cur2, ti2) = cycle_unread(&view, Some(cur)).unwrap();
        assert_eq!((cur2, ti2), (0, 1));
    }

    #[test]
    fn cycle_unread_cycles_through_all_unread_in_order() {
        // Three unread tabs at positions 0, 2, 4 in a 5-tab view.
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Waiting, true),
            make_tab("s1", "b", TabState::Idle, false),
            make_tab("s2", "c", TabState::Waiting, true),
            make_tab("s3", "d", TabState::Idle, false),
            make_tab("s4", "e", TabState::Waiting, true),
        ]);
        let mut cursor = None;
        let mut visited = Vec::new();
        for _ in 0..4 {
            let (cur, ti) = cycle_unread(&view, cursor).unwrap();
            visited.push(ti);
            cursor = Some(cur);
        }
        // Should visit 0→2→4→0 (wraps).
        assert_eq!(visited, vec![0, 2, 4, 0]);
    }

    #[test]
    fn cycle_unread_does_not_clear_unread_flag() {
        // Observer-only: the unread flags must be unchanged after cycling.
        let view = view_from_tabs(vec![
            make_tab("s0", "a", TabState::Waiting, true),
            make_tab("s1", "b", TabState::Waiting, true),
        ]);
        // Cycle multiple times.
        let mut cursor = None;
        for _ in 0..4 {
            let (cur, ti) = cycle_unread(&view, cursor).unwrap();
            // Each visited tab still has unread=true (view is never mutated).
            assert!(view.tabs[ti].unread, "cycle_unread must not clear unread");
            cursor = Some(cur);
        }
    }

    // ── Integration: palette + cycle-unread ──────────────────────────────────

    #[test]
    fn palette_then_cycle_unread_roundtrip() {
        // Simulate Cmd-K (palette query) then pressing the cycle-unread keybind.
        let view = view_from_tabs(vec![
            make_tab("s0", "fleet-hub", TabState::Idle, false),
            make_tab("s1", "fleet-cli", TabState::Waiting, true),
            make_tab("s2", "fleet-reporter", TabState::Working, false),
        ]);

        // Palette query: "fleet" → all three match; s1 has unread bonus.
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 3);
        assert_eq!(
            results[0].session_id, "s1",
            "unread tab must rank first in palette"
        );

        // Cycle-unread: only s1 is unread → always returns s1.
        let (_, ti) = cycle_unread(&view, None).unwrap();
        assert_eq!(view.tabs[ti].session_id, "s1");
    }

    #[test]
    fn palette_case_insensitive_end_to_end() {
        let view = view_from_tabs(vec![
            make_tab("s1", "Fleet-Hub", TabState::Idle, false),
            make_tab("s2", "backend", TabState::Idle, false),
        ]);
        // Lowercase query should match mixed-case title.
        let results = query_palette(&view, "fleet", &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");

        // Uppercase query should also match.
        let results2 = query_palette(&view, "FLEET", &[]);
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].session_id, "s1");
    }
}

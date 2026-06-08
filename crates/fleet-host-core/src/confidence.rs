//! Confidence surfacing — slice S22 (node `UICONF`).
//!
//! S22 renders `inferred` vs `high` distinctly: a **hollow badge** for
//! `Confidence::Inferred` and a **solid badge** for `Confidence::High` (PLAN
//! S22 / §15.3). The view-model already carries the *truthful* worst waiting
//! confidence on [`crate::SessionTab::confidence`] (invariant 5 — never
//! upgraded in the reducer); this slice only decides **how to render it**.
//!
//! ## Design
//!
//! [`BadgeMarker`] is the single renderable unit this module exposes. The host
//! shell maps each variant to its icon asset (e.g. a filled circle vs an
//! outlined circle). Keeping the mapping here — in pure Rust, free of any
//! window dependency — satisfies the `◆G3` gate criterion for confidence
//! render and lets the host be a thin pass-through.
//!
//! ## Confidence honesty (invariant 5)
//!
//! [`BadgeMarker::from_confidence`] is a **total, injective** function:
//! - `Confidence::High` → [`BadgeMarker::Solid`] (authoritative channel)
//! - `Confidence::Inferred` → [`BadgeMarker::Hollow`] (heuristic)
//!
//! The two variants are **distinct** — `Solid != Hollow` — so the GUI can
//! never accidentally render them identically. No upgrade path exists: the
//! reducer in `view.rs` already ensures `confidence` is the *worst* across
//! waiting runs (invariant 5), so `BadgeMarker` merely renders what it
//! receives.
//!
//! ## Optional wrapper helper
//!
//! [`badge_for`] wraps the common `Option<Confidence>` case (a `SessionTab`
//! carries `None` when nothing is waiting): `None` → `None`, `Some(c)` →
//! `Some(BadgeMarker::from_confidence(c))`.

use fleet_protocol::Confidence;

// ── Public types ──────────────────────────────────────────────────────────────

/// The visual badge marker used to render waiting confidence in the inbox.
///
/// The host shell maps these to its icon assets:
/// - [`Solid`](BadgeMarker::Solid) — a filled / solid circle — authoritative
///   channel (high confidence).
/// - [`Hollow`](BadgeMarker::Hollow) — an outlined / hollow circle — heuristic
///   inference (inferred confidence).
///
/// The variants are deliberately *named for their visual appearance*, not for
/// the underlying confidence level, so the host maps them to icon names without
/// needing to know about the protocol's `Confidence` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeMarker {
    /// Solid / filled badge — rendered for [`Confidence::High`].
    ///
    /// Indicates the waiting signal came from an authoritative channel (e.g.
    /// `PermissionRequest` in Use-Terminal mode, Codex `requestApproval`).
    Solid,

    /// Hollow / outlined badge — rendered for [`Confidence::Inferred`].
    ///
    /// Indicates the waiting signal was inferred heuristically (e.g.
    /// `PreToolUse`-without-`Stop` debounce, native extension UI path). The
    /// badge is displayed but visually distinguished so the user knows the
    /// system is less certain.
    Hollow,
}

impl BadgeMarker {
    /// All variants, for exhaustive testing.
    pub const ALL: [BadgeMarker; 2] = [BadgeMarker::Solid, BadgeMarker::Hollow];

    /// Map a [`Confidence`] value to its [`BadgeMarker`].
    ///
    /// This is the **total, injective** render mapping required by PLAN S22 /
    /// §15.3 and the `◆G3` gate criterion:
    ///
    /// | Confidence | BadgeMarker | Visual |
    /// |---|---|---|
    /// | `High` | `Solid` | filled circle — authoritative |
    /// | `Inferred` | `Hollow` | outlined circle — heuristic |
    ///
    /// The mapping is *injective*: no two distinct `Confidence` values map to
    /// the same `BadgeMarker`. This is the compiler-enforced guarantee that the
    /// GUI can never accidentally render `inferred` and `high` identically.
    pub fn from_confidence(c: Confidence) -> Self {
        match c {
            Confidence::High => BadgeMarker::Solid,
            Confidence::Inferred => BadgeMarker::Hollow,
        }
    }

    /// A short descriptive label the host can use for accessibility text or
    /// tooltip copy.
    pub fn label(self) -> &'static str {
        match self {
            BadgeMarker::Solid => "high-confidence",
            BadgeMarker::Hollow => "inferred",
        }
    }
}

/// Map an `Option<Confidence>` to an `Option<BadgeMarker>`.
///
/// `None` (nothing is waiting, or the tab has no confidence) → `None` (no badge
/// to render). `Some(c)` → `Some(BadgeMarker::from_confidence(c))`.
///
/// This is the convenience wrapper callers use when iterating over
/// [`crate::SessionTab`]s: `badge_for(tab.confidence)`.
pub fn badge_for(confidence: Option<Confidence>) -> Option<BadgeMarker> {
    confidence.map(BadgeMarker::from_confidence)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::Confidence;

    // ── Core render-marker invariants (PLAN S22 / §15.3) ─────────────────────

    /// `Confidence::High` renders as [`BadgeMarker::Solid`].
    ///
    /// A solid badge signals an authoritative (high-confidence) waiting state.
    /// This is the first half of the S22 "hollow vs solid badge" requirement.
    #[test]
    fn high_confidence_renders_solid() {
        assert_eq!(
            BadgeMarker::from_confidence(Confidence::High),
            BadgeMarker::Solid
        );
    }

    /// `Confidence::Inferred` renders as [`BadgeMarker::Hollow`].
    ///
    /// A hollow badge signals a heuristic (inferred) waiting state. This is the
    /// second half of the S22 "hollow vs solid badge" requirement.
    #[test]
    fn inferred_confidence_renders_hollow() {
        assert_eq!(
            BadgeMarker::from_confidence(Confidence::Inferred),
            BadgeMarker::Hollow
        );
    }

    /// `Inferred` and `High` produce **distinct** render markers.
    ///
    /// This is the core S22 invariant: the two confidence levels must never
    /// collapse to the same visual. If this test ever fails, the renderer would
    /// show both `inferred` and `high` identically — a direct violation of
    /// §15.3 and invariant 5.
    #[test]
    fn inferred_and_high_produce_distinct_markers() {
        let solid = BadgeMarker::from_confidence(Confidence::High);
        let hollow = BadgeMarker::from_confidence(Confidence::Inferred);
        assert_ne!(
            solid, hollow,
            "inferred and high must produce distinct render markers (S22 / §15.3)"
        );
    }

    // ── Injectivity: every Confidence variant maps to a unique BadgeMarker ─────

    /// The mapping is **injective** — no two distinct `Confidence` values share a
    /// `BadgeMarker`. Verified exhaustively over `Confidence::ALL`.
    ///
    /// This means the host GUI cannot accidentally render `inferred` and `high`
    /// identically even if it iterates over all possible values.
    #[test]
    fn mapping_is_injective_over_all_confidence_variants() {
        let markers: Vec<BadgeMarker> = Confidence::ALL
            .iter()
            .map(|&c| BadgeMarker::from_confidence(c))
            .collect();

        // All produced markers must be unique.
        let mut deduped = markers.clone();
        deduped.sort_by_key(|m| *m as u8);
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            markers.len(),
            "every Confidence variant must map to a distinct BadgeMarker"
        );
    }

    // ── badge_for (Option wrapper) ────────────────────────────────────────────

    /// `badge_for(None)` returns `None` — no badge when nothing is waiting.
    #[test]
    fn badge_for_none_returns_none() {
        assert_eq!(badge_for(None), None);
    }

    /// `badge_for(Some(High))` returns `Some(Solid)`.
    #[test]
    fn badge_for_high_returns_some_solid() {
        assert_eq!(badge_for(Some(Confidence::High)), Some(BadgeMarker::Solid));
    }

    /// `badge_for(Some(Inferred))` returns `Some(Hollow)`.
    #[test]
    fn badge_for_inferred_returns_some_hollow() {
        assert_eq!(
            badge_for(Some(Confidence::Inferred)),
            Some(BadgeMarker::Hollow)
        );
    }

    /// `badge_for(Some(Inferred))` and `badge_for(Some(High))` produce
    /// **distinct** `Option<BadgeMarker>` values — the same distinctness
    /// invariant through the `Option` wrapper.
    #[test]
    fn badge_for_inferred_and_high_are_distinct() {
        let solid = badge_for(Some(Confidence::High));
        let hollow = badge_for(Some(Confidence::Inferred));
        assert_ne!(
            solid, hollow,
            "badge_for must preserve distinctness through the Option wrapper"
        );
    }

    // ── BadgeMarker::label (accessibility / tooltip copy) ────────────────────

    /// The `label()` for `Solid` and `Hollow` are distinct non-empty strings.
    #[test]
    fn badge_marker_labels_are_distinct_and_non_empty() {
        let solid_label = BadgeMarker::Solid.label();
        let hollow_label = BadgeMarker::Hollow.label();
        assert!(!solid_label.is_empty(), "Solid label must not be empty");
        assert!(!hollow_label.is_empty(), "Hollow label must not be empty");
        assert_ne!(
            solid_label, hollow_label,
            "Solid and Hollow labels must be distinct"
        );
    }

    /// `Solid` has the `"high-confidence"` label (documents the authoritative tier).
    #[test]
    fn solid_label_is_high_confidence() {
        assert_eq!(BadgeMarker::Solid.label(), "high-confidence");
    }

    /// `Hollow` has the `"inferred"` label (documents the heuristic tier).
    #[test]
    fn hollow_label_is_inferred() {
        assert_eq!(BadgeMarker::Hollow.label(), "inferred");
    }

    // ── ALL array exhaustiveness ──────────────────────────────────────────────

    /// `BadgeMarker::ALL` covers exactly the two variants: `Solid` and `Hollow`.
    #[test]
    fn all_array_covers_both_variants() {
        assert!(BadgeMarker::ALL.contains(&BadgeMarker::Solid));
        assert!(BadgeMarker::ALL.contains(&BadgeMarker::Hollow));
        assert_eq!(BadgeMarker::ALL.len(), 2);
    }

    // ── Integration: view-model round-trip via badge_for ─────────────────────

    /// Simulate the host shell rendering the confidence badge for a waiting tab.
    ///
    /// This is an end-to-end unit test of the S22 rendering path:
    /// 1. The view-model carries `confidence: Some(Confidence::X)`.
    /// 2. The host calls `badge_for(tab.confidence)`.
    /// 3. The result is `Some(BadgeMarker::Y)`.
    /// 4. The host maps the marker to its icon asset.
    ///
    /// Asserted: `Inferred → Hollow`, `High → Solid`, `None → None`.
    #[test]
    fn host_rendering_round_trip_for_all_confidence_cases() {
        // Simulate tab.confidence values and the expected badge outcome.
        let cases: &[(Option<Confidence>, Option<BadgeMarker>)] = &[
            (None, None),
            (Some(Confidence::High), Some(BadgeMarker::Solid)),
            (Some(Confidence::Inferred), Some(BadgeMarker::Hollow)),
        ];

        for (confidence, expected_badge) in cases {
            let actual = badge_for(*confidence);
            assert_eq!(
                actual, *expected_badge,
                "badge_for({confidence:?}) should be {expected_badge:?}"
            );
        }
    }
}

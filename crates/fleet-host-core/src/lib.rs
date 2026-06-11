//! Fleet host core — the **pure-Rust inbox view-model**.
//!
//! The Tauri host face (the GUI inbox) is, like every Fleet face, a *pure
//! projection* of Hub state (README §4.3 — "all faces see the same thing").
//! This crate owns that projection's **logic** with **zero GUI glue**: it
//! consumes the protocol [`Event`] stream (a `fleet.snapshot` followed by
//! `session.*`/`run.*` deltas) and reduces it into an [`InboxView`] — the
//! vertical list of session tabs the window draws (glyph, agent icon, title,
//! state).
//!
//! ## Why a separate crate from `fleet-cli`'s reducer
//!
//! `fleet-cli`'s `render.rs` is the terminal face's reducer. This is the GUI
//! face's reducer. They must agree (both are pure projections of the same Hub
//! state, both reuse [`fleet_protocol::rollup`] for the rollup ordering), but
//! the GUI needs a richer, **window-independent** view-model: glyphs, agent
//! icons, and the seams (sort / notify / confidence / focus / palette / mute)
//! that extend the base view. Keeping it pure-Rust and free of any
//! `tauri`/window dependency means the **reducer determinism** unit tests run
//! with no window at all.
//!
//! ## The stable ViewModel API (what later slices extend)
//!
//! - [`InboxView`] — the reduced, renderable view: an ordered list of
//!   [`SessionTab`]s. **This is the contract the host window renders and the
//!   slices extend** — they add *fields*/*methods*, never reshape the reduce.
//! - [`InboxModel`] — the reducer: [`InboxModel::apply`] folds one [`Event`],
//!   [`InboxModel::view`] produces the current [`InboxView`]. Determinism: the
//!   same event sequence always yields the same view, independent of any window.
//! - The seam modules — [`sort`], [`notify`], [`confidence`], [`focus`],
//!   [`palette`], [`mute`] — keep view-specific behavior in disjoint files.
//!
//! ## Locked decisions honored
//!
//! - **D9** — [`fleet_protocol::State::Done`] is surfaced as its own
//!   [`TabState`] variant, never folded into idle.
//! - **Invariant 5 (confidence honesty)** — every tab carries the worst
//!   [`Confidence`] of its waiting runs truthfully; the [`confidence`] seam will
//!   render `inferred` vs `high` distinctly (S22) but never *upgrade* it.
//! - **§4.3** — the view is a pure function of the event stream; no I/O here.
//!
//! ## Example
//!
//! ```
//! use fleet_host_core::{InboxModel, TabState};
//! use fleet_protocol::{
//!     Event, Session, Location, LocationKind, LocationGlyph, Server, ServerKind,
//!     State, Extra,
//! };
//!
//! let loc = Location {
//!     kind: LocationKind::Local, label: "laptop".into(),
//!     glyph: LocationGlyph::Laptop, attach_hint: None, extra: Extra::new(),
//! };
//! let srv = Server { kind: ServerKind::Local, version: None, extra: Extra::new() };
//! let s = Session::new("s1", "repo @ main", loc, srv, State::Working, "2026-06-08T00:00:00Z");
//!
//! let mut model = InboxModel::new();
//! model.apply(Event::snapshot(vec![]));        // initial empty snapshot
//! model.apply(Event::session_added(s));        // a session appears
//!
//! let view = model.view();
//! assert_eq!(view.tabs.len(), 1);
//! assert_eq!(view.tabs[0].title, "repo @ main");
//! assert_eq!(view.tabs[0].state, TabState::Working);
//! ```

#![forbid(unsafe_code)]

mod view;

// Seam modules. Each is pure and unit-testable; the Tauri shell is deliberately thin.
pub mod confidence;
pub mod focus;
pub mod mute;
pub mod notify;
pub mod palette;
pub mod sort;

// Multi-editor descriptor table + launch/focus. A data-driven table (one row
// per editor) plus a single launcher that reuses the per-OS [`focus`] seam.
pub mod editors;

pub use view::{AgentIcon, InboxModel, InboxView, SessionTab, TabState};

// Re-export the editor descriptor table API at the crate root so the host shell
// can `use fleet_host_core::{EditorDescriptor, installed_targets, …};`.
pub use editors::{
    descriptor_for, focus_editor, installed_targets, is_kind_installed, launch_command, Detector,
    EditorDescriptor, LaunchTarget, PathDetector, EDITORS,
};

// Re-export the protocol confidence enum at the crate root so host consumers can
// `use fleet_host_core::Confidence;` without a second `fleet_protocol` import.
pub use fleet_protocol::Confidence;

//! Fleet end-to-end integration harness for the engineering spec acceptance tests.
//!
//! This crate is the v1 **acceptance** layer. It owns no product logic of its own:
//! it **composes the already-built components** — the real Hub
//! (`fleet_hub`: merge engine + SQLite persistence + WebSocket server), the real
//! detection adapters (`fleet_reporter`: `CodexAdapter`, `ClaudeAdapter`,
//! `ClaudeInferAdapter`/machine, `ClaudeShimAdapter`), the real host-face
//! view-model (`fleet_host_core::InboxModel`), and the real `fleet` CLI binary —
//! into automated integration tests that cover **every** §21 Definition-of-Done
//! item (1–11). Each DoD item is a named `#[test]` in `tests/dod.rs`.
//!
//! ## What "compose the real components" means here
//!
//! - **Hub.** [`TestHub`] runs the *actual* [`fleet_hub::server`] WebSocket
//!   listener over an in-process [`fleet_hub::HubState`] (in-memory SQLite event
//!   log by default; a temp-file DB for the restart-persistence item). Reporters
//!   and faces talk to it over a real loopback WebSocket — nothing is stubbed.
//! - **Reporters.** [`drive_codex`]/[`drive_claude_*`] feed **recorded** Codex /
//!   Claude hook-event JSON (the [`fixtures`] module) through the real adapters,
//!   which emit [`fleet_reporter::ReporterCommand`]s; [`apply_commands`] turns
//!   those into the same `session.upsert` / `run.upsert` Hub ingests a real
//!   reporter would send. No agent is launched — the fixtures *stand in* for an
//!   agent's hook stream (observer-not-owner, §3 invariant 3 / §21.10).
//! - **Faces.** [`FaceClient`] is a real WebSocket subscriber that folds the Hub's
//!   `fleet.snapshot` + delta stream into a [`fleet_host_core::InboxModel`] — the
//!   host (sidebar) view-model. The `fleet ls --once` binary is the second face
//!   ([`cli_ls_once`]). Both read the **same** protocol off the **same** Hub.
//! - **OS focus is mocked.** Focus uses `fleet_host_core::focus::MockBackend`; no
//!   real editor/GUI/window-manager is required (§21 "use mocked OS focus").
//!
//! Everything here is `cfg`-portable so the suite runs on **macOS and Linux**
//! (§21.11): the WebSocket transport binds loopback on both; no test depends on a
//! unix-only path for its assertions.

#![forbid(unsafe_code)]

pub mod fixtures;
pub mod harness;

pub use harness::{
    apply_commands, cli_ls_once, drive_claude_infer, drive_claude_shim, drive_codex,
    drive_codex_thread, find_fleet_binary, local_session, mock_focus, wait_for, FaceClient,
    TestHub,
};

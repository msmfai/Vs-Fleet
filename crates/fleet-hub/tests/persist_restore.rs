//! Integration tests for durable Hub state (PLAN S7, D3, D17).
//!
//! These drive the *public* `HubState` API — the same surface the server's
//! connection tasks use — proving that a Hub restart (a fresh `HubState::with_db`
//! over the same log) restores all sessions/runs, and that the `gc()` timer path
//! reaps `dead` runs past their grace and sweeps expired sessions.

use std::time::Duration;

use fleet_hub::HubState;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State,
};

fn loc() -> Location {
    Location {
        kind: LocationKind::Local,
        label: "l".into(),
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
fn sess(id: &str, updated: &str) -> Session {
    Session::new(id, "t", loc(), srv(), State::Idle, updated)
}
fn run(id: &str, state: State, updated: &str) -> AgentRun {
    AgentRun::new(
        id,
        AgentKind::Codex,
        "native",
        "/",
        state,
        Confidence::High,
        updated,
    )
}

/// A Hub restart over the same on-disk log restores every session and run.
#[tokio::test]
async fn hub_restart_restores_state_from_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");

    // First Hub lifetime.
    {
        let state = HubState::with_db(&path).unwrap();
        state
            .ingest_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .await;
        state
            .ingest_session_upsert(sess("s2", "2026-06-08T00:00:00Z"))
            .await;
        state
            .ingest_run_upsert("s1", run("r1", State::Working, "2026-06-08T00:01:00Z"))
            .await;
    }

    // Second Hub lifetime: a subscribe must see the restored snapshot.
    {
        let state = HubState::with_db(&path).unwrap();
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                assert_eq!(sessions.len(), 2);
                let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
                assert_eq!(s1.rollup_state, State::Working);
                assert_eq!(s1.runs.len(), 1);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

/// The `gc()` path reaps a `dead` run past grace and the reap survives restart.
#[tokio::test]
async fn gc_reaps_dead_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");

    {
        let state = HubState::with_db(&path).unwrap();
        state
            .ingest_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .await;
        state
            .ingest_run_upsert("s1", run("d", State::Dead, "2026-06-08T00:00:00Z"))
            .await;
        // GC well past the 1 h grace → the dead run is reaped (≥1 broadcast event).
        let n = state
            .gc(
                "2026-06-08T12:00:00Z",
                Duration::from_secs(3600),
                Duration::from_secs(24 * 3600),
            )
            .await
            .unwrap();
        assert!(n >= 1, "expected at least one reap event");
    }

    // Restart: the reap was logged, so the dead run does not resurrect.
    {
        let state = HubState::with_db(&path).unwrap();
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
                assert!(
                    s1.runs.is_empty(),
                    "reaped dead run stays gone after restart"
                );
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

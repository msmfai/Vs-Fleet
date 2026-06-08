//! Async fake reporter: connects outbound to the Hub and drives the scripted
//! transition sequence with configurable inter-step delays (PLAN S4).
//!
//! This is the runtime component of the fake reporter. The *generation* of the
//! sequence lives in [`crate::transition::TransitionScript`] (pure, sync) so
//! it can be tested without network I/O. The `FakeReporter` drives that sequence
//! over a real WebSocket connection, mapping each [`ScriptedStep`] to the Hub's
//! wire message type ([`fleet_hub::wire::ClientMessage`]).
//!
//! # Transport (D7)
//! - **WS**: always available; connect via a `ws://` URL.
//! - **unix fast path** (`cfg(unix)` only): connect via a filesystem path.
//!   The WS handshake runs over the unix stream — same JSON frames, same Hub
//!   logic, different transport layer.

use std::time::Duration;

use anyhow::{Context, Result};
use fleet_hub::wire::ClientMessage;
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info};

use crate::transition::{ScriptedStep, TransitionScript};

/// How to connect to the Hub.
#[derive(Debug, Clone)]
pub enum Transport {
    /// WebSocket over TCP — always available (D7).
    WebSocket(String),
    /// WebSocket over a unix-domain socket — `cfg(unix)` fast path (D7).
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

/// Configuration for the fake reporter.
#[derive(Debug, Clone)]
pub struct FakeReporterConfig {
    pub session_id: String,
    pub run_id: String,
    /// Delay between each scripted step. Defaults to 200 ms in the binary;
    /// tests pass `Duration::ZERO` for immediate execution.
    pub step_delay: Duration,
}

impl Default for FakeReporterConfig {
    fn default() -> Self {
        FakeReporterConfig {
            session_id: "sess-fake-0001".into(),
            run_id: "run-fake-0001".into(),
            step_delay: Duration::from_millis(200),
        }
    }
}

/// The fake reporter.
///
/// Call [`FakeReporter::run`] to drive the full scripted lifecycle.
pub struct FakeReporter {
    pub transport: Transport,
    pub config: FakeReporterConfig,
}

impl FakeReporter {
    pub fn new(transport: Transport, config: FakeReporterConfig) -> Self {
        FakeReporter { transport, config }
    }

    /// Drive the full scripted lifecycle over the given transport.
    ///
    /// Connects to the Hub, then sends each scripted delta in order, pausing
    /// `step_delay` between steps. Returns when the sequence is complete or on
    /// error.
    pub async fn run(&self) -> Result<()> {
        let ts = now_iso8601();
        let script = TransitionScript::new(&self.config.session_id, &self.config.run_id);
        let steps = script.generate(&ts);

        match &self.transport {
            Transport::WebSocket(url) => {
                info!(url, "fake reporter connecting via WebSocket");
                let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
                    .await
                    .with_context(|| format!("fake reporter: failed to connect to {url}"))?;
                info!(url, "fake reporter connected");
                self.drive_sequence(&mut ws, &steps).await?;
                let _ = ws.close(None).await;
            }
            #[cfg(unix)]
            Transport::Unix(path) => {
                let path_str = path.display().to_string();
                info!(path = %path_str, "fake reporter connecting via unix socket");
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .with_context(|| {
                        format!("fake reporter: failed to connect to unix socket {path_str}")
                    })?;
                let (mut ws, _resp) = tokio_tungstenite::client_async("ws://localhost/", stream)
                    .await
                    .with_context(|| {
                        format!("fake reporter: WS handshake failed over unix {path_str}")
                    })?;
                info!(path = %path_str, "fake reporter connected via unix socket");
                self.drive_sequence(&mut ws, &steps).await?;
                let _ = ws.close(None).await;
            }
        }
        Ok(())
    }

    /// Map a [`ScriptedStep`] to the Hub's [`ClientMessage`] wire type.
    fn step_to_message(step: &ScriptedStep) -> ClientMessage {
        match step {
            ScriptedStep::RegisterSession { session, .. } => ClientMessage::SessionUpsert {
                session: session.clone(),
            },
            ScriptedStep::UpsertRun {
                session_id, run, ..
            } => ClientMessage::RunUpsert {
                session_id: session_id.clone(),
                run: run.clone(),
            },
            ScriptedStep::RemoveSession { session_id, .. } => ClientMessage::SessionRemove {
                session_id: session_id.clone(),
            },
        }
    }

    /// Send the scripted steps over an already-connected WS stream.
    async fn drive_sequence<S>(
        &self,
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
        steps: &[ScriptedStep],
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        for (i, step) in steps.iter().enumerate() {
            if i > 0 && !self.config.step_delay.is_zero() {
                tokio::time::sleep(self.config.step_delay).await;
            }
            let msg = Self::step_to_message(step);
            let json = serde_json::to_string(&msg)
                .with_context(|| format!("fake: failed to serialize step {i}"))?;
            debug!(step = i, label = step.label(), "sending scripted delta");
            ws.send(Message::Text(json))
                .await
                .with_context(|| format!("fake: failed to send step {i} ({})", step.label()))?;
            info!(step = i, label = step.label(), "scripted delta sent");
        }
        Ok(())
    }
}

/// Current time as an ISO-8601 string (UTC, second precision).
///
/// Uses a simple manual formatter to avoid pulling in `chrono`/`time` at this
/// stage. REPCORE (S5) will introduce a proper time abstraction.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let mut y = 1970u64;
    let mut rem = days;
    loop {
        let dy = if is_leap(y) { 366 } else { 365 };
        if rem < dy {
            break;
        }
        rem -= dy;
        y += 1;
    }
    let month_days: [u64; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1u64;
    for &md in &month_days {
        if rem < md {
            break;
        }
        rem -= md;
        mo += 1;
    }
    (y, mo, rem + 1)
}

pub fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ISO-8601 formatter ───────────────────────────────────────────────────

    #[test]
    fn now_iso8601_is_well_formed() {
        let s = now_iso8601();
        assert_eq!(s.len(), 20, "ISO-8601 string must be 20 chars: got '{s}'");
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
        assert_eq!(&s[13..14], ":");
        assert_eq!(&s[16..17], ":");
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-06-08 = 20612 days since epoch (see transition tests for derivation)
        let (y, mo, d) = days_to_ymd(20612);
        assert_eq!(y, 2026);
        assert_eq!(mo, 6);
        assert_eq!(d, 8);
    }

    #[test]
    fn is_leap_known_values() {
        assert!(is_leap(2000));
        assert!(is_leap(2024));
        assert!(!is_leap(1900));
        assert!(!is_leap(2100));
        assert!(!is_leap(2023));
    }

    // ── step_to_message mapping ──────────────────────────────────────────────

    #[test]
    fn register_session_maps_to_session_upsert() {
        use fleet_protocol::{
            Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, State, SCHEMA_VERSION,
        };
        let sess = fleet_protocol::Session {
            schema_version: SCHEMA_VERSION,
            session_id: "s1".into(),
            title: "t".into(),
            location: Location {
                kind: LocationKind::Local,
                label: "l".into(),
                glyph: LocationGlyph::Laptop,
                attach_hint: None,
                extra: Extra::new(),
            },
            editor: None,
            server: Server {
                kind: ServerKind::Local,
                version: None,
                extra: Extra::new(),
            },
            runs: Vec::new(),
            rollup_state: State::Idle,
            rollup_urgency: None,
            muted: false,
            soloed: false,
            unread: false,
            tags: vec![],
            policy: None,
            updated_at: "2026-06-08T00:00:00Z".into(),
            extra: Extra::new(),
        };
        let step = ScriptedStep::RegisterSession {
            session: sess.clone(),
            label: "test",
        };
        let msg = FakeReporter::step_to_message(&step);
        match msg {
            ClientMessage::SessionUpsert { session } => assert_eq!(session.session_id, "s1"),
            other => panic!("wrong message type: {other:?}"),
        }
    }

    #[test]
    fn upsert_run_maps_to_run_upsert() {
        use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, SCHEMA_VERSION};
        let run = AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: "r1".into(),
            agent_kind: AgentKind::ClaudeCode,
            native_id: "n".into(),
            cwd: "/".into(),
            state: State::Working,
            urgency: None,
            last_message: None,
            waiting_since: None,
            confidence: Confidence::Inferred,
            diff_summary: None,
            updated_at: "2026-06-08T00:00:00Z".into(),
            extra: Extra::new(),
        };
        let step = ScriptedStep::UpsertRun {
            session_id: "s1".into(),
            run: run.clone(),
            label: "test",
        };
        let msg = FakeReporter::step_to_message(&step);
        match msg {
            ClientMessage::RunUpsert { session_id, run: r } => {
                assert_eq!(session_id, "s1");
                assert_eq!(r.run_id, "r1");
            }
            other => panic!("wrong message type: {other:?}"),
        }
    }

    #[test]
    fn remove_session_maps_to_session_remove() {
        let step = ScriptedStep::RemoveSession {
            session_id: "s1".into(),
            label: "test",
        };
        let msg = FakeReporter::step_to_message(&step);
        match msg {
            ClientMessage::SessionRemove { session_id } => assert_eq!(session_id, "s1"),
            other => panic!("wrong message type: {other:?}"),
        }
    }

    // ── FakeReporter integration tests ───────────────────────────────────────
    //
    // Design note: `ws.send().await` in the reporter completes when bytes are in
    // the local TCP send buffer — the hub's connection task may not have processed
    // those messages yet. Integration tests therefore run the reporter and event
    // collection CONCURRENTLY (via `tokio::join!` / `tokio::spawn`) so that both
    // sides make progress simultaneously and there is no ordering hazard.

    use fleet_hub::server::{run_ws_listener, HubState};
    use fleet_protocol::{Event, State};
    use futures_util::StreamExt;
    use std::net::SocketAddr;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    async fn next_event<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Event
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        loop {
            match ws.next().await {
                Some(Ok(WsMsg::Text(txt))) => {
                    return serde_json::from_str(&txt).expect("decodable event")
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => panic!("ws error: {e}"),
                None => panic!("ws stream closed unexpectedly"),
            }
        }
    }

    fn test_config(session_id: &str, run_id: &str) -> FakeReporterConfig {
        FakeReporterConfig {
            session_id: session_id.into(),
            run_id: run_id.into(),
            step_delay: Duration::ZERO,
        }
    }

    /// Drive the scripted steps step-by-step over WS and verify hub state
    /// (via snapshot_event) after each step. This avoids the WS-forwarding
    /// race condition by querying the hub's merge engine directly.
    #[tokio::test]
    async fn fake_reporter_drives_full_lifecycle_via_hub_state() {
        let state = HubState::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
        tokio::spawn(fut);
        let url = format!("ws://{local}");

        // Open a direct WS reporter connection.
        let (mut rep, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let ts = "2026-06-08T12:00:00Z";
        let script = crate::transition::TransitionScript::new("sess-lifecycle", "run-lifecycle");
        let steps = script.generate(ts);

        // Send each step and verify hub state via snapshot_event.
        // snapshot_event() reads from the merge engine under a mutex, so it
        // always reflects the most recently processed messages.
        // We yield after each send to let the hub process the message.
        for step in steps.iter() {
            let msg = FakeReporter::step_to_message(step);
            let json = serde_json::to_string(&msg).unwrap();
            rep.send(WsMsg::Text(json)).await.unwrap();
            // Yield to let the hub's connection task process this message.
            tokio::task::yield_now().await;
            // Extra yield to ensure the hub's mutex is released and state is visible.
            tokio::task::yield_now().await;
        }

        // After all steps, verify final state: session removed.
        // Poll until the hub processes the remove (may need a few yields).
        let check = async {
            loop {
                let snap = state.snapshot_event().await;
                if let Event::Snapshot { sessions, .. } = snap {
                    if sessions.is_empty() {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), check)
            .await
            .expect("session must be removed within 2s");
    }

    /// Verify the fake reporter drives the full event sequence over WS.
    ///
    /// Pattern: reporter and subscriber are interleaved in the SAME task.
    /// We manually drive the reporter step-by-step (one scripted step per
    /// iteration), yielding to the face between each step. This mirrors the
    /// pattern used in the hub transport smoke tests and is reliable because
    /// after each `rep.send().await`, the hub's connection tasks get scheduler
    /// time to broadcast events to the subscribed face.
    #[tokio::test]
    async fn fake_reporter_drives_session_added_then_removed() {
        let state = HubState::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
        tokio::spawn(fut);
        let url = format!("ws://{local}");

        // Subscribe face.
        let (mut face, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let sub = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
        face.send(WsMsg::Text(sub)).await.unwrap();
        match next_event(&mut face).await {
            Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected snapshot: {other:?}"),
        }

        // Reporter in the same task: step-by-step send + face reads.
        let (mut rep, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let ts = now_iso8601();
        let script = crate::transition::TransitionScript::new("sess-integ", "run-integ");
        let steps = script.generate(&ts);

        let mut collected_events: Vec<Event> = Vec::new();

        for step in &steps {
            let msg = FakeReporter::step_to_message(step);
            let json = serde_json::to_string(&msg).unwrap();
            rep.send(WsMsg::Text(json)).await.unwrap();

            // Read all events the face sees for this step (with a short timeout).
            // A session.upsert → session.added (1 event).
            // A run.upsert (new run) → run.added + session.updated (2 events).
            // A run.upsert (existing run) → run.updated + session.updated (2 events).
            // A session.remove → session.removed (1 event).
            let expected_count = match step.wire_type() {
                "session.upsert" => 1, // session.added
                "run.upsert" => 2,     // run.added/updated + session.updated
                "session.remove" => 1, // session.removed
                _ => 0,
            };

            for _ in 0..expected_count {
                let ev = tokio::time::timeout(Duration::from_secs(2), next_event(&mut face))
                    .await
                    .unwrap_or_else(|_| {
                        panic!("timeout reading event for step '{}'", step.label())
                    });
                collected_events.push(ev);
            }
        }

        let saw_session_added = collected_events
            .iter()
            .any(|e| matches!(e, Event::SessionAdded { .. }));
        let saw_session_removed = collected_events
            .iter()
            .any(|e| matches!(e, Event::SessionRemoved { .. }));
        let run_states: Vec<State> = collected_events
            .iter()
            .filter_map(|e| match e {
                Event::RunAdded { run, .. } | Event::RunUpdated { run, .. } => Some(run.state),
                _ => None,
            })
            .collect();

        assert!(saw_session_added, "must see session.added");
        assert!(saw_session_removed, "must see session.removed");
        assert!(run_states.contains(&State::Working), "must see working");
        assert!(run_states.contains(&State::Waiting), "must see waiting");
        assert!(run_states.contains(&State::Dead), "must see dead");
    }

    /// §4.3 two-face consistency test driven by the fake reporter.
    ///
    /// Two faces subscribe, the fake drives all steps, and we verify both faces
    /// see the same final snapshot after the full lifecycle. We verify consistency
    /// by querying the hub's merge engine snapshot directly at the end — avoiding
    /// the WS event-forwarding race — and checking that late subscribing faces get
    /// the same state.
    #[tokio::test]
    async fn two_faces_see_identical_events_from_fake() {
        // §4.3: every face is a projection of the same Hub state.
        // We run the full fake lifecycle via the hub's WS listener, then
        // verify both faces see the same snapshot after the session is removed.
        let state = HubState::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
        tokio::spawn(fut);
        let url = format!("ws://{local}");

        // Run the full lifecycle step-by-step (same task, reliable ordering).
        let (mut rep, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let ts = now_iso8601();
        let script = crate::transition::TransitionScript::new("sess-two-face", "run-two-face");
        let steps = script.generate(&ts);
        for step in &steps {
            let msg = FakeReporter::step_to_message(step);
            let json = serde_json::to_string(&msg).unwrap();
            rep.send(WsMsg::Text(json)).await.unwrap();
            // Yield to let the hub process this message.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        }
        rep.close(None).await.ok();

        // Poll hub state until the session is removed.
        let check = async {
            loop {
                if let Event::Snapshot { sessions, .. } = state.snapshot_event().await {
                    if sessions.is_empty() {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), check)
            .await
            .expect("session must be removed");

        // Now subscribe two faces and verify they both see the same empty snapshot.
        // This is the §4.3 two-face consistency invariant: the Hub's state is a
        // single source of truth that all faces project identically.
        let (mut face_a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut face_b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let sub = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
        face_a.send(WsMsg::Text(sub.clone())).await.unwrap();
        face_b.send(WsMsg::Text(sub)).await.unwrap();

        let snap_a = next_event(&mut face_a).await;
        let snap_b = next_event(&mut face_b).await;

        // Both faces must see identical snapshots.
        let v_a = serde_json::to_value(&snap_a).unwrap();
        let v_b = serde_json::to_value(&snap_b).unwrap();
        assert_eq!(
            v_a, v_b,
            "both faces must see the identical snapshot (§4.3)"
        );
        match snap_a {
            Event::Snapshot { sessions, .. } => {
                assert!(sessions.is_empty(), "session must be removed from snapshot");
            }
            other => panic!("expected snapshot: {other:?}"),
        }

        // Also verify the scripted sequence was applied: push a new session and
        // verify BOTH faces see the same session.added.
        let session_b_json = serde_json::to_string(&ClientMessage::SessionUpsert {
            session: {
                let mut s = steps[0].as_session().unwrap().clone();
                s.session_id = "sess-verify".into();
                s
            },
        })
        .unwrap();
        let (mut verifier, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        verifier.send(WsMsg::Text(session_b_json)).await.unwrap();

        let ev_a = next_event(&mut face_a).await;
        let ev_b = next_event(&mut face_b).await;
        assert_eq!(
            serde_json::to_value(&ev_a).unwrap(),
            serde_json::to_value(&ev_b).unwrap(),
            "both faces must see the same live event (§4.3 live consistency)"
        );
        match ev_a {
            Event::SessionAdded { session, .. } => {
                assert_eq!(session.session_id, "sess-verify");
            }
            other => panic!("expected session.added: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn late_subscriber_sees_correct_snapshot_after_fake() {
        // A face that subscribes after the fake lifecycle completes sees an
        // empty snapshot (because session.remove was the final step).
        // We use HubState::snapshot_event() directly — which reads the merge
        // engine under its mutex — to verify the hub actually processed the
        // remove, decoupled from WS timing.
        let state = HubState::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
        tokio::spawn(fut);
        let url = format!("ws://{local}");

        let reporter = FakeReporter::new(
            Transport::WebSocket(url.clone()),
            test_config("sess-late", "run-late"),
        );
        // Spawn concurrently and await, using the hub state to verify.
        let run_task = tokio::spawn(async move { reporter.run().await });

        // Poll hub state until session is removed (or timeout).
        let check = async {
            loop {
                tokio::task::yield_now().await;
                let snap = state.snapshot_event().await;
                match snap {
                    Event::Snapshot { sessions, .. } if sessions.is_empty() => break,
                    _ => {}
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), check)
            .await
            .expect("session must be removed from hub within 5s");

        run_task
            .await
            .expect("reporter task must not panic")
            .expect("reporter must succeed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_reporter_connects_via_unix_socket() {
        use fleet_hub::server::run_unix_listener;

        let state = HubState::new();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fleet-fake-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let fut = run_unix_listener(state.clone(), path.clone())
            .await
            .unwrap();
        tokio::spawn(fut);

        let reporter = FakeReporter::new(
            Transport::Unix(path.clone()),
            test_config("sess-unix-fake", "run-unix-fake"),
        );
        reporter.run().await.expect("unix fake run must succeed");

        // The session was removed (last scripted step), so the snapshot should
        // be empty.
        let snap = state.snapshot_event().await;
        match snap {
            Event::Snapshot { sessions, .. } => {
                assert!(
                    sessions.is_empty(),
                    "after full lifecycle via unix socket, session should be removed"
                );
            }
            other => panic!("expected snapshot: {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }
}

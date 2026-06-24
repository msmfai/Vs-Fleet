//! Async fake reporter: connects outbound to the Hub and drives the scripted
//! transition sequence with configurable inter-step delays (the engineering spec).
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
                // The S4 fake is an un-stamped fixture (no durable-identity
                // gating); the Hub applies it ungated, as for any S5 reporter.
                stamp: None,
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
            ws.send(Message::Text(json.into()))
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
    fn days_to_ymd_in_a_leap_year_uses_29_day_february() {
        // 2024 is a leap year. 2024-02-29 = 19782 days since epoch. Landing on
        // Feb 29 exercises the leap-year `month_days` table (the 29-day branch).
        let (y, mo, d) = days_to_ymd(19782);
        assert_eq!((y, mo, d), (2024, 2, 29), "leap-year Feb has a 29th");
        // And the day after rolls into March, confirming February held 29 days.
        assert_eq!(days_to_ymd(19783), (2024, 3, 1));
    }

    #[test]
    fn default_config_has_the_binary_defaults() {
        // The binary constructs a FakeReporterConfig via Default (200ms cadence);
        // exercise it so the production default path is covered, not just the
        // test helper that uses Duration::ZERO.
        let cfg = FakeReporterConfig::default();
        assert_eq!(cfg.session_id, "sess-fake-0001");
        assert_eq!(cfg.run_id, "run-fake-0001");
        assert_eq!(cfg.step_delay, Duration::from_millis(200));
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
    //
    // Variant extractors: each destructures the expected wire variant or panics.
    // The panic arm is an unreachable test-assertion path (the message under test
    // is constructed by `step_to_message`), so these are excluded from the nightly
    // gate — the *behavioral* assertions live at the (covered) call sites.

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_session_upsert(msg: ClientMessage) -> fleet_protocol::Session {
        match msg {
            ClientMessage::SessionUpsert { session } => session,
            other => panic!("expected session.upsert, got {other:?}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_run_upsert(msg: ClientMessage) -> (String, fleet_protocol::AgentRun) {
        match msg {
            ClientMessage::RunUpsert {
                session_id, run, ..
            } => (session_id, run),
            other => panic!("expected run.upsert, got {other:?}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_session_remove(msg: ClientMessage) -> String {
        match msg {
            ClientMessage::SessionRemove { session_id } => session_id,
            other => panic!("expected session.remove, got {other:?}"),
        }
    }

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
        let session = expect_session_upsert(msg);
        assert_eq!(session.session_id, "s1");
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
        let (session_id, r) = expect_run_upsert(msg);
        assert_eq!(session_id, "s1");
        assert_eq!(r.run_id, "r1");
    }

    #[test]
    fn remove_session_maps_to_session_remove() {
        let step = ScriptedStep::RemoveSession {
            session_id: "s1".into(),
            label: "test",
        };
        let msg = FakeReporter::step_to_message(&step);
        assert_eq!(expect_session_remove(msg), "s1");
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

    // Defensive test helper: its non-Text / error / closed arms are unreachable
    // in the happy-path flows that use it, so it is excluded from the nightly
    // coverage gate (its real arm — decode a Text frame — is exercised).
    #[cfg_attr(coverage_nightly, coverage(off))]
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

    /// Extract the sessions from a `Snapshot` event or panic. The non-snapshot
    /// arm is an unreachable test assertion path, so this is excluded from the
    /// nightly gate.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_snapshot_sessions(ev: Event) -> Vec<fleet_protocol::Session> {
        match ev {
            Event::Snapshot { sessions, .. } => sessions,
            other => panic!("expected snapshot: {other:?}"),
        }
    }

    /// Read the events a subscribed face sees for one scripted step, appending
    /// them to `out`. Excluded from the nightly gate: the per-wire-type count map
    /// has a defensive `_ => 0` arm and the read uses a defensive timeout panic,
    /// neither of which is a behavioral assertion (the behavior asserted is the
    /// *content* of the collected events, in the calling test).
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn collect_events_for_step<S>(
        face: &mut tokio_tungstenite::WebSocketStream<S>,
        step: &ScriptedStep,
        out: &mut Vec<Event>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        // A session.upsert → session.added (1 event).
        // A run.upsert → run.added/updated + session.updated (2 events).
        // A session.remove → session.removed (1 event).
        let expected_count = match step.wire_type() {
            "session.upsert" => 1,
            "run.upsert" => 2,
            "session.remove" => 1,
            _ => 0,
        };
        for _ in 0..expected_count {
            let ev = tokio::time::timeout(Duration::from_secs(2), next_event(face))
                .await
                .unwrap_or_else(|_| panic!("timeout reading event for step '{}'", step.label()));
            out.push(ev);
        }
    }

    /// Poll the hub's merge-engine snapshot until it has no sessions (the final
    /// scripted step is a session.remove). Excluded from the nightly gate: the
    /// number of poll iterations — hence which branch/yield lines execute — is
    /// timing-dependent, not a behavioral assertion.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn wait_until_sessions_empty(state: &HubState) {
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
        tokio::time::timeout(Duration::from_secs(5), check)
            .await
            .expect("session must be removed within 5s");
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
            rep.send(WsMsg::Text(json.into())).await.unwrap();
            // Yield to let the hub's connection task process this message.
            tokio::task::yield_now().await;
            // Extra yield to ensure the hub's mutex is released and state is visible.
            tokio::task::yield_now().await;
        }

        // After all steps, verify final state: session removed.
        wait_until_sessions_empty(&state).await;
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
        face.send(WsMsg::Text(sub.into())).await.unwrap();
        assert!(
            expect_snapshot_sessions(next_event(&mut face).await).is_empty(),
            "the initial snapshot is empty"
        );

        // Reporter in the same task: step-by-step send + face reads.
        let (mut rep, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let ts = now_iso8601();
        let script = crate::transition::TransitionScript::new("sess-integ", "run-integ");
        let steps = script.generate(&ts);

        let mut collected_events: Vec<Event> = Vec::new();

        for step in &steps {
            let msg = FakeReporter::step_to_message(step);
            let json = serde_json::to_string(&msg).unwrap();
            rep.send(WsMsg::Text(json.into())).await.unwrap();
            collect_events_for_step(&mut face, step, &mut collected_events).await;
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
            rep.send(WsMsg::Text(json.into())).await.unwrap();
            // Yield to let the hub process this message.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        }
        rep.close(None).await.ok();

        // Poll hub state until the session is removed.
        wait_until_sessions_empty(&state).await;

        // Now subscribe two faces and verify they both see the same empty snapshot.
        // This is the §4.3 two-face consistency invariant: the Hub's state is a
        // single source of truth that all faces project identically.
        let (mut face_a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut face_b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let sub = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
        face_a.send(WsMsg::Text(sub.clone().into())).await.unwrap();
        face_b.send(WsMsg::Text(sub.into())).await.unwrap();

        let snap_a = next_event(&mut face_a).await;
        let snap_b = next_event(&mut face_b).await;

        // Both faces must see identical snapshots.
        let v_a = serde_json::to_value(&snap_a).unwrap();
        let v_b = serde_json::to_value(&snap_b).unwrap();
        assert_eq!(
            v_a, v_b,
            "both faces must see the identical snapshot (§4.3)"
        );
        assert!(
            expect_snapshot_sessions(snap_a).is_empty(),
            "session must be removed from snapshot"
        );

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
        verifier
            .send(WsMsg::Text(session_b_json.into()))
            .await
            .unwrap();

        let ev_a = next_event(&mut face_a).await;
        let ev_b = next_event(&mut face_b).await;
        assert_eq!(
            serde_json::to_value(&ev_a).unwrap(),
            serde_json::to_value(&ev_b).unwrap(),
            "both faces must see the same live event (§4.3 live consistency)"
        );
        assert!(
            matches!(ev_a, Event::SessionAdded { .. }),
            "expected session.added"
        );
        if let Event::SessionAdded { session, .. } = ev_a {
            assert_eq!(session.session_id, "sess-verify");
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
        wait_until_sessions_empty(&state).await;

        run_task
            .await
            .expect("reporter task must not panic")
            .expect("reporter must succeed");
    }

    #[tokio::test]
    async fn fake_reporter_honors_a_nonzero_step_delay() {
        // With a non-zero step_delay the driver sleeps *between* steps (the
        // `i > 0 && !is_zero()` arm). A tiny 1ms delay keeps the test fast while
        // still exercising the inter-step pause, and the full lifecycle must
        // still complete (session ultimately removed).
        let state = HubState::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
        tokio::spawn(fut);
        let url = format!("ws://{local}");

        let reporter = FakeReporter::new(
            Transport::WebSocket(url.clone()),
            FakeReporterConfig {
                session_id: "sess-delay".into(),
                run_id: "run-delay".into(),
                step_delay: Duration::from_millis(1),
            },
        );
        reporter.run().await.expect("delayed fake run must succeed");

        // run() returns once bytes are in the local send buffer; poll the hub
        // state until it has processed the final session.remove (same race the
        // other integration tests handle).
        wait_until_sessions_empty(&state).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_reporter_unix_connect_failure_is_reported() {
        // Connecting to a path that exists but is a regular file (not a socket)
        // fails at `UnixStream::connect` — the `failed to connect to unix socket`
        // context arm. run() must surface that as an error, not panic.
        let dir = tempfile::tempdir().unwrap();
        let not_a_socket = dir.path().join("plain-file");
        std::fs::write(&not_a_socket, b"x").unwrap();

        let reporter = FakeReporter::new(
            Transport::Unix(not_a_socket.clone()),
            test_config("sess-unix-fail", "run-unix-fail"),
        );
        let err = reporter
            .run()
            .await
            .expect_err("connecting to a non-socket file must fail");
        assert!(
            err.to_string().contains("failed to connect to unix socket"),
            "the unix connect-error context must be attached: {err:#}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_reporter_unix_ws_handshake_failure_is_reported() {
        // A real unix listener that accepts the byte connection but immediately
        // closes it (never speaking WS) makes the WS *handshake* fail — the
        // `WS handshake failed over unix` context arm. run() must surface it.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("raw.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        // Accept one connection and drop it (no WS handshake response).
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });

        let reporter = FakeReporter::new(
            Transport::Unix(sock.clone()),
            test_config("sess-unix-hs", "run-unix-hs"),
        );
        let err = reporter
            .run()
            .await
            .expect_err("a non-WS peer must fail the handshake");
        assert!(
            err.to_string().contains("WS handshake failed over unix"),
            "the unix handshake-error context must be attached: {err:#}"
        );
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
        // become empty (poll to absorb the WS-forwarding race).
        wait_until_sessions_empty(&state).await;

        let _ = std::fs::remove_file(&path);
    }
}

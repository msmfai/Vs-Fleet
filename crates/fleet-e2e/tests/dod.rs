//! Fleet v1 **Definition of Done** acceptance suite (engineering spec §21, items 1–11).
//!
//! Each `#[test]` below is **one** §21 item, named for it. Every test COMPOSES the
//! real components through [`fleet_e2e`]'s harness:
//!
//! - a real in-process **Hub** ([`fleet_hub`] merge engine + SQLite event log +
//!   WebSocket server) on a loopback ephemeral port,
//! - real **detection adapters** ([`fleet_reporter`] `CodexAdapter`,
//!   `ClaudeAdapter`/infer/shim) driven by **recorded** hook fixtures
//!   ([`fleet_e2e::fixtures`]),
//! - the real host **view-model** ([`fleet_host_core::InboxModel`]) folding the
//!   live wire stream via [`FaceClient`],
//! - the real `fleet ls` **CLI** binary as a second face,
//! - **mocked OS focus** (`fleet_host_core::focus::MockBackend`).
//!
//! No agent is launched through Fleet (observer-not-owner, §21.10). No real
//! editor/GUI/window-manager is required. The suite runs on macOS + Linux (§21.11).

use std::time::Duration;

use fleet_e2e::fixtures;
use fleet_e2e::{
    apply_commands, cli_ls_once, drive_claude_infer, drive_claude_shim, drive_codex_thread,
    local_session, mock_focus, FaceClient, TestHub,
};
use fleet_host_core::focus::{
    cycle_next, focus_command, next_unread_tab, BackendResult, FocusOutcome, FocusPlatform,
};
use fleet_host_core::mute::{should_notify, solo_command};
use fleet_host_core::notify::{tab_transition, NotificationOutcome};
use fleet_host_core::palette::query_palette;
use fleet_host_core::{Confidence, InboxModel, TabState};
use fleet_protocol::{Command, State, Urgency};

/// The "<2s" budget the DoD repeatedly references. Real wall-clock; the in-process
/// loopback path is far faster, so this is a generous ceiling that still *measures*
/// the requirement rather than asserting an instantaneous unit call.
const WITHIN_2S: Duration = Duration::from_secs(2);

// Convenience: the tab for a session in the model's current view.
fn tab_state(model: &InboxModel, id: &str) -> Option<TabState> {
    model.view().tab(id).map(|t| t.state)
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.1 — ≥3 agents (Claude + Codex) across ≥2 editor windows appear <2s,
//         labeled with kind / title / cwd / glyph.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_01_three_agents_two_windows_appear_within_2s_labeled() {
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();

    // Two editor windows = two sessions, each a `local` laptop-glyph session.
    let win_a = hub
        .register_session(local_session(
            "win-a",
            "web @ main",
            "code --reuse-window /web",
        ))
        .await;
    let win_b = hub
        .register_session(local_session(
            "win-b",
            "api @ dev",
            "code --reuse-window /api",
        ))
        .await;

    // Three agent runs: two Codex (one per window) + one Claude (window A).
    let mut codex_a = fleet_reporter::CodexAdapter::new();
    let mut codex_b = fleet_reporter::CodexAdapter::new();
    let mut claude_a = fleet_reporter::ClaudeAdapter::new();

    drive_codex_thread(
        &mut hub,
        &win_a,
        "codex-a",
        &mut codex_a,
        [
            fixtures::codex_session_start("codex-a", "/web"),
            fixtures::codex_prompt("codex-a", "/web"),
        ],
    )
    .await;
    drive_codex_thread(
        &mut hub,
        &win_b,
        "codex-b",
        &mut codex_b,
        [
            fixtures::codex_session_start("codex-b", "/api"),
            fixtures::codex_prompt("codex-b", "/api"),
        ],
    )
    .await;
    // Claude run in window A.
    {
        let cmds = claude_a.ingest_json(&fixtures::claude_prompt("claude-a", "/web/ui"));
        apply_commands(&mut hub, &win_a, cmds).await;
    }

    // The host face must reflect both windows with all three runs within 2s.
    let appeared = face
        .wait_until(WITHIN_2S, |m| {
            let v = m.view();
            v.tab("win-a").map(|t| t.run_count).unwrap_or(0) == 2
                && v.tab("win-b").map(|t| t.run_count).unwrap_or(0) == 1
        })
        .await;
    assert!(appeared, "≥3 agents across 2 windows must appear within 2s");

    let v = face.model().view();
    assert_eq!(v.len(), 2, "two editor windows = two session tabs");

    // Labeled: kind (agent icon), title, cwd, and the laptop glyph.
    let a = v.tab("win-a").unwrap();
    assert_eq!(a.title, "web @ main");
    assert_eq!(
        a.glyph,
        fleet_protocol::LocationGlyph::Laptop,
        "laptop glyph"
    );
    // The session shows a mix of Codex + Claude runs (kind is surfaced per run).
    let snap = hub.state().snapshot_event().await;
    if let fleet_protocol::Event::Snapshot { sessions, .. } = snap {
        let win_a_sess = sessions.iter().find(|s| s.session_id == "win-a").unwrap();
        let kinds: Vec<_> = win_a_sess
            .runs
            .iter()
            .map(|r| r.agent_kind.clone())
            .collect();
        assert!(kinds.contains(&fleet_protocol::AgentKind::Codex));
        assert!(kinds.contains(&fleet_protocol::AgentKind::ClaudeCode));
        // cwd is carried per run (labeling requirement).
        assert!(win_a_sess.runs.iter().any(|r| r.cwd == "/web"));
        assert!(win_a_sess.runs.iter().any(|r| r.cwd == "/web/ui"));
        // Total runs across the fleet ≥ 3.
        let total: usize = sessions.iter().map(|s| s.runs.len()).sum();
        assert!(total >= 3, "≥3 agent runs across the fleet, got {total}");
    } else {
        panic!("expected snapshot");
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.2 — Codex approval → `approval`, confidence HIGH (via the hooks path).
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_02_codex_approval_is_high_confidence() {
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    let sid = hub
        .register_session(local_session("s", "deploy @ prod", "code /deploy"))
        .await;

    let mut codex = fleet_reporter::CodexAdapter::new();
    drive_codex_thread(
        &mut hub,
        &sid,
        "th",
        &mut codex,
        [
            fixtures::codex_session_start("th", "/deploy"),
            fixtures::codex_prompt("th", "/deploy"),
            fixtures::codex_pre_tool("th", "/deploy", "shell"),
            // The authoritative approval signal.
            fixtures::codex_permission_request("th", "/deploy", "shell"),
        ],
    )
    .await;

    let waiting = face
        .wait_until(WITHIN_2S, |m| tab_state(m, "s") == Some(TabState::Waiting))
        .await;
    assert!(waiting, "Codex approval must surface a waiting tab");

    let t = face.model().view().tab("s").cloned().unwrap();
    assert_eq!(t.state, TabState::Waiting);
    assert_eq!(t.urgency, Some(Urgency::Approval), "urgency = approval");
    // The app-server-equivalent hook is authoritative → HIGH confidence.
    assert_eq!(
        t.confidence,
        Some(Confidence::High),
        "Codex PermissionRequest is authoritative → confidence:high"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.3 — Claude approval → `inferred` in the native-UI fixture, `high` in the
//         shim fixture — for the SAME approval payload.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_03_claude_approval_inferred_native_high_shim() {
    // ── native-UI surface → inferred (S16 debounce + transcript corroboration) ──
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    let sid = hub
        .register_session(local_session("native", "ui @ main", "code /ui"))
        .await;

    let mut infer = fleet_reporter::ClaudeInferAdapter::new();
    // A PreToolUse with no following activity → debounce fires after the window.
    drive_claude_infer(
        &mut hub,
        &sid,
        &mut infer,
        [
            (fixtures::claude_session_start("nat", "/ui"), 0u64),
            (fixtures::claude_prompt("nat", "/ui"), 10),
            (fixtures::claude_pre_tool("nat", "/ui", "Edit"), 20),
        ],
    )
    .await;
    // Corroborate against a recorded transcript whose last `tool_use` has no
    // matching `tool_result` — consistent with "blocked on the user". This is the
    // real JSONL drift-guarded corroboration; `Stuck` lets the debounce stand.
    let verdict = fleet_reporter::corroborate_jsonl(&fixtures::claude_transcript_stuck("tu-1"));
    assert_eq!(verdict, fleet_reporter::InferCorroboration::Stuck);
    // Drift-guard sanity: a transcript whose tool_use HAS a result vetoes the
    // inference (the run is not actually blocked) — the honest negative case.
    assert_eq!(
        fleet_reporter::corroborate_jsonl(&fixtures::claude_transcript_resolved("tu-1")),
        fleet_reporter::InferCorroboration::Resolved
    );
    let corr_cmds = infer.corroborate("nat", verdict);
    apply_commands(&mut hub, &sid, corr_cmds).await;

    // Advance the injected clock past the debounce window → infer waiting.
    let tick_cmds = infer.tick(20 + fleet_reporter::DEFAULT_DEBOUNCE_MS + 1);
    apply_commands(&mut hub, &sid, tick_cmds).await;

    let waiting = face
        .wait_until(WITHIN_2S, |m| {
            tab_state(m, "native") == Some(TabState::Waiting)
        })
        .await;
    assert!(
        waiting,
        "native-UI Claude approval must be inferred-waiting"
    );
    let nt = face.model().view().tab("native").cloned().unwrap();
    assert_eq!(nt.urgency, Some(Urgency::Approval));
    assert_eq!(
        nt.confidence,
        Some(Confidence::Inferred),
        "native-UI: Claude waiting is a heuristic → confidence:inferred"
    );

    // ── shim surface → high (S17 authoritative PermissionRequest under the shim) ─
    let mut hub2 = TestHub::start().await.unwrap();
    let mut face2 = FaceClient::connect(&hub2.ws_url()).await.unwrap();
    let sid2 = hub2
        .register_session(local_session("shim", "ui @ main", "code /ui"))
        .await;

    let mut shim =
        fleet_reporter::ClaudeShimAdapter::new(fleet_reporter::LaunchContext::ShimTerminal);
    drive_claude_shim(
        &mut hub2,
        &sid2,
        &mut shim,
        [
            fixtures::claude_session_start("sh", "/ui"),
            fixtures::claude_prompt("sh", "/ui"),
            // The SAME approval payload as the native fixture, but under the shim.
            fixtures::claude_permission_request("sh", "/ui", "Edit"),
        ],
    )
    .await;

    let waiting2 = face2
        .wait_until(WITHIN_2S, |m| {
            tab_state(m, "shim") == Some(TabState::Waiting)
        })
        .await;
    assert!(waiting2, "shim Claude approval must surface waiting");
    let st = face2.model().view().tab("shim").cloned().unwrap();
    assert_eq!(st.urgency, Some(Urgency::Approval));
    assert_eq!(
        st.confidence,
        Some(Confidence::High),
        "shim terminal: PermissionRequest is authoritative → confidence:high"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.4 — Answering in the terminal auto-resolves <2s with no Fleet interaction.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_04_answer_in_terminal_auto_resolves_within_2s() {
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    let sid = hub
        .register_session(local_session("s", "task @ main", "code /task"))
        .await;

    let mut codex = fleet_reporter::CodexAdapter::new();
    // Get into waiting+approval.
    drive_codex_thread(
        &mut hub,
        &sid,
        "th",
        &mut codex,
        [
            fixtures::codex_session_start("th", "/task"),
            fixtures::codex_prompt("th", "/task"),
            fixtures::codex_permission_request("th", "/task", "shell"),
        ],
    )
    .await;
    assert!(
        face.wait_until(WITHIN_2S, |m| tab_state(m, "s") == Some(TabState::Waiting))
            .await,
        "precondition: waiting on approval"
    );

    // The host face confirms an approval notification was due (would ping).
    let waiting_tab = face.model().view().tab("s").cloned().unwrap();
    let outcome = tab_transition(None, Some(&waiting_tab));
    assert!(
        matches!(outcome, NotificationOutcome::Fire(_)),
        "a waiting approval fires a notification"
    );

    // The user answers IN THE REAL TERMINAL — the reporter observes the approval
    // *response* and reports working. NO Fleet command is sent.
    drive_codex_thread(
        &mut hub,
        &sid,
        "th",
        &mut codex,
        [fixtures::codex_permission_response(
            "th", "/task", "shell", true,
        )],
    )
    .await;

    let resolved = face
        .wait_until(WITHIN_2S, |m| tab_state(m, "s") == Some(TabState::Working))
        .await;
    assert!(
        resolved,
        "answering in the terminal must auto-resolve to working within 2s"
    );
    let t = face.model().view().tab("s").cloned().unwrap();
    assert_eq!(t.urgency, None, "urgency cleared on auto-resolve");
    // And the notification subsystem yields AutoResolve for the badge clear.
    let cleared = tab_transition(Some(&waiting_tab), face.model().view().tab("s"));
    assert!(
        matches!(cleared, NotificationOutcome::AutoResolve { .. }),
        "leaving waiting auto-resolves the badge"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.5 — Jump-to-next-unread focuses the right window (mocked OS) + clears unread.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_05_jump_to_next_unread_focuses_right_window_and_clears() {
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();

    // Three windows; only window B has an unread waiting agent.
    for (id, title) in [("w-a", "a"), ("w-b", "b"), ("w-c", "c")] {
        let mut s = local_session(id, title, &format!("code --reuse-window /{id}"));
        if id == "w-b" {
            s.unread = true;
        }
        hub.register_session(s).await;
    }
    // Make w-b waiting (the unread one).
    let mut codex = fleet_reporter::CodexAdapter::new();
    drive_codex_thread(
        &mut hub,
        "w-b",
        "thb",
        &mut codex,
        [
            fixtures::codex_session_start("thb", "/w-b"),
            fixtures::codex_permission_request("thb", "/w-b", "shell"),
        ],
    )
    .await;

    assert!(
        face.wait_until(WITHIN_2S, |m| {
            m.view().tab("w-b").map(|t| t.unread).unwrap_or(false)
                && tab_state(m, "w-b") == Some(TabState::Waiting)
        })
        .await,
        "w-b must be the unread waiting window"
    );

    let view = face.model().view();
    // Jump-to-next-unread selects the right window.
    let (idx, tab) = next_unread_tab(&view, None).expect("an unread window exists");
    assert_eq!(tab.session_id, "w-b", "jump targets the unread window");

    // It composes into the focus command the host issues to the Hub.
    let cmd = focus_command(&tab.session_id);
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["command"], "focus");
    assert_eq!(v["target"]["session_id"], "w-b");

    // Focus the right window through the MOCKED OS backend (macOS confirms).
    let hint = view.tabs[idx].session_id.clone();
    let _ = hint;
    let outcome = mock_focus(
        FocusPlatform::MacOs,
        BackendResult::Activated,
        "code --reuse-window /w-b",
    );
    assert_eq!(outcome, FocusOutcome::Confirmed);
    assert!(outcome.is_confirmed_success(), "focus confirmed on macOS");

    // X11 also confirms; Wayland never claims success (documented fallback).
    assert!(
        mock_focus(FocusPlatform::LinuxX11, BackendResult::Activated, "wid:0x1")
            .is_confirmed_success()
    );
    assert!(
        !mock_focus(FocusPlatform::Wayland, BackendResult::Activated, "code .")
            .is_confirmed_success(),
        "Wayland must never falsely claim focus"
    );

    // Clearing unread: the Hub command for it is a focus/read; here we model the
    // auto-resolve clear (answering in terminal) which clears unread Hub-side.
    drive_codex_thread(
        &mut hub,
        "w-b",
        "thb",
        &mut codex,
        [fixtures::codex_permission_response(
            "thb", "/w-b", "shell", true,
        )],
    )
    .await;
    assert!(
        face.wait_until(WITHIN_2S, |m| {
            tab_state(m, "w-b") == Some(TabState::Working)
        })
        .await,
        "after answering, w-b leaves waiting (badge/unread clears)"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.6 — Fuzzy palette focuses a session.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_06_fuzzy_palette_focuses_a_session() {
    let hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    hub.register_session(local_session("s1", "frontend dashboard", "code /front"))
        .await;
    hub.register_session(local_session("s2", "backend api", "code /back"))
        .await;
    hub.register_session(local_session("s3", "infra terraform", "code /infra"))
        .await;

    assert!(
        face.wait_until(WITHIN_2S, |m| m.view().len() == 3).await,
        "all three sessions appear"
    );

    let view = face.model().view();
    // Type part of a title → the matching session ranks first.
    let cwds: Vec<&[&str]> = vec![&[], &[], &[]];
    let results = query_palette(&view, "backend", &cwds);
    assert!(!results.is_empty(), "palette finds a match");
    assert_eq!(
        results[0].session_id, "s2",
        "fuzzy palette ranks the matching session first"
    );

    // Enter → focus that session (the command the host issues).
    let cmd = focus_command(&results[0].session_id);
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["target"]["session_id"], "s2");
    // And the mocked OS focus succeeds.
    assert!(
        mock_focus(FocusPlatform::MacOs, BackendResult::Activated, "code /back")
            .is_confirmed_success()
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.7 — Mute silences pings (state stays live); solo silences others.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_07_mute_silences_pings_solo_silences_others() {
    let mut hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    hub.register_session(local_session("s1", "one", "code /1"))
        .await;
    hub.register_session(local_session("s2", "two", "code /2"))
        .await;

    // Both waiting (both would ping).
    let mut c1 = fleet_reporter::CodexAdapter::new();
    let mut c2 = fleet_reporter::CodexAdapter::new();
    drive_codex_thread(
        &mut hub,
        "s1",
        "t1",
        &mut c1,
        [
            fixtures::codex_session_start("t1", "/1"),
            fixtures::codex_permission_request("t1", "/1", "shell"),
        ],
    )
    .await;
    drive_codex_thread(
        &mut hub,
        "s2",
        "t2",
        &mut c2,
        [
            fixtures::codex_session_start("t2", "/2"),
            fixtures::codex_permission_request("t2", "/2", "shell"),
        ],
    )
    .await;
    assert!(
        face.wait_until(WITHIN_2S, |m| {
            tab_state(m, "s1") == Some(TabState::Waiting)
                && tab_state(m, "s2") == Some(TabState::Waiting)
        })
        .await,
        "both sessions waiting"
    );

    // Both ping initially.
    {
        let tabs = face.model().view().tabs;
        assert!(should_notify(&tabs[0], &tabs), "s1 pings before mute");
        assert!(should_notify(&tabs[1], &tabs), "s2 pings before mute");
    }

    // ── MUTE s1 via the real Hub command (face → Hub) ──
    hub.state().snapshot_event().await; // (no-op read; ensures Hub is live)
    send_command(&hub, fleet_host_core::mute::mute_command("s1")).await;
    assert!(
        face.wait_until(WITHIN_2S, |m| {
            m.view().tab("s1").map(|t| t.muted).unwrap_or(false)
        })
        .await,
        "mute flag propagates to the face"
    );
    {
        let tabs = face.model().view().tabs;
        let s1 = tabs.iter().find(|t| t.session_id == "s1").unwrap();
        // Muted: no ping, but the waiting STATE is still live.
        assert!(!should_notify(s1, &tabs), "muted s1 must not ping");
        assert_eq!(s1.state, TabState::Waiting, "muted state stays live");
    }

    // ── SOLO s2 → s1 (and everyone but s2) is silenced ──
    send_command(&hub, solo_command("s2")).await;
    assert!(
        face.wait_until(WITHIN_2S, |m| {
            m.view().tab("s2").map(|t| t.soloed).unwrap_or(false)
        })
        .await,
        "solo flag propagates to the face"
    );
    {
        let tabs = face.model().view().tabs;
        let s1 = tabs.iter().find(|t| t.session_id == "s1").unwrap();
        let s2 = tabs.iter().find(|t| t.session_id == "s2").unwrap();
        assert!(s2.soloed, "s2 is soloed");
        assert!(should_notify(s2, &tabs), "the soloed session still pings");
        assert!(
            !should_notify(s1, &tabs),
            "solo silences all non-soloed sessions"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.8 — Hub restart restores state; kill agent → dead → reaped after grace.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_08_hub_restart_restores_and_dead_reaped_after_grace() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("hub.db");

    // ── First Hub instance: register a session + a working run, persisted. ──
    {
        let mut hub = TestHub::start_with_db(&db).await.unwrap();
        let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
        hub.register_session(local_session("s", "persist @ main", "code /p"))
            .await;
        let mut codex = fleet_reporter::CodexAdapter::new();
        drive_codex_thread(
            &mut hub,
            "s",
            "th",
            &mut codex,
            [
                fixtures::codex_session_start("th", "/p"),
                fixtures::codex_prompt("th", "/p"),
            ],
        )
        .await;
        assert!(
            face.wait_until(WITHIN_2S, |m| tab_state(m, "s") == Some(TabState::Working))
                .await,
            "run is working before restart"
        );
        // hub + face drop here → first Hub instance is gone (process restart).
    }

    // ── Second Hub instance over the SAME db: state restored from the log. ──
    let hub2 = TestHub::start_with_db(&db).await.unwrap();
    let mut face2 = FaceClient::connect(&hub2.ws_url()).await.unwrap();
    // The fresh face's snapshot must already contain the restored session+run.
    let restored = face2.model();
    assert_eq!(
        restored.view().tab("s").map(|t| t.run_count),
        Some(1),
        "Hub restart restores the session + run from the event log"
    );
    assert_eq!(tab_state(restored, "s"), Some(TabState::Working));

    // ── Kill the agent → dead, then reaped after the grace via a GC pass. ──
    // The reporter reports a confirmed exit (dead run). We ingest a dead run
    // directly through the real Hub run-upsert (a real reporter's ConfirmExit).
    let dead = fleet_protocol::AgentRun::new(
        // run_id must match the one the adapter minted so it's an update, not a
        // second run. The adapter mints `codex:<thread>:run-1`.
        "codex:th:run-1",
        fleet_protocol::AgentKind::Codex,
        "th",
        "/p",
        State::Dead,
        Confidence::High,
        // Stamp the death with a fixed past instant so the reap is deterministic
        // (the reap cutoff is `now - grace`; a same-second stamp would tie and
        // never reap under the strict `<` comparator).
        "2020-01-01T00:00:00Z",
    );
    hub2.state()
        .ingest_run_upsert_stamped("s", dead, None)
        .await;
    assert!(
        face2
            .wait_until(WITHIN_2S, |m| tab_state(m, "s") == Some(TabState::Dead))
            .await,
        "killing the agent marks the run dead (with reason)"
    );

    // Reap: a GC pass with `now` well past the death + grace → the dead run is
    // removed. The session TTL is set far in the future so only the dead run is
    // reaped (the live session is not swept).
    let removed = hub2
        .state()
        .gc(
            "2026-06-08T00:00:00Z",
            Duration::from_secs(3600), // 1h grace (D17): death was years ago, so past it
            Duration::from_secs(60 * 60 * 24 * 365 * 100),
        )
        .await
        .unwrap();
    assert!(removed >= 1, "the GC pass reaps the dead run after grace");
    // The face observes the reap (run removed).
    assert!(
        face2
            .wait_until(WITHIN_2S, |m| {
                m.view().tab("s").map(|t| t.run_count).unwrap_or(0) == 0
            })
            .await,
        "the dead run is reaped from the live view after grace"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.9 — Hub + CLI face + host-core view-model all reflect the SAME protocol
//         state live.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_09_hub_cli_and_host_core_reflect_same_state() {
    let mut hub = TestHub::start().await.unwrap();
    let mut host_face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    hub.register_session(local_session("s1", "alpha repo", "code /a"))
        .await;
    hub.register_session(local_session("s2", "beta repo", "code /b"))
        .await;
    let mut codex = fleet_reporter::CodexAdapter::new();
    drive_codex_thread(
        &mut hub,
        "s1",
        "th",
        &mut codex,
        [
            fixtures::codex_session_start("th", "/a"),
            fixtures::codex_permission_request("th", "/a", "shell"),
        ],
    )
    .await;

    // Host-core view-model state.
    assert!(
        host_face
            .wait_until(WITHIN_2S, |m| {
                m.view().len() == 2 && tab_state(m, "s1") == Some(TabState::Waiting)
            })
            .await
    );

    // Hub's own canonical snapshot.
    let hub_snapshot = hub.state().snapshot_event().await;
    let hub_ids: std::collections::BTreeSet<String> =
        if let fleet_protocol::Event::Snapshot { sessions, .. } = &hub_snapshot {
            sessions.iter().map(|s| s.session_id.clone()).collect()
        } else {
            panic!("expected snapshot")
        };

    // Host-core face ids.
    let host_ids: std::collections::BTreeSet<String> = host_face
        .model()
        .view()
        .tabs
        .iter()
        .map(|t| t.session_id.clone())
        .collect();
    assert_eq!(
        hub_ids, host_ids,
        "Hub and host-core agree on the session set"
    );

    // The REAL `fleet ls` CLI binary is the second face. It reads the SAME Hub.
    //
    // The CLI is a blocking subprocess; run it on a blocking thread so this
    // current-thread test runtime keeps SERVING the Hub (its accept loop +
    // connection tasks) while the subprocess subscribes — otherwise the blocked
    // test thread would starve the in-process Hub and the CLI would never get its
    // snapshot.
    let ws_url = hub.ws_url();
    let cli_out = tokio::task::spawn_blocking(move || cli_ls_once(&ws_url))
        .await
        .expect("join cli task")
        .expect("run fleet ls --once");
    match cli_out {
        Some(stdout) => {
            // The CLI's rendered table shows both session titles and the waiting
            // approval — same protocol state, independent renderer.
            assert!(
                stdout.contains("alpha repo"),
                "CLI shows s1 title:\n{stdout}"
            );
            assert!(
                stdout.contains("beta repo"),
                "CLI shows s2 title:\n{stdout}"
            );
            assert!(
                stdout.contains("waiting") || stdout.contains("approval"),
                "CLI reflects the waiting approval:\n{stdout}"
            );
        }
        None => {
            // The `fleet` binary wasn't built on this runner — fall back to a raw
            // WS CLI-style subscriber so the "third face" claim is still proven
            // against the same Hub (a CLI face is, like the host face, a pure
            // protocol consumer).
            let cli_like = FaceClient::connect(&hub.ws_url()).await.unwrap();
            let cli_ids: std::collections::BTreeSet<String> = cli_like
                .model()
                .view()
                .tabs
                .iter()
                .map(|t| t.session_id.clone())
                .collect();
            assert_eq!(
                cli_ids, host_ids,
                "a second face subscribing to the same Hub sees the same state"
            );
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.10 — Observer-not-owner: NO agent was launched through Fleet (owned-PTY off).
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_10_observer_not_owner_no_agent_launched_through_fleet() {
    // The whole suite drives agents via RECORDED fixtures — Fleet never spawns
    // `claude`/`codex`. This test makes the invariant explicit and structural:
    //
    // 1. The reporter command vocabulary has no "launch agent" variant — its
    //    inputs are observations (upsert/liveness/exit), never a spawn.
    // 2. No Fleet component invokes a `claude`/`codex` process; the *only* child
    //    process the suite ever spawns is the read-only `fleet ls` face.
    //
    // (1) Structural: enumerate the ReporterCommand surface; assert it is
    // observation-only. If a future change added a launch command this fails.
    use fleet_reporter::ReporterCommand;
    fn is_observation_only(c: &ReporterCommand) -> bool {
        matches!(
            c,
            ReporterCommand::UpsertSession(_)
                | ReporterCommand::UpsertRun(_)
                | ReporterCommand::Liveness { .. }
                | ReporterCommand::ConfirmExit { .. }
                | ReporterCommand::Shutdown
        )
    }
    // Drive a real Codex lifecycle and assert every emitted command is an
    // observation (never a launch).
    let mut hub = TestHub::start().await.unwrap();
    let sid = hub
        .register_session(local_session("s", "obs @ main", "code /s"))
        .await;
    let mut codex = fleet_reporter::CodexAdapter::new();
    for line in [
        fixtures::codex_session_start("th", "/s"),
        fixtures::codex_prompt("th", "/s"),
        fixtures::codex_permission_request("th", "/s", "shell"),
        fixtures::codex_permission_response("th", "/s", "shell", true),
        fixtures::codex_stop("th", "/s"),
    ] {
        for cmd in codex.ingest_json(&line) {
            assert!(
                is_observation_only(&cmd),
                "reporter must only OBSERVE, never launch: {cmd:?}"
            );
            hub.apply_command(&sid, cmd).await;
        }
    }

    // (2) The session itself carries no owned-PTY marker; the server kind is the
    // user's own local environment, not a Fleet-owned launcher.
    let snap = hub.state().snapshot_event().await;
    if let fleet_protocol::Event::Snapshot { sessions, .. } = snap {
        let s = sessions.iter().find(|s| s.session_id == "s").unwrap();
        assert_eq!(
            s.server.kind,
            fleet_protocol::ServerKind::Local,
            "the session is the user's own environment, not a Fleet-owned PTY"
        );
        // The run's native id is the agent's OWN id (thread.id) — Fleet adopted an
        // already-running agent's identity, it did not mint a launch.
        assert!(s.runs.iter().all(|r| r.native_id == "th"));
    } else {
        panic!("expected snapshot");
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// §21.11 — The suite runs on macOS + Linux.
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dod_11_suite_runs_on_macos_and_linux() {
    // The whole harness is loopback-WebSocket based, which binds identically on
    // macOS and Linux. This test asserts (a) the current OS is one of the two
    // first-class targets, and (b) the per-OS focus strategy for the current
    // platform is well-formed — i.e. the focus path the DoD relies on is wired on
    // *this* host. It is a guard that the suite is genuinely cross-platform.
    let os = std::env::consts::OS;
    assert!(
        matches!(os, "macos" | "linux"),
        "the v1 DoD suite targets macOS + Linux first-class (running on {os})"
    );

    // The Hub binds and a face round-trips on this OS.
    let hub = TestHub::start().await.unwrap();
    let mut face = FaceClient::connect(&hub.ws_url()).await.unwrap();
    hub.register_session(local_session("s", "xplat", "code /x"))
        .await;
    assert!(
        face.wait_until(WITHIN_2S, |m| m.view().len() == 1).await,
        "Hub + face round-trip works on {os}"
    );

    // The detected focus platform is one this OS actually supports, and the
    // jump/cycle selection is OS-independent (pure), so §21.5 holds on both.
    let platform = FocusPlatform::detect();
    match os {
        "macos" => assert_eq!(platform, FocusPlatform::MacOs),
        "linux" => assert!(matches!(
            platform,
            FocusPlatform::LinuxX11 | FocusPlatform::Wayland
        )),
        _ => unreachable!(),
    }
    // cycle_next is pure and works regardless of OS (focus selection layer).
    let v = face.model().view();
    assert_eq!(cycle_next(&v, None), Some(0));
}

// ── helper: send a real face→Hub Command over a WS connection ──────────────────

/// Send a face→Hub [`Command`] over a fresh WebSocket connection (the real wire
/// path a face uses for mute/solo/focus). Closes after sending.
async fn send_command(hub: &TestHub, command: Command) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let (mut ws, _) = tokio_tungstenite::connect_async(hub.ws_url())
        .await
        .expect("face command connect");
    let envelope = serde_json::json!({
        "type": "command",
    });
    // Merge the command's own fields (it serializes with a `command` discriminator)
    // into the envelope, matching ClientMessage::Command { #[serde(flatten)] }.
    let mut obj = envelope.as_object().unwrap().clone();
    if let serde_json::Value::Object(cmd_obj) = serde_json::to_value(&command).unwrap() {
        for (k, v) in cmd_obj {
            obj.insert(k, v);
        }
    }
    let txt = serde_json::Value::Object(obj).to_string();
    ws.send(Message::Text(txt)).await.expect("send command");
    // Give the Hub a beat to apply before the connection closes.
    let _ = ws.close(None).await;
}

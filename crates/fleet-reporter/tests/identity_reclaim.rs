//! S6 durable-identity / reclaim integration tests across the reporter and the
//! **real** `fleet-hub` server (PLAN S6 demo).
//!
//! These prove the end-to-end reclaim story the spec calls out:
//! - bounce the reporter mid-lifecycle → the entry is **reclaimed, not
//!   duplicated** (no ghost), and the buffered deltas replay across the gap;
//! - a **duplicate** + an **out-of-order** delta → **no ghost, no regression**;
//! - the reporter-stamped `(durable_id, epoch, seq)` rides the wire and the Hub
//!   applies it idempotently and in seq order.

use std::net::SocketAddr;
use std::time::Duration;

use fleet_hub::reclaim::{Decision, DurableId};
use fleet_hub::server::{run_ws_listener, HubState};
use fleet_hub::wire::SeqStamp;
use fleet_hub::StateStore;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, SCHEMA_VERSION,
};
use fleet_reporter::identity::IdentityLedger;
use fleet_reporter::{Backoff, Reporter, ReporterConfig, WsConnector};

fn session(id: &str) -> Session {
    Session {
        schema_version: SCHEMA_VERSION,
        session_id: id.into(),
        title: "s6-integ".into(),
        location: Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
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
        runs: vec![],
        rollup_state: State::Idle,
        rollup_urgency: None,
        muted: false,
        soloed: false,
        unread: false,
        tags: vec![],
        policy: None,
        updated_at: "2026-06-08T00:00:00Z".into(),
        extra: Extra::new(),
    }
}

/// A run anchored on a durable native id (the §7.5 anchor the stamp uses).
fn run(run_id: &str, native: &str, state: State) -> AgentRun {
    AgentRun::new(
        run_id,
        AgentKind::Codex,
        native,
        "/work",
        state,
        Confidence::High,
        "2026-06-08T00:00:00Z",
    )
}

fn fast_config(session_id: &str) -> ReporterConfig {
    let mut c = ReporterConfig::new(session_id);
    c.heartbeat_interval = Duration::from_millis(20);
    c.backoff = Backoff::new(Duration::from_millis(5), Duration::from_millis(50), 2);
    c
}

async fn spawn_hub() -> (String, HubState) {
    let state = HubState::new();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
    tokio::spawn(fut);
    (format!("ws://{local}"), state)
}

async fn wait_until(state: &HubState, secs: u64, pred: impl Fn(&[Session]) -> bool) {
    let check = async {
        loop {
            if let Event::Snapshot { sessions, .. } = state.snapshot_event().await {
                if pred(&sessions) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(secs), check)
        .await
        .expect("condition not met before timeout");
}

/// PLAN S6 demo (part 1): bounce the reporter mid-lifecycle → entry reclaimed,
/// not duplicated; buffered deltas replay across the gap.
#[tokio::test]
async fn reporter_bounce_reclaims_entry_no_ghost() {
    let (url, state) = spawn_hub().await;

    // Reporter #1 registers and drives working → waiting.
    let reporter = Reporter::new(
        fast_config("sess-s6"),
        Box::new(WsConnector::new(url.clone())),
    );
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));
    handle.upsert_session(session("sess-s6"));
    handle.upsert_run(run("sess-s6:run-1", "native-d", State::Working));
    handle.upsert_run(run("sess-s6:run-1", "native-d", State::Waiting));
    wait_until(&state, 5, |s| {
        s.iter()
            .any(|x| x.session_id == "sess-s6" && x.rollup_state == State::Waiting)
    })
    .await;
    handle.shutdown();
    task.await.unwrap().unwrap();

    // Reporter #2 — same session id, same durable id, RECONNECT (clean_start =
    // false, the default). It re-registers and advances the run to idle. The Hub
    // must RECLAIM the existing entry: exactly one session, one run — no ghost.
    let reporter = Reporter::new(
        fast_config("sess-s6"),
        Box::new(WsConnector::new(url.clone())),
    );
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));
    handle.upsert_session(session("sess-s6"));
    handle.upsert_run(run("sess-s6:run-1", "native-d", State::Idle));
    wait_until(&state, 5, |s| {
        s.iter()
            .any(|x| x.session_id == "sess-s6" && x.rollup_state == State::Idle)
    })
    .await;
    handle.shutdown();
    task.await.unwrap().unwrap();

    // Exactly one session, exactly one run — no ghost duplicate.
    if let Event::Snapshot { sessions, .. } = state.snapshot_event().await {
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id == "sess-s6")
            .collect();
        assert_eq!(matching.len(), 1, "reclaim: no duplicate session");
        assert_eq!(
            matching[0].runs.len(),
            1,
            "reclaim: no duplicate run (no ghost)"
        );
        assert_eq!(matching[0].runs[0].state, State::Idle, "latest state wins");
    }
}

/// PLAN S6 demo (part 2): a duplicate + an out-of-order delta delivered straight
/// to the Hub (simulating an at-least-once / reordering channel) → no ghost, no
/// regression. Driven through the Hub's stamped ingest path (the same path the
/// reporter's wire frames hit).
#[tokio::test]
async fn duplicate_and_out_of_order_over_the_wire_no_ghost_no_regression() {
    let (_url, state) = spawn_hub().await;
    state.ingest_session_upsert(session("sess-s6b")).await;

    let stamp = |seq: u64| Some(SeqStamp::new("native-d", 0, seq));

    // working(1) → waiting(2) → working(3)
    state
        .ingest_run_upsert_stamped("sess-s6b", run("r1", "native-d", State::Working), stamp(1))
        .await;
    state
        .ingest_run_upsert_stamped("sess-s6b", run("r1", "native-d", State::Waiting), stamp(2))
        .await;
    state
        .ingest_run_upsert_stamped("sess-s6b", run("r1", "native-d", State::Working), stamp(3))
        .await;

    // DUPLICATE seq 2 (waiting) — must not resurrect waiting.
    state
        .ingest_run_upsert_stamped("sess-s6b", run("r1", "native-d", State::Waiting), stamp(2))
        .await;
    // OUT-OF-ORDER stale seq 1 (idle) arriving late — must not regress.
    state
        .ingest_run_upsert_stamped("sess-s6b", run("r1", "native-d", State::Idle), stamp(1))
        .await;

    if let Event::Snapshot { sessions, .. } = state.snapshot_event().await {
        let s = sessions
            .iter()
            .find(|s| s.session_id == "sess-s6b")
            .unwrap();
        assert_eq!(s.runs.len(), 1, "NO GHOST: exactly one run");
        assert_eq!(
            s.runs[0].state,
            State::Working,
            "NO REGRESSION: state stays at the seq-3 working"
        );
    }
}

/// The reporter ledger and the Hub reclaim table agree end-to-end: stamping with
/// the reporter's [`IdentityLedger`] and gating with the Hub's `StateStore`
/// produces idempotent, ordered apply for an at-least-once redelivery + reorder.
#[test]
fn reporter_ledger_and_hub_gate_agree_under_redelivery() {
    let mut ledger = IdentityLedger::new();
    let mut store = StateStore::open_in_memory().unwrap();
    store.apply_session_upsert(session("s1")).unwrap();
    let did = DurableId::new("native-d");

    // The reporter stamps 5 ordered deltas.
    let mut frames = Vec::new();
    for st in [
        State::Working,
        State::Waiting,
        State::Working,
        State::Idle,
        State::Done,
    ] {
        let stamp = ledger.stamp("native-d").unwrap();
        frames.push((stamp.seq, st));
    }

    // The channel delivers them, then redelivers the whole batch (at-least-once),
    // then replays an out-of-order shuffle. Only the first ordered pass applies.
    let mut applied = 0;
    for &(seq, st) in &frames {
        let (d, _) = store
            .apply_run_upsert_seq("s1", run("r1", "native-d", st), &did, 0, seq)
            .unwrap();
        if d.applies() {
            applied += 1;
        }
    }
    assert_eq!(applied, 5, "each fresh ordered seq applies once");

    // Redeliver everything — all duplicates now.
    for &(seq, st) in &frames {
        let (d, _) = store
            .apply_run_upsert_seq("s1", run("r1", "native-d", st), &did, 0, seq)
            .unwrap();
        assert_eq!(d, Decision::DuplicateDrop, "redelivery is idempotent");
    }
    // Out-of-order replay of an early frame — still a drop, no regression.
    let (d, _) = store
        .apply_run_upsert_seq("s1", run("r1", "native-d", State::Working), &did, 0, 1)
        .unwrap();
    assert!(d.drops());

    // Final projection: last-writer-by-seq = the Done at seq 5; one run only.
    let s = store.engine().session("s1").unwrap();
    assert_eq!(s.runs.len(), 1);
    assert_eq!(s.runs[0].state, State::Done);
    assert_eq!(store.reclaim().high_seq(&did), Some(5));
}

//! Integration tests for the real reporter framework (REPCORE / the engineering spec)
//! against the **actual** `fleet-hub` server — not the in-memory transport.
//!
//! These prove the framework rides the existing Hub APIs:
//! - registration handshake (session appears in the Hub's projection),
//! - run deltas land and the rollup updates,
//! - **kill + restore the Hub → the reporter reconnects and reconciles** (the
//!   session reappears after a Hub restart, demo),
//! - a confirmed exit yields a `dead` run (never declared dead just because the
//!   Hub link dropped).

use std::net::SocketAddr;
use std::time::Duration;

use fleet_hub::server::{run_ws_listener, HubState};
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, SCHEMA_VERSION,
};
use fleet_reporter::{Backoff, Reporter, ReporterConfig, WsConnector};

fn session(id: &str) -> Session {
    Session {
        schema_version: SCHEMA_VERSION,
        session_id: id.into(),
        title: "repcore-integ".into(),
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

fn run(id: &str, state: State) -> AgentRun {
    AgentRun::new(
        id,
        AgentKind::Codex,
        "native",
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

/// Spin up a real Hub WS listener; return its `ws://` url and the shared state.
async fn spawn_hub() -> (String, HubState) {
    let state = HubState::new();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
    tokio::spawn(fut);
    (format!("ws://{local}"), state)
}

/// Poll the Hub projection until `pred` holds or the timeout elapses.
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

#[tokio::test]
async fn registration_handshake_and_run_delta_land_in_hub() {
    let (url, state) = spawn_hub().await;
    let reporter = Reporter::new(fast_config("sess-reg"), Box::new(WsConnector::new(url)));
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));

    handle.upsert_session(session("sess-reg"));
    handle.upsert_run(run("sess-reg:run-1", State::Working));

    // The session registers and its rollup reflects the working run.
    wait_until(&state, 5, |sessions| {
        sessions
            .iter()
            .any(|s| s.session_id == "sess-reg" && s.rollup_state == State::Working)
    })
    .await;

    handle.shutdown();
    task.await.unwrap().unwrap();
}

/// Run a Hub on its own dedicated current-thread runtime in a separate OS
/// thread. Returning the [`HubHandle`] keeps it alive; **dropping it tears the
/// whole runtime down** — listener *and* every accepted connection task — which
/// is the faithful simulation of the Hub process dying (unlike `JoinHandle::abort`,
/// which only stops the accept loop and leaves connection tasks running).
struct HubHandle {
    state: HubState,
    _shutdown: tokio::sync::oneshot::Sender<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl HubHandle {
    fn spawn_on(addr: SocketAddr) -> HubHandle {
        let state = HubState::new();
        let state_for_thread = state.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let join = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let (_local, fut) = run_ws_listener(state_for_thread, addr).await.unwrap();
                ready_tx.send(()).unwrap();
                tokio::select! {
                    _ = fut => {},
                    _ = rx => {}, // shutdown signal → return → runtime drops, killing all tasks
                }
            });
        });
        ready_rx.recv().unwrap(); // wait until the listener is bound
        HubHandle {
            state,
            _shutdown: tx,
            join: Some(join),
        }
    }

    /// Kill the Hub (drop the runtime, closing the listener + all connections).
    fn kill(mut self) {
        // Dropping `_shutdown` fires the oneshot → the runtime's block_on
        // returns → the runtime drops → all tasks abort, sockets close.
        if let Some(j) = self.join.take() {
            drop(self._shutdown);
            let _ = j.join();
        }
    }
}

#[tokio::test]
async fn reconnects_and_reconciles_after_hub_restart() {
    // demo: "kill+restore Hub → reconciles". Bind the Hub on a fixed
    // port, register, KILL the Hub (drop its whole runtime), restart on the SAME
    // port, and assert the reporter reconnects (with backoff), re-registers, and
    // replays its buffered delta — never giving up, never reporting the run dead.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let url = format!("ws://127.0.0.1:{port}");

    // Hub #1, on its own runtime/thread.
    let hub1 = HubHandle::spawn_on(addr);

    let reporter = Reporter::new(
        fast_config("sess-reconcile"),
        Box::new(WsConnector::new(url.clone())),
    );
    let (reporter, handle, rx) = reporter.with_channel();
    let rep_task = tokio::spawn(reporter.run(rx));

    handle.upsert_session(session("sess-reconcile"));
    handle.upsert_run(run("sess-reconcile:run-1", State::Working));
    wait_until(&hub1.state, 5, |s| {
        s.iter().any(|x| x.session_id == "sess-reconcile")
    })
    .await;

    // Kill Hub #1 — listener + connection tasks all die; the reporter's link breaks.
    hub1.kill();
    // Let the OS release the port.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The reporter keeps observing the agent meanwhile — it must buffer this
    // (no Hub link) and must NOT mark the run dead.
    handle.upsert_run(run("sess-reconcile:run-1", State::Waiting));

    // Hub #2 on the same port: a *fresh* empty projection.
    let hub2 = loop {
        // Retry until the port is reusable.
        if std::net::TcpListener::bind(addr).is_ok() {
            break HubHandle::spawn_on(addr);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // The reporter reconnects with backoff, re-registers, and replays the
    // buffered Waiting delta → the fresh Hub reconciles the full state.
    wait_until(&hub2.state, 10, |sessions| {
        sessions
            .iter()
            .any(|s| s.session_id == "sess-reconcile" && s.rollup_state == State::Waiting)
    })
    .await;

    // Crucially, the run was NOT reported dead by the disconnect.
    if let Event::Snapshot { sessions, .. } = hub2.state.snapshot_event().await {
        let s = sessions
            .iter()
            .find(|s| s.session_id == "sess-reconcile")
            .unwrap();
        assert_ne!(
            s.rollup_state,
            State::Dead,
            "dropped Hub link must not kill the run"
        );
    }

    handle.shutdown();
    rep_task.await.unwrap().unwrap();
    hub2.kill();
}

#[tokio::test]
async fn confirmed_exit_reports_dead() {
    let (url, state) = spawn_hub().await;
    let reporter = Reporter::new(fast_config("sess-exit"), Box::new(WsConnector::new(url)));
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));

    handle.upsert_session(session("sess-exit"));
    handle.upsert_run(run("sess-exit:run-1", State::Working));
    wait_until(&state, 5, |s| {
        s.iter()
            .any(|x| x.session_id == "sess-exit" && x.rollup_state == State::Working)
    })
    .await;

    // Authoritative process exit → the run goes dead.
    handle.confirm_exit("sess-exit:run-1", "process exited");
    wait_until(&state, 5, |s| {
        s.iter()
            .any(|x| x.session_id == "sess-exit" && x.rollup_state == State::Dead)
    })
    .await;

    handle.shutdown();
    task.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn registers_via_unix_fast_path() {
    use fleet_hub::server::run_unix_listener;
    use fleet_reporter::UnixConnector;

    let state = HubState::new();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "fleet-repcore-{}-{}.sock",
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

    let reporter = Reporter::new(
        fast_config("sess-unix"),
        Box::new(UnixConnector::new(path.clone())),
    );
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));

    handle.upsert_session(session("sess-unix"));
    handle.upsert_run(run("sess-unix:run-1", State::Working));
    wait_until(&state, 5, |s| {
        s.iter()
            .any(|x| x.session_id == "sess-unix" && x.rollup_state == State::Working)
    })
    .await;

    handle.shutdown();
    task.await.unwrap().unwrap();
    let _ = std::fs::remove_file(&path);
}

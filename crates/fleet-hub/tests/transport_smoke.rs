//! Transport smoke tests (G0 gate criterion: "transport tests (WS **and**
//! unix)").
//!
//! These drive the Hub over its real listeners with a real client:
//! - **WS**: connect over TCP WebSocket, `subscribe` → empty snapshot; push a
//!   reporter delta from a second connection → the subscriber sees the live
//!   `session.added` event. Proves the end-to-end subscribe + broadcast path.
//! - **unix** (`cfg(unix)`): the same handshake over a unix-domain socket via a
//!   WebSocket client speaking over the UDS stream — proves the D7 fast path.

use fleet_hub::server::{run_ws_listener, HubState};
use fleet_hub::wire::ClientMessage;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;

fn sample_session(id: &str) -> Session {
    Session::new(
        id,
        "repo @ main",
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        State::Idle,
        "2026-06-08T00:00:00Z",
    )
}

fn sample_run(id: &str) -> AgentRun {
    AgentRun::new(
        id,
        AgentKind::Codex,
        "thread-1",
        "/work",
        State::Working,
        Confidence::High,
        "2026-06-08T00:00:00Z",
    )
}

async fn next_event<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Event
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let msg = ws.next().await.expect("stream open").expect("frame ok");
        if let Message::Text(txt) = msg {
            return serde_json::from_str(&txt).expect("decodable event");
        }
    }
}

#[tokio::test]
async fn ws_subscribe_empty_then_live_delta() {
    let state = HubState::new();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
    tokio::spawn(fut);

    let url = format!("ws://{local}");

    // Subscriber connects and subscribes → empty snapshot.
    let (mut sub, _resp) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    sub.send(Message::Text(subscribe)).await.unwrap();
    match next_event(&mut sub).await {
        Event::Snapshot { sessions, .. } => assert!(sessions.is_empty(), "snapshot starts empty"),
        other => panic!("expected snapshot, got {other:?}"),
    }

    // A reporter connects and pushes a session + run.
    let (mut rep, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("s1"),
    })
    .unwrap();
    rep.send(Message::Text(upsert)).await.unwrap();

    // Subscriber observes the live session.added.
    match next_event(&mut sub).await {
        Event::SessionAdded { session, .. } => assert_eq!(session.session_id, "s1"),
        other => panic!("expected session.added, got {other:?}"),
    }

    // Reporter pushes a run → subscriber sees run.added + session.updated with
    // the recomputed rollup.
    let run_up = serde_json::to_string(&ClientMessage::RunUpsert {
        session_id: "s1".into(),
        run: sample_run("r1"),
        stamp: None,
    })
    .unwrap();
    rep.send(Message::Text(run_up)).await.unwrap();

    let mut saw_run_added = false;
    let mut saw_rollup_working = false;
    for _ in 0..2 {
        match next_event(&mut sub).await {
            Event::RunAdded { run, .. } => {
                assert_eq!(run.run_id, "r1");
                saw_run_added = true;
            }
            Event::SessionUpdated { session, .. } => {
                assert_eq!(session.rollup_state, State::Working);
                saw_rollup_working = true;
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert!(saw_run_added && saw_rollup_working);
}

#[tokio::test]
async fn ws_two_faces_see_identical_state() {
    // README §4.3: every face is a projection of the same Hub state.
    let state = HubState::new();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (local, fut) = run_ws_listener(state.clone(), addr).await.unwrap();
    tokio::spawn(fut);
    let url = format!("ws://{local}");

    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    for f in [&mut a, &mut b] {
        let s = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
        f.send(Message::Text(s)).await.unwrap();
        // Drain each face's snapshot.
        let _ = next_event(f).await;
    }

    // A reporter pushes a session.
    let (mut rep, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("s9"),
    })
    .unwrap();
    rep.send(Message::Text(upsert)).await.unwrap();

    // Both faces observe the identical session.added.
    let ea = next_event(&mut a).await;
    let eb = next_event(&mut b).await;
    assert_eq!(ea, eb, "both faces must see the identical event");
    match ea {
        Event::SessionAdded { session, .. } => assert_eq!(session.session_id, "s9"),
        other => panic!("expected session.added, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unix_subscribe_empty_then_live_delta() {
    use fleet_hub::server::run_unix_listener;

    let state = HubState::new();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "fleet-hub-smoke-{}-{}.sock",
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

    // Connect a unix stream and run the WS handshake over it. The client uses a
    // dummy request URI; the Hub ignores the URI and just upgrades.
    let stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (mut sub, _resp) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap();

    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    sub.send(Message::Text(subscribe)).await.unwrap();
    match next_event(&mut sub).await {
        Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
        other => panic!("expected snapshot over unix, got {other:?}"),
    }

    // Push a delta over a second unix connection.
    let stream2 = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (mut rep, _r) = tokio_tungstenite::client_async("ws://localhost/", stream2)
        .await
        .unwrap();
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("u1"),
    })
    .unwrap();
    rep.send(Message::Text(upsert)).await.unwrap();

    match next_event(&mut sub).await {
        Event::SessionAdded { session, .. } => assert_eq!(session.session_id, "u1"),
        other => panic!("expected session.added over unix, got {other:?}"),
    }

    let _ = std::fs::remove_file(&path);
}

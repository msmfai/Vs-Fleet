//! Wire-frame coverage for `serve_ws_connection`: binary frames, ping→pong,
//! undecodable text/binary, and a clean close. These drive the real WebSocket
//! server loop over a TCP socket so every inbound `Message` arm is exercised
//! end-to-end (not just the JSON `apply` path the unit tests cover).

use std::net::SocketAddr;

use fleet_hub::server::{run_ws_listener, HubState};
use fleet_hub::wire::ClientMessage;
use fleet_protocol::{
    Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn sample_session(id: &str) -> Session {
    Session::new(
        id,
        "repo",
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

async fn start() -> (SocketAddr, String) {
    let state = HubState::new();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (local, fut) = run_ws_listener(state, addr).await.unwrap();
    tokio::spawn(fut);
    (local, format!("ws://{local}"))
}

#[tokio::test]
async fn binary_subscribe_frame_yields_snapshot() {
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Send the subscribe envelope as a BINARY frame — exercises the binary arm.
    let bytes = serde_json::to_vec(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Binary(bytes.into())).await.unwrap();
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(txt) => {
                let ev: Event = serde_json::from_str(&txt).unwrap();
                assert_eq!(ev.type_name(), "fleet.snapshot");
                break;
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn unsolicited_pong_frame_is_ignored() {
    // A client-initiated Pong (an unsolicited keepalive) hits the server's
    // "ignore other frame" arm and must not disturb the connection.
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Pong(b"keepalive".to_vec().into()))
        .await
        .unwrap();
    // The connection keeps serving: a subscribe still works.
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        if let Message::Text(_) = ws.next().await.unwrap().unwrap() {
            break;
        }
    }
}

#[tokio::test]
async fn binary_delta_frame_applies_without_reply() {
    // A binary frame carrying a reporter delta (SessionUpsert) applies to None —
    // no immediate reply — exercising the binary branch's `if let Some(reply)`
    // no-op path. We confirm the state mutated by subscribing afterward.
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let bytes = serde_json::to_vec(&ClientMessage::SessionUpsert {
        session: sample_session("bn1"),
    })
    .unwrap();
    ws.send(Message::Binary(bytes.into())).await.unwrap();
    // Subscribe (text) and confirm the binary-applied session is present.
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        if let Message::Text(txt) = ws.next().await.unwrap().unwrap() {
            let ev: Event = serde_json::from_str(&txt).unwrap();
            if let Event::Snapshot { sessions, .. } = ev {
                assert!(
                    sessions.iter().any(|s| s.session_id == "bn1"),
                    "binary SessionUpsert must have applied"
                );
                break;
            }
        }
    }
}

#[tokio::test]
async fn ping_frame_is_answered_with_pong() {
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Ping(b"hi".to_vec().into())).await.unwrap();
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Pong(p) => {
                assert_eq!(&p[..], b"hi");
                break;
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn undecodable_text_is_ignored_then_connection_keeps_serving() {
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Garbage text frame: logged + ignored, connection stays open.
    ws.send(Message::Text("not json at all".to_string().into()))
        .await
        .unwrap();
    // Garbage binary frame: the binary-decode error arm.
    ws.send(Message::Binary(b"\xff\x00not-json".to_vec().into()))
        .await
        .unwrap();
    // A valid subscribe still works afterwards (proves the loop did not break).
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(txt) => {
                let ev: Event = serde_json::from_str(&txt).unwrap();
                assert_eq!(ev.type_name(), "fleet.snapshot");
                break;
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn clean_close_ends_the_connection() {
    let (_a, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Subscribe so the server task is fully engaged, then close cleanly.
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    let _ = ws.next().await; // snapshot
    ws.close(None).await.unwrap();
    // The stream drains to None after the close handshake.
    while let Some(Ok(_)) = ws.next().await {}
}

#[tokio::test]
async fn non_websocket_bytes_fail_the_handshake_cleanly() {
    use tokio::io::AsyncWriteExt;
    // A raw TCP client that sends garbage (no WS upgrade) must make the server's
    // `accept_async` fail and the connection task return without panicking.
    let (addr, _url) = start().await;
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GARBAGE NOT HTTP\r\n\r\n").await.unwrap();
    // The server closes the connection; our read returns 0 (EOF) eventually.
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 64];
    // Either an immediate close (Ok(0)) or a connection-reset error is fine — the
    // point is the server did not hang or crash.
    let _ = sock.read(&mut buf).await;
    // Server still serves new, valid connections afterwards.
    let url = format!("ws://{addr}");
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        if let Message::Text(_) = ws.next().await.unwrap().unwrap() {
            break;
        }
    }
}

#[tokio::test]
async fn abrupt_client_drop_ends_connection_without_close_frame() {
    // Dropping the client TCP stream mid-stream (no Close frame) drives the
    // server's read-error / None inbound arm and ends the task.
    let (_addr, url) = start().await;
    let (mut ws, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws.send(Message::Text(subscribe.into())).await.unwrap();
    let _ = ws.next().await; // snapshot
                             // Drop without a close handshake; the server observes the disconnect.
    drop(ws);
    // Give the server a moment to react, then prove it still accepts new clients.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (mut ws2, _r2) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    ws2.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        if let Message::Text(_) = ws2.next().await.unwrap().unwrap() {
            break;
        }
    }
}

#[tokio::test]
async fn slow_subscriber_lags_when_backlog_overflows() {
    // Deterministically drive the server's `RecvError::Lagged` arm by running
    // `serve_ws_connection` over an in-process duplex whose buffer we let fill,
    // so the server stops draining its broadcast receiver and the channel
    // overflows its 1024 capacity. We use a SHARED `HubState` so the flood and
    // the served connection observe the same broadcast sender.
    use fleet_hub::server::serve_ws_connection;

    let state = HubState::new();
    // A small duplex buffer guarantees the server's sink backpressures quickly
    // once the (non-reading) client stops draining.
    let (server_side, client_side) = tokio::io::duplex(256);
    let serve_state = state.clone();
    let server_task = tokio::spawn(async move {
        serve_ws_connection(serve_state, server_side).await;
    });

    let (mut client, _resp) = tokio_tungstenite::client_async("ws://localhost/", client_side)
        .await
        .unwrap();
    // Subscribe and drain the snapshot so the connection is attached to the
    // broadcast stream.
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    client.send(Message::Text(subscribe.into())).await.unwrap();
    loop {
        if let Message::Text(_) = client.next().await.unwrap().unwrap() {
            break;
        }
    }

    // Flood far past the broadcast capacity (1024) WITHOUT the client reading.
    // The server forwards until its sink (the tiny duplex) backpressures, then
    // blocks — so it stops calling `rx.recv()` and the channel overflows.
    for i in 0..5000 {
        state
            .ingest_session_upsert(sample_session(&format!("s{i}")))
            .await;
    }
    // Give the server task a moment to drain what it can and then block on send.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Now the client reads: the server resumes, its next `rx.recv()` returns
    // `Lagged` (logged + skipped), and the connection STAYS ALIVE — we keep
    // receiving frames rather than the connection closing.
    let mut frames = 0;
    for _ in 0..5000 {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.next()).await {
            Ok(Some(Ok(Message::Text(_)))) => {
                frames += 1;
                if frames >= 3 {
                    break; // proved it keeps delivering post-lag
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    assert!(
        frames >= 1,
        "a lagged subscriber keeps its connection and resumes delivering"
    );

    drop(client);
    let _ = server_task.await;
}

#[tokio::test]
async fn reply_send_failure_after_client_drop_ends_connection() {
    // Drive the reply-send-error `break` arm: the client subscribes then drops,
    // so the server's attempt to send the snapshot reply fails and the connection
    // task returns. We pre-populate a session so the snapshot frame is non-empty,
    // and use a duplex so dropping the client deterministically closes the peer.
    use fleet_hub::server::serve_ws_connection;

    let state = HubState::new();
    state.ingest_session_upsert(sample_session("s1")).await;

    let (server_side, client_side) = tokio::io::duplex(64);
    let serve_state = state.clone();
    let server_task = tokio::spawn(async move {
        serve_ws_connection(serve_state, server_side).await;
    });

    let (mut client, _resp) = tokio_tungstenite::client_async("ws://localhost/", client_side)
        .await
        .unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    client.send(Message::Text(subscribe.into())).await.unwrap();
    // Drop the client immediately so the server's snapshot reply send fails.
    drop(client);

    // The server task must return (not hang) once its send errors.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), server_task).await;
    assert!(
        ended.is_ok(),
        "server connection task must end after the peer drops"
    );
}

#[tokio::test]
async fn binary_reply_send_failure_after_client_drop_ends_connection() {
    // Same as above but the subscribe arrives as a BINARY frame, covering the
    // binary branch's reply-send-error `break`.
    use fleet_hub::server::serve_ws_connection;

    let state = HubState::new();
    state.ingest_session_upsert(sample_session("s1")).await;

    let (server_side, client_side) = tokio::io::duplex(64);
    let serve_state = state.clone();
    let server_task = tokio::spawn(async move {
        serve_ws_connection(serve_state, server_side).await;
    });

    let (mut client, _resp) = tokio_tungstenite::client_async("ws://localhost/", client_side)
        .await
        .unwrap();
    let bytes = serde_json::to_vec(&ClientMessage::Subscribe).unwrap();
    client.send(Message::Binary(bytes.into())).await.unwrap();
    drop(client);

    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), server_task).await;
    assert!(
        ended.is_ok(),
        "server connection task must end after the peer drops (binary path)"
    );
}

#[tokio::test]
async fn reporter_push_broadcasts_outbound_over_socket() {
    // Drives the outbound `rx.recv()` arm of the select loop end-to-end: a
    // subscriber receives a delta pushed by a separate connection.
    let (_a, url) = start().await;
    let (mut sub, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    sub.send(Message::Text(subscribe.into())).await.unwrap();
    // Drain snapshot.
    loop {
        if let Message::Text(_) = sub.next().await.unwrap().unwrap() {
            break;
        }
    }
    let (mut rep, _r2) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("bx"),
    })
    .unwrap();
    rep.send(Message::Text(upsert.into())).await.unwrap();
    loop {
        if let Message::Text(txt) = sub.next().await.unwrap().unwrap() {
            let ev: Event = serde_json::from_str(&txt).unwrap();
            if let Event::SessionAdded { session, .. } = ev {
                assert_eq!(session.session_id, "bx");
                break;
            }
        }
    }
}

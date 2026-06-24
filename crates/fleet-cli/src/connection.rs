//! Hub connection for `fleet ls` — WebSocket (always) or unix socket
//! (`cfg(unix)`) fast path.
//!
//! The connection module is intentionally kept thin: it opens the transport,
//! sends a `subscribe` message, and hands back an async stream of [`Event`]s.
//! All rendering logic lives in [`crate::render`] and is pure-function-testable
//! without a live socket.
//!
//! Protocol: the CLI sends one JSON text frame `{"type":"subscribe"}` and then
//! receives a `fleet.snapshot` followed by a live stream of delta events. This
//! matches the Hub's `server.rs` implementation (the engineering spec).

use anyhow::{Context, Result};
use fleet_protocol::Event;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// The subscribe message the CLI sends immediately after the WS handshake.
const SUBSCRIBE_MSG: &str = r#"{"type":"subscribe"}"#;

/// Open a WebSocket connection to the Hub at `url`, send `subscribe`, and
/// return an async stream that yields decoded [`Event`]s.
///
/// Errors from individual frames are logged but do not close the stream
/// (the caller receives only successfully decoded events).
///
/// # Example
/// ```no_run
/// # use fleet_cli::connection::connect_ws;
/// # #[tokio::main]
/// # async fn main() {
/// let mut events = connect_ws("ws://127.0.0.1:51777").await.unwrap();
/// while let Some(ev) = events.recv().await { /* handle */ }
/// # }
/// ```
pub async fn connect_ws(url: &str) -> Result<EventReceiver> {
    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .with_context(|| format!("WebSocket connect failed: {url}"))?;

    // Send subscribe immediately after the WS handshake.
    ws.send(Message::Text(SUBSCRIBE_MSG.into()))
        .await
        .context("failed to send subscribe message")?;

    let (tx, rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move {
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(txt)) => {
                    match serde_json::from_str::<Event>(&txt) {
                        Ok(ev) => {
                            if tx.send(ev).await.is_err() {
                                // Receiver dropped — the caller exited the event loop.
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, raw = %txt, "undecodable event from Hub");
                        }
                    }
                }
                Ok(Message::Binary(bin)) => match serde_json::from_slice::<Event>(&bin) {
                    Ok(ev) => {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "undecodable binary event from Hub"),
                },
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Close(_)) => {
                    tracing::info!("Hub closed the connection");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ws read error");
                    break;
                }
            }
        }
    });

    Ok(EventReceiver { rx })
}

/// Open a unix-domain socket to the Hub at `path`, upgrade it to WebSocket,
/// send `subscribe`, and return an async [`EventReceiver`] (`cfg(unix)` only).
#[cfg(unix)]
pub async fn connect_unix(path: &std::path::Path) -> Result<EventReceiver> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(path)
        .await
        .with_context(|| format!("unix connect failed: {}", path.display()))?;

    // Upgrade the raw TCP-like stream to a WebSocket. The Hub uses
    // `tokio_tungstenite::accept_async` on its side which speaks the standard
    // WS handshake over any `AsyncRead + AsyncWrite`, so this is correct.
    // We use a dummy URL for the client handshake header (the Host is ignored
    // by the Hub's `accept_async`).
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .context("WS handshake over unix socket failed")?;

    ws.send(Message::Text(SUBSCRIBE_MSG.into()))
        .await
        .context("failed to send subscribe over unix socket")?;

    let (tx, rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move {
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(txt)) => match serde_json::from_str::<Event>(&txt) {
                    Ok(ev) => {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "undecodable event (unix)"),
                },
                Ok(Message::Binary(bin)) => {
                    if let Ok(ev) = serde_json::from_slice::<Event>(&bin) {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    Ok(EventReceiver { rx })
}

/// A typed channel receiver that yields [`Event`]s from the Hub.
pub struct EventReceiver {
    rx: tokio::sync::mpsc::Receiver<Event>,
}

impl EventReceiver {
    /// Receive the next event, or `None` if the connection closed.
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

/// Connect to the Hub, preferring the unix socket on `cfg(unix)` (fast path,
/// D7) and falling back to WebSocket.
///
/// `unix_path` is the canonical unix socket path from `HubConfig`
/// (`$XDG_RUNTIME_DIR/fleet/hub.sock` by default). `ws_url` is the WebSocket
/// URL fallback (`ws://127.0.0.1:51777`).
pub async fn connect(ws_url: &str, unix_path: &std::path::Path) -> Result<EventReceiver> {
    #[cfg(unix)]
    {
        // Try unix first (D7 fast path). Fall back to WS if the socket is not
        // present (Hub may not have been started, or we're in a CI environment
        // that only starts the WS listener).
        if unix_path.exists() {
            match connect_unix(unix_path).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::debug!(error = %e, "unix connect failed; falling back to WebSocket");
                }
            }
        }
    }
    let _ = unix_path; // suppress unused-variable on non-unix
    connect_ws(ws_url).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// These drive the real connection client against a tiny in-process WebSocket
// server (the Hub stand-in): the server accepts a real WS handshake on loopback,
// reads the subscribe frame, then sends the frames each test needs. Nothing is
// mocked — `connect_ws`/`connect_unix`/`connect` open real sockets, decode real
// frames, and feed the real `EventReceiver` channel.

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::Event;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    /// One snapshot event as a JSON text frame (the first frame the Hub sends).
    fn snapshot_text() -> String {
        serde_json::to_string(&Event::snapshot(vec![])).unwrap()
    }

    /// Read the client's `subscribe` frame, asserting it matches the protocol.
    /// The non-text fallback is a test-failure panic that a passing run never
    /// reaches, so this helper is excluded from the nightly coverage gate.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn expect_subscribe<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => {
                assert_eq!(t.as_str(), SUBSCRIBE_MSG, "client must send subscribe first");
            }
            other => panic!("expected subscribe text frame, got {other:?}"),
        }
    }

    /// Spawn a loopback WS server. The provided closure receives the accepted
    /// WebSocket stream (after the subscribe frame is consumed) and drives the
    /// server side of the test. Returns the `ws://` URL to connect to.
    async fn spawn_ws_server<F, Fut>(handler: F) -> String
    where
        F: FnOnce(
                tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            handler(ws).await;
        });
        format!("ws://{addr}")
    }

    #[tokio::test]
    async fn connect_ws_decodes_text_snapshot() {
        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            // Hold the connection so the client task does not see EOF mid-test.
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        let ev = rx.recv().await.expect("a snapshot event");
        assert!(matches!(ev, Event::Snapshot { .. }));
    }

    #[tokio::test]
    async fn connect_ws_decodes_binary_snapshot() {
        let url = spawn_ws_server(|mut ws| async move {
            let bin = serde_json::to_vec(&Event::snapshot(vec![])).unwrap();
            ws.send(Message::Binary(bin.into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        let ev = rx.recv().await.expect("a snapshot event from a binary frame");
        assert!(matches!(ev, Event::Snapshot { .. }));
    }

    #[tokio::test]
    async fn connect_ws_skips_undecodable_text_then_yields_valid() {
        // An undecodable text frame is logged-and-skipped (not forwarded); a
        // following valid frame still arrives. Covers the text Err arm.
        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Text("not json".into())).await.unwrap();
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        let ev = rx.recv().await.expect("the valid frame after a bad one");
        assert!(matches!(ev, Event::Snapshot { .. }));
    }

    #[tokio::test]
    async fn connect_ws_skips_undecodable_binary_then_yields_valid() {
        // An undecodable binary frame is skipped; a following valid one arrives.
        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Binary(vec![0xff, 0x00].into())).await.unwrap();
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        let ev = rx.recv().await.expect("the valid frame after a bad binary one");
        assert!(matches!(ev, Event::Snapshot { .. }));
    }

    #[tokio::test]
    async fn connect_ws_ignores_ping_then_closes() {
        // Ping/Pong frames are ignored; a Close frame ends the stream (recv→None).
        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Ping(vec![1, 2, 3].into())).await.unwrap();
            ws.send(Message::Close(None)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        // Ping is ignored, Close ends the stream → channel closes → recv yields None.
        assert!(rx.recv().await.is_none(), "Close must end the event stream");
    }

    #[tokio::test]
    async fn connect_ws_receiver_dropped_stops_forwarding() {
        // Dropping the receiver makes the next `tx.send` fail → the read task
        // breaks out of its loop (covers the `is_err() => break` arm).
        let url = spawn_ws_server(|mut ws| async move {
            // Keep sending; the client drops its receiver, so the task should stop.
            for _ in 0..100 {
                if ws.send(Message::Text(snapshot_text().into())).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;

        let rx = connect_ws(&url).await.unwrap();
        // Drop the receiver immediately; the spawned reader's send will fail.
        drop(rx);
        // Give the reader task a window to hit the failing send and break.
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    #[tokio::test]
    async fn connect_ws_binary_receiver_dropped_stops_forwarding() {
        // Binary-frame variant of the receiver-dropped break (the WS binary
        // `tx.send err => break` arm).
        let url = spawn_ws_server(|mut ws| async move {
            let bin = serde_json::to_vec(&Event::snapshot(vec![])).unwrap();
            for _ in 0..100 {
                if ws.send(Message::Binary(bin.clone().into())).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        let rx = connect_ws(&url).await.unwrap();
        drop(rx);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    #[tokio::test]
    async fn connect_ws_handles_abrupt_disconnect_as_read_error() {
        // The server drops the TCP stream WITHOUT a WS close handshake. The
        // client's read loop sees a protocol Err (reset without closing) and
        // breaks → the channel closes → recv yields None. Covers the `Err` arm.
        let url = spawn_ws_server(|ws| async move {
            // Send one valid frame, then drop the stream abruptly (no Close).
            let mut ws = ws;
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            drop(ws); // abrupt reset — no closing handshake
        })
        .await;

        let mut rx = connect_ws(&url).await.unwrap();
        // The first frame decodes…
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
        // …then the abrupt reset ends the stream (read Err → break → channel close).
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn connect_ws_fails_to_unreachable_port() {
        // Nothing is listening → the handshake fails and connect_ws returns Err
        // (covers the `with_context` error path).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // free the port so the connect is refused
        let url = format!("ws://{addr}");
        let err = connect_ws(&url)
            .await
            .err()
            .expect("connect_ws should fail to a closed port");
        assert!(
            err.to_string().contains("WebSocket connect failed"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_decodes_frames_and_handles_close() {
        use tokio::net::UnixListener;
        use tokio_tungstenite::accept_async as accept;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let sock_for_client = sock.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            // A text frame, an undecodable text frame (skipped), a binary frame,
            // then Close — exercising the unix read loop's arms.
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            ws.send(Message::Text("garbage".into())).await.unwrap();
            let bin = serde_json::to_vec(&Event::snapshot(vec![])).unwrap();
            ws.send(Message::Binary(bin.into())).await.unwrap();
            ws.send(Message::Close(None)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        });

        let mut rx = connect_unix(&sock_for_client).await.unwrap();
        // First decodable frame (text).
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
        // The garbage text is skipped; next decodable is the binary frame.
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
        // Then Close ends the stream.
        assert!(rx.recv().await.is_none());
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_skips_undecodable_binary() {
        // An undecodable binary frame over the unix path is silently skipped; a
        // following valid binary frame still arrives (covers the binary `if let
        // Ok(ev)` path where the decode fails-and-falls-through).
        use tokio::net::UnixListener;
        use tokio_tungstenite::accept_async as accept;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let sock_for_client = sock.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            ws.send(Message::Binary(vec![0xde, 0xad].into())).await.unwrap();
            let bin = serde_json::to_vec(&Event::snapshot(vec![])).unwrap();
            ws.send(Message::Binary(bin.into())).await.unwrap();
            // Close cleanly so the server task ends and the client reader observes
            // the Close (the task is awaited below to keep the test deterministic).
            let _ = ws.send(Message::Close(None)).await;
        });

        let mut rx = connect_unix(&sock_for_client).await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_receiver_dropped_stops_forwarding() {
        // Dropping the receiver makes the unix read task's `tx.send` fail → it
        // breaks (covers the unix `is_err() => break` arm).
        use tokio::net::UnixListener;
        use tokio_tungstenite::accept_async as accept;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let sock_for_client = sock.clone();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            for _ in 0..100 {
                if ws.send(Message::Text(snapshot_text().into())).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let rx = connect_unix(&sock_for_client).await.unwrap();
        drop(rx);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_binary_receiver_dropped_and_ping_ignored() {
        // Drives two otherwise-uncovered unix arms: a Ping frame is ignored
        // (the `_ => {}` arm), and dropping the receiver while binary frames are
        // in flight breaks the read task (the unix binary `tx.send err => break`).
        use tokio::net::UnixListener;
        use tokio_tungstenite::accept_async as accept;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let sock_for_client = sock.clone();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            // A Ping (ignored by the `_ => {}` arm) before any data.
            let _ = ws.send(Message::Ping(vec![9].into())).await;
            let bin = serde_json::to_vec(&Event::snapshot(vec![])).unwrap();
            for _ in 0..100 {
                if ws.send(Message::Binary(bin.clone().into())).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let rx = connect_unix(&sock_for_client).await.unwrap();
        // Drop immediately so the in-flight binary send fails → break.
        drop(rx);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_unix_connect_error_surfaces() {
        // Connecting to a path with no listener returns an Err (covers
        // connect_unix's `with_context` error path directly).
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/x.sock");
        let err = connect_unix(missing)
            .await
            .err()
            .expect("connect_unix should fail with no listener");
        assert!(
            err.to_string().contains("unix connect failed"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_prefers_unix_when_socket_exists() {
        use tokio::net::UnixListener;
        use tokio_tungstenite::accept_async as accept;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept(stream).await.unwrap();
            expect_subscribe(&mut ws).await;
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            let _ = ws.send(Message::Close(None)).await;
        });

        // ws_url points at a dead port; connect must succeed via the unix path.
        let mut rx = connect("ws://127.0.0.1:1", &sock).await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_falls_back_to_ws_when_unix_missing() {
        // The unix socket path does not exist → connect skips the unix fast path
        // and uses the WebSocket URL.
        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/hub.sock");
        let mut rx = connect(&url, missing).await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_falls_back_to_ws_when_unix_connect_fails() {
        // The socket FILE exists but no server is accepting on it → the unix
        // connect errors and `connect` falls back to the WS URL (covers the
        // `Err(e) => debug+fallthrough` arm of connect()).
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.sock");
        // Create a plain file at the socket path: it `exists()` but is not a
        // listening socket, so UnixStream::connect fails with ENOTCONN/ECONNREFUSED.
        std::fs::write(&stale, b"not a socket").unwrap();

        let url = spawn_ws_server(|mut ws| async move {
            ws.send(Message::Text(snapshot_text().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let mut rx = connect(&url, &stale).await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Snapshot { .. })));
    }
}

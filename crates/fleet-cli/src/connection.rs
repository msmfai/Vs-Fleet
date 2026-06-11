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

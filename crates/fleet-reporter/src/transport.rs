//! Transport abstraction for the real reporter (the engineering spec).
//!
//! The reporter's reconnect/backoff/buffer logic must be testable without real
//! sockets, while production uses a WebSocket (always) or a unix-domain socket
//! (`cfg(unix)` fast path, D7). We get both from a small seam:
//!
//! - [`Connector`] — opens a fresh [`Connection`] to the Hub. The reporter calls
//!   it once per (re)connect attempt; a failure here is what drives backoff.
//! - [`Connection`] — a live, ordered, send-only channel of JSON frames to the
//!   Hub. (The reporter is write-mostly; S6 adds the read path for reclaim acks.)
//!
//! Both are object-safe and return boxed futures, so we avoid an `async-trait`
//! dependency and can store `Box<dyn Connector>` in the reporter.
//!
//! Production connectors live in [`WsConnector`] / [`UnixConnector`]; the
//! deterministic in-memory [`MemoryConnector`] (test-only seam) lets the unit
//! tests script connect failures, mid-flush drops, and ordered delivery.

use std::future::Future;
use std::pin::Pin;

use fleet_hub::wire::ClientMessage;

/// Boxed future alias for object-safe async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Transport-level error (connect failure or send failure).
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Could not establish a connection to the Hub.
    #[error("connect failed: {0}")]
    Connect(String),
    /// The connection dropped while sending (Hub gone, socket closed).
    #[error("send failed: {0}")]
    Send(String),
}

/// A live, ordered, send channel of JSON frames to the Hub.
pub trait Connection: Send {
    /// Send one wire message. Resolves `Ok` when the frame is handed to the
    /// transport, `Err` if the connection has dropped (which triggers a
    /// reconnect; the un-acked delta stays buffered).
    fn send<'a>(&'a mut self, msg: &'a ClientMessage) -> BoxFuture<'a, Result<(), TransportError>>;

    /// Close the connection gracefully. Best-effort; errors are ignored.
    fn close(self: Box<Self>) -> BoxFuture<'static, ()>;
}

/// Opens a fresh [`Connection`] to the Hub.
pub trait Connector: Send {
    /// Attempt to connect. A failure drives the reporter's backoff loop.
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>>;

    /// A human-readable description of the endpoint (for logs).
    fn endpoint(&self) -> String;
}

// ─────────────────────────────── WebSocket ────────────────────────────────

use futures_util::SinkExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// A WebSocket connection to the Hub (D7 — universal transport).
pub struct WsConnection {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Connection for WsConnection {
    fn send<'a>(&'a mut self, msg: &'a ClientMessage) -> BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            let json = serde_json::to_string(msg)
                .map_err(|e| TransportError::Send(format!("serialize: {e}")))?;
            self.ws
                .send(Message::Text(json.into()))
                .await
                .map_err(|e| TransportError::Send(e.to_string()))
        })
    }

    fn close(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = self.ws.close(None).await;
        })
    }
}

/// Connects to the Hub over a `ws://` URL (D7).
pub struct WsConnector {
    pub url: String,
}

impl WsConnector {
    pub fn new(url: impl Into<String>) -> Self {
        WsConnector { url: url.into() }
    }
}

impl Connector for WsConnector {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        Box::pin(async move {
            let (ws, _resp) = tokio_tungstenite::connect_async(&self.url)
                .await
                .map_err(|e| TransportError::Connect(format!("{}: {e}", self.url)))?;
            Ok(Box::new(WsConnection { ws }) as Box<dyn Connection>)
        })
    }

    fn endpoint(&self) -> String {
        self.url.clone()
    }
}

// ─────────────────────────── Unix fast path (D7) ──────────────────────────

#[cfg(unix)]
mod unix {
    use super::*;
    use tokio::net::UnixStream;

    /// A WebSocket-over-unix-socket connection (D7 fast path, `cfg(unix)`).
    pub struct UnixConnection {
        pub(super) ws: WebSocketStream<UnixStream>,
    }

    impl Connection for UnixConnection {
        fn send<'a>(
            &'a mut self,
            msg: &'a ClientMessage,
        ) -> BoxFuture<'a, Result<(), TransportError>> {
            Box::pin(async move {
                let json = serde_json::to_string(msg)
                    .map_err(|e| TransportError::Send(format!("serialize: {e}")))?;
                self.ws
                    .send(Message::Text(json.into()))
                    .await
                    .map_err(|e| TransportError::Send(e.to_string()))
            })
        }

        fn close(mut self: Box<Self>) -> BoxFuture<'static, ()> {
            Box::pin(async move {
                let _ = self.ws.close(None).await;
            })
        }
    }

    /// Connects to the Hub over a unix-domain socket path (D7 fast path).
    pub struct UnixConnector {
        pub path: std::path::PathBuf,
    }

    impl UnixConnector {
        pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
            UnixConnector { path: path.into() }
        }
    }

    impl Connector for UnixConnector {
        fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
            Box::pin(async move {
                let stream = UnixStream::connect(&self.path).await.map_err(|e| {
                    TransportError::Connect(format!("{}: {e}", self.path.display()))
                })?;
                let (ws, _resp) = tokio_tungstenite::client_async("ws://localhost/", stream)
                    .await
                    .map_err(|e| {
                        TransportError::Connect(format!(
                            "ws handshake over {}: {e}",
                            self.path.display()
                        ))
                    })?;
                Ok(Box::new(UnixConnection { ws }) as Box<dyn Connection>)
            })
        }

        fn endpoint(&self) -> String {
            format!("unix:{}", self.path.display())
        }
    }
}

#[cfg(unix)]
pub use unix::{UnixConnection, UnixConnector};

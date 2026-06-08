//! The command bridge + server registry — Fleet's phone-home endpoint.
//!
//! **Invariant: servers PUSH to Fleet; Fleet never pulls.** Each code-server runs
//! the `fleet-bridge` extension, which dials this WS server and registers itself
//! (`hello` with id + the URL Fleet should embed + a label). That registration IS
//! how a server appears in the multiplexer — there is no static server list. The
//! same connection then carries `executeCommand` forwarding (the native menu →
//! the active server). A server vanishes from the rail when its bridge drops.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::mux::Server;

/// Event emitted to the rail whenever the registered-server set changes.
pub const SERVERS_CHANGED: &str = "servers-changed";

/// A live bridge connection's outbound sender (JSON command frames).
type Tx = tokio::sync::mpsc::UnboundedSender<String>;

/// What Fleet knows about one connected server.
struct Conn {
    tx: Tx,
    url: String,
    label: String,
}

/// Registry of connected (registered) servers, keyed by server id. This is the
/// authoritative, push-driven server list — populated only by phone-home.
#[derive(Clone, Default)]
pub struct BridgeRegistry {
    inner: Arc<Mutex<HashMap<String, Conn>>>,
}

impl BridgeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently-registered servers (id, url, label), in id order for stable
    /// rail ordering. The multiplexer's server list.
    pub fn servers(&self) -> Vec<Server> {
        let Ok(map) = self.inner.lock() else {
            return Vec::new();
        };
        let mut servers: Vec<Server> = map
            .iter()
            .map(|(id, c)| Server {
                id: id.clone(),
                label: c.label.clone(),
                url: c.url.clone(),
            })
            .collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        servers
    }

    /// Forward a VS Code command id to a server's bridge (no-op if not connected).
    /// Synchronous + thread-safe — callable from the UI thread.
    pub fn send_command(&self, server_id: &str, command: &str) {
        let frame = serde_json::json!({ "type": "command", "id": command }).to_string();
        if let Ok(map) = self.inner.lock() {
            match map.get(server_id) {
                Some(c) => {
                    let sent = c.tx.send(frame).is_ok();
                    tracing::info!(%server_id, %command, sent, "forwarding command to bridge");
                }
                None => {
                    tracing::warn!(%server_id, %command, "no bridge for active server — dropped")
                }
            }
        }
    }

    fn register(&self, id: String, conn: Conn) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(id.clone(), conn);
        }
        tracing::info!(server_id = %id, "server registered (phone-home)");
    }

    fn unregister(&self, id: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(id);
        }
        tracing::info!(server_id = %id, "server deregistered (bridge dropped)");
    }
}

/// Start the bridge WS server on `127.0.0.1:port`. `app` is used to emit
/// [`SERVERS_CHANGED`] so the rail re-renders as servers come and go.
pub async fn serve(app: AppHandle, registry: BridgeRegistry, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let app = app.clone();
            let registry = registry.clone();
            tokio::spawn(handle_conn(app, stream, registry));
        }
    });
    tracing::info!(port, "command-bridge / phone-home WS server listening");
    Ok(())
}

async fn handle_conn(app: AppHandle, stream: tokio::net::TcpStream, registry: BridgeRegistry) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // The first message must be the phone-home registration.
    let (server_id, url, label) = loop {
        match read.next().await {
            Some(Ok(Message::Text(t))) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("hello") {
                        if let Some(id) = v.get("server_id").and_then(|i| i.as_str()) {
                            let url = v
                                .get("url")
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            let label = v
                                .get("label")
                                .and_then(|l| l.as_str())
                                .unwrap_or(id)
                                .to_string();
                            break (id.to_string(), url, label);
                        }
                    }
                }
            }
            Some(Ok(_)) => continue,
            _ => return,
        }
    };

    registry.register(server_id.clone(), Conn { tx, url, label });
    let _ = app.emit(SERVERS_CHANGED, registry.servers());

    loop {
        tokio::select! {
            outbound = rx.recv() => match outbound {
                Some(frame) => { if write.send(Message::Text(frame)).await.is_err() { break; } }
                None => break,
            },
            inbound = read.next() => match inbound {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
    registry.unregister(&server_id);
    let _ = app.emit(SERVERS_CHANGED, registry.servers());
}

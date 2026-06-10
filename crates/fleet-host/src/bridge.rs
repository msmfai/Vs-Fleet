//! The command bridge + server registry — Fleet's phone-home endpoint.
//!
//! **Invariant: servers PUSH to Fleet; Fleet never pulls.** Each code-server runs
//! the `fleet-bridge` extension, which dials this WS server and registers itself
//! (`hello` with id + the URL Fleet should embed + a label). That registration IS
//! how a server appears in the multiplexer — there is no static server list. The
//! same connection then carries `executeCommand` forwarding (the native menu →
//! the active server). A server vanishes from the rail when its bridge drops.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::Message;

use crate::mux::Server;

/// Event emitted to the rail whenever the registered-server set changes.
pub const SERVERS_CHANGED: &str = "servers-changed";
const BRIDGE_DROP_GRACE: Duration = Duration::from_secs(8);

/// A live bridge connection's outbound sender (JSON command frames).
type Tx = tokio::sync::mpsc::UnboundedSender<String>;

/// What Fleet knows about one connected server.
struct Conn {
    tx: Tx,
    url: String,
    label: String,
    generation: u64,
}

/// Registry of connected (registered) servers, keyed by server id. This is the
/// authoritative, push-driven server list — populated only by phone-home.
#[derive(Clone, Default)]
pub struct BridgeRegistry {
    inner: Arc<Mutex<HashMap<String, Conn>>>,
    next_generation: Arc<AtomicU64>,
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
                owned: false,
            })
            .collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        servers
    }

    /// Forward a VS Code command id to a server's bridge.
    /// Synchronous + thread-safe — callable from the UI thread.
    pub fn send_command(&self, server_id: &str, command: &str) -> bool {
        let frame = serde_json::json!({ "type": "command", "id": command }).to_string();
        if let Ok(map) = self.inner.lock() {
            match map.get(server_id) {
                Some(c) => {
                    let sent = c.tx.send(frame).is_ok();
                    tracing::info!(%server_id, %command, sent, "forwarding command to bridge");
                    sent
                }
                None => {
                    tracing::warn!(%server_id, %command, "no bridge for active server — dropped");
                    false
                }
            }
        } else {
            false
        }
    }

    /// Explicitly remove a server from the push registry, used when Fleet closes
    /// a server it spawned. The bridge drop task may still run later; generation
    /// checks make that stale unregister harmless.
    pub fn forget(&self, server_id: &str) -> bool {
        if let Ok(mut map) = self.inner.lock() {
            if map.remove(server_id).is_some() {
                tracing::info!(%server_id, "server forgotten by explicit close");
                return true;
            }
        }
        false
    }

    fn register(&self, id: String, tx: Tx, url: String, label: String) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut map) = self.inner.lock() {
            map.insert(
                id.clone(),
                Conn {
                    tx,
                    url,
                    label,
                    generation,
                },
            );
        }
        tracing::info!(server_id = %id, generation, "server registered (phone-home)");
        generation
    }

    fn unregister(&self, id: &str, generation: u64) -> bool {
        if let Ok(mut map) = self.inner.lock() {
            if map
                .get(id)
                .is_some_and(|conn| conn.generation == generation)
            {
                map.remove(id);
                tracing::info!(server_id = %id, generation, "server deregistered (bridge dropped)");
                return true;
            }
        }
        tracing::debug!(server_id = %id, generation, "stale bridge drop ignored");
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hello {
    server_id: String,
    url: String,
    label: String,
}

/// Per-host launch token for the bridge phone-home endpoint.
///
/// The bridge port is intentionally stable for local reachability, but stale
/// Fleet-spawned VS Code servers from an older host process can otherwise phone
/// home into a later Fleet launch. This token makes a bridge registration belong
/// to the current host lifetime without changing the push model.
pub fn launch_token() -> String {
    let mut bytes = [0_u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        let fallback = format!(
            "{}:{}:{:?}",
            std::process::id(),
            current_time_nanos(),
            std::thread::current().id()
        );
        for (idx, b) in fallback.bytes().enumerate() {
            bytes[idx % bytes.len()] ^= b;
        }
    }
    let mut token = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut token, "{b:02x}");
    }
    token
}

fn current_time_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// Start the bridge WS server on `127.0.0.1:port`. `app` is used to emit
/// [`SERVERS_CHANGED`] so the rail re-renders as servers come and go.
pub async fn serve(
    app: AppHandle,
    registry: BridgeRegistry,
    port: u16,
    expected_token: String,
) -> std::io::Result<()> {
    // Loopback by default; `FLEET_BRIDGE_ADDR=0.0.0.0` lets containerized servers
    // reach the bridge over the host gateway.
    let addr = std::env::var("FLEET_BRIDGE_ADDR").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = TcpListener::bind((addr.as_str(), port)).await?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let app = app.clone();
            let registry = registry.clone();
            let expected_token = expected_token.clone();
            tokio::spawn(handle_conn(app, stream, registry, expected_token));
        }
    });
    tracing::info!(port, "command-bridge / phone-home WS server listening");
    Ok(())
}

async fn handle_conn(
    app: AppHandle,
    stream: tokio::net::TcpStream,
    registry: BridgeRegistry,
    expected_token: String,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // The first message must be the phone-home registration.
    let hello = loop {
        match read.next().await {
            Some(Ok(Message::Text(t))) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    match parse_hello(&v, &expected_token) {
                        HelloDecision::Accept(hello) => break hello,
                        HelloDecision::Reject { server_id } => {
                            tracing::warn!(
                                server_id = server_id.as_deref().unwrap_or("<unknown>"),
                                "bridge hello rejected; stale or foreign launch token"
                            );
                            return;
                        }
                        HelloDecision::Ignore => {}
                    }
                }
            }
            Some(Ok(_)) => continue,
            _ => return,
        }
    };

    let generation = registry.register(
        hello.server_id.clone(),
        tx,
        hello.url.clone(),
        hello.label.clone(),
    );
    let _ = app.emit(SERVERS_CHANGED, registry.servers());
    crate::mux::refresh_menu(&app);

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
    tokio::spawn(async move {
        sleep(BRIDGE_DROP_GRACE).await;
        if registry.unregister(&hello.server_id, generation) {
            let _ = app.emit(SERVERS_CHANGED, registry.servers());
            crate::mux::refresh_menu(&app);
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
enum HelloDecision {
    Accept(Hello),
    Reject { server_id: Option<String> },
    Ignore,
}

fn parse_hello(v: &serde_json::Value, expected_token: &str) -> HelloDecision {
    if v.get("type").and_then(|t| t.as_str()) != Some("hello") {
        return HelloDecision::Ignore;
    }
    let Some(id) = v.get("server_id").and_then(|i| i.as_str()) else {
        return HelloDecision::Ignore;
    };
    let token = v.get("token").and_then(|t| t.as_str()).unwrap_or("");
    if !expected_token.is_empty() && token != expected_token {
        return HelloDecision::Reject {
            server_id: Some(id.to_string()),
        };
    }

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
    HelloDecision::Accept(Hello {
        server_id: id.to_string(),
        url,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_parser_accepts_matching_launch_token() {
        let msg = serde_json::json!({
            "type": "hello",
            "server_id": "server-1",
            "url": "http://127.0.0.1:9000/",
            "label": "alpha",
            "token": "current"
        });

        assert_eq!(
            parse_hello(&msg, "current"),
            HelloDecision::Accept(Hello {
                server_id: "server-1".into(),
                url: "http://127.0.0.1:9000/".into(),
                label: "alpha".into(),
            })
        );
    }

    #[test]
    fn hello_parser_rejects_missing_or_wrong_launch_token() {
        let missing = serde_json::json!({
            "type": "hello",
            "server_id": "server-1",
        });
        let wrong = serde_json::json!({
            "type": "hello",
            "server_id": "server-1",
            "token": "old"
        });

        assert_eq!(
            parse_hello(&missing, "current"),
            HelloDecision::Reject {
                server_id: Some("server-1".into())
            }
        );
        assert_eq!(
            parse_hello(&wrong, "current"),
            HelloDecision::Reject {
                server_id: Some("server-1".into())
            }
        );
    }

    #[test]
    fn hello_parser_ignores_non_hello_and_idless_hello() {
        assert_eq!(
            parse_hello(&serde_json::json!({"type":"query"}), "t"),
            HelloDecision::Ignore
        );
        assert_eq!(
            parse_hello(&serde_json::json!({"type":"hello"}), "t"),
            HelloDecision::Ignore
        );
    }

    #[test]
    fn launch_token_is_non_empty_hex() {
        let token = launch_token();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn send_command_reports_delivery() {
        let registry = BridgeRegistry::new();
        assert!(!registry.send_command("server-1", "workbench.action.terminal.new"));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );

        assert!(registry.send_command("server-1", "workbench.action.terminal.new"));
        let frame = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "type": "command",
                "id": "workbench.action.terminal.new",
            })
        );

        drop(rx);
        assert!(!registry.send_command("server-1", "workbench.action.files.save"));
    }
}

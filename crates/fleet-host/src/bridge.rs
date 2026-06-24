//! The command bridge + server registry — Fleet's phone-home endpoint.
//!
//! **Invariant: servers PUSH to Fleet; Fleet never pulls.** Each code-server runs
//! the `fleet-bridge` extension, which dials this WS server and registers itself
//! (`hello` with id + the URL Fleet should embed + a label). That registration IS
//! how a server appears in the multiplexer — there is no static server list. The
//! same connection can carry harness/probe command frames. A server vanishes
//! from the rail when its bridge drops.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::Path;
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
struct ServerConnSet {
    url: String,
    label: String,
    /// True once the user renamed this server; a subsequent phone-home re-register
    /// then keeps the user's label instead of overwriting it with the reported one.
    renamed: bool,
    conns: BTreeMap<u64, Tx>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Registration {
    generation: u64,
    changed: bool,
}

/// Registry of connected (registered) servers, keyed by server id. This is the
/// authoritative, push-driven server list — populated only by phone-home.
#[derive(Clone, Default)]
pub struct BridgeRegistry {
    inner: Arc<Mutex<HashMap<String, ServerConnSet>>>,
    next_generation: Arc<AtomicU64>,
}

impl BridgeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently-registered servers (id, url, label), in id order for stable
    /// rail ordering. The multiplexer's server list.
    pub fn servers(&self) -> Vec<Server> {
        let map = self.lock_map();
        let mut servers: Vec<Server> = map
            .iter()
            .map(|(id, c)| Server {
                id: id.clone(),
                label: c.label.clone(),
                url: c.url.clone(),
                owned: false,
                renamed: c.renamed,
            })
            .collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        servers
    }

    /// Forward a VS Code command id to a server's bridge.
    /// Synchronous + thread-safe — callable from the UI thread.
    #[allow(dead_code)]
    pub fn send_command(&self, server_id: &str, command: &str) -> bool {
        let frame = serde_json::json!({ "type": "command", "id": command }).to_string();
        let map = self.lock_map();
        match map.get(server_id) {
            Some(c) => {
                let sent = c
                    .conns
                    .iter()
                    .next_back()
                    .map(|(_, tx)| tx.send(frame).is_ok())
                    .unwrap_or(false);
                tracing::info!(%server_id, %command, sent, "forwarding command to bridge");
                sent
            }
            None => {
                tracing::warn!(%server_id, %command, "no bridge for active server — dropped");
                false
            }
        }
    }

    /// Explicitly remove a server from the push registry, used when Fleet closes
    /// a server it spawned. The bridge drop task may still run later; generation
    /// checks make that stale unregister harmless.
    pub fn forget(&self, server_id: &str) -> bool {
        if self.lock_map().remove(server_id).is_some() {
            tracing::info!(%server_id, "server forgotten by explicit close");
            return true;
        }
        false
    }

    pub fn rename(&self, server_id: &str, label: &str) -> bool {
        if let Some(conn) = self.lock_map().get_mut(server_id) {
            conn.label = label.to_string();
            conn.renamed = true;
            tracing::info!(%server_id, %label, "bridge server label renamed");
            return true;
        }
        false
    }

    /// Test-only: register a synthetic phone-home server (drops the bridge tx) so
    /// other modules' tests can exercise State mutations against the registry. The
    /// returned generation lets a test reconnect the same id to replay the
    /// phone-home re-register seam.
    #[cfg(test)]
    pub(crate) fn register_test_server(&self, id: &str, url: &str, label: &str) {
        // The receiver is dropped immediately: these tests assert on the registry's
        // label/renamed State, never on command delivery, so a live rx is moot.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.register(id.into(), tx, url.into(), label.into());
    }

    fn register(&self, id: String, tx: Tx, url: String, label: String) -> Registration {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let mut map = self.lock_map();
        let changed = match map.get_mut(&id) {
            Some(entry) => {
                // A user rename pins the label: a reconnecting reporter must not
                // clobber it with the auto-reported one.
                let label_changes = !entry.renamed && entry.label != label;
                let changed = entry.url != url || label_changes;
                entry.url = url;
                if label_changes {
                    entry.label = label;
                }
                entry.conns.insert(generation, tx);
                changed
            }
            None => {
                map.insert(
                    id.clone(),
                    ServerConnSet {
                        url,
                        label,
                        renamed: false,
                        conns: BTreeMap::from([(generation, tx)]),
                    },
                );
                true
            }
        };
        drop(map);
        tracing::info!(server_id = %id, generation, changed, "server registered (phone-home)");
        Registration {
            generation,
            changed,
        }
    }

    fn unregister(&self, id: &str, generation: u64) -> bool {
        let mut map = self.lock_map();
        if let Some(entry) = map.get_mut(id) {
            if entry.conns.remove(&generation).is_some() {
                if entry.conns.is_empty() {
                    map.remove(id);
                    tracing::info!(
                        server_id = %id,
                        generation,
                        "server deregistered (last bridge dropped)"
                    );
                    return true;
                }
                let remaining = entry.conns.len();
                tracing::debug!(
                    server_id = %id,
                    generation,
                    remaining,
                    "bridge dropped; server still has live bridge connections"
                );
                return false;
            }
        }
        tracing::debug!(server_id = %id, generation, "stale bridge drop ignored");
        false
    }

    /// Lock the registry map, recovering the guard if a previous holder panicked
    /// (a poisoned registry is still safe to read/mutate — entries are independent).
    fn lock_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, ServerConnSet>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hello {
    server_id: String,
    url: String,
    label: String,
}

/// Load or create the bridge phone-home token.
///
/// The bridge port is intentionally stable for local reachability. Persisting
/// the token lets Fleet cold-reboot while already-running VS Code web servers
/// reconnect with the token they were launched with, without admitting arbitrary
/// local processes that do not know the token.
pub fn launch_token_from_path(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let token = raw.trim();
            if is_launch_token(token) {
                return token.to_string();
            }
            tracing::warn!(path = %path.display(), "bridge token file is invalid; replacing it");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "bridge token file unreadable; replacing it")
        }
    }

    let token = launch_token();
    if let Err(e) = write_launch_token(path, &token) {
        tracing::warn!(path = %path.display(), error = %e, "bridge token could not be persisted");
    }
    token
}

fn write_launch_token(path: &Path, token: &str) -> std::io::Result<()> {
    // The bridge token path always has a parent (it lives under the runtime dir).
    let parent = path
        .parent()
        .expect("bridge token path always has a parent");
    std::fs::create_dir_all(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn is_launch_token(token: &str) -> bool {
    token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// Generate a fresh launch token for the bridge phone-home endpoint.
pub fn launch_token() -> String {
    let mut bytes = [0_u8; 16];
    let from_urandom = read_urandom(&mut bytes);
    token_from_entropy(bytes, from_urandom)
}

/// Finish a token from 16 entropy bytes: if they did NOT come from urandom, mix
/// in the pid/time/thread fallback first, then hex-encode. Pure (both arms
/// reachable from a test).
fn token_from_entropy(mut bytes: [u8; 16], from_urandom: bool) -> String {
    if !from_urandom {
        mix_fallback_entropy(&mut bytes);
    }
    encode_token(&bytes)
}

/// Fill `bytes` from `/dev/urandom`, returning whether it succeeded. Glue: the
/// failure path can't be induced in a test (urandom is always present), so the
/// fallback it triggers is exercised directly via `mix_fallback_entropy`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn read_urandom(bytes: &mut [u8; 16]) -> bool {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(bytes))
        .is_ok()
}

/// XOR pid/time/thread-id entropy into `bytes` — the no-urandom fallback. Pure.
fn mix_fallback_entropy(bytes: &mut [u8; 16]) {
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

/// Lowercase-hex encode a token's bytes. Pure.
fn encode_token(bytes: &[u8]) -> String {
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
// Glue: binds a real TCP listener and drives the Tauri `AppHandle` (window
// emits, menu refresh) per accepted WS connection — needs a live webview, so
// it can't run headless in CI. The wire decisions it delegates to (`parse_hello`)
// and the registry mutations (`register`/`unregister`) are unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
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

// Glue: the per-connection WS loop — accepts the upgrade, folds the hello/command
// frames, and emits SERVERS_CHANGED / refreshes the menu through the `AppHandle`.
// Needs a live webview; the pure decisions (`parse_hello`) and registry mutations
// it calls are unit-tested separately.
#[cfg_attr(coverage_nightly, coverage(off))]
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

    let registration = registry.register(
        hello.server_id.clone(),
        tx,
        hello.url.clone(),
        hello.label.clone(),
    );
    if registration.changed {
        let _ = app.emit(SERVERS_CHANGED, registry.servers());
        crate::mux::refresh_menu(&app);
    }

    loop {
        tokio::select! {
            outbound = rx.recv() => match outbound {
                Some(frame) => { if write.send(Message::Text(frame.into())).await.is_err() { break; } }
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
        if registry.unregister(&hello.server_id, registration.generation) {
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
    use std::path::PathBuf;

    fn temp_token_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-bridge-token-{name}-{}-{}",
            std::process::id(),
            current_time_nanos()
        ))
    }

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
    fn encode_token_is_lowercase_hex_of_every_byte() {
        assert_eq!(encode_token(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(encode_token(&[]), "");
    }

    #[test]
    fn token_from_entropy_mixes_fallback_only_when_not_from_urandom() {
        // From urandom: bytes are hex-encoded verbatim.
        let bytes = [0xAB_u8; 16];
        assert_eq!(token_from_entropy(bytes, true), "ab".repeat(16));
        // Not from urandom: the fallback perturbs the (zeroed) buffer first.
        let mixed = token_from_entropy([0_u8; 16], false);
        assert_eq!(mixed.len(), 32);
        assert_ne!(mixed, "00".repeat(16));
    }

    #[test]
    fn mix_fallback_entropy_perturbs_the_buffer() {
        // The no-urandom fallback XORs pid/time/thread entropy in place; a
        // zeroed buffer becomes non-zero, and the result is still 16 bytes.
        let mut bytes = [0_u8; 16];
        mix_fallback_entropy(&mut bytes);
        assert!(bytes.iter().any(|b| *b != 0), "fallback must perturb bytes");
        // It always yields a well-formed 32-hex-char token.
        let token = encode_token(&bytes);
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn launch_token_from_path_reuses_existing_valid_token() {
        let path = temp_token_path("reuse");
        let token = "0123456789abcdef0123456789abcdef";
        std::fs::write(&path, format!("{token}\n")).unwrap();

        assert_eq!(launch_token_from_path(&path), token);
        assert_eq!(launch_token_from_path(&path), token);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn launch_token_from_path_creates_and_repairs_token_file() {
        let path = temp_token_path("repair");

        let first = launch_token_from_path(&path);
        assert!(is_launch_token(&first));
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), first);

        std::fs::write(&path, "not-a-valid-token").unwrap();
        let repaired = launch_token_from_path(&path);
        assert!(is_launch_token(&repaired));
        assert_ne!(repaired, "not-a-valid-token");
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), repaired);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn launch_token_from_path_regenerates_when_path_is_unreadable() {
        // A directory at the token path makes read_to_string fail with a
        // non-NotFound error (the "unreadable; replacing it" branch). Writing the
        // replacement also fails (can't write a file over a dir), but the function
        // still returns a fresh valid token rather than erroring.
        let dir = temp_token_path("unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        let token = launch_token_from_path(&dir);
        assert!(is_launch_token(&token));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_launch_token_creates_parent_and_persists_value() {
        let base = temp_token_path("write-token");
        let path = base.join("nested").join("bridge.token");
        let _ = std::fs::remove_dir_all(&base);

        write_launch_token(&path, "0123456789abcdef0123456789abcdef").unwrap();
        let stored = std::fs::read_to_string(&path).unwrap();
        assert_eq!(stored.trim(), "0123456789abcdef0123456789abcdef");
        assert!(stored.ends_with('\n'));

        let _ = std::fs::remove_dir_all(&base);
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

    #[test]
    fn duplicate_bridge_connections_do_not_churn_visible_server() {
        let registry = BridgeRegistry::new();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let first = registry.register(
            "server-1".into(),
            tx1,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        assert!(first.changed);

        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let second = registry.register(
            "server-1".into(),
            tx2,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        assert!(!second.changed);
        assert_eq!(registry.servers().len(), 1);

        assert!(!registry.unregister("server-1", first.generation));
        assert_eq!(registry.servers().len(), 1);

        assert!(registry.send_command("server-1", "workbench.action.files.save"));
        let frame = rx2.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(parsed["id"], "workbench.action.files.save");

        assert!(registry.unregister("server-1", second.generation));
        assert!(registry.servers().is_empty());
    }

    #[test]
    fn bridge_metadata_change_reports_visible_server_change() {
        let registry = BridgeRegistry::new();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let first = registry.register(
            "server-1".into(),
            tx1,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        assert!(first.changed);

        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let second = registry.register(
            "server-1".into(),
            tx2,
            "http://127.0.0.1:9000/".into(),
            "Project API".into(),
        );
        assert!(second.changed);
        assert_eq!(registry.servers()[0].label, "Project API");
    }

    #[test]
    fn forget_removes_registered_server_and_reports_presence() {
        let registry = BridgeRegistry::new();
        // Forgetting an unknown server is a no-op (false).
        assert!(!registry.forget("server-1"));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        assert_eq!(registry.servers().len(), 1);

        // An explicit close forgets it and reports it was present.
        assert!(registry.forget("server-1"));
        assert!(registry.servers().is_empty());
        // Forgetting again is a no-op.
        assert!(!registry.forget("server-1"));
    }

    #[test]
    fn rename_unknown_server_is_a_noop() {
        let registry = BridgeRegistry::new();
        assert!(!registry.rename("missing", "Nope"));
    }

    #[test]
    fn registry_recovers_from_a_poisoned_lock_without_panicking() {
        let registry = BridgeRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        // Poison the inner registry map from a panicking thread.
        let inner = registry.inner.clone();
        let result = std::thread::spawn(move || {
            let _guard = inner.lock().unwrap();
            panic!("poison the registry map");
        })
        .join();
        assert!(result.is_err());
        assert!(registry.inner.is_poisoned());

        // Readers/mutators recover the guard rather than propagating the poison:
        // the server is still visible, rename works, and forget removes it.
        assert_eq!(registry.servers().len(), 1);
        assert!(registry.rename("server-1", "Renamed"));
        assert_eq!(registry.servers()[0].label, "Renamed");
        assert!(registry.forget("server-1"));
        assert!(registry.servers().is_empty());
        assert!(!registry.forget("server-1"));
        assert!(!registry.rename("server-1", "x"));
        assert!(!registry.send_command("server-1", "noop"));
    }

    #[test]
    fn unregister_ignores_unknown_server_and_stale_generation() {
        let registry = BridgeRegistry::new();
        // Unknown server id ⇒ false (the early "stale bridge drop" path).
        assert!(!registry.unregister("never", 0));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = registry.register(
            "server-1".into(),
            tx,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );
        // A generation that was never inserted for this server ⇒ false (stale).
        assert!(!registry.unregister("server-1", reg.generation + 999));
        // The real generation drops the last bridge ⇒ true + server removed.
        assert!(registry.unregister("server-1", reg.generation));
        assert!(registry.servers().is_empty());
    }

    #[test]
    fn rename_updates_registered_server_label() {
        let registry = BridgeRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx,
            "http://127.0.0.1:9000/".into(),
            "server-1".into(),
        );

        assert!(registry.rename("server-1", "Project API"));
        assert_eq!(registry.servers()[0].label, "Project API");
        // The rename pins the label: the `renamed` flag tells the rail to show it
        // verbatim instead of letting the agent/session title override it.
        assert!(registry.servers()[0].renamed);
        assert!(!registry.rename("missing", "Nope"));
    }

    // Regression: a user rename must survive a reporter reconnect. Previously a
    // re-register (phone-home) overwrote the label with the auto-reported one, so
    // the rename silently reverted a moment after it was applied.
    #[test]
    fn rename_survives_phone_home_reregister() {
        let registry = BridgeRegistry::new();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx1,
            "http://127.0.0.1:9000/".into(),
            "auto-reported".into(),
        );
        assert!(registry.rename("server-1", "My Project"));

        // The reporter reconnects and re-registers with its auto label again.
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        registry.register(
            "server-1".into(),
            tx2,
            "http://127.0.0.1:9000/".into(),
            "auto-reported".into(),
        );

        let server = &registry.servers()[0];
        assert_eq!(
            server.label, "My Project",
            "reconnect must not clobber the rename"
        );
        assert!(server.renamed);
    }
}

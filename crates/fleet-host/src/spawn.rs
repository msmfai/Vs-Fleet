//! Server supervisor — Fleet **spawns** new code-servers (and closes ones it
//! spawned). A spawned server is launched with the phone-home env, so it dials
//! Fleet's bridge and appears in the rail on its own — Fleet never pulls it.
//!
//! Launches **code-server** (license-clean, Open-VSX — the product editor) with a
//! SHARED `--extensions-dir` (the `fleet-bridge` installed once) and a PER-SERVER
//! `--user-data-dir` (so concurrent servers don't collide). Configurable via env:
//!   - `FLEET_EDITOR_BIN`        — the code-server binary (default `code-server`)
//!   - `FLEET_EDITOR_EXTENSIONS_DIR` — shared extensions dir (with fleet-bridge)

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Tracks Fleet-spawned servers: their child processes (to close them) and their
/// [`Server`] identity (the rail's source of truth for servers Fleet created — it
/// knows these directly, since it made them; that is not "pulling").
pub struct ServerSupervisor {
    children: Mutex<HashMap<String, Child>>,
    servers: Mutex<Vec<crate::mux::Server>>,
    counter: AtomicU64,
    bridge_port: u16,
}

impl ServerSupervisor {
    pub fn new(bridge_port: u16) -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            servers: Mutex::new(Vec::new()),
            counter: AtomicU64::new(1),
            bridge_port,
        }
    }

    /// The servers Fleet has spawned (and not yet closed).
    pub fn servers(&self) -> Vec<crate::mux::Server> {
        self.servers.lock().unwrap().clone()
    }

    /// Spawn a new code-server and record it. Returns its [`Server`]; Fleet adds it
    /// to the rail immediately (it created it). The bridge connects later, when the
    /// editor opens this server's workbench, for command forwarding.
    pub fn spawn(&self) -> std::io::Result<crate::mux::Server> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("server-{n}");
        let port = free_port()?;

        let base = std::env::temp_dir().join("fleet-mux");
        let folder = base.join(format!("ws-{id}"));
        let _ = std::fs::create_dir_all(&folder);
        let _ = std::fs::write(
            folder.join(format!("{id}.md")),
            format!("# {id}\n\nSpawned by Fleet at port {port}.\n"),
        );
        // Shared extensions dir (fleet-bridge installed once); PER-SERVER user-data
        // dir so concurrent code-servers don't collide.
        let exts_dir = std::env::var("FLEET_EDITOR_EXTENSIONS_DIR")
            .unwrap_or_else(|_| base.join("cs-exts").to_string_lossy().into_owned());
        let user_data = base.join(format!("cs-userdata-{id}"));
        let editor = std::env::var("FLEET_EDITOR_BIN").unwrap_or_else(|_| "code-server".into());
        let url = format!("http://127.0.0.1:{port}/?folder={}", folder.display());

        let child = Command::new(&editor)
            .args([
                "--bind-addr",
                &format!("127.0.0.1:{port}"),
                "--auth",
                "none",
                "--disable-telemetry",
                "--disable-update-check",
                "--extensions-dir",
                &exts_dir,
                "--user-data-dir",
                &user_data.to_string_lossy(),
                &folder.to_string_lossy(),
            ])
            .env(
                "FLEET_BRIDGE_URL",
                format!("ws://127.0.0.1:{}", self.bridge_port),
            )
            .env("FLEET_SERVER_ID", &id)
            .env("FLEET_SERVER_LABEL", &id)
            .env("FLEET_SERVER_URL", &url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        tracing::info!(%id, port, "spawned code-server");
        let server = crate::mux::Server {
            id: id.clone(),
            label: id.clone(),
            url,
        };
        self.children.lock().unwrap().insert(id.clone(), child);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    /// Close (kill) a Fleet-spawned server and drop it from the rail.
    pub fn close(&self, id: &str) -> bool {
        self.servers.lock().unwrap().retain(|s| s.id != id);
        if let Some(mut child) = self.children.lock().unwrap().remove(id) {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!(%id, "closed spawned server");
            true
        } else {
            false
        }
    }
}

/// Pick a free loopback port (small TOCTOU window; fine for local spawn).
fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

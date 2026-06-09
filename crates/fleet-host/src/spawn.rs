//! Server supervisor — Fleet **spawns** new code-servers (and closes ones it
//! spawned). A spawned server is launched with the phone-home env, so it dials
//! Fleet's bridge and appears in the rail on its own — Fleet never pulls it.
//!
//! Launches **code-server** (license-clean, Open-VSX — the product editor) with a
//! SHARED `--extensions-dir` (the `fleet-bridge` installed once) and a PER-SERVER
//! `--user-data-dir` (so concurrent servers don't collide).
//!
//! Each server also gets the **agent-state pipeline** so its rail tab reflects a
//! running agent:
//!   - a per-server `fleet-reporter --serve` (session id = the server id) that
//!     phones home to the Hub and listens on a reporter socket;
//!   - a `claude` **shim** prepended to the code-server terminal's PATH, wrapping
//!     the real `claude` with `--settings <fleet-hooks>` whose hooks relay each
//!     lifecycle payload to that socket. So `claude` in the server's terminal
//!     lights up its tab (working / waiting / idle) with zero user setup.
//!
//! Env knobs: `FLEET_EDITOR_BIN`, `FLEET_EDITOR_EXTENSIONS_DIR`,
//! `FLEET_REPORTER_BIN`, `FLEET_CLAUDE_BIN`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Tracks Fleet-spawned servers: their child processes (code-server + reporter,
/// to close them) and their [`Server`] identity (the rail's source of truth for
/// servers Fleet created — it knows these directly, since it made them).
pub struct ServerSupervisor {
    children: Mutex<HashMap<String, Vec<Child>>>,
    servers: Mutex<Vec<crate::mux::Server>>,
    counter: AtomicU64,
    bridge_port: u16,
    hub_url: String,
}

impl ServerSupervisor {
    pub fn new(bridge_port: u16, hub_url: String) -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            servers: Mutex::new(Vec::new()),
            counter: AtomicU64::new(1),
            bridge_port,
            hub_url,
        }
    }

    /// The servers Fleet has spawned (and not yet closed).
    pub fn servers(&self) -> Vec<crate::mux::Server> {
        self.servers.lock().unwrap().clone()
    }

    /// Spawn a new code-server (+ its reporter + claude shim) and record it.
    /// Returns its [`Server`]; Fleet adds it to the rail immediately.
    pub fn spawn(&self) -> std::io::Result<crate::mux::Server> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("server-{n}");
        let port = free_port()?;

        let base = std::env::temp_dir().join("fleet-mux");
        let folder = base.join(format!("ws-{id}"));
        let _ = std::fs::create_dir_all(&folder);
        let _ = std::fs::write(
            folder.join(format!("{id}.md")),
            format!("# {id}\n\nSpawned by Fleet at port {port}.\nRun `claude` in the terminal — this tab will light up.\n"),
        );

        // --- agent-state pipeline: a per-server reporter + a claude shim ---------
        let reporter_socket = base.join(format!("reporter-{id}.sock"));
        let _ = std::fs::remove_file(&reporter_socket);
        let mut children: Vec<Child> = Vec::new();
        match self.spawn_reporter(&id, &reporter_socket) {
            Ok(child) => children.push(child),
            Err(e) => {
                tracing::warn!(%id, error = %e, "reporter not started (no agent state)")
            }
        }
        let shim_dir = base.join(format!("shim-{id}"));
        let shim_path = install_claude_shim(&shim_dir, &reporter_socket)
            .map_err(|e| tracing::warn!(%id, error = %e, "claude shim not installed"))
            .ok();

        // --- the editor: code-server --------------------------------------------
        let exts_dir = std::env::var("FLEET_EDITOR_EXTENSIONS_DIR")
            .unwrap_or_else(|_| base.join("cs-exts").to_string_lossy().into_owned());
        let user_data = base.join(format!("cs-userdata-{id}"));
        let editor = std::env::var("FLEET_EDITOR_BIN").unwrap_or_else(|_| "code-server".into());
        let url = format!("http://127.0.0.1:{port}/?folder={}", folder.display());
        let (cs_out, cs_err) = log_files(&format!("cs-{id}"));

        // Prepend the shim dir so the terminal's `claude` is Fleet-wrapped.
        let path = match (&shim_path, std::env::var("PATH")) {
            (Some(_), Ok(p)) => format!("{}:{}", shim_dir.display(), p),
            (_, Ok(p)) => p,
            _ => shim_dir.display().to_string(),
        };

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
            .env("PATH", path)
            .env("FLEET_REPORTER_SOCKET", &reporter_socket)
            .env("FLEET_SESSION_ID", &id)
            .env(
                "FLEET_BRIDGE_URL",
                format!("ws://127.0.0.1:{}", self.bridge_port),
            )
            .env("FLEET_SERVER_ID", &id)
            .env("FLEET_SERVER_LABEL", &id)
            .env("FLEET_SERVER_URL", &url)
            .stdout(cs_out)
            .stderr(cs_err)
            .spawn()?;
        children.push(child);

        tracing::info!(%id, port, "spawned code-server + reporter");
        let server = crate::mux::Server {
            id: id.clone(),
            label: id.clone(),
            url,
        };
        self.children.lock().unwrap().insert(id.clone(), children);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    /// Launch this server's `fleet-reporter --serve` (session id = server id).
    fn spawn_reporter(&self, id: &str, socket: &Path) -> std::io::Result<Child> {
        let bin = std::env::var("FLEET_REPORTER_BIN").unwrap_or_else(|_| "fleet-reporter".into());
        let (out, err) = log_files(&format!("reporter-{id}"));
        Command::new(bin)
            .args([
                "--serve",
                "--ws",
                &self.hub_url,
                "--socket",
                &socket.to_string_lossy(),
                "--session-id",
                id,
            ])
            .stdout(out)
            .stderr(err)
            .spawn()
    }

    /// Close (kill) a Fleet-spawned server — its code-server AND reporter — and
    /// drop it from the rail.
    pub fn close(&self, id: &str) -> bool {
        self.servers.lock().unwrap().retain(|s| s.id != id);
        if let Some(children) = self.children.lock().unwrap().remove(id) {
            for mut child in children {
                let _ = child.kill();
                let _ = child.wait();
            }
            tracing::info!(%id, "closed spawned server");
            true
        } else {
            false
        }
    }
}

/// Install a `claude` shim in `shim_dir` that wraps the real `claude` with a
/// `--settings` file whose hooks relay lifecycle payloads to `reporter_socket`.
/// Returns the shim binary path. Skipped (Err) if the real `claude` isn't found.
fn install_claude_shim(shim_dir: &Path, reporter_socket: &Path) -> std::io::Result<PathBuf> {
    let real = find_real_claude(shim_dir).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "real `claude` not on PATH")
    })?;
    std::fs::create_dir_all(shim_dir)?;

    let hooks_file = shim_dir.join("fleet-hooks.json");
    std::fs::write(
        &hooks_file,
        serde_json::to_vec_pretty(&claude_hooks_settings(reporter_socket))?,
    )?;

    // exec the REAL claude (never the shim — would recurse) with our hooks file.
    let shim = shim_dir.join("claude");
    let script = format!(
        "#!/bin/sh\n# Fleet claude shim — adds lifecycle hooks that relay to this\n# server's reporter, then runs the real claude unchanged.\nexec '{}' --settings '{}' \"$@\"\n",
        real.display(),
        hooks_file.display(),
    );
    std::fs::write(&shim, script)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    Ok(shim)
}

/// Find the real `claude` binary: `FLEET_CLAUDE_BIN`, else the first `claude` on
/// PATH that isn't our shim dir (so the shim never execs itself).
fn find_real_claude(shim_dir: &Path) -> Option<PathBuf> {
    if let Ok(bin) = std::env::var("FLEET_CLAUDE_BIN") {
        let p = PathBuf::from(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        if dir.is_empty() || Path::new(dir) == shim_dir {
            continue;
        }
        let cand = Path::new(dir).join("claude");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// The `--settings` hooks document: every relayed Claude lifecycle event sends its
/// payload (tagged `claude`, CR/LF stripped) to the reporter socket via `nc -U`.
/// `|| true` keeps Claude's flow alive on any relay error (observer, never denies).
fn claude_hooks_settings(reporter_socket: &Path) -> serde_json::Value {
    let relay = format!(
        "printf 'claude %s\\n' \"$(cat | tr -d '\\r\\n')\" | nc -U '{}' 2>/dev/null || true",
        reporter_socket.display()
    );
    let plain = serde_json::json!([{ "hooks": [{ "type": "command", "command": relay }] }]);
    let matched =
        serde_json::json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": relay }] }]);
    serde_json::json!({
        "hooks": {
            "SessionStart": plain,
            "UserPromptSubmit": plain,
            "PreToolUse": matched,
            "PostToolUse": matched,
            "Stop": plain,
            "SessionEnd": plain,
        }
    })
}

/// Pick a free loopback port (small TOCTOU window; fine for local spawn).
fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// `(stdout, stderr)` redirected to `<temp>/fleet-mux/<name>.log` so spawned
/// processes are debuggable (falls back to null if the file can't be created).
fn log_files(name: &str) -> (Stdio, Stdio) {
    let path = std::env::temp_dir()
        .join("fleet-mux")
        .join(format!("{name}.log"));
    match std::fs::File::create(&path).and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
        Err(_) => (Stdio::null(), Stdio::null()),
    }
}

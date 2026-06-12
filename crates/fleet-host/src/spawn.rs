//! Server supervisor — Fleet **spawns** new VS Code web servers (and closes ones it
//! spawned). A spawned server is launched with the phone-home env, so it dials
//! Fleet's bridge and appears in the rail on its own — Fleet never pulls it.
//!
//! Launches Microsoft's official **VS Code** web server (`code serve-web`) with a SHARED
//! `fleet-bridge` extension install and a PER-SERVER `--server-data-dir` (so concurrent
//! servers don't collide). `serve-web` reads extensions from `<server-data-dir>/extensions`,
//! so Fleet installs the bridge there before launch. Fleet uses the official server for
//! the full MS Marketplace and clean aarch64-darwin packaging in user-provided local
//! VS Code installs.
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
//! Env knobs: `FLEET_EDITOR_BIN`, `FLEET_CODE_SERVER_BIN`, `FLEET_BRIDGE_VSIX`,
//! `FLEET_REPORTER_BIN`, `FLEET_CLAUDE_BIN`, `FLEET_MUX_DIR`.
//!
//! ## Spawn modes
//! `FLEET_SPAWN_MODE` selects how a server is launched:
//!   - `local` (default) — the local-process path above (code-server + reporter +
//!     claude shim on the host). Unchanged.
//!   - `container` — `docker run` the `fleet-env` image (code-server + reporter +
//!     bridge baked in). The container phones home to Fleet's bridge on its own
//!     (`FLEET_HOST_ADDR` → the host gateway, `FLEET_BRIDGE_PORT` → Fleet's bridge),
//!     so it appears in the rail like any other env. We publish a free host port to
//!     the container's `:8080`, inspect the container for the reachable URL, and on
//!     close `docker rm -f` it. This mirrors the eval harness's `docker run` contract
//!     (see `containers/fleet-env/eval/harness.mjs`).
//!
//! Container-mode knobs: `FLEET_SPAWN_IMAGE` (default `fleet-env:latest`),
//! `FLEET_DOCKER_BIN` (default `docker`), `FLEET_HOST_ADDR` (default
//! `host.docker.internal` — how the container dials back to Fleet).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

/// Tracks Fleet-spawned servers: their child processes (code-server + reporter,
/// to close them) and their [`Server`] identity (the rail's source of truth for
/// servers Fleet created — it knows these directly, since it made them).
pub struct ServerSupervisor {
    children: Mutex<HashMap<String, Vec<Child>>>,
    /// `server id -> docker container name` for container-mode servers (so `close`
    /// knows to `docker rm` rather than kill child processes).
    containers: Mutex<HashMap<String, String>>,
    servers: Mutex<Vec<crate::mux::Server>>,
    counter: AtomicU64,
    launch_id: String,
    bridge_port: u16,
    hub_url: String,
    bridge_token: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    pub mode: Option<String>,
    pub folder: Option<String>,
}

impl ServerSupervisor {
    pub fn new(bridge_port: u16, hub_url: String, bridge_token: String) -> Self {
        Self::new_with_launch_id(bridge_port, hub_url, bridge_token, launch_id())
    }

    fn new_with_launch_id(
        bridge_port: u16,
        hub_url: String,
        bridge_token: String,
        launch_id: String,
    ) -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            containers: Mutex::new(HashMap::new()),
            servers: Mutex::new(Vec::new()),
            counter: AtomicU64::new(1),
            launch_id,
            bridge_port,
            hub_url,
            bridge_token,
        }
    }

    /// The servers Fleet has spawned (and not yet closed).
    pub fn servers(&self) -> Vec<crate::mux::Server> {
        self.prune_dead_local_servers();
        match self.servers.lock() {
            Ok(servers) => servers.clone(),
            Err(e) => {
                tracing::warn!(error = %e, "spawned server list lock poisoned");
                Vec::new()
            }
        }
    }

    pub fn rename(&self, id: &str, label: &str) -> bool {
        let mut renamed = false;
        if let Ok(mut servers) = self.servers.lock() {
            renamed = rename_server_label(&mut servers, id, label);
        }
        if renamed {
            tracing::info!(%id, %label, "spawned server label renamed");
        }
        renamed
    }

    /// Spawn a new server. Routes to the container path when `FLEET_SPAWN_MODE=container`,
    /// else the default local-process path. Both return a [`Server`] that Fleet adds
    /// to the rail immediately; the spawned env phones home to the bridge on its own.
    pub fn spawn(&self) -> std::io::Result<crate::mux::Server> {
        self.spawn_with(SpawnRequest::default())
    }

    pub fn spawn_with(&self, request: SpawnRequest) -> std::io::Result<crate::mux::Server> {
        let mode = request
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| !mode.is_empty());
        match spawn_mode() {
            _ if mode == Some("container") => self.spawn_container(),
            _ if mode == Some("local") => self.spawn_local(Some(
                request
                    .folder
                    .as_deref()
                    .map(expand_user_path)
                    .unwrap_or_else(default_spawn_folder),
            )),
            SpawnMode::Container => self.spawn_container(),
            SpawnMode::Ssh => self.spawn_ssh(),
            SpawnMode::Local => self.spawn_local(request.folder.as_deref().map(expand_user_path)),
        }
    }

    /// Spawn a new VS Code web server (+ its reporter + claude shim) and record it.
    /// Returns its [`Server`]; Fleet adds it to the rail immediately.
    fn spawn_local(&self, folder_override: Option<PathBuf>) -> std::io::Result<crate::mux::Server> {
        let base = fleet_mux_base();
        let (n, id) = self.allocate_server_id();
        let label = server_label(n);
        let port = free_port()?;

        let _ = std::fs::create_dir_all(&base);
        let process_cwd = fleet_spawn_cwd(&base);
        std::fs::create_dir_all(&process_cwd)?;
        let tool_path = fleet_tool_path();
        let folder = match folder_override {
            Some(folder) => folder,
            None => match std::env::var("FLEET_SPAWN_REPO") {
                Ok(spec) if !spec.trim().is_empty() => {
                    let folder = base.join(format!("ws-{id}"));
                    let url = resolve_repo(&spec);
                    let ok = Command::new("git")
                        .args(["clone", "--depth", "1", &url, &folder.to_string_lossy()])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if ok {
                        tracing::info!(%id, %url, "cloned repo into workspace");
                        folder
                    } else {
                        tracing::warn!(%id, %url, "git clone failed; using home folder");
                        default_spawn_folder()
                    }
                }
                _ => default_spawn_folder(),
            },
        };
        if !folder.exists() {
            std::fs::create_dir_all(&folder)?;
        }

        // --- agent-state pipeline: a per-server reporter + a claude shim ---------
        let reporter_socket = base.join(format!("reporter-{id}.sock"));
        let _ = std::fs::remove_file(&reporter_socket);
        let mut children: Vec<Child> = Vec::new();
        match self.spawn_reporter(&id, &reporter_socket, &process_cwd, &tool_path) {
            Ok(child) => children.push(child),
            Err(e) => {
                tracing::warn!(%id, error = %e, "reporter not started (no agent state)")
            }
        }
        let shim_dir = base.join(format!("shim-{id}"));
        let shim_path = install_claude_shim(&shim_dir, &reporter_socket, &tool_path)
            .map_err(|e| tracing::warn!(%id, error = %e, "claude shim not installed"))
            .ok();

        // --- the editor: VS Code serve-web --------------------------------------
        let user_data = base.join(format!("cs-userdata-{id}"));
        let tmp_dir = base.join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let editor = editor_bin(&tool_path);
        write_spawned_server_settings(&user_data)?;
        install_fleet_bridge(&editor, &user_data)?;
        let server_bin = local_code_server_bin(&editor).unwrap_or_else(|| editor.clone());
        let url = format!(
            "http://127.0.0.1:{port}/?folder={}",
            query_escape(&folder.to_string_lossy())
        );
        let (cs_out, cs_err) = log_files(&format!("cs-{id}"));

        // Prepend the shim dir so the terminal's `claude` is Fleet-wrapped.
        let path = match &shim_path {
            Some(_) => prepend_path(&shim_dir, &tool_path),
            None => tool_path.clone(),
        };

        let args = local_server_args(&server_bin, "127.0.0.1", port, &user_data, &folder);
        let env = fleet_env(
            &reporter_socket.to_string_lossy(),
            &id,
            &label,
            &format!("ws://127.0.0.1:{}", self.bridge_port),
            &url,
            &self.bridge_token,
            &base.to_string_lossy(),
        );
        let (spawn_bin, spawn_prefix_args) = server_spawn_target(&server_bin);
        let mut cmd = fleet_command(&spawn_bin);
        cmd.args(&spawn_prefix_args)
            .args(&args)
            .env("PATH", path)
            .env("TMPDIR", &tmp_dir)
            .current_dir(&process_cwd);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let child = spawn_fleet_child(cmd.stdout(cs_out).stderr(cs_err))?;
        children.push(child);

        tracing::info!(%id, port, server_bin = %server_bin.display(), "spawned VS Code web server + reporter");
        let server = crate::mux::Server {
            id: id.clone(),
            label,
            url,
            owned: true,
        };
        self.children.lock().unwrap().insert(id.clone(), children);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    /// Launch this server's `fleet-reporter --serve` (session id = server id).
    fn spawn_reporter(
        &self,
        id: &str,
        socket: &Path,
        cwd: &Path,
        path: &OsStr,
    ) -> std::io::Result<Child> {
        let bin = reporter_bin();
        let (out, err) = log_files(&format!("reporter-{id}"));
        let mut cmd = fleet_command(bin);
        cmd.args([
            "--serve",
            "--ws",
            &self.hub_url,
            "--socket",
            &socket.to_string_lossy(),
            "--session-id",
            id,
        ])
        .env("PATH", path)
        .current_dir(cwd)
        .stdout(out)
        .stderr(err);
        spawn_fleet_child(&mut cmd)
    }

    /// Spawn a new server as a **container** (`docker run` the `fleet-env` image) and
    /// record it. The image bakes in code-server + reporter + bridge; it dials back to
    /// Fleet's bridge using `FLEET_HOST_ADDR` + `FLEET_BRIDGE_PORT`, so it appears in
    /// the rail on its own. We publish a free host port to the container's `:8080` and
    /// inspect the container for the reachable URL.
    ///
    /// NOTE: for the container to reach Fleet's bridge, Fleet must bind it on all
    /// interfaces — launch with `FLEET_BRIDGE_ADDR=0.0.0.0` (see `bridge.rs`).
    fn spawn_container(&self) -> std::io::Result<crate::mux::Server> {
        let (n, id) = self.allocate_server_id();
        let label = server_label(n);
        let name = format!("fleet-{id}");
        let port = free_port()?;

        let docker = docker_bin();
        let image =
            std::env::var("FLEET_SPAWN_IMAGE").unwrap_or_else(|_| "fleet-env:latest".into());
        // How the container dials back to Fleet (host gateway). `host.docker.internal`
        // resolves to the host on Docker Desktop / colima.
        let host_addr =
            std::env::var("FLEET_HOST_ADDR").unwrap_or_else(|_| "host.docker.internal".into());

        // Best-effort: remove any stale container of the same name first.
        let _ = Command::new(&docker)
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        // `docker run -d` the image. Mirrors the eval harness contract: the env phones
        // home (bridge dials `ws://<host_addr>:<bridge_port>`) and registers itself.
        let status = Command::new(&docker)
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "-e",
                &format!("FLEET_SERVER_ID={id}"),
                "-e",
                &format!("FLEET_SERVER_LABEL={label}"),
                "-e",
                &format!("FLEET_HOST_ADDR={host_addr}"),
                "-e",
                &format!("FLEET_BRIDGE_PORT={}", self.bridge_port),
                "-e",
                &format!("FLEET_BRIDGE_TOKEN={}", self.bridge_token),
                "-e",
                &format!("FLEET_HUB_URL={}", self.hub_url),
                "-p",
                &format!("{port}:8080"),
                &image,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "`docker run` failed for {name} (image {image})"
            )));
        }

        // Inspect the container for the host-reachable URL (its published :8080 port).
        // Prefer what docker reports; fall back to the port we asked for.
        let url =
            inspect_url(&docker, &name).unwrap_or_else(|| format!("http://127.0.0.1:{port}/"));

        tracing::info!(%id, %name, port, url, "spawned fleet-env container");
        let server = crate::mux::Server {
            id: id.clone(),
            label,
            url,
            owned: true,
        };
        self.containers.lock().unwrap().insert(id.clone(), name);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    /// Deploy a server to a **remote SSH host** (`FLEET_SSH_TARGET`, e.g. `user@host`)
    /// by running the EXACT SAME code-server + reporter invocation as `spawn_local`,
    /// but over `ssh` with tunnels so the deployed server can call home:
    ///   - `-L <local>:127.0.0.1:<remote cs>`     the editor surface, reachable locally;
    ///   - `-R <remote hub>:127.0.0.1:<Hub>`       the reporter dials home → the Hub;
    ///   - `-R <remote bridge>:127.0.0.1:<bridge>` the bridge dials home → Fleet's rail.
    ///
    /// The remote must already have `code-server` (with the fleet-bridge extension in
    /// `FLEET_REMOTE_EXTENSIONS_DIR`) and `fleet-reporter` available — knobs:
    /// `FLEET_REMOTE_EDITOR_BIN`, `FLEET_REMOTE_REPORTER_BIN`, `FLEET_REMOTE_EXTENSIONS_DIR`.
    /// Closing the server kills the ssh process, tearing down the tunnels and (via
    /// SIGHUP + an EXIT trap) the remote code-server + reporter.
    fn spawn_ssh(&self) -> std::io::Result<crate::mux::Server> {
        let target = std::env::var("FLEET_SSH_TARGET")
            .map_err(|_| std::io::Error::other("FLEET_SSH_TARGET not set (e.g. user@host)"))?;
        let (n, id) = self.allocate_server_id();
        let label = format!("{} @ {target}", server_label(n));

        // local_editor is free on THIS host; the remote ports are bound on the remote
        // by ssh's tunnels (ExitOnForwardFailure makes a collision fail loudly).
        let local_editor = free_port()?;
        let r_cs = 18000 + (n as u16 % 1000);
        let r_hub = 19000 + (n as u16 % 1000);
        let r_bridge = 20000 + (n as u16 % 1000);
        let local_hub = ws_port(&self.hub_url).unwrap_or(51777);
        let local_bridge = self.bridge_port;

        // Remote paths, relative to the ssh login home.
        let remote_ws = format!(".fleet/ws-{id}");
        let remote_sock = format!("/tmp/fleet-reporter-{id}.sock");
        // (serve-web has no --extensions-dir; the remote fleet-bridge install is the same
        // open item as local — see spawn_local.)
        let remote_userdata = format!(".fleet/cs-userdata-{id}");
        let remote_editor =
            std::env::var("FLEET_REMOTE_EDITOR_BIN").unwrap_or_else(|_| "code".into());
        let remote_reporter =
            std::env::var("FLEET_REMOTE_REPORTER_BIN").unwrap_or_else(|_| "fleet-reporter".into());

        // The editor surface is reached locally through the -L tunnel.
        let url = format!("http://127.0.0.1:{local_editor}/");
        // The SAME invocation as local — bound to the remote loopback; the bridge dials
        // the -R bridge tunnel back to Fleet.
        let args = serve_web_args("127.0.0.1", r_cs, &remote_userdata, &remote_ws);
        let env = fleet_env(
            &remote_sock,
            &id,
            &label,
            &format!("ws://127.0.0.1:{r_bridge}"),
            &url,
            &self.bridge_token,
            ".fleet",
        );

        let env_str = env
            .iter()
            .map(|(k, v)| format!("{k}={}", shq(v)))
            .collect::<Vec<_>>()
            .join(" ");
        let args_str = args.iter().map(|a| shq(a)).collect::<Vec<_>>().join(" ");
        // Workspace: clone FLEET_SPAWN_REPO into the remote workspace (repo-as-workspace),
        // else just create it. Then reporter (background, dials the -R hub tunnel) +
        // code-server (foreground); an EXIT trap reaps the reporter when ssh drops.
        let ws_prep = match std::env::var("FLEET_SPAWN_REPO") {
            Ok(spec) if !spec.trim().is_empty() => format!(
                "git clone --depth 1 {} {} 2>/dev/null || mkdir -p {};",
                shq(&resolve_repo(&spec)),
                shq(&remote_ws),
                shq(&remote_ws)
            ),
            _ => format!("mkdir -p {};", shq(&remote_ws)),
        };
        let remote_cmd = format!(
            "{ws_prep} mkdir -p {ud} 2>/dev/null; rm -f {sock}; \
             {rep} --serve --ws ws://127.0.0.1:{rhub} --socket {sock} --session-id {id} >/tmp/fleet-rep-{id}.log 2>&1 & \
             RPID=$!; trap 'kill $RPID 2>/dev/null' EXIT INT TERM; \
             env {env} {ed} {args}; kill $RPID 2>/dev/null",
            ws_prep = ws_prep,
            ud = shq(&remote_userdata),
            sock = shq(&remote_sock),
            rep = shq(&remote_reporter),
            rhub = r_hub,
            id = id,
            env = env_str,
            ed = shq(&remote_editor),
            args = args_str,
        );

        let (out, err) = log_files(&format!("ssh-{id}"));
        let mut cmd = fleet_command("ssh");
        cmd.args([
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-L",
            &format!("{local_editor}:127.0.0.1:{r_cs}"),
            "-R",
            &format!("{r_hub}:127.0.0.1:{local_hub}"),
            "-R",
            &format!("{r_bridge}:127.0.0.1:{local_bridge}"),
            &target,
            &remote_cmd,
        ])
        .stdout(out)
        .stderr(err);
        let child = spawn_fleet_child(&mut cmd)?;

        tracing::info!(%id, %target, local_editor, r_cs, "deployed code-server over ssh");
        let server = crate::mux::Server {
            id: id.clone(),
            label,
            url,
            owned: true,
        };
        self.children
            .lock()
            .unwrap()
            .insert(id.clone(), vec![child]);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    fn allocate_server_id(&self) -> (u64, String) {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        (n, format!("server-{}-{n}", self.launch_id))
    }

    /// Close (kill) a Fleet-spawned server — its code-server AND reporter (local
    /// mode), or its container (`docker rm -f`, container mode) — and drop it from
    /// the rail.
    pub fn close(&self, id: &str) -> bool {
        self.servers.lock().unwrap().retain(|s| s.id != id);

        // Container-mode server: remove the container.
        if let Some(name) = self.containers.lock().unwrap().remove(id) {
            let docker = docker_bin();
            let _ = Command::new(&docker)
                .args(["rm", "-f", &name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            tracing::info!(%id, %name, "removed spawned container");
            return true;
        }

        // Local-mode server: kill its child processes.
        if let Some(children) = self.children.lock().unwrap().remove(id) {
            for child in children {
                terminate_child_tree(child);
            }
            tracing::info!(%id, "closed spawned server");
            true
        } else {
            false
        }
    }

    fn prune_dead_local_servers(&self) {
        let mut removed = Vec::new();

        if let Ok(mut children) = self.children.lock() {
            let dead_ids = children
                .iter_mut()
                .filter_map(|(id, group)| match primary_child_exited(group) {
                    ChildHealth::Alive => None,
                    ChildHealth::Exited { pid, status } => {
                        tracing::warn!(%id, pid, %status, "local server child exited; pruning session");
                        Some(id.clone())
                    }
                    ChildHealth::Unavailable { reason } => {
                        tracing::warn!(%id, %reason, "local server child unavailable; pruning session");
                        Some(id.clone())
                    }
                })
                .collect::<Vec<_>>();
            for id in dead_ids {
                if let Some(group) = children.remove(&id) {
                    for child in group {
                        terminate_child_tree(child);
                    }
                    removed.push(id);
                }
            }
        } else {
            tracing::warn!("local server child lock poisoned; skipping dead-session prune");
        }

        if removed.is_empty() {
            return;
        }

        removed.sort();
        removed.dedup();
        if let Ok(mut servers) = self.servers.lock() {
            servers.retain(|server| !removed.iter().any(|id| id == &server.id));
        } else {
            tracing::warn!("spawned server list lock poisoned while pruning dead sessions");
        }
        tracing::info!(servers = ?removed, "pruned dead local sessions");
    }
}

fn rename_server_label(servers: &mut [crate::mux::Server], id: &str, label: &str) -> bool {
    if let Some(server) = servers.iter_mut().find(|server| server.id == id) {
        server.label = label.to_string();
        true
    } else {
        false
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ChildHealth {
    Alive,
    Exited { pid: u32, status: String },
    Unavailable { reason: String },
}

fn primary_child_exited(children: &mut [Child]) -> ChildHealth {
    let Some(child) = children.last_mut() else {
        return ChildHealth::Unavailable {
            reason: "no tracked child process".into(),
        };
    };
    let pid = child.id();
    match child.try_wait() {
        Ok(None) => ChildHealth::Alive,
        Ok(Some(status)) => ChildHealth::Exited {
            pid,
            status: status.to_string(),
        },
        Err(e) => ChildHealth::Unavailable {
            reason: e.to_string(),
        },
    }
}

fn spawn_manifest_path(base: &Path) -> PathBuf {
    base.join("servers.json")
}

pub fn clear_legacy_spawn_state() {
    let base = fleet_mux_base();
    clear_legacy_spawn_state_in(&base);
}

fn clear_legacy_spawn_state_in(base: &Path) {
    for path in [spawn_manifest_path(base), next_server_counter_path(base)] {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "removed legacy Fleet client state"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "legacy Fleet client state could not be removed")
            }
        }
    }
}

fn launch_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:x}-{:x}", std::process::id(), nanos)
}

fn server_label(n: u64) -> String {
    format!("server-{n}")
}

fn spawn_fleet_child(cmd: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// Trampoline flag: `fleet-host --fleet-disclaim-exec <prog> [args…]` replaces
/// the process with `<prog>`, disclaiming TCC responsibility (macOS only).
#[cfg(target_os = "macos")]
pub const DISCLAIM_EXEC_ARG: &str = "--fleet-disclaim-exec";

/// Command for a long-lived Fleet-spawned process (code-server tree, reporter,
/// ssh deploy). On macOS the spawn routes through Fleet's own binary with
/// [`DISCLAIM_EXEC_ARG`], which immediately replaces itself with the target via
/// `posix_spawn(POSIX_SPAWN_SETEXEC)` after disclaiming TCC responsibility — so
/// privacy prompts for anything the server tree touches (Documents, Desktop, …)
/// attribute to the server binary, not to Fleet. Fleet stays a passthrough.
/// The exec keeps pid, process group, stdio, cwd, and env, so `Child` handles
/// and group termination behave exactly as a direct spawn.
fn fleet_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(target_os = "macos")]
    // Under `cargo test` the current executable is the libtest harness, which
    // has no trampoline branch — spawn directly there.
    if !cfg!(test) {
        if let Ok(exe) = std::env::current_exe() {
            return disclaim_command(exe.as_os_str(), program.as_ref());
        }
    }
    Command::new(program)
}

#[cfg(target_os = "macos")]
fn disclaim_command(trampoline: &OsStr, program: &OsStr) -> Command {
    let mut cmd = Command::new(trampoline);
    cmd.arg(DISCLAIM_EXEC_ARG).arg(program);
    cmd
}

/// What to actually spawn for a local server. serve-web's `bin/code-server` is
/// a bash wrapper that keeps running as the tree's parent, so on macOS the TCC
/// disclaim (see [`fleet_command`]) would attribute every privacy prompt to
/// "bash" — generic, and a grant to bash is a grant to every shell script. When
/// the standard serve-web layout is present (`<root>/node` +
/// `<root>/out/server-main.js` next to `<root>/bin/code-server`), spawn the
/// node binary directly so prompts name the editor process. Falls back to the
/// wrapper whenever the layout doesn't match.
fn server_spawn_target(server_bin: &Path) -> (PathBuf, Vec<OsString>) {
    #[cfg(target_os = "macos")]
    if let Some((node, server_main)) = direct_server_invocation(server_bin) {
        return (node, vec![server_main.into_os_string()]);
    }
    (server_bin.to_path_buf(), Vec::new())
}

#[cfg(target_os = "macos")]
fn direct_server_invocation(server_bin: &Path) -> Option<(PathBuf, PathBuf)> {
    let root = server_bin.parent()?.parent()?;
    let node = root.join("node");
    let server_main = root.join("out").join("server-main.js");
    (node.is_file() && server_main.is_file()).then_some((node, server_main))
}

/// TCC responsibility disclaim (macOS). By default macOS attributes a child
/// tree's protected-folder access to the app that spawned it, so every prompt
/// says "Fleet would like to access…". `responsibility_spawnattrs_setdisclaim`
/// (private but ABI-stable since 10.14; shipped by Chromium, WezTerm, Emacs)
/// makes the spawned image its own responsible process instead.
#[cfg(target_os = "macos")]
mod disclaim {
    use std::ffi::{c_char, c_int, c_short, c_void, CString, OsString};
    use std::os::unix::ffi::OsStrExt;

    /// `typedef void *posix_spawnattr_t;` — spawn.h
    type PosixSpawnattrT = *mut c_void;
    /// sys/spawn.h — `posix_spawn` acts like exec, replacing this process.
    const POSIX_SPAWN_SETEXEC: c_short = 0x0040;

    extern "C" {
        fn posix_spawnattr_init(attr: *mut PosixSpawnattrT) -> c_int;
        fn posix_spawnattr_setflags(attr: *mut PosixSpawnattrT, flags: c_short) -> c_int;
        fn responsibility_spawnattrs_setdisclaim(
            attr: *mut PosixSpawnattrT,
            disclaim: c_int,
        ) -> c_int;
        fn posix_spawnp(
            pid: *mut c_int,
            file: *const c_char,
            file_actions: *const c_void,
            attrp: *const PosixSpawnattrT,
            argv: *const *const c_char,
            envp: *const *const c_char,
        ) -> c_int;
        static environ: *const *const c_char;
    }

    fn disclaiming_spawnattr() -> Result<PosixSpawnattrT, std::io::Error> {
        let mut attr: PosixSpawnattrT = std::ptr::null_mut();
        let rc = unsafe { posix_spawnattr_init(&mut attr) };
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc));
        }
        let rc = unsafe { responsibility_spawnattrs_setdisclaim(&mut attr, 1) };
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc));
        }
        Ok(attr)
    }

    /// Replace this process with `argv`, disclaimed. Returns only on failure.
    pub fn exec_disclaimed(argv: &[OsString]) -> std::io::Error {
        let cstrings: Vec<CString> = match argv
            .iter()
            .map(|arg| CString::new(arg.as_bytes()))
            .collect()
        {
            Ok(args) => args,
            Err(e) => return std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
        };
        let Some(program) = cstrings.first() else {
            return std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "disclaim-exec: no program given",
            );
        };
        let mut argv_ptrs: Vec<*const c_char> = cstrings.iter().map(|arg| arg.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let mut attr = match disclaiming_spawnattr() {
            Ok(attr) => attr,
            Err(e) => return e,
        };
        let rc = unsafe { posix_spawnattr_setflags(&mut attr, POSIX_SPAWN_SETEXEC) };
        if rc != 0 {
            return std::io::Error::from_raw_os_error(rc);
        }
        let mut pid: c_int = 0;
        let rc = unsafe {
            posix_spawnp(
                &mut pid,
                program.as_ptr(),
                std::ptr::null(),
                &attr,
                argv_ptrs.as_ptr(),
                environ,
            )
        };
        // SETEXEC means success never returns; reaching here is an error.
        std::io::Error::from_raw_os_error(rc)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn disclaim_spawnattr_is_supported() {
            // Validates the private symbol resolves and the kernel-facing attr
            // call succeeds on this macOS — the trampoline depends on both.
            super::disclaiming_spawnattr().expect("disclaim spawnattr");
        }
    }
}

#[cfg(target_os = "macos")]
pub use disclaim::exec_disclaimed;

fn terminate_child_tree(mut child: Child) {
    #[cfg(unix)]
    {
        terminate_unix_child_tree(&mut child);
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
fn terminate_unix_child_tree(child: &mut Child) {
    if let Some(group_pid) = process_group_signal_pid(child.id()) {
        let _ = signal_process_group(group_pid, SIGTERM);
        let _ = wait_child_for(child, std::time::Duration::from_millis(300));
        let _ = signal_process_group(group_pid, SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn wait_child_for(child: &mut Child, timeout: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

#[cfg(unix)]
fn process_group_signal_pid(child_pid: u32) -> Option<i32> {
    let pid = i32::try_from(child_pid).ok()?;
    Some(-pid)
}

#[cfg(unix)]
fn signal_process_group(group_pid: i32, signal: i32) -> std::io::Result<()> {
    let rc = unsafe { libc_kill(group_pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// Which launch path `spawn()` takes. `FLEET_SPAWN_MODE=container` opts into Docker,
/// `=ssh` deploys to the remote in `FLEET_SSH_TARGET`; anything else (incl. unset)
/// keeps the default local-process path.
enum SpawnMode {
    Local,
    Container,
    Ssh,
}

fn spawn_mode() -> SpawnMode {
    match std::env::var("FLEET_SPAWN_MODE").as_deref() {
        Ok("container") => SpawnMode::Container,
        Ok("ssh") => SpawnMode::Ssh,
        _ => SpawnMode::Local,
    }
}

/// The `docker` CLI to drive (`FLEET_DOCKER_BIN`, default `docker`).
fn docker_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("FLEET_DOCKER_BIN") {
        if !bin.trim().is_empty() {
            return PathBuf::from(bin);
        }
    }
    find_on_path("docker", &fleet_tool_path()).unwrap_or_else(|| PathBuf::from("docker"))
}

fn editor_bin(path: &OsStr) -> PathBuf {
    if let Ok(bin) = std::env::var("FLEET_EDITOR_BIN") {
        if !bin.trim().is_empty() {
            return PathBuf::from(bin);
        }
    }
    find_on_path("code", path)
        .or_else(|| {
            let mut candidates: Vec<PathBuf> = [
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
                "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code",
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect();
            // Default Windows install locations (per-user, then machine-wide).
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                candidates.extend(executable_candidates(
                    &PathBuf::from(local)
                        .join("Programs")
                        .join("Microsoft VS Code")
                        .join("bin"),
                    "code",
                ));
            }
            if let Some(pf) = std::env::var_os("ProgramFiles") {
                candidates.extend(executable_candidates(
                    &PathBuf::from(pf).join("Microsoft VS Code").join("bin"),
                    "code",
                ));
            }
            candidates.into_iter().find(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("code"))
}

fn local_code_server_bin(editor: &Path) -> Option<PathBuf> {
    if is_code_server_bin(editor) {
        return Some(editor.to_path_buf());
    }
    if let Ok(bin) = std::env::var("FLEET_CODE_SERVER_BIN") {
        if !bin.trim().is_empty() {
            return Some(PathBuf::from(bin));
        }
    }
    let from_home = code_server_bin_from_home(home_dir());
    if from_home.is_some() {
        return from_home;
    }

    // Best effort for first run: ask the official `code serve-web` wrapper to
    // materialize its downloaded server bundle, then look again.
    let _ = Command::new(editor)
        .args(["serve-web", "--help"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    code_server_bin_from_home(home_dir())
}

fn is_code_server_bin(path: &Path) -> bool {
    // file_stem so the Windows `code-server.cmd` / `code-server.exe` forms match.
    path.file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "code-server")
}

fn code_server_bin_from_home(home: Option<PathBuf>) -> Option<PathBuf> {
    let home = home?;
    let mut candidates = Vec::new();
    for root in [
        home.join(".vscode").join("cli").join("serve-web"),
        home.join(".vscode-insiders").join("cli").join("serve-web"),
    ] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let bin_dir = entry.path().join("bin");
            let Some(bin) = executable_candidates(&bin_dir, "code-server")
                .into_iter()
                .find(|candidate| candidate.is_file())
            else {
                continue;
            };
            let modified = bin
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or_default();
            candidates.push((modified, bin));
        }
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    candidates.pop().map(|(_, bin)| bin)
}

fn reporter_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("FLEET_REPORTER_BIN") {
        if !bin.trim().is_empty() {
            return PathBuf::from(bin);
        }
    }
    bundled_bin("fleet-reporter")
        .or_else(|| find_on_path("fleet-reporter", &fleet_tool_path()))
        .unwrap_or_else(|| PathBuf::from("fleet-reporter"))
}

fn bundled_bin(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    bundle_search_dirs(exe_dir)
        .into_iter()
        .flat_map(|dir| executable_candidates(&dir, name))
        .find(|path| path.is_file())
}

/// Where a Tauri bundle can put files relative to the running binary:
/// next to it (Windows install root, macOS Contents/MacOS sidecars),
/// `../Resources` (macOS), or `../lib/<app>` (Linux deb/rpm/AppImage).
fn bundle_search_dirs(exe_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![exe_dir.to_path_buf()];
    if let Some(parent) = exe_dir.parent() {
        dirs.push(parent.join("Resources"));
        for app in ["fleet", "Fleet", "fleet-host"] {
            dirs.push(parent.join("lib").join(app));
        }
    }
    dirs
}

/// PATH used for Fleet-launched tools.
///
/// GUI-launched macOS apps do not inherit an interactive shell's PATH, so build a
/// conservative tool path that still finds common user-installed CLIs.
fn fleet_tool_path() -> OsString {
    fleet_tool_path_from(
        std::env::var_os("PATH"),
        home_dir(),
        std::env::var("USER").ok(),
    )
}

fn fleet_tool_path_from(
    current: Option<OsString>,
    home: Option<PathBuf>,
    user: Option<String>,
) -> OsString {
    let mut dirs = current
        .as_ref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();

    if let Some(home) = home.as_ref() {
        // Home Manager app bins often contain wrapper CLIs such as cmux's `claude`.
        // Prefer them before raw user bins so a GUI-launched Fleet behaves like the
        // user's interactive shell when those wrappers are first there.
        push_home_manager_app_bins(&mut dirs, home);
        push_existing_dir(&mut dirs, home.join(".local/bin"));
        push_existing_dir(&mut dirs, home.join("bin"));
        push_existing_dir(&mut dirs, home.join(".cargo/bin"));
    }

    if let Some(user) = user.filter(|s| !s.is_empty()) {
        push_existing_dir(
            &mut dirs,
            PathBuf::from(format!("/etc/profiles/per-user/{user}/bin")),
        );
    }

    for dir in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
        "/run/current-system/sw/bin",
        "/nix/var/nix/profiles/default/bin",
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin",
        "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin",
    ] {
        push_existing_dir(&mut dirs, PathBuf::from(dir));
    }

    std::env::join_paths(dirs).unwrap_or_else(|_| current.unwrap_or_default())
}

fn push_home_manager_app_bins(dirs: &mut Vec<PathBuf>, home: &Path) {
    let app_dir = home.join("Applications").join("Home Manager Apps");
    let Ok(entries) = std::fs::read_dir(app_dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        {
            push_existing_dir(dirs, path.join("Contents").join("Resources").join("bin"));
        }
    }
}

fn push_existing_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_dir() && !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn prepend_path(dir: &Path, path: &OsStr) -> OsString {
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(std::env::split_paths(path));
    std::env::join_paths(dirs).unwrap_or_else(|_| path.to_os_string())
}

fn find_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .flat_map(|dir| executable_candidates(&dir, name))
        .find(|path| path.is_file())
}

/// Candidate file names for executable `name` in `dir`. On Windows the VS Code
/// CLI (and most CLI launchers) ship as `name.cmd` / `name.exe`; the bare `name`
/// next to them is a POSIX shell script, so the Windows-native forms come first.
fn executable_candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        for ext in ["exe", "cmd", "bat"] {
            candidates.push(dir.join(format!("{name}.{ext}")));
        }
    }
    candidates.push(dir.join(name));
    candidates
}

/// The user's home directory: `HOME` (unix, and respected if set on Windows),
/// else `USERPROFILE` (Windows).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Inspect a running container for the host-reachable editor URL: the host port that
/// docker bound to the container's `8080/tcp`. Returns `None` if inspection fails.
fn inspect_url(docker: &Path, name: &str) -> Option<String> {
    let out = Command::new(docker)
        .args([
            "inspect",
            "-f",
            // HostPort of the first binding published for the container's :8080.
            "{{(index (index .NetworkSettings.Ports \"8080/tcp\") 0).HostPort}}",
            name,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let host_port = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if host_port.is_empty() {
        return None;
    }
    Some(format!("http://127.0.0.1:{host_port}/"))
}

/// Install the Fleet bridge VSIX into the extension store that `code serve-web`
/// actually reads: `<server-data-dir>/extensions`. This is intentionally tied to
/// the per-server data dir, because `serve-web` ignores the desktop/user extension
/// store when `--server-data-dir` is set.
fn install_fleet_bridge(editor: &Path, server_data: &Path) -> std::io::Result<()> {
    let extensions_dir = fleet_bridge_extensions_dir(server_data);
    let already_installed = fleet_bridge_installed(&extensions_dir);

    std::fs::create_dir_all(&extensions_dir)?;
    let vsix = find_fleet_bridge_vsix().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "fleet-bridge VSIX not found (set FLEET_BRIDGE_VSIX or bundle fleet-bridge.vsix)",
        )
    })?;

    // Always refresh with --force. Fleet reuses per-server data dirs across host
    // restarts, and the local bridge VSIX can change during development without
    // a version bump.
    let out = Command::new(editor)
        .arg("--install-extension")
        .arg(&vsix)
        .arg("--extensions-dir")
        .arg(&extensions_dir)
        .arg("--force")
        .output()?;
    if out.status.success() {
        tracing::info!(
            extensions_dir = %extensions_dir.display(),
            refreshed = already_installed,
            "installed fleet-bridge extension"
        );
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(std::io::Error::other(format!(
        "failed to install fleet-bridge extension with `{}`: {detail}",
        editor.display()
    )))
}

fn fleet_bridge_extensions_dir(server_data: &Path) -> PathBuf {
    server_data.join("extensions")
}

fn fleet_bridge_installed(extensions_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(extensions_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("fleet-team.fleet-bridge-"))
    })
}

/// Write Fleet-owned VS Code user settings into the per-server data dir before
/// `serve-web` starts. This is intentionally scoped to Fleet's isolated
/// `--server-data-dir`, not the user's desktop VS Code settings.
fn write_spawned_server_settings(server_data: &Path) -> std::io::Result<()> {
    let settings_path = spawned_server_settings_path(server_data);
    let mut settings = read_json_object_or_empty(&settings_path);
    let obj = settings.as_object_mut().expect("settings object");
    obj.insert(
        "terminal.integrated.gpuAcceleration".into(),
        serde_json::Value::String("off".into()),
    );
    obj.insert(
        "window.commandCenter".into(),
        serde_json::Value::Bool(false),
    );
    obj.insert(
        "workbench.layoutControl.enabled".into(),
        serde_json::Value::Bool(false),
    );
    obj.insert(
        "workbench.navigationControl.enabled".into(),
        serde_json::Value::Bool(false),
    );
    obj.insert(
        "security.workspace.trust.enabled".into(),
        serde_json::Value::Bool(false),
    );

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(settings_path, serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn spawned_server_settings_path(server_data: &Path) -> PathBuf {
    server_data.join("data").join("User").join("settings.json")
}

fn read_json_object_or_empty(path: &Path) -> serde_json::Value {
    let Ok(bytes) = std::fs::read(path) else {
        return serde_json::json!({});
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            tracing::warn!(path = %path.display(), "VS Code settings file is not an object; replacing it");
            serde_json::json!({})
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "VS Code settings file is invalid; replacing it");
            serde_json::json!({})
        }
    }
}

fn find_fleet_bridge_vsix() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FLEET_BRIDGE_VSIX") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.extend(bundle_search_dirs(exe_dir));
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/fleet-bridge"));

    for dir in dirs {
        let named = dir.join("fleet-bridge.vsix");
        if named.is_file() {
            return Some(named);
        }
        if let Some(found) = first_vsix_in(&dir) {
            return Some(found);
        }
    }
    None
}

fn first_vsix_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vsix"))
        })
}

/// Install a `claude` shim in `shim_dir` that wraps the real `claude` with a
/// `--settings` file whose hooks relay lifecycle payloads to `reporter_socket`.
/// Returns the shim binary path. Skipped (Err) if the real `claude` isn't found.
#[cfg(unix)]
fn install_claude_shim(
    shim_dir: &Path,
    reporter_socket: &Path,
    path: &OsStr,
) -> std::io::Result<PathBuf> {
    let real = find_real_claude(shim_dir, path).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "real `claude` not on PATH")
    })?;
    std::fs::create_dir_all(shim_dir)?;

    let hooks_file = shim_dir.join("fleet-hooks.json");
    std::fs::write(
        &hooks_file,
        serde_json::to_vec_pretty(&claude_hooks_settings(reporter_socket))?,
    )?;

    // Exec the selected claude (never this shim) with our hooks file. The script
    // removes Fleet's shim dir from PATH first so wrapper tools such as cmux can
    // resolve their own real `claude` without bouncing back into Fleet.
    let shim = shim_dir.join("claude");
    let script = claude_shim_script(&real, &hooks_file, shim_dir);
    std::fs::write(&shim, script)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    Ok(shim)
}

/// The shim is a POSIX shell script relaying through `nc -U` — neither exists on
/// Windows. Degrade gracefully: no shim means `claude` runs unwrapped and the
/// rail tab simply shows no agent state (the spawn itself still works).
#[cfg(not(unix))]
fn install_claude_shim(
    _shim_dir: &Path,
    _reporter_socket: &Path,
    _path: &OsStr,
) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "claude shim requires a POSIX shell and unix sockets; not supported on this platform yet",
    ))
}

#[cfg(unix)]
fn claude_shim_script(real: &Path, hooks_file: &Path, shim_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
# fleet-host-claude-shim
# Fleet adds lifecycle hooks for this server, then execs the selected claude.
# Remove only Fleet's shim dir from PATH first so downstream wrappers that call
# `claude` do not resolve back to this shim and stack --settings recursively.
fleet_shim_dir={shim_dir}
clean_path=""
saved_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  [ "$dir" = "$fleet_shim_dir" ] && continue
  if [ -z "$clean_path" ]; then
    clean_path="$dir"
  else
    clean_path="$clean_path:$dir"
  fi
done
IFS=$saved_ifs
PATH="$clean_path"
export PATH

if [ "${{FLEET_CLAUDE_SHIM_ACTIVE:-}}" = "1" ]; then
  exec {real} "$@"
fi
export FLEET_CLAUDE_SHIM_ACTIVE=1
exec {real} --settings {hooks_file} "$@"
"#,
        real = shq(&real.to_string_lossy()),
        hooks_file = shq(&hooks_file.to_string_lossy()),
        shim_dir = shq(&shim_dir.to_string_lossy()),
    )
}

/// Find the real `claude` binary: `FLEET_CLAUDE_BIN`, else the first `claude` on
/// PATH that isn't our shim dir (so the shim never execs itself). Portable logic,
/// but only the unix shim install calls it (hence the test-or-unix gate).
#[cfg(any(unix, test))]
fn find_real_claude(shim_dir: &Path, path: &OsStr) -> Option<PathBuf> {
    if let Ok(bin) = std::env::var("FLEET_CLAUDE_BIN") {
        let p = PathBuf::from(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() || dir == shim_dir {
            continue;
        }
        let cand = dir.join("claude");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// The `--settings` hooks document: every relayed Claude lifecycle event sends its
/// payload (tagged `claude`, CR/LF stripped) to the reporter socket via `nc -U`.
/// `|| true` keeps Claude's flow alive on any relay error (observer, never denies).
#[cfg(unix)]
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

fn local_server_args(
    server_bin: &Path,
    host: &str,
    port: u16,
    server_data: &Path,
    default_folder: &Path,
) -> Vec<String> {
    if is_code_server_bin(server_bin) {
        code_server_args(
            host,
            port,
            &server_data.to_string_lossy(),
            &default_folder.to_string_lossy(),
        )
    } else {
        serve_web_args(
            host,
            port,
            &server_data.to_string_lossy(),
            &default_folder.to_string_lossy(),
        )
    }
}

/// The `code serve-web` invocation Fleet can launch through the user's Code CLI.
/// It remains the bootstrap/install path for Microsoft's official VS Code Web
/// server bundle, but the wrapper rejects some server-main flags that the
/// downloaded `bin/code-server` entrypoint accepts.
/// `--without-connection-token` mirrors code-server's `--auth none`: Fleet binds loopback
/// (local) or tunnels (ssh), so no token is needed. The workspace folder is opened via the
/// URL `?folder=` rather than a positional arg.
fn serve_web_args(host: &str, port: u16, server_data: &str, default_folder: &str) -> Vec<String> {
    vec![
        "serve-web".into(),
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
        "--without-connection-token".into(),
        "--accept-server-license-terms".into(),
        "--server-data-dir".into(),
        server_data.into(),
        "--default-folder".into(),
        default_folder.into(),
    ]
}

/// Direct invocation of the official VS Code Web server bundle downloaded by
/// `code serve-web`. This accepts the hidden `--disable-workspace-trust` flag
/// that `serve-web`'s public CLI parser rejects, and the web workbench maps it
/// to `enableWorkspaceTrust: false` in its generated configuration.
fn code_server_args(host: &str, port: u16, server_data: &str, default_folder: &str) -> Vec<String> {
    vec![
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
        "--without-connection-token".into(),
        "--accept-server-license-terms".into(),
        "--disable-workspace-trust".into(),
        "--server-data-dir".into(),
        server_data.into(),
        "--default-folder".into(),
        default_folder.into(),
    ]
}

/// The Fleet phone-home env every spawned editor carries: the bridge dials
/// `FLEET_BRIDGE_URL`, the reporter writes `FLEET_REPORTER_SOCKET`, and the rail tab is
/// keyed by `FLEET_SERVER_ID`/`FLEET_SESSION_ID`. Identical keys for local and remote;
/// only the values (paths/ports) differ by location.
fn fleet_env(
    reporter_socket: &str,
    id: &str,
    label: &str,
    bridge_url: &str,
    server_url: &str,
    bridge_token: &str,
    bridge_log_dir: &str,
) -> Vec<(String, String)> {
    vec![
        ("FLEET_REPORTER_SOCKET".into(), reporter_socket.into()),
        ("FLEET_SESSION_ID".into(), id.into()),
        ("FLEET_BRIDGE_URL".into(), bridge_url.into()),
        ("FLEET_BRIDGE_TOKEN".into(), bridge_token.into()),
        ("FLEET_BRIDGE_LOG_DIR".into(), bridge_log_dir.into()),
        ("FLEET_SERVER_ID".into(), id.into()),
        ("FLEET_SERVER_LABEL".into(), label.into()),
        ("FLEET_SERVER_URL".into(), server_url.into()),
    ]
}

/// POSIX single-quote a string for safe interpolation into a remote `ssh` shell command.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The port a `ws://127.0.0.1:PORT` URL points at (to reverse-tunnel the Hub home).
fn ws_port(url: &str) -> Option<u16> {
    url.rsplit(':').next()?.trim_end_matches('/').parse().ok()
}

/// Resolve a repo spec to a clone URL. Full URLs (`https://…`, `git@…`, `ssh://…`) pass
/// through. Shorthands prefer GitHub then GitLab: `owner/repo` / `gh:owner/repo` →
/// GitHub, `gl:owner/repo` → GitLab. (Auth is left to the user's existing git creds —
/// ssh keys / credential helper — exactly like a normal `git clone`.)
fn resolve_repo(spec: &str) -> String {
    let s = spec.trim();
    if s.contains("://") || s.starts_with("git@") {
        return s.to_string();
    }
    if let Some(r) = s.strip_prefix("gl:") {
        return format!("https://gitlab.com/{}.git", r.trim_end_matches(".git"));
    }
    let r = s.strip_prefix("gh:").unwrap_or(s).trim_end_matches(".git");
    format!("https://github.com/{r}.git")
}

/// Base directory for local Fleet-managed VS Code servers.
///
/// Defaulting under HOME avoids macOS temp/TCC surprises from `/var/folders` and
/// gives spawned terminals a normal user-owned root. `FLEET_MUX_DIR` is an escape
/// hatch for tests and custom layouts.
fn fleet_mux_base() -> PathBuf {
    fleet_mux_base_from(
        std::env::var_os("FLEET_MUX_DIR").map(PathBuf::from),
        home_dir(),
    )
}

fn fleet_mux_base_from(override_dir: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = override_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir;
    }
    home.unwrap_or_else(std::env::temp_dir)
        .join(".fleet")
        .join("mux")
}

fn next_server_counter_path(base: &Path) -> PathBuf {
    base.join("server-counter")
}

/// Working directory for local child processes Fleet spawns.
///
/// Tauri apps launched by macOS can inherit a temp cwd under `/private/var/folders`.
/// Keep the process root boring and user-owned while `--default-folder` still opens
/// the per-server workspace under `~/.fleet/mux/ws-*`.
fn fleet_spawn_cwd(base: &Path) -> PathBuf {
    fleet_spawn_cwd_from(
        std::env::var_os("FLEET_SPAWN_CWD").map(PathBuf::from),
        home_dir(),
        base,
    )
}

fn fleet_spawn_cwd_from(
    override_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    base: &Path,
) -> PathBuf {
    if let Some(dir) = override_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir;
    }
    home.filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| base.to_path_buf())
}

fn default_spawn_folder() -> PathBuf {
    home_dir()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(fleet_mux_base)
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return default_spawn_folder();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return default_spawn_folder().join(rest);
    }
    PathBuf::from(value)
}

fn query_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `(stdout, stderr)` redirected to `<fleet-mux>/<name>.log` so spawned
/// processes are debuggable (falls back to null if the file can't be created).
fn log_files(name: &str) -> (Stdio, Stdio) {
    let dir = fleet_mux_base();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.log"));
    match std::fs::File::create(&path).and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
        Err(_) => (Stdio::null(), Stdio::null()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn disclaim_command_routes_program_through_trampoline() {
        let cmd = disclaim_command(OsStr::new("/Applications/Fleet"), OsStr::new("/bin/echo"));
        assert_eq!(cmd.get_program(), "/Applications/Fleet");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(args, [DISCLAIM_EXEC_ARG, "/bin/echo"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn server_spawn_target_bypasses_serve_web_bash_wrapper() {
        let root = temp_test_dir("fleet-direct-node-spawn");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("bin/code-server"), "#!/bin/bash\n").unwrap();
        std::fs::write(root.join("node"), "").unwrap();
        std::fs::write(root.join("out/server-main.js"), "").unwrap();

        let (bin, prefix) = server_spawn_target(&root.join("bin/code-server"));
        assert_eq!(bin, root.join("node"));
        assert_eq!(
            prefix,
            vec![root.join("out/server-main.js").into_os_string()]
        );

        // Unknown layout (no sibling node) falls back to the wrapper itself.
        std::fs::remove_file(root.join("node")).unwrap();
        let wrapper = root.join("bin/code-server");
        let (bin, prefix) = server_spawn_target(&wrapper);
        assert_eq!(bin, wrapper);
        assert!(prefix.is_empty());
    }

    #[test]
    fn fleet_command_under_test_spawns_directly() {
        // The libtest harness has no trampoline branch, so in-tree tests must
        // get a plain spawn (the trampoline path is exercised by the smoke run).
        let cmd = fleet_command("/bin/echo");
        assert_eq!(cmd.get_program(), "/bin/echo");
        assert_eq!(cmd.get_args().count(), 0);
    }

    #[test]
    fn resolve_repo_shorthands_prefer_github_then_gitlab() {
        assert_eq!(
            resolve_repo("owner/repo"),
            "https://github.com/owner/repo.git"
        );
        assert_eq!(
            resolve_repo("gh:owner/repo"),
            "https://github.com/owner/repo.git"
        );
        assert_eq!(
            resolve_repo("gl:owner/repo"),
            "https://gitlab.com/owner/repo.git"
        );
        assert_eq!(
            resolve_repo("owner/repo.git"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn resolve_repo_full_urls_pass_through() {
        assert_eq!(
            resolve_repo("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            resolve_repo("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        assert_eq!(resolve_repo("ssh://git@host/o/r"), "ssh://git@host/o/r");
    }

    #[test]
    fn shq_single_quotes_and_escapes() {
        assert_eq!(shq("simple"), "'simple'");
        assert_eq!(shq("a b"), "'a b'");
        assert_eq!(shq("it's"), "'it'\\''s'");
    }

    #[test]
    fn ws_port_parses_with_or_without_trailing_slash() {
        assert_eq!(ws_port("ws://127.0.0.1:51777"), Some(51777));
        assert_eq!(ws_port("ws://127.0.0.1:51777/"), Some(51777));
    }

    #[test]
    fn query_escape_preserves_paths_but_escapes_query_chars() {
        assert_eq!(
            query_escape("/Users/example/My Project?a=b"),
            "/Users/example/My%20Project%3Fa%3Db"
        );
    }

    #[test]
    fn fleet_mux_base_defaults_under_home_and_honors_override() {
        assert_eq!(
            fleet_mux_base_from(None, Some(PathBuf::from("/Users/example"))),
            PathBuf::from("/Users/example/.fleet/mux")
        );
        assert_eq!(
            fleet_mux_base_from(
                Some(PathBuf::from("/custom/fleet")),
                Some(PathBuf::from("/Users/example"))
            ),
            PathBuf::from("/custom/fleet")
        );
    }

    #[test]
    fn server_id_allocator_is_launch_local_and_stateless() {
        let dir = temp_test_dir("fleet-server-allocator-stateless");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws-server-1")).unwrap();
        std::fs::write(next_server_counter_path(&dir), "99\n").unwrap();

        let supervisor = ServerSupervisor::new_with_launch_id(
            51778,
            "ws://127.0.0.1:51777".into(),
            "token".into(),
            "testlaunch".into(),
        );
        let (first_n, first_id) = supervisor.allocate_server_id();
        let (second_n, second_id) = supervisor.allocate_server_id();

        assert_eq!(first_n, 1);
        assert_eq!(first_id, "server-testlaunch-1");
        assert_eq!(second_n, 2);
        assert_eq!(second_id, "server-testlaunch-2");
        assert_eq!(
            std::fs::read_to_string(next_server_counter_path(&dir)).unwrap(),
            "99\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_legacy_spawn_state_removes_manifest_and_counter_only() {
        let dir = temp_test_dir("fleet-clear-legacy-state");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(spawn_manifest_path(&dir), "{}").unwrap();
        std::fs::write(next_server_counter_path(&dir), "42\n").unwrap();
        std::fs::write(dir.join("keep.log"), "debug").unwrap();

        clear_legacy_spawn_state_in(&dir);

        assert!(!spawn_manifest_path(&dir).exists());
        assert!(!next_server_counter_path(&dir).exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("keep.log")).unwrap(),
            "debug"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_server_label_updates_matching_server_only() {
        let mut servers = vec![
            crate::mux::Server {
                id: "server-1".into(),
                label: "server-1".into(),
                url: "http://127.0.0.1:1/".into(),
                owned: true,
            },
            crate::mux::Server {
                id: "server-2".into(),
                label: "server-2".into(),
                url: "http://127.0.0.1:2/".into(),
                owned: true,
            },
        ];

        assert!(rename_server_label(&mut servers, "server-2", "Docs"));
        assert_eq!(servers[0].label, "server-1");
        assert_eq!(servers[1].label, "Docs");
        assert!(!rename_server_label(&mut servers, "missing", "Nope"));
    }

    #[test]
    fn primary_child_without_process_is_unavailable() {
        assert_eq!(
            primary_child_exited(&mut []),
            ChildHealth::Unavailable {
                reason: "no tracked child process".into()
            }
        );
    }

    #[test]
    fn fleet_spawn_cwd_defaults_to_home_and_honors_override() {
        let base = PathBuf::from("/Users/example/.fleet/mux");
        assert_eq!(
            fleet_spawn_cwd_from(None, Some(PathBuf::from("/Users/example")), &base),
            PathBuf::from("/Users/example")
        );
        assert_eq!(
            fleet_spawn_cwd_from(
                Some(PathBuf::from("/custom/cwd")),
                Some(PathBuf::from("/Users/example")),
                &base
            ),
            PathBuf::from("/custom/cwd")
        );
        assert_eq!(fleet_spawn_cwd_from(None, None, &base), base);
    }

    #[test]
    fn fleet_tool_path_adds_gui_launch_tool_dirs() {
        let home = temp_test_dir("fleet-tool-path-home");
        let current = temp_test_dir("fleet-tool-path-current");
        let local_bin = home.join(".local/bin");
        let home_bin = home.join("bin");
        let cargo_bin = home.join(".cargo/bin");
        let hm_bin = home
            .join("Applications")
            .join("Home Manager Apps")
            .join("cmux.app")
            .join("Contents")
            .join("Resources")
            .join("bin");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&current);
        for dir in [&current, &local_bin, &home_bin, &cargo_bin, &hm_bin] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let current_path = std::env::join_paths([&current]).unwrap();
        let path = fleet_tool_path_from(Some(current_path), Some(home.clone()), None);
        let dirs = std::env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(dirs.first(), Some(&current));
        let hm_pos = dirs.iter().position(|dir| dir == &hm_bin).unwrap();
        let local_pos = dirs.iter().position(|dir| dir == &local_bin).unwrap();
        assert!(hm_pos < local_pos);
        assert!(dirs.contains(&local_bin));
        assert!(dirs.contains(&home_bin));
        assert!(dirs.contains(&cargo_bin));
        assert!(dirs.contains(&hm_bin));
        assert_eq!(dirs.iter().filter(|dir| *dir == &local_bin).count(), 1);

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&current);
    }

    #[test]
    fn prepend_path_keeps_shim_first() {
        let shim = PathBuf::from("/tmp/fleet-shim");
        let first = PathBuf::from("/usr/bin");
        let second = PathBuf::from("/bin");
        let path = std::env::join_paths([&first, &second]).unwrap();

        let out = prepend_path(&shim, &path);
        let dirs = std::env::split_paths(&out).collect::<Vec<_>>();

        assert_eq!(dirs, vec![shim, first, second]);
    }

    #[test]
    fn spawned_server_settings_live_under_server_data_user_dir() {
        assert_eq!(
            spawned_server_settings_path(Path::new("/tmp/fleet-server-data")),
            PathBuf::from("/tmp/fleet-server-data/data/User/settings.json")
        );
    }

    #[test]
    fn write_spawned_server_settings_creates_fleet_owned_ui_settings() {
        let dir = temp_test_dir("fleet-spawn-settings-create");
        let _ = std::fs::remove_dir_all(&dir);

        write_spawned_server_settings(&dir).unwrap();

        let settings = read_json(&spawned_server_settings_path(&dir));
        assert_eq!(
            settings["terminal.integrated.gpuAcceleration"],
            serde_json::Value::String("off".into())
        );
        assert_eq!(settings["window.commandCenter"], serde_json::json!(false));
        assert_eq!(
            settings["workbench.layoutControl.enabled"],
            serde_json::json!(false)
        );
        assert_eq!(
            settings["workbench.navigationControl.enabled"],
            serde_json::json!(false)
        );
        assert_eq!(
            settings["security.workspace.trust.enabled"],
            serde_json::json!(false)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_spawned_server_settings_preserves_existing_settings_object() {
        let dir = temp_test_dir("fleet-spawn-settings-merge");
        let settings_path = spawned_server_settings_path(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(
            &settings_path,
            br#"{
  "editor.fontSize": 17,
  "terminal.integrated.gpuAcceleration": "auto"
}"#,
        )
        .unwrap();

        write_spawned_server_settings(&dir).unwrap();

        let settings = read_json(&settings_path);
        assert_eq!(settings["editor.fontSize"], serde_json::json!(17));
        assert_eq!(
            settings["terminal.integrated.gpuAcceleration"],
            serde_json::Value::String("off".into())
        );
        assert_eq!(settings["window.commandCenter"], serde_json::json!(false));
        assert_eq!(
            settings["workbench.layoutControl.enabled"],
            serde_json::json!(false)
        );
        assert_eq!(
            settings["workbench.navigationControl.enabled"],
            serde_json::json!(false)
        );
        assert_eq!(
            settings["security.workspace.trust.enabled"],
            serde_json::json!(false)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fleet_env_includes_bridge_token_and_log_dir() {
        let env = fleet_env(
            "/tmp/reporter.sock",
            "server-testlaunch-1",
            "server-1",
            "ws://127.0.0.1:51778",
            "http://127.0.0.1:9000/",
            "token-1",
            "/Users/example/.fleet/mux",
        );
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();

        assert_eq!(map["FLEET_BRIDGE_TOKEN"], "token-1");
        assert_eq!(map["FLEET_BRIDGE_LOG_DIR"], "/Users/example/.fleet/mux");
        assert_eq!(map["FLEET_BRIDGE_URL"], "ws://127.0.0.1:51778");
        assert_eq!(map["FLEET_SERVER_ID"], "server-testlaunch-1");
        assert_eq!(map["FLEET_SERVER_LABEL"], "server-1");
    }

    #[test]
    fn serve_web_args_avoid_desktop_only_workspace_trust_flag() {
        let args = serve_web_args("127.0.0.1", 12345, "/tmp/fleet-data", "/tmp/fleet-ws");

        assert!(!args.iter().any(|arg| arg == "--disable-workspace-trust"));
        assert!(args
            .iter()
            .any(|arg| arg == "--accept-server-license-terms"));
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--server-data-dir")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-data")
        );
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--default-folder")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-ws")
        );
    }

    #[test]
    fn code_server_args_disable_workspace_trust_for_web_config() {
        let args = code_server_args("127.0.0.1", 12345, "/tmp/fleet-data", "/tmp/fleet-ws");

        assert!(!args.iter().any(|arg| arg == "serve-web"));
        assert!(args.iter().any(|arg| arg == "--disable-workspace-trust"));
        assert!(args
            .iter()
            .any(|arg| arg == "--accept-server-license-terms"));
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--server-data-dir")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-data")
        );
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--default-folder")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-ws")
        );
    }

    #[test]
    fn local_server_args_use_direct_code_server_when_available() {
        let args = local_server_args(
            Path::new("/tmp/code-server"),
            "127.0.0.1",
            12345,
            Path::new("/tmp/fleet-data"),
            Path::new("/tmp/fleet-ws"),
        );

        assert!(!args.iter().any(|arg| arg == "serve-web"));
        assert!(args.iter().any(|arg| arg == "--disable-workspace-trust"));
    }

    #[test]
    fn local_server_args_fall_back_to_code_serve_web_for_code_cli() {
        let args = local_server_args(
            Path::new("/tmp/code"),
            "127.0.0.1",
            12345,
            Path::new("/tmp/fleet-data"),
            Path::new("/tmp/fleet-ws"),
        );

        assert_eq!(args.first().map(String::as_str), Some("serve-web"));
        assert!(!args.iter().any(|arg| arg == "--disable-workspace-trust"));
    }

    #[test]
    fn local_server_args_use_serve_web_for_code_tunnel_wrapper() {
        let args = local_server_args(
            Path::new(
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code-tunnel",
            ),
            "127.0.0.1",
            12345,
            Path::new("/tmp/fleet-data"),
            Path::new("/tmp/fleet-ws"),
        );

        assert_eq!(args.first().map(String::as_str), Some("serve-web"));
        assert!(!args.iter().any(|arg| arg == "--disable-workspace-trust"));
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--server-data-dir")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-data")
        );
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--default-folder")
                .map(|pair| pair[1].as_str()),
            Some("/tmp/fleet-ws")
        );
    }

    #[test]
    fn code_server_bin_from_home_finds_downloaded_serve_web_bundle() {
        let home = temp_test_dir("fleet-code-server-home");
        let _ = std::fs::remove_dir_all(&home);
        let bin = home
            .join(".vscode")
            .join("cli")
            .join("serve-web")
            .join("commit-1")
            .join("bin")
            .join("code-server");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();

        assert_eq!(code_server_bin_from_home(Some(home.clone())), Some(bin));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn terminate_child_tree_kills_background_descendants() {
        let dir = temp_test_dir("fleet-child-tree-kill");
        let pid_file = dir.join("background.pid");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(
                r#"
bg=""
cleanup() {
  if [ -n "$bg" ]; then
    kill "$bg" 2>/dev/null
    wait "$bg" 2>/dev/null
  fi
  exit 0
}
trap cleanup TERM INT
sleep 30 &
bg=$!
printf '%s\n' "$bg" > "$1"
wait "$bg"
"#,
            )
            .arg("fleet-child-tree-kill")
            .arg(&pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_fleet_child(&mut cmd).unwrap();

        let background_pid = read_pid_file_for(&pid_file, std::time::Duration::from_secs(2));
        assert!(
            process_exists(background_pid),
            "background child should be alive before termination"
        );

        terminate_child_tree(child);

        let gone = eventually(std::time::Duration::from_secs(2), || {
            !process_exists(background_pid)
        });
        assert!(gone, "background child should exit with its process group");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_close_clears_server_and_kills_descendants() {
        let dir = temp_test_dir("fleet-explicit-close-kill");
        let pid_file = dir.join("background.pid");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(
                r#"
sleep 30 &
printf '%s\n' "$!" > "$1"
wait
"#,
            )
            .arg("fleet-explicit-close-kill")
            .arg(&pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_fleet_child(&mut cmd).unwrap();
        let background_pid = read_pid_file_for(&pid_file, std::time::Duration::from_secs(2));

        let supervisor = ServerSupervisor::new_with_launch_id(
            51778,
            "ws://127.0.0.1:51777".into(),
            "token".into(),
            "testlaunch".into(),
        );
        supervisor.servers.lock().unwrap().push(crate::mux::Server {
            id: "server-test".into(),
            label: "server-test".into(),
            url: "http://127.0.0.1:1/".into(),
            owned: true,
        });
        supervisor
            .children
            .lock()
            .unwrap()
            .insert("server-test".into(), vec![child]);

        assert!(supervisor.close("server-test"));

        assert!(supervisor.servers().is_empty());
        let gone = eventually(std::time::Duration::from_secs(2), || {
            !process_exists(background_pid)
        });
        assert!(gone, "explicit close should terminate managed descendants");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_supervisor_does_not_kill_descendants() {
        let dir = temp_test_dir("fleet-drop-keeps-child");
        let pid_file = dir.join("background.pid");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(
                r#"
sleep 30 &
printf '%s\n' "$!" > "$1"
wait
"#,
            )
            .arg("fleet-drop-keeps-child")
            .arg(&pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_fleet_child(&mut cmd).unwrap();
        let child_pid = child.id();
        let background_pid = read_pid_file_for(&pid_file, std::time::Duration::from_secs(2));

        {
            let supervisor = ServerSupervisor::new_with_launch_id(
                51778,
                "ws://127.0.0.1:51777".into(),
                "token".into(),
                "testlaunch".into(),
            );
            supervisor.servers.lock().unwrap().push(crate::mux::Server {
                id: "server-test".into(),
                label: "server-test".into(),
                url: "http://127.0.0.1:1/".into(),
                owned: true,
            });
            supervisor
                .children
                .lock()
                .unwrap()
                .insert("server-test".into(), vec![child]);
        }

        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            process_exists(background_pid),
            "dropping Fleet supervisor must not terminate server descendants"
        );

        kill_process_group_for_test(child_pid);
        let gone = eventually(std::time::Duration::from_secs(2), || {
            !process_exists(background_pid)
        });
        assert!(gone, "test cleanup should terminate managed descendants");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn process_group_signal_pid_negates_child_pid() {
        assert_eq!(process_group_signal_pid(42), Some(-42));
    }

    #[cfg(unix)]
    #[test]
    fn claude_shim_removes_fleet_path_before_execing_wrapper() {
        let dir = temp_test_dir("fleet-claude-shim-path-clean");
        let shim_dir = dir.join("fleet-shim");
        let wrapper_dir = dir.join("cmux-wrapper");
        let real_dir = dir.join("real-claude");
        let hooks_file = shim_dir.join("fleet-hooks.json");
        let shim = shim_dir.join("claude");
        let wrapper = wrapper_dir.join("claude");
        let real = real_dir.join("claude");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(&hooks_file, "{}").unwrap();

        write_executable(
            &wrapper,
            r#"#!/bin/sh
self_dir=${0%/*}
clean_path=""
saved_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  [ "$dir" = "$self_dir" ] && continue
  if [ -z "$clean_path" ]; then clean_path="$dir"; else clean_path="$clean_path:$dir"; fi
done
IFS=$saved_ifs
PATH="$clean_path"
export PATH
exec claude CMUX "$@"
"#,
        );
        write_executable(
            &real,
            r#"#!/bin/sh
printf 'REAL_CLAUDE\n'
for arg in "$@"; do printf 'ARG:%s\n' "$arg"; done
"#,
        );
        write_executable(&shim, &claude_shim_script(&wrapper, &hooks_file, &shim_dir));

        let out = Command::new(&shim)
            .env(
                "PATH",
                format!(
                    "{}:{}:{}",
                    shim_dir.display(),
                    wrapper_dir.display(),
                    real_dir.display()
                ),
            )
            .arg("hello")
            .output()
            .unwrap();

        assert!(out.status.success());
        let stdout = String::from_utf8(out.stdout).unwrap();
        let args = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("ARG:"))
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["CMUX", "--settings", hooks_file.to_str().unwrap(), "hello"]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_real_claude_uses_supplied_tool_path() {
        let dir = temp_test_dir("fleet-claude-tool-path");
        let shim_dir = dir.join("shim");
        let real_dir = dir.join("real");
        let real = real_dir.join("claude");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(&real, "").unwrap();
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        assert_eq!(find_real_claude(&shim_dir, &path), Some(real));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bridge_extensions_dir_lives_under_server_data() {
        assert_eq!(
            fleet_bridge_extensions_dir(Path::new("/tmp/fleet-server-data")),
            PathBuf::from("/tmp/fleet-server-data/extensions")
        );
    }

    #[test]
    fn bridge_installed_detects_fleet_bridge_directory() {
        let dir = std::env::temp_dir().join(format!(
            "fleet-bridge-installed-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fleet-team.fleet-bridge-0.2.0")).unwrap();

        assert!(fleet_bridge_installed(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
    }

    fn read_json(path: &Path) -> serde_json::Value {
        let bytes = std::fs::read(path).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn read_pid_file_for(path: &Path, timeout: std::time::Duration) -> u32 {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("timed out waiting for pid file {}", path.display());
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        unsafe { libc_kill(pid, 0) == 0 }
    }

    #[cfg(unix)]
    fn kill_process_group_for_test(pid: u32) {
        if let Some(group_pid) = process_group_signal_pid(pid) {
            let _ = signal_process_group(group_pid, SIGTERM);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = signal_process_group(group_pid, SIGKILL);
        }
    }

    #[cfg(unix)]
    fn eventually(timeout: std::time::Duration, mut pred: impl FnMut() -> bool) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if pred() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        pred()
    }
}

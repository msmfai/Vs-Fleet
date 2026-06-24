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

    /// Test-only: push a synthetic spawned server (no real process tree) so other
    /// modules' tests can exercise State mutations against the supervisor without
    /// launching VS Code. Mirrors the spawn.rs `push_server` test helper.
    #[cfg(test)]
    pub(crate) fn push_test_server(&self, id: &str) {
        self.servers.lock().unwrap().push(crate::mux::Server {
            id: id.into(),
            label: id.into(),
            url: format!("http://127.0.0.1:1/{id}"),
            owned: true,
            renamed: false,
        });
    }

    /// Spawn a new server. Routes to the container path when `FLEET_SPAWN_MODE=container`,
    /// else the default local-process path. Both return a [`Server`] that Fleet adds
    /// to the rail immediately; the spawned env phones home to the bridge on its own.
    // Glue: thin default wrapper over `spawn_with`, which launches a real
    // process tree (excluded below).
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn spawn(&self) -> std::io::Result<crate::mux::Server> {
        self.spawn_with(SpawnRequest::default())
    }

    // Routing shell: every arm launches a real editor/reporter/docker/ssh
    // process tree (see the `spawn_*` methods below), which needs a host with
    // VS Code / Docker / SSH — not available in CI. The pure pieces it composes
    // (`spawn_mode`, `expand_user_path`, `default_spawn_folder`) are unit-tested.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn spawn_with(&self, request: SpawnRequest) -> std::io::Result<crate::mux::Server> {
        let mode = request
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| !mode.is_empty());
        match resolve_spawn_route(mode, spawn_mode()) {
            SpawnRoute::Container => self.spawn_container(),
            SpawnRoute::Ssh => self.spawn_ssh(),
            // An explicit `mode == "local"` request forces a concrete folder
            // (defaulting it) so an explicit local create never lands on the
            // env-default folder path; the env-driven local route keeps the
            // request's optional folder as-is.
            SpawnRoute::LocalExplicit => self.spawn_local(Some(
                request
                    .folder
                    .as_deref()
                    .map(expand_user_path)
                    .unwrap_or_else(default_spawn_folder),
            )),
            SpawnRoute::Local => {
                self.spawn_local(request.folder.as_deref().map(expand_user_path))
            }
        }
    }

    /// Spawn a new VS Code web server (+ its reporter + claude shim) and record it.
    /// Returns its [`Server`]; Fleet adds it to the rail immediately.
    // Glue: launches `code serve-web` (or the downloaded code-server) plus a
    // reporter child, installs the bridge VSIX, and writes server settings — all
    // requiring a real local VS Code install. Its pure helpers (`local_server_args`,
    // `fleet_env`, `query_escape`, `prepend_path`, settings/shim writers) are tested.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        if !editor_resolved(&editor, &tool_path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                EDITOR_MISSING_MESSAGE,
            ));
        }
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
            renamed: false,
        };
        self.children.lock().unwrap().insert(id.clone(), children);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
    }

    /// Launch this server's `fleet-reporter --serve` (session id = server id).
    // Glue: spawns the real `fleet-reporter` binary as a child process.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    // Glue: shells out to `docker run`/`docker inspect`/`docker rm` — needs a
    // working Docker daemon. The reachable-URL parser (`inspect_url`) is separate.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
            renamed: false,
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
    // Glue: launches `ssh` with reverse/forward tunnels and runs the remote
    // editor+reporter shell. The pure remote-command and tunnel-arg builders it
    // composes (`ssh_remote_command`, `ssh_tunnel_args`, `ssh_remote_ports`) are
    // unit-tested below.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn spawn_ssh(&self) -> std::io::Result<crate::mux::Server> {
        let target = std::env::var("FLEET_SSH_TARGET")
            .map_err(|_| std::io::Error::other("FLEET_SSH_TARGET not set (e.g. user@host)"))?;
        let (n, id) = self.allocate_server_id();
        let label = format!("{} @ {target}", server_label(n));

        // local_editor is free on THIS host; the remote ports are bound on the remote
        // by ssh's tunnels (ExitOnForwardFailure makes a collision fail loudly).
        let local_editor = free_port()?;
        let RemotePorts {
            r_cs,
            r_hub,
            r_bridge,
        } = ssh_remote_ports(n);
        let local_hub = ws_port(&self.hub_url).unwrap_or(51777);
        let local_bridge = self.bridge_port;

        let remote_editor =
            std::env::var("FLEET_REMOTE_EDITOR_BIN").unwrap_or_else(|_| "code".into());
        let remote_reporter =
            std::env::var("FLEET_REMOTE_REPORTER_BIN").unwrap_or_else(|_| "fleet-reporter".into());

        // The editor surface is reached locally through the -L tunnel.
        let url = format!("http://127.0.0.1:{local_editor}/");
        let remote_cmd = ssh_remote_command(
            &id,
            &label,
            r_cs,
            r_hub,
            r_bridge,
            &self.bridge_token,
            &remote_editor,
            &remote_reporter,
            std::env::var("FLEET_SPAWN_REPO").ok().as_deref(),
        );

        let (out, err) = log_files(&format!("ssh-{id}"));
        let mut cmd = fleet_command("ssh");
        cmd.args(ssh_tunnel_args(
            local_editor,
            r_cs,
            r_hub,
            local_hub,
            r_bridge,
            local_bridge,
        ))
        .arg(&target)
        .arg(&remote_cmd)
        .stdout(out)
        .stderr(err);
        let child = spawn_fleet_child(&mut cmd)?;

        tracing::info!(%id, %target, local_editor, r_cs, "deployed code-server over ssh");
        let server = crate::mux::Server {
            id: id.clone(),
            label,
            url,
            owned: true,
            renamed: false,
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
            remove_container(id, &name);
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
                // `id` came straight from iterating `children`, so the entry is
                // always present.
                let group = children.remove(&id).unwrap_or_default();
                for child in group {
                    terminate_child_tree(child);
                }
                removed.push(id);
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

/// Glue: `docker rm -f` a container-mode server. Needs a Docker daemon.
#[cfg_attr(coverage_nightly, coverage(off))]
fn remove_container(id: &str, name: &str) {
    let docker = docker_bin();
    let _ = Command::new(&docker)
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    tracing::info!(%id, %name, "removed spawned container");
}

/// The three remote loopback ports an ssh deploy binds (editor, hub home,
/// bridge home), derived from the server's launch-local sequence number.
struct RemotePorts {
    r_cs: u16,
    r_hub: u16,
    r_bridge: u16,
}

fn ssh_remote_ports(n: u64) -> RemotePorts {
    RemotePorts {
        r_cs: 18000 + (n as u16 % 1000),
        r_hub: 19000 + (n as u16 % 1000),
        r_bridge: 20000 + (n as u16 % 1000),
    }
}

/// The `ssh` tunnel + keepalive arguments (everything before `<target> <cmd>`):
/// a `-L` editor tunnel and two `-R` reverse tunnels (hub + bridge phone-home).
fn ssh_tunnel_args(
    local_editor: u16,
    r_cs: u16,
    r_hub: u16,
    local_hub: u16,
    r_bridge: u16,
    local_bridge: u16,
) -> Vec<String> {
    vec![
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-L".into(),
        format!("{local_editor}:127.0.0.1:{r_cs}"),
        "-R".into(),
        format!("{r_hub}:127.0.0.1:{local_hub}"),
        "-R".into(),
        format!("{r_bridge}:127.0.0.1:{local_bridge}"),
    ]
}

/// The remote shell command an ssh deploy runs: prepare the workspace (clone
/// `FLEET_SPAWN_REPO` or mkdir), start the reporter in the background dialing the
/// `-R` hub tunnel, then run the code-server (serve-web) bound to the remote
/// loopback so its bridge dials the `-R` bridge tunnel home. An EXIT trap reaps
/// the reporter when ssh drops. Pure: the exact same wire as `spawn_local`.
#[allow(clippy::too_many_arguments)]
fn ssh_remote_command(
    id: &str,
    label: &str,
    r_cs: u16,
    r_hub: u16,
    r_bridge: u16,
    bridge_token: &str,
    remote_editor: &str,
    remote_reporter: &str,
    spawn_repo: Option<&str>,
) -> String {
    let remote_ws = format!(".fleet/ws-{id}");
    let remote_sock = format!("/tmp/fleet-reporter-{id}.sock");
    let remote_userdata = format!(".fleet/cs-userdata-{id}");
    let url = format!("http://127.0.0.1:{r_cs}/");

    let args = serve_web_args("127.0.0.1", r_cs, &remote_userdata, &remote_ws);
    let env = fleet_env(
        &remote_sock,
        id,
        label,
        &format!("ws://127.0.0.1:{r_bridge}"),
        &url,
        bridge_token,
        ".fleet",
    );

    let env_str = env
        .iter()
        .map(|(k, v)| format!("{k}={}", shq(v)))
        .collect::<Vec<_>>()
        .join(" ");
    let args_str = args.iter().map(|a| shq(a)).collect::<Vec<_>>().join(" ");
    let ws_prep = match spawn_repo {
        Some(spec) if !spec.trim().is_empty() => format!(
            "git clone --depth 1 {} {} 2>/dev/null || mkdir -p {};",
            shq(&resolve_repo(spec)),
            shq(&remote_ws),
            shq(&remote_ws)
        ),
        _ => format!("mkdir -p {};", shq(&remote_ws)),
    };
    format!(
        "{ws_prep} mkdir -p {ud} 2>/dev/null; rm -f {sock}; \
         {rep} --serve --ws ws://127.0.0.1:{rhub} --socket {sock} --session-id {id} >/tmp/fleet-rep-{id}.log 2>&1 & \
         RPID=$!; trap 'kill $RPID 2>/dev/null' EXIT INT TERM; \
         env {env} {ed} {args}; kill $RPID 2>/dev/null",
        ws_prep = ws_prep,
        ud = shq(&remote_userdata),
        sock = shq(&remote_sock),
        rep = shq(remote_reporter),
        rhub = r_hub,
        id = id,
        env = env_str,
        ed = shq(remote_editor),
        args = args_str,
    )
}

fn rename_server_label(servers: &mut [crate::mux::Server], id: &str, label: &str) -> bool {
    if let Some(server) = servers.iter_mut().find(|server| server.id == id) {
        server.label = label.to_string();
        server.renamed = true;
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
    classify_try_wait(pid, child.try_wait())
}

/// Classify a `try_wait` result. Split out so the genuinely reachable arms
/// (Alive / Exited / the no-child Unavailable at the call site) stay tested; the
/// `Err` arm only fires on a kernel-level waitpid failure for a child we own,
/// which can't be induced deterministically from a unit test, hence excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
fn classify_try_wait(
    pid: u32,
    result: std::io::Result<Option<std::process::ExitStatus>>,
) -> ChildHealth {
    match result {
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
// The macOS `!cfg!(test)` trampoline branch is dead under `cargo test` (the test
// harness binary has no trampoline), so it can never run from a unit test; the
// non-macOS / under-test path (a plain `Command::new`) is validated by
// `fleet_command_under_test_spawns_directly`. Excluded wholesale because the
// macOS branch is unreachable in the coverage harness.
#[cfg_attr(coverage_nightly, coverage(off))]
fn fleet_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(target_os = "macos")]
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

    // FFI glue: the success path is validated at runtime by
    // `disclaim_spawnattr_is_supported`, but the `rc != 0` kernel-failure arms
    // can't be induced from a unit test.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    // FFI glue: on success `posix_spawn(SETEXEC)` replaces this process and never
    // returns; reaching the end is itself the error path. The whole hop is exercised
    // by the live smoke run (the `--fleet-disclaim-exec` trampoline in `main`), not
    // by unit tests.
    #[cfg_attr(coverage_nightly, coverage(off))]
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

/// The concrete spawn path `spawn_with` dispatches to, decided purely from the
/// request's `mode` override (already trimmed + non-blank-filtered) and the
/// env-derived [`SpawnMode`]. Split out so the routing — the testable decision
/// logic of `spawn_server_with_options` — is verifiable without launching a
/// process tree (the live spawn itself is covered only by the CI smoke/E2E).
#[derive(Debug, PartialEq, Eq)]
enum SpawnRoute {
    Container,
    Ssh,
    /// `mode == "local"` was requested explicitly (force a concrete folder).
    LocalExplicit,
    /// The env default chose local (carry the request's optional folder).
    Local,
}

fn resolve_spawn_route(mode_override: Option<&str>, env_mode: SpawnMode) -> SpawnRoute {
    // An explicit request mode wins over the env knob.
    match mode_override {
        Some("container") => SpawnRoute::Container,
        Some("local") => SpawnRoute::LocalExplicit,
        _ => match env_mode {
            SpawnMode::Container => SpawnRoute::Container,
            SpawnMode::Ssh => SpawnRoute::Ssh,
            SpawnMode::Local => SpawnRoute::Local,
        },
    }
}

/// The `docker` CLI to drive (`FLEET_DOCKER_BIN`, default `docker`).
fn docker_bin() -> PathBuf {
    if let Some(bin) = nonblank_env("FLEET_DOCKER_BIN") {
        return PathBuf::from(bin);
    }
    find_on_path("docker", &fleet_tool_path()).unwrap_or_else(|| PathBuf::from("docker"))
}

/// Shown as a rail **warning** (not an error) when a local spawn is requested
/// without a local VS Code: Fleet without VS Code is a supported setup —
/// externally started / remote sessions phone home on their own; only local
/// spawns need the `code` CLI. See `mux::emit_spawn_error`.
pub(crate) const EDITOR_MISSING_MESSAGE: &str = "VS Code not found — local sessions need a VS Code install with the `code` CLI (remote sessions are unaffected)";

/// Whether the resolved editor actually exists: an absolute path that is a
/// file, or a bare name (e.g. a `FLEET_EDITOR_BIN` override) found on PATH.
fn editor_resolved(editor: &Path, tool_path: &OsStr) -> bool {
    editor.is_file()
        || editor
            .to_str()
            .is_some_and(|name| find_on_path(name, tool_path).is_some())
}

fn editor_bin(path: &OsStr) -> PathBuf {
    if let Some(bin) = nonblank_env("FLEET_EDITOR_BIN") {
        return PathBuf::from(bin);
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
    if let Some(bin) = nonblank_env("FLEET_CODE_SERVER_BIN") {
        return Some(PathBuf::from(bin));
    }
    let from_home = code_server_bin_from_home(home_dir());
    if from_home.is_some() {
        return from_home;
    }

    // Best effort for first run: ask the official `code serve-web` wrapper to
    // materialize its downloaded server bundle, then look again.
    materialize_serve_web_bundle(editor);
    code_server_bin_from_home(home_dir())
}

/// Glue: run `code serve-web --help` once so the official CLI downloads its
/// server bundle to `~/.vscode/cli`. Needs a real VS Code CLI.
#[cfg_attr(coverage_nightly, coverage(off))]
fn materialize_serve_web_bundle(editor: &Path) {
    let _ = Command::new(editor)
        .args(["serve-web", "--help"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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

/// Read `key` from the environment, returning it only when set to a non-blank
/// value (the common "override knob" shape: blank/unset both mean "not set").
fn nonblank_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn reporter_bin() -> PathBuf {
    if let Some(bin) = nonblank_env("FLEET_REPORTER_BIN") {
        return PathBuf::from(bin);
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
    let mut candidates = windows_executable_candidates(dir, name);
    candidates.push(dir.join(name));
    candidates
}

/// Windows-native launcher forms (`name.exe`/`.cmd`/`.bat`), which precede the
/// bare POSIX-script `name`. Compiled to a no-op on non-Windows targets so the
/// extension loop is never dead code in the coverage measurement.
#[cfg(windows)]
fn windows_executable_candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    ["exe", "cmd", "bat"]
        .into_iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .collect()
}

#[cfg(not(windows))]
fn windows_executable_candidates(_dir: &Path, _name: &str) -> Vec<PathBuf> {
    Vec::new()
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
// Glue: shells out to `docker inspect`; needs a running Docker daemon.
#[cfg_attr(coverage_nightly, coverage(off))]
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
// Glue: runs `code --install-extension`; needs a real VS Code CLI. The pure path
// helpers it composes (`fleet_bridge_extensions_dir`, `fleet_bridge_installed`,
// `find_fleet_bridge_vsix`) are unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
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

    // The settings path is `<server_data>/data/User/settings.json`, so it always
    // has a parent; create it (and surface any I/O error to the caller).
    let parent = settings_path
        .parent()
        .expect("spawned settings path always has a parent");
    std::fs::create_dir_all(parent)?;
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
    let override_file = nonblank_env("FLEET_BRIDGE_VSIX")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    match override_file {
        Some(path) => Some(path),
        None => find_bundled_vsix(&bridge_vsix_search_dirs()),
    }
}

/// Where to look for a bundled `fleet-bridge` VSIX: dirs relative to the running
/// binary (a Tauri bundle), plus the in-repo `packages/fleet-bridge`. Thin glue
/// over `current_exe`; the actual matching is the pure `find_bundled_vsix`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn bridge_vsix_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.extend(bundle_search_dirs(exe_dir));
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/fleet-bridge"));
    dirs
}

/// First VSIX across `dirs`: prefer an exact `fleet-bridge.vsix`, else any `.vsix`
/// in the directory. `None` if no directory holds one. Pure given its dirs.
fn find_bundled_vsix(dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let named = dir.join("fleet-bridge.vsix");
        if named.is_file() {
            return Some(named);
        }
        if let Some(found) = first_vsix_in(dir) {
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
// The happy path and the no-`claude` NotFound path are validated by
// `install_claude_shim_*` tests; the residual uncovered lines are the `?`
// error arms of the intermediate filesystem writes (create_dir_all / write /
// set_permissions), which can't be made to fail deterministically mid-sequence
// from a unit test, so the whole writer is excluded from the line bar.
#[cfg(unix)]
#[cfg_attr(coverage_nightly, coverage(off))]
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

    // Off macOS there is no TCC-disclaim concern, so `server_spawn_target` is a
    // pass-through: it spawns the serve-web wrapper as-is (the direct-node rewrite
    // is macOS-only, cfg'd out here). Covers the non-macOS fallback path.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn server_spawn_target_is_passthrough_off_macos() {
        let bin = PathBuf::from("/opt/serve-web/bin/code-server");
        let (target, prefix) = server_spawn_target(&bin);
        assert_eq!(target, bin);
        assert!(prefix.is_empty());
    }

    #[test]
    fn editor_resolved_accepts_files_and_path_hits_only() {
        let empty_path = OsString::new();
        assert!(editor_resolved(Path::new("/bin/echo"), &empty_path));
        // The bare-name fallback (`editor_bin` found nothing): unresolved on an
        // empty PATH — this is the case the rail downgrades to a warning.
        assert!(!editor_resolved(Path::new("code"), &empty_path));
        assert!(!editor_resolved(
            Path::new("/nonexistent/bin/code"),
            &empty_path
        ));
        // A bare-name override is fine when PATH can find it.
        let bin_dir: OsString = "/bin".into();
        assert!(editor_resolved(Path::new("echo"), &bin_dir));
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
    fn clear_legacy_spawn_state_logs_but_survives_unremovable_entry() {
        let dir = temp_test_dir("fleet-clear-legacy-unremovable");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Make `servers.json` a NON-EMPTY directory: `remove_file` then returns an
        // error that is neither Ok nor NotFound, hitting the warn arm.
        let manifest = spawn_manifest_path(&dir);
        std::fs::create_dir_all(&manifest).unwrap();
        std::fs::write(manifest.join("inner"), "x").unwrap();

        // Must not panic, and must still clear the (absent) counter cleanly.
        clear_legacy_spawn_state_in(&dir);
        assert!(manifest.is_dir(), "directory-as-manifest is left in place");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_real_claude_ignores_nonfile_override_and_empty_path_dirs() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-find-claude-nonfile");
        let real_dir = dir.join("real");
        let claudeless_dir = dir.join("no-claude");
        let real = real_dir.join("claude");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::create_dir_all(&claudeless_dir).unwrap();
        std::fs::write(&real, "").unwrap();

        // Override points at a directory (not a file): the override is skipped and
        // PATH search proceeds. An empty component is skipped, a dir WITHOUT a
        // `claude` is passed over (is_file false → continue), then the real one wins.
        let _g = EnvGuard::set("FLEET_CLAUDE_BIN", real_dir.to_str().unwrap());
        let path =
            std::env::join_paths([PathBuf::from(""), claudeless_dir.clone(), real_dir.clone()])
                .unwrap();
        assert_eq!(find_real_claude(&dir.join("shim"), &path), Some(real));

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
                renamed: false,
            },
            crate::mux::Server {
                id: "server-2".into(),
                label: "server-2".into(),
                url: "http://127.0.0.1:2/".into(),
                owned: true,
                renamed: false,
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
    fn write_spawned_server_settings_errors_when_parent_is_a_file() {
        let dir = temp_test_dir("fleet-spawn-settings-enotdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Put a regular FILE where the `data` dir component must be, so
        // create_dir_all of the settings parent fails (ENOTDIR) — the `?` error
        // propagation path.
        std::fs::write(dir.join("data"), "x").unwrap();
        let err = write_spawned_server_settings(&dir).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
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

        // A home with no serve-web dir at all yields nothing.
        let empty = temp_test_dir("fleet-code-server-home-empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(code_server_bin_from_home(Some(empty.clone())), None);
        assert_eq!(code_server_bin_from_home(None), None);

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn code_server_bin_from_home_skips_binless_dirs_and_picks_newest() {
        let home = temp_test_dir("fleet-code-server-newest");
        let _ = std::fs::remove_dir_all(&home);
        let root = home.join(".vscode").join("cli").join("serve-web");
        // A commit dir with no bin/ at all (hits the `continue` skip arm).
        std::fs::create_dir_all(root.join("commit-empty")).unwrap();
        // Two real bundles; the one touched later must win the modified-time sort.
        let older = root.join("commit-old").join("bin").join("code-server");
        let newer = root.join("commit-new").join("bin").join("code-server");
        std::fs::create_dir_all(older.parent().unwrap()).unwrap();
        std::fs::create_dir_all(newer.parent().unwrap()).unwrap();
        std::fs::write(&older, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&newer, "").unwrap();

        assert_eq!(code_server_bin_from_home(Some(home.clone())), Some(newer));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn local_code_server_bin_returns_none_when_no_bundle_anywhere() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_CODE_SERVER_BIN");
        let home = temp_test_dir("fleet-local-cs-empty-home");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let _h = EnvGuard::set("HOME", home.to_str().unwrap());
        let _u = EnvGuard::unset("USERPROFILE");

        // No override, no home bundle; the serve-web bootstrap (best-effort, points
        // at a nonexistent editor) can't materialize one either ⇒ None.
        assert_eq!(local_code_server_bin(Path::new("/nonexistent/code")), None);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn wait_child_for_times_out_on_a_live_child() {
        // A long-lived child never exits within a tiny window ⇒ false.
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(!wait_child_for(
            &mut child,
            std::time::Duration::from_millis(40)
        ));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn local_code_server_bin_finds_home_bundle_when_editor_is_plain_code() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_CODE_SERVER_BIN");
        let home = temp_test_dir("fleet-local-cs-home");
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
        let _h = EnvGuard::set("HOME", home.to_str().unwrap());
        let _u = EnvGuard::unset("USERPROFILE");

        // Editor is the plain `code` CLI (not code-server, no override) ⇒ the
        // downloaded home bundle is found without the serve-web bootstrap.
        assert_eq!(local_code_server_bin(Path::new("/usr/bin/code")), Some(bin));

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
            renamed: false,
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
                renamed: false,
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

    #[cfg(unix)]
    #[test]
    fn install_claude_shim_writes_executable_shim_and_hooks() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-install-claude-shim");
        let shim_dir = dir.join("shim");
        let real = dir.join("real-claude");
        let socket = dir.join("reporter.sock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&real, "").unwrap();

        // Point the shim at a known real claude via the explicit override.
        let _g = EnvGuard::set("FLEET_CLAUDE_BIN", real.to_str().unwrap());
        let shim = install_claude_shim(&shim_dir, &socket, &OsString::new()).unwrap();

        assert_eq!(shim, shim_dir.join("claude"));
        // The hooks document was written and references the reporter socket.
        let hooks = std::fs::read_to_string(shim_dir.join("fleet-hooks.json")).unwrap();
        assert!(hooks.contains(&socket.display().to_string()));
        // The shim is an executable script that execs the real claude with --settings.
        let script = std::fs::read_to_string(&shim).unwrap();
        assert!(script.contains("exec"));
        assert!(script.contains("--settings"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&shim).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "shim must be executable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_claude_shim_errors_when_no_real_claude_found() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_CLAUDE_BIN");
        let dir = temp_test_dir("fleet-install-claude-shim-none");
        let _ = std::fs::remove_dir_all(&dir);
        // Empty PATH ⇒ no real claude ⇒ NotFound.
        let err = install_claude_shim(&dir.join("shim"), &dir.join("s.sock"), &OsString::new())
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
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
    fn bridge_installed_is_false_when_dir_absent_or_empty() {
        let dir = temp_test_dir("fleet-bridge-installed-absent");
        let _ = std::fs::remove_dir_all(&dir);
        // Missing directory ⇒ false.
        assert!(!fleet_bridge_installed(&dir));
        // Present but with only non-bridge entries ⇒ false.
        std::fs::create_dir_all(dir.join("some.other.extension-1.0.0")).unwrap();
        assert!(!fleet_bridge_installed(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_bundled_vsix_prefers_exact_name_then_any_then_none() {
        let dir = temp_test_dir("fleet-bundled-vsix");
        let exact = dir.join("exact");
        let other = dir.join("other");
        let empty = dir.join("empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&exact).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&empty).unwrap();

        // No directory holds a vsix ⇒ None.
        assert_eq!(find_bundled_vsix(std::slice::from_ref(&empty)), None);

        // A dir with only a differently-named vsix ⇒ that one (via first_vsix_in).
        let some_vsix = other.join("fleet-bridge-9.9.9.vsix");
        std::fs::write(&some_vsix, "").unwrap();
        assert_eq!(
            find_bundled_vsix(&[empty.clone(), other.clone()]),
            Some(some_vsix)
        );

        // An exact fleet-bridge.vsix wins over any other in an earlier dir.
        let named = exact.join("fleet-bridge.vsix");
        std::fs::write(&named, "").unwrap();
        assert_eq!(
            find_bundled_vsix(&[exact.clone(), other.clone()]),
            Some(named)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_files_fall_back_to_null_when_dir_is_unusable() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-log-files-enotdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Make a regular FILE, then point the mux base *inside* it: create_dir_all
        // and File::create both fail (ENOTDIR), exercising the null fallback.
        let blocker = dir.join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let _g = EnvGuard::set("FLEET_MUX_DIR", blocker.join("sub").to_str().unwrap());

        // Must not panic; both handles are produced (null-backed).
        let (_out, _err) = log_files("whatever");
        assert!(!blocker.join("sub").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fleet_tool_path_handles_missing_hm_apps_and_per_user_dir() {
        let _lock = lock_env();
        // A home with NO "Applications/Home Manager Apps" dir hits the early return
        // in push_home_manager_app_bins; a user adds the /etc/profiles per-user dir
        // candidate (which won't exist, so it's filtered out — but the branch runs).
        let home = temp_test_dir("fleet-tool-path-no-hm");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();

        let path = fleet_tool_path_from(None, Some(home.clone()), Some("someuser".into()));
        // The result is a valid joined PATH (never panics) even with no extra dirs.
        assert!(std::env::split_paths(&path).count() >= 1 || path.is_empty());

        // An empty user string is ignored (the per-user branch is skipped).
        let path2 = fleet_tool_path_from(None, Some(home.clone()), Some(String::new()));
        let _ = std::env::split_paths(&path2).count();

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn push_home_manager_app_bins_filters_non_app_entries() {
        let home = temp_test_dir("fleet-hm-filter");
        let _ = std::fs::remove_dir_all(&home);
        let hm = home.join("Applications").join("Home Manager Apps");
        // One real .app with a resources bin, plus a non-.app entry to be filtered.
        let app_bin = hm
            .join("tool.app")
            .join("Contents")
            .join("Resources")
            .join("bin");
        std::fs::create_dir_all(&app_bin).unwrap();
        std::fs::create_dir_all(hm.join("README.txt-dir")).unwrap();

        let mut dirs = Vec::new();
        push_home_manager_app_bins(&mut dirs, &home);
        assert!(dirs.contains(&app_bin));
        assert_eq!(dirs.len(), 1, "only the .app bin is added");

        let _ = std::fs::remove_dir_all(&home);
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

    // Env-mutating tests share one mutex so concurrent threads can't see each
    // other's transient `set_var`/`remove_var`. Each restores unconditionally.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // Tolerate a prior test panicking while holding the guard: the env is always
    // restored by EnvGuard's Drop, so the mutex state itself stays usable.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn test_supervisor() -> ServerSupervisor {
        ServerSupervisor::new_with_launch_id(
            51778,
            "ws://127.0.0.1:51777".into(),
            "token".into(),
            "testlaunch".into(),
        )
    }

    fn push_server(sup: &ServerSupervisor, id: &str) {
        sup.servers.lock().unwrap().push(crate::mux::Server {
            id: id.into(),
            label: id.into(),
            url: format!("http://127.0.0.1:1/{id}"),
            owned: true,
            renamed: false,
        });
    }

    #[test]
    fn ssh_remote_ports_are_offset_by_sequence_number() {
        let p = ssh_remote_ports(3);
        assert_eq!(p.r_cs, 18003);
        assert_eq!(p.r_hub, 19003);
        assert_eq!(p.r_bridge, 20003);
        // Wraps at 1000 to stay in range.
        let wrapped = ssh_remote_ports(1003);
        assert_eq!(wrapped.r_cs, 18003);
    }

    #[test]
    fn ssh_tunnel_args_carry_editor_and_reverse_tunnels() {
        let args = ssh_tunnel_args(7000, 18001, 19001, 51777, 20001, 51778);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-L" && w[1] == "7000:127.0.0.1:18001"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-R" && w[1] == "19001:127.0.0.1:51777"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-R" && w[1] == "20001:127.0.0.1:51778"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-o" && w[1] == "ExitOnForwardFailure=yes"));
    }

    #[test]
    fn ssh_remote_command_runs_reporter_then_editor_with_trap() {
        let cmd = ssh_remote_command(
            "server-x-1",
            "server-1 @ host",
            18001,
            19001,
            20001,
            "tok",
            "code",
            "fleet-reporter",
            None,
        );
        // No repo ⇒ plain mkdir for the workspace.
        assert!(cmd.contains("mkdir -p '.fleet/ws-server-x-1';"));
        // Reporter dials the -R hub tunnel and uses the per-server socket + id.
        assert!(cmd.contains("'fleet-reporter' --serve --ws ws://127.0.0.1:19001"));
        assert!(cmd.contains("--session-id server-x-1"));
        assert!(cmd.contains("/tmp/fleet-reporter-server-x-1.sock"));
        // EXIT trap reaps the reporter; the bridge env points at the -R bridge tunnel.
        assert!(cmd.contains("trap 'kill $RPID 2>/dev/null' EXIT INT TERM"));
        assert!(cmd.contains("FLEET_BRIDGE_URL='ws://127.0.0.1:20001'"));
        assert!(cmd.contains("FLEET_BRIDGE_TOKEN='tok'"));
        // serve-web is launched bound to the remote loopback editor port.
        assert!(cmd.contains("'serve-web'"));
        assert!(cmd.contains("'--port' '18001'"));
    }

    #[test]
    fn ssh_remote_command_clones_spawn_repo_when_set() {
        let cmd = ssh_remote_command(
            "server-x-2",
            "lbl",
            18002,
            19002,
            20002,
            "tok",
            "code",
            "fleet-reporter",
            Some("owner/repo"),
        );
        assert!(cmd.contains(
            "git clone --depth 1 'https://github.com/owner/repo.git' '.fleet/ws-server-x-2'"
        ));
        // A blank repo spec falls back to mkdir.
        let plain = ssh_remote_command(
            "server-x-3",
            "lbl",
            1,
            2,
            3,
            "t",
            "code",
            "fleet-reporter",
            Some("   "),
        );
        assert!(plain.contains("mkdir -p '.fleet/ws-server-x-3';"));
        assert!(!plain.contains("git clone"));
    }

    #[test]
    fn new_constructs_supervisor_with_a_real_launch_id() {
        // Exercises the public `new` (which derives a real launch id) rather than
        // the test-only `new_with_launch_id`.
        let sup = ServerSupervisor::new(51778, "ws://127.0.0.1:51777".into(), "token".into());
        let (n, id) = sup.allocate_server_id();
        assert_eq!(n, 1);
        assert!(id.starts_with("server-"));
        assert!(id.ends_with("-1"));
    }

    #[test]
    fn free_port_returns_a_bindable_loopback_port() {
        let port = free_port().unwrap();
        assert!(port > 0);
        // It was free a moment ago, so we can bind it again here.
        let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        assert_eq!(l.local_addr().unwrap().port(), port);
    }

    #[test]
    fn fleet_mux_base_and_cwd_read_env_overrides() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_MUX_DIR", "/custom/mux");
            assert_eq!(fleet_mux_base(), PathBuf::from("/custom/mux"));
        }
        {
            let _g = EnvGuard::unset("FLEET_MUX_DIR");
            let _h = EnvGuard::set("HOME", "/home/dora");
            assert_eq!(fleet_mux_base(), PathBuf::from("/home/dora/.fleet/mux"));
        }
        {
            let _g = EnvGuard::set("FLEET_SPAWN_CWD", "/custom/cwd");
            assert_eq!(
                fleet_spawn_cwd(Path::new("/home/dora/.fleet/mux")),
                PathBuf::from("/custom/cwd")
            );
        }
    }

    #[test]
    fn log_files_create_named_log_under_mux_base() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-log-files");
        let _ = std::fs::remove_dir_all(&dir);
        let _g = EnvGuard::set("FLEET_MUX_DIR", dir.to_str().unwrap());

        let (_out, _err) = log_files("cs-server-7");
        // The log file is materialized (out + err clone the same handle).
        assert!(dir.join("cs-server-7.log").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_legacy_spawn_state_runs_against_resolved_mux_base() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-clear-legacy-public");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(spawn_manifest_path(&dir), "{}").unwrap();
        let _g = EnvGuard::set("FLEET_MUX_DIR", dir.to_str().unwrap());

        // The public entry point resolves the base from env and clears it.
        clear_legacy_spawn_state();
        assert!(!spawn_manifest_path(&dir).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn supervisor_rename_updates_only_matching_server() {
        let sup = test_supervisor();
        push_server(&sup, "server-a");
        push_server(&sup, "server-b");

        assert!(sup.rename("server-b", "Docs"));
        assert!(!sup.rename("missing", "Nope"));
        let servers = sup.servers();
        assert_eq!(
            servers.iter().find(|s| s.id == "server-a").unwrap().label,
            "server-a"
        );
        assert_eq!(
            servers.iter().find(|s| s.id == "server-b").unwrap().label,
            "Docs"
        );
    }

    #[test]
    fn servers_returns_recorded_servers_in_order() {
        let sup = test_supervisor();
        push_server(&sup, "server-2");
        push_server(&sup, "server-1");
        // No children are tracked, so prune leaves the list intact.
        let ids: Vec<String> = sup.servers().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["server-2", "server-1"]);
    }

    #[cfg(unix)]
    #[test]
    fn servers_prunes_a_session_whose_primary_child_exited() {
        let sup = test_supervisor();
        // A child that exits immediately is the "primary" (last) child of the group.
        let child = Command::new("true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        push_server(&sup, "server-dead");
        sup.children
            .lock()
            .unwrap()
            .insert("server-dead".into(), vec![child]);
        // Give it a moment to actually exit so try_wait sees the Exited status.
        let pruned = eventually(std::time::Duration::from_secs(2), || {
            sup.servers().is_empty()
        });
        assert!(pruned, "an exited session should be pruned from the rail");
        assert!(!sup.children.lock().unwrap().contains_key("server-dead"));
    }

    #[test]
    fn servers_prunes_a_session_with_no_tracked_child() {
        let sup = test_supervisor();
        push_server(&sup, "server-empty");
        // An empty child group ⇒ primary_child_exited is Unavailable ⇒ pruned.
        sup.children
            .lock()
            .unwrap()
            .insert("server-empty".into(), Vec::new());
        assert!(sup.servers().is_empty());
        assert!(!sup.children.lock().unwrap().contains_key("server-empty"));
    }

    #[test]
    fn prune_tolerates_a_poisoned_server_list_lock() {
        let sup = test_supervisor();
        push_server(&sup, "server-empty");
        // This empty-child session is "dead", so prune wants to remove it.
        sup.children
            .lock()
            .unwrap()
            .insert("server-empty".into(), Vec::new());
        // Poison the servers lock so prune's retain branch hits its warn arm.
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = sup.servers.lock().unwrap();
                    panic!("poison the servers lock mid-prune");
                })
                .join()
        });
        assert!(result.is_err());
        // servers() runs prune (which can't retain) then returns empty (poisoned).
        assert!(sup.servers().is_empty());
        // The dead child was still dropped from the children map.
        assert!(!sup.children.lock().unwrap().contains_key("server-empty"));
    }

    #[test]
    fn close_returns_false_for_unknown_server() {
        let sup = test_supervisor();
        assert!(!sup.close("never-spawned"));
    }

    #[test]
    fn close_removes_a_container_mode_server_record() {
        let sup = test_supervisor();
        push_server(&sup, "server-ctr");
        sup.containers
            .lock()
            .unwrap()
            .insert("server-ctr".into(), "fleet-server-ctr".into());
        // Closing a container-mode server drops its record and reports success.
        // (The `docker rm` itself is best-effort glue.)
        assert!(sup.close("server-ctr"));
        assert!(sup.servers().is_empty());
        assert!(!sup.containers.lock().unwrap().contains_key("server-ctr"));
    }

    #[test]
    fn servers_tolerates_a_poisoned_children_lock() {
        let sup = test_supervisor();
        push_server(&sup, "server-1");
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = sup.children.lock().unwrap();
                    panic!("poison the children lock");
                })
                .join()
        });
        assert!(result.is_err());
        assert!(sup.children.is_poisoned());
        // prune skips (children lock poisoned), and servers still clones the list.
        let ids: Vec<String> = sup.servers().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["server-1".to_string()]);
    }

    #[test]
    fn servers_tolerates_a_poisoned_list_lock() {
        let sup = test_supervisor();
        push_server(&sup, "server-1");
        // Poison the servers lock from a panicking thread, then confirm the
        // defensive branch returns an empty list rather than propagating.
        let handle = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = sup.servers.lock().unwrap();
                    panic!("poison the servers lock");
                })
                .join()
        });
        assert!(handle.is_err());
        assert!(sup.servers.is_poisoned());
        assert!(sup.servers().is_empty());
        assert!(!sup.rename("server-1", "x"));
    }

    #[test]
    fn find_real_claude_prefers_explicit_bin_override() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-find-claude-override");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("my-claude");
        std::fs::write(&real, "").unwrap();

        let _g = EnvGuard::set("FLEET_CLAUDE_BIN", real.to_str().unwrap());
        // Empty PATH and a shim dir — the override wins regardless.
        assert_eq!(
            find_real_claude(&dir.join("shim"), &OsString::new()),
            Some(real)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_real_claude_skips_shim_dir_and_returns_none_when_absent() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_CLAUDE_BIN");
        let dir = temp_test_dir("fleet-find-claude-shim-skip");
        let shim_dir = dir.join("shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&shim_dir).unwrap();
        // A `claude` living only in the shim dir must be ignored.
        std::fs::write(shim_dir.join("claude"), "").unwrap();
        let path = std::env::join_paths([&shim_dir]).unwrap();
        assert_eq!(find_real_claude(&shim_dir, &path), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn server_label_and_launch_id_are_well_formed() {
        assert_eq!(server_label(7), "server-7");
        let id = launch_id();
        // `<pid hex>-<nanos hex>` — both halves present and hex.
        let (pid, nanos) = id.split_once('-').expect("launch id has two halves");
        assert!(!pid.is_empty() && pid.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!nanos.is_empty() && nanos.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn nonblank_env_treats_blank_and_unset_alike() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_TEST_NONBLANK", "value");
            assert_eq!(nonblank_env("FLEET_TEST_NONBLANK"), Some("value".into()));
        }
        {
            let _g = EnvGuard::set("FLEET_TEST_NONBLANK", "   ");
            assert_eq!(nonblank_env("FLEET_TEST_NONBLANK"), None);
        }
        {
            let _g = EnvGuard::unset("FLEET_TEST_NONBLANK");
            assert_eq!(nonblank_env("FLEET_TEST_NONBLANK"), None);
        }
    }

    #[test]
    fn spawn_mode_reads_env_knob() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_SPAWN_MODE", "container");
            assert!(matches!(spawn_mode(), SpawnMode::Container));
        }
        {
            let _g = EnvGuard::set("FLEET_SPAWN_MODE", "ssh");
            assert!(matches!(spawn_mode(), SpawnMode::Ssh));
        }
        {
            let _g = EnvGuard::set("FLEET_SPAWN_MODE", "local");
            assert!(matches!(spawn_mode(), SpawnMode::Local));
        }
        {
            let _g = EnvGuard::unset("FLEET_SPAWN_MODE");
            assert!(matches!(spawn_mode(), SpawnMode::Local));
        }
    }

    // The pure routing decision of `spawn_server_with_options` → `spawn_with`.
    // The live spawn (a real code-server/docker/ssh tree) is CI-smoke-only, but
    // its dispatch — which path each (request mode, env mode) lands on — is fully
    // deterministic and asserted here.
    #[test]
    fn spawn_route_dispatches_request_mode_over_env_mode() {
        use super::{resolve_spawn_route, SpawnMode, SpawnRoute};

        // An explicit request mode wins over the env knob.
        assert_eq!(
            resolve_spawn_route(Some("container"), SpawnMode::Local),
            SpawnRoute::Container
        );
        assert_eq!(
            resolve_spawn_route(Some("container"), SpawnMode::Ssh),
            SpawnRoute::Container
        );
        // An explicit `local` request forces the explicit-folder local path even
        // when the env knob says container/ssh.
        assert_eq!(
            resolve_spawn_route(Some("local"), SpawnMode::Container),
            SpawnRoute::LocalExplicit
        );
        assert_eq!(
            resolve_spawn_route(Some("local"), SpawnMode::Ssh),
            SpawnRoute::LocalExplicit
        );

        // No request mode ⇒ the env knob decides.
        assert_eq!(
            resolve_spawn_route(None, SpawnMode::Container),
            SpawnRoute::Container
        );
        assert_eq!(resolve_spawn_route(None, SpawnMode::Ssh), SpawnRoute::Ssh);
        assert_eq!(resolve_spawn_route(None, SpawnMode::Local), SpawnRoute::Local);

        // An unrecognized request mode falls through to the env knob (not an error).
        assert_eq!(
            resolve_spawn_route(Some("bogus"), SpawnMode::Local),
            SpawnRoute::Local
        );
        assert_eq!(
            resolve_spawn_route(Some("bogus"), SpawnMode::Container),
            SpawnRoute::Container
        );
    }

    // `SpawnRequest` is what the `spawn_server_with_options` command deserializes
    // from the frontend invoke; its camelCase parsing + the trim/non-blank
    // filtering `spawn_with` applies to `mode` are part of the command's contract.
    #[test]
    fn spawn_request_parses_camel_case_and_trims_mode() {
        let req: SpawnRequest =
            serde_json::from_value(serde_json::json!({ "mode": "container", "folder": "~/code" }))
                .unwrap();
        assert_eq!(req.mode.as_deref(), Some("container"));
        assert_eq!(req.folder.as_deref(), Some("~/code"));

        // Absent fields default to None (the home-folder create path).
        let empty: SpawnRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.mode.is_none());
        assert!(empty.folder.is_none());

        // A blank/whitespace mode is filtered to None by `spawn_with`, so it routes
        // by the env knob rather than as an explicit mode.
        let blank = Some("   ");
        let trimmed = blank.map(str::trim).filter(|m| !m.is_empty());
        assert_eq!(trimmed, None);
    }

    #[test]
    fn docker_bin_honors_override_then_falls_back() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_DOCKER_BIN", "/opt/docker/bin/docker");
            assert_eq!(docker_bin(), PathBuf::from("/opt/docker/bin/docker"));
        }
        {
            let _g = EnvGuard::set("FLEET_DOCKER_BIN", "   ");
            // Blank override is ignored; PATH search falls back to bare "docker".
            assert_eq!(docker_bin().file_name().unwrap(), "docker");
        }
    }

    #[test]
    fn reporter_bin_honors_override() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_REPORTER_BIN", "/custom/fleet-reporter");
            assert_eq!(reporter_bin(), PathBuf::from("/custom/fleet-reporter"));
        }
        {
            let _g = EnvGuard::set("FLEET_REPORTER_BIN", "  ");
            // Blank override ignored; resolves to a bare/bundled name ending in the bin.
            assert_eq!(reporter_bin().file_name().unwrap(), "fleet-reporter");
        }
    }

    #[test]
    fn editor_bin_prefers_override_and_ignores_blank() {
        let _lock = lock_env();
        let empty_path = OsString::new();
        {
            let _g = EnvGuard::set("FLEET_EDITOR_BIN", "/custom/code");
            assert_eq!(editor_bin(&empty_path), PathBuf::from("/custom/code"));
        }
        {
            // Blank override is trimmed away and ignored, so resolution falls
            // through to the PATH/well-known-install search. The final fallback
            // (when nothing resolves) is the bare `code` name; on a machine with
            // a real VS Code install an absolute install path may resolve first,
            // but in every case the resolved binary is named `code`.
            let _g = EnvGuard::set("FLEET_EDITOR_BIN", "  ");
            let _local = EnvGuard::unset("LOCALAPPDATA");
            let _pf = EnvGuard::unset("ProgramFiles");
            assert_eq!(editor_bin(&empty_path).file_name().unwrap(), "code");
        }
    }

    #[test]
    fn editor_bin_searches_windows_install_locations() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_EDITOR_BIN");
        let dir = temp_test_dir("fleet-editor-windows-install");
        let _ = std::fs::remove_dir_all(&dir);
        // Two Windows-style install roots (per-user LOCALAPPDATA + machine-wide
        // ProgramFiles); both are appended to the candidate set before resolution.
        let local = dir.join("local");
        let pf = dir.join("pf");
        let user_code = local
            .join("Programs")
            .join("Microsoft VS Code")
            .join("bin")
            .join("code");
        let machine_code = pf.join("Microsoft VS Code").join("bin").join("code");
        std::fs::create_dir_all(user_code.parent().unwrap()).unwrap();
        std::fs::create_dir_all(machine_code.parent().unwrap()).unwrap();
        std::fs::write(&user_code, "").unwrap();
        std::fs::write(&machine_code, "").unwrap();
        let _la = EnvGuard::set("LOCALAPPDATA", local.to_str().unwrap());
        let _pf = EnvGuard::set("ProgramFiles", pf.to_str().unwrap());

        // The resolved editor is always named `code`. On a host with a real
        // /Applications VS Code that absolute candidate may win the find; on a
        // host without one, the LOCALAPPDATA candidate resolves. Either way the
        // Windows-install candidate branches execute.
        let resolved = editor_bin(&OsString::new());
        assert_eq!(resolved.file_name().unwrap(), "code");
        assert!(resolved.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editor_bin_finds_code_on_path() {
        let _lock = lock_env();
        let _g = EnvGuard::unset("FLEET_EDITOR_BIN");
        let dir = temp_test_dir("fleet-editor-on-path");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let code = dir.join("code");
        std::fs::write(&code, "").unwrap();
        let path = std::env::join_paths([&dir]).unwrap();

        assert_eq!(editor_bin(&path), code);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_code_server_bin_passes_through_code_server_path() {
        // An editor path that is already a code-server binary returns itself
        // (no env lookup, no Command spawn).
        assert_eq!(
            local_code_server_bin(Path::new("/tmp/serve/bin/code-server")),
            Some(PathBuf::from("/tmp/serve/bin/code-server"))
        );
    }

    #[test]
    fn local_code_server_bin_honors_explicit_override() {
        let _lock = lock_env();
        {
            let _g = EnvGuard::set("FLEET_CODE_SERVER_BIN", "/custom/code-server");
            // The editor itself is the plain `code` CLI (not code-server), so the
            // override is consulted before the home/serve-web bootstrap.
            assert_eq!(
                local_code_server_bin(Path::new("/usr/bin/code")),
                Some(PathBuf::from("/custom/code-server"))
            );
        }
        {
            // A blank override is ignored (falls through to home/None).
            let _g = EnvGuard::set("FLEET_CODE_SERVER_BIN", "  ");
            let _h = EnvGuard::set(
                "HOME",
                temp_test_dir("fleet-cs-blank-home").to_str().unwrap(),
            );
            let _u = EnvGuard::unset("USERPROFILE");
            assert_eq!(local_code_server_bin(Path::new("/nonexistent/code")), None);
        }
    }

    #[test]
    fn is_code_server_bin_matches_stem_only() {
        assert!(is_code_server_bin(Path::new("/x/code-server")));
        assert!(is_code_server_bin(Path::new("/x/code-server.cmd")));
        assert!(is_code_server_bin(Path::new("/x/code-server.exe")));
        assert!(!is_code_server_bin(Path::new("/x/code")));
        assert!(!is_code_server_bin(Path::new("/x/code-tunnel")));
    }

    #[test]
    fn home_dir_prefers_home_then_userprofile() {
        let _lock = lock_env();
        {
            let _h = EnvGuard::set("HOME", "/home/alice");
            assert_eq!(home_dir(), Some(PathBuf::from("/home/alice")));
        }
        {
            let _h = EnvGuard::unset("HOME");
            let _u = EnvGuard::set("USERPROFILE", "C:/Users/Alice");
            assert_eq!(home_dir(), Some(PathBuf::from("C:/Users/Alice")));
        }
        {
            let _h = EnvGuard::set("HOME", "");
            let _u = EnvGuard::unset("USERPROFILE");
            assert_eq!(home_dir(), None);
        }
    }

    #[test]
    fn default_spawn_folder_falls_back_to_home() {
        let _lock = lock_env();
        let _h = EnvGuard::set("HOME", "/home/bob");
        assert_eq!(default_spawn_folder(), PathBuf::from("/home/bob"));
    }

    #[test]
    fn expand_user_path_expands_tilde_only_at_prefix() {
        let _lock = lock_env();
        let _h = EnvGuard::set("HOME", "/home/carol");
        assert_eq!(expand_user_path("~"), PathBuf::from("/home/carol"));
        assert_eq!(
            expand_user_path("~/projects/api"),
            PathBuf::from("/home/carol/projects/api")
        );
        assert_eq!(expand_user_path("/abs/path"), PathBuf::from("/abs/path"));
        // A tilde not at the start is left literal.
        assert_eq!(expand_user_path("rel/~weird"), PathBuf::from("rel/~weird"));
    }

    #[test]
    fn read_json_object_or_empty_handles_missing_invalid_and_nonobject() {
        let dir = temp_test_dir("fleet-read-json-object");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Missing file ⇒ empty object.
        assert_eq!(
            read_json_object_or_empty(&dir.join("absent.json")),
            serde_json::json!({})
        );

        // Valid object ⇒ preserved.
        let obj = dir.join("obj.json");
        std::fs::write(&obj, br#"{"a":1}"#).unwrap();
        assert_eq!(read_json_object_or_empty(&obj), serde_json::json!({"a":1}));

        // Valid JSON but not an object (an array) ⇒ replaced with empty object.
        let arr = dir.join("arr.json");
        std::fs::write(&arr, br#"[1,2,3]"#).unwrap();
        assert_eq!(read_json_object_or_empty(&arr), serde_json::json!({}));

        // Invalid JSON ⇒ replaced with empty object.
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"{not json").unwrap();
        assert_eq!(read_json_object_or_empty(&bad), serde_json::json!({}));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_search_dirs_covers_macos_linux_windows_layouts() {
        let dirs = bundle_search_dirs(Path::new("/app/Contents/MacOS"));
        assert!(dirs.contains(&PathBuf::from("/app/Contents/MacOS")));
        assert!(dirs.contains(&PathBuf::from("/app/Contents/Resources")));
        assert!(dirs.contains(&PathBuf::from("/app/Contents/lib/fleet")));
        assert!(dirs.contains(&PathBuf::from("/app/Contents/lib/Fleet")));
        assert!(dirs.contains(&PathBuf::from("/app/Contents/lib/fleet-host")));

        // A root with no parent still yields at least itself.
        let root = bundle_search_dirs(Path::new("/"));
        assert!(root.contains(&PathBuf::from("/")));
    }

    #[test]
    fn first_vsix_in_finds_a_vsix_or_none() {
        let dir = temp_test_dir("fleet-first-vsix");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No vsix yet.
        assert_eq!(first_vsix_in(&dir), None);
        // A missing directory is also None.
        assert_eq!(first_vsix_in(&dir.join("absent")), None);

        let vsix = dir.join("fleet-bridge-9.9.9.vsix");
        std::fs::write(&vsix, "").unwrap();
        assert_eq!(first_vsix_in(&dir), Some(vsix));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_fleet_bridge_vsix_uses_explicit_override_file() {
        let _lock = lock_env();
        let dir = temp_test_dir("fleet-vsix-override");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vsix = dir.join("custom.vsix");
        std::fs::write(&vsix, "").unwrap();

        let _g = EnvGuard::set("FLEET_BRIDGE_VSIX", vsix.to_str().unwrap());
        assert_eq!(find_fleet_bridge_vsix(), Some(vsix));

        // A non-file override (a directory) is ignored — it must not be returned
        // verbatim. Whether the subsequent bundle search then finds a staged VSIX
        // is environment-dependent (present in-repo locally, absent next to the
        // test binary in CI), so assert only the deterministic contract: the
        // invalid override is rejected, not used.
        let _g2 = EnvGuard::set("FLEET_BRIDGE_VSIX", dir.to_str().unwrap());
        assert_ne!(find_fleet_bridge_vsix(), Some(dir.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_server_counter_and_manifest_paths_live_under_base() {
        assert_eq!(
            next_server_counter_path(Path::new("/b")),
            PathBuf::from("/b/server-counter")
        );
        assert_eq!(
            spawn_manifest_path(Path::new("/b")),
            PathBuf::from("/b/servers.json")
        );
    }

    #[test]
    fn primary_child_exited_reports_alive_then_exited() {
        // A short-lived `true` exits promptly; before reap it's Alive, after Exited.
        let mut group = vec![Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()];
        assert_eq!(primary_child_exited(&mut group), ChildHealth::Alive);
        for child in &mut group {
            let _ = child.kill();
            let _ = child.wait();
        }

        let mut done = vec![Command::new("true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()];
        // Reap so try_wait sees the exit deterministically.
        done[0].wait().unwrap();
        assert!(matches!(
            primary_child_exited(&mut done),
            ChildHealth::Exited { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn claude_hooks_settings_relays_every_lifecycle_event_to_socket() {
        let settings = claude_hooks_settings(Path::new("/tmp/fleet-reporter.sock"));
        let hooks = &settings["hooks"];
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ] {
            let cmd = hooks[event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_else(|| panic!("missing command for {event}"));
            assert!(cmd.contains("nc -U '/tmp/fleet-reporter.sock'"));
            assert!(cmd.contains("|| true"));
        }
        // Matched events (Pre/PostToolUse) carry a "*" matcher; plain ones don't.
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "*");
        assert!(hooks["SessionStart"][0].get("matcher").is_none());
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

    // Test scaffolding: the timeout-`panic!` and pid-overflow arms are defensive
    // guards that don't fire on the green path, so they're excluded from the bar.
    #[cfg(unix)]
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    #[cfg_attr(coverage_nightly, coverage(off))]
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

    // Test scaffolding: the final post-timeout `pred()` only runs if the loop
    // never observed the condition, which doesn't happen on the green path.
    #[cfg(unix)]
    #[cfg_attr(coverage_nightly, coverage(off))]
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

//! Server supervisor — Fleet **spawns** new VS Code web servers (and closes ones it
//! spawned). A spawned server is launched with the phone-home env, so it dials
//! Fleet's bridge and appears in the rail on its own — Fleet never pulls it.
//!
//! Launches Microsoft's official **VS Code** web server (`code serve-web`) with a SHARED
//! `fleet-bridge` extension install and a PER-SERVER `--server-data-dir` (so concurrent
//! servers don't collide). `serve-web` reads extensions from `<server-data-dir>/extensions`,
//! so Fleet installs the bridge there before launch. (Was code-server; switched to the
//! official server for the full MS Marketplace + clean aarch64-darwin packaging — fine for
//! personal / own-hardware use; see NORTH_STAR on the licensing line.)
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
//! Env knobs: `FLEET_EDITOR_BIN`, `FLEET_BRIDGE_VSIX`,
//! `FLEET_REPORTER_BIN`, `FLEET_CLAUDE_BIN`.
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
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

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
    bridge_port: u16,
    hub_url: String,
}

impl ServerSupervisor {
    pub fn new(bridge_port: u16, hub_url: String) -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            containers: Mutex::new(HashMap::new()),
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

    /// Spawn a new server. Routes to the container path when `FLEET_SPAWN_MODE=container`,
    /// else the default local-process path. Both return a [`Server`] that Fleet adds
    /// to the rail immediately; the spawned env phones home to the bridge on its own.
    pub fn spawn(&self) -> std::io::Result<crate::mux::Server> {
        match spawn_mode() {
            SpawnMode::Container => self.spawn_container(),
            SpawnMode::Ssh => self.spawn_ssh(),
            SpawnMode::Local => self.spawn_local(),
        }
    }

    /// Spawn a new VS Code web server (+ its reporter + claude shim) and record it.
    /// Returns its [`Server`]; Fleet adds it to the rail immediately.
    fn spawn_local(&self) -> std::io::Result<crate::mux::Server> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("server-{n}");
        let port = free_port()?;

        let base = std::env::temp_dir().join("fleet-mux");
        let _ = std::fs::create_dir_all(&base);
        let folder = base.join(format!("ws-{id}"));
        // The workspace is either a fresh clone of FLEET_SPAWN_REPO (repo-as-workspace,
        // the north-star ergonomic) or an empty folder with a hint file.
        match std::env::var("FLEET_SPAWN_REPO") {
            Ok(spec) if !spec.trim().is_empty() => {
                let url = resolve_repo(&spec);
                let ok = Command::new("git")
                    .args(["clone", "--depth", "1", &url, &folder.to_string_lossy()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok {
                    tracing::info!(%id, %url, "cloned repo into workspace");
                } else {
                    tracing::warn!(%id, %url, "git clone failed; using an empty workspace");
                    let _ = std::fs::create_dir_all(&folder);
                }
            }
            _ => {
                let _ = std::fs::create_dir_all(&folder);
                let _ = std::fs::write(
                    folder.join(format!("{id}.md")),
                    format!("# {id}\n\nSpawned by Fleet at port {port}.\nRun `claude` in the terminal — this tab will light up.\n"),
                );
            }
        }

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

        // --- the editor: VS Code serve-web --------------------------------------
        let user_data = base.join(format!("cs-userdata-{id}"));
        let editor = std::env::var("FLEET_EDITOR_BIN").unwrap_or_else(|_| "code".into());
        install_fleet_bridge(&editor, &user_data)?;
        let url = format!("http://127.0.0.1:{port}/?folder={}", folder.display());
        let (cs_out, cs_err) = log_files(&format!("cs-{id}"));

        // Prepend the shim dir so the terminal's `claude` is Fleet-wrapped.
        let path = match (&shim_path, std::env::var("PATH")) {
            (Some(_), Ok(p)) => format!("{}:{}", shim_dir.display(), p),
            (_, Ok(p)) => p,
            _ => shim_dir.display().to_string(),
        };

        // The SAME serve-web flags + Fleet phone-home env the SSH path uses, so a
        // local and a remote server are launched identically (only the location differs).
        let args = serve_web_args(
            "127.0.0.1",
            port,
            &user_data.to_string_lossy(),
            &folder.to_string_lossy(),
        );
        let env = fleet_env(
            &reporter_socket.to_string_lossy(),
            &id,
            &format!("ws://127.0.0.1:{}", self.bridge_port),
            &url,
        );
        let mut cmd = Command::new(&editor);
        cmd.args(&args).env("PATH", path);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let child = cmd.stdout(cs_out).stderr(cs_err).spawn()?;
        children.push(child);

        tracing::info!(%id, port, "spawned VS Code serve-web + reporter");
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

    /// Spawn a new server as a **container** (`docker run` the `fleet-env` image) and
    /// record it. The image bakes in code-server + reporter + bridge; it dials back to
    /// Fleet's bridge using `FLEET_HOST_ADDR` + `FLEET_BRIDGE_PORT`, so it appears in
    /// the rail on its own. We publish a free host port to the container's `:8080` and
    /// inspect the container for the reachable URL.
    ///
    /// NOTE: for the container to reach Fleet's bridge, Fleet must bind it on all
    /// interfaces — launch with `FLEET_BRIDGE_ADDR=0.0.0.0` (see `bridge.rs`).
    fn spawn_container(&self) -> std::io::Result<crate::mux::Server> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("server-{n}");
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
                &format!("FLEET_SERVER_LABEL={id}"),
                "-e",
                &format!("FLEET_HOST_ADDR={host_addr}"),
                "-e",
                &format!("FLEET_BRIDGE_PORT={}", self.bridge_port),
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
            label: id.clone(),
            url,
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
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("server-{n}");

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
            &format!("ws://127.0.0.1:{r_bridge}"),
            &url,
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
        let child = Command::new("ssh")
            .args([
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
            .stderr(err)
            .spawn()?;

        tracing::info!(%id, %target, local_editor, r_cs, "deployed code-server over ssh");
        let server = crate::mux::Server {
            id: id.clone(),
            label: format!("{id} @ {target}"),
            url,
        };
        self.children
            .lock()
            .unwrap()
            .insert(id.clone(), vec![child]);
        self.servers.lock().unwrap().push(server.clone());
        Ok(server)
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
fn docker_bin() -> String {
    std::env::var("FLEET_DOCKER_BIN").unwrap_or_else(|_| "docker".into())
}

/// Inspect a running container for the host-reachable editor URL: the host port that
/// docker bound to the container's `8080/tcp`. Returns `None` if inspection fails.
fn inspect_url(docker: &str, name: &str) -> Option<String> {
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
fn install_fleet_bridge(editor: &str, server_data: &Path) -> std::io::Result<()> {
    let extensions_dir = fleet_bridge_extensions_dir(server_data);
    if fleet_bridge_installed(&extensions_dir) {
        return Ok(());
    }

    std::fs::create_dir_all(&extensions_dir)?;
    let vsix = find_fleet_bridge_vsix().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "fleet-bridge VSIX not found (set FLEET_BRIDGE_VSIX or bundle fleet-bridge.vsix)",
        )
    })?;

    let out = Command::new(editor)
        .arg("--install-extension")
        .arg(&vsix)
        .arg("--extensions-dir")
        .arg(&extensions_dir)
        .arg("--force")
        .output()?;
    if out.status.success() {
        tracing::info!(extensions_dir = %extensions_dir.display(), "installed fleet-bridge extension");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(std::io::Error::other(format!(
        "failed to install fleet-bridge extension with `{editor}`: {detail}"
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
            dirs.push(exe_dir.to_path_buf());
            if let Some(contents_dir) = exe_dir.parent() {
                dirs.push(contents_dir.join("Resources"));
            }
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

/// The `code serve-web` invocation Fleet launches — Microsoft's OFFICIAL VS Code web
/// server. Chosen over code-server for the full MS Marketplace + clean aarch64-darwin
/// packaging; fine for personal / own-hardware use (see NORTH_STAR + REQUIREMENTS for the
/// licensing line — it stops being clean only when hosting editors for OTHERS). Identical
/// for local and remote spawns, so a server behaves the same wherever it runs.
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

/// The Fleet phone-home env every spawned editor carries: the bridge dials
/// `FLEET_BRIDGE_URL`, the reporter writes `FLEET_REPORTER_SOCKET`, and the rail tab is
/// keyed by `FLEET_SERVER_ID`/`FLEET_SESSION_ID`. Identical keys for local and remote;
/// only the values (paths/ports) differ by location.
fn fleet_env(
    reporter_socket: &str,
    id: &str,
    bridge_url: &str,
    server_url: &str,
) -> Vec<(String, String)> {
    vec![
        ("FLEET_REPORTER_SOCKET".into(), reporter_socket.into()),
        ("FLEET_SESSION_ID".into(), id.into()),
        ("FLEET_BRIDGE_URL".into(), bridge_url.into()),
        ("FLEET_SERVER_ID".into(), id.into()),
        ("FLEET_SERVER_LABEL".into(), id.into()),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

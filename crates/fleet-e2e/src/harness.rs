//! The composition harness — spins the real Hub, the real faces, and routes the
//! real adapters' output into the Hub exactly as a real reporter would.
//!
//! Nothing here re-implements product logic. Every type below either *is* a real
//! component (`fleet_hub::HubState`, `fleet_host_core::InboxModel`) or is a thin
//! adapter that moves bytes between the real components (a WebSocket subscriber,
//! a `ReporterCommand` → Hub-ingest translator, a `fleet ls --once` runner).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use fleet_host_core::InboxModel;
use fleet_hub::server::{run_ws_listener, HubState};
use fleet_hub::wire::SeqStamp;
use fleet_protocol::{
    AgentRun, Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session,
    State,
};
use fleet_reporter::ReporterCommand;

/// An in-process Fleet Hub: the **real** [`HubState`] (merge engine + SQLite event
/// log) served over the **real** [`fleet_hub::server`] WebSocket listener on a
/// loopback ephemeral port.
///
/// Faces (`FaceClient`, `fleet ls`) and reporters (driven via [`apply_commands`])
/// all talk to this over a real WebSocket — there is no stubbing between them.
pub struct TestHub {
    state: HubState,
    addr: SocketAddr,
    /// A per-durable-id monotonic seq counter so each run delta is stamped just
    /// like a real S6 reporter (exercising the Hub's reclaim/identity path).
    seqs: HashMap<String, u64>,
}

impl TestHub {
    /// Start a Hub backed by an **in-memory** event log (no restart durability).
    /// The common case for the DoD items that don't test persistence.
    pub async fn start() -> Result<Self> {
        Self::start_with_state(HubState::new()).await
    }

    /// Start a Hub backed by a **durable on-disk** event log at `db_path` — used by
    /// the restart-persistence item (§21.8) to prove state survives a Hub restart.
    pub async fn start_with_db(db_path: impl AsRef<Path>) -> Result<Self> {
        let state = HubState::with_db(db_path).context("open durable Hub event log")?;
        Self::start_with_state(state).await
    }

    async fn start_with_state(state: HubState) -> Result<Self> {
        // Bind the real WS listener on loopback:0 (OS-assigned port — portable on
        // macOS + Linux, no fixed-port collisions across parallel tests).
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (addr, fut) = run_ws_listener(state.clone(), bind)
            .await
            .context("bind Hub WebSocket listener")?;
        tokio::spawn(fut);
        Ok(TestHub {
            state,
            addr,
            seqs: HashMap::new(),
        })
    }

    /// The `ws://` URL a face connects to.
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// The bound socket address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A clone of the underlying real [`HubState`] (e.g. to take a direct snapshot
    /// in an assertion, or to run a GC pass).
    pub fn state(&self) -> HubState {
        self.state.clone()
    }

    /// Register a session shell on the Hub (what a reporter sends first on connect).
    /// Returns the session id for convenience.
    pub async fn register_session(&self, session: Session) -> String {
        let id = session.session_id.clone();
        self.state.ingest_session_upsert(session).await;
        id
    }

    /// Apply one [`ReporterCommand`] — the unit a real adapter emits — to the Hub,
    /// stamping run upserts with a monotonic per-durable-id `seq` so the Hub's
    /// real reclaim/identity gate is exercised (S6). Returns `true` if the command
    /// produced a Hub mutation.
    pub async fn apply_command(&mut self, session_id: &str, cmd: ReporterCommand) -> bool {
        match cmd {
            ReporterCommand::UpsertSession(s) => {
                self.state.ingest_session_upsert(s).await;
                true
            }
            ReporterCommand::UpsertRun(run) => {
                let durable = run.native_id.clone();
                let seq = {
                    let e = self.seqs.entry(durable.clone()).or_insert(0);
                    *e += 1;
                    *e
                };
                let stamp = SeqStamp::new(durable, 0, seq);
                self.state
                    .ingest_run_upsert_stamped(session_id, run, Some(stamp))
                    .await;
                true
            }
            // Pure liveness signals are not Hub deltas (they only refresh the
            // reporter's own timeout window) — nothing to apply to the Hub.
            ReporterCommand::Liveness { .. } => false,
            ReporterCommand::ConfirmExit { run_id, reason } => {
                // A confirmed exit marks the run dead (the reporter would send a
                // dead run upsert). We model it as a direct run remove for the
                // reap path; the adapters here emit dead via UpsertRun instead.
                let _ = reason;
                self.state
                    .ingest_run_upsert_stamped(session_id, dead_run(&run_id), None)
                    .await;
                true
            }
            ReporterCommand::Shutdown => false,
        }
    }
}

/// Apply a batch of [`ReporterCommand`]s (as a real adapter returns) to the Hub.
pub async fn apply_commands(
    hub: &mut TestHub,
    session_id: &str,
    cmds: impl IntoIterator<Item = ReporterCommand>,
) {
    for cmd in cmds {
        hub.apply_command(session_id, cmd).await;
    }
}

/// Minimal dead run for the confirm-exit path.
fn dead_run(run_id: &str) -> AgentRun {
    AgentRun::new(
        run_id,
        fleet_protocol::AgentKind::Other,
        run_id,
        "/",
        State::Dead,
        fleet_protocol::Confidence::High,
        "1970-01-01T00:00:00Z",
    )
}

/// A real WebSocket **face**: subscribes to the Hub and folds its
/// `fleet.snapshot` + delta stream into a [`fleet_host_core::InboxModel`] — the
/// host (sidebar) view-model. This is the actual host face reducer reading the
/// actual wire protocol; nothing is faked.
pub struct FaceClient {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    model: InboxModel,
}

impl FaceClient {
    /// Connect to `ws_url`, send `subscribe`, and consume the initial
    /// `fleet.snapshot` so the model reflects current Hub state immediately.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (mut ws, _resp) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("face connect failed: {ws_url}"))?;
        ws.send(Message::Text(r#"{"type":"subscribe"}"#.into()))
            .await
            .context("send subscribe")?;
        let mut face = FaceClient {
            ws,
            model: InboxModel::new(),
        };
        // The Hub replies with the `fleet.snapshot` first; fold exactly that frame
        // so the model reflects current Hub state before the caller asserts.
        let first = face.next_event().await?;
        debug_assert_eq!(first, "fleet.snapshot", "Hub sends the snapshot first");
        Ok(face)
    }

    /// The current host view-model.
    pub fn model(&self) -> &InboxModel {
        &self.model
    }

    /// Pump and fold exactly one inbound event frame (blocking until one arrives).
    /// Returns the event's wire type name.
    pub async fn next_event(&mut self) -> Result<String> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(txt))) => {
                    let ev: Event = serde_json::from_str(&txt).context("decode event")?;
                    let name = ev.type_name().to_string();
                    self.model.apply(ev);
                    return Ok(name);
                }
                Some(Ok(Message::Binary(bin))) => {
                    let ev: Event = serde_json::from_slice(&bin).context("decode event")?;
                    let name = ev.type_name().to_string();
                    self.model.apply(ev);
                    return Ok(name);
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => {
                    anyhow::bail!("Hub closed the face connection")
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(anyhow::anyhow!("face ws error: {e}")),
            }
        }
    }

    /// Pump-and-fold inbound frames until `pred(model)` holds or the deadline
    /// elapses. Returns `true` if the predicate was satisfied in time. This is the
    /// "<2s" assertion primitive (§21.1/§21.4) over the **real** wire + reducer.
    pub async fn wait_until(
        &mut self,
        within: Duration,
        pred: impl Fn(&InboxModel) -> bool,
    ) -> bool {
        if pred(&self.model) {
            return true;
        }
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return pred(&self.model);
            }
            match tokio::time::timeout(remaining, self.next_event()).await {
                Ok(Ok(_)) => {
                    if pred(&self.model) {
                        return true;
                    }
                }
                Ok(Err(_)) | Err(_) => return pred(&self.model),
            }
        }
    }
}

/// Poll an async predicate until it holds or the deadline elapses. Generic timing
/// helper for items whose signal isn't carried on the face stream (e.g. a direct
/// Hub snapshot after a GC pass).
pub async fn wait_for<F, Fut>(within: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if f().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ── A standard local session shell ────────────────────────────────────────────

/// Build a `local` session shell (the `laptop` glyph, §21.1) with a focus hint so
/// the focus item can target a concrete editor window.
pub fn local_session(id: &str, title: &str, focus_hint: &str) -> Session {
    let mut s = Session::new(
        id,
        title,
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        State::Idle,
        "2026-06-08T00:00:00Z",
    );
    s.editor = Some(fleet_protocol::Editor {
        kind: Some(fleet_protocol::EditorKind::Vscode),
        focus_hint: Some(focus_hint.into()),
        extra: Extra::new(),
    });
    s
}

// ── Reporter drivers: recorded fixtures → real adapters → Hub ─────────────────

/// Drive a **Codex** run: feed each recorded Codex hook JSON line through the real
/// [`fleet_reporter::CodexAdapter`] and apply the resulting commands to the Hub.
pub async fn drive_codex(
    hub: &mut TestHub,
    session_id: &str,
    adapter: &mut fleet_reporter::CodexAdapter,
    lines: impl IntoIterator<Item = String>,
) {
    for line in lines {
        let cmds = adapter.ingest_json(&line);
        apply_commands(hub, session_id, cmds).await;
    }
}

/// Drive a Codex run for a known `thread`, returning the minted run-id.
pub async fn drive_codex_thread(
    hub: &mut TestHub,
    session_id: &str,
    thread: &str,
    adapter: &mut fleet_reporter::CodexAdapter,
    lines: impl IntoIterator<Item = String>,
) -> Option<String> {
    for line in lines {
        let cmds = adapter.ingest_json(&line);
        apply_commands(hub, session_id, cmds).await;
    }
    adapter.run_id_of(thread).map(|s| s.to_string())
}

/// Drive a **Claude (native-UI / inferred)** run through the real S16
/// [`fleet_reporter::ClaudeInferAdapter`]: feed lifecycle hooks at the given
/// monotonic ms timestamps, then `tick` past the debounce so the inferred waiting
/// fires. The clock is injected (no real sleeps) so the suite is fast + portable.
pub async fn drive_claude_infer(
    hub: &mut TestHub,
    session_id: &str,
    adapter: &mut fleet_reporter::ClaudeInferAdapter,
    lines: impl IntoIterator<Item = (String, u64)>,
) {
    for (line, now_ms) in lines {
        let cmds = adapter.ingest_json(&line, now_ms);
        apply_commands(hub, session_id, cmds).await;
    }
}

/// Drive a **Claude (shim terminal / high-confidence)** run through the real S17
/// [`fleet_reporter::ClaudeShimAdapter`]. Raw lines are dispatched by the adapter
/// (lifecycle vs `PermissionRequest`) exactly as a real shim reporter would.
pub async fn drive_claude_shim(
    hub: &mut TestHub,
    session_id: &str,
    adapter: &mut fleet_reporter::ClaudeShimAdapter,
    lines: impl IntoIterator<Item = String>,
) {
    for line in lines {
        let cmds = adapter.ingest_json(&line);
        apply_commands(hub, session_id, cmds).await;
    }
}

// ── Mocked OS focus (§21.5) ───────────────────────────────────────────────────

/// Run the real host-core focus pipeline against a **mocked OS backend** for the
/// given platform, returning the honest [`fleet_host_core::focus::FocusOutcome`].
/// No real window manager / editor is touched.
pub fn mock_focus(
    platform: fleet_host_core::focus::FocusPlatform,
    backend_result: fleet_host_core::focus::BackendResult,
    focus_hint: &str,
) -> fleet_host_core::focus::FocusOutcome {
    use fleet_host_core::focus::{focus_on_platform, MockBackend};
    let backend = MockBackend::new(backend_result).with_fallback(true);
    focus_on_platform(&backend, platform, focus_hint)
}

// ── CLI face: the real `fleet ls --once` binary ───────────────────────────────

/// Locate the real `fleet` CLI binary built by the workspace.
///
/// The integration test runner lives at `target/<profile>/deps/<test>-<hash>`, so
/// the workspace binaries sit two directories up at `target/<profile>/fleet`
/// (`fleet.exe` on Windows). `fleet-cli` is binary-only and cannot be a path
/// dependency (no lib target), so we discover the artifact at runtime rather than
/// relying on `CARGO_BIN_EXE_fleet`. Returns `None` if the binary is not present
/// (e.g. it was not built) so the CLI item can skip gracefully on such a runner.
pub fn find_fleet_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // …/target/<profile>/deps/<test-bin>  →  …/target/<profile>/
    let profile_dir = exe.parent()?.parent()?;
    let name = if cfg!(windows) { "fleet.exe" } else { "fleet" };
    let candidate = profile_dir.join(name);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Run the **real** `fleet ls --once` binary against the Hub at `ws_url` and
/// return its captured stdout. This proves the shipped CLI face reads the same
/// protocol off the same Hub (§21.9) — not a reducer copy, the actual binary.
///
/// Returns `Ok(None)` if the `fleet` binary is not present on this runner (so the
/// caller can fall back to a non-subprocess CLI-face check), `Ok(Some(stdout))`
/// otherwise.
pub fn cli_ls_once(ws_url: &str) -> Result<Option<String>> {
    use std::io::Read;
    use std::process::Stdio;

    let exe = match find_fleet_binary() {
        Some(p) => p,
        None => return Ok(None),
    };
    let mut child = std::process::Command::new(exe)
        .args(["--hub", ws_url, "ls", "--once"])
        // Point the unix-path at a nonexistent socket so the CLI uses the WS URL
        // (its connect() prefers unix only when the socket file exists).
        .env("FLEET_UNIX_PATH", "/nonexistent/fleet-e2e/hub.sock")
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn `fleet ls --once`")?;

    // Hard wall-clock guard: `--once` returns after the first snapshot, but never
    // let a stuck CLI hang the suite. Poll for exit, kill past the deadline.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().context("poll fleet CLI")? {
            Some(_status) => break,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("`fleet ls --once` did not exit within 10s");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let status = child.wait().context("wait fleet CLI")?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        anyhow::bail!("`fleet ls --once` exited {:?}: {stderr}", status.code());
    }
    Ok(Some(stdout))
}

//! The reporter **hook-receiver** (`fleet-reporter --serve`).
//!
//! This is the missing half that makes Fleet actually run: the thing that
//! *listens* on the reporter socket for the hook payloads `fleet init` wires
//! Claude/Codex to send, turns each into [`ReporterCommand`]s via the already-
//! tested detection adapters, and forwards them through the [`crate::reporter`]
//! framework to the Hub.
//!
//! ## The pipeline (monad-as-code: the `Result` / Kleisli flow)
//!
//! Each inbound line is one hook payload. It flows through a short, total
//! pipeline where every fallible stage returns a [`Result`] and the stages are
//! Kleisli-composed (`?` / `and_then`) in the `Result` monad:
//!
//! ```text
//!   line: &str
//!     │  parse_frame                       (fallible: empty / no-body → DriftError)
//!     ▼
//!   Ok((Agent, body)) ──┐
//!     │  dispatch         │  (stateful: routes to the Claude/Codex adapter)
//!     ▼                   │
//!   Vec<ReporterCommand>  │
//!     │  forward          │
//!     ▼                   ▼
//!   Hub               Err(DriftError) ── drift guard ── log + drop (∅ commands)
//! ```
//!
//! **Drift guard (invariant 2 + invariant 5).** A malformed frame, an unknown
//! agent tag, or an adapter parse failure never panics and never fabricates a
//! state: the pipeline collapses to *zero* commands and a `debug!` line. The
//! adapters themselves already swallow JSON parse errors (`ingest_json` returns
//! an empty `Vec`), so confidence honesty is structural here too — a drifted
//! payload simply produces no Hub delta.
//!
//! ## Framing
//!
//! One payload per line: `"<agent-tag> <compact-json>\n"`. The agent tag
//! (`claude` / `codex`) is required because the two hook payload shapes overlap
//! (both carry `hook_event_name` + `session_id`), so the *sender* declares which
//! agent it is. `fleet init` writes hook commands that strip embedded newlines
//! and prepend the tag, so each socket write is exactly one framed line. An
//! untagged line is accepted as a legacy/manual Claude payload (the validated
//! hooks-first path) so a hand-sent `printf '{...}' | nc -U` still works.

use std::time::Instant;

use tracing::debug;

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;
#[cfg(unix)]
use tokio::sync::Mutex;
#[cfg(unix)]
use tracing::{info, warn};

use crate::claude::ClaudeAdapter;
use crate::claude_infer::ClaudeInferAdapter;
use crate::codex::CodexAdapter;
use crate::reporter::ReporterCommand;
#[cfg(unix)]
use crate::reporter::ReporterHandle;

/// How often the receiver advances the S16 inference clock (the debounce TICK).
///
/// A `PreToolUse`-without-followup only becomes an inferred `Waiting` once the
/// debounce window *elapses with no new input*. Since no further hook frame may
/// arrive while the human is sat on an approval, `serve_unix` must drive the
/// machine forward itself. This interval is the granularity of that drive: it
/// must be comfortably finer than [`crate::claude_infer::DEFAULT_DEBOUNCE_MS`]
/// so the inferred waiting surfaces within roughly one window of the real
/// stall, but coarse enough not to spin.
#[cfg(unix)]
const INFER_TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Which agent a hook frame came from. The frame's leading tag selects this; the
/// payload shapes alone cannot (they overlap), so the sender must declare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// Claude Code — routed to [`ClaudeAdapter`] (hooks-first, S15).
    Claude,
    /// OpenAI Codex — routed to [`CodexAdapter`] (S12).
    Codex,
}

impl Agent {
    /// Parse a leading agent tag token. Case-insensitive; `None` for anything
    /// that isn't a recognised agent (so `parse_frame` can fall back to treating
    /// the whole line as an untagged Claude payload).
    fn from_tag(tag: &str) -> Option<Agent> {
        match tag.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

/// A drift in the receive pipeline. Every variant is a *soft* failure: the guard
/// logs at `debug` and drops the frame. None of these ever propagate as an error
/// that could stop the receiver or overstate an agent's state.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DriftError {
    /// The line was empty / whitespace-only.
    #[error("empty hook frame")]
    Empty,
}

/// Parse one framed line into `(agent, json-body)`.
///
/// - `"claude {…}"` → `(Claude, "{…}")`
/// - `"codex {…}"`  → `(Codex,  "{…}")`
/// - `"{…}"` (no recognised tag) → `(Claude, "{…}")` — untagged legacy/manual
///   payloads default to the validated hooks-first Claude path.
///
/// This is the fallible first stage of the pipeline (the `Result` monad's unit).
pub fn parse_frame(line: &str) -> Result<(Agent, &str), DriftError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(DriftError::Empty);
    }
    // Split off a leading whitespace-delimited token and test it as an agent tag.
    // (`line` is already `trim`ed, so a tag with only trailing whitespace can't
    // reach here as a bodyless frame — it collapses to a single bare token and
    // falls through to the untagged branch, which the adapter then drops.)
    if let Some((first, rest)) = line.split_once(char::is_whitespace) {
        if let Some(agent) = Agent::from_tag(first) {
            let body = rest.trim_start();
            if !body.is_empty() {
                return Ok((agent, body));
            }
        }
    }
    // No recognised tag (or a tag with no body): treat the whole line as an
    // untagged Claude payload. If it isn't valid JSON the adapter drops it.
    Ok((Agent::Claude, line))
}

/// The stateful core of the receiver: one long-lived adapter per agent, each
/// owning its per-session/-thread state machines and minting stable Fleet
/// run-ids. Shared across connections (a window's hooks may arrive on many
/// short-lived socket connections, but they belong to the same sessions).
///
/// ## The `Waiting` half (S16 inference, additive over S15)
///
/// The [`ClaudeAdapter`] is the authoritative S15 hooks path: it drives
/// `working`/`idle`/`done` but — by design — **never** emits [`State::Waiting`],
/// because in the native UI no hook announces an approval (PLAN s1). "Waiting"
/// (the ping — the core of Fleet) is *inferred* by a second, parallel adapter,
/// [`ClaudeInferAdapter`] (S16): a claude `PreToolUse` not followed by activity
/// for a debounce window ⇒ inferred `Waiting`; any later activity cancels it.
///
/// So each Claude frame is fed to **both** adapters: the S15 adapter produces
/// the working/idle/done deltas, and the infer adapter arms/cancels/resolves the
/// waiting inference. The infer adapter's *fire* happens on a [`tick`](Self::tick)
/// once the debounce elapses with no new frame, which [`serve_unix`] drives on a
/// periodic timer. Both adapters key on the claude `session_id`, so they agree on
/// the durable run anchor even though they mint independent Fleet run-ids.
#[derive(Debug)]
pub struct Receiver {
    claude: ClaudeAdapter,
    /// The S16 inferred-`Waiting` adapter, fed every Claude frame in parallel
    /// with `claude` and advanced by [`Self::tick`].
    infer: ClaudeInferAdapter,
    codex: CodexAdapter,
    /// Monotonic origin for the inference clock. Both [`Self::process`] (frame
    /// timestamps) and [`Self::tick`] (debounce advance) measure `now_ms`
    /// relative to this single instant, so they share one timeline.
    start: Instant,
    /// Total frames accepted (for `--serve` observability / tests).
    frames: u64,
    /// Frames dropped by the drift guard (for observability / tests).
    dropped: u64,
}

impl Default for Receiver {
    fn default() -> Self {
        Receiver {
            claude: ClaudeAdapter::default(),
            infer: ClaudeInferAdapter::new(),
            codex: CodexAdapter::default(),
            start: Instant::now(),
            frames: 0,
            dropped: 0,
        }
    }
}

impl Receiver {
    /// A fresh receiver tracking no agents.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames seen so far (including dropped ones).
    pub fn frames_seen(&self) -> u64 {
        self.frames
    }
    /// Frames dropped by the drift guard so far.
    pub fn frames_dropped(&self) -> u64 {
        self.dropped
    }

    /// Milliseconds since this receiver's monotonic origin — the shared clock the
    /// infer adapter uses to relate frame arrivals to debounce ticks.
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Run one framed line through the whole pipeline, returning the commands to
    /// forward (empty on any drift). Pure w.r.t. I/O; fully unit-testable.
    pub fn process(&mut self, line: &str) -> Vec<ReporterCommand> {
        self.process_at(line, self.now_ms())
    }

    /// Like [`Self::process`] but at an explicit monotonic `now_ms` — the
    /// injection point that makes the debounce timing unit-testable (the infer
    /// adapter arms at this `now_ms`; a later `tick(now_ms + window)` fires).
    pub fn process_at(&mut self, line: &str, now_ms: u64) -> Vec<ReporterCommand> {
        self.frames += 1;
        match parse_frame(line) {
            Ok((agent, body)) => {
                // An adapter that returns no commands for a well-formed line is a
                // legitimate no-op (e.g. PostToolUse liveness with nothing to
                // change), not a drift — don't count it as dropped.
                self.dispatch(agent, body, now_ms)
            }
            Err(e) => {
                self.dropped += 1;
                debug!(error = %e, frame = %truncate(line), "dropping drifted hook frame");
                Vec::new()
            }
        }
    }

    /// Advance the S16 inference clock with no new frame, firing any debounce that
    /// has now elapsed (the inferred `Waiting` ping) and auto-resolutions. Returns
    /// the commands to forward. Called on a periodic timer by [`serve_unix`].
    pub fn tick(&mut self) -> Vec<ReporterCommand> {
        self.tick_at(self.now_ms())
    }

    /// Like [`Self::tick`] but at an explicit monotonic `now_ms` (test injection).
    pub fn tick_at(&mut self, now_ms: u64) -> Vec<ReporterCommand> {
        self.infer.tick(now_ms)
    }

    /// Stage 2: route the body to the agent's adapter(s) (the stateful step).
    ///
    /// For Claude this fans out to **both** the authoritative S15 adapter (the
    /// working/idle/done path) and the S16 infer adapter (the inferred-`Waiting`
    /// path). The S15 commands lead; any waiting-inference command (an arm
    /// auto-resolution surfaced immediately, e.g. activity cancelling a raised
    /// waiting) follows. The debounce *fire* itself is deferred to [`Self::tick`].
    fn dispatch(&mut self, agent: Agent, body: &str, now_ms: u64) -> Vec<ReporterCommand> {
        match agent {
            Agent::Claude => {
                let mut cmds = self.claude.ingest_json(body);
                cmds.extend(self.infer.ingest_json(body, now_ms));
                cmds
            }
            Agent::Codex => self.codex.ingest_json(body),
        }
    }
}

/// Truncate a frame for log lines so a giant payload can't flood the log.
fn truncate(s: &str) -> String {
    const MAX: usize = 120;
    let s = s.trim();
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

/// Bind the reporter socket and serve hook frames until cancelled, forwarding
/// every resulting [`ReporterCommand`] through `handle` to the Hub.
///
/// Single-instance handling: if the socket path is already bound by a *live*
/// reporter we refuse (returning an error); if it's a *stale* file left by a
/// dead reporter we remove it and rebind. This mirrors the Hub's lockfile
/// discipline (D2) for the receiver socket.
///
/// Runs until the returned future is dropped/aborted (the binary races it
/// against Ctrl-C). The `receiver` is shared so its adapter state persists
/// across the many short-lived hook connections a window produces.
#[cfg(unix)]
pub async fn serve_unix(
    socket_path: std::path::PathBuf,
    receiver: Arc<Mutex<Receiver>>,
    handle: ReporterHandle,
) -> anyhow::Result<()> {
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = bind_reclaiming(&socket_path).await?;
    info!(socket = %socket_path.display(), "reporter --serve listening for hook frames");

    // The debounce TICK. A `PreToolUse`-without-followup inferred `Waiting` only
    // fires once the debounce window elapses with *no new frame* — and while the
    // human sits on an approval, no new frame arrives. So we must advance the
    // infer machine ourselves on a periodic timer, forwarding any inferred
    // `Waiting` (and its later auto-resolution) it emits. This runs for the life
    // of the receiver and is dropped/aborted with it.
    {
        let receiver = receiver.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(INFER_TICK_INTERVAL);
            // Skip-missed so a stalled lock can't pile up backlogged ticks; we
            // only ever need the *latest* clock advance.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let cmds = {
                    let mut rx = receiver.lock().await;
                    rx.tick()
                };
                for cmd in cmds {
                    // If the reporter loop has exited, stop ticking.
                    if !handle.send(cmd) {
                        return;
                    }
                }
            }
        });
    }

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                // An accept error on one connection must not kill the receiver.
                warn!(error = %e, "accept failed; continuing");
                continue;
            }
        };
        let receiver = receiver.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let cmds = {
                            let mut rx = receiver.lock().await;
                            rx.process(&line)
                        };
                        for cmd in cmds {
                            // If the reporter loop has exited, stop forwarding.
                            if !handle.send(cmd) {
                                return;
                            }
                        }
                    }
                    Ok(None) => return, // peer closed the connection
                    Err(e) => {
                        debug!(error = %e, "hook connection read error; closing");
                        return;
                    }
                }
            }
        });
    }
}

/// Bind a [`UnixListener`] at `path`, reclaiming a stale socket file if its
/// previous owner is gone. Errors only if a *live* peer already owns the socket.
///
/// The bound socket is restricted to the owner (mode `0600`) so no other local
/// user can connect and inject spoofed hook frames — defence-in-depth on the
/// local trust boundary (a hook frame can mutate this window's reported agent
/// state, so the channel must be owner-only).
#[cfg(unix)]
async fn bind_reclaiming(path: &Path) -> anyhow::Result<UnixListener> {
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Someone bound this path before. Probe whether they're still alive.
            match tokio::net::UnixStream::connect(path).await {
                Ok(_live) => anyhow::bail!(
                    "another fleet-reporter --serve already owns {} (live)",
                    path.display()
                ),
                Err(_) => {
                    // Stale socket file from a dead reporter — remove and rebind.
                    std::fs::remove_file(path).ok();
                    UnixListener::bind(path)?
                }
            }
        }
        Err(e) => return Err(e.into()),
    };
    restrict_socket_perms(path);
    Ok(listener)
}

/// Restrict the reporter socket to owner-only (`0600`) on unix. Best-effort: a
/// permission-set failure must not stop the receiver (the socket is still bound
/// in the user's own runtime dir).
#[cfg(unix)]
fn restrict_socket_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        warn!(error = %e, socket = %path.display(), "could not restrict reporter socket to 0600");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::State;

    // A minimal valid Claude Stop payload (matches the real 2.1.x envelope: no
    // `reason`; `stop_hook_active:false` ⇒ a real turn boundary → idle).
    fn claude_stop(session: &str) -> String {
        format!(
            r#"{{"hook_event_name":"Stop","session_id":"{session}","cwd":"/repo","stop_hook_active":false}}"#
        )
    }
    fn claude_prompt(session: &str) -> String {
        format!(
            r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{session}","cwd":"/repo"}}"#
        )
    }

    // ── parse_frame: the fallible first stage ────────────────────────────────

    #[test]
    fn parse_frame_tagged_claude() {
        let frame = format!("claude {}", claude_stop("s1"));
        let (agent, body) = parse_frame(&frame).unwrap();
        assert_eq!(agent, Agent::Claude);
        assert!(body.starts_with('{') && body.contains("Stop"));
    }

    #[test]
    fn parse_frame_tag_is_case_insensitive() {
        assert_eq!(parse_frame("CLAUDE {}").unwrap().0, Agent::Claude);
        assert_eq!(parse_frame("Codex {}").unwrap().0, Agent::Codex);
    }

    #[test]
    fn parse_frame_untagged_defaults_to_claude() {
        let frame = claude_stop("s1");
        let (agent, body) = parse_frame(&frame).unwrap();
        assert_eq!(agent, Agent::Claude, "untagged ⇒ validated Claude path");
        assert!(body.contains("Stop"));
    }

    #[test]
    fn parse_frame_empty_is_drift() {
        assert_eq!(parse_frame("   "), Err(DriftError::Empty));
        assert_eq!(parse_frame(""), Err(DriftError::Empty));
    }

    #[test]
    fn parse_frame_bodyless_tag_falls_through_harmlessly() {
        // A tag with no JSON body (whether bare or with only trailing space,
        // which the leading trim removes) isn't a usable tagged frame: it falls
        // through to the untagged-Claude branch as a non-JSON body, which the
        // Claude adapter drops downstream — safe, never a panic or a state.
        assert_eq!(parse_frame("claude   "), Ok((Agent::Claude, "claude")));
        assert_eq!(parse_frame("codex"), Ok((Agent::Claude, "codex")));
        // And such a frame yields zero commands through the full pipeline.
        let mut rx = Receiver::new();
        assert!(rx.process("claude").is_empty());
        assert!(rx.process("codex").is_empty());
    }

    // ── process: the whole pipeline incl. the drift guard ────────────────────

    #[test]
    fn process_claude_prompt_then_stop_drives_working_then_idle() {
        let mut rx = Receiver::new();

        // First a prompt → the session's run goes Working (a state change ⇒
        // exactly one UpsertRun).
        let cmds = rx.process(&format!("claude {}", claude_prompt("sess-A")));
        let run = cmds
            .iter()
            .find_map(|c| match c {
                ReporterCommand::UpsertRun(r) => Some(r.clone()),
                _ => None,
            })
            .expect("prompt should upsert a run");
        assert_eq!(run.state, State::Working);
        assert_eq!(
            run.native_id, "sess-A",
            "durable anchor is the claude session_id"
        );

        // Then a Stop → Idle.
        let cmds = rx.process(&format!("claude {}", claude_stop("sess-A")));
        let run = cmds
            .iter()
            .find_map(|c| match c {
                ReporterCommand::UpsertRun(r) => Some(r.clone()),
                _ => None,
            })
            .expect("stop should upsert a run");
        assert_eq!(run.state, State::Idle);

        assert_eq!(rx.frames_seen(), 2);
        assert_eq!(rx.frames_dropped(), 0);
    }

    #[test]
    fn process_garbage_json_is_dropped_not_panicked() {
        let mut rx = Receiver::new();
        // Tagged but the body is not valid JSON: the adapter swallows it, so the
        // pipeline yields no commands. (The drift counter only tracks frame-level
        // drift, not adapter-level no-ops, but the key property is: no panic, no
        // command, no fabricated state.)
        let cmds = rx.process("claude this-is-not-json");
        assert!(cmds.is_empty(), "garbage must yield no commands");
        // A truly empty frame is frame-level drift.
        let cmds = rx.process("   ");
        assert!(cmds.is_empty());
        assert_eq!(rx.frames_dropped(), 1, "the empty frame was dropped");
    }

    // ── serve_unix: the real socket → adapter → handle path ──────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn serve_unix_forwards_a_framed_hook_to_the_handle() {
        use crate::reporter::{Reporter, ReporterConfig};
        use tokio::io::AsyncWriteExt;

        // A reporter channel: we don't run the full reporter loop — we just read
        // the commands serve_unix pushes onto the handle's channel.
        let reporter = Reporter::new(
            ReporterConfig::new("sess-window"),
            Box::new(crate::transport::WsConnector::new("ws://127.0.0.1:1")),
        );
        let (_reporter, handle, mut rx) = reporter.with_channel();

        // Bind the receiver on a temp unix socket.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("reporter.sock");
        let receiver = Arc::new(Mutex::new(Receiver::new()));
        let serve = tokio::spawn(serve_unix(sock.clone(), receiver, handle));
        // give the listener a moment to bind
        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // A client (the stand-in for `nc -U`) writes one framed line.
        let mut client = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let frame = format!("claude {}\n", claude_prompt("sess-A"));
        client.write_all(frame.as_bytes()).await.unwrap();
        client.flush().await.unwrap();
        drop(client); // EOF the connection

        // The handle must receive an UpsertRun(Working) for that session.
        let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("a command should arrive within 2s")
            .expect("channel open");
        match cmd {
            ReporterCommand::UpsertRun(run) => {
                assert_eq!(run.state, State::Working);
                assert_eq!(run.native_id, "sess-A");
            }
            other => panic!("expected UpsertRun, got {other:?}"),
        }

        serve.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn serve_unix_reclaims_a_stale_socket_file() {
        // A leftover socket *file* with no live owner must be reclaimed, not fatal.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("reporter.sock");
        // Create-and-drop a listener to leave the socket file behind, dead.
        {
            let _dead = UnixListener::bind(&sock).unwrap();
        }
        assert!(sock.exists(), "stale socket file present");
        let l = bind_reclaiming(&sock).await;
        assert!(l.is_ok(), "stale socket must be reclaimed: {:?}", l.err());
    }

    fn claude_pretool(session: &str) -> String {
        format!(
            r#"{{"hook_event_name":"PreToolUse","session_id":"{session}","cwd":"/repo","tool_name":"Bash"}}"#
        )
    }

    // ── S16 inference wiring: the Waiting half, additive over S15 ─────────────

    #[test]
    fn pretool_then_tick_past_debounce_infers_waiting() {
        use crate::claude_infer::DEFAULT_DEBOUNCE_MS;
        let mut rx = Receiver::new();
        let window = DEFAULT_DEBOUNCE_MS;

        // A PreToolUse at t=0: S15 drives Working; the infer adapter arms but does
        // NOT yet emit Waiting (the debounce hasn't elapsed).
        let cmds = rx.process_at(&format!("claude {}", claude_pretool("sess-W")), 0);
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                ReporterCommand::UpsertRun(r) if r.state == State::Waiting
            )),
            "no Waiting before the debounce elapses"
        );

        // A tick before the window: still no Waiting.
        let cmds = rx.tick_at(window - 1);
        assert!(cmds.is_empty(), "a tick before the window is a no-op");

        // A tick at/after the window: the inferred Waiting fires.
        let cmds = rx.tick_at(window);
        let run = cmds
            .iter()
            .find_map(|c| match c {
                ReporterCommand::UpsertRun(r) => Some(r.clone()),
                _ => None,
            })
            .expect("tick past the debounce should infer a Waiting run");
        assert_eq!(run.state, State::Waiting, "the inferred ping");
        assert_eq!(
            run.native_id, "sess-W",
            "the inference keys on the same claude session_id"
        );
        assert_eq!(
            run.confidence,
            fleet_protocol::Confidence::Inferred,
            "inferred waiting is never High (invariant 5)"
        );
    }

    #[test]
    fn activity_before_debounce_cancels_the_inference() {
        use crate::claude_infer::DEFAULT_DEBOUNCE_MS;
        let mut rx = Receiver::new();
        let window = DEFAULT_DEBOUNCE_MS;

        // Arm at t=0, then a Stop at t=window/2 cancels the pending inference.
        rx.process_at(&format!("claude {}", claude_pretool("sess-X")), 0);
        rx.process_at(&format!("claude {}", claude_stop("sess-X")), window / 2);

        // A tick well past the original window must NOT fire a Waiting.
        let cmds = rx.tick_at(window * 5);
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                ReporterCommand::UpsertRun(r) if r.state == State::Waiting
            )),
            "a cancelled debounce never fires Waiting"
        );
    }

    #[test]
    fn s15_working_idle_path_stays_intact_and_never_waits() {
        // The additive infer adapter must not disturb the S15 working/idle path:
        // a prompt → Working, a stop → Idle, and neither emits Waiting.
        let mut rx = Receiver::new();
        let cmds = rx.process(&format!("claude {}", claude_prompt("sess-Y")));
        assert!(cmds.iter().any(|c| matches!(
            c,
            ReporterCommand::UpsertRun(r) if r.state == State::Working
        )));
        let cmds = rx.process(&format!("claude {}", claude_stop("sess-Y")));
        assert!(cmds.iter().any(|c| matches!(
            c,
            ReporterCommand::UpsertRun(r) if r.state == State::Idle
        )));
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                ReporterCommand::UpsertRun(r) if r.state == State::Waiting
            )),
            "the S15 path never fabricates Waiting"
        );
    }

    #[test]
    fn process_two_sessions_get_distinct_runs() {
        let mut rx = Receiver::new();
        let a = rx.process(&format!("claude {}", claude_prompt("sess-A")));
        let b = rx.process(&format!("claude {}", claude_prompt("sess-B")));
        let id = |cmds: &[ReporterCommand]| {
            cmds.iter().find_map(|c| match c {
                ReporterCommand::UpsertRun(r) => Some(r.run_id.clone()),
                _ => None,
            })
        };
        assert_ne!(id(&a), id(&b), "distinct sessions ⇒ distinct Fleet run-ids");
    }
}

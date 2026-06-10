//! The Hub server: tokio runtime, WebSocket listener (always) + unix-socket
//! fast path (`cfg(unix)`), shared merge engine, subscribe→snapshot, delta
//! broadcast (PLAN S2, D2, D7).
//!
//! Transport (D7): a WebSocket listener binds **always** (universal, cross-OS);
//! on `cfg(unix)` a unix-domain socket also binds as the local fast path. Both
//! speak the same JSON frames — a line/text-framed [`crate::wire::ClientMessage`]
//! inbound and a [`fleet_protocol::Event`] outbound — so a client on either
//! transport is indistinguishable to the merge engine.
//!
//! Concurrency model: one authoritative [`MergeEngine`] behind an async
//! [`Mutex`]; a [`broadcast`] channel fans every applied delta out to all
//! subscribers. A subscriber's connection task: (1) locks the engine, applies
//! any inbound delta, releasing the broadcast events; (2) on `subscribe`, sends
//! the current snapshot **then** attaches to the broadcast stream. Snapshot is
//! taken under the same lock that gates new deltas, so no delta is lost or
//! double-applied across the subscribe boundary.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fleet_protocol::Event;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::persist::{PersistError, StateStore};
use crate::wire::ClientMessage;

/// Default capacity of the per-connection broadcast backlog. A slow subscriber
/// that lags beyond this many events is lagged (it will observe a `Lagged`
/// error and can re-subscribe for a fresh snapshot).
const BROADCAST_CAPACITY: usize = 1024;

/// Shared Hub state handed to every connection task.
///
/// State lives in a durable [`StateStore`] (PLAN S7, D3): every accepted reporter
/// delta is appended to a SQLite event log **then** projected into memory and
/// broadcast, so a Hub restart restores all sessions/runs from the log. Opening
/// the store with an existing log replays it, restoring the projection before
/// the first connection is served.
#[derive(Clone)]
pub struct HubState {
    store: Arc<Mutex<StateStore>>,
    tx: broadcast::Sender<Event>,
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}

impl HubState {
    /// A fresh Hub backed by an **in-memory** event log (no durability across
    /// restart). Used by tests and the transport smoke harness; the daemon uses
    /// [`Self::with_db`] for a persistent log.
    pub fn new() -> Self {
        let store = StateStore::open_in_memory().expect("in-memory sqlite log always opens");
        Self::from_store(store)
    }

    /// A Hub backed by a durable on-disk event log at `db_path` (D3). Replays any
    /// existing log to restore the projection before serving.
    pub fn with_db(db_path: impl AsRef<Path>) -> Result<Self, PersistError> {
        let store = StateStore::open(db_path)?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: StateStore) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        HubState {
            store: Arc::new(Mutex::new(store)),
            tx,
        }
    }

    /// Apply one inbound message, persisting + projecting the mutation and
    /// broadcasting any resulting events. `subscribe`/`command` return an
    /// optional immediate reply for the calling connection (the snapshot for
    /// `subscribe`).
    ///
    /// Returning the snapshot here — under the same lock that serializes deltas
    /// — is what makes subscribe atomic w.r.t. the delta stream. A persistence
    /// error on a delta is logged and the delta is dropped (the in-memory
    /// projection is only updated *after* a successful append, so memory and the
    /// log never diverge).
    async fn apply(&self, msg: ClientMessage) -> Option<Event> {
        match msg {
            ClientMessage::Subscribe => {
                let store = self.store.lock().await;
                Some(Event::snapshot(store.snapshot()))
            }
            ClientMessage::Command { command } => {
                // Commands are accepted asynchronously: mutations are broadcast
                // as normal deltas rather than returned as immediate replies.
                self.apply_command(command).await;
                None
            }
            ClientMessage::SessionUpsert { session } => {
                self.ingest_session_upsert(session).await;
                None
            }
            ClientMessage::SessionRemove { session_id } => {
                let mut store = self.store.lock().await;
                match store.apply_session_remove(&session_id) {
                    Ok(evs) => {
                        for ev in evs {
                            let _ = self.tx.send(ev);
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "persist session.remove failed; dropped"),
                }
                None
            }
            ClientMessage::RunUpsert {
                session_id,
                run,
                stamp,
            } => {
                self.ingest_run_upsert_stamped(&session_id, run, stamp)
                    .await;
                None
            }
            ClientMessage::RunRemove { session_id, run_id } => {
                let mut store = self.store.lock().await;
                match store.apply_run_remove(&session_id, &run_id) {
                    Ok(evs) => {
                        for ev in evs {
                            let _ = self.tx.send(ev);
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "persist run.remove failed; dropped"),
                }
                None
            }
        }
    }

    /// Apply a face→Hub command and broadcast the resulting persisted events.
    /// Unknown commands are logged and accepted for forward compatibility.
    async fn apply_command(&self, command: fleet_protocol::Command) {
        use fleet_protocol::{Command, Target};
        match command {
            Command::Focus { target, .. } => {
                let mut store = self.store.lock().await;
                let session_id = match target {
                    Target::Session { session_id } => Some(session_id),
                    Target::Run { run_id } => store.snapshot().into_iter().find_map(|session| {
                        session
                            .runs
                            .iter()
                            .any(|run| run.run_id == run_id)
                            .then_some(session.session_id)
                    }),
                };
                if let Some(session_id) = session_id {
                    if let Some(ev) = store.apply_focus(&session_id) {
                        let _ = self.tx.send(ev);
                    }
                }
            }
            Command::Mute { session_id, .. } => {
                let mut store = self.store.lock().await;
                for ev in store.apply_mute(&session_id) {
                    let _ = self.tx.send(ev);
                }
            }
            Command::Unmute { session_id, .. } => {
                let mut store = self.store.lock().await;
                for ev in store.apply_unmute(&session_id) {
                    let _ = self.tx.send(ev);
                }
            }
            Command::Solo { session_id, .. } => {
                let mut store = self.store.lock().await;
                for ev in store.apply_solo(&session_id) {
                    let _ = self.tx.send(ev);
                }
            }
            Command::Dismiss { target, .. } => {
                let mut store = self.store.lock().await;
                match target {
                    Target::Session { session_id } => match store.apply_session_remove(&session_id)
                    {
                        Ok(evs) => {
                            for ev in evs {
                                let _ = self.tx.send(ev);
                            }
                        }
                        Err(e) => tracing::error!(
                            error = %e,
                            session_id,
                            "persist dismiss session failed; dropped"
                        ),
                    },
                    Target::Run { run_id } => {
                        let session_id = store.snapshot().into_iter().find_map(|session| {
                            session
                                .runs
                                .iter()
                                .any(|run| run.run_id == run_id)
                                .then_some(session.session_id)
                        });
                        if let Some(session_id) = session_id {
                            match store.apply_run_remove(&session_id, &run_id) {
                                Ok(evs) => {
                                    for ev in evs {
                                        let _ = self.tx.send(ev);
                                    }
                                }
                                Err(e) => tracing::error!(
                                    error = %e,
                                    session_id,
                                    run_id,
                                    "persist dismiss run failed; dropped"
                                ),
                            }
                        }
                    }
                }
            }
            other => {
                tracing::debug!(
                    command = other.command_name(),
                    "command received (not yet implemented in this slice)"
                );
            }
        }
    }

    /// Current snapshot (used by tests and the unix/ws snapshot path).
    pub async fn snapshot_event(&self) -> Event {
        let store = self.store.lock().await;
        Event::snapshot(store.snapshot())
    }

    /// Persist + project + broadcast a session upsert. The public, transport-
    /// agnostic equivalent of receiving a `session.upsert` delta — used by the
    /// fake reporter and integration tests that drive the Hub without a socket.
    pub async fn ingest_session_upsert(&self, session: fleet_protocol::Session) {
        let mut store = self.store.lock().await;
        match store.apply_session_upsert(session) {
            Ok(ev) => {
                let _ = self.tx.send(ev);
            }
            Err(e) => tracing::error!(error = %e, "persist session.upsert failed; dropped"),
        }
    }

    /// Persist + project + broadcast a run upsert (see [`Self::ingest_session_upsert`]).
    ///
    /// Un-stamped (S5 reporters): applied ungated, preserving prior behavior.
    pub async fn ingest_run_upsert(&self, session_id: &str, run: fleet_protocol::AgentRun) {
        let mut store = self.store.lock().await;
        match store.apply_run_upsert(session_id, run) {
            Ok(evs) => {
                for ev in evs {
                    let _ = self.tx.send(ev);
                }
            }
            Err(e) => tracing::error!(error = %e, "persist run.upsert failed; dropped"),
        }
    }

    /// Persist + project + broadcast a run upsert, **gated by the durable-identity
    /// stamp** when present (S6). A stamped delta flows through the reclaim table
    /// ([`crate::reclaim`]): applied once per `(durable_id, seq)` in `seq` order;
    /// a duplicate or stale out-of-order delta is dropped (no broadcast). A delta
    /// with no stamp (S5 reporter) is applied ungated.
    pub async fn ingest_run_upsert_stamped(
        &self,
        session_id: &str,
        run: fleet_protocol::AgentRun,
        stamp: Option<crate::wire::SeqStamp>,
    ) {
        let mut store = self.store.lock().await;
        let result = match stamp {
            Some(s) => {
                let did = crate::reclaim::DurableId::new(s.durable_id);
                store
                    .apply_run_upsert_seq(session_id, run, &did, s.epoch, s.seq)
                    .map(|(_decision, evs)| evs)
            }
            None => store.apply_run_upsert(session_id, run),
        };
        match result {
            Ok(evs) => {
                for ev in evs {
                    let _ = self.tx.send(ev);
                }
            }
            Err(e) => tracing::error!(error = %e, "persist run.upsert failed; dropped"),
        }
    }

    /// Run one GC pass: reap `dead` runs past `grace` (D17) and sweep sessions
    /// untouched past `session_ttl`, broadcasting every resulting removal. `now`
    /// is an ISO-8601 UTC instant (the daemon passes the wall clock). Returns the
    /// number of broadcast events. Intended to be called on the reap timer.
    pub async fn gc(
        &self,
        now: &str,
        grace: std::time::Duration,
        session_ttl: std::time::Duration,
    ) -> Result<usize, PersistError> {
        let mut store = self.store.lock().await;
        let mut events = store.reap_dead(now, grace)?;
        events.extend(store.sweep_expired_sessions(now, session_ttl)?);
        let n = events.len();
        for ev in events {
            let _ = self.tx.send(ev);
        }
        Ok(n)
    }

    /// Subscribe to the broadcast stream of applied deltas.
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

/// Encode an outbound [`Event`] as a JSON text frame.
fn encode(ev: &Event) -> Message {
    // Events are always serializable; fall back to an empty object on the
    // impossible error rather than panicking a connection task.
    let txt = serde_json::to_string(ev).unwrap_or_else(|_| "{}".to_string());
    Message::Text(txt)
}

/// Drive a single accepted WebSocket connection to completion.
///
/// This is generic over the underlying stream so the same logic serves a TCP
/// socket and (on unix) a unix-domain socket.
pub async fn serve_ws_connection<S>(state: HubState, stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!(error = %e, "ws handshake failed");
            return;
        }
    };
    let (mut sink, mut source) = ws.split();
    let mut rx = state.subscribe();

    loop {
        tokio::select! {
            // Inbound client frame.
            incoming = source.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<ClientMessage>(&txt) {
                            Ok(msg) => {
                                if let Some(reply) = state.apply(msg).await {
                                    if sink.send(encode(&reply)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "undecodable client message");
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bin))) => {
                        match serde_json::from_slice::<ClientMessage>(&bin) {
                            Ok(msg) => {
                                if let Some(reply) = state.apply(msg).await {
                                    if sink.send(encode(&reply)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => tracing::warn!(error = %e, "undecodable binary message"),
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if sink.send(Message::Pong(p)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // pong / frame — ignore
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "ws read error; closing connection");
                        break;
                    }
                }
            }
            // Outbound broadcast delta.
            ev = rx.recv() => {
                match ev {
                    Ok(ev) => {
                        if sink.send(encode(&ev)).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "subscriber lagged; deltas dropped (re-subscribe for snapshot)");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

/// Bind the WebSocket listener on `addr` and accept connections forever.
///
/// Returns the bound [`SocketAddr`] (resolving `:0` to the OS-assigned port) via
/// the `bound` callback before entering the accept loop, so callers/tests can
/// learn the real port.
pub async fn run_ws_listener(
    state: HubState,
    addr: SocketAddr,
) -> std::io::Result<(SocketAddr, impl std::future::Future<Output = ()>)> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    tracing::info!(%local, "ws listener bound");
    let fut = async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    tracing::debug!(%peer, "ws connection accepted");
                    let st = state.clone();
                    tokio::spawn(async move {
                        serve_ws_connection(st, stream).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ws accept error");
                }
            }
        }
    };
    Ok((local, fut))
}

/// Bind the unix-domain socket listener and accept connections forever
/// (`cfg(unix)` only — D7 fast path). Removes any stale socket file first.
#[cfg(unix)]
pub async fn run_unix_listener(
    state: HubState,
    path: std::path::PathBuf,
) -> std::io::Result<impl std::future::Future<Output = ()>> {
    use tokio::net::UnixListener;
    // A leftover socket file blocks bind with EADDRINUSE; clear it. The
    // single-instance lockfile (D2) guarantees no live Hub owns it.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    tracing::info!(path = %path.display(), "unix listener bound");
    let fut = async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    tracing::debug!("unix connection accepted");
                    let st = state.clone();
                    tokio::spawn(async move {
                        serve_ws_connection(st, stream).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "unix accept error");
                }
            }
        }
    };
    Ok(fut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
        ServerKind, Session, State,
    };

    fn sess(id: &str) -> Session {
        Session::new(
            id,
            "t",
            Location {
                kind: LocationKind::Local,
                label: "l".into(),
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
        )
    }

    #[tokio::test]
    async fn subscribe_returns_empty_snapshot() {
        let state = HubState::new();
        let reply = state.apply(ClientMessage::Subscribe).await.unwrap();
        match reply {
            Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delta_then_subscribe_reflects_state() {
        let state = HubState::new();
        // A reporter registers a session, then a run.
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let run = AgentRun::new(
            "r1",
            AgentKind::Codex,
            "n",
            "/",
            State::Working,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        state
            .apply(ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run,
                stamp: None,
            })
            .await;
        // A late subscriber sees the accumulated state.
        let reply = state.apply(ClientMessage::Subscribe).await.unwrap();
        match reply {
            Event::Snapshot { sessions, .. } => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session_id, "s1");
                assert_eq!(sessions[0].rollup_state, State::Working);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deltas_are_broadcast_to_subscribers() {
        let state = HubState::new();
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.added");
    }

    #[tokio::test]
    async fn command_is_accepted_without_panic() {
        let state = HubState::new();
        let reply = state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::mute("s1"),
            })
            .await;
        assert!(reply.is_none(), "command has no immediate reply");
    }

    #[tokio::test]
    async fn focus_session_clears_unread_and_broadcasts() {
        let state = HubState::new();
        let mut session = sess("s1");
        session.unread = true;
        session.rollup_state = State::Waiting;
        state.apply(ClientMessage::SessionUpsert { session }).await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::focus(fleet_protocol::Target::session("s1")),
            })
            .await;

        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.updated");
        match ev {
            Event::SessionUpdated { session, .. } => {
                assert!(!session.unread, "focus must clear unread");
                assert_eq!(session.rollup_state, State::Waiting);
            }
            other => panic!("expected session.updated, got {other:?}"),
        }

        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            Event::Snapshot { sessions, .. } => assert!(!sessions[0].unread),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn focus_run_clears_owning_session_unread() {
        let state = HubState::new();
        let mut session = sess("s1");
        session.unread = true;
        state.apply(ClientMessage::SessionUpsert { session }).await;
        state
            .apply(ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run: AgentRun::new(
                    "r1",
                    AgentKind::Codex,
                    "n",
                    "/",
                    State::Waiting,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
                stamp: None,
            })
            .await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::focus(fleet_protocol::Target::run("r1")),
            })
            .await;

        let ev = rx.recv().await.unwrap();
        match ev {
            Event::SessionUpdated { session, .. } => {
                assert_eq!(session.session_id, "s1");
                assert!(!session.unread);
            }
            other => panic!("expected session.updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dismiss_session_removes_it_and_broadcasts() {
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::session("s1")),
            })
            .await;

        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.removed");
        match ev {
            Event::SessionRemoved { session_id, .. } => assert_eq!(session_id, "s1"),
            other => panic!("expected session.removed, got {other:?}"),
        }

        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dismiss_soloed_session_broadcasts_rearmed_waiting_session() {
        let state = HubState::new();
        for id in ["solo", "other"] {
            state
                .apply(ClientMessage::SessionUpsert { session: sess(id) })
                .await;
        }
        for (session_id, run_id) in [("solo", "r1"), ("other", "r2")] {
            state
                .apply(ClientMessage::RunUpsert {
                    session_id: session_id.into(),
                    run: AgentRun::new(
                        run_id,
                        AgentKind::Codex,
                        "n",
                        "/",
                        State::Waiting,
                        Confidence::High,
                        "2026-06-08T00:00:00Z",
                    ),
                    stamp: None,
                })
                .await;
            state
                .apply(ClientMessage::Command {
                    command: fleet_protocol::Command::focus(fleet_protocol::Target::session(
                        session_id,
                    )),
                })
                .await;
        }
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::solo("solo"),
            })
            .await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::session("solo")),
            })
            .await;

        let removed = rx.recv().await.unwrap();
        let updated = rx.recv().await.unwrap();
        assert!(matches!(removed, Event::SessionRemoved { .. }));
        match updated {
            Event::SessionUpdated { session, .. } => {
                assert_eq!(session.session_id, "other");
                assert!(session.unread, "remaining waiting session must be re-armed");
            }
            other => panic!("expected session.updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dismiss_run_removes_it_and_updates_session() {
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        state
            .apply(ClientMessage::RunUpsert {
                session_id: "s1".into(),
                run: AgentRun::new(
                    "r1",
                    AgentKind::Codex,
                    "n",
                    "/",
                    State::Dead,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
                stamp: None,
            })
            .await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::run("r1")),
            })
            .await;

        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert!(matches!(first, Event::RunRemoved { .. }));
        assert!(matches!(second, Event::SessionUpdated { .. }));

        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            Event::Snapshot { sessions, .. } => assert!(sessions[0].runs.is_empty()),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    // ── S25: mute / solo command handling ─────────────────────────────────────

    #[tokio::test]
    async fn mute_command_sets_flag_and_broadcasts() {
        let state = HubState::new();
        let mut rx = state.subscribe();
        // Register a session first.
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let _ = rx.recv().await; // consume session.added

        // Send a mute command.
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::mute("s1"),
            })
            .await;

        // Must broadcast a session.updated with muted=true.
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.updated");
        match ev {
            fleet_protocol::Event::SessionUpdated { session, .. } => {
                assert!(session.muted, "muted flag must be set in broadcast event");
            }
            other => panic!("expected session.updated, got {other:?}"),
        }

        // Snapshot reflects the flag.
        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            fleet_protocol::Event::Snapshot { sessions, .. } => {
                assert!(sessions[0].muted, "snapshot must show muted=true");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unmute_command_clears_flag() {
        let state = HubState::new();
        // Register + mute.
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::mute("s1"),
            })
            .await;
        let mut rx = state.subscribe();

        // Unmute.
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::unmute("s1"),
            })
            .await;
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.updated");
        match ev {
            fleet_protocol::Event::SessionUpdated { session, .. } => {
                assert!(!session.muted, "unmuted flag must be cleared in broadcast");
            }
            other => panic!("expected session.updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mute_on_absent_session_does_not_broadcast() {
        let state = HubState::new();
        let mut rx = state.subscribe();
        // Mute a session that doesn't exist — no broadcast.
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::mute("ghost"),
            })
            .await;
        // Channel should be empty (no events broadcast).
        assert!(
            rx.try_recv().is_err(),
            "muting an absent session must not broadcast"
        );
    }

    #[tokio::test]
    async fn solo_command_sets_solo_and_clears_others() {
        let state = HubState::new();
        // Register two sessions.
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s2"),
            })
            .await;
        let mut rx = state.subscribe();
        // Consume the two session.added / session.updated events from upserts.
        // (They may or may not have been consumed already depending on timing.)

        // Solo s1.
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::solo("s1"),
            })
            .await;

        // Should get a session.updated for s1 with soloed=true.
        let mut saw_s1_solo = false;
        // Drain the broadcast channel — may contain earlier events too.
        while let Ok(ev) = rx.try_recv() {
            if let fleet_protocol::Event::SessionUpdated { session, .. } = &ev {
                if session.session_id == "s1" && session.soloed {
                    saw_s1_solo = true;
                }
            }
        }
        assert!(
            saw_s1_solo,
            "solo must broadcast session.updated with soloed=true for s1"
        );

        // Snapshot reflects the solo.
        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            fleet_protocol::Event::Snapshot { sessions, .. } => {
                let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
                let s2 = sessions.iter().find(|s| s.session_id == "s2").unwrap();
                assert!(s1.soloed, "s1 must be soloed in snapshot");
                assert!(!s2.soloed, "s2 must not be soloed in snapshot");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

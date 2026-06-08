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
use std::sync::Arc;

use fleet_protocol::Event;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::merge::MergeEngine;
use crate::wire::ClientMessage;

/// Default capacity of the per-connection broadcast backlog. A slow subscriber
/// that lags beyond this many events is lagged (it will observe a `Lagged`
/// error and can re-subscribe for a fresh snapshot).
const BROADCAST_CAPACITY: usize = 1024;

/// Shared Hub state handed to every connection task.
#[derive(Clone)]
pub struct HubState {
    engine: Arc<Mutex<MergeEngine>>,
    tx: broadcast::Sender<Event>,
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}

impl HubState {
    /// A fresh Hub with an empty engine and a live broadcast channel.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        HubState {
            engine: Arc::new(Mutex::new(MergeEngine::new())),
            tx,
        }
    }

    /// Apply one inbound message, mutating the engine and broadcasting any
    /// resulting events. `subscribe`/`command` return an optional immediate
    /// reply for the calling connection (the snapshot for `subscribe`).
    ///
    /// Returning the snapshot here — under the same lock that serializes deltas
    /// — is what makes subscribe atomic w.r.t. the delta stream.
    async fn apply(&self, msg: ClientMessage) -> Option<Event> {
        match msg {
            ClientMessage::Subscribe => {
                let engine = self.engine.lock().await;
                Some(Event::snapshot(engine.snapshot()))
            }
            ClientMessage::Command { command } => {
                // S2: commands are accepted and acknowledged but not yet acted
                // on (mute/solo land in MUTE/S25). Logged for observability.
                tracing::debug!(
                    command = command.command_name(),
                    "command received (no-op in S2)"
                );
                None
            }
            ClientMessage::SessionUpsert { session } => {
                let mut engine = self.engine.lock().await;
                let ev = engine.upsert_session(session);
                let _ = self.tx.send(ev);
                None
            }
            ClientMessage::SessionRemove { session_id } => {
                let mut engine = self.engine.lock().await;
                if let Some(ev) = engine.remove_session(&session_id) {
                    let _ = self.tx.send(ev);
                }
                None
            }
            ClientMessage::RunUpsert { session_id, run } => {
                let mut engine = self.engine.lock().await;
                for ev in engine.upsert_run(&session_id, run) {
                    let _ = self.tx.send(ev);
                }
                None
            }
            ClientMessage::RunRemove { session_id, run_id } => {
                let mut engine = self.engine.lock().await;
                for ev in engine.remove_run(&session_id, &run_id) {
                    let _ = self.tx.send(ev);
                }
                None
            }
        }
    }

    /// Current snapshot (used by tests and the unix/ws snapshot path).
    pub async fn snapshot_event(&self) -> Event {
        let engine = self.engine.lock().await;
        Event::snapshot(engine.snapshot())
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
        assert!(reply.is_none(), "command has no immediate reply in S2");
    }
}

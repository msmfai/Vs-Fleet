//! The Hub server: tokio runtime, WebSocket listener (always) + unix-socket
//! fast path (`cfg(unix)`), shared merge engine, subscribe→snapshot, delta
//! broadcast (the engineering spec, D2, D7).
//!
//! Transport (D7): a WebSocket listener binds **always** (universal, cross-OS);
//! on `cfg(unix)` a unix-domain socket also binds as the local fast path. Both
//! speak the same JSON frames — a line/text-framed [`crate::wire::ClientMessage`]
//! inbound and a [`fleet_protocol::Event`] outbound — so a client on either
//! transport is indistinguishable to the merge engine.
//!
//! Concurrency model: one authoritative [`StateStore`] behind an async
//! [`Mutex`]; a [`broadcast`] channel fans every applied delta out to all
//! subscribers. Mutations run *off* the async worker via `spawn_blocking` (the
//! blocking SQLite append never stalls a tokio worker — T1.2) and, after
//! applying, refresh a **published snapshot** (an `RwLock<Vec<Session>>`).
//! Faces read that published snapshot on `subscribe`/`snapshot` WITHOUT taking
//! the store mutex, so a slow durable append can never block a snapshot read.
//! A connection attaches its broadcast receiver *before* it sends `subscribe`,
//! so every post-attach delta is delivered on the stream; any delta also present
//! in the snapshot is idempotent for faces — no delta is lost across the
//! subscribe boundary.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fleet_protocol::{Event, Session};
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
/// State lives in a durable [`StateStore`] (the engineering spec): every accepted reporter
/// delta is appended to a SQLite event log **then** projected into memory and
/// broadcast, so a Hub restart restores all sessions/runs from the log. Opening
/// the store with an existing log replays it, restoring the projection before
/// the first connection is served.
#[derive(Clone)]
pub struct HubState {
    /// Serializes MUTATIONS end-to-end (durable append + in-memory project). It
    /// is acquired on the blocking pool and held across the slow SQLite append
    /// (T1.2), so a mutation's disk I/O never runs on a tokio worker and a face
    /// read never waits behind the append (see [`Self::publish`]).
    store: Arc<Mutex<StateStore>>,
    /// The single FAST serialization point for subscribe-atomicity. It pairs the
    /// published read snapshot with the broadcast sender so that a mutation
    /// publishes its new snapshot and broadcasts its events in ONE critical
    /// section, and a subscribe reads the snapshot and attaches its receiver in
    /// ONE critical section. Every face's stream is therefore exactly
    /// `{snapshot at the instant it subscribed} + {every broadcast after that}` —
    /// identical for all faces regardless of scheduling. The slow append is NOT
    /// under this lock, so a snapshot read never waits on disk.
    publish: Arc<std::sync::Mutex<Publish>>,
}

/// Snapshot + broadcast sender guarded together as the one subscribe-atomicity
/// serialization point (see [`HubState::publish`]).
struct Publish {
    snapshot: Vec<Session>,
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
        let publish = Arc::new(std::sync::Mutex::new(Publish {
            snapshot: store.snapshot(),
            tx,
        }));
        HubState {
            store: Arc::new(Mutex::new(store)),
            publish,
        }
    }

    /// Wrap an arbitrary [`StateStore`] (tests only). Used to inject a store whose
    /// log is read-only so the persist-failure (`Err(e) => tracing::error!`) arms
    /// of `apply`/`apply_command`/`ingest_*` fire deterministically.
    #[cfg(test)]
    pub(crate) fn from_store_for_test(store: StateStore) -> Self {
        Self::from_store(store)
    }

    /// Apply one inbound message, persisting + projecting the mutation and
    /// broadcasting any resulting events. `subscribe`/`command` return an
    /// optional immediate reply for the calling connection (the snapshot for
    /// `subscribe`).
    ///
    /// The `subscribe` reply is served from the **published snapshot** (refreshed
    /// after every applied mutation), not under the `store` mutex — so a slow
    /// durable append can never stall a subscribe. No delta is lost across the
    /// boundary: a connection attaches its broadcast receiver *before* it sends
    /// `subscribe` (see [`serve_ws_connection`]), so every post-attach delta is
    /// delivered on the stream; a delta already reflected in the snapshot and also
    /// re-delivered is idempotent for faces.
    ///
    /// A persistence error on a delta is logged and the delta is dropped; the
    /// durable append happens off the async worker (see [`Self::mutate`]) with the
    /// in-memory projection only committed on a successful append (or rolled back),
    /// so memory and the log never diverge.
    async fn apply(&self, msg: ClientMessage) -> Option<Event> {
        match msg {
            ClientMessage::Subscribe => Some(Event::snapshot(self.published_snapshot())),
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
                self.commit_or_log("session.remove", move |store| {
                    store.apply_session_remove(&session_id)
                })
                .await;
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
                self.commit_or_log("run.remove", move |store| {
                    store.apply_run_remove(&session_id, &run_id)
                })
                .await;
                None
            }
        }
    }

    /// Apply a face→Hub command and broadcast the resulting persisted events.
    /// Unknown commands are logged and accepted for forward compatibility. Every
    /// mutating arm runs off the async worker and publishes+broadcasts atomically
    /// via [`Self::commit_or_log`].
    async fn apply_command(&self, command: fleet_protocol::Command) {
        use fleet_protocol::{Command, Target};
        match command {
            Command::Focus { target, .. } => {
                self.commit_or_log("focus", move |store| {
                    let session_id = match target {
                        Target::Session { session_id } => Some(session_id),
                        Target::Run { run_id } => {
                            store.snapshot().into_iter().find_map(|session| {
                                session
                                    .runs
                                    .iter()
                                    .any(|run| run.run_id == run_id)
                                    .then_some(session.session_id)
                            })
                        }
                    };
                    // apply_focus swallows its own append error and never fails;
                    // normalize its `Option<Event>` into the broadcast-events vec.
                    Ok(session_id
                        .and_then(|sid| store.apply_focus(&sid))
                        .into_iter()
                        .collect())
                })
                .await;
            }
            Command::Mute { session_id, .. } => {
                self.commit_or_log("mute", move |store| store.apply_mute(&session_id))
                    .await;
            }
            Command::Unmute { session_id, .. } => {
                self.commit_or_log("unmute", move |store| store.apply_unmute(&session_id))
                    .await;
            }
            Command::Solo { session_id, .. } => {
                self.commit_or_log("solo", move |store| store.apply_solo(&session_id))
                    .await;
            }
            Command::Dismiss { target, .. } => self.apply_dismiss(target).await,
            other => log_unimplemented_command(&other),
        }
    }

    /// Handle a `dismiss` command (remove a session or a run) atomically via
    /// [`Self::commit_or_log`].
    async fn apply_dismiss(&self, target: fleet_protocol::Target) {
        use fleet_protocol::Target;
        match target {
            Target::Session { session_id } => {
                self.commit_or_log("dismiss session", move |store| {
                    store.apply_session_remove(&session_id)
                })
                .await;
            }
            Target::Run { run_id } => {
                self.commit_or_log("dismiss run", move |store| {
                    match store.snapshot().into_iter().find_map(|session| {
                        session
                            .runs
                            .iter()
                            .any(|run| run.run_id == run_id)
                            .then_some(session.session_id)
                    }) {
                        Some(session_id) => store.apply_run_remove(&session_id, &run_id),
                        None => Ok(Vec::new()), // run id present in no session: no-op
                    }
                })
                .await;
            }
        }
    }

    /// Current snapshot (used by tests and the unix/ws snapshot path). Served from
    /// the published snapshot, never blocked by an in-flight durable append.
    pub async fn snapshot_event(&self) -> Event {
        Event::snapshot(self.published_snapshot())
    }

    /// Persist + project + broadcast a session upsert. The public, transport-
    /// agnostic equivalent of receiving a `session.upsert` delta — used by the
    /// fake reporter and integration tests that drive the Hub without a socket.
    pub async fn ingest_session_upsert(&self, session: fleet_protocol::Session) {
        self.commit_or_log("session.upsert", move |store| {
            store.apply_session_upsert(session).map(|ev| vec![ev])
        })
        .await;
    }

    /// Persist + project + broadcast a run upsert (see [`Self::ingest_session_upsert`]).
    ///
    /// Un-stamped (S5 reporters): applied ungated, preserving prior behavior.
    pub async fn ingest_run_upsert(&self, session_id: &str, run: fleet_protocol::AgentRun) {
        let session_id = session_id.to_string();
        self.commit_or_log("run.upsert", move |store| {
            store.apply_run_upsert(&session_id, run)
        })
        .await;
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
        let session_id = session_id.to_string();
        self.commit_or_log("run.upsert", move |store| match stamp {
            Some(s) => {
                let did = crate::reclaim::DurableId::new(s.durable_id);
                store
                    .apply_run_upsert_seq(&session_id, run, &did, s.epoch, s.seq)
                    .map(|(_decision, evs)| evs)
            }
            None => store.apply_run_upsert(&session_id, run),
        })
        .await;
    }

    /// Apply a mutation **off the async worker** and, atomically with its
    /// projection, publish the new snapshot and broadcast its events.
    ///
    /// The closure `f` performs the durable append + in-memory projection inside
    /// [`StateStore`] while `store` is held on the blocking pool, so the slow
    /// SQLite I/O never runs on a tokio worker (T1.2) and the append-before-project
    /// ordering is preserved. Once it returns the events to broadcast, the fast
    /// `publish` lock is taken — **still holding `store`, so publishes/broadcasts
    /// stay in mutation order** — and the new snapshot is published + the events
    /// broadcast in one critical section. A [`Self::subscribe_with_snapshot`] that
    /// interleaves takes only `publish`, so it observes either the state-before
    /// (and its receiver, attached in the same section, will catch the broadcast)
    /// or the state-after (and the broadcast has already fired) — never a split
    /// view. On a persist error nothing is published or broadcast; the error is
    /// returned to the caller. Returns the number of events broadcast.
    async fn commit<F>(&self, f: F) -> Result<usize, PersistError>
    where
        F: FnOnce(&mut StateStore) -> Result<Vec<Event>, PersistError> + Send + 'static,
    {
        let store = self.store.clone();
        let publish = self.publish.clone();
        let handle = tokio::task::spawn_blocking(move || -> Result<usize, PersistError> {
            let mut guard = store.blocking_lock();
            let events = f(&mut guard)?;
            let snapshot = guard.snapshot();
            let mut p = publish.lock().unwrap_or_else(|e| e.into_inner());
            p.snapshot = snapshot;
            let n = events.len();
            for ev in events {
                let _ = p.tx.send(ev);
            }
            Ok(n)
        });
        join_blocking(handle).await
    }

    /// [`Self::commit`], logging (and dropping) a persist error under `context`
    /// rather than surfacing it — the log-and-drop policy for reporter/face deltas.
    async fn commit_or_log<F>(&self, context: &'static str, f: F)
    where
        F: FnOnce(&mut StateStore) -> Result<Vec<Event>, PersistError> + Send + 'static,
    {
        if let Err(e) = self.commit(f).await {
            tracing::error!(error = %e, op = context, "persist mutation failed; dropped");
        }
    }

    /// The current published read snapshot (a clone). Takes only the fast
    /// `publish` lock, so it is never blocked by an in-flight durable append.
    fn published_snapshot(&self) -> Vec<Session> {
        self.publish
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
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
        let now = now.to_string();
        self.commit(move |store| -> Result<Vec<Event>, PersistError> {
            let mut events = store.reap_dead(&now, grace)?;
            events.extend(store.sweep_expired_sessions(&now, session_ttl)?);
            Ok(events)
        })
        .await
    }

    /// Subscribe to the broadcast stream of applied deltas (no snapshot). Used by
    /// tests that only need to observe broadcasts; connections use
    /// [`Self::subscribe_with_snapshot`] for atomicity.
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.publish
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tx
            .subscribe()
    }

    /// Atomically read the current snapshot AND attach a broadcast receiver under
    /// the one `publish` lock. This is the subscribe-atomicity guarantee: the
    /// returned receiver will deliver **exactly** the broadcasts of every mutation
    /// ordered after this call, and the snapshot reflects **exactly** the
    /// mutations ordered before it — so a face's stream is `{snapshot} + {later
    /// broadcasts}` with every mutation appearing once, identical for all faces.
    fn subscribe_with_snapshot(&self) -> (Vec<Session>, broadcast::Receiver<Event>) {
        let p = self.publish.lock().unwrap_or_else(|e| e.into_inner());
        (p.snapshot.clone(), p.tx.subscribe())
    }
}

/// Await a `spawn_blocking` join, propagating a panic raised inside the blocking
/// store closure so it is not silently swallowed.
///
/// Coverage: the `Err(JoinError)` arm only fires if the store closure panics (a
/// bug no bounded test induces — the closures are infallible mutations that
/// return `Result` rather than panic), so it is excluded from the nightly gate;
/// a no-op on stable.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn join_blocking<R>(handle: tokio::task::JoinHandle<R>) -> R {
    handle.await.expect("store mutation task must not panic")
}

/// Log an accepted-but-not-yet-implemented face command.
///
/// Coverage: a pure diagnostic. The `debug!` argument-format region is gated out
/// at the default log level even when this arm runs, so it never registers as
/// covered; isolating it here keeps the call site (the `other` match arm) clean.
/// Excluded from the nightly gate; a no-op on stable.
#[cfg_attr(coverage_nightly, coverage(off))]
fn log_unimplemented_command(command: &fleet_protocol::Command) {
    tracing::debug!(
        command = command.command_name(),
        "command received (not yet implemented in this slice)"
    );
}

/// Encode an outbound [`Event`] as a JSON text frame.
fn encode(ev: &Event) -> Message {
    // Events are always serializable; fall back to an empty object on the
    // impossible error rather than panicking a connection task.
    let txt = serde_json::to_string(ev).unwrap_or_else(|_| "{}".to_string());
    Message::Text(txt.into())
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
    // The broadcast receiver is (re)attached ATOMICALLY with the snapshot when the
    // client sends `subscribe` (see `dispatch_client_message`), not at connection
    // time — so this face's stream is exactly {snapshot} + {broadcasts after it},
    // with no dependency on rx-attach-before-subscribe ordering. `rx` starts as a
    // placeholder receiver whose deltas are never forwarded until `subscribed`.
    let mut rx = state.subscribe();
    let mut subscribed = false;

    loop {
        tokio::select! {
            // Inbound client frame.
            incoming = source.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<ClientMessage>(&txt) {
                            Ok(msg) => {
                                if !dispatch_client_message(
                                    &state, msg, &mut sink, &mut rx, &mut subscribed,
                                ).await {
                                    break;
                                }
                            }
                            Err(e) => tracing::warn!(error = %e, "undecodable client message"),
                        }
                    }
                    Some(Ok(Message::Binary(bin))) => {
                        match serde_json::from_slice::<ClientMessage>(&bin) {
                            Ok(msg) => {
                                if !dispatch_client_message(
                                    &state, msg, &mut sink, &mut rx, &mut subscribed,
                                ).await {
                                    break;
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
            // Outbound broadcast delta — only once the client has subscribed (so
            // pre-subscribe broadcasts are never forwarded ahead of the snapshot).
            ev = rx.recv(), if subscribed => {
                if forward_broadcast(ev, &mut sink).await { break; }
            }
        }
    }
}

/// Handle one decoded inbound client message on a connection. Returns `true` to
/// keep the connection open, `false` to close it (a sink write failed).
///
/// `subscribe` is special-cased here so the snapshot reply and the broadcast
/// receiver are obtained ATOMICALLY ([`HubState::subscribe_with_snapshot`]) and
/// the receiver replaces `rx` — this is the subscribe-atomicity join point. Every
/// other message is a mutation/no-op routed through [`HubState::apply`].
async fn dispatch_client_message<Si>(
    state: &HubState,
    msg: ClientMessage,
    sink: &mut Si,
    rx: &mut broadcast::Receiver<Event>,
    subscribed: &mut bool,
) -> bool
where
    Si: SinkExt<Message> + Unpin,
{
    match msg {
        ClientMessage::Subscribe => {
            let (snapshot, receiver) = state.subscribe_with_snapshot();
            *rx = receiver;
            *subscribed = true;
            sink.send(encode(&Event::snapshot(snapshot))).await.is_ok()
        }
        other => match state.apply(other).await {
            Some(reply) => sink.send(encode(&reply)).await.is_ok(),
            None => true,
        },
    }
}

/// Forward one broadcast result to a connection's sink. Returns `true` when the
/// connection should close (a send error, or the channel closed).
///
/// Coverage: the `Ok` (deliver) and `Lagged` arms are covered (the
/// `reporter_push_broadcasts_outbound_over_socket` and
/// `slow_subscriber_lags_when_backlog_overflows` tests), but the
/// `RecvError::Closed` arm is unreachable: `serve_ws_connection` owns a
/// `HubState` clone (hence a live broadcast `Sender`) for the connection's entire
/// life, so the channel can never close under it. Excluded from the nightly gate
/// to keep that one defensive arm from showing as uncovered; a no-op on stable.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn forward_broadcast<Si>(
    ev: Result<Event, broadcast::error::RecvError>,
    sink: &mut Si,
) -> bool
where
    Si: SinkExt<Message> + Unpin,
{
    match ev {
        Ok(ev) => sink.send(encode(&ev)).await.is_err(),
        Err(broadcast::error::RecvError::Lagged(n)) => {
            tracing::warn!(
                skipped = n,
                "subscriber lagged; deltas dropped (re-subscribe for snapshot)"
            );
            false
        }
        Err(broadcast::error::RecvError::Closed) => true,
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
    Ok((local, ws_accept_loop(state, listener)))
}

/// The forever accept loop for the WS listener. Spawns a `serve_ws_connection`
/// task per accepted stream.
///
/// Coverage: this is a daemon-forever serve loop. Its connection-serving logic
/// is covered directly by `serve_ws_connection` tests (over an in-process
/// duplex). The loop never returns, and the `Err` arm fires only on an OS accept
/// failure (e.g. EMFILE) that no deterministic, root-safe test can induce, so the
/// whole loop is excluded from the nightly coverage gate (a no-op on stable).
#[cfg_attr(coverage_nightly, coverage(off))]
async fn ws_accept_loop(state: HubState, listener: TcpListener) {
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
    Ok(unix_accept_loop(state, listener))
}

/// The forever accept loop for the unix listener (`cfg(unix)`). See
/// [`ws_accept_loop`] — same rationale for the nightly coverage exclusion: the
/// serve logic is tested directly, and the `Err` accept arm is uninducible.
#[cfg(unix)]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn unix_accept_loop(state: HubState, listener: tokio::net::UnixListener) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
        ServerKind, Session, State,
    };

    // Event/Message unwrap helpers. The happy-path assertions stay at the call
    // sites; only the variant-mismatch `panic!` arm lives here, in functions
    // excluded from the nightly gate so it never shows as uncovered (a no-op on
    // stable).
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_session_updated(ev: Event) -> Session {
        match ev {
            Event::SessionUpdated { session, .. } => session,
            other => panic!("expected session.updated, got {other:?}"),
        }
    }
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_session_removed(ev: Event) -> String {
        match ev {
            Event::SessionRemoved { session_id, .. } => session_id,
            other => panic!("expected session.removed, got {other:?}"),
        }
    }
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_text(msg: Message) -> String {
        match msg {
            Message::Text(txt) => txt.to_string(),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
    /// Whether `ev` is a `session.updated` for `id` with `soloed = true`. Excluded
    /// from the nightly gate so the non-`SessionUpdated` drain arm (untaken in the
    /// solo test) never shows as uncovered (no-op on stable).
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn is_soloed_update(ev: &Event, id: &str) -> bool {
        match ev {
            Event::SessionUpdated { session, .. } => session.session_id == id && session.soloed,
            _ => false,
        }
    }

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
        // A genuine waiting run gives a legitimate Waiting rollup and arms unread
        // (a run-less session normalizes to the idle sentinel).
        let mut session = sess("s1");
        session.runs.push(AgentRun::new(
            "r1",
            AgentKind::Codex,
            "n",
            "/",
            State::Waiting,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        ));
        state.apply(ClientMessage::SessionUpsert { session }).await;
        let mut rx = state.subscribe();

        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::focus(fleet_protocol::Target::session("s1")),
            })
            .await;

        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.updated");
        let session = expect_session_updated(ev);
        assert!(!session.unread, "focus must clear unread");
        assert_eq!(session.rollup_state, State::Waiting);

        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            Event::Snapshot { sessions, .. } => assert!(!sessions[0].unread),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn focus_unknown_run_resolves_to_no_session_and_broadcasts_nothing() {
        // Focus-by-run targeting a run id present in no session: the find_map
        // resolves to None, so the `if let Some(session_id)` no-op branch is taken.
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::focus(fleet_protocol::Target::run("nope")),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "focusing an unknown run must not broadcast"
        );
    }

    #[tokio::test]
    async fn focus_already_read_session_broadcasts_nothing() {
        // Focus on a session with no unread badge: apply_focus returns None, so the
        // inner `if let Some(ev)` no-op branch is taken (no broadcast).
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            }) // unread=false
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::focus(fleet_protocol::Target::session("s1")),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "focusing an already-read session must not broadcast"
        );
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
        assert_eq!(expect_session_removed(ev), "s1");

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
        assert!(
            expect_session_updated(ev).muted,
            "muted flag must be set in broadcast event"
        );

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

    /// A [`HubState`] backed by a READ-ONLY on-disk store pre-populated with a
    /// session + waiting run, so every mutation projects in memory but its durable
    /// `log.append` fails — driving the persist-failure error arms deterministically
    /// (root-safe; no chmod). Returns the tempdir guard too (must outlive the state).
    fn read_only_state() -> (tempfile::TempDir, HubState) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut store = crate::persist::StateStore::open(&path).unwrap();
            let mut s = sess("s1");
            s.runs.push(AgentRun::new(
                "r1",
                AgentKind::Codex,
                "native",
                "/",
                State::Working,
                Confidence::High,
                "2026-06-08T00:00:00Z",
            ));
            store.apply_session_upsert(s).unwrap();
        }
        let store = crate::persist::StateStore::open_read_only_for_test(&path).unwrap();
        (dir, HubState::from_store_for_test(store))
    }

    #[tokio::test]
    async fn persist_failure_paths_are_logged_and_dropped() {
        // Drive every server-side persist-failure arm: each mutation is attempted
        // against a read-only log, so the append fails, the error is logged, and
        // nothing is broadcast (the in-memory projection is only updated on a
        // successful append for these paths).
        let (_dir, state) = read_only_state();
        // The TRACE subscriber makes the error! format regions execute. We run the
        // async applies inside the scoped subscriber via block_in_place is overkill;
        // instead set a default for this thread for the duration.
        let _guard = {
            use tracing::level_filters::LevelFilter;
            let sub = tracing_subscriber::fmt()
                .with_max_level(LevelFilter::TRACE)
                .with_test_writer()
                .finish();
            tracing::subscriber::set_default(sub)
        };
        let mut rx = state.subscribe();

        // session.remove (server.rs ~114) — append fails, Err arm logs.
        state
            .apply(ClientMessage::SessionRemove {
                session_id: "s1".into(),
            })
            .await;
        // run.remove (server.rs ~135).
        state
            .apply(ClientMessage::RunRemove {
                session_id: "s1".into(),
                run_id: "r1".into(),
            })
            .await;
        // session.upsert via ingest (server.rs ~249).
        state.ingest_session_upsert(sess("s2")).await;
        // run.upsert un-stamped via ingest (server.rs ~264) and stamped (~295).
        state
            .ingest_run_upsert(
                "s1",
                AgentRun::new(
                    "r9",
                    AgentKind::Codex,
                    "n",
                    "/",
                    State::Idle,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
            )
            .await;
        state
            .ingest_run_upsert_stamped(
                "s1",
                AgentRun::new(
                    "r8",
                    AgentKind::Codex,
                    "native",
                    "/",
                    State::Idle,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
                Some(crate::wire::SeqStamp::new("native", 0, 1)),
            )
            .await;
        // dismiss session (server.rs ~193) and dismiss run (~214).
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::session("s1")),
            })
            .await;
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::run("r1")),
            })
            .await;

        // None of the failed appends broadcast anything.
        assert!(
            rx.try_recv().is_err(),
            "a persist failure must drop the delta, never broadcast it"
        );
    }

    #[tokio::test]
    async fn session_remove_message_removes_and_broadcasts() {
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();
        let reply = state
            .apply(ClientMessage::SessionRemove {
                session_id: "s1".into(),
            })
            .await;
        assert!(reply.is_none(), "remove has no immediate reply");
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.type_name(), "session.removed");
        // The state is gone.
        let snap = state.apply(ClientMessage::Subscribe).await.unwrap();
        match snap {
            Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_remove_of_absent_session_broadcasts_nothing() {
        let state = HubState::new();
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::SessionRemove {
                session_id: "ghost".into(),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "removing an absent session must not broadcast"
        );
    }

    #[tokio::test]
    async fn run_remove_message_removes_and_updates_session() {
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
                    State::Working,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
                stamp: None,
            })
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::RunRemove {
                session_id: "s1".into(),
                run_id: "r1".into(),
            })
            .await;
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert_eq!(first.type_name(), "run.removed");
        assert_eq!(second.type_name(), "session.updated");
    }

    #[tokio::test]
    async fn run_remove_of_absent_run_broadcasts_nothing() {
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::RunRemove {
                session_id: "s1".into(),
                run_id: "ghost".into(),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "removing an absent run must not broadcast"
        );
    }

    #[tokio::test]
    async fn dismiss_run_with_unknown_run_id_broadcasts_nothing() {
        // Dismiss targeting a run id that exists in no session: the find_map
        // resolves to None and nothing is removed/broadcast.
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::dismiss(fleet_protocol::Target::run("nope")),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "dismissing an unknown run must not broadcast"
        );
    }

    #[tokio::test]
    async fn unimplemented_command_is_accepted_silently() {
        // A command we don't handle in this slice (set_tags) hits the catch-all
        // `other` arm: accepted, no broadcast.
        let state = HubState::new();
        state
            .apply(ClientMessage::SessionUpsert {
                session: sess("s1"),
            })
            .await;
        let mut rx = state.subscribe();
        state
            .apply(ClientMessage::Command {
                command: fleet_protocol::Command::set_tags("s1", vec!["x".into()]),
            })
            .await;
        assert!(
            rx.try_recv().is_err(),
            "an unimplemented command must not broadcast"
        );
    }

    #[tokio::test]
    async fn ingest_run_upsert_unstamped_broadcasts() {
        // The public un-stamped (S5) ingest path used by the fake reporter.
        let state = HubState::new();
        state.ingest_session_upsert(sess("s1")).await;
        let mut rx = state.subscribe();
        state
            .ingest_run_upsert(
                "s1",
                AgentRun::new(
                    "r1",
                    AgentKind::Codex,
                    "n",
                    "/",
                    State::Working,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
            )
            .await;
        // run.added then session.updated.
        let a = rx.recv().await.unwrap();
        let b = rx.recv().await.unwrap();
        assert_eq!(a.type_name(), "run.added");
        assert_eq!(b.type_name(), "session.updated");
    }

    #[tokio::test]
    async fn ingest_run_upsert_stamped_gates_duplicates() {
        let state = HubState::new();
        state.ingest_session_upsert(sess("s1")).await;
        let run = AgentRun::new(
            "r1",
            AgentKind::Codex,
            "native",
            "/",
            State::Working,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        let stamp = crate::wire::SeqStamp::new("native", 0, 5);
        state
            .ingest_run_upsert_stamped("s1", run.clone(), Some(stamp.clone()))
            .await;
        let mut rx = state.subscribe();
        // A duplicate stamp (same seq) is gated out — nothing broadcast.
        state
            .ingest_run_upsert_stamped("s1", run, Some(stamp))
            .await;
        assert!(
            rx.try_recv().is_err(),
            "a stamped duplicate must be gated out (no broadcast)"
        );
    }

    #[tokio::test]
    async fn snapshot_event_reflects_current_state() {
        let state = HubState::new();
        state.ingest_session_upsert(sess("s1")).await;
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session_id, "s1");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gc_reaps_dead_runs_and_returns_count() {
        let state = HubState::new();
        state.ingest_session_upsert(sess("s1")).await;
        state
            .ingest_run_upsert(
                "s1",
                AgentRun::new(
                    "r1",
                    AgentKind::Codex,
                    "n",
                    "/",
                    State::Dead,
                    Confidence::High,
                    "2026-06-08T00:00:00Z",
                ),
            )
            .await;
        // Reap well past a 1s grace at a far-future `now`.
        let n = state
            .gc(
                "2026-06-09T00:00:00Z",
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(48 * 3600),
            )
            .await
            .unwrap();
        assert!(n >= 1, "gc must report the reaped run.removed (+update)");
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                assert!(sessions[0].runs.is_empty(), "dead run reaped");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[test]
    fn encode_serializes_event_as_text_frame() {
        let ev = Event::snapshot(Vec::new());
        let txt = expect_text(encode(&ev));
        let back: Event = serde_json::from_str(&txt).unwrap();
        assert_eq!(back.type_name(), "fleet.snapshot");
    }

    #[tokio::test]
    async fn default_hubstate_is_fresh_empty() {
        // The Default impl just delegates to new(); confirm it yields empty state.
        let state = HubState::default();
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected snapshot, got {other:?}"),
        }
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
            saw_s1_solo |= is_soloed_update(&ev, "s1");
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

    // ── T1.2: blocking append offloaded + snapshot lock narrowed ──────────────

    /// A Hub whose durable append sleeps `delay` before each write (slow disk),
    /// used to exercise the blocking-offload + published-snapshot paths.
    fn slow_hub(delay: std::time::Duration) -> HubState {
        let store = crate::persist::StateStore::open_in_memory_slow(delay).unwrap();
        HubState::from_store_for_test(store)
    }

    #[tokio::test]
    async fn mutations_apply_in_order_through_offloaded_writes() {
        // Sequentially-awaited mutations must still land in order once the blocking
        // append is offloaded (the append-before-project ordering is preserved).
        let state = HubState::new();
        for i in 0..6 {
            state.ingest_session_upsert(sess(&format!("s{i}"))).await;
        }
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                let ids: Vec<_> = sessions.iter().map(|s| s.session_id.clone()).collect();
                assert_eq!(ids, vec!["s0", "s1", "s2", "s3", "s4", "s5"]);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_snapshot_not_blocked_by_in_flight_append() {
        use std::time::{Duration, Instant};
        // A durable append that takes 200ms (slow disk).
        let state = slow_hub(Duration::from_millis(200));

        // Start a slow mutation and let it enter the in-flight (offloaded) append.
        let start = Instant::now();
        let s = state.clone();
        let mutation = tokio::spawn(async move { s.ingest_session_upsert(sess("slow")).await });
        tokio::task::yield_now().await;

        // A snapshot read must return promptly: it is served from the published
        // snapshot, so it neither waits on the store lock the append is holding
        // (lock narrowed) nor is starved by the append (offloaded off the worker).
        // The elapsed time is measured from BEFORE the mutation was spawned, so a
        // regression in EITHER the offload or the lock-narrowing would push it to
        // ~200ms.
        let _ = state.snapshot_event().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(120),
            "snapshot blocked by the in-flight durable append ({elapsed:?})"
        );

        // The slow mutation still completes and durably commits.
        mutation.await.unwrap();
        match state.snapshot_event().await {
            Event::Snapshot { sessions, .. } => {
                assert!(
                    sessions.iter().any(|x| x.session_id == "slow"),
                    "the offloaded mutation must still commit"
                );
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

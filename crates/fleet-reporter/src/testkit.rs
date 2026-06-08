//! Deterministic in-memory transport for testing the reporter framework
//! (PLAN S5). Lets tests script connect failures, mid-flush connection drops,
//! and inspect the exact ordered stream of frames the reporter delivered.
//!
//! This is compiled into the library (not gated behind `#[cfg(test)]`) so that
//! both the in-crate unit tests *and* the crate's integration tests
//! (`tests/*.rs`) can drive the reporter without a real Hub. It is `#[doc(hidden)]`
//! because it is a test seam, not part of the supported public API.

#![doc(hidden)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use fleet_hub::wire::ClientMessage;

use crate::transport::{BoxFuture, Connection, Connector, TransportError};

/// Shared, observable state of a [`MemoryConnector`]. Cloneable handle so a test
/// can both hand the connector to a reporter and inspect what was delivered.
#[derive(Clone, Default)]
pub struct MemoryHub {
    inner: Arc<Mutex<MemoryHubInner>>,
}

#[derive(Default)]
struct MemoryHubInner {
    /// Every message the reporter successfully delivered, in order, across all
    /// connections (the Hub's view of the ordered stream).
    delivered: Vec<ClientMessage>,
    /// Scripted outcomes for upcoming `connect()` calls. `false` = fail.
    /// Empties from the front; once empty, connects succeed by default.
    connect_script: VecDeque<bool>,
    /// If set to `Some(n)`, the next opened connection drops (send fails) after
    /// `n` successful sends. Decremented per opened connection.
    drop_after_sends: Option<usize>,
    /// Count of connect() calls (including failed ones).
    connect_attempts: usize,
    /// Count of successful connects.
    connect_successes: usize,
}

impl MemoryHub {
    /// A fresh hub whose connects all succeed and deliver everything.
    pub fn new() -> Self {
        MemoryHub::default()
    }

    /// Script the next `connect()` calls: `true` succeeds, `false` fails.
    /// Calls beyond the script length succeed.
    pub fn script_connects(&self, outcomes: impl IntoIterator<Item = bool>) {
        let mut g = self.inner.lock().unwrap();
        g.connect_script = outcomes.into_iter().collect();
    }

    /// Make the next opened connection drop after `n` successful sends (the
    /// `n+1`-th send fails, modeling a mid-flush Hub crash).
    pub fn drop_next_connection_after(&self, n: usize) {
        self.inner.lock().unwrap().drop_after_sends = Some(n);
    }

    /// All messages delivered so far, in order.
    pub fn delivered(&self) -> Vec<ClientMessage> {
        self.inner.lock().unwrap().delivered.clone()
    }

    /// Number of delivered messages.
    pub fn delivered_count(&self) -> usize {
        self.inner.lock().unwrap().delivered.len()
    }

    /// Total connect attempts (success + failure).
    pub fn connect_attempts(&self) -> usize {
        self.inner.lock().unwrap().connect_attempts
    }

    /// Successful connects.
    pub fn connect_successes(&self) -> usize {
        self.inner.lock().unwrap().connect_successes
    }

    /// Build a [`Connector`] backed by this hub.
    pub fn connector(&self) -> MemoryConnector {
        MemoryConnector { hub: self.clone() }
    }
}

/// A [`Connector`] over a [`MemoryHub`].
pub struct MemoryConnector {
    hub: MemoryHub,
}

impl Connector for MemoryConnector {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        let hub = self.hub.clone();
        Box::pin(async move {
            let drop_after = {
                let mut g = hub.inner.lock().unwrap();
                g.connect_attempts += 1;
                let ok = g.connect_script.pop_front().unwrap_or(true);
                if !ok {
                    return Err(TransportError::Connect("scripted failure".into()));
                }
                g.connect_successes += 1;
                g.drop_after_sends.take()
            };
            Ok(Box::new(MemoryConnection {
                hub,
                remaining_before_drop: drop_after,
            }) as Box<dyn Connection>)
        })
    }

    fn endpoint(&self) -> String {
        "memory://hub".into()
    }
}

/// A single in-memory connection. Appends delivered messages to the shared hub.
pub struct MemoryConnection {
    hub: MemoryHub,
    /// `Some(0)` means the next send fails; `Some(n)` decrements; `None` = never
    /// drops on its own.
    remaining_before_drop: Option<usize>,
}

impl Connection for MemoryConnection {
    fn send<'a>(&'a mut self, msg: &'a ClientMessage) -> BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if let Some(rem) = self.remaining_before_drop {
                if rem == 0 {
                    return Err(TransportError::Send("scripted mid-flush drop".into()));
                }
                self.remaining_before_drop = Some(rem - 1);
            }
            self.hub.inner.lock().unwrap().delivered.push(msg.clone());
            Ok(())
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{AgentKind, AgentRun, Confidence, State};

    fn msg(id: &str) -> ClientMessage {
        ClientMessage::RunUpsert {
            session_id: "s".into(),
            run: AgentRun::new(
                id,
                AgentKind::Codex,
                "n",
                "/",
                State::Working,
                Confidence::High,
                "2026-06-08T00:00:00Z",
            ),
            stamp: None,
        }
    }

    #[tokio::test]
    async fn delivers_in_order() {
        let hub = MemoryHub::new();
        let conn_factory = hub.connector();
        let mut c = conn_factory.connect().await.unwrap();
        c.send(&msg("a")).await.unwrap();
        c.send(&msg("b")).await.unwrap();
        assert_eq!(hub.delivered_count(), 2);
    }

    #[tokio::test]
    async fn scripted_connect_failure() {
        let hub = MemoryHub::new();
        hub.script_connects([false, false, true]);
        let c = hub.connector();
        assert!(c.connect().await.is_err());
        assert!(c.connect().await.is_err());
        assert!(c.connect().await.is_ok());
        assert_eq!(hub.connect_attempts(), 3);
        assert_eq!(hub.connect_successes(), 1);
    }

    #[tokio::test]
    async fn drop_mid_flush() {
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(1);
        let c = hub.connector();
        let mut conn = c.connect().await.unwrap();
        assert!(conn.send(&msg("a")).await.is_ok());
        assert!(conn.send(&msg("b")).await.is_err(), "drops after 1 send");
        assert_eq!(hub.delivered_count(), 1);
    }
}

//! The Hub link: subscribe to the Hub over WebSocket, fold its
//! `fleet.snapshot` + delta stream into the **real** [`fleet_host_core::InboxModel`]
//! reducer, and push every resulting view to the webview (managed state for the
//! initial `get_inbox` pull, plus a live `inbox` event on every change).
//!
//! Reconnects with a fixed backoff; a dropped link shows the last known tabs with
//! `connected:false` rather than blanking the window. This is the exact wire +
//! reducer the CLI and e2e faces use — nothing is faked.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter};
use tokio_tungstenite::tungstenite::Message;

use fleet_host_core::InboxModel;
use fleet_protocol::Event;

use crate::render::{render, RenderedInbox};

/// Shared latest-rendered-inbox, read by the `get_inbox` command on webview load.
pub type Shared = Arc<Mutex<RenderedInbox>>;

/// Run the Hub link forever: connect, subscribe, fold, push; reconnect on drop.
pub async fn run(app: AppHandle, shared: Shared, ws_url: String) {
    loop {
        if let Err(e) = connect_once(&app, &shared, &ws_url).await {
            tracing::warn!(error = %e, url = %ws_url, "hub link error; retrying");
        }
        // Link is down — show the last tabs as disconnected, then back off.
        mark_disconnected(&app, &shared);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn connect_once(app: &AppHandle, shared: &Shared, ws_url: &str) -> Result<()> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(ws_url).await?;
    ws.send(Message::Text(r#"{"type":"subscribe"}"#.into()))
        .await?;
    tracing::info!(url = %ws_url, "host face connected to Hub; subscribed");

    // A fresh model per connection: the Hub resends a full snapshot on subscribe,
    // so reconnection reconciles rather than accumulating stale state.
    let mut model = InboxModel::new();

    while let Some(msg) = ws.next().await {
        let ev = match msg? {
            Message::Text(txt) => serde_json::from_str::<Event>(&txt).ok(),
            Message::Binary(bin) => serde_json::from_slice::<Event>(&bin).ok(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => None,
            _ => None,
        };
        if let Some(ev) = ev {
            model.apply(ev);
            push(app, shared, render(&model.view(), true));
        }
    }
    Ok(())
}

/// Store the rendered inbox in shared state and emit it to the webview.
fn push(app: &AppHandle, shared: &Shared, rendered: RenderedInbox) {
    // Observability: the window renders from this exact payload, so logging it
    // proves (without needing to inspect the webview) what the window shows.
    let summary: Vec<String> = rendered
        .tabs
        .iter()
        .map(|t| format!("{}={}", t.title, t.state))
        .collect();
    tracing::info!(
        connected = rendered.connected,
        tabs = rendered.tabs.len(),
        waiting = rendered.waiting_count,
        "inbox → window: [{}]",
        summary.join(", ")
    );
    if let Ok(mut g) = shared.lock() {
        *g = rendered.clone();
    }
    let _ = app.emit("inbox", rendered);
}

/// Flip the last-known inbox to `connected:false` (keep the tabs visible).
fn mark_disconnected(app: &AppHandle, shared: &Shared) {
    let mut snapshot = shared.lock().ok().map(|g| g.clone()).unwrap_or_default();
    if snapshot.connected {
        snapshot.connected = false;
        push(app, shared, snapshot);
    }
}

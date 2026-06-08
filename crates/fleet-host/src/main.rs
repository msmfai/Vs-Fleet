//! Fleet host face — the Tauri sidebar window (the primary v1 GUI).
//!
//! A real window that subscribes to the Hub over WebSocket, folds its event
//! stream into the **real** `fleet-host-core` reducer, and renders the live
//! inbox. The Hub URL comes from `FLEET_HUB_URL` (default `ws://127.0.0.1:51777`,
//! the Hub's WS port) or the first CLI argument.
//!
//! ## Shape
//! - A background thread runs a single-threaded tokio runtime hosting the Hub
//!   link ([`hub_client`]); Tauri keeps the main thread for the window event loop.
//! - The link pushes each rendered inbox into Tauri **managed state** (read once
//!   by the `get_inbox` command on webview load) and **emits** an `inbox` event
//!   for every live update. The static frontend (no bundler — `withGlobalTauri`)
//!   invokes `get_inbox` then `listen("inbox", …)`.

mod hub_client;
mod render;

use std::sync::{Arc, Mutex};

use render::RenderedInbox;
use tracing_subscriber::EnvFilter;

/// The webview's initial pull of current inbox state (live updates arrive via the
/// `inbox` event). App-defined command — not gated by the v2 capability ACL.
#[tauri::command]
fn get_inbox(state: tauri::State<'_, hub_client::Shared>) -> RenderedInbox {
    state.lock().map(|g| g.clone()).unwrap_or_default()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let ws_url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("FLEET_HUB_URL").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ws://127.0.0.1:51777".to_string());

    let shared: hub_client::Shared = Arc::new(Mutex::new(RenderedInbox::default()));

    tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![get_inbox])
        .setup(move |app| {
            let handle = app.handle().clone();
            let shared = shared.clone();
            let ws_url = ws_url.clone();
            // The Hub link lives on its own thread with its own tokio runtime so
            // Tauri owns the main thread for the window event loop.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build hub-link runtime");
                rt.block_on(hub_client::run(handle, shared, ws_url));
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Fleet host");
}

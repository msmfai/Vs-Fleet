//! Fleet host face — the Tauri sidebar window (the primary v1 GUI).
//!
//! A real window that starts Fleet's local push endpoint, subscribes to it over
//! WebSocket, folds its event stream into the **real** `fleet-host-core` reducer,
//! and renders the live inbox. An explicit first CLI argument or `FLEET_HUB_URL`
//! makes the app connect to an external Hub instead.
//!
//! ## Shape
//! - A background thread runs a single-threaded tokio runtime hosting the local
//!   Hub, Hub link ([`hub_client`]), and bridge phone-home server; Tauri keeps
//!   the main thread for the window event loop.
//! - The link pushes each rendered inbox into Tauri **managed state** (read once
//!   by the `get_inbox` command on webview load) and **emits** an `inbox` event
//!   for every live update. The static frontend (no bundler — `withGlobalTauri`)
//!   invokes `get_inbox` then `listen("inbox", …)`.

mod bridge;
mod hub_client;
mod mux;
mod render;
mod spawn;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use render::RenderedInbox;
use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

const DEFAULT_HUB_URL: &str = "ws://127.0.0.1:51777";

fn default_hub_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], fleet_hub::DEFAULT_WS_PORT))
}

/// Fixed loopback port for the command-bridge WS server, so each code-server can
/// be launched with `FLEET_BRIDGE_URL=ws://127.0.0.1:<this>` before Fleet starts.
const BRIDGE_PORT: u16 = 51778;

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

    let explicit_ws_url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("FLEET_HUB_URL").ok())
        .filter(|s| !s.is_empty());
    let start_local_hub = explicit_ws_url.is_none();
    let ws_url = explicit_ws_url.unwrap_or_else(|| DEFAULT_HUB_URL.to_string());

    let shared: hub_client::Shared = Arc::new(Mutex::new(RenderedInbox::default()));
    let registry = bridge::BridgeRegistry::new();

    tauri::Builder::default()
        .manage(shared.clone())
        .manage(mux::MuxState::new())
        .manage(registry.clone())
        .manage(spawn::ServerSupervisor::new(BRIDGE_PORT, ws_url.clone()))
        .invoke_handler(tauri::generate_handler![
            get_inbox,
            mux::get_servers,
            mux::selected_server,
            mux::select_server,
            mux::spawn_server,
            mux::close_server
        ])
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "spawn:new" {
                if let Some(sup) = app.try_state::<spawn::ServerSupervisor>() {
                    if let Ok(server) = sup.spawn() {
                        let _ = app.emit(bridge::SERVERS_CHANGED, ());
                        mux::select_spawned(app.clone(), server.id);
                    }
                }
            } else if id == "spawn:close-current" {
                if let (Some(sup), Some(mux)) = (
                    app.try_state::<spawn::ServerSupervisor>(),
                    app.try_state::<mux::MuxState>(),
                ) {
                    if let Some(active) = mux.selected.lock().ok().and_then(|g| g.clone()) {
                        sup.close(&active);
                    }
                }
            } else if let Some(server_id) = id.strip_prefix("server:") {
                mux::select(app, server_id.to_string());
            } else if let Some(command) = id.strip_prefix("cmd:") {
                // Forward a VS Code command to the active server's bridge.
                if let (Some(mux), Some(reg)) = (
                    app.try_state::<mux::MuxState>(),
                    app.try_state::<bridge::BridgeRegistry>(),
                ) {
                    if let Some(active) = mux.selected.lock().ok().and_then(|g| g.clone()) {
                        reg.send_command(&active, command);
                    }
                }
            }
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
                app.set_dock_visibility(true);
            }

            let handle = app.handle().clone();
            let bridge_handle = app.handle().clone();
            let shared = shared.clone();
            let ws_url = ws_url.clone();
            let registry = registry.clone();
            // The embedded Hub, Hub link, and command-bridge / phone-home server
            // share one background runtime so Tauri owns the main thread.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build background runtime");
                rt.block_on(async move {
                    if start_local_hub {
                        tokio::spawn(async {
                            let mut config = fleet_hub::HubConfig::default();
                            config.ws_addr = default_hub_addr();
                            config.persist = true;
                            match fleet_hub::run(config).await {
                                Ok(()) => tracing::info!("embedded Fleet Hub stopped"),
                                Err(e) if e.downcast_ref::<fleet_hub::LockError>().is_some() => {
                                    tracing::info!(error = %e, "external Fleet Hub already running; using it")
                                }
                                Err(e) => tracing::error!(error = %e, "embedded Fleet Hub failed"),
                            }
                        });
                        // Give the embedded listener a head start; the client also
                        // reconnects, so this is only to avoid a visible first-frame
                        // disconnected state on normal launches.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    if let Err(e) = bridge::serve(bridge_handle, registry, BRIDGE_PORT).await {
                        tracing::error!(error = %e, "bridge server failed to bind");
                    }
                    hub_client::run(handle, shared, ws_url).await;
                });
            });

            mux::build_window(app)?;
            mux::build_menu(app)?;

            // Test harness hook: auto-spawn N servers on startup so an integration
            // test can drive Fleet without clicking (`FLEET_AUTOSPAWN=n`).
            if let Some(n) = std::env::var("FLEET_AUTOSPAWN")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            {
                if let Some(sup) = app.try_state::<spawn::ServerSupervisor>() {
                    for _ in 0..n {
                        if let Ok(server) = sup.spawn() {
                            let _ = app.emit(bridge::SERVERS_CHANGED, ());
                            mux::select_spawned(app.handle().clone(), server.id);
                        }
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Fleet host");
}

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

// Enable the `#[coverage(off)]` attribute under cargo-llvm-cov's nightly gate
// (a no-op on stable). Lets genuine GUI/FFI/daemon glue — the Tauri builder, the
// forever event loop, webview/IPC FFI — drop out of the line-coverage bar while
// the pure logic stays fully tested.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod bridge;
mod hub_client;
mod mux;
mod render;
mod spawn;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use render::RenderedInbox;
use tauri::{Emitter, Manager};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

const DEFAULT_HUB_URL: &str = "ws://127.0.0.1:51777";

fn default_hub_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], fleet_hub::DEFAULT_WS_PORT))
}

// Thin env wrapper around the tested `embedded_hub_runtime_dir_from`; reading
// process env (HOME/FLEET_RUNTIME_DIR) can't be done without racing the other
// HOME-mutating tests, so the pure resolver carries the logic.
#[cfg_attr(coverage_nightly, coverage(off))]
fn embedded_hub_runtime_dir() -> PathBuf {
    embedded_hub_runtime_dir_from(
        std::env::var_os("FLEET_RUNTIME_DIR").map(PathBuf::from),
        std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
            .map(PathBuf::from),
    )
}

fn embedded_hub_runtime_dir_from(override_dir: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = override_dir.filter(|d| !d.as_os_str().is_empty()) {
        return dir;
    }
    home.unwrap_or_else(std::env::temp_dir)
        .join(".fleet")
        .join("run")
}

// Thin env wrapper; derives from the env-reading `embedded_hub_runtime_dir`
// (excluded). The pure join is covered via `host_log_path_from_runtime_dir`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn host_log_path() -> PathBuf {
    embedded_hub_runtime_dir().join("fleet-host.log")
}

#[cfg(test)]
fn host_log_path_from_runtime_dir(runtime_dir: PathBuf) -> PathBuf {
    runtime_dir.join("fleet-host.log")
}

#[derive(Clone)]
struct FleetLogWriter {
    file: Arc<Mutex<std::fs::File>>,
}

struct FleetLogGuard {
    file: Arc<Mutex<std::fs::File>>,
}

impl Write for FleetLogGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        match self.file.lock() {
            Ok(mut file) => file.write(buf),
            Err(_) => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Ok(mut file) = self.file.lock() {
            file.flush()?;
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for FleetLogWriter {
    type Writer = FleetLogGuard;

    fn make_writer(&'a self) -> Self::Writer {
        FleetLogGuard {
            file: self.file.clone(),
        }
    }
}

// Glue: installs the global tracing subscriber (a process-wide singleton that
// can only be initialized once, so it can't be exercised per-test) and opens the
// host log file. The log tee it wires up (`FleetLogWriter`) is tested directly.
#[cfg_attr(coverage_nightly, coverage(off))]
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let path = host_log_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Fleet host log directory could not be created: {e}");
        }
    }

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(FleetLogWriter {
                    file: Arc::new(Mutex::new(file)),
                })
                .init();
            tracing::info!(path = %path.display(), "Fleet host logging initialized");
        }
        Err(e) => {
            eprintln!(
                "Fleet host log file could not be opened at {}: {e}",
                path.display()
            );
            tracing_subscriber::fmt().with_env_filter(filter).init();
            tracing::warn!(path = %path.display(), error = %e, "Fleet host logging fell back to stderr");
        }
    }
}

// Thin env wrapper over the tested `embedded_hub_persist_enabled_from`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn embedded_hub_persist_enabled() -> bool {
    embedded_hub_persist_enabled_from(std::env::var("FLEET_EMBEDDED_HUB_PERSIST").ok().as_deref())
}

fn embedded_hub_persist_enabled_from(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()),
        Some(v) if matches!(v.as_str(), "1" | "true" | "on" | "yes")
    )
}

/// Fixed loopback port for the command-bridge WS server, so each code-server can
/// be launched with `FLEET_BRIDGE_URL=ws://127.0.0.1:<this>` before Fleet starts.
const BRIDGE_PORT: u16 = 51778;

/// The webview's initial pull of current inbox state (live updates arrive via the
/// `inbox` event). App-defined command — not gated by the v2 capability ACL.
// Glue: a Tauri command reading managed `State`, which needs a running app.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
fn get_inbox(state: tauri::State<'_, hub_client::Shared>) -> RenderedInbox {
    state.lock().map(|g| g.clone()).unwrap_or_default()
}

// Glue: evaluates JS in the rail webview through the `AppHandle` — needs a live
// webview.
#[cfg_attr(coverage_nightly, coverage(off))]
fn run_rail_action(app: &tauri::AppHandle, function_name: &str) {
    if let Some(rail) = app.get_webview(mux::RAIL) {
        let _ = rail.eval(format!(
            "window.{function_name} && window.{function_name}()"
        ));
    }
}

// Glue: the entire Tauri application — builder, managed state, command handlers,
// menu/window-event callbacks, the background runtime thread, and the forever
// event loop (`.run`). Needs a real webview/event loop, so it can't run headless;
// every pure helper it wires up is tested individually.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() {
    // TCC disclaim trampoline (see `spawn::fleet_command`): replace this process
    // with the target before any logging/Tauri init so the short-lived hop has
    // no side effects. Must stay ahead of the hub-URL positional arg below.
    #[cfg(target_os = "macos")]
    if std::env::args_os().nth(1).as_deref() == Some(spawn::DISCLAIM_EXEC_ARG.as_ref()) {
        let argv: Vec<_> = std::env::args_os().skip(2).collect();
        let err = spawn::exec_disclaimed(&argv);
        eprintln!("fleet-host: disclaim-exec failed: {err}");
        std::process::exit(127);
    }

    init_logging();
    spawn::clear_legacy_spawn_state();

    let explicit_ws_url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("FLEET_HUB_URL").ok())
        .filter(|s| !s.is_empty());
    let start_local_hub = explicit_ws_url.is_none();
    let ws_url = explicit_ws_url.unwrap_or_else(|| DEFAULT_HUB_URL.to_string());

    let shared: hub_client::Shared = Arc::new(Mutex::new(RenderedInbox::default()));
    let (hub_commands, hub_command_rx) = hub_client::command_channel();
    let registry = bridge::BridgeRegistry::new();
    let bridge_token =
        bridge::launch_token_from_path(&embedded_hub_runtime_dir().join("bridge.token"));

    tauri::Builder::default()
        .menu(mux::build_menu)
        .manage(shared.clone())
        .manage(hub_commands)
        .manage(mux::MuxState::new())
        .manage(registry.clone())
        .manage(spawn::ServerSupervisor::new(
            BRIDGE_PORT,
            ws_url.clone(),
            bridge_token.clone(),
        ))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_inbox,
            mux::get_servers,
            mux::selected_server,
            mux::select_server,
            mux::spawn_server_with_options,
            mux::close_server,
            mux::rename_server,
            mux::open_server_external,
            mux::get_host_status,
            mux::clear_host_status_if_current,
            hub_client::set_session_muted,
            hub_client::set_session_soloed,
            hub_client::dismiss_session,
            hub_client::focus_session
        ])
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "spawn:new" {
                if let Some(sup) = app.try_state::<spawn::ServerSupervisor>() {
                    match sup.spawn() {
                        Ok(server) => {
                            mux::clear_host_status(app);
                            let _ = app.emit(bridge::SERVERS_CHANGED, ());
                            mux::select_spawned(app.clone(), server.id);
                        }
                        Err(e) => mux::emit_spawn_error(app, "menu", &e.to_string()),
                    }
                } else {
                    mux::emit_spawn_error(app, "menu", "server supervisor unavailable");
                }
            } else if id == "spawn:close-current" {
                if let (Some(sup), Some(mux)) = (
                    app.try_state::<spawn::ServerSupervisor>(),
                    app.try_state::<mux::MuxState>(),
                ) {
                    if let Some(active) = mux.selected.lock().ok().and_then(|g| g.clone()) {
                        mux::close_server_by_id(app, &sup, &active);
                    } else {
                        mux::emit_host_status(app, "warning", "menu", "no active server");
                    }
                } else {
                    mux::emit_host_status(app, "error", "menu", "server supervisor unavailable");
                }
            } else if id == "rail:palette" {
                run_rail_action(app, "__fleetOpenPalette");
            } else if id == "rail:jump-unread" {
                run_rail_action(app, "__fleetJumpNextUnread");
            } else if id == "rail:cycle-unread" {
                run_rail_action(app, "__fleetCycleUnread");
            } else if id == "external:open-current" {
                let Some(mux_state) = app.try_state::<mux::MuxState>() else {
                    mux::emit_host_status(app, "error", "menu", "server selector unavailable");
                    return;
                };
                let Some(active) = mux_state.selected.lock().ok().and_then(|g| g.clone()) else {
                    mux::emit_host_status(app, "warning", "menu", "no active server");
                    return;
                };
                if let Err(e) = mux::open_server_external_by_id(app, &active) {
                    mux::emit_host_status(app, "error", "menu", e);
                }
            } else if let Some(server_id) = id.strip_prefix("server:") {
                if server_id != "none" {
                    mux::select(app, server_id.to_string());
                }
            } else if let Some(command) = id.strip_prefix("cmd:") {
                let Some(mux_state) = app.try_state::<mux::MuxState>() else {
                    mux::emit_host_status(app, "error", "menu", "command bridge unavailable");
                    return;
                };
                let Some(reg) = app.try_state::<bridge::BridgeRegistry>() else {
                    mux::emit_host_status(app, "error", "menu", "command bridge unavailable");
                    return;
                };
                let Some(active) = mux_state.selected.lock().ok().and_then(|g| g.clone()) else {
                    mux::emit_host_status(app, "warning", "menu", "no active server");
                    return;
                };
                if !reg.send_command(&active, command) {
                    mux::emit_host_status(
                        app,
                        "warning",
                        "menu",
                        format!("command unavailable for {active}"),
                    );
                }
            }
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
                app.set_dock_visibility(true);
            }

            start_probe_control(app.handle().clone());

            let handle = app.handle().clone();
            let bridge_handle = app.handle().clone();
            let shared = shared.clone();
            let ws_url = ws_url.clone();
            let registry = registry.clone();
            let bridge_token = bridge_token.clone();
            let hub_command_rx = hub_command_rx;
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
                            let runtime_dir = embedded_hub_runtime_dir();
                            let config = fleet_hub::HubConfig {
                                ws_addr: default_hub_addr(),
                                unix_path: runtime_dir.join("hub.sock"),
                                lock_path: runtime_dir.join("hub.lock"),
                                db_path: runtime_dir.join("hub.db"),
                                persist: embedded_hub_persist_enabled(),
                                ..Default::default()
                            };
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
                    if let Err(e) =
                        bridge::serve(bridge_handle, registry, BRIDGE_PORT, bridge_token).await
                    {
                        tracing::error!(error = %e, "bridge server failed to bind");
                    }
                    hub_client::run(handle, shared, ws_url, hub_command_rx).await;
                });
            });

            mux::build_window(app)?;

            // Test harness hook: auto-spawn N servers on startup so an integration
            // test can drive Fleet without clicking (`FLEET_AUTOSPAWN=n`).
            if let Some(n) = std::env::var("FLEET_AUTOSPAWN")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            {
                if let Some(sup) = app.try_state::<spawn::ServerSupervisor>() {
                    for _ in 0..n {
                        match sup.spawn() {
                            Ok(server) => {
                                mux::clear_host_status(app.handle());
                                let _ = app.emit(bridge::SERVERS_CHANGED, ());
                                mux::select_spawned(app.handle().clone(), server.id);
                            }
                            Err(e) => {
                                mux::emit_spawn_error(app.handle(), "startup", &e.to_string());
                            }
                        }
                    }
                } else {
                    mux::emit_spawn_error(app.handle(), "startup", "server supervisor unavailable");
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Fleet host");
}

// Thin env wrapper over the tested `probe_control_port_from_value`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn probe_control_port() -> Option<u16> {
    let raw = std::env::var("FLEET_PROBE_CONTROL_PORT").ok();
    probe_control_port_from_value(raw.as_deref())
}

fn probe_control_port_from_value(value: Option<&str>) -> Option<u16> {
    value
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
}

// Glue: spawns a TCP control listener (test harness hook) that drives the
// `AppHandle` (server list/select/close) per request. Needs a live app and a
// bound socket; the HTTP response builder (`probe_http_response`) is tested.
#[cfg_attr(coverage_nightly, coverage(off))]
fn start_probe_control(app: tauri::AppHandle) {
    let Some(port) = probe_control_port() else {
        return;
    };
    std::thread::Builder::new()
        .name("fleet-probe-control".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => listener,
                Err(e) => {
                    tracing::warn!(port, error = %e, "probe control failed to bind");
                    return;
                }
            };
            tracing::info!(port, "probe control listening");
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_probe_control(app.clone(), stream),
                    Err(e) => tracing::warn!(error = %e, "probe control accept failed"),
                }
            }
        })
        .expect("spawn probe control thread");
}

// Glue: parses a probe HTTP request and dispatches it against the live `AppHandle`
// (mux server list/select/close) — needs a running app + a real socket.
#[cfg_attr(coverage_nightly, coverage(off))]
fn handle_probe_control(app: tauri::AppHandle, mut stream: TcpStream) {
    let mut buf = [0_u8; 2048];
    let Ok(n) = stream.read(&mut buf) else {
        return;
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let Some(path) = req
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    else {
        let _ = write_probe_response(&mut stream, 400, "bad request");
        return;
    };

    if path == "/healthz" {
        let _ = write_probe_json(&mut stream, 200, r#"{"ok":true}"#);
    } else if path == "/servers" {
        let body = serde_json::json!({ "servers": mux::servers_for_app(&app) }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if path == "/selected" {
        let selected = app
            .try_state::<mux::MuxState>()
            .and_then(|state| state.selected.lock().ok().and_then(|g| g.clone()));
        let body = serde_json::json!({ "selected": selected }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if path == "/inbox" {
        // Read command (`get_inbox`): the webview's initial pull of inbox state.
        // Mirror the command — clone the latest rendered inbox out of `Shared`.
        let inbox = app
            .try_state::<hub_client::Shared>()
            .map(|shared| shared.lock().map(|g| g.clone()).unwrap_or_default())
            .unwrap_or_default();
        let body = serde_json::to_string(&inbox).unwrap_or_else(|_| "null".to_string());
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if path == "/host-status" {
        // Read command (`get_host_status`): the current host-status override.
        let status = app
            .try_state::<mux::MuxState>()
            .and_then(|state| mux::read_host_status(&state));
        let body = serde_json::json!({ "status": status }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if let Some(rest) = path.strip_prefix("/host-status/set") {
        // Seed a host-status override so the GET + conditional-clear round-trips
        // have observable state to act on (`?message=<urlencoded>`). Headless: it
        // mutates `MuxState` directly, not via the live emit path.
        let message = rest
            .strip_prefix('?')
            .and_then(probe_query_message)
            .unwrap_or_default();
        let set = app
            .try_state::<mux::MuxState>()
            .map(|state| {
                mux::set_host_status_state(
                    &state,
                    mux::HostStatus {
                        level: "warning".into(),
                        source: "probe".into(),
                        message: message.clone(),
                    },
                );
                true
            })
            .unwrap_or(false);
        let body = serde_json::json!({ "set": set, "message": message }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if let Some(rest) = path.strip_prefix("/host-status/clear") {
        // `clear_host_status_if_current`: clears ONLY if the stored message still
        // matches the one the caller raised (`?message=<urlencoded>`).
        let message = rest
            .strip_prefix('?')
            .and_then(probe_query_message)
            .unwrap_or_default();
        let cleared = app
            .try_state::<mux::MuxState>()
            .map(|state| mux::clear_host_status_if_message(&state, &message))
            .unwrap_or(false);
        let body = serde_json::json!({ "cleared": cleared, "message": message }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if let Some(id) = path.strip_prefix("/select/") {
        let id = id.split(['?', '#']).next().unwrap_or(id).to_string();
        mux::select(&app, id.clone());
        let body = serde_json::json!({ "selected": id }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if let Some(id) = path.strip_prefix("/close/") {
        let id = id.split(['?', '#']).next().unwrap_or(id).to_string();
        let closed = app
            .try_state::<spawn::ServerSupervisor>()
            .map(|sup| mux::close_server_by_id(&app, &sup, &id))
            .unwrap_or(false);
        let body = serde_json::json!({ "closed": closed, "server": id }).to_string();
        let _ = write_probe_json(&mut stream, 200, &body);
    } else if let Some(rest) = path.strip_prefix("/rename/") {
        // `/rename/<id>?label=<urlencoded label>` — the only State-mutating probe
        // command that reduces purely to supervisor + registry mutation (no live
        // window). Run the headless dispatch, then add the window side effects.
        let (id, query) = match rest.split_once('?') {
            Some((id, query)) => (id, Some(query)),
            None => (rest, None),
        };
        let id = id.split('#').next().unwrap_or(id).to_string();
        let label = query.and_then(probe_query_label);
        let result = match (
            app.try_state::<spawn::ServerSupervisor>(),
            app.try_state::<bridge::BridgeRegistry>(),
        ) {
            (Some(sup), Some(registry)) => {
                apply_state_probe_command("rename", &id, label.as_deref(), &sup, &registry)
            }
            _ => None,
        };
        match result {
            Some(body) => {
                // Mirror the rail rename path: notify the rail to re-render.
                let _ = app.emit(bridge::SERVERS_CHANGED, ());
                let _ = write_probe_json(&mut stream, 200, &body.to_string());
            }
            None => {
                let _ = write_probe_response(&mut stream, 404, "not found");
            }
        }
    } else {
        let _ = write_probe_response(&mut stream, 404, "not found");
    }
}

/// Apply a State-mutating probe command against the live managed State objects
/// (supervisor + bridge registry) and return the JSON response body. Headless and
/// AppHandle-free so it is unit-testable without a window; `handle_probe_control`
/// only adds the `app.emit`/window glue around it.
///
/// The State-mutating probe commands reduce to supervisor + registry mutation:
/// - `rename` renames the server in BOTH the supervisor (Fleet-spawned servers)
///   and the registry (phone-home servers) — whichever holds the id — exactly like
///   the `rename_server` Tauri command.
/// - `close` drops the server from BOTH stores (the supervisor `close` kill +
///   `registry.forget`) — the headless core of `close_server` / `close_server_by_id`.
///   The live `/close/<id>` probe wraps this with the window glue (re-tile, blank /
///   re-select the editor surface), which needs the `AppHandle`; here we assert the
///   observable store effect.
///
/// The mute/solo/dismiss/focus actions are NOT here: they dispatch over the Hub
/// command channel (`hub_client::HubCommandSender`), not supervisor/registry
/// State, so they need the live link and are covered by the in-process Hub harness
/// (`hub_client` tests) + the CI smoke.
fn apply_state_probe_command(
    action: &str,
    id: &str,
    label: Option<&str>,
    sup: &spawn::ServerSupervisor,
    registry: &bridge::BridgeRegistry,
) -> Option<serde_json::Value> {
    match action {
        "rename" => {
            let label = label.unwrap_or("");
            let renamed_spawned = sup.rename(id, label);
            let renamed_registered = registry.rename(id, label);
            Some(serde_json::json!({
                "renamed": renamed_spawned || renamed_registered,
                "server": id,
                "label": label,
            }))
        }
        "close" => {
            let closed_spawned = sup.close(id);
            let forgot_registered = registry.forget(id);
            Some(serde_json::json!({
                "closed": closed_spawned || forgot_registered,
                "server": id,
            }))
        }
        _ => None,
    }
}

/// Extract and percent-decode the named parameter from a probe query string
/// (`key=My%20Project&…`). Pure. Returns `None` when absent.
fn probe_query_value(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, value)| percent_decode(value))
}

/// The `label` probe query parameter (`/rename/<id>?label=…`).
fn probe_query_label(query: &str) -> Option<String> {
    probe_query_value(query, "label")
}

/// The `message` probe query parameter (`/host-status/{set,clear}?message=…`).
fn probe_query_message(query: &str) -> Option<String> {
    probe_query_value(query, "message")
}

/// Minimal percent-decoder for probe query values: turns `%XX` escapes back into
/// bytes and `+` into a space (form-url-encoded form). Invalid escapes pass
/// through verbatim. Pure.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Glue: writes the pure `probe_http_response` to a live TCP stream.
#[cfg_attr(coverage_nightly, coverage(off))]
fn write_probe_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    stream
        .write_all(probe_http_response(status, "application/json; charset=utf-8", body).as_bytes())
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn write_probe_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    stream.write_all(probe_http_response(status, "text/plain; charset=utf-8", body).as_bytes())
}

/// Build the full HTTP/1.1 probe response (status line + headers + body). Pure.
fn probe_http_response(status: u16, content_type: &str, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_hub_runtime_defaults_under_home_and_honors_override() {
        assert_eq!(
            embedded_hub_runtime_dir_from(None, Some(PathBuf::from("/Users/example"))),
            PathBuf::from("/Users/example/.fleet/run")
        );
        assert_eq!(
            embedded_hub_runtime_dir_from(
                Some(PathBuf::from("/custom/fleet-run")),
                Some(PathBuf::from("/Users/example"))
            ),
            PathBuf::from("/custom/fleet-run")
        );
    }

    #[test]
    fn host_log_path_lives_under_runtime_dir() {
        assert_eq!(
            host_log_path_from_runtime_dir(PathBuf::from("/tmp/fleet-run")),
            PathBuf::from("/tmp/fleet-run/fleet-host.log")
        );
    }

    #[test]
    fn default_hub_addr_is_loopback_on_the_hub_port() {
        let addr = default_hub_addr();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), fleet_hub::DEFAULT_WS_PORT);
    }

    #[test]
    fn embedded_hub_persistence_defaults_off_and_can_be_enabled() {
        assert!(!embedded_hub_persist_enabled_from(None));
        assert!(!embedded_hub_persist_enabled_from(Some("")));
        assert!(embedded_hub_persist_enabled_from(Some("1")));
        assert!(embedded_hub_persist_enabled_from(Some("true")));
        assert!(embedded_hub_persist_enabled_from(Some("on")));
        assert!(embedded_hub_persist_enabled_from(Some("yes")));
        assert!(!embedded_hub_persist_enabled_from(Some("0")));
        assert!(!embedded_hub_persist_enabled_from(Some("false")));
        assert!(!embedded_hub_persist_enabled_from(Some("off")));
        assert!(!embedded_hub_persist_enabled_from(Some("no")));
    }

    #[test]
    fn fleet_log_writer_tees_to_its_backing_file() {
        let dir = std::env::temp_dir().join(format!("fleet-host-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fleet-host.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();

        let writer = FleetLogWriter {
            file: Arc::new(Mutex::new(file)),
        };
        // The MakeWriter impl hands out a guard that writes to the same file.
        let mut guard = writer.make_writer();
        let n = guard.write(b"hello fleet\n").unwrap();
        assert_eq!(n, "hello fleet\n".len());
        guard.flush().unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("hello fleet"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fleet_log_guard_tolerates_a_poisoned_file_lock() {
        let dir =
            std::env::temp_dir().join(format!("fleet-host-log-poison-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("fleet-host.log")).unwrap();
        let shared = Arc::new(Mutex::new(file));

        // Poison the file mutex from a panicking thread.
        let poison = shared.clone();
        let result = std::thread::spawn(move || {
            let _guard = poison.lock().unwrap();
            panic!("poison the log file lock");
        })
        .join();
        assert!(result.is_err());
        assert!(shared.is_poisoned());

        // Writes and flushes must NOT panic; write reports the bytes as accepted
        // (tee to stderr still happened), flush is a no-op on the poisoned file.
        let mut guard = FleetLogGuard {
            file: shared.clone(),
        };
        assert_eq!(guard.write(b"dropped\n").unwrap(), "dropped\n".len());
        guard.flush().unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_http_response_builds_status_line_headers_and_body() {
        let ok = probe_http_response(200, "application/json; charset=utf-8", r#"{"ok":true}"#);
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(ok.contains("content-type: application/json; charset=utf-8\r\n"));
        assert!(ok.contains("content-length: 11\r\n"));
        assert!(ok.contains("connection: close\r\n"));
        assert!(ok.ends_with("\r\n\r\n{\"ok\":true}"));

        // Each mapped status renders its reason phrase; unknown ones fall back.
        assert!(probe_http_response(400, "text/plain", "x").starts_with("HTTP/1.1 400 Bad Request"));
        assert!(probe_http_response(404, "text/plain", "x").starts_with("HTTP/1.1 404 Not Found"));
        assert!(probe_http_response(503, "text/plain", "x").starts_with("HTTP/1.1 503 Error"));
    }

    #[test]
    fn probe_control_port_requires_positive_integer() {
        assert_eq!(probe_control_port_from_value(None), None);
        assert_eq!(probe_control_port_from_value(Some("")), None);
        assert_eq!(probe_control_port_from_value(Some("0")), None);
        assert_eq!(probe_control_port_from_value(Some("not-a-port")), None);
        assert_eq!(probe_control_port_from_value(Some("51776")), Some(51776));
    }

    fn probe_supervisor() -> spawn::ServerSupervisor {
        spawn::ServerSupervisor::new(51778, "ws://127.0.0.1:51777".into(), "token".into())
    }

    #[test]
    fn probe_query_label_decodes_only_the_label_param() {
        assert_eq!(
            probe_query_label("label=Renamed%20Session").as_deref(),
            Some("Renamed Session")
        );
        assert_eq!(
            probe_query_label("x=1&label=My+Project&y=2").as_deref(),
            Some("My Project")
        );
        // No label param ⇒ None; a label of the literal text passes through.
        assert_eq!(probe_query_label("foo=bar"), None);
        assert_eq!(probe_query_label("label=plain").as_deref(), Some("plain"));
    }

    #[test]
    fn percent_decode_handles_escapes_plus_and_invalid_sequences() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%41%42%43"), "ABC");
        // A dangling/invalid escape is emitted verbatim rather than dropped.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    // The rename probe command performs a REAL round-trip through the dispatch:
    // it renames the server in whichever State (supervisor or registry) holds the
    // id, pins `.renamed`, and returns the {"renamed":true,...} body.
    #[test]
    fn apply_state_probe_rename_round_trips_through_supervisor_and_registry() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        sup.push_test_server("server-sup");
        registry.register_test_server("server-reg", "http://127.0.0.1:9/", "auto-reported");

        // Renaming a supervisor-held server.
        let body = apply_state_probe_command(
            "rename",
            "server-sup",
            Some("Spawned Renamed"),
            &sup,
            &registry,
        )
        .expect("rename returns a body");
        assert_eq!(body["renamed"], true);
        assert_eq!(body["server"], "server-sup");
        assert_eq!(body["label"], "Spawned Renamed");
        let server = sup
            .servers()
            .into_iter()
            .find(|s| s.id == "server-sup")
            .unwrap();
        assert_eq!(server.label, "Spawned Renamed");
        assert!(server.renamed);

        // Renaming a registry-held server.
        let body = apply_state_probe_command(
            "rename",
            "server-reg",
            Some("Bridge Renamed"),
            &sup,
            &registry,
        )
        .expect("rename returns a body");
        assert_eq!(body["renamed"], true);
        let server = registry
            .servers()
            .into_iter()
            .find(|s| s.id == "server-reg")
            .unwrap();
        assert_eq!(server.label, "Bridge Renamed");
        assert!(server.renamed);
    }

    // An unknown id mutates nothing and reports {"renamed":false}.
    #[test]
    fn apply_state_probe_rename_unknown_id_reports_not_renamed() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        sup.push_test_server("server-1");

        let body =
            apply_state_probe_command("rename", "ghost", Some("Nope"), &sup, &registry).unwrap();
        assert_eq!(body["renamed"], false);
        assert_eq!(body["server"], "ghost");
        // The real server is untouched.
        let server = &sup.servers()[0];
        assert_eq!(server.label, "server-1");
        assert!(!server.renamed);
    }

    // Regression-lock at the PROBE-DISPATCH seam: a rename driven through
    // `apply_state_probe_command` must survive a subsequent reporter re-register
    // with the auto label (the exact bug that motivated the `renamed` flag).
    #[test]
    fn apply_state_probe_rename_survives_reporter_reregister() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        registry.register_test_server("server-reg", "http://127.0.0.1:9/", "auto-reported");

        let body =
            apply_state_probe_command("rename", "server-reg", Some("My Project"), &sup, &registry)
                .expect("rename returns a body");
        assert_eq!(body["renamed"], true);

        // The reporter reconnects and re-registers with its auto label again.
        registry.register_test_server("server-reg", "http://127.0.0.1:9/", "auto-reported");

        let server = registry
            .servers()
            .into_iter()
            .find(|s| s.id == "server-reg")
            .unwrap();
        assert_eq!(
            server.label, "My Project",
            "reconnect must not clobber the probe rename"
        );
        assert!(server.renamed);
    }

    // Actions that are NOT State-only (mute/solo/dismiss/focus dispatch over the
    // Hub command channel, not supervisor/registry State) return None here, so the
    // probe handler falls through to 404 rather than silently succeeding.
    #[test]
    fn apply_state_probe_rejects_non_state_actions() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        sup.push_test_server("server-1");

        for action in ["mute", "solo", "dismiss", "focus", "unknown"] {
            assert!(
                apply_state_probe_command(action, "server-1", None, &sup, &registry).is_none(),
                "{action} must not be a State-only probe command"
            );
        }
        // The server is untouched by any of those.
        assert!(!sup.servers()[0].renamed);
        assert_eq!(sup.servers().len(), 1);
    }

    // The close probe command performs a REAL round-trip through the dispatch: it
    // drops the server from whichever store (supervisor or registry) holds the id
    // and reports {"closed":true}. This is the headless State core of the
    // `close_server` Tauri command (the live `/close/<id>` adds only window glue).
    #[test]
    fn apply_state_probe_close_round_trips_through_supervisor_and_registry() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        sup.push_test_server("server-sup");
        registry.register_test_server("server-reg", "http://127.0.0.1:9/", "auto-reported");
        assert_eq!(sup.servers().len(), 1);
        assert_eq!(registry.servers().len(), 1);

        // Closing a supervisor-held server drops it from the supervisor list. (The
        // `closed` bool reflects whether real child processes were killed; the
        // synthetic test server has none, so the OBSERVABLE store removal — not the
        // bool — is the signal here, exactly as `close_server_by_id` reports it.)
        let body = apply_state_probe_command("close", "server-sup", None, &sup, &registry)
            .expect("close returns a body");
        assert_eq!(body["server"], "server-sup");
        assert!(
            sup.servers().iter().all(|s| s.id != "server-sup"),
            "the closed server is gone from the supervisor list"
        );

        // Closing a registry-held server forgets it from the registry and reports
        // closed=true (forget reports presence).
        let body = apply_state_probe_command("close", "server-reg", None, &sup, &registry)
            .expect("close returns a body");
        assert_eq!(body["closed"], true);
        assert!(registry.servers().is_empty());
    }

    // Closing an unknown id mutates nothing and reports {"closed":false}.
    #[test]
    fn apply_state_probe_close_unknown_id_reports_not_closed() {
        let sup = probe_supervisor();
        let registry = bridge::BridgeRegistry::new();
        sup.push_test_server("server-1");

        let body = apply_state_probe_command("close", "ghost", None, &sup, &registry).unwrap();
        assert_eq!(body["closed"], false);
        assert_eq!(body["server"], "ghost");
        // The real server is untouched.
        assert_eq!(sup.servers().len(), 1);
        assert_eq!(sup.servers()[0].id, "server-1");
    }

    #[test]
    fn probe_query_message_decodes_only_the_message_param() {
        assert_eq!(
            probe_query_message("message=spawn%20failed%3A%20boom").as_deref(),
            Some("spawn failed: boom")
        );
        assert_eq!(
            probe_query_message("x=1&message=No+VS+Code&y=2").as_deref(),
            Some("No VS Code")
        );
        assert_eq!(probe_query_message("label=plain"), None);
    }
}

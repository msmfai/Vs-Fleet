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

fn embedded_hub_runtime_dir() -> PathBuf {
    embedded_hub_runtime_dir_from(
        std::env::var_os("FLEET_RUNTIME_DIR").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
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
#[tauri::command]
fn get_inbox(state: tauri::State<'_, hub_client::Shared>) -> RenderedInbox {
    state.lock().map(|g| g.clone()).unwrap_or_default()
}

fn main() {
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
            mux::spawn_server,
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

fn probe_control_port() -> Option<u16> {
    let raw = std::env::var("FLEET_PROBE_CONTROL_PORT").ok();
    probe_control_port_from_value(raw.as_deref())
}

fn probe_control_port_from_value(value: Option<&str>) -> Option<u16> {
    value
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
}

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
    } else {
        let _ = write_probe_response(&mut stream, 404, "not found");
    }
}

fn write_probe_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    write_probe_response_with_type(stream, status, "application/json; charset=utf-8", body)
}

fn write_probe_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    write_probe_response_with_type(stream, status, "text/plain; charset=utf-8", body)
}

fn write_probe_response_with_type(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    write!(
        stream,
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
    fn probe_control_port_requires_positive_integer() {
        assert_eq!(probe_control_port_from_value(None), None);
        assert_eq!(probe_control_port_from_value(Some("")), None);
        assert_eq!(probe_control_port_from_value(Some("0")), None);
        assert_eq!(probe_control_port_from_value(Some("not-a-port")), None);
        assert_eq!(probe_control_port_from_value(Some("51776")), Some(51776));
    }
}

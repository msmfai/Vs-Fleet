//! The multiplexer window — Fleet's core value proposition.
//!
//! ONE window hosting the Discord-style **rail** (Fleet's own UI: the list of
//! VS Code *server* workspaces + their agent state) plus persistent embedded
//! **editor surfaces** (one child webview per live server). Switching servers is
//! a visibility/layout operation, not a navigation away from the previous VS Code
//! client, so terminals and bridge connections stay warm like a tmux/cmux pane.
//!
//! Only the rail webview gets Fleet's IPC; each editor surface is a plain
//! external origin (the code-server) with no Fleet API access.

use std::{collections::HashMap, process::Command, sync::Mutex};

use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
use tauri::{
    utils::config::BackgroundThrottlingPolicy, webview::WebviewBuilder, App, AppHandle, Emitter,
    LogicalPosition, LogicalSize, Manager, State, WindowEvent,
};

/// Width of the rail, in logical pixels.
const RAIL_W: f64 = 248.0;
/// The single multiplexer window's label.
pub const WINDOW: &str = "main";
/// The rail webview's label.
pub const RAIL: &str = "rail";
/// Event emitted whenever the host has a user-visible status override.
pub const HOST_STATUS: &str = "host-status";

/// One VS Code server workspace (a code-server the rail can switch to).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Server {
    /// Stable id (also the agent session id the server's reporter registers).
    pub id: String,
    /// Display label shown in the rail.
    pub label: String,
    /// The code-server URL Fleet embeds.
    pub url: String,
    /// Whether Fleet owns the backing process/container and can kill it.
    pub owned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostStatus {
    pub level: String,
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RailMenuState {
    row_count: usize,
    unread_count: usize,
    openable_unread_count: usize,
}

/// One persistent editor webview owned by a server id.
#[derive(Debug, Clone)]
struct EditorEntry {
    label: String,
    loaded_url: Option<String>,
}

/// Multiplexer state: which server is selected + editor webviews by server id.
/// The server LIST is the supervisor (spawned) + the push-driven
/// [`crate::bridge::BridgeRegistry`].
#[derive(Default)]
pub struct MuxState {
    pub selected: Mutex<Option<String>>,
    pub status: Mutex<Option<HostStatus>>,
    /// Legacy singleton loaded URL, used only when `FLEET_EDITOR_KEEPALIVE=0`.
    loaded: Mutex<Option<String>>,
    /// Persistent editor webviews keyed by Fleet server id.
    editors: Mutex<HashMap<String, EditorEntry>>,
}

impl MuxState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The placeholder/singleton editor surface webview label.
pub const EDITOR: &str = "editor";

fn keepalive_enabled() -> bool {
    keepalive_env_enabled(std::env::var("FLEET_EDITOR_KEEPALIVE").ok().as_deref())
}

fn keepalive_env_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()),
        Some(v) if matches!(v.as_str(), "0" | "false" | "off" | "no")
    )
}

#[cfg(target_os = "macos")]
fn macos_title_bar_style() -> TitleBarStyle {
    macos_title_bar_style_from_env_value(
        std::env::var("FLEET_MACOS_TITLEBAR_STYLE").ok().as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn macos_title_bar_style_from_env_value(value: Option<&str>) -> TitleBarStyle {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "overlay" => TitleBarStyle::Overlay,
        Some(v) if v == "transparent" => TitleBarStyle::Transparent,
        Some(v) if v == "visible" => TitleBarStyle::Visible,
        _ => TitleBarStyle::Transparent,
    }
}

fn editor_label_for(id: &str) -> String {
    let mut label = String::from("editor:");
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
            label.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(label, "~{b:02x}");
        }
    }
    label
}

fn editor_builder(label: impl Into<String>, url: tauri::WebviewUrl) -> WebviewBuilder<tauri::Wry> {
    WebviewBuilder::new(label, url).background_throttling(BackgroundThrottlingPolicy::Disabled)
}

/// Look up the editor URL for a server id.
///
/// Bridge-registered servers are already known-good phone-home entries. For
/// Fleet-spawned servers, though, waiting for bridge registration can deadlock:
/// the bridge extension may not activate until the editor surface first loads
/// the VS Code web server. So spawned servers can navigate from the supervisor's
/// URL immediately, then bridge registration catches up once VS Code starts.
fn server_url(app: &AppHandle, id: &str) -> Option<String> {
    if let Some(url) = app
        .try_state::<crate::bridge::BridgeRegistry>()
        .and_then(|reg| {
            reg.servers()
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.url)
        })
    {
        return Some(url);
    }

    app.try_state::<crate::spawn::ServerSupervisor>()
        .and_then(|sup| {
            sup.servers()
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.url)
        })
}

fn bridge_registered(app: &AppHandle, id: &str) -> bool {
    app.try_state::<crate::bridge::BridgeRegistry>()
        .map(|reg| reg.servers().into_iter().any(|s| s.id == id))
        .unwrap_or(false)
}

pub fn servers_for_app(app: &AppHandle) -> Vec<Server> {
    let spawned = app
        .try_state::<crate::spawn::ServerSupervisor>()
        .map(|sup| sup.servers())
        .unwrap_or_default();
    let registered = app
        .try_state::<crate::bridge::BridgeRegistry>()
        .map(|reg| reg.servers())
        .unwrap_or_default();
    merged_servers(spawned, registered)
}

/// Build the multiplexer window: the rail plus a placeholder editor surface.
/// Persistent server editor webviews are created on first selection.
pub fn build_window(app: &mut App) -> tauri::Result<()> {
    let width = 1320.0_f64;
    let height = 860.0_f64;

    // `mut` is only exercised by the macOS titlebar branch below.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = tauri::window::WindowBuilder::new(app, WINDOW)
        .title("Fleet")
        .inner_size(width, height)
        .min_inner_size(760.0, 480.0)
        .icon(tauri::include_image!("icons/128x128.png"))?;
    // macOS: keep content out from under the native titlebar. Overlay lets the
    // child VS Code webview paint behind AppKit chrome, which can leave stale
    // tab/titlebar fragments when WebKit layers move.
    #[cfg(target_os = "macos")]
    {
        let titlebar_style = macos_title_bar_style();
        tracing::info!(?titlebar_style, "configured macOS titlebar style");
        builder = builder.title_bar_style(titlebar_style).hidden_title(true);
    }
    let window = builder.build()?;

    // Rail: Fleet's own UI (server list + agent state).
    window.add_child(
        WebviewBuilder::new(RAIL, tauri::WebviewUrl::App("index.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(RAIL_W, height),
    )?;

    // Placeholder editor surface (blank until a server is selected). It also acts
    // as the rollback singleton when FLEET_EDITOR_KEEPALIVE=0.
    let blank = "about:blank".parse().expect("about:blank is a valid url");
    window.add_child(
        editor_builder(EDITOR, tauri::WebviewUrl::External(blank)),
        LogicalPosition::new(RAIL_W, 0.0),
        LogicalSize::new(width - RAIL_W, height),
    )?;
    tracing::info!(
        keepalive = keepalive_enabled(),
        "multiplexer window built (awaiting registrations)"
    );

    let app_handle = app.handle().clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Resized(_)) {
            retile(&app_handle);
        }
    });

    Ok(())
}

/// Tauri command: the rail's server list — Fleet-spawned servers (supervisor) +
/// externally phoned-home servers (registry), deduped by id.
#[tauri::command]
pub fn get_servers(
    registry: State<'_, crate::bridge::BridgeRegistry>,
    sup: State<'_, crate::spawn::ServerSupervisor>,
) -> Vec<Server> {
    merged_servers(sup.servers(), registry.servers())
}

fn merged_servers(spawned: Vec<Server>, registered: Vec<Server>) -> Vec<Server> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Server> = Vec::new();
    for s in spawned.into_iter().chain(registered) {
        if seen.insert(s.id.clone()) {
            out.push(s);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Tauri command: the currently-selected server id.
#[tauri::command]
pub fn selected_server(state: State<'_, MuxState>) -> Option<String> {
    state.selected.lock().ok().and_then(|g| g.clone())
}

/// Tauri command: switch the editor surface to server `id`.
#[tauri::command]
pub fn select_server(app: AppHandle, id: String) -> bool {
    select(&app, id)
}

/// Select a newly-spawned server and retry navigation while VS Code is still
/// coming up. `code serve-web` can take a few seconds to bind; an early WebView
/// navigation to a not-yet-listening port otherwise fails once and stays blank.
pub fn select_spawned(app: AppHandle, id: String) {
    select_impl(&app, id.clone(), true);
    std::thread::spawn(move || {
        for delay_ms in [750_u64, 1_500, 3_000, 6_000] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            if bridge_registered(&app, &id) {
                tracing::info!(server_id = %id, "spawned server registered; stopping navigation retries");
                break;
            }
            if keepalive_enabled() {
                if server_url(&app, &id).is_none() {
                    break;
                }
                refresh_persistent_editor(&app, &id, true);
            } else {
                let still_selected = app
                    .try_state::<MuxState>()
                    .and_then(|state| state.selected.lock().ok().and_then(|g| g.clone()))
                    .as_deref()
                    == Some(id.as_str());
                if !still_selected {
                    break;
                }
                select_impl(&app, id.clone(), true);
            }
        }
    });
}

/// Tauri command: spawn a new code-server and add it to the rail. Returns its id.
#[tauri::command]
pub fn spawn_server(
    app: AppHandle,
    sup: State<'_, crate::spawn::ServerSupervisor>,
) -> Result<String, String> {
    match sup.spawn() {
        Ok(server) => {
            clear_host_status(&app);
            let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());
            select_spawned(app, server.id.clone());
            Ok(server.id)
        }
        Err(e) => {
            let message = e.to_string();
            emit_spawn_error(&app, "rail", &message);
            Err(message)
        }
    }
}

#[tauri::command]
pub fn spawn_server_with_options(
    app: AppHandle,
    sup: State<'_, crate::spawn::ServerSupervisor>,
    request: crate::spawn::SpawnRequest,
) -> Result<String, String> {
    match sup.spawn_with(request) {
        Ok(server) => {
            clear_host_status(&app);
            let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());
            select_spawned(app, server.id.clone());
            Ok(server.id)
        }
        Err(e) => {
            let message = e.to_string();
            emit_spawn_error(&app, "rail", &message);
            Err(message)
        }
    }
}

/// Tauri command: close server `id` (kills the process Fleet spawned).
#[tauri::command]
pub fn close_server(
    app: AppHandle,
    sup: State<'_, crate::spawn::ServerSupervisor>,
    id: String,
) -> bool {
    close_server_by_id(&app, &sup, &id)
}

#[tauri::command]
pub fn rename_server(
    app: AppHandle,
    registry: State<'_, crate::bridge::BridgeRegistry>,
    sup: State<'_, crate::spawn::ServerSupervisor>,
    id: String,
    label: String,
) -> Result<String, String> {
    let label = sanitize_server_label(&label)?;
    let renamed_spawned = sup.rename(&id, &label);
    let renamed_registered = registry.rename(&id, &label);
    if !renamed_spawned && !renamed_registered {
        tracing::warn!(server_id = %id, %label, "rename requested for unknown server");
        return Err("server not found".into());
    }

    tracing::info!(server_id = %id, %label, renamed_spawned, renamed_registered, "server renamed");
    let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());
    refresh_menu(&app);
    Ok(label)
}

#[tauri::command]
pub fn open_server_external(app: AppHandle, id: String) -> Result<(), String> {
    open_server_external_by_id(&app, &id)
}

#[tauri::command]
pub fn get_host_status(state: State<'_, MuxState>) -> Option<HostStatus> {
    state.status.lock().ok().and_then(|status| status.clone())
}

#[tauri::command]
pub fn clear_host_status_if_current(state: State<'_, MuxState>, message: String) {
    if let Ok(mut status) = state.status.lock() {
        if status.as_ref().is_some_and(|s| s.message == message) {
            *status = None;
        }
    }
}

pub fn emit_spawn_error(app: &AppHandle, source: &str, error: &str) {
    tracing::warn!(source, error, "spawn failed");
    emit_host_status(app, "error", source, format!("spawn failed: {error}"));
}

pub fn emit_host_status(
    app: &AppHandle,
    level: impl Into<String>,
    source: impl Into<String>,
    message: impl Into<String>,
) {
    set_host_status(
        app,
        HostStatus {
            level: level.into(),
            source: source.into(),
            message: message.into(),
        },
    );
}

pub fn clear_host_status(app: &AppHandle) {
    if let Some(state) = app.try_state::<MuxState>() {
        if let Ok(mut status) = state.status.lock() {
            *status = None;
        }
    }
    let _ = app.emit(HOST_STATUS, Option::<HostStatus>::None);
}

fn set_host_status(app: &AppHandle, status: HostStatus) {
    if let Some(state) = app.try_state::<MuxState>() {
        if let Ok(mut stored) = state.status.lock() {
            *stored = Some(status.clone());
        }
    }
    let _ = app.emit(HOST_STATUS, Some(status));
}

/// Switch the editor surface to server `id` (shared by the rail and the menu).
/// In keepalive mode this shows that server's persistent editor webview; in
/// rollback singleton mode it navigates the single editor webview.
pub fn select(app: &AppHandle, id: String) -> bool {
    select_impl(app, id, false)
}

fn select_impl(app: &AppHandle, id: String, force_navigate: bool) -> bool {
    let Some(state) = app.try_state::<MuxState>() else {
        return false;
    };
    if server_url(app, &id).is_none() {
        tracing::warn!(
            server_id = %id,
            force = force_navigate,
            "selection ignored because server URL is not known"
        );
        return false;
    }
    if let Ok(mut sel) = state.selected.lock() {
        *sel = Some(id.clone());
    }
    tracing::info!(server_id = %id, force = force_navigate, "selected server");

    if keepalive_enabled() {
        select_persistent(app, &state, &id, force_navigate);
        sync_rail_selection(app);
        refresh_menu(app);
        return true;
    }

    select_singleton(app, &state, &id, force_navigate);
    sync_rail_selection(app);
    refresh_menu(app);
    true
}

fn select_singleton(app: &AppHandle, state: &State<'_, MuxState>, id: &str, force_navigate: bool) {
    // Navigate the editor to the server's URL (only if it changed, so re-selecting
    // the same server doesn't reload it). The loading overlay is raised/lowered by
    // the editor's own page-load events (see `build_window`).
    if let Some(target) = server_url(app, id) {
        if let Ok(mut loaded) = state.loaded.lock() {
            if force_navigate || loaded.as_deref() != Some(target.as_str()) {
                if let (Some(wv), Ok(parsed)) = (app.get_webview(EDITOR), target.parse()) {
                    tracing::info!(
                        mode = "singleton",
                        server_id = %id,
                        editor_label = EDITOR,
                        url = %target,
                        force = force_navigate,
                        "navigating editor surface"
                    );
                    match wv.navigate(parsed) {
                        Ok(()) => *loaded = Some(target),
                        Err(e) => tracing::warn!(
                            mode = "singleton",
                            server_id = %id,
                            editor_label = EDITOR,
                            url = %target,
                            error = %e,
                            "editor surface navigation failed"
                        ),
                    }
                }
            }
        }
    }
    retile(app);
}

fn select_persistent(app: &AppHandle, state: &State<'_, MuxState>, id: &str, force_navigate: bool) {
    let Some(target) = server_url(app, id) else {
        retile(app);
        return;
    };
    let Some(label) = ensure_persistent_editor(app, state, id) else {
        retile(app);
        return;
    };
    navigate_persistent_editor(app, state, id, &label, &target, force_navigate);
    retile(app);
}

fn refresh_persistent_editor(app: &AppHandle, id: &str, force_navigate: bool) {
    let Some(state) = app.try_state::<MuxState>() else {
        return;
    };
    let Some(target) = server_url(app, id) else {
        return;
    };
    let Some(label) = ensure_persistent_editor(app, &state, id) else {
        return;
    };
    navigate_persistent_editor(app, &state, id, &label, &target, force_navigate);
}

fn ensure_persistent_editor(
    app: &AppHandle,
    state: &State<'_, MuxState>,
    id: &str,
) -> Option<String> {
    if let Ok(editors) = state.editors.lock() {
        if let Some(entry) = editors.get(id) {
            return Some(entry.label.clone());
        }
    }

    let label = editor_label_for(id);
    if app.get_webview(&label).is_none() {
        let win = app.get_window(WINDOW)?;
        let (pos, size) = editor_parking_pane(app).unwrap_or((
            LogicalPosition::new(RAIL_W, 784.0),
            LogicalSize::new(1.0, 1.0),
        ));
        let blank = "about:blank".parse().expect("about:blank is a valid url");
        match win.add_child(
            editor_builder(label.clone(), tauri::WebviewUrl::External(blank)),
            pos,
            size,
        ) {
            Ok(wv) => {
                let _ = wv.hide();
                tracing::info!(
                    mode = "persistent",
                    server_id = %id,
                    editor_label = %label,
                    "created persistent editor surface"
                );
            }
            Err(e) => {
                tracing::warn!(
                    mode = "persistent",
                    server_id = %id,
                    editor_label = %label,
                    error = %e,
                    "persistent editor surface creation failed"
                );
                return None;
            }
        }
    }

    if let Ok(mut editors) = state.editors.lock() {
        editors.entry(id.to_string()).or_insert(EditorEntry {
            label: label.clone(),
            loaded_url: None,
        });
    }
    Some(label)
}

fn navigate_persistent_editor(
    app: &AppHandle,
    state: &State<'_, MuxState>,
    id: &str,
    label: &str,
    target: &str,
    force_navigate: bool,
) {
    let should_navigate = state
        .editors
        .lock()
        .ok()
        .and_then(|editors| {
            editors
                .get(id)
                .map(|entry| force_navigate || entry.loaded_url.as_deref() != Some(target))
        })
        .unwrap_or(force_navigate);
    if !should_navigate {
        return;
    }

    let Ok(parsed) = target.parse() else {
        tracing::warn!(
            mode = "persistent",
            server_id = %id,
            editor_label = %label,
            url = %target,
            "persistent editor URL could not be parsed"
        );
        return;
    };
    let Some(wv) = app.get_webview(label) else {
        tracing::warn!(
            mode = "persistent",
            server_id = %id,
            editor_label = %label,
            url = %target,
            "persistent editor surface missing during navigation"
        );
        return;
    };
    tracing::info!(
        mode = "persistent",
        server_id = %id,
        editor_label = %label,
        url = %target,
        force = force_navigate,
        "navigating persistent editor surface"
    );
    match wv.navigate(parsed) {
        Ok(()) => {
            if let Ok(mut editors) = state.editors.lock() {
                if let Some(entry) = editors.get_mut(id) {
                    entry.loaded_url = Some(target.to_string());
                }
            }
        }
        Err(e) => tracing::warn!(
            mode = "persistent",
            server_id = %id,
            editor_label = %label,
            url = %target,
            error = %e,
            "persistent editor surface navigation failed"
        ),
    }
}

pub fn close_server_by_id(app: &AppHandle, sup: &crate::spawn::ServerSupervisor, id: &str) -> bool {
    let was_selected = app
        .try_state::<MuxState>()
        .and_then(|state| state.selected.lock().ok().and_then(|g| g.clone()))
        .as_deref()
        == Some(id);

    close_editor(app, id);
    let closed = sup.close(id);
    if let Some(reg) = app.try_state::<crate::bridge::BridgeRegistry>() {
        let _ = reg.forget(id);
    }

    if was_selected {
        if let Some(state) = app.try_state::<MuxState>() {
            if let Ok(mut selected) = state.selected.lock() {
                *selected = None;
            }
            if !keepalive_enabled() {
                blank_singleton_editor(app, &state);
            }
        }
    }

    let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());

    if was_selected {
        if let Some(next) = first_server_id(app, Some(id)) {
            select_impl(app, next, false);
        } else {
            retile(app);
            sync_rail_selection(app);
        }
    } else {
        retile(app);
    }

    tracing::info!(server_id = %id, closed, "close server requested");
    refresh_menu(app);
    closed
}

pub fn open_server_external_by_id(app: &AppHandle, id: &str) -> Result<(), String> {
    let url = server_url(app, id).ok_or_else(|| "server URL unavailable".to_string())?;
    open_external_url(&url).map_err(|e| format!("open browser failed: {e}"))
}

fn open_external_url(url: &str) -> std::io::Result<()> {
    let (program, args) = external_open_command(url);
    Command::new(program).args(args).spawn().map(|_| ())
}

#[cfg(target_os = "macos")]
fn external_open_command(url: &str) -> (&'static str, Vec<String>) {
    ("open", vec![url.to_string()])
}

#[cfg(target_os = "windows")]
fn external_open_command(url: &str) -> (&'static str, Vec<String>) {
    (
        "cmd",
        vec![
            "/C".to_string(),
            "start".to_string(),
            "".to_string(),
            url.to_string(),
        ],
    )
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn external_open_command(url: &str) -> (&'static str, Vec<String>) {
    ("xdg-open", vec![url.to_string()])
}

fn close_editor(app: &AppHandle, id: &str) {
    let Some(state) = app.try_state::<MuxState>() else {
        return;
    };
    let entry = state
        .editors
        .lock()
        .ok()
        .and_then(|mut editors| editors.remove(id));
    if let Some(entry) = entry {
        if let Some(wv) = app.get_webview(&entry.label) {
            match wv.close() {
                Ok(()) => tracing::info!(
                    server_id = %id,
                    editor_label = %entry.label,
                    "closed persistent editor surface"
                ),
                Err(e) => tracing::warn!(
                    server_id = %id,
                    editor_label = %entry.label,
                    error = %e,
                    "persistent editor surface close failed"
                ),
            }
        }
    }
}

fn blank_singleton_editor(app: &AppHandle, state: &State<'_, MuxState>) {
    let blank = "about:blank".parse().expect("about:blank is a valid url");
    if let Some(wv) = app.get_webview(EDITOR) {
        let _ = wv.navigate(blank);
    }
    if let Ok(mut loaded) = state.loaded.lock() {
        *loaded = None;
    }
}

fn first_server_id(app: &AppHandle, exclude: Option<&str>) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    if let Some(sup) = app.try_state::<crate::spawn::ServerSupervisor>() {
        for server in sup.servers() {
            if exclude != Some(server.id.as_str()) && seen.insert(server.id.clone()) {
                ids.push(server.id);
            }
        }
    }
    if let Some(reg) = app.try_state::<crate::bridge::BridgeRegistry>() {
        for server in reg.servers() {
            if exclude != Some(server.id.as_str()) && seen.insert(server.id.clone()) {
                ids.push(server.id);
            }
        }
    }
    ids.sort();
    ids.into_iter().next()
}

fn sync_rail_selection(app: &AppHandle) {
    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.eval("window.__fleetSyncSelection && window.__fleetSyncSelection()");
    }
}

/// Build Fleet's static native menu bar.
///
/// Fleet embeds full VS Code surfaces, but a child webview cannot own the macOS
/// menu bar. This mirrors the VS Code command menus and forwards clicked items
/// through the active server's bridge. Items deliberately have no Fleet-level
/// accelerators; keyboard shortcuts stay owned by the focused VS Code webview.
pub fn build_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    build_app_menu(app, &[], None, RailMenuState::default())
}

fn build_app_menu<R: tauri::Runtime>(
    manager: &tauri::AppHandle<R>,
    servers: &[Server],
    selected: Option<&str>,
    rail_state: RailMenuState,
) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{
        AboutMetadata, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
    };

    let pkg_info = manager.package_info();
    let config = manager.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    let app_menu = SubmenuBuilder::new(manager, "Fleet")
        .about(Some(about_metadata))
        .separator()
        .item(
            &MenuItemBuilder::with_id("cmd:workbench.action.openSettings", "Settings...")
                .build(manager)?,
        )
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let vscode_menus: &[(&str, &[MItem])] = &[
        ("File", FILE),
        ("Edit", EDIT),
        ("Selection", SELECTION),
        ("View", VIEW),
        ("Go", GO),
        ("Run", RUN),
        ("Terminal", TERMINAL),
    ];
    let mut subs = Vec::new();
    for (title, items) in vscode_menus {
        subs.push(build_sub(manager, title, items)?);
    }

    let server_menu = build_server_menu(manager, servers, selected, rail_state)?;

    let window_menu = SubmenuBuilder::new(manager, "Window")
        .item(&PredefinedMenuItem::minimize(manager, None)?)
        .item(&PredefinedMenuItem::fullscreen(manager, None)?)
        .separator()
        .item(&PredefinedMenuItem::close_window(manager, None)?)
        .build()?;
    let help_menu = build_sub(manager, "Help", HELP)?;

    let mut menu = MenuBuilder::new(manager).item(&app_menu);
    for sub in &subs {
        menu = menu.item(sub);
    }
    menu.item(&server_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()
}

pub fn refresh_menu(app: &AppHandle) {
    let _ = app;
}

fn build_server_menu<R: tauri::Runtime>(
    manager: &tauri::AppHandle<R>,
    servers: &[Server],
    selected: Option<&str>,
    rail_state: RailMenuState,
) -> tauri::Result<tauri::menu::Submenu<R>> {
    use tauri::menu::{CheckMenuItemBuilder, MenuItemBuilder, SubmenuBuilder};
    let close_current = close_current_menu_item(servers, selected);
    let open_current_enabled = selected_server_has_url(servers, selected);

    let mut menu = SubmenuBuilder::new(manager, "Server")
        .item(&MenuItemBuilder::with_id("spawn:new", "New Server").build(manager)?)
        .item(
            &MenuItemBuilder::with_id("spawn:close-current", close_current.label)
                .enabled(close_current.enabled)
                .build(manager)?,
        )
        .item(
            &MenuItemBuilder::with_id("external:open-current", "Open Current in Browser")
                .enabled(open_current_enabled)
                .build(manager)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("rail:palette", "Session Palette")
                .enabled(rail_state.row_count > 0)
                .build(manager)?,
        )
        .item(
            &MenuItemBuilder::with_id("rail:jump-unread", "Jump to Next Unread")
                .enabled(rail_state.openable_unread_count > 0)
                .build(manager)?,
        )
        .item(
            &MenuItemBuilder::with_id("rail:cycle-unread", "Cycle Unread Without Marking Read")
                .enabled(rail_state.unread_count > 0)
                .build(manager)?,
        )
        .separator();

    if servers.is_empty() {
        return menu
            .item(
                &MenuItemBuilder::with_id("server:none", "No Servers")
                    .enabled(false)
                    .build(manager)?,
            )
            .build();
    }

    for server in servers {
        let item = CheckMenuItemBuilder::with_id(
            format!("server:{}", server.id),
            menu_server_label(server),
        )
        .checked(selected == Some(server.id.as_str()))
        .build(manager)?;
        menu = menu.item(&item);
    }

    menu.build()
}

fn menu_server_label(server: &Server) -> String {
    if server.owned {
        server.label.clone()
    } else {
        format!("{} (external)", server.label)
    }
}

struct CloseCurrentMenuItem {
    label: &'static str,
    enabled: bool,
}

fn close_current_menu_item(servers: &[Server], selected: Option<&str>) -> CloseCurrentMenuItem {
    let Some(selected) = selected else {
        return CloseCurrentMenuItem {
            label: "Close Current Server",
            enabled: false,
        };
    };
    let Some(server) = servers.iter().find(|server| server.id == selected) else {
        return CloseCurrentMenuItem {
            label: "Close Current Server",
            enabled: false,
        };
    };
    CloseCurrentMenuItem {
        label: if server.owned {
            "Close Current Server"
        } else {
            "Forget Current Server"
        },
        enabled: true,
    }
}

fn selected_server_has_url(servers: &[Server], selected: Option<&str>) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    servers
        .iter()
        .any(|server| server.id == selected && !server.url.is_empty())
}

/// One entry in a mirrored VS Code menu.
enum MItem {
    Cmd(&'static str, &'static str),
    Sep,
    Cut,
    Copy,
    Paste,
}

fn build_sub<R: tauri::Runtime>(
    manager: &tauri::AppHandle<R>,
    title: &str,
    items: &[MItem],
) -> tauri::Result<tauri::menu::Submenu<R>> {
    use tauri::menu::{MenuItemBuilder, SubmenuBuilder};
    let mut b = SubmenuBuilder::new(manager, title);
    for it in items {
        b = match it {
            MItem::Sep => b.separator(),
            MItem::Cut => b.cut(),
            MItem::Copy => b.copy(),
            MItem::Paste => b.paste(),
            MItem::Cmd(label, id) => {
                let mi = MenuItemBuilder::with_id(format!("cmd:{id}"), *label).build(manager)?;
                b.item(&mi)
            }
        };
    }
    b.build()
}

use MItem::{Cmd, Copy as MCopy, Cut as MCut, Paste as MPaste, Sep};

const FILE: &[MItem] = &[
    Cmd("New Text File", "workbench.action.files.newUntitledFile"),
    Cmd("New Window", "workbench.action.newWindow"),
    Sep,
    Cmd("Open File...", "workbench.action.files.openFile"),
    Cmd("Open Folder...", "workbench.action.files.openFolder"),
    Cmd("Open Recent...", "workbench.action.openRecent"),
    Sep,
    Cmd("Save", "workbench.action.files.save"),
    Cmd("Save As...", "workbench.action.files.saveAs"),
    Cmd("Save All", "workbench.action.files.saveAll"),
    Cmd("Auto Save", "workbench.action.toggleAutoSave"),
    Sep,
    Cmd("Revert File", "workbench.action.files.revert"),
    Cmd("Close Editor", "workbench.action.closeActiveEditor"),
    Cmd("Close Folder", "workbench.action.closeFolder"),
];

const EDIT: &[MItem] = &[
    Cmd("Undo", "undo"),
    Cmd("Redo", "redo"),
    Sep,
    MCut,
    MCopy,
    MPaste,
    Sep,
    Cmd("Find", "actions.find"),
    Cmd("Replace", "editor.action.startFindReplaceAction"),
    Sep,
    Cmd("Find in Files", "workbench.action.findInFiles"),
    Cmd("Replace in Files", "workbench.action.replaceInFiles"),
    Sep,
    Cmd("Toggle Line Comment", "editor.action.commentLine"),
    Cmd("Toggle Block Comment", "editor.action.blockComment"),
];

const SELECTION: &[MItem] = &[
    Cmd("Select All", "editor.action.selectAll"),
    Cmd("Expand Selection", "editor.action.smartSelect.expand"),
    Cmd("Shrink Selection", "editor.action.smartSelect.shrink"),
    Sep,
    Cmd("Copy Line Up", "editor.action.copyLinesUpAction"),
    Cmd("Copy Line Down", "editor.action.copyLinesDownAction"),
    Cmd("Move Line Up", "editor.action.moveLinesUpAction"),
    Cmd("Move Line Down", "editor.action.moveLinesDownAction"),
    Cmd("Duplicate Selection", "editor.action.duplicateSelection"),
    Sep,
    Cmd("Add Cursor Above", "editor.action.insertCursorAbove"),
    Cmd("Add Cursor Below", "editor.action.insertCursorBelow"),
    Cmd(
        "Add Next Occurrence",
        "editor.action.addSelectionToNextFindMatch",
    ),
    Cmd(
        "Column Selection Mode",
        "editor.action.toggleColumnSelection",
    ),
];

const VIEW: &[MItem] = &[
    Cmd("Command Palette...", "workbench.action.showCommands"),
    Cmd("Open View...", "workbench.action.openView"),
    Sep,
    Cmd("Explorer", "workbench.view.explorer"),
    Cmd("Search", "workbench.view.search"),
    Cmd("Source Control", "workbench.view.scm"),
    Cmd("Run and Debug", "workbench.view.debug"),
    Cmd("Extensions", "workbench.view.extensions"),
    Sep,
    Cmd("Problems", "workbench.actions.view.problems"),
    Cmd("Output", "workbench.action.output.toggleOutput"),
    Cmd("Terminal", "workbench.action.terminal.toggleTerminal"),
    Sep,
    Cmd("Word Wrap", "editor.action.toggleWordWrap"),
    Cmd(
        "Toggle Side Bar",
        "workbench.action.toggleSidebarVisibility",
    ),
    Cmd("Toggle Panel", "workbench.action.togglePanel"),
    Cmd("Zoom In", "workbench.action.zoomIn"),
    Cmd("Zoom Out", "workbench.action.zoomOut"),
];

const GO: &[MItem] = &[
    Cmd("Back", "workbench.action.navigateBack"),
    Cmd("Forward", "workbench.action.navigateForward"),
    Sep,
    Cmd("Go to File...", "workbench.action.quickOpen"),
    Cmd(
        "Go to Symbol in Workspace...",
        "workbench.action.showAllSymbols",
    ),
    Cmd("Go to Symbol in Editor...", "workbench.action.gotoSymbol"),
    Sep,
    Cmd("Go to Definition", "editor.action.revealDefinition"),
    Cmd("Go to References", "editor.action.goToReferences"),
    Cmd("Go to Line/Column...", "workbench.action.gotoLine"),
    Sep,
    Cmd("Next Problem", "editor.action.marker.next"),
    Cmd("Previous Problem", "editor.action.marker.prev"),
];

const RUN: &[MItem] = &[
    Cmd("Start Debugging", "workbench.action.debug.start"),
    Cmd("Run Without Debugging", "workbench.action.debug.run"),
    Cmd("Stop Debugging", "workbench.action.debug.stop"),
    Cmd("Restart Debugging", "workbench.action.debug.restart"),
    Sep,
    Cmd("Open Configurations", "workbench.action.debug.configure"),
    Cmd("Add Configuration...", "debug.addConfiguration"),
    Sep,
    Cmd("Toggle Breakpoint", "editor.debug.action.toggleBreakpoint"),
];

const TERMINAL: &[MItem] = &[
    Cmd("New Terminal", "workbench.action.terminal.new"),
    Cmd("Split Terminal", "workbench.action.terminal.split"),
    Sep,
    Cmd("Run Task...", "workbench.action.tasks.runTask"),
    Cmd("Run Build Task...", "workbench.action.tasks.build"),
    Sep,
    Cmd("Kill Terminal", "workbench.action.terminal.kill"),
];

const HELP: &[MItem] = &[
    Cmd("Welcome", "workbench.action.openWalkthrough"),
    Cmd("Show All Commands", "workbench.action.showCommands"),
    Cmd("Editor Playground", "editor.action.inspectTMScopes"),
];

fn sanitize_server_label(label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        tracing::warn!("empty server label rejected");
        return Err("label cannot be empty".into());
    }
    let sanitized = label.chars().take(80).collect::<String>();
    if sanitized.len() < label.len() {
        tracing::warn!(label_len = label.len(), "server label truncated");
    }
    Ok(sanitized)
}

/// Re-tile the rail + show the selected editor surface (hide the rest).
fn retile(app: &AppHandle) {
    let Some(win) = app.get_window(WINDOW) else {
        return;
    };
    let (Ok(size), Ok(sf)) = (win.inner_size(), win.scale_factor()) else {
        return;
    };
    let w = size.width as f64 / sf;
    let h = size.height as f64 / sf;

    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = rail.set_size(LogicalSize::new(RAIL_W, h));
    }

    let pos = LogicalPosition::new(RAIL_W, 0.0);
    let pane = LogicalSize::new((w - RAIL_W).max(120.0), h);
    let park_pos = LogicalPosition::new(RAIL_W, h + 64.0);
    let park_size = LogicalSize::new(1.0, 1.0);

    if keepalive_enabled() {
        let Some(state) = app.try_state::<MuxState>() else {
            return;
        };
        let selected = state.selected.lock().ok().and_then(|g| g.clone());
        let mut active_label = None;
        let mut inactive_labels = Vec::new();
        if let Ok(editors) = state.editors.lock() {
            for (id, entry) in editors.iter() {
                if selected.as_deref() == Some(id.as_str()) {
                    active_label = Some(entry.label.clone());
                } else {
                    inactive_labels.push(entry.label.clone());
                }
            }
        }

        for label in inactive_labels {
            if let Some(wv) = app.get_webview(&label) {
                let _ = wv.set_position(park_pos);
                let _ = wv.set_size(park_size);
                let _ = wv.hide();
            }
        }

        if let Some(wv) = app.get_webview(EDITOR) {
            if active_label.is_some() {
                let _ = wv.set_position(park_pos);
                let _ = wv.set_size(park_size);
                let _ = wv.hide();
            } else {
                let _ = wv.set_position(pos);
                let _ = wv.set_size(pane);
                let _ = wv.show();
            }
        }

        if let Some(label) = active_label {
            if let Some(wv) = app.get_webview(&label) {
                let _ = wv.set_position(pos);
                let _ = wv.set_size(pane);
                let _ = wv.show();
            }
        }
    } else if let Some(wv) = app.get_webview(EDITOR) {
        let _ = wv.set_position(pos);
        let _ = wv.set_size(pane);
        let _ = wv.show();
    }
}

fn editor_parking_pane(app: &AppHandle) -> Option<(LogicalPosition<f64>, LogicalSize<f64>)> {
    let win = app.get_window(WINDOW)?;
    let size = win.inner_size().ok()?;
    let sf = win.scale_factor().ok()?;
    let h = size.height as f64 / sf;
    Some((
        LogicalPosition::new(RAIL_W, h + 64.0),
        LogicalSize::new(1.0, 1.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        editor_label_for, external_open_command, keepalive_env_enabled, merged_servers,
        sanitize_server_label, Server,
    };

    #[cfg(target_os = "macos")]
    use super::macos_title_bar_style_from_env_value;

    #[cfg(target_os = "macos")]
    use tauri::TitleBarStyle;

    #[test]
    fn keepalive_defaults_on_with_common_disable_values() {
        assert!(keepalive_env_enabled(None));
        assert!(keepalive_env_enabled(Some("1")));
        assert!(keepalive_env_enabled(Some("true")));
        assert!(!keepalive_env_enabled(Some("0")));
        assert!(!keepalive_env_enabled(Some("false")));
        assert!(!keepalive_env_enabled(Some("OFF")));
        assert!(!keepalive_env_enabled(Some(" no ")));
    }

    #[test]
    fn editor_labels_escape_external_server_ids() {
        assert_eq!(editor_label_for("server-1"), "editor:server-1");
        assert_eq!(
            editor_label_for("host/ws 1@prod"),
            "editor:host~2fws~201~40prod"
        );
    }

    #[test]
    fn server_labels_are_trimmed_bounded_and_nonempty() {
        assert_eq!(
            sanitize_server_label("  Project API  ").unwrap(),
            "Project API"
        );
        assert_eq!(
            sanitize_server_label(&"a".repeat(90)).unwrap(),
            "a".repeat(80)
        );
        assert_eq!(
            sanitize_server_label("   ").unwrap_err(),
            "label cannot be empty"
        );
    }

    #[test]
    fn external_open_command_targets_requested_url() {
        let (program, args) = external_open_command("http://127.0.0.1:51780/");
        #[cfg(target_os = "macos")]
        {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["http://127.0.0.1:51780/"]);
        }
        #[cfg(target_os = "windows")]
        {
            assert_eq!(program, "cmd");
            assert_eq!(args, vec!["/C", "start", "", "http://127.0.0.1:51780/"]);
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["http://127.0.0.1:51780/"]);
        }
    }

    #[test]
    fn merged_servers_prefers_fleet_owned_entries() {
        let owned = Server {
            id: "server-1".into(),
            label: "server-1".into(),
            url: "http://127.0.0.1:1/".into(),
            owned: true,
        };
        let registered = Server {
            id: "server-1".into(),
            label: "bridge label".into(),
            url: "http://127.0.0.1:2/".into(),
            owned: false,
        };
        assert_eq!(
            merged_servers(vec![owned.clone()], vec![registered]),
            vec![owned]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_titlebar_style_defaults_to_transparent() {
        assert_eq!(
            macos_title_bar_style_from_env_value(None),
            TitleBarStyle::Transparent
        );
        assert_eq!(
            macos_title_bar_style_from_env_value(Some("unknown")),
            TitleBarStyle::Transparent
        );
        assert_eq!(
            macos_title_bar_style_from_env_value(Some(" transparent ")),
            TitleBarStyle::Transparent
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_titlebar_style_accepts_diagnostic_variants() {
        assert_eq!(
            macos_title_bar_style_from_env_value(Some("OVERLAY")),
            TitleBarStyle::Overlay
        );
        assert_eq!(
            macos_title_bar_style_from_env_value(Some("visible")),
            TitleBarStyle::Visible
        );
    }
}

//! The multiplexer window — Fleet's core value proposition.
//!
//! ONE window hosting the Discord-style **rail** (Fleet's own UI: the list of
//! VS Code *server* workspaces + their agent state) plus one embedded **editor
//! surface per server** (the code-server rendered in a webview). Selecting a rail
//! entry shows that server's surface filling the main pane; the others are kept
//! alive but hidden, so switching is instant and preserves editor/terminal state
//! (the cmux model: window → workspace → surface).
//!
//! Only the rail webview gets Fleet's IPC; each editor surface is a plain
//! external origin (the code-server) with no Fleet API access.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    webview::WebviewBuilder, App, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State,
    WindowEvent,
};

/// Width of the rail, in logical pixels.
const RAIL_W: f64 = 248.0;
/// The single multiplexer window's label.
pub const WINDOW: &str = "main";
/// The rail webview's label.
pub const RAIL: &str = "rail";

/// One VS Code server workspace (a code-server the rail can switch to).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    /// Stable id (also the agent session id the server's reporter registers).
    pub id: String,
    /// Display label shown in the rail.
    pub label: String,
    /// The code-server URL Fleet embeds.
    pub url: String,
}

/// Multiplexer state: which server is selected + what URL the editor currently
/// shows + whether the loading overlay is up. The server LIST is the supervisor
/// (spawned) + the push-driven [`crate::bridge::BridgeRegistry`].
#[derive(Default)]
pub struct MuxState {
    pub selected: Mutex<Option<String>>,
    /// The URL currently loaded in the editor surface (so we only re-navigate on
    /// an actual change).
    loaded: Mutex<Option<String>>,
    /// Whether the loading overlay currently covers the editor.
    loading: Mutex<bool>,
}

impl MuxState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The loading-overlay webview label (a spinner that covers the editor while a
/// server's workbench loads, instead of a white screen).
pub const LOADING: &str = "loading";

/// The static spinner page shown in the loading overlay.
fn loading_url() -> String {
    let html = "<!doctype html><html><body style='margin:0;background:rgb(13,15,20);\
color:rgb(138,145,160);font:14px -apple-system,sans-serif;display:flex;\
align-items:center;justify-content:center;height:100vh'><div style='text-align:center'>\
<div style='width:30px;height:30px;border:3px solid rgb(35,42,58);\
border-top-color:rgb(59,130,246);border-radius:50%;margin:0 auto 16px;\
animation:s 0.8s linear infinite'></div>Loading…\
<style>@keyframes s{to{transform:rotate(360deg)}}</style></div></body></html>";
    format!("data:text/html,{}", pct(html))
}

/// Show the loading overlay over the editor.
pub fn show_loading(app: &AppHandle) {
    if let Some(state) = app.try_state::<MuxState>() {
        if let Ok(mut l) = state.loading.lock() {
            *l = true;
        }
    }
    retile(app);
}

/// Hide the loading overlay (reveal the loaded editor).
pub fn hide_loading(app: &AppHandle) {
    if let Some(state) = app.try_state::<MuxState>() {
        if let Ok(mut l) = state.loading.lock() {
            *l = false;
        }
    }
    retile(app);
}

/// Minimal percent-encoding so the loading HTML is a valid `data:` URL.
fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The single editor surface webview label.
pub const EDITOR: &str = "editor";

/// Look up a server's URL by id — a Fleet-spawned server (supervisor) or an
/// externally phoned-home one (registry).
fn server_url(app: &AppHandle, id: &str) -> Option<String> {
    if let Some(sup) = app.try_state::<crate::spawn::ServerSupervisor>() {
        if let Some(s) = sup.servers().into_iter().find(|s| s.id == id) {
            return Some(s.url);
        }
    }
    app.try_state::<crate::bridge::BridgeRegistry>()
        .and_then(|reg| {
            reg.servers()
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.url)
        })
}

/// Build the multiplexer window: the rail + ONE editor surface that navigates
/// between servers on switch (a single full-size webview is stable; multiple
/// occluded/1×1 webviews churn the VS Code connection).
pub fn build_window(app: &mut App) -> tauri::Result<()> {
    let width = 1320.0_f64;
    let height = 860.0_f64;

    let mut builder = tauri::window::WindowBuilder::new(app, WINDOW)
        .title("Fleet")
        .inner_size(width, height)
        .min_inner_size(760.0, 480.0);
    // macOS: overlay the title bar so the top strip passes through to the
    // embedded VS Code's own toolbar (menus / command center / tabs) instead of
    // covering it with an empty native title bar. The traffic lights float over
    // the rail (top-left), which insets its header to clear them.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    let window = builder.build()?;

    // Rail: Fleet's own UI (server list + agent state).
    window.add_child(
        WebviewBuilder::new(RAIL, tauri::WebviewUrl::App("index.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(RAIL_W, height),
    )?;

    // ONE editor surface (blank until a server is selected). Its page-load events
    // raise/lower the loading overlay so you see a spinner — not white — while a
    // server's workbench builds.
    let blank = "about:blank".parse().expect("about:blank is a valid url");
    let load_app = app.handle().clone();
    window.add_child(
        WebviewBuilder::new(EDITOR, tauri::WebviewUrl::External(blank)).on_page_load(
            move |_wv, payload| {
                use tauri::webview::PageLoadEvent;
                match payload.event() {
                    PageLoadEvent::Started => show_loading(&load_app),
                    PageLoadEvent::Finished => {
                        // The HTML is loaded; give the VS Code workbench a beat to
                        // render before revealing it (else a flash of white).
                        let a = load_app.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(1200));
                            let b = a.clone();
                            let _ = a.run_on_main_thread(move || hide_loading(&b));
                        });
                    }
                }
            },
        ),
        LogicalPosition::new(RAIL_W, 0.0),
        LogicalSize::new(width - RAIL_W, height),
    )?;

    // Loading overlay (a spinner), mounted at 1×1 (hidden) on top of the editor.
    let spinner = loading_url().parse().expect("loading data url is valid");
    window.add_child(
        WebviewBuilder::new(LOADING, tauri::WebviewUrl::External(spinner)),
        LogicalPosition::new(RAIL_W, 0.0),
        LogicalSize::new(1.0, 1.0),
    )?;
    tracing::info!("multiplexer window built (awaiting registrations)");

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
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Server> = Vec::new();
    for s in sup.servers().into_iter().chain(registry.servers()) {
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
pub fn select_server(app: AppHandle, id: String) {
    select(&app, id);
}

/// Tauri command: spawn a new code-server and add it to the rail. Returns its id.
#[tauri::command]
pub fn spawn_server(
    app: AppHandle,
    sup: State<'_, crate::spawn::ServerSupervisor>,
) -> Result<String, String> {
    let server = sup.spawn().map_err(|e| e.to_string())?;
    let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());
    Ok(server.id)
}

/// Tauri command: close server `id` (kills the process Fleet spawned).
#[tauri::command]
pub fn close_server(app: AppHandle, sup: State<'_, crate::spawn::ServerSupervisor>, id: String) {
    sup.close(&id);
    let _ = app.emit(crate::bridge::SERVERS_CHANGED, ());
}

/// Switch the editor surface to server `id` (shared by the rail and the menu):
/// navigate the single editor webview to that server's URL, or a loading page if
/// it hasn't phoned home yet. Only navigates when the target URL actually changes
/// (so re-selecting a loaded server doesn't reload it, but a pending→ready
/// transition does).
pub fn select(app: &AppHandle, id: String) {
    let Some(state) = app.try_state::<MuxState>() else {
        return;
    };
    if let Ok(mut sel) = state.selected.lock() {
        *sel = Some(id.clone());
    }
    // Navigate the editor to the server's URL (only if it changed, so re-selecting
    // the same server doesn't reload it). The loading overlay is raised/lowered by
    // the editor's own page-load events (see `build_window`).
    if let Some(target) = server_url(app, &id) {
        if let Ok(mut loaded) = state.loaded.lock() {
            if loaded.as_deref() != Some(target.as_str()) {
                if let (Some(wv), Ok(parsed)) = (app.get_webview(EDITOR), target.parse()) {
                    let _ = wv.navigate(parsed);
                    *loaded = Some(target);
                }
            }
        }
    }
    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.eval("window.__fleetSyncSelection && window.__fleetSyncSelection()");
    }
}

/// Build Fleet's native macOS menu bar: a real Edit menu (so ⌘C/⌘V work in the
/// editor webview), a Server switcher mirroring the rail, plus the standard app /
/// window menus. The webview can't own the OS menu bar, so Fleet provides one
/// wired to the active surface.
pub fn build_menu(app: &mut App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    // Standard app menu (the bold first menu on macOS).
    let app_menu = SubmenuBuilder::new(app, "Fleet")
        .about(None)
        .separator()
        .item(
            &MenuItemBuilder::with_id("cmd:workbench.action.openSettings", "Settings…")
                .build(app)?,
        )
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    // The VS Code menu tree. Each `Cmd(label, id)` forwards a real VS Code command
    // through the bridge; clipboard items are native so they work in the webview.
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
        subs.push(build_sub(app, title, items)?);
    }

    // Server: spawn a new code-server (it phones home) / close the current one.
    // Switching between servers is the rail's job (the dynamic push-registered
    // list); these are the lifecycle actions.
    let server_menu = SubmenuBuilder::new(app, "Server")
        .item(&MenuItemBuilder::with_id("spawn:new", "New Server").build(app)?)
        .item(&MenuItemBuilder::with_id("spawn:close-current", "Close Current Server").build(app)?)
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .fullscreen()
        .separator()
        .close_window()
        .build()?;

    let help_menu = build_sub(app, "Help", HELP)?;

    let mut menu = MenuBuilder::new(app).item(&app_menu);
    for sub in &subs {
        menu = menu.item(sub);
    }
    menu = menu.item(&server_menu).item(&window_menu).item(&help_menu);
    app.set_menu(menu.build()?)?;
    Ok(())
}

/// One entry in a mirrored VS Code menu.
enum MItem {
    /// Forward a VS Code command: `(label, command_id)`.
    Cmd(&'static str, &'static str),
    Sep,
    /// Native clipboard items (reliable in the webview).
    Cut,
    Copy,
    Paste,
}

/// Build a submenu from a list of [`MItem`]s.
fn build_sub(
    app: &App,
    title: &str,
    items: &[MItem],
) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{MenuItemBuilder, SubmenuBuilder};
    let mut b = SubmenuBuilder::new(app, title);
    for it in items {
        b = match it {
            MItem::Sep => b.separator(),
            MItem::Cut => b.cut(),
            MItem::Copy => b.copy(),
            MItem::Paste => b.paste(),
            MItem::Cmd(label, id) => {
                let mi = MenuItemBuilder::with_id(format!("cmd:{id}"), *label).build(app)?;
                b.item(&mi)
            }
        };
    }
    b.build()
}

// ── VS Code's default menu structure (real command ids, forwarded) ───────────
use MItem::{Cmd, Copy as MCopy, Cut as MCut, Paste as MPaste, Sep};

const FILE: &[MItem] = &[
    Cmd("New Text File", "workbench.action.files.newUntitledFile"),
    Cmd("New Window", "workbench.action.newWindow"),
    Sep,
    Cmd("Open File…", "workbench.action.files.openFile"),
    Cmd("Open Folder…", "workbench.action.files.openFolder"),
    Cmd("Open Recent…", "workbench.action.openRecent"),
    Sep,
    Cmd("Save", "workbench.action.files.save"),
    Cmd("Save As…", "workbench.action.files.saveAs"),
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
    Cmd("Command Palette…", "workbench.action.showCommands"),
    Cmd("Open View…", "workbench.action.openView"),
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
    Cmd("Go to File…", "workbench.action.quickOpen"),
    Cmd(
        "Go to Symbol in Workspace…",
        "workbench.action.showAllSymbols",
    ),
    Cmd("Go to Symbol in Editor…", "workbench.action.gotoSymbol"),
    Sep,
    Cmd("Go to Definition", "editor.action.revealDefinition"),
    Cmd("Go to References", "editor.action.goToReferences"),
    Cmd("Go to Line/Column…", "workbench.action.gotoLine"),
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
    Cmd("Add Configuration…", "debug.addConfiguration"),
    Sep,
    Cmd("Toggle Breakpoint", "editor.debug.action.toggleBreakpoint"),
];

const TERMINAL: &[MItem] = &[
    Cmd("New Terminal", "workbench.action.terminal.new"),
    Cmd("Split Terminal", "workbench.action.terminal.split"),
    Sep,
    Cmd("Run Task…", "workbench.action.tasks.runTask"),
    Cmd("Run Build Task…", "workbench.action.tasks.build"),
    Sep,
    Cmd("Kill Terminal", "workbench.action.terminal.kill"),
];

const HELP: &[MItem] = &[
    Cmd("Welcome", "workbench.action.openWalkthrough"),
    Cmd("Show All Commands", "workbench.action.showCommands"),
    Cmd("Editor Playground", "editor.action.inspectTMScopes"),
];

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

    // The single editor surface fills the pane to the right of the rail.
    let pane = LogicalSize::new((w - RAIL_W).max(120.0), h);
    if let Some(wv) = app.get_webview(EDITOR) {
        let _ = wv.set_position(LogicalPosition::new(RAIL_W, 0.0));
        let _ = wv.set_size(pane);
    }

    // The loading overlay covers the editor pane while loading, else shrinks away.
    let loading = app
        .try_state::<MuxState>()
        .and_then(|s| s.loading.lock().ok().map(|g| *g))
        .unwrap_or(false);
    if let Some(wv) = app.get_webview(LOADING) {
        let _ = wv.set_position(LogicalPosition::new(RAIL_W, 0.0));
        let _ = wv.set_size(if loading {
            pane
        } else {
            LogicalSize::new(1.0, 1.0)
        });
    }
}

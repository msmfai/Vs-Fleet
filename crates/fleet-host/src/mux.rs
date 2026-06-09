//! The multiplexer window — Fleet's core value proposition.
//!
//! ONE window hosting the Discord-style **rail** (Fleet's own UI: the list of
//! VS Code *server* workspaces + their agent state) plus ONE embedded **editor
//! surface** (a single webview) that navigates between servers on switch.
//! code-server keeps each workspace's session server-side, so navigating back
//! reattaches its terminals + running agent (the cmux model: window → workspace →
//! surface). A single full-size webview is stable; multiple occluded/1×1 webviews
//! churn the connection or garble the GPU terminal.
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
/// shows. The server LIST is the supervisor (spawned) + the push-driven
/// [`crate::bridge::BridgeRegistry`].
#[derive(Default)]
pub struct MuxState {
    pub selected: Mutex<Option<String>>,
    /// The URL currently loaded in the editor surface (so we only re-navigate on
    /// an actual change).
    loaded: Mutex<Option<String>>,
}

impl MuxState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The single editor surface webview label.
pub const EDITOR: &str = "editor";

/// Look up a server's URL by id, but only after the server's bridge has phoned
/// home. Fleet-spawned servers are recorded by the supervisor immediately so
/// they can be closed while starting; the editor surface should not navigate to
/// them until the bridge registration proves VS Code's extension host is alive.
fn ready_server_url(app: &AppHandle, id: &str) -> Option<String> {
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

    // ONE editor surface (blank until a server is selected). code-server shows its
    // own loading screen while the workbench builds, so no Fleet-side overlay.
    let blank = "about:blank".parse().expect("about:blank is a valid url");
    window.add_child(
        WebviewBuilder::new(EDITOR, tauri::WebviewUrl::External(blank)),
        LogicalPosition::new(RAIL_W, 0.0),
        LogicalSize::new(width - RAIL_W, height),
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
/// navigate the single editor webview to that server's URL. Only navigates when
/// the target URL actually changes (so re-selecting a loaded server doesn't reload
/// it, but a pending→ready transition does). code-server shows its own loading
/// screen while the workbench builds.
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
    if let Some(target) = ready_server_url(app, &id) {
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
}

# VS Code Server Dossier

Date: 2026-06-10

Scope: VS Code Server / `code serve-web` behavior relevant to Fleet's Tauri
multiplexer, with emphasis on Claude Code inside connected servers, terminal text
rendering, and keeping editor tabs connected while switching.

## Detailed Notes

- [Source Notes](source-notes.md)
- [Verification Plan](verification-plan.md)
- [Least-Disruptive Keepalive Plan](least-disruptive-keepalive.md)

## Executive Findings

1. Fleet's bridge cannot be treated as independent of the browser workbench. The
   `fleet-bridge` extension is a workspace extension with `onStartupFinished`; it
   phones home after a VS Code workbench client loads and the remote extension
   host activates. This matches Fleet's recent observed deadlock: waiting for
   bridge registration before navigating the editor can prevent the extension
   from ever activating.

2. Fleet's old one-editor-webview design necessarily disconnected the previous
   VS Code client on tab switch. A WebSocket is owned by the loaded page; when
   Fleet navigates the only editor webview to another server, the old page and
   its client-side sockets go away. That is why bridge generations changed and
   why a terminal UI could visibly rebuild on return.

3. Keeping tabs live means keeping one loaded workbench document alive per
   server. The strongest design direction is a persistent child-webview pool:
   create `editor:<server-id>` once, show the selected one, and hide or move the
   rest without navigating them away. Do not attach multiple browser clients to
   the same VS Code Server instance; upstream docs say the server is designed for
   one user/client at a time.

4. Claude Code failures inside connected servers should be debugged as a
   stack of environment, trust, terminal, and rendering problems, not as a single
   Claude bug. Known upstream points: VS Code GUI launches may not inherit shell
   env vars; Restricted Mode can block terminals; Claude's VS Code extension
   calls out env inheritance/login issues; Claude's own troubleshooting says
   garbled text in editor integrated terminals is usually the terminal GPU
   renderer.

5. Terminal text rendering issues have a direct upstream mitigation:
   set `terminal.integrated.gpuAcceleration` to `"off"` for Fleet-spawned server
   data. VS Code documents GPU-renderer failures, and Claude Code explicitly
   recommends the same setting for boxes, smears, or wrong glyphs in VS Code,
   Cursor, and Devin Desktop integrated terminals.

## Fleet Facts

- Fleet's host now builds one rail webview plus a placeholder editor webview,
  then creates one persistent editor child webview per selected server:
  `crates/fleet-host/src/mux.rs`.
- Keepalive is default-on. `FLEET_EDITOR_KEEPALIVE=0` restores the legacy
  singleton editor path for rollback/comparison.
- Fleet's bridge registry removes a server after the bridge WebSocket drops
  and a short grace period: `crates/fleet-host/src/bridge.rs`.
- `fleet-bridge` is a workspace extension with `activationEvents:
  ["onStartupFinished"]`, so it starts after the workbench reaches startup-finished
  state in that server's remote extension host:
  `packages/fleet-bridge/package.json`.
- The bridge extension keeps the command connection by opening a WebSocket from
  the server-side extension host to Fleet's bridge URL:
  `packages/fleet-bridge/src/extension.ts`.

## Keep-Alive Options

### Preferred: persistent child webview per live server

Model:

- On first selection/spawn, create a child webview with a stable label like
  `editor-server-1`.
- Navigate it once to that server's `code serve-web` URL.
- On tab switch, show the selected webview and hide or move the previous
  webview; do not call `navigate` on either one.
- Retile only the visible editor to the editor pane; keep hidden editors at a
  stable offscreen/minimal layout only if true hide/show causes rendering bugs.
- Keep exactly one workbench client per Fleet server.

Why this is the leading option:

- It preserves the page, VS Code client sockets, extension host connection, and
  terminal DOM/canvas state across switches.
- It matches Tauri's multi-webview API shape: `WebviewBuilder::new(...)` and
  `window.add_child(...)` are documented for child webviews behind the `unstable`
  feature, which Fleet already enables.
- It avoids the current reconnect loop by making tab switch a visibility/layout
  operation rather than a navigation operation.

Risks:

- Memory grows roughly with live workbench count. Fleet needs measured RSS per
  hidden VS Code webview before enabling many persistent tabs by default.
- Hidden WebKit/Wry webviews may throttle timers or have GPU/canvas quirks.
  The current source comment says earlier occluded/1x1 approaches churned
  connections or garbled GPU terminals; that must be retested with hide/show,
  GPU acceleration off, and full screenshots.
- Tauri child webviews need unique labels and careful lifecycle cleanup on
  server close.

### Alternative: warm LRU webview pool

Model:

- Keep the selected server and the most recent N servers live.
- Evict older hidden webviews by closing them, while leaving server processes
  running.

Tradeoff:

- Lower memory than "all tabs live", but it does not satisfy the full goal if
  the user expects every tab to remain connected indefinitely. Treat this as a
  fallback only if VM memory measurements prove unbounded live webviews are not
  viable.

### Alternative: separate hidden windows

Model:

- One hidden `WebviewWindow` per server rather than child webviews inside the
  main Fleet window.

Tradeoff:

- May isolate renderer state better, but it risks macOS window/Alt-Tab artifacts
  and makes tiling/focus harder. It is a fallback if child webview hiding is
  broken.

### Not sufficient: bridge-only keepalive

Keeping only Fleet's bridge WebSocket alive is not enough if the VS Code
workbench page is unloaded. The workbench client owns the browser-side VS Code
session; without a loaded client, terminals and UI state still rebuild on return.
Bridge-only work may be useful for command/control, but it does not solve the
visible disconnect/reconnect problem.

## Claude Code Inside Fleet Servers

The likely fault domains, in priority order:

1. Environment inheritance. VS Code's CLI docs state that separate instances can
   inherit environment from an already-running instance unless isolated with a
   user data directory. Claude's VS Code docs also warn that VS Code may not
   inherit `ANTHROPIC_API_KEY` from the shell. Fleet should explicitly check
   environment inside the spawned integrated terminal, not assume the host shell
   env survived LaunchServices and `code serve-web`.

2. Authentication and workspace trust. Claude's Remote Control docs require a
   logged-in CLI and workspace trust. VS Code terminal docs note terminals are
   blocked in Restricted Mode. Fleet's bridge supports untrusted workspaces, but
   Claude Code itself may still need trust/login state in the server environment.

3. Hook dependencies. Fleet's current Claude shim relies on shell hooks and local
   tooling such as `nc -U`. Claude hooks receive JSON on stdin and execute shell
   commands; missing PATH entries or tools will make agent-state reporting fail
   even if Claude itself starts.

4. Terminal renderer. If Claude appears to corrupt terminal text, apply the GPU
   renderer mitigation before deeper debugging:

   ```json
   {
     "terminal.integrated.gpuAcceleration": "off"
   }
   ```

5. Search/index dependencies. Claude troubleshooting calls out ripgrep problems
   when file discovery is incomplete. Fleet server images should either ship a
   working `rg` or set `USE_BUILTIN_RIPGREP=0` and install a platform package.

## Recommended Next Implementation Shape

1. Change `MuxState.loaded: Option<String>` into a map of server id to editor
   webview metadata: URL, loaded state, visible state, and last active time.

2. Replace `EDITOR` singleton with generated labels:

   ```text
   rail
   editor-server-1
   editor-server-2
   ...
   ```

3. On `select_server(id)`:

   - ensure the server has an editor webview, creating it if missing;
   - if not yet loaded, navigate it to the supervisor/registry URL;
   - hide the previously selected editor webview;
   - show and retile the selected editor webview.

4. Stop using bridge drop as equivalent to server disappearance for Fleet-spawned
   servers. A spawned server should stay in the rail while its supervisor process
   exists, with a bridge status of connected/reconnecting/disconnected.

5. Persist server-side settings for spawned servers before `code serve-web`
   starts:

   ```json
   {
     "terminal.integrated.gpuAcceleration": "off",
     "terminal.integrated.minimumContrastRatio": 1
   }
   ```

   The contrast setting should be tested visually; it fixes color surprises but
   could reduce accessibility.

6. Add VM/container verification before and after the webview-pool change:

   - spawn two servers;
   - wait for both bridge hellos;
   - open terminals in both;
   - run a long command in one terminal;
   - switch tabs repeatedly;
   - assert bridge generation is stable and no server deregisters;
   - capture full Fleet-window screenshots including rail and editor;
   - record Fleet host RSS, `code serve-web` RSS, renderer process RSS, and total
     process count.

## Open Questions

- Does Tauri/Wry `Webview::hide()` preserve WebKit networking and VS Code
  terminal canvas state on macOS, or should Fleet keep hidden editors visible but
  offscreen?
- Does `code serve-web` honor a prewritten settings file under Fleet's current
  `--server-data-dir`, or does it need a separate user-data path/settings path?
- Does Fleet need to launch `claude doctor` automatically inside a new server to
  expose missing login/trust/PATH state in the rail?
- What is the actual memory slope per additional live hidden VS Code workbench on
  the current macOS WebKit stack?

## Source Notes

See [source-notes.md](source-notes.md) for links and extracted implications.
See [verification-plan.md](verification-plan.md) for the concrete tests to run
before coding the persistent-webview implementation.

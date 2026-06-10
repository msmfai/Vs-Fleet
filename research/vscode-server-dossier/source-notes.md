# Source Notes

This file records upstream facts used by the dossier and the Fleet-specific
inference drawn from them.

## VS Code Server And Web Workbench

- VS Code Server docs: <https://code.visualstudio.com/docs/remote/vscode-server>

  Upstream facts:

  - The VS Code Server is a backend service for remote/browser VS Code.
  - Access is built into the `code` CLI.
  - The server is designed for a single user.
  - Hosting it as a service is not allowed by the VS Code Server license.
  - Pure UI extensions are not supported in web-based server instances.

  Fleet implications:

  - Fleet can run `code serve-web` for personal/own-hardware dogfooding, but the
    product line still needs the documented license boundary from `NORTH_STAR.md`.
  - Do not create multiple simultaneous workbench clients for the same server.
    Persistent tabs should mean one hidden/visible client per Fleet server.

- VS Code for the Web docs: <https://code.visualstudio.com/docs/remote/vscode-web>

  Upstream facts:

  - Browser VS Code has limitations compared with desktop.
  - Web extension support is partial.
  - Browser-specific behavior can differ, especially Webviews in Firefox/Safari.
  - Keyboard shortcuts and browser sandboxing differ from desktop.

  Fleet implications:

  - Fleet's embedded WebKit workbench is closer to browser VS Code than desktop
    VS Code. Pixel, keyboard, clipboard, popup, and extension behavior all need
    direct verification in the Fleet app, not only in Chrome.

- VS Code CLI docs: <https://code.visualstudio.com/docs/configure/command-line>

  Upstream facts:

  - Additional VS Code instances can inherit environment variables from an
    already-running instance.
  - A unique `--user-data-dir` isolates environment, settings, extensions, and UI
    state.
  - macOS may need the `code` command manually installed into PATH.

  Fleet implications:

  - Fleet-spawned servers need explicit binary discovery and explicit per-server
    state directories.
  - Claude-related env should be validated from inside the spawned terminal.

## Extension Host, Activation, And Fleet Bridge

- Remote extension authoring docs:
  <https://code.visualstudio.com/api/advanced-topics/remote-extensions>

  Upstream facts:

  - Workspace extensions run where the workspace is located.
  - Workspace extensions can access workspace files and run scripts/tools in that
    environment.
  - An extension running in the wrong location should be fixed with
    `extensionKind`.

  Fleet implications:

  - `fleet-bridge` correctly declares `extensionKind: ["workspace"]`; it needs
    the remote/server Node extension host, not a browser-only extension host.

- Extension Host docs:
  <https://code.visualstudio.com/api/advanced-topics/extension-host>

  Upstream facts:

  - VS Code can have local, web, and remote extension hosts.
  - Node.js extension hosts are used for local and remote extension hosts.
  - Browser extensions need a `browser` entry file.

  Fleet implications:

  - Fleet's bridge is a Node workspace extension, so it should run in the server
    extension host once the workbench connects.

- Activation Events docs:
  <https://code.visualstudio.com/api/references/activation-events>

  Upstream facts:

  - `onStartupFinished` activates some time after VS Code starts.
  - `*` activates at startup but is discouraged unless no other event works.

  Fleet implications:

  - `fleet-bridge` using `onStartupFinished` means phone-home is expected only
    after a client has loaded the workbench far enough. Fleet should not wait for
    phone-home before first navigation.

## Terminal Rendering

- VS Code terminal appearance docs:
  <https://code.visualstudio.com/docs/terminal/appearance>

  Upstream facts:

  - VS Code terminals can have GPU-accelerated rendering problems in some
    environments.
  - The documented workaround is launching with `--disable-gpu` or setting
    `terminal.integrated.gpuAcceleration` to `off`.
  - Minimum contrast can change terminal colors and can be disabled with
    `terminal.integrated.minimumContrastRatio: 1`.

  Fleet implications:

  - Fleet should write GPU-off terminal settings for spawned server profiles
    before investigating deeper text corruption.
  - Screenshots should explicitly include colored terminal output so contrast
    changes are visible.

- VS Code terminal basics:
  <https://code.visualstudio.com/docs/terminal/basics>

  Upstream facts:

  - Integrated terminals run commands like standalone terminals.
  - Terminals are blocked in Restricted Mode.
  - Shell integration tracks command execution and terminal output.
  - Scrollback defaults to a finite line count.

  Fleet implications:

  - Claude smoke tests need to check workspace trust and terminal availability.
  - Fleet's bridge terminal-buffer capture depends on shell integration being
    available, so tests must record whether shell integration subscribed.

## Claude Code

- Claude Code overview: <https://code.claude.com/docs/en/overview>

  Upstream facts:

  - Claude Code is available in terminal, IDE, desktop app, and browser.
  - It reads code, edits files, runs commands, uses MCP, and supports hooks.
  - The CLI starts in a project directory and prompts login on first use.

  Fleet implications:

  - Fleet's agent-state path is aligned with the CLI/hook model, but new servers
    must have login, filesystem access, and hook dependencies available.

- Claude Code VS Code integration:
  <https://code.claude.com/docs/en/ide-integrations>

  Upstream facts:

  - The VS Code extension includes the CLI and recommends the graphical extension
    for VS Code.
  - If `ANTHROPIC_API_KEY` is set in a shell but sign-in still appears, VS Code
    may not have inherited the shell environment.
  - The Spark icon requires a file open and the extension does not work in
    Restricted Mode.
  - The extension can run multiple conversations and exposes status for hidden
    tabs.

  Fleet implications:

  - A Fleet server may fail Claude auth simply because the VS Code server process
    did not receive expected env. Test from inside the integrated terminal.
  - If Fleet later chooses to integrate Anthropic's VS Code extension instead of
    only CLI hooks, it must treat Claude session tabs as VS Code editor state that
    needs persistent webview lifetime too.

- Claude Code troubleshooting:
  <https://code.claude.com/docs/en/troubleshooting>

  Upstream facts:

  - High CPU/memory can come from large codebases, plugins, MCP servers, or hooks.
  - `/doctor` and `claude doctor` are the first-line health checks.
  - Garbled or corrupted text in editor integrated terminals is likely the GPU
    renderer; `/terminal-setup` sets `terminal.integrated.gpuAcceleration` to
    `off`.
  - Search problems can come from the bundled ripgrep binary not running.

  Fleet implications:

  - Add a Claude health-check behavior to the VM/container suite:
    `claude doctor`, `claude --version`, terminal GPU setting, and a trivial
    non-mutating prompt where credentials are available.

- Claude Code hooks reference: <https://code.claude.com/docs/en/hooks>

  Upstream facts:

  - Hooks run at specific lifecycle events and receive JSON context on stdin.
  - Hooks can be command, HTTP, prompt, MCP, or async hooks.
  - `CLAUDE_PROJECT_DIR` is available for project-root-relative hook paths.
  - Hook scripts should use absolute paths and quote shell variables.

  Fleet implications:

  - Fleet's hook shim should avoid relying on an interactive shell profile.
  - Hook failures should be logged separately from Claude startup failures.

- Claude Code Remote Control:
  <https://code.claude.com/docs/en/remote-control>

  Upstream facts:

  - Remote Control keeps the session running locally and remote surfaces are
    windows into that local session.
  - It requires login and workspace trust.
  - It can reconnect after sleep or network drop.

  Fleet implications:

  - The architecture reinforces the same principle Fleet needs: keep the runtime
    session alive, and make UI surfaces attach without moving execution to the UI.

## Tauri / Webview / WebSocket

- Tauri `WebviewBuilder` docs:
  <https://docs.rs/tauri/2.11.2/tauri/webview/struct.WebviewBuilder.html>

  Upstream facts:

  - Tauri 2 has an unstable `WebviewBuilder` for child webviews.
  - `window.add_child(...)` is the documented way to add a webview to a native
    window.
  - Webview labels must be unique.
  - `on_page_load` can observe started/finished page loads.
  - `background_throttling(...)` can change the default hidden/minimized browser
    suspend behavior. Tauri documents that browsers may throttle timers or unload
    a hidden view after roughly five minutes by default; the policy is supported
    on macOS 14+ and unsupported on Linux, Windows, and Android.

  Fleet implications:

  - Fleet already enables Tauri's `unstable` feature. A persistent child-webview
    pool is a direct extension of the API currently used for rail + editor.
  - Persistent hidden VS Code tabs should be built with background throttling
    disabled on supported macOS versions, then verified with a greater-than-five
    minute hidden-tab soak.

- Tauri `Webview` docs:
  <https://docs.rs/tauri/2.11.2/tauri/webview/struct.Webview.html>

  Upstream facts:

  - Child webviews can be navigated, resized, moved, focused, hidden, shown, and
    closed through `Webview` methods.

  Fleet implications:

  - Fleet does not need a new window model to prototype persistent tabs. The
    existing child-webview host can switch editor visibility in place.

- MDN WebSocket docs:
  <https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>
  and <https://developer.mozilla.org/en-US/docs/Web/API/WebSocket/close_event>

  Upstream facts:

  - `WebSocket` creates and manages a connection from a page to a server.
  - The `close` event fires when that connection is closed.
  - The API has no backpressure; excessive incoming messages can consume memory
    or CPU.

  Fleet implications:

  - Navigating away from a server's loaded workbench destroys the page that owns
    its sockets. Persistent tabs need persistent pages.
  - Hidden-tab soak tests should monitor buffered traffic and memory, not only
    connection status.

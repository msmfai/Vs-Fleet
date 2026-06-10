# Verification Plan

This is the proof plan for the research claims before Fleet changes its editor
switching architecture.

## 1. Current Baseline

Goal: prove the current one-webview navigation model reconnects.

Procedure:

1. Launch Fleet.
2. Spawn two VS Code servers.
3. Wait for both to register.
4. Record bridge generation for each server.
5. Open a terminal in each server and run:

   ```sh
   printf 'FLEET_TERMINAL_%s\n' "$FLEET_SERVER_ID"
   ```

6. Switch between servers ten times.
7. Record:

   - bridge generations;
   - deregister/register events;
   - terminal count and visible terminal text through bridge snapshot;
   - full Fleet-window screenshot including rail and editor;
   - Fleet host RSS, VS Code server RSS, webview renderer RSS, process count.

Expected current result: at least the inactive server's workbench client reloads
or reconnects when its URL is reloaded into the singleton editor webview.

## 2. Persistent Child Webview Prototype

Goal: prove a hidden child webview keeps one server connected while another is
selected.

Procedure:

1. Implement a prototype branch with one child webview per spawned server.
2. Use a unique webview label per server.
3. Show/hide child webviews on selection without navigating already-loaded
   webviews.
4. Repeat the baseline procedure.

Acceptance:

- bridge generation remains stable for both servers through tab switches;
- no `server deregistered (bridge dropped)` events during switches;
- terminal output remains visible when switching back;
- screenshots show no black artifacts, blank panes, cropped editor, or missing rail;
- memory slope per extra hidden server is recorded.

## 3. Rendering Mitigation Test

Goal: verify whether terminal GPU settings fix Claude/terminal text corruption.

Procedure:

1. Prewrite spawned-server settings with:

   ```json
   {
     "terminal.integrated.gpuAcceleration": "off"
   }
   ```

2. Open a terminal and render:

   ```sh
   printf '\033[31mred\033[0m \033[32mgreen\033[0m \033[34mblue\033[0m\n'
   printf 'box: [] braces: {} arrows: -> <- unicode: lambda\n'
   ```

3. Capture screenshots with GPU setting on and off.
4. Keep a text assertion via bridge snapshot and a visual assertion via screenshot.

Acceptance:

- no smears, boxes, black pane, or glyph substitution in GPU-off screenshot;
- bridge terminal text still captures the expected output.

## 4. Claude Smoke Test

Goal: identify whether Claude failures are env/auth/trust/hook/rendering.

Procedure:

1. In a Fleet-spawned server terminal, run:

   ```sh
   which claude
   claude --version
   claude doctor
   env | sort | rg 'ANTHROPIC|CLAUDE|FLEET|PATH'
   ```

2. If credentials are available, run a minimal prompt:

   ```sh
   claude -p 'reply with FLEET_CLAUDE_OK only'
   ```

3. Inspect Fleet reporter logs and hook relay logs.

Record:

- exit code and terminal text;
- whether `/doctor` reports auth, PATH, MCP, hook, or context issues;
- whether Fleet agent state transitions fire;
- whether rendering corrupts the output visually.

Acceptance:

- failure class is identified, not just "Claude did not work";
- if the command succeeds, Fleet's rail shows the expected agent-state lifecycle.

## 5. Memory Guardrail

Goal: avoid another local RAM overflow while testing live tabs.

Procedure:

1. Run 1, 2, 4, 8, and 12 live server tabs.
2. At each level, record:

   ```sh
   ps -axo pid,ppid,rss,command | rg 'fleet-host|code serve-web|Code Helper|WebKit|fleet-reporter'
   ```

3. Sum RSS by role.
4. Stop before host memory pressure becomes visible.

Acceptance:

- report includes per-server memory slope;
- persistent-webview default tab count is chosen from evidence rather than guesswork.

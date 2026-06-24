// Shared helpers for the Layer-D tauri-driver E2E specs.
//
// Two themes, both proven by rename.e2e.js:
//   1. LAUNCHER→WORKER handoff: wdio.conf onPrepare (launcher process) writes a
//      JSON config file; specs (worker process) read it lazily in `before` — never
//      at module load. `loadE2EConfig()` does the read; the path is derived
//      identically to wdio.conf.js.
//   2. RENDERING-INDEPENDENCE: WebKitWebGTK under Xvfb is software-rendered, so
//      WebDriver's rendering-dependent ops (`getText`, `isDisplayed`/
//      `waitForDisplayed`, coordinate `.click()`/right-click) are unreliable. Every
//      read/action here goes through `browser.execute` (textContent + dispatched
//      DOM events), so it depends only on the live, correct DOM.
//
// Sessions get into the rail two ways, mirroring production:
//   • BRIDGE phone-home (`hello` over ws://127.0.0.1:<bridgePort>): registers a
//     *server* row (the fleet-bridge VS Code extension path). No agent state.
//   • HUB reporter delta (`session.upsert` over ws://127.0.0.1:<hubPort>): produces
//     an *agent* inbox tab keyed by `session_id` (the reporter path). This is what
//     mute/solo/dismiss/unread need — they gate on `agentFor(id)` + `inbox.connected`.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// Derived IDENTICALLY to wdio.conf.js (fixed runtime dir under tmp).
export const RUNTIME_DIR = path.join(os.tmpdir(), "fleet-e2e-run");
export const CONFIG_PATH = path.join(RUNTIME_DIR, "e2e-config.json");

// The embedded Fleet Hub the host starts in rail-only mode (no explicit hub URL):
// fleet_hub::DEFAULT_WS_PORT. Reporters connect here to push agent/session state.
export const HUB_PORT = 51777;

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Load the launcher→worker handoff file. Call inside `before` (after onPrepare),
// NOT at module load.
export function loadE2EConfig() {
  if (!fs.existsSync(CONFIG_PATH)) {
    throw new Error(
      `shared E2E config file missing at ${CONFIG_PATH} (wdio.conf onPrepare must run first)`
    );
  }
  const cfg = JSON.parse(fs.readFileSync(CONFIG_PATH, "utf8"));
  if (!cfg.runtimeDir || !cfg.bridgePort || !cfg.editorUrl) {
    throw new Error("incomplete E2E config");
  }
  return cfg;
}

// Wait for + read the bridge launch token the app writes to <runtime>/bridge.token.
export function readBridgeToken(E2E) {
  const tokenPath = path.join(E2E.runtimeDir, "bridge.token");
  return browser
    .waitUntil(
      async () => {
        if (!fs.existsSync(tokenPath)) return false;
        const t = fs.readFileSync(tokenPath, "utf8").trim();
        return t.length ? t : false;
      },
      { timeout: 30000, interval: 250, timeoutMsg: `bridge token never appeared at ${tokenPath}` }
    )
    .then(() => fs.readFileSync(tokenPath, "utf8").trim());
}

// Open a bridge WS and send one `hello` (registers a SERVER row). The bridge reads
// only the FIRST hello per connection, so re-registering an id needs a NEW socket
// (how a reconnecting reporter behaves). Returns the open socket; caller closes it.
export function phoneHome(E2E, token, { serverId, label, url }) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${E2E.bridgePort}`);
    const timer = setTimeout(() => reject(new Error("bridge ws connect timeout")), 30000);
    ws.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        ws.send(
          JSON.stringify({
            type: "hello",
            server_id: serverId,
            url: url ?? E2E.editorUrl,
            token,
            ...(label != null ? { label } : {}),
          })
        );
        resolve(ws);
      },
      { once: true }
    );
    ws.addEventListener(
      "error",
      (e) => {
        clearTimeout(timer);
        reject(new Error(`bridge ws error: ${e?.message ?? e}`));
      },
      { once: true }
    );
  });
}

// Build a protocol-correct `session.upsert` ClientMessage (the reporter delta the
// embedded Hub folds into agent/inbox state). Shape mirrors fleet_protocol::Session
// (schema_version 1, kebab-case enums) + one AgentRun. `state` is a State token
// ("working"|"waiting"|"idle"|"done"|"error"|"dead"); "waiting" is the only
// attention/ping state and is what drives the unread dot.
export function sessionUpsertFrame({
  sessionId,
  title,
  state = "idle",
  agentKind = "claude-code",
  lastMessage = null,
  waitingSince = null,
  updatedAt = "2026-06-08T00:00:00Z",
}) {
  const run = {
    schema_version: 1,
    run_id: `${sessionId}-run-1`,
    agent_kind: agentKind,
    native_id: sessionId,
    cwd: "/tmp",
    state,
    confidence: "high",
    updated_at: updatedAt,
    ...(lastMessage != null ? { last_message: lastMessage } : {}),
    ...(waitingSince != null ? { waiting_since: waitingSince } : {}),
  };
  const session = {
    schema_version: 1,
    session_id: sessionId,
    title: title ?? sessionId,
    location: { kind: "local", label: "laptop", glyph: "laptop" },
    server: { kind: "local" },
    runs: [run],
    rollup_state: state,
    updated_at: updatedAt,
  };
  return JSON.stringify({ type: "session.upsert", session });
}

// Connect a reporter-style WS to the embedded Hub and push a `session.upsert` so
// the rail gains a real agent inbox tab for `sessionId`. Returns the open socket;
// caller closes it. (The Hub keeps the session while the delta stands — a later
// `session.remove` or dismiss command removes it.)
export function pushHubSession(opts) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${HUB_PORT}`);
    const timer = setTimeout(() => reject(new Error("hub ws connect timeout")), 30000);
    ws.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        ws.send(sessionUpsertFrame(opts));
        resolve(ws);
      },
      { once: true }
    );
    ws.addEventListener(
      "error",
      (e) => {
        clearTimeout(timer);
        reject(new Error(`hub ws error: ${e?.message ?? e}`));
      },
      { once: true }
    );
  });
}

// ── DOM-driven, rendering-independent primitives ─────────────────────────────

export const rowSel = (id) => `.srv[data-server-id="${id}"]`;
export const rowLabelSel = (id) => `${rowSel(id)} .label`;

// textContent of the first match (NOT getText, which needs rendering). null if absent.
export function readText(selector) {
  return browser.execute((sel) => {
    const el = document.querySelector(sel);
    return el ? el.textContent : null;
  }, selector);
}

// Does an element match `selector`? (Existence is rendering-independent.)
export function exists(selector) {
  return browser.execute((sel) => !!document.querySelector(sel), selector);
}

// className string of the first match (so the caller can assert on state classes).
export function classOf(selector) {
  return browser.execute((sel) => {
    const el = document.querySelector(sel);
    return el ? el.className : null;
  }, selector);
}

// Poll until predicate(value) is true, reading via `read()` each tick.
export function waitFor(read, predicate, timeoutMsg, opts = {}) {
  return browser.waitUntil(async () => predicate(await read()), {
    timeout: opts.timeout ?? 20000,
    interval: opts.interval ?? 250,
    timeoutMsg,
  });
}

export function waitText(selector, expected, timeoutMsg, opts) {
  return waitFor(() => readText(selector), (t) => t === expected, timeoutMsg, opts);
}

export function waitExists(selector, timeoutMsg, opts) {
  return waitFor(() => exists(selector), (v) => v === true, timeoutMsg, opts);
}

export function waitGone(selector, timeoutMsg, opts) {
  return waitFor(() => exists(selector), (v) => v === false, timeoutMsg, opts);
}

export function waitClassContains(selector, cls, timeoutMsg, opts) {
  return waitFor(
    () => classOf(selector),
    (c) => typeof c === "string" && c.split(/\s+/).includes(cls),
    timeoutMsg,
    opts
  );
}

// Dispatch a real DOM event on the first match. `type` is the event name; `kind`
// selects the constructor ("mouse" → MouseEvent, "keyboard" → KeyboardEvent,
// default → Event). Returns whether the element was present.
export function dispatchOn(selector, type, kind = "mouse", init = {}) {
  return browser.execute(
    (sel, evType, evKind, evInit) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      let ev;
      if (evKind === "keyboard") {
        ev = new KeyboardEvent(evType, { bubbles: true, cancelable: true, ...evInit });
      } else if (evKind === "mouse") {
        ev = new MouseEvent(evType, { bubbles: true, cancelable: true, ...evInit });
      } else {
        ev = new Event(evType, { bubbles: true, cancelable: true, ...evInit });
      }
      el.dispatchEvent(ev);
      return true;
    },
    selector,
    type,
    kind,
    init
  );
}

// Click an element by id via the DOM (el.click()), not a coordinate click.
export function clickById(id) {
  return browser.execute((elId) => {
    const el = document.getElementById(elId);
    if (!el) return false;
    el.click();
    return true;
  }, id);
}

// Open a row's context menu by dispatching a real `contextmenu` event on the row
// (→ row.oncontextmenu → openRowMenu → renderRowMenu). Returns whether the row
// existed.
export function openRowMenu(sessionId) {
  return dispatchOn(rowSel(sessionId), "contextmenu", "mouse", { clientX: 10, clientY: 10 });
}

// Answer the in-DOM prompt overlay (#prompt-input): set value + dispatch Enter
// keydown → input.onkeydown reads value → closePrompt(value). Returns presence.
export function answerPrompt(value) {
  return browser.execute((text) => {
    const input = document.getElementById("prompt-input");
    if (!input) return false;
    input.focus();
    input.value = text;
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    return true;
  }, value);
}

// Is the prompt overlay open? (#prompt loses the `hidden` class in domPrompt.)
export function promptOpen() {
  return browser.execute(() => {
    const p = document.getElementById("prompt");
    const input = document.getElementById("prompt-input");
    return !!input && !!p && !p.classList.contains("hidden");
  });
}

// Layer D — real-UI E2E for the Fleet host rename flow.
//
// Drives the REAL built fleet-host binary (launched by tauri-driver in its
// FLEET_E2E_RAIL_ONLY composition, so the rail is the WebDriver-reachable
// top-level webview) through the user-visible rename flow and asserts observable
// rail DOM. Then it regression-locks the exact bug the whole effort exists for:
// a fresh reporter re-register with the AUTO label must NOT revert the rename.
//
// How a session gets into the rail (mirrors scripts/release-smoke.mjs and the
// fleet-bridge VS Code extension): we open a WebSocket to the host's phone-home
// bridge (ws://127.0.0.1:51778) and send a `hello` frame carrying the launch
// token read from <runtime>/bridge.token. That registration IS how a row appears
// — there is no static server list.
//
// Determinism: every wait is an explicit poll on a backend/DOM condition, never a
// fixed sleep, except a single short settle after the rename invoke to let the
// `servers-changed` → refreshServers re-render flush before asserting non-revert.
//
// Rendering note: WebKitWebGTK under Xvfb falls back to software rendering, which
// makes WebDriver's RENDERING-dependent ops (`getText()`, `isDisplayed()`/
// `waitForDisplayed()`, coordinate-based `.click()`/right-click) unreliable — yet
// the DOM itself is fully correct and functional (the IPC/bridge/render path all
// work). So this spec drives and reads everything through the DOM via
// `browser.execute` (dispatched DOM events + `textContent`), never through
// rendering-dependent calls. It stays a REAL E2E: real rail webview + real IPC
// (`window.__TAURI__` → `rename_server`) + real bridge phone-home; only the input
// mechanism is dispatched events instead of synthetic OS input.

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// Shared config is handed off from the wdio LAUNCHER process (onPrepare) to this
// WORKER process via a FILE, because globalThis/module state does NOT cross the
// process boundary. We derive the config-file path IDENTICALLY to wdio.conf.js (a
// fixed runtime dir under tmp) and READ it lazily inside `before` — never at module
// load, which would race ahead of onPrepare writing it.
const RUNTIME_DIR = path.join(os.tmpdir(), "fleet-e2e-run");
const CONFIG_PATH = path.join(RUNTIME_DIR, "e2e-config.json");
let E2E;

const SERVER_ID = "e2e-session-1";
const AUTO_LABEL = "auto reported"; // what the reporter phones home with
const RENAMED_LABEL = "My Renamed Project"; // what the user types

const ROW = `.srv[data-server-id="${SERVER_ID}"]`;
const ROW_LABEL = `${ROW} .label`;

// ── helpers ──────────────────────────────────────────────────────────────────

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function readBridgeToken() {
  const tokenPath = path.join(E2E.runtimeDir, "bridge.token");
  return browser.waitUntil(
    async () => {
      if (!fs.existsSync(tokenPath)) return false;
      const t = fs.readFileSync(tokenPath, "utf8").trim();
      return t.length ? t : false;
    },
    {
      timeout: 30000,
      interval: 250,
      timeoutMsg: `bridge token never appeared at ${tokenPath}`,
    }
  ).then(() => fs.readFileSync(tokenPath, "utf8").trim());
}

// Open a bridge WS and send one `hello`. The bridge only reads the FIRST hello
// per connection, so re-registering the same id needs a NEW socket — exactly how
// a reconnecting reporter behaves. Returns the open socket (caller closes it).
function phoneHome(token, label) {
  // Node >= 22 has a global WebSocket; the wdio worker runs under it.
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
            server_id: SERVER_ID,
            url: E2E.editorUrl,
            label,
            token,
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

// ── DOM-driven helpers (rendering-independent) ───────────────────────────────
// All reads/actions go through browser.execute so they depend only on the live
// DOM, not on WebKitGTK's (software-rendered, unreliable) layout/paint.

// Read the row's label via textContent (NOT getText(), which needs rendering).
function readRowLabel() {
  return browser.execute((sel) => {
    const el = document.querySelector(sel);
    return el ? el.textContent : null;
  }, ROW_LABEL);
}

// Poll the row label until it equals `expected`.
function waitRowLabel(expected, timeoutMsg) {
  return browser.waitUntil(async () => (await readRowLabel()) === expected, {
    timeout: 20000,
    interval: 250,
    timeoutMsg,
  });
}

// Open the row context menu by dispatching a real `contextmenu` MouseEvent on the
// row element → fires `row.oncontextmenu` → openRowMenu → renderRowMenu (main.js).
// Returns whether the row existed to dispatch on.
function openRowContextMenu() {
  return browser.execute((rowSel) => {
    const row = document.querySelector(rowSel);
    if (!row) return false;
    row.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })
    );
    return true;
  }, ROW);
}

// Click the "Rename" menu item. renderRowMenu gives each button id
// `row-menu-${item.id}`, and the rename item's id is "rename" → `#row-menu-rename`.
// Returns whether the button was present to click.
function clickRenameMenuItem() {
  return browser.execute(() => {
    const btn = document.getElementById("row-menu-rename");
    if (!btn) return false;
    btn.click();
    return true;
  });
}

// Answer the in-DOM prompt overlay: set #prompt-input.value and dispatch a real
// Enter keydown → input.onkeydown reads input.value → closePrompt(value) resolves
// domPrompt → renameRow invokes rename_server (main.js domPrompt/closePrompt).
// Returns whether the input was present.
function answerPrompt(value) {
  return browser.execute((text) => {
    const input = document.getElementById("prompt-input");
    if (!input) return false;
    input.focus();
    input.value = text;
    input.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })
    );
    return true;
  }, value);
}

// ── the suite ────────────────────────────────────────────────────────────────

describe("Fleet rail — rename flow (real UI)", () => {
  let token;
  let helloWs;

  before(async () => {
    // Load the launcher→worker handoff file written by wdio.conf onPrepare. Read
    // here (in the worker, after onPrepare ran), NOT at module load.
    assert.ok(
      fs.existsSync(CONFIG_PATH),
      `shared E2E config file missing at ${CONFIG_PATH} (wdio.conf onPrepare must run first)`
    );
    E2E = JSON.parse(fs.readFileSync(CONFIG_PATH, "utf8"));
    assert.ok(E2E.runtimeDir && E2E.bridgePort && E2E.editorUrl, "incomplete E2E config");

    // The rail must have booted: its status pill exists once main.js init runs.
    await $("#status").waitForExist({ timeout: 60000 });
    token = await readBridgeToken();
    helloWs = await phoneHome(token, AUTO_LABEL);
  });

  after(() => {
    try {
      helloWs?.close();
    } catch {}
  });

  it("shows the phoned-home session as a rail row with its reported label", async () => {
    // Wait for the row to exist in the DOM (existence is rendering-independent).
    await $(ROW).waitForExist({ timeout: 45000 });

    // The row's label is the auto-reported one before any rename. Read via
    // textContent through the DOM, not getText() (which needs reliable rendering).
    await waitRowLabel(AUTO_LABEL, `row label never became "${AUTO_LABEL}"`);
    assert.equal(await readRowLabel(), AUTO_LABEL);
  });

  it("renames the row via the context menu + prompt overlay", async () => {
    await $(ROW).waitForExist({ timeout: 20000 });

    // Open the row context menu by dispatching a real `contextmenu` event on the
    // row (→ openRowMenu → renderRowMenu), then click the "Rename" item. Both go
    // through the DOM so they don't depend on coordinate hit-testing/paint.
    assert.equal(await openRowContextMenu(), true, "row missing for contextmenu dispatch");
    await browser.waitUntil(
      async () =>
        browser.execute(() => !!document.getElementById("row-menu-rename")),
      { timeout: 10000, interval: 200, timeoutMsg: "row menu / rename item never rendered" }
    );
    assert.equal(await clickRenameMenuItem(), true, "rename menu item missing");

    // Answer the in-DOM prompt overlay (#prompt-input): set the value and dispatch
    // a real Enter keydown → closePrompt(value) → renameRow → invoke rename_server.
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const p = document.getElementById("prompt");
          const input = document.getElementById("prompt-input");
          // Overlay is shown when #prompt loses the `hidden` class (main.js domPrompt).
          return !!input && !!p && !p.classList.contains("hidden");
        }),
      { timeout: 10000, interval: 200, timeoutMsg: "rename prompt overlay never opened" }
    );
    assert.equal(await answerPrompt(RENAMED_LABEL), true, "prompt input missing");

    // ASSERT the observable DOM: the row's .label textContent becomes the new label.
    await waitRowLabel(RENAMED_LABEL, `row label never became "${RENAMED_LABEL}" after rename`);
    assert.equal(await readRowLabel(), RENAMED_LABEL);
  });

  it("keeps the renamed label when the reporter re-registers with the AUTO label", async () => {
    // The exact regression: a FRESH phone-home (new socket) carrying the AUTO
    // label must not clobber the user rename. The host pins it via the `renamed`
    // flag in the bridge registry (see bridge.rs::register).
    const reWs = await phoneHome(token, AUTO_LABEL);
    try {
      // Let the re-register + servers-changed → refreshServers re-render flush.
      // Poll the DOM (textContent) and require it to STAY the renamed value: a
      // revert bug would surface as the label flipping back to AUTO_LABEL.
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        assert.equal(
          await readRowLabel(),
          RENAMED_LABEL,
          "renamed label reverted to the auto-reported one after re-register"
        );
        await sleep(500);
      }
      // Final hard assertion after the observation window.
      assert.equal(await readRowLabel(), RENAMED_LABEL);
    } finally {
      try {
        reWs.close();
      } catch {}
    }
  });
});

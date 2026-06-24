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
    const row = await $(ROW);
    await row.waitForExist({ timeout: 45000 });

    // DIAGNOSTIC (temporary): the row renders but its label text was wrong in CI;
    // dump the backend server list + IPC state + rendered label so we can see the
    // real srv.label/renamed and whether the webview has Tauri IPC. Printed to the
    // CI log as `E2E-DIAG ...`. Removed once the render path is fixed.
    const diag = await browser.execute(async () => {
      const t = window.__TAURI__;
      let servers = null;
      let invokeErr = null;
      try {
        servers = t ? await t.core.invoke("get_servers") : null;
      } catch (e) {
        invokeErr = String(e);
      }
      const rail = document.getElementById("rail");
      const row = document.querySelector('.srv[data-server-id="e2e-session-1"]');
      return {
        hasTauri: !!t,
        servers,
        invokeErr,
        renderedLabel: row ? row.querySelector(".label")?.textContent : null,
        railText: rail ? rail.textContent.slice(0, 300) : null,
      };
    });
    // eslint-disable-next-line no-console
    console.log("E2E-DIAG", JSON.stringify(diag));

    const label = await $(ROW_LABEL);
    // The row's visible label is the auto-reported one before any rename.
    await browser.waitUntil(async () => (await label.getText()) === AUTO_LABEL, {
      timeout: 20000,
      interval: 250,
      timeoutMsg: `row label never became "${AUTO_LABEL}"`,
    });
    assert.equal(await label.getText(), AUTO_LABEL);
  });

  it("renames the row via the context menu + prompt overlay", async () => {
    const row = await $(ROW);
    await row.waitForExist({ timeout: 20000 });

    // Open the row context menu (right-click the row), then click "Rename". The
    // rename menu button is rendered with id `row-menu-rename` (main.js).
    await row.click({ button: "right" });
    const renameItem = await $("#row-menu-rename");
    await renameItem.waitForDisplayed({ timeout: 10000 });
    await renameItem.click();

    // The in-DOM prompt overlay (#prompt-input) replaces window.prompt (which
    // returns null in WKWebView). It is pre-filled with the current label.
    const input = await $("#prompt-input");
    await input.waitForDisplayed({ timeout: 10000 });
    // Clear the prefilled value, type the new label, commit with Enter.
    await input.click();
    // Select-all + delete is webview-safe; main.js also `.select()`s on open.
    await browser.keys(["Control", "a"]);
    await browser.keys("Delete");
    await input.setValue(RENAMED_LABEL);
    await browser.keys("Enter");

    // ASSERT the observable DOM: the row's .label text becomes the new label.
    const label = await $(ROW_LABEL);
    await browser.waitUntil(async () => (await label.getText()) === RENAMED_LABEL, {
      timeout: 20000,
      interval: 250,
      timeoutMsg: `row label never became "${RENAMED_LABEL}" after rename`,
    });
    assert.equal(await label.getText(), RENAMED_LABEL);
  });

  it("keeps the renamed label when the reporter re-registers with the AUTO label", async () => {
    // The exact regression: a FRESH phone-home (new socket) carrying the AUTO
    // label must not clobber the user rename. The host pins it via the `renamed`
    // flag in the bridge registry (see bridge.rs::register).
    const reWs = await phoneHome(token, AUTO_LABEL);
    try {
      // Let the re-register + servers-changed → refreshServers re-render flush.
      // We then poll the DOM and require it to STAY the renamed value: a revert
      // bug would surface as the label flipping back to AUTO_LABEL.
      const label = await $(ROW_LABEL);
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        assert.equal(
          await label.getText(),
          RENAMED_LABEL,
          "renamed label reverted to the auto-reported one after re-register"
        );
        await sleep(500);
      }
      // Final hard assertion after the observation window.
      assert.equal(await label.getText(), RENAMED_LABEL);
    } finally {
      try {
        reWs.close();
      } catch {}
    }
  });
});

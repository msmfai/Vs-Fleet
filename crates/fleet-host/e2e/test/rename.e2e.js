// Layer D — real-UI E2E for the Fleet host rename flow.
//
// Drives the REAL built fleet-host binary (launched by tauri-driver in its
// FLEET_E2E_RAIL_ONLY composition, so the rail is the WebDriver-reachable
// top-level webview) through the user-visible rename flow and asserts observable
// rail DOM. Then it regression-locks the exact bug the whole effort exists for:
// a fresh reporter re-register with the AUTO label must NOT revert the rename.
//
// All reads/actions are DOM-driven via browser.execute (textContent + dispatched
// events) — never getText/isDisplayed/coordinate-click — because WebKitGTK under
// Xvfb is software-rendered and those rendering-dependent ops are unreliable. The
// DOM itself is fully correct. Shared machinery lives in ./helpers.js.

import assert from "node:assert/strict";

import {
  loadE2EConfig,
  readBridgeToken,
  phoneHome,
  rowSel,
  rowLabelSel,
  readText,
  waitText,
  waitExists,
  openRowMenu,
  answerPrompt,
  promptOpen,
  clickById,
  sleep,
} from "./helpers.js";

const SERVER_ID = "e2e-session-1";
const AUTO_LABEL = "auto reported"; // what the reporter phones home with
const RENAMED_LABEL = "My Renamed Project"; // what the user types

describe("Fleet rail — rename flow (real UI)", () => {
  let E2E;
  let token;
  let helloWs;

  before(async () => {
    E2E = loadE2EConfig();
    // The rail must have booted: its status pill exists once main.js init runs.
    await $("#status").waitForExist({ timeout: 60000 });
    token = await readBridgeToken(E2E);
    helloWs = await phoneHome(E2E, token, { serverId: SERVER_ID, label: AUTO_LABEL });
  });

  after(() => {
    try {
      helloWs?.close();
    } catch {}
  });

  it("shows the phoned-home session as a rail row with its reported label", async () => {
    await waitExists(rowSel(SERVER_ID), "row never appeared", { timeout: 45000 });
    await waitText(rowLabelSel(SERVER_ID), AUTO_LABEL, `row label never became "${AUTO_LABEL}"`);
    assert.equal(await readText(rowLabelSel(SERVER_ID)), AUTO_LABEL);
  });

  it("renames the row via the context menu + prompt overlay", async () => {
    await waitExists(rowSel(SERVER_ID), "row missing before rename");

    // Open the row context menu (dispatched contextmenu → openRowMenu →
    // renderRowMenu), then click the Rename item. renderRowMenu ids each button
    // `row-menu-${item.id}`; the rename item's id is "rename".
    assert.equal(await openRowMenu(SERVER_ID), true, "row missing for contextmenu dispatch");
    await waitExists("#row-menu-rename", "row menu / rename item never rendered", { timeout: 10000 });
    assert.equal(await clickById("row-menu-rename"), true, "rename menu item missing");

    // Answer the in-DOM prompt overlay (#prompt-input): set value + dispatch Enter
    // → closePrompt(value) → renameRow → invoke("rename_server").
    await browser.waitUntil(async () => promptOpen(), {
      timeout: 10000,
      interval: 200,
      timeoutMsg: "rename prompt overlay never opened",
    });
    assert.equal(await answerPrompt(RENAMED_LABEL), true, "prompt input missing");

    // ASSERT the observable DOM: the row's .label textContent becomes the new label.
    await waitText(
      rowLabelSel(SERVER_ID),
      RENAMED_LABEL,
      `row label never became "${RENAMED_LABEL}" after rename`
    );
    assert.equal(await readText(rowLabelSel(SERVER_ID)), RENAMED_LABEL);
  });

  it("keeps the renamed label when the reporter re-registers with the AUTO label", async () => {
    // The exact regression: a FRESH phone-home (new socket) with the AUTO label
    // must not clobber the user rename. The host pins it via the `renamed` flag in
    // the bridge registry (bridge.rs::register).
    const reWs = await phoneHome(E2E, token, { serverId: SERVER_ID, label: AUTO_LABEL });
    try {
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        assert.equal(
          await readText(rowLabelSel(SERVER_ID)),
          RENAMED_LABEL,
          "renamed label reverted to the auto-reported one after re-register"
        );
        await sleep(500);
      }
      assert.equal(await readText(rowLabelSel(SERVER_ID)), RENAMED_LABEL);
    } finally {
      try {
        reWs.close();
      } catch {}
    }
  });
});

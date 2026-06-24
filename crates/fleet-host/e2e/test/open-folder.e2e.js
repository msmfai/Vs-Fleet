// Layer D E2E — the create-menu "Open Folder..." flow (spawn_server_with_options).
//
// Opens the create menu (the `+` spawn button → toggleCreateMenu → renderCreateMenu),
// clicks "Open Folder...", answers the domPrompt overlay with a path, and asserts
// the observable outcome of the spawn ATTEMPT.
//
// What's deterministic in CI: there is no `code` binary on the CI runner, so the
// real `code serve-web` spawn fails. main.js `spawnServer` catches that and calls
// `showHostStatus({level, source:"rail", message})`, which renders the
// `#status-detail` overlay (removes its `hidden` class) with the failure message.
// So we assert: create menu opens → Open Folder item present → prompt opens →
// after Enter the prompt closes AND a status-detail override appears. We do NOT
// require a real server row to appear (that needs `code`, absent in CI) — the
// observable, deterministic signal is the spawn-attempt status render.
//
// All DOM-driven (browser.execute), per the WebKitGTK/Xvfb software-render note.

import assert from "node:assert/strict";

import {
  loadE2EConfig,
  exists,
  promptOpen,
  answerPrompt,
  clickById,
  sleep,
} from "./helpers.js";

// Click a create-menu item by its visible label (the buttons have no id). The
// create menu lives in #create-menu; each item is a `.row-menu-item` button.
function clickCreateMenuItem(labelText) {
  return browser.execute((text) => {
    const menu = document.getElementById("create-menu");
    if (!menu) return false;
    const items = Array.from(menu.querySelectorAll(".row-menu-item"));
    const btn = items.find((b) => (b.textContent || "").trim().includes(text));
    if (!btn) return false;
    btn.click();
    return true;
  }, labelText);
}

function createMenuOpen() {
  return browser.execute(() => {
    const menu = document.getElementById("create-menu");
    return !!menu && !menu.classList.contains("hidden") && menu.children.length > 0;
  });
}

function statusDetailVisible() {
  return browser.execute(() => {
    const el = document.getElementById("status-detail");
    return !!el && !el.classList.contains("hidden");
  });
}

describe("Fleet rail — open-folder create flow (real UI)", () => {
  before(async () => {
    // No session registration needed — this flow is about the create menu + spawn.
    loadE2EConfig();
    await $("#status").waitForExist({ timeout: 60000 });
    await $("#spawn").waitForExist({ timeout: 60000 });
  });

  it("opens the create menu from the spawn button", async () => {
    // #spawn.onclick === toggleCreateMenu → openCreateMenu → renderCreateMenu.
    assert.equal(await clickById("spawn"), true, "spawn button missing");
    await browser.waitUntil(async () => createMenuOpen(), {
      timeout: 10000,
      interval: 200,
      timeoutMsg: "create menu never opened",
    });
    // The "Open Folder..." item is present in the menu.
    const hasItem = await browser.execute(() => {
      const menu = document.getElementById("create-menu");
      if (!menu) return false;
      return Array.from(menu.querySelectorAll(".row-menu-item")).some((b) =>
        (b.textContent || "").includes("Open Folder")
      );
    });
    assert.equal(hasItem, true, "Open Folder item missing from create menu");
  });

  it("Open Folder... opens the path prompt, and answering it attempts a spawn", async () => {
    assert.equal(await clickCreateMenuItem("Open Folder"), true, "Open Folder item not clickable");

    // openFolderPrompt → domPrompt("Open folder path", "~") opens #prompt-input.
    await browser.waitUntil(async () => promptOpen(), {
      timeout: 10000,
      interval: 200,
      timeoutMsg: "open-folder prompt never opened",
    });

    // Answer with a path + Enter → spawnServer({mode:"local", folder}). With no
    // `code` in CI the spawn fails; spawnServer catches and renders a status detail.
    assert.equal(await answerPrompt("/tmp/fleet-e2e-folder"), true, "prompt input missing");

    // The prompt closes (closePrompt re-adds `hidden`).
    await browser.waitUntil(async () => !(await promptOpen()), {
      timeout: 10000,
      interval: 200,
      timeoutMsg: "prompt never closed after Enter",
    });

    // The spawn ATTEMPT surfaces a status-detail override (error/warning — either
    // way the overlay un-hides). This is the deterministic observable.
    await browser.waitUntil(async () => statusDetailVisible(), {
      timeout: 30000,
      interval: 250,
      timeoutMsg: "spawn attempt never produced a status-detail override",
    });
    assert.equal(await statusDetailVisible(), true);
    // The create menu is gone (spawnServer/closeCreateMenu).
    await sleep(300);
    assert.equal(await exists("#create-menu:not(.hidden)"), false);
  });
});

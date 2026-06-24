// Layer D E2E — select / switch between sessions (the `select_server` flow).
//
// Registers TWO server rows via bridge `hello`, clicks row A (dispatched `click`
// → row.onclick → activateServer → invoke("select_server")) and asserts A gains
// the selected class, then clicks B and asserts the selection MOVES to B.
//
// Optimistic-vs-Hub note: selection is OPTIMISTIC. main.js `selectServer` sets
// `selected = id` and re-renders BEFORE `await invoke("select_server")` resolves
// (it only rolls back if the invoke returns false / throws). With both servers
// registered + a valid editor URL, the invoke succeeds, so the selected class is
// stable. We assert the observable `.srv.selected` class (render() adds it when
// `srv.id === selected`) and `aria-current="true"`.

import assert from "node:assert/strict";

import {
  loadE2EConfig,
  readBridgeToken,
  phoneHome,
  rowSel,
  waitExists,
  waitClassContains,
  classOf,
  dispatchOn,
  sleep,
} from "./helpers.js";

const A = "e2e-select-a";
const B = "e2e-select-b";

function isSelected(sessionId) {
  return browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    return {
      hasClass: el.classList.contains("selected"),
      ariaCurrent: el.getAttribute("aria-current"),
    };
  }, rowSel(sessionId));
}

describe("Fleet rail — select / switch (real UI)", () => {
  let E2E;
  let token;
  const sockets = [];

  before(async () => {
    E2E = loadE2EConfig();
    await $("#status").waitForExist({ timeout: 60000 });
    token = await readBridgeToken(E2E);
    // Both rows carry a real (reachable) editor URL so select_server succeeds.
    sockets.push(await phoneHome(E2E, token, { serverId: A, label: "Session A" }));
    sockets.push(await phoneHome(E2E, token, { serverId: B, label: "Session B" }));
  });

  after(() => {
    for (const ws of sockets) {
      try {
        ws.close();
      } catch {}
    }
  });

  it("registers both rows", async () => {
    await waitExists(rowSel(A), "row A never appeared", { timeout: 45000 });
    await waitExists(rowSel(B), "row B never appeared", { timeout: 45000 });
  });

  it("selecting row A marks A selected", async () => {
    // Dispatch a real click on the row → row.onclick → activateServer(A).
    assert.equal(await dispatchOn(rowSel(A), "click", "mouse"), true, "row A missing for click");
    await waitClassContains(rowSel(A), "selected", "row A never became selected");
    const a = await isSelected(A);
    assert.equal(a.hasClass, true);
    assert.equal(a.ariaCurrent, "true");
  });

  it("selecting row B moves the selection from A to B", async () => {
    assert.equal(await dispatchOn(rowSel(B), "click", "mouse"), true, "row B missing for click");
    // B becomes selected …
    await waitClassContains(rowSel(B), "selected", "row B never became selected");
    // … and A is no longer selected (single-selection invariant).
    await browser.waitUntil(
      async () => {
        const c = await classOf(rowSel(A));
        return typeof c === "string" && !c.split(/\s+/).includes("selected");
      },
      { timeout: 20000, interval: 250, timeoutMsg: "row A stayed selected after switching to B" }
    );
    const a = await isSelected(A);
    const b = await isSelected(B);
    assert.equal(b.hasClass, true);
    assert.equal(b.ariaCurrent, "true");
    assert.equal(a.hasClass, false);
    assert.equal(a.ariaCurrent, "false");
    // Belt: let any async refresh settle and re-confirm the invariant holds.
    await sleep(500);
    assert.equal((await isSelected(B)).hasClass, true);
    assert.equal((await isSelected(A)).hasClass, false);
  });
});

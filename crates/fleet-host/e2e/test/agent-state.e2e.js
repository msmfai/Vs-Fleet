// Layer D E2E — agent-state row flows: mute / solo visuals, dismiss-removes-row,
// and the waiting/attention indicator.
//
// These flows need AGENT state (an inbox tab keyed by session_id) + a connected
// Hub link — a bridge `hello` alone makes a SERVER row with no agent, and the row
// menu's Mute/Solo/Dismiss items + their handlers gate on `agentFor(id)` and
// `inbox.connected`. So we push a real reporter `session.upsert` into the embedded
// Fleet Hub (ws://127.0.0.1:51777). That produces an agent-only row (`.srv` with
// `data-server-id === session_id`), exactly like a reporter phoning the Hub.
//
// Optimistic-vs-Hub notes (read from ui/main.js):
//   • MUTE/SOLO: `toggleMuteRow`/`toggleSoloRow` call `applyLocalMute/Solo`
//     OPTIMISTICALLY and re-render BEFORE the invoke — the row's `muted-state` /
//     `soloed-state` class flips immediately, no Hub echo required. We assert that.
//   • DISMISS: `dismissSession` invokes `dismiss_session`, and on success calls
//     `removeInboxSession(id)` (optimistic local removal) — for an agent-only row
//     that drops the row from `displayed()`. The invoke needs `inbox.connected`
//     (true once the Hub link is up). We assert the row disappears.
//   • WAITING/ATTENTION: a `waiting` session is the one attention-demanding state.
//     The Rust render marks it pinging → the row gets the `attention` class and a
//     `.right .badge`, and the status pill shows "N waiting". These are
//     deterministic and observable. (The precise UNREAD-DOT reconciliation across
//     focus/transition is reducer-internal and is covered by Layer C — vitest
//     `deriveInboxTabs`/`reconcileUnread` — and host-core reducer unit tests, so we
//     assert the attention indicator here rather than fake the unread transition.)

import assert from "node:assert/strict";

import {
  loadE2EConfig,
  pushHubSession,
  rowSel,
  readText,
  classOf,
  waitExists,
  waitGone,
  waitClassContains,
  openRowMenu,
  clickById,
  exists,
  sleep,
} from "./helpers.js";

const MUTE_ID = "e2e-agent-mute";
const SOLO_ID = "e2e-agent-solo";
const DISMISS_ID = "e2e-agent-dismiss";
const WAIT_ID = "e2e-agent-waiting";

// Open the row menu and wait for a specific item button to render, then click it.
async function chooseRowMenuItem(sessionId, itemId) {
  assert.equal(await openRowMenu(sessionId), true, `row ${sessionId} missing for contextmenu`);
  await waitExists(`#row-menu-${itemId}`, `row menu item #row-menu-${itemId} never rendered`, {
    timeout: 10000,
  });
  assert.equal(await clickById(`row-menu-${itemId}`), true, `menu item ${itemId} missing`);
}

describe("Fleet rail — agent-state flows (real UI)", () => {
  let E2E;
  const sockets = [];

  before(async () => {
    E2E = loadE2EConfig();
    await $("#status").waitForExist({ timeout: 60000 });
    // Push four independent agent sessions to the embedded Hub. Each becomes an
    // agent-only rail row keyed by its session_id.
    sockets.push(await pushHubSession({ sessionId: MUTE_ID, title: "Mute Me", state: "idle" }));
    sockets.push(await pushHubSession({ sessionId: SOLO_ID, title: "Solo Me", state: "idle" }));
    // Dismiss uses an `error`-state session: the row menu only offers the "Dismiss"
    // item (`#row-menu-dismiss`, via canDismissAgent) when state is dead/error. (An
    // idle agent-only row instead offers "Forget Session" / `#row-menu-forget-session`,
    // which also dismisses — but we exercise the explicit Dismiss item here.)
    sockets.push(await pushHubSession({ sessionId: DISMISS_ID, title: "Dismiss Me", state: "error" }));
    sockets.push(
      await pushHubSession({
        sessionId: WAIT_ID,
        title: "Waiting One",
        state: "waiting",
        lastMessage: "Approve this?",
        waitingSince: "2026-06-08T00:00:00Z",
      })
    );
  });

  after(() => {
    for (const ws of sockets) {
      try {
        ws.close();
      } catch {}
    }
  });

  it("renders the injected agent sessions as rail rows", async () => {
    await waitExists(rowSel(MUTE_ID), "mute row never appeared", { timeout: 45000 });
    await waitExists(rowSel(SOLO_ID), "solo row never appeared", { timeout: 45000 });
    await waitExists(rowSel(DISMISS_ID), "dismiss row never appeared", { timeout: 45000 });
    await waitExists(rowSel(WAIT_ID), "waiting row never appeared", { timeout: 45000 });
  });

  it("mute applies the muted-state class optimistically", async () => {
    await chooseRowMenuItem(MUTE_ID, "mute");
    // applyLocalMute → render() adds `muted-state` immediately (no Hub echo).
    await waitClassContains(rowSel(MUTE_ID), "muted-state", "row never gained muted-state");
  });

  it("solo (alert focus) applies the soloed-state class optimistically", async () => {
    await chooseRowMenuItem(SOLO_ID, "solo");
    // applyLocalSolo → render() adds `soloed-state` immediately.
    await waitClassContains(rowSel(SOLO_ID), "soloed-state", "row never gained soloed-state");
  });

  it("dismiss removes the row", async () => {
    await waitExists(rowSel(DISMISS_ID), "dismiss row missing before dismiss");
    await chooseRowMenuItem(DISMISS_ID, "dismiss");
    // dismissSession → invoke succeeds (Hub connected) → removeInboxSession drops
    // the agent-only row from displayed().
    await waitGone(rowSel(DISMISS_ID), "dismissed row never disappeared", { timeout: 20000 });
    assert.equal(await exists(rowSel(DISMISS_ID)), false);
  });

  it("a waiting session shows the waiting state on its row", async () => {
    // Assert the DETERMINISTIC observable a `waiting` session always produces: the
    // `waiting` STATE class on the row and its `⏸` state glyph. render() derives
    // the row state class straight from the tab's state (`serverState(agent)` →
    // `agent.state`), so it is unaffected by mute/solo.
    //
    // We deliberately do NOT assert the `attention`/pinging class, the `.right`
    // attention badge, or a `#status` "N waiting" pill here: those come from the
    // PING/NOTIFY decision (`should_notify` = is_attention && !ping_suppressed, in
    // fleet-host-core::mute), which is reducer-internal — and, notably, suppressed
    // whenever ANY session is soloed (Rule 2). This very spec soloes another
    // session in an earlier test, so a solo is active and the waiting row's ping is
    // correctly suppressed (no attention class, status shows "muted" not "waiting").
    // That ping/notify/suppression logic is covered by Layer C (vitest
    // `shouldNotifyTab`/`deriveInboxTabs`) + host-core `should_notify`/`ping_suppressed`
    // unit tests, so we assert only the always-true waiting state here.
    await waitClassContains(rowSel(WAIT_ID), "waiting", "waiting row never gained the waiting state class");
    await browser.waitUntil(
      async () => (await readText(`${rowSel(WAIT_ID)} .glyph`)) === "⏸",
      { timeout: 20000, interval: 250, timeoutMsg: "waiting row never showed the ⏸ state glyph" }
    );
    assert.equal(await readText(`${rowSel(WAIT_ID)} .glyph`), "⏸");
    // Settle + re-confirm the waiting state class is stable.
    await sleep(500);
    const cls = await classOf(rowSel(WAIT_ID));
    assert.ok(cls.split(/\s+/).includes("waiting"), "waiting state class vanished");
  });
});

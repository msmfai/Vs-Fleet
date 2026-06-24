// Event-handler / render / IPC-dispatch tests. These exercise the rail's
// observable behavior: backend events update the DOM, toolbar buttons reflect
// state, the palette and row menus open/close, and the action handlers invoke
// the right Tauri commands with the right (camelCase) argument shapes — the
// exact contract the IPC-contract lint guards statically.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { bootRail, fire, answerPrompt } from "./harness.js";

// Build an invoke mock with a server list + inbox, used to drive a populated
// rail. Returns the rail plus a `setBackend` to mutate the server list.
async function populatedRail({ servers = [], inbox } = {}) {
  const rail = await bootRail();
  const state = {
    servers,
    selected: null,
    inbox: inbox || { tabs: [], waiting_count: 0, waiting_total: 0, connected: true },
  };
  rail.invoke.mockImplementation((name, args) => {
    switch (name) {
      case "get_servers":
        return Promise.resolve(state.servers.map((s) => ({ ...s })));
      case "selected_server":
        return Promise.resolve(state.selected);
      case "select_server":
        state.selected = args.id;
        return Promise.resolve(true);
      case "get_inbox":
        return Promise.resolve(state.inbox);
      case "get_host_status":
        return Promise.resolve(null);
      default:
        return Promise.resolve(undefined);
    }
  });
  return { ...rail, state };
}

async function refresh(rail) {
  await fire(rail.listeners, "servers-changed");
  await Promise.resolve();
  await Promise.resolve();
}

// A safe default so that any stray refreshServers/init (whose get_servers must
// be an array) never throws, while letting a test focus on one command. Returns
// `undefined` for unlisted commands like the real handlers' void results.
function withDefaults(map) {
  return (name, args) => {
    if (name in map) return map[name](args);
    if (name === "get_servers") return Promise.resolve([]);
    if (name === "selected_server") return Promise.resolve(null);
    if (name === "get_inbox") return Promise.resolve({ tabs: [], connected: true });
    if (name === "get_host_status") return Promise.resolve(null);
    return Promise.resolve(undefined);
  };
}

describe("render: rail rows from the server list", () => {
  it("renders one row per server with title and state", async () => {
    const rail = await populatedRail({
      servers: [
        { id: "a", label: "Alpha", url: "http://a", owned: true },
        { id: "b", label: "Beta", url: "http://b", owned: true },
      ],
    });
    await refresh(rail);
    const rows = rail.document.querySelectorAll(".srv");
    expect(rows.length).toBe(2);
    expect(rows[0].querySelector(".label").textContent).toBe("Alpha");
    expect(rows[0].dataset.serverId).toBe("a");
  });

  it("renders the empty state when there are no servers", async () => {
    const rail = await populatedRail({ servers: [] });
    await refresh(rail);
    expect(rail.document.querySelector(".empty-state")).toBeTruthy();
    expect(rail.document.querySelector(".srv")).toBeNull();
  });

  it("clicking a row selects the server (invokes select_server)", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "Alpha", url: "http://a", owned: true }] });
    await refresh(rail);
    rail.invoke.mockClear();
    const row = rail.document.querySelector(".srv");
    row.onclick();
    await Promise.resolve();
    await Promise.resolve();
    expect(rail.invoke).toHaveBeenCalledWith("select_server", { id: "a" });
  });
});

describe("inbox event updates the rail", () => {
  it("an inbox event re-renders waiting status and unread dots", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "Alpha", url: "http://a", owned: true }] });
    await refresh(rail);
    // The backend pushes already-derived inbox state (with waiting_count) — the
    // event handler stores it verbatim and re-renders.
    await fire(rail.listeners, "inbox", {
      tabs: [{ session_id: "a", attention: true, muted: false, pinging: true, waiting_since: new Date().toISOString() }],
      waiting_count: 1,
      waiting_total: 1,
      connected: true,
    });
    // The status pill should reflect a waiting session.
    const status = rail.document.getElementById("status");
    expect(status.textContent).toMatch(/waiting/);
  });
});

describe("host-status event", () => {
  it("shows and auto-clears a host status override", async () => {
    vi.useFakeTimers();
    try {
      const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
      await refresh(rail);
      await fire(rail.listeners, "host-status", { level: "error", source: "test", message: "boom" });
      const detail = rail.document.getElementById("status-detail");
      expect(detail.textContent).toContain("boom");
      // The dismiss button clears it.
      const close = detail.querySelector(".status-detail-close");
      expect(close).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("toolbar buttons", () => {
  it("jump button enables only when there is an openable unread session", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    const jump = rail.document.getElementById("jump");
    // No unread yet.
    expect(jump.disabled).toBe(true);
    await fire(rail.listeners, "inbox", {
      tabs: [{ session_id: "a", attention: true, muted: false, unread: true, waiting_since: new Date().toISOString() }],
      connected: true,
    });
    expect(jump.disabled).toBe(false);
    expect(jump.classList.contains("attention")).toBe(true);
  });

  it("palette button disabled when there are no sessions", async () => {
    const rail = await populatedRail({ servers: [] });
    await refresh(rail);
    expect(rail.document.getElementById("palette-open").disabled).toBe(true);
  });
});

describe("palette open/close + selection", () => {
  let rail;
  beforeEach(async () => {
    rail = await populatedRail({
      servers: [
        { id: "alpha", label: "Alpha", url: "http://a", owned: true },
        { id: "beta", label: "Beta", url: "http://b", owned: true },
      ],
    });
    await refresh(rail);
  });

  it("opens via the global hook and filters by query", () => {
    rail.window.__fleetOpenPalette();
    const palette = rail.document.getElementById("palette");
    expect(palette.classList.contains("hidden")).toBe(false);

    const input = rail.document.getElementById("palette-input");
    input.value = "alph";
    input.dispatchEvent(new rail.window.Event("input"));
    const items = rail.document.querySelectorAll(".palette-item .palette-title");
    expect(items.length).toBe(1);
    expect(items[0].textContent).toBe("Alpha");
  });

  it("choosing a palette item closes the palette and selects", async () => {
    rail.window.__fleetOpenPalette();
    rail.invoke.mockClear();
    const item = rail.document.querySelector(".palette-item");
    item.onclick();
    await Promise.resolve();
    await Promise.resolve();
    expect(rail.document.getElementById("palette").classList.contains("hidden")).toBe(true);
    expect(rail.invoke).toHaveBeenCalledWith("select_server", expect.objectContaining({ id: expect.any(String) }));
  });
});

describe("row context menu", () => {
  let rail;
  beforeEach(async () => {
    rail = await populatedRail({ servers: [{ id: "a", label: "Alpha", url: "http://a", owned: true }] });
    await refresh(rail);
  });

  it("right-click opens the menu with Open/Rename/Close items", () => {
    const row = rail.document.querySelector(".srv");
    const ev = new rail.window.MouseEvent("contextmenu", { clientX: 10, clientY: 10 });
    row.oncontextmenu(ev);
    const menu = rail.document.getElementById("row-menu");
    expect(menu.classList.contains("hidden")).toBe(false);
    const labels = [...menu.querySelectorAll(".row-menu-label")].map((n) => n.textContent);
    expect(labels).toContain("Open");
    expect(labels).toContain("Rename");
    expect(labels).toContain("Close");
  });

  it("the Rename menu item opens the real domPrompt overlay", async () => {
    rail.window.openRowMenu("a", 10, 10);
    const menu = rail.document.getElementById("row-menu");
    const renameBtn = [...menu.querySelectorAll("button")].find(
      (b) => b.querySelector(".row-menu-label").textContent === "Rename"
    );
    renameBtn.onclick();
    await Promise.resolve();
    // The in-DOM prompt overlay is now visible (no native window.prompt).
    const promptEl = rail.document.getElementById("prompt");
    expect(promptEl.classList.contains("hidden")).toBe(false);
    // Cancel it so nothing dangles.
    await answerPrompt(rail.window, null);
  });
});

describe("create menu + open-folder prompt (domPrompt path)", () => {
  it("Open Folder... uses the real domPrompt and spawns with the folder", async () => {
    const rail = await populatedRail({ servers: [] });
    await refresh(rail);
    rail.invoke.mockClear();
    rail.invoke.mockImplementation(
      withDefaults({
        spawn_server_with_options: () => Promise.resolve("new-id"),
        select_server: () => Promise.resolve(true),
      })
    );

    const flow = rail.window.openFolderPrompt();
    await answerPrompt(rail.window, "/work/dir");
    await flow;
    expect(rail.invoke).toHaveBeenCalledWith(
      "spawn_server_with_options",
      { request: { mode: "local", folder: "/work/dir" } }
    );
  });

  it("cancelling Open Folder... does not spawn", async () => {
    const rail = await populatedRail({ servers: [] });
    await refresh(rail);
    rail.invoke.mockClear();
    const flow = rail.window.openFolderPrompt();
    await answerPrompt(rail.window, null); // Escape
    await flow;
    expect(rail.invoke.mock.calls.filter((c) => c[0] === "spawn_server_with_options")).toHaveLength(0);
  });
});

describe("session action handlers dispatch the right IPC", () => {
  let rail;
  beforeEach(async () => {
    rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    // Seed an agent for session "a" so the toggles have state to act on.
    await fire(rail.listeners, "inbox", {
      tabs: [{ session_id: "a", attention: true, muted: false, soloed: false, state: "dead", unread: true }],
      connected: true,
    });
    rail.invoke.mockClear();
    rail.invoke.mockImplementation(withDefaults({}));
  });

  it("toggleMuteRow invokes set_session_muted with {sessionId, muted}", async () => {
    rail.window.toggleMuteRow("a");
    await Promise.resolve();
    expect(rail.invoke).toHaveBeenCalledWith("set_session_muted", { sessionId: "a", muted: true });
  });

  it("toggleSoloRow invokes set_session_soloed with {sessionId, soloed}", async () => {
    rail.window.toggleSoloRow("a");
    await Promise.resolve();
    expect(rail.invoke).toHaveBeenCalledWith("set_session_soloed", { sessionId: "a", soloed: true });
  });

  it("dismissRow invokes dismiss_session with {sessionId}", async () => {
    rail.window.dismissRow("a");
    await Promise.resolve();
    expect(rail.invoke).toHaveBeenCalledWith("dismiss_session", { sessionId: "a" });
  });

  it("focusSession invokes focus_session with {sessionId}", async () => {
    await rail.window.focusSession("a");
    expect(rail.invoke).toHaveBeenCalledWith("focus_session", { sessionId: "a" });
  });
});

describe("spawn / close / open-in-browser", () => {
  it("spawnServer invokes spawn_server_with_options then selects the new id", async () => {
    const rail = await populatedRail({ servers: [] });
    await refresh(rail);
    rail.invoke.mockClear();
    rail.invoke.mockImplementation((name) => {
      if (name === "spawn_server_with_options") return Promise.resolve("fresh");
      if (name === "select_server") return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    await rail.window.spawnServer({ mode: "local" });
    expect(rail.invoke).toHaveBeenCalledWith("spawn_server_with_options", { request: { mode: "local" } });
    expect(rail.invoke).toHaveBeenCalledWith("select_server", { id: "fresh" });
  });

  it("closeServer invokes close_server with {id}", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    rail.invoke.mockClear();
    rail.invoke.mockImplementation((name) => {
      if (name === "close_server") return Promise.resolve(true);
      if (name === "get_servers") return Promise.resolve([]);
      if (name === "selected_server") return Promise.resolve(null);
      return Promise.resolve(undefined);
    });
    await rail.window.closeServer("a");
    expect(rail.invoke).toHaveBeenCalledWith("close_server", { id: "a" });
  });

  it("openRowInBrowser invokes open_server_external with {id}", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    rail.invoke.mockClear();
    rail.invoke.mockImplementation(withDefaults({}));
    await rail.window.openRowInBrowser("a");
    expect(rail.invoke).toHaveBeenCalledWith("open_server_external", { id: "a" });
  });
});

describe("clearStatusOverride invokes clear_host_status_if_current", () => {
  it("clears and notifies the backend with {message}", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    rail.window.showHostStatus({ level: "error", source: "t", message: "kaboom" });
    rail.invoke.mockClear();
    rail.window.clearStatusOverride();
    await Promise.resolve();
    expect(rail.invoke).toHaveBeenCalledWith("clear_host_status_if_current", { message: "kaboom" });
  });
});

describe("__fleetSyncSelection global hook", () => {
  it("reads selected_server and updates the highlight", async () => {
    const rail = await populatedRail({ servers: [{ id: "a", label: "A", url: "http://a", owned: true }] });
    await refresh(rail);
    rail.state.selected = "a";
    await rail.window.__fleetSyncSelection();
    await Promise.resolve();
    const row = rail.document.querySelector(".srv");
    expect(row.classList.contains("selected")).toBe(true);
  });
});

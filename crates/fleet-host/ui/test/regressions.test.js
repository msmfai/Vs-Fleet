// Regression tests for the two bugs 100% Rust coverage missed:
//   1. window.prompt no-ops in macOS WKWebView, so rename/open-folder silently
//      died. Fixed by the in-DOM domPrompt() overlay.
//   2. After fixing (1), the rename didn't stick because rowTitle let the
//      agent/session title clobber the user's label. Fixed with a `renamed`
//      flag: rowTitle returns displayLabel(srv) when srv.renamed, and
//      applyLocalServerLabel sets renamed:true.
//
// These tests drive the REAL main.js functions via the jsdom harness and assert
// observable behavior. They must fail if either fix is reverted.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { bootRail, answerPrompt } from "./harness.js";

// Seed the rail's `servers` list (a private `let`, only mutated via
// refreshServers / applyLocalServerLabel). We push them in by mocking
// get_servers and firing the servers-changed event the rail listens for.
async function seedServers(rail, servers) {
  rail.invoke.mockImplementation((name) => {
    if (name === "get_servers") return Promise.resolve(servers);
    if (name === "selected_server") return Promise.resolve(null);
    if (name === "get_inbox") {
      return Promise.resolve({ tabs: [], waiting_count: 0, waiting_total: 0, connected: true });
    }
    if (name === "get_host_status") return Promise.resolve(null);
    if (name === "select_server") return Promise.resolve(true);
    if (name === "rename_server") return Promise.resolve("");
    return Promise.resolve(undefined);
  });
  const handler = rail.listeners.get("servers-changed");
  await handler();
  // Let any chained selectServer settle.
  await Promise.resolve();
  await Promise.resolve();
}

describe("domPrompt (WKWebView prompt fix)", () => {
  let rail;
  beforeEach(async () => {
    rail = await bootRail();
  });

  it("resolves the typed value when Enter is pressed", async () => {
    const { window } = rail;
    const promptEl = window.document.getElementById("prompt");
    const input = window.document.getElementById("prompt-input");

    const pending = window.domPrompt("Rename", "old");
    // Overlay is visible and pre-filled.
    expect(promptEl.classList.contains("hidden")).toBe(false);
    expect(input.value).toBe("old");

    input.value = "New Name";
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Enter" }));

    await expect(pending).resolves.toBe("New Name");
    expect(promptEl.classList.contains("hidden")).toBe(true);
  });

  it("resolves null when Escape is pressed", async () => {
    const { window } = rail;
    const input = window.document.getElementById("prompt-input");
    const pending = window.domPrompt("Rename", "old");
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Escape" }));
    await expect(pending).resolves.toBeNull();
  });

  it("resolves null when Cancel is clicked", async () => {
    const { window } = rail;
    const cancel = window.document.getElementById("prompt-cancel");
    const pending = window.domPrompt("Rename", "old");
    cancel.onclick(new window.MouseEvent("click"));
    await expect(pending).resolves.toBeNull();
  });

  it("resolves the value when OK is clicked", async () => {
    const { window } = rail;
    const input = window.document.getElementById("prompt-input");
    const ok = window.document.getElementById("prompt-ok");
    const pending = window.domPrompt("Rename", "old");
    input.value = "via-ok";
    ok.onclick(new window.MouseEvent("click"));
    await expect(pending).resolves.toBe("via-ok");
  });

  it("resolves null when the backdrop is clicked", async () => {
    const { window } = rail;
    const promptEl = window.document.getElementById("prompt");
    const pending = window.domPrompt("Rename", "old");
    // A click whose target is the overlay itself (the dim backdrop) cancels.
    const ev = new window.MouseEvent("click");
    Object.defineProperty(ev, "target", { value: promptEl });
    promptEl.onclick(ev);
    await expect(pending).resolves.toBeNull();
  });

  it("does NOT use the native window.prompt (which no-ops in WKWebView)", async () => {
    const { window } = rail;
    const spy = vi.fn(() => "should-not-be-called");
    window.prompt = spy;
    const input = window.document.getElementById("prompt-input");
    const pending = window.domPrompt("Rename", "x");
    input.value = "y";
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Enter" }));
    await pending;
    expect(spy).not.toHaveBeenCalled();
  });

  it("cancels an already-open prompt when a second opens", async () => {
    const { window } = rail;
    const first = window.domPrompt("First", "a");
    const second = window.domPrompt("Second", "b");
    // Opening the second resolves the first with null.
    await expect(first).resolves.toBeNull();
    const input = window.document.getElementById("prompt-input");
    expect(input.value).toBe("b");
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Escape" }));
    await expect(second).resolves.toBeNull();
  });
});

describe("renameRow flow (rename must invoke + stick)", () => {
  let rail;
  beforeEach(async () => {
    rail = await bootRail();
    await seedServers(rail, [
      { id: "srv-1", label: "Original", url: "http://x", owned: true },
    ]);
  });

  // A faithful backend: rename_server persists the label AND the renamed flag on
  // the Server struct; the post-rename refreshServers (get_servers) returns both
  // (mirrors src/mux.rs::rename_server + the Server.renamed field).
  function wireBackend(rail) {
    const backend = { id: "srv-1", label: "Original", url: "http://x", owned: true, renamed: false };
    rail.invoke.mockImplementation((name, args) => {
      if (name === "rename_server") {
        backend.label = args.label;
        backend.renamed = true;
        return Promise.resolve(args.label);
      }
      if (name === "get_servers") return Promise.resolve([{ ...backend }]);
      if (name === "selected_server") return Promise.resolve("srv-1");
      if (name === "select_server") return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    return backend;
  }

  it("invokes rename_server with {id, label} and pins the local label", async () => {
    const { window } = rail;
    wireBackend(rail);

    // Drive the REAL domPrompt overlay: type the new name, press Enter.
    const flow = window.renameRow("srv-1");
    await answerPrompt(window, "New Name");
    await flow;

    expect(rail.invoke).toHaveBeenCalledWith("rename_server", { id: "srv-1", label: "New Name" });
    // The local server now carries the renamed label, surviving the post-rename
    // refreshServers round-trip.
    const srv = window.serverById("srv-1");
    expect(srv.label).toBe("New Name");
  });

  it("agent phone-home does NOT clobber a fresh rename (rowTitle pins it)", async () => {
    const { window } = rail;
    wireBackend(rail);

    const flow = window.renameRow("srv-1");
    await answerPrompt(window, "New Name");
    await flow;

    const srv = window.serverById("srv-1");
    // An agent with a competing title arrives. rowTitle must still show the
    // user's label because the server is marked renamed.
    expect(srv.renamed).toBe(true);
    expect(window.rowTitle(srv, { title: "agent wants this" })).toBe("New Name");
  });

  it("does NOT invoke rename_server when the prompt is cancelled (Escape)", async () => {
    const { window } = rail;
    wireBackend(rail);
    rail.invoke.mockClear();
    const flow = window.renameRow("srv-1");
    await answerPrompt(window, null); // Escape → domPrompt resolves null
    await flow;
    expect(rail.invoke.mock.calls.filter((c) => c[0] === "rename_server")).toHaveLength(0);
  });

  it("does NOT invoke when the label is unchanged", async () => {
    const { window } = rail;
    wireBackend(rail);
    rail.invoke.mockClear();
    const flow = window.renameRow("srv-1");
    await answerPrompt(window, "Original"); // same as current label
    await flow;
    expect(rail.invoke.mock.calls.filter((c) => c[0] === "rename_server")).toHaveLength(0);
  });

  it("rejects an empty/whitespace label without invoking", async () => {
    const { window } = rail;
    wireBackend(rail);
    rail.invoke.mockClear();
    const flow = window.renameRow("srv-1");
    await answerPrompt(window, "   ");
    await flow;
    expect(rail.invoke.mock.calls.filter((c) => c[0] === "rename_server")).toHaveLength(0);
  });
});

describe("rowTitle precedence (rename pins the label)", () => {
  let window;
  beforeEach(async () => {
    window = (await bootRail()).window;
  });

  it("returns the user label when renamed, even if an agent title exists", () => {
    expect(window.rowTitle({ renamed: true, label: "Mine" }, { title: "agent title" })).toBe("Mine");
  });

  it("returns the agent title when not renamed", () => {
    expect(window.rowTitle({ renamed: false, label: "x" }, { title: "agent title" })).toBe("agent title");
  });

  it("falls back to the label when there is no agent title", () => {
    expect(window.rowTitle({ renamed: false, label: "x" }, null)).toBe("x");
    expect(window.rowTitle({ renamed: false, label: "x" }, { title: "   " })).toBe("x");
  });

  it("falls back to id when the renamed server has no label", () => {
    expect(window.rowTitle({ renamed: true, id: "srv-9" }, { title: "agent" })).toBe("srv-9");
  });
});

describe("applyLocalServerLabel sets renamed:true", () => {
  it("marks the matching server renamed and updates its label", async () => {
    const rail = await bootRail();
    await seedServers(rail, [
      { id: "a", label: "A", owned: true },
      { id: "b", label: "B", owned: true },
    ]);
    const { window } = rail;
    window.applyLocalServerLabel("a", "Renamed A");
    const a = window.serverById("a");
    const b = window.serverById("b");
    expect(a.label).toBe("Renamed A");
    expect(a.renamed).toBe(true);
    // Other servers are untouched and NOT marked renamed.
    expect(b.label).toBe("B");
    expect(b.renamed).toBeUndefined();
  });
});

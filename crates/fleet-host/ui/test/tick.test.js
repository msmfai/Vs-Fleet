// Tests for the 1s age-tick path: tickAges / applyAgeText / tickSignature.
//
// In production these run on setInterval(tickAges, 1000); the harness stubs
// setInterval at boot (so re-imports don't accumulate live timers), which means
// nothing ELSE exercises them — hence they drag function-coverage down unless
// driven here. They're column-0 bindings, so the harness footer exposes them on
// `window`; we call them directly and control the clock with a Date.now spy.
//
// The point of the tick is: advance live age labels IN PLACE (no railEl rebuild)
// so text selection survives and nodes/handlers aren't churned every second. So
// these assert both the updated text AND node identity, plus the tier-crossing
// fallback to a full render() when a pending row flips working→slow→timed-out.

import { describe, it, expect, vi, afterEach } from "vitest";
import { bootRail, fire } from "./harness.js";

afterEach(() => {
  vi.restoreAllMocks();
});

// A rail with one registered server (auto-selected by refreshServers).
async function railWithServer(id = "a") {
  const rail = await bootRail();
  const state = {
    servers: [{ id, label: id.toUpperCase(), url: `http://${id}`, owned: true }],
    selected: null,
    inbox: { tabs: [], connected: true },
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
  await fire(rail.listeners, "servers-changed");
  await Promise.resolve();
  await Promise.resolve();
  return rail;
}

// A rail with a single optimistic PENDING row (spawned, never phones home).
// startedAt is Date.now() at spawn time, so pin the clock BEFORE calling.
async function railWithPending(id = "p1") {
  const rail = await bootRail();
  rail.invoke.mockImplementation((name) => {
    switch (name) {
      case "spawn_server_with_options":
        return Promise.resolve(id);
      case "select_server":
        return Promise.resolve(true);
      case "get_servers":
        return Promise.resolve([]);
      case "selected_server":
        return Promise.resolve(null);
      case "get_inbox":
        return Promise.resolve({ tabs: [], connected: true });
      case "get_host_status":
        return Promise.resolve(null);
      default:
        return Promise.resolve(undefined);
    }
  });
  await rail.window.spawnServer({ mode: "local" });
  return rail;
}

describe("tickAges: waiting-since badge (in place)", () => {
  it("advances the .right .badge age text without rebuilding the row/badge nodes", async () => {
    const rail = await railWithServer("a");
    const { window, document } = rail;

    const base = Date.parse("2026-01-01T00:00:00.000Z");
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(base);
    const waitingSince = new Date(base - 5_000).toISOString();

    // A pinging session for "a" gives its row a waiting-since age badge.
    await fire(rail.listeners, "inbox", {
      tabs: [{ session_id: "a", attention: true, muted: false, pinging: true, waiting_since: waitingSince }],
      waiting_count: 1,
      waiting_total: 1,
      connected: true,
    });

    const row = document.querySelector(".srv");
    const badge = row.querySelector(".right .badge");
    expect(badge.textContent).toBe("5s");

    // 65s elapsed → "1m"; a non-pending row's signature is "s" (never changes),
    // so tickAges pokes text only — no full render.
    nowSpy.mockReturnValue(base + 60_000);
    window.tickAges();

    expect(document.querySelector(".srv")).toBe(row); // same row node (not rebuilt)
    expect(row.querySelector(".right .badge")).toBe(badge); // same badge node
    expect(badge.textContent).toBe("1m");
  });

  it("leaves a plain (non-waiting, non-pending) row untouched", async () => {
    const rail = await railWithServer("a");
    const { window, document } = rail;
    const row = document.querySelector(".srv");
    const label = row.querySelector(".label");
    // No inbox agent → not pinging, not pending: applyAgeText falls through.
    window.tickAges();
    expect(document.querySelector(".srv")).toBe(row);
    expect(row.querySelector(".label")).toBe(label);
  });
});

describe("tickAges: pending 'starting Ns' preview (in place)", () => {
  it("updates the .sub .preview text in place while the tier is unchanged", async () => {
    const base = Date.parse("2026-02-01T00:00:00.000Z");
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(base);
    const rail = await railWithPending("p1");
    const { window, document } = rail;

    const row = document.querySelector(".srv");
    const preview = row.querySelector(".sub .preview");
    expect(preview.textContent).toBe("starting 0s");

    // +5s, still < PENDING_SLOW_MS (15s): working tier, signature unchanged.
    nowSpy.mockReturnValue(base + 5_000);
    window.tickAges();

    expect(document.querySelector(".srv")).toBe(row); // same row node
    expect(row.querySelector(".sub .preview")).toBe(preview); // same preview node
    expect(preview.textContent).toBe("starting 5s");
  });
});

describe("tickAges: pending tier crossings fall back to a full render", () => {
  it("crossing PENDING_SLOW_MS rebuilds the row and flips it to the slow tier", async () => {
    const base = Date.parse("2026-03-01T00:00:00.000Z");
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(base);
    const rail = await railWithPending("p1");
    const { window, document } = rail;

    const row = document.querySelector(".srv");
    expect(row.classList.contains("slow")).toBe(false);

    // +20s → 15s..45s: slow tier. Signature "p:waiting:slow" != stamped
    // "p:working:", so tickAges triggers a full render().
    nowSpy.mockReturnValue(base + 20_000);
    window.tickAges();

    const newRow = document.querySelector(".srv");
    expect(newRow).not.toBe(row); // full rebuild (new node)
    expect(newRow.classList.contains("slow")).toBe(true);
    expect(newRow.querySelector(".sub .preview").textContent).toBe("still starting 20s");
  });

  it("crossing PENDING_TIMEOUT_MS rebuilds the row and flips the glyph to error", async () => {
    const base = Date.parse("2026-03-02T00:00:00.000Z");
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(base);
    const rail = await railWithPending("p1");
    const { window, document } = rail;

    const row = document.querySelector(".srv");
    expect(row.querySelector(".glyph.spinner")).toBeTruthy(); // working → spinner
    expect(row.classList.contains("error")).toBe(false);

    // +50s → > PENDING_TIMEOUT_MS (45s): error tier → full render + glyph flip.
    nowSpy.mockReturnValue(base + 50_000);
    window.tickAges();

    const newRow = document.querySelector(".srv");
    expect(newRow).not.toBe(row);
    expect(newRow.querySelector(".glyph.spinner")).toBeNull(); // spinner gone
    expect(newRow.querySelector(".glyph").textContent).toBe("✕"); // error glyph
    expect(newRow.classList.contains("error")).toBe(true);
    expect(newRow.classList.contains("timed-out")).toBe(true);
  });
});

describe("tickAges: guards", () => {
  it("early-returns when there are no rows (empty state)", async () => {
    const rail = await bootRail(); // no servers → empty state, no .srv rows
    const { window, document } = rail;
    expect(document.querySelector(".srv")).toBeNull();
    expect(() => window.tickAges()).not.toThrow();
    expect(document.querySelector(".srv")).toBeNull();
  });

  it("falls back to a full render when a DOM row is not in the current model", async () => {
    const rail = await railWithServer("a");
    const { window, document } = rail;
    // Inject a phantom .srv whose id isn't in displayed(): membership drift.
    const phantom = document.createElement("div");
    phantom.className = "srv";
    phantom.dataset.serverId = "ghost";
    document.getElementById("rail").appendChild(phantom);

    window.tickAges(); // byId.get("ghost") === undefined → render()

    expect(document.querySelector('.srv[data-server-id="ghost"]')).toBeNull();
    expect(document.querySelector('.srv[data-server-id="a"]')).toBeTruthy();
  });
});

describe("applyAgeText: guards (direct)", () => {
  it("returns early for a closing row without touching its preview text", async () => {
    const { window, document } = await bootRail();
    const row = document.createElement("div");
    const sub = document.createElement("div");
    sub.className = "sub";
    const preview = document.createElement("span");
    preview.className = "preview";
    preview.textContent = "closing…";
    sub.appendChild(preview);
    row.appendChild(sub);

    window.applyAgeText(row, { isClosing: true, pendingState: { text: "starting 9s" } });
    expect(preview.textContent).toBe("closing…"); // guard held
  });

  it("is a no-op when a pending row has no preview node", async () => {
    const { window, document } = await bootRail();
    const row = document.createElement("div"); // no .sub .preview
    expect(() => window.applyAgeText(row, { pendingState: { text: "starting 3s" } })).not.toThrow();
  });

  it("is a no-op when a pinging row has no badge node", async () => {
    const { window, document } = await bootRail();
    const row = document.createElement("div"); // no .right .badge
    expect(() =>
      window.applyAgeText(row, { pinging: true, agent: { waiting_since: null } })
    ).not.toThrow();
  });
});

describe("tickSignature (direct)", () => {
  it("is 's' for a non-pending row and 'p:state:level' for a pending one", async () => {
    const { window } = await bootRail();
    expect(window.tickSignature({})).toBe("s");
    expect(window.tickSignature({ pendingState: null })).toBe("s");
    expect(window.tickSignature({ pendingState: { state: "working", level: "" } })).toBe("p:working:");
    expect(window.tickSignature({ pendingState: { state: "error", level: "timed-out" } })).toBe(
      "p:error:timed-out"
    );
  });
});

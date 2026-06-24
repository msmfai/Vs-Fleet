// Pure-helper unit tests for main.js. These exercise the rail's formatting,
// classification, inbox-reconciliation, palette-scoring, and predicate logic
// with real assertions — the deterministic core that should be fully covered.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { bootRail } from "./harness.js";

let window;
beforeEach(async () => {
  window = (await bootRail()).window;
});

describe("formatAge", () => {
  it("renders seconds, minutes, hours and clamps negatives", () => {
    expect(window.formatAge(0)).toBe("0s");
    expect(window.formatAge(5_000)).toBe("5s");
    expect(window.formatAge(90_000)).toBe("1m");
    expect(window.formatAge(3_600_000)).toBe("1h");
    expect(window.formatAge(-100)).toBe("0s");
  });
});

describe("waitingAge", () => {
  it("returns 'waiting' for missing or future timestamps", () => {
    expect(window.waitingAge(null)).toBe("waiting");
    expect(window.waitingAge("not-a-date")).toBe("waiting");
    const future = new Date(Date.now() + 60_000).toISOString();
    expect(window.waitingAge(future)).toBe("waiting");
  });
  it("formats elapsed time since the ISO timestamp", () => {
    const past = new Date(Date.now() - 5_000).toISOString();
    expect(window.waitingAge(past)).toMatch(/^\d+s$/);
  });
});

describe("token", () => {
  it("lowercases and null-coalesces to empty string", () => {
    expect(window.token(null)).toBe("");
    expect(window.token(undefined)).toBe("");
    expect(window.token("ABC")).toBe("abc");
    expect(window.token(42)).toBe("42");
  });
});

describe("displayLabel / predicates", () => {
  it("displayLabel prefers label, falls back to id", () => {
    expect(window.displayLabel({ label: "L", id: "i" })).toBe("L");
    expect(window.displayLabel({ id: "i" })).toBe("i");
  });
  it("isOwned defaults true unless owned===false", () => {
    expect(window.isOwned({})).toBe(true);
    expect(window.isOwned({ owned: false })).toBe(false);
    expect(window.isOwned(null)).toBeFalsy();
  });
  it("canClose/canRename gate on agentOnly", () => {
    expect(window.canCloseServerRow({ agentOnly: false })).toBe(true);
    expect(window.canCloseServerRow({ agentOnly: true })).toBe(false);
    expect(window.canRenameServerRow({ agentOnly: false })).toBe(true);
    expect(window.canRenameServerRow(null)).toBe(false);
  });
});

describe("pendingVisual", () => {
  it("classifies starting / slow / timed-out by age", () => {
    const now = Date.now();
    expect(window.pendingVisual({ startedAt: now }).state).toBe("working");
    expect(window.pendingVisual({ startedAt: now - 20_000 }).state).toBe("waiting");
    expect(window.pendingVisual({ startedAt: now - 50_000 }).state).toBe("error");
  });
});

describe("serverState / serverPreview", () => {
  it("derives state from agent, else connection", () => {
    expect(window.serverState({ state: "working" })).toBe("working");
    expect(window.serverState({})).toBe("idle");
    // No agent: depends on inbox.connected. After bootRail, connected defaults
    // false until init's get_inbox resolves; assert both branches via setInbox.
    window.setInbox({ tabs: [], connected: true });
    expect(window.serverState(null)).toBe("idle");
    window.setInbox({ tabs: [], connected: false });
    expect(window.serverState(null)).toBe("dead");
  });

  it("serverPreview reflects connection + last message + meta", () => {
    window.setInbox({ tabs: [], connected: true });
    expect(window.serverPreview(null, "idle")).toBe("no agent activity");
    expect(window.serverPreview({ last_message: "hi" }, "idle")).toBe("hi");
    expect(window.serverPreview({ agent: "claude", location: "docker" }, "working"))
      .toBe("claude · docker · working");
    window.setInbox({ tabs: [], connected: false });
    expect(window.serverPreview(null, "idle")).toBe("hub disconnected");
    expect(window.serverPreview({ last_message: "bye" }, "idle")).toBe("last: bye");
  });
});

describe("agentMeta", () => {
  it("joins agent / location / run-count", () => {
    expect(window.agentMeta(null)).toBe("");
    expect(window.agentMeta({ agent: "claude", location: "laptop", run_count: 3 }))
      .toBe("claude · laptop · 3 runs");
    expect(window.agentMeta({ agent: "claude", run_count: 1 })).toBe("claude");
  });
});

describe("dismiss / forget / retry predicates", () => {
  it("canDismissAgent only on dead/error", () => {
    expect(window.canDismissAgent({}, "dead")).toBe(true);
    expect(window.canDismissAgent({}, "error")).toBe(true);
    expect(window.canDismissAgent({}, "idle")).toBe(false);
    expect(window.canDismissAgent(null, "dead")).toBe(false);
  });
  it("canForgetAgentOnly requires agentOnly + agent", () => {
    expect(window.canForgetAgentOnly({ agentOnly: true }, {})).toBe(true);
    expect(window.canForgetAgentOnly({ agentOnly: false }, {})).toBe(false);
    expect(window.canForgetAgentOnly({ agentOnly: true }, null)).toBe(false);
  });
  it("canRetryServer requires pending+owned+error state", () => {
    const srv = { pending: true, owned: true };
    expect(window.canRetryServer(srv, { state: "error" })).toBe(true);
    expect(window.canRetryServer(srv, { state: "waiting" })).toBe(false);
    expect(window.canRetryServer({ pending: false, owned: true }, { state: "error" })).toBe(false);
  });
});

describe("attention / confidence / state flags", () => {
  it("attentionUrgency maps known and unknown urgencies", () => {
    expect(window.attentionUrgency({ urgency: "approval" })).toEqual({ token: "approval", label: "approval" });
    expect(window.attentionUrgency({ urgency: "idle-done" })).toEqual({ token: "idle-done", label: "done" });
    expect(window.attentionUrgency({ urgency: "weird" })).toEqual({ token: "weird", label: "weird" });
    expect(window.attentionUrgency({})).toBeNull();
    expect(window.attentionUrgency({ urgency: "null" })).toBeNull();
  });
  it("confidenceClass/Title map inferred/high", () => {
    expect(window.confidenceClass({ confidence: "inferred" })).toBe("confidence-inferred");
    expect(window.confidenceClass({ confidence: "high" })).toBe("confidence-high");
    expect(window.confidenceClass({})).toBe("");
    expect(window.confidenceTitle({ confidence: "inferred" })).toBe("inferred");
    expect(window.confidenceTitle({ confidence: "high" })).toBe("high confidence");
    expect(window.confidenceTitle({})).toBe("");
  });
  it("stateFlags prioritizes solo > muted > silenced", () => {
    expect(window.stateFlags(null)).toEqual([]);
    expect(window.stateFlags({ soloed: true, muted: true })).toEqual(["solo"]);
    expect(window.stateFlags({ muted: true })).toEqual(["muted"]);
    expect(window.stateFlags({ ping_suppressed: true })).toEqual(["silenced"]);
    expect(window.stateFlags({})).toEqual([]);
  });
});

describe("inbox reconciliation", () => {
  it("deriveInboxTabs counts pinging/attention and reconciles unread on a rising edge", () => {
    const tabs = [
      { session_id: "a", attention: true, muted: false },
      { session_id: "b", attention: false },
    ];
    // With no prior notify map, the tab's own (false) unread is preserved — the
    // unread "rising edge" only fires on a transition from not-notifying.
    const initial = window.deriveInboxTabs(tabs);
    const a0 = initial.tabs.find((t) => t.session_id === "a");
    expect(a0.pinging).toBe(true);
    expect(a0.unread).toBe(false);
    expect(initial.waiting_count).toBe(1);
    expect(initial.waiting_total).toBe(1);

    // Now reconcile from a prior state where "a" was NOT notifying → unread rises.
    const oldNotify = new Map([["a", false], ["b", false]]);
    const risen = window.deriveInboxTabs(tabs, oldNotify);
    expect(risen.tabs.find((t) => t.session_id === "a").unread).toBe(true);
  });

  it("solo suppresses non-soloed attention (ping_suppressed)", () => {
    const tabs = [
      { session_id: "a", attention: true, soloed: true },
      { session_id: "b", attention: true, soloed: false },
    ];
    const derived = window.deriveInboxTabs(tabs);
    const b = derived.tabs.find((t) => t.session_id === "b");
    expect(b.pinging).toBe(false);
    expect(b.ping_suppressed).toBe(true);
  });

  it("applyLocalMute / applyLocalSolo / applyLocalFocus mutate the right tab", () => {
    window.setInbox(window.deriveInboxTabs([
      { session_id: "a", attention: true },
      { session_id: "b", attention: true },
    ]));
    window.applyLocalMute("a", true);
    expect(window.agentFor("a").muted).toBe(true);

    window.applyLocalSolo("b", true);
    expect(window.agentFor("b").soloed).toBe(true);
    expect(window.agentFor("a").soloed).toBe(false);

    window.applyLocalFocus("a");
    expect(window.agentFor("a").unread).toBe(false);
  });

  it("removeInboxSession drops the tab", () => {
    window.setInbox(window.deriveInboxTabs([{ session_id: "a" }, { session_id: "b" }]));
    window.removeInboxSession("a");
    expect(window.agentFor("a")).toBeUndefined();
    expect(window.agentFor("b")).toBeDefined();
  });
});

describe("palette scoring", () => {
  it("fuzzyTokenScore matches subsequences and rejects misses", () => {
    expect(window.fuzzyTokenScore("", "anything")).toBe(0);
    expect(window.fuzzyTokenScore("abc", "")).toBeNull();
    expect(window.fuzzyTokenScore("abc", "abc")).toBeGreaterThan(0);
    expect(window.fuzzyTokenScore("xyz", "abc")).toBeNull();
    // Consecutive + leading matches score higher than scattered ones.
    expect(window.fuzzyTokenScore("ab", "abc")).toBeGreaterThan(
      window.fuzzyTokenScore("ac", "abXc")
    );
  });

  it("countBadge / countPhrase format counts", () => {
    expect(window.countBadge(0)).toBeNull();
    expect(window.countBadge(3)).toBe("3");
    expect(window.countBadge(15)).toBe("9+");
    expect(window.countPhrase(1, "session")).toBe("1 session");
    expect(window.countPhrase(2, "session")).toBe("2 sessions");
  });
});

describe("status classification", () => {
  it("statusClearDelay maps level to ms with default", () => {
    expect(window.statusClearDelay("error")).toBe(30_000);
    expect(window.statusClearDelay("warning")).toBe(20_000);
    expect(window.statusClearDelay("info")).toBe(10_000);
    expect(window.statusClearDelay("other")).toBe(8_000);
  });

  it("isRecoverableStatus recognizes the transient open/connect messages", () => {
    expect(window.isRecoverableStatus(null)).toBe(false);
    expect(window.isRecoverableStatus({ message: "server is not ready to open" })).toBe(true);
    expect(window.isRecoverableStatus({ message: "open failed: boom" })).toBe(true);
    expect(window.isRecoverableStatus({ message: "session is visible; editor is still connecting" })).toBe(true);
    expect(window.isRecoverableStatus({ message: "fatal" })).toBe(false);
  });
});

describe("actionKey / sessionActionBusy", () => {
  it("composes keys and reflects no busy actions initially", () => {
    expect(window.actionKey("id", "mute")).toBe("mute:id");
    expect(window.sessionActionBusy("id")).toBe(false);
  });
});

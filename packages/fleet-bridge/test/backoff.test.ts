/**
 * Deterministic exponential-backoff tests for the bridge reconnect loop.
 *
 * This file mocks `ws` entirely so `new WebSocket(...)` never opens a real socket;
 * we capture each fake instance's event handlers and drive `open`/`close`
 * ourselves under fake timers. That lets us assert the exact reconnect schedule:
 * the delay doubles on each consecutive failure, caps at 30s, and resets to the
 * fast start after a successful `open`. (The unix/TCP transport itself is covered
 * against a *real* ws server in extension.test.ts.)
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The captured fake WebSocket instances, in creation order.
const instances: FakeWS[] = [];

class FakeWS {
  handlers = new Map<string, (...a: any[]) => void>();
  closed = false;
  constructor(public readonly target: string) {
    instances.push(this);
  }
  on(ev: string, cb: (...a: any[]) => void): this {
    this.handlers.set(ev, cb);
    return this;
  }
  send(): void {
    /* swallow the hello frame */
  }
  close(): void {
    this.closed = true;
  }
  fire(ev: string, ...a: any[]): void {
    this.handlers.get(ev)?.(...a);
  }
}

// `import WebSocket from "ws"` resolves to the module's default export (the class
// itself), so the mock's default must BE the constructor.
vi.mock("ws", () => ({ default: FakeWS }));

// Imported after the mock is registered (vi.mock is hoisted above imports anyway).
import { activate } from "../src/extension";

const ENV_KEYS = ["FLEET_BRIDGE_URL", "FLEET_BRIDGE_SOCKET", "FLEET_SERVER_ID"];
const saved: Record<string, string | undefined> = {};

beforeEach(() => {
  for (const k of ENV_KEYS) saved[k] = process.env[k];
  instances.length = 0;
  vi.useFakeTimers();
  delete process.env.FLEET_BRIDGE_SOCKET;
  process.env.FLEET_BRIDGE_URL = "ws://127.0.0.1:65000";
  process.env.FLEET_SERVER_ID = "srv-backoff";
});

afterEach(() => {
  vi.useRealTimers();
  for (const k of ENV_KEYS) {
    if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
  }
});

function fakeContext(): { subscriptions: Array<{ dispose(): void }> } {
  return { subscriptions: [] };
}

describe("reconnect backoff schedule", () => {
  it("doubles the reconnect delay on repeated failures, then resets after open", () => {
    const setTimeoutSpy = vi.spyOn(global, "setTimeout");
    const ctx = fakeContext();
    activate(ctx as any);

    const delays = (): number[] =>
      setTimeoutSpy.mock.calls.map((c) => c[1] as number);

    // socket #0 created on activate; close it → first reconnect armed at 1000ms.
    expect(instances).toHaveLength(1);
    instances[0].fire("close");
    expect(delays()).toEqual([1000]);

    // fire the timer → socket #1; close → next delay doubles to 2000.
    vi.advanceTimersByTime(1000);
    expect(instances).toHaveLength(2);
    instances[1].fire("close");
    expect(delays()).toEqual([1000, 2000]);

    // → 4000, then 8000.
    vi.advanceTimersByTime(2000);
    instances[2].fire("close");
    vi.advanceTimersByTime(4000);
    instances[3].fire("close");
    expect(delays()).toEqual([1000, 2000, 4000, 8000]);

    // A successful open on socket #4 resets the backoff to the fast start, so the
    // NEXT close schedules 1000ms again — snappy reconnect when Fleet returns.
    vi.advanceTimersByTime(8000);
    instances[4].fire("open");
    instances[4].fire("close");
    expect(delays()).toEqual([1000, 2000, 4000, 8000, 1000]);

    for (const s of ctx.subscriptions) s.dispose();
  });

  it("caps the reconnect delay at 30s under sustained failure", () => {
    const setTimeoutSpy = vi.spyOn(global, "setTimeout");
    const ctx = fakeContext();
    activate(ctx as any);

    // Drive many consecutive failures; the delay climbs 1s→2→4→…→30s and holds.
    let delay = 1000;
    instances[0].fire("close");
    for (let i = 0; i < 10; i++) {
      vi.advanceTimersByTime(delay);
      instances[i + 1].fire("close");
      delay = Math.min(delay * 2, 30000);
    }

    const delays = setTimeoutSpy.mock.calls.map((c) => c[1] as number);
    expect(delays).toContain(30000);
    expect(Math.max(...delays)).toBe(30000); // never exceeds the ceiling
    // Monotonic non-decreasing until the cap (each step ≥ the previous).
    for (let i = 1; i < delays.length; i++) {
      expect(delays[i]).toBeGreaterThanOrEqual(delays[i - 1]);
    }

    for (const s of ctx.subscriptions) s.dispose();
  });

  it("dispose during a pending backoff cancels the reconnect", () => {
    const ctx = fakeContext();
    activate(ctx as any);
    instances[0].fire("close"); // arm a reconnect timer
    const before = instances.length;

    for (const s of ctx.subscriptions) s.dispose();
    // Advancing well past any backoff must NOT create a new socket.
    vi.advanceTimersByTime(60000);
    expect(instances.length).toBe(before);
  });
});

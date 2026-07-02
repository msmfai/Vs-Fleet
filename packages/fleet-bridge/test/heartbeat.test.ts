/**
 * App-level heartbeat tests for the bridge.
 *
 * While connected, the extension sends a `{type:"ping"}` frame every
 * PING_INTERVAL_MS (20s) so Fleet has a continuous "I'm up" signal that its
 * liveness GC uses to reap stale / half-open sockets a clean WS close never
 * reports. Like backoff.test.ts, this file mocks `ws` entirely so `new
 * WebSocket(...)` never opens a real socket: we capture each fake instance's
 * handlers + the frames it sends and drive `open`/`close` ourselves under fake
 * timers. That lets us assert the exact ping cadence and that the interval is
 * torn down on drop/reconnect/dispose.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Must match PING_INTERVAL_MS in src/extension.ts (not exported).
const PING_INTERVAL_MS = 20000;

// FakeWS + its instance log live inside vi.hoisted so they exist when the hoisted
// vi.mock("ws") factory runs. Unlike backoff's FakeWS, this one RECORDS every
// frame it sends (so we can assert pings) and can be told to throw on the next
// send (to exercise the ping catch branch).
const { FakeWS, instances } = vi.hoisted(() => {
  const instances: any[] = [];
  class FakeWS {
    handlers = new Map<string, (...a: any[]) => void>();
    sent: string[] = [];
    throwNextSend = false;
    closed = false;
    constructor(public readonly target: string) {
      instances.push(this);
    }
    on(ev: string, cb: (...a: any[]) => void): this {
      this.handlers.set(ev, cb);
      return this;
    }
    send(data: string): void {
      if (this.throwNextSend) {
        this.throwNextSend = false;
        throw new Error("send failed (socket closing)");
      }
      this.sent.push(data);
    }
    close(): void {
      this.closed = true;
    }
    fire(ev: string, ...a: any[]): void {
      this.handlers.get(ev)?.(...a);
    }
  }
  return { FakeWS, instances };
});

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
  process.env.FLEET_SERVER_ID = "srv-heartbeat";
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

/** The `{type:"ping"}` frames a fake socket sent (hello + result frames excluded). */
function pings(inst: any): any[] {
  return inst.sent
    .map((s: string) => {
      try {
        return JSON.parse(s);
      } catch {
        return null;
      }
    })
    .filter((f: any) => f && f.type === "ping");
}

describe("heartbeat", () => {
  it("sends a {type:'ping'} frame every PING_INTERVAL_MS once open", () => {
    const ctx = fakeContext();
    activate(ctx as any);

    // socket #0 created on activate; open it → hello sent, heartbeat armed.
    expect(instances).toHaveLength(1);
    instances[0].fire("open");
    // The first frame is the hello registration, not a ping.
    expect(pings(instances[0])).toHaveLength(0);

    // Each interval tick emits exactly one ping frame, and it keeps repeating.
    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(pings(instances[0])).toEqual([{ type: "ping" }]);

    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(pings(instances[0])).toEqual([{ type: "ping" }, { type: "ping" }]);

    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(pings(instances[0])).toHaveLength(3);

    for (const s of ctx.subscriptions) s.dispose();
  });

  it("swallows a send that throws mid-ping (catch branch) and keeps beating", () => {
    const ctx = fakeContext();
    activate(ctx as any);

    instances[0].fire("open"); // hello sent (throwNextSend still false)

    // Force the NEXT send (the ping) to throw; the interval must swallow it.
    instances[0].throwNextSend = true;
    expect(() => vi.advanceTimersByTime(PING_INTERVAL_MS)).not.toThrow();
    // That ping was lost to the throw — nothing recorded.
    expect(pings(instances[0])).toHaveLength(0);

    // The heartbeat survives: the following interval sends normally.
    expect(() => vi.advanceTimersByTime(PING_INTERVAL_MS)).not.toThrow();
    expect(pings(instances[0])).toEqual([{ type: "ping" }]);

    for (const s of ctx.subscriptions) s.dispose();
  });

  it("clears the heartbeat on close/reconnect (no ping after the socket drops)", () => {
    const ctx = fakeContext();
    activate(ctx as any);

    instances[0].fire("open");
    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(pings(instances[0])).toHaveLength(1);

    // Drop the socket: reconnect() runs clearHeartbeat(), so the old interval is
    // torn down. Advancing well past several intervals sends no further ping on
    // the dead socket (the 1s reconnect creates socket #1, which never opens).
    instances[0].fire("close");
    vi.advanceTimersByTime(PING_INTERVAL_MS * 5);
    expect(pings(instances[0])).toHaveLength(1);
    expect(pings(instances[1] ?? { sent: [] })).toHaveLength(0);

    for (const s of ctx.subscriptions) s.dispose();
  });

  it("clears the heartbeat on dispose (no ping after teardown)", () => {
    const ctx = fakeContext();
    activate(ctx as any);

    instances[0].fire("open");
    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(pings(instances[0])).toHaveLength(1);

    // dispose() must clearHeartbeat() + close the socket.
    for (const s of ctx.subscriptions) s.dispose();
    expect(instances[0].closed).toBe(true);

    vi.advanceTimersByTime(PING_INTERVAL_MS * 3);
    expect(pings(instances[0])).toHaveLength(1); // no further beats
  });

  it("dispose before open exercises clearHeartbeat's null branch (no throw)", () => {
    const ctx = fakeContext();
    activate(ctx as any);

    // Never fired "open", so no heartbeat interval exists. dispose() still calls
    // clearHeartbeat(), which must no-op cleanly (the `if (heartbeat)` false path).
    expect(instances).toHaveLength(1);
    expect(() => {
      for (const s of ctx.subscriptions) s.dispose();
    }).not.toThrow();
    expect(instances[0].closed).toBe(true);
    expect(pings(instances[0])).toHaveLength(0);
  });
});

// Bridge hub — the WS server (:51778) every in-container bridge dials into.
//
// Wire protocol (§3.3): server→bridge messages carry a `reqId`; the bridge replies
// `{type:"result", reqId, ok, ...}`. On connect the bridge sends `{type:"hello",
// server_id, caps?}`; `caps` (when present) is the list of supported §3.3
// capabilities — absent ⇒ assume the shipped baseline {command,query}.
//
// Ported verbatim from harness.mjs's BridgeHub, plus: capability tracking, a generic
// request() round-trip (for openFile/typeText/… and the new queries), and
// freeing :51778 before binding (§8: always free the port before a run).

import { WebSocketServer } from "ws";
import { execSync } from "node:child_process";

export const BRIDGE_PORT = 51778;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Capabilities the original (shipped) bridge always satisfies — even if it never
// advertises a `caps` array in its hello. Track E will advertise the full set.
const BASELINE_CAPS = ["command", "query"];

export class BridgeHub {
  /** @param {{port?:number}} [opts] */
  constructor({ port = BRIDGE_PORT } = {}) {
    this.port = port;
    this.conns = new Map();   // server_id -> ws
    this.caps = new Map();    // server_id -> Set<capability>
    this.waiters = new Map(); // reqId -> { resolve }
    this.seq = 1;
    this._freePort(port);
    this.wss = new WebSocketServer({ host: "0.0.0.0", port });
    this.wss.on("connection", (ws) => {
      let id = null;
      ws.on("message", (buf) => {
        let m;
        try { m = JSON.parse(buf.toString()); } catch { return; }
        if (m.type === "hello") {
          id = m.server_id;
          this.conns.set(id, ws);
          const advertised = Array.isArray(m.caps) ? m.caps : [];
          this.caps.set(id, new Set([...BASELINE_CAPS, ...advertised]));
        } else if (m.type === "result" && this.waiters.has(m.reqId)) {
          this.waiters.get(m.reqId)(m);
          this.waiters.delete(m.reqId);
        }
      });
      ws.on("close", () => { if (id) { this.conns.delete(id); this.caps.delete(id); } });
    });
  }

  // §8: kill any stale bridge holding the port so a run never wedges.
  _freePort(port) {
    try { execSync(`lsof -ti tcp:${port} | xargs kill -9`, { stdio: "ignore" }); } catch {}
  }

  connected(id) { return this.conns.has(id); }

  // Capabilities advertised by a connected bridge. Unknown/unconnected ⇒ baseline.
  capsFor(id) { return this.caps.get(id) || new Set(BASELINE_CAPS); }
  supports(id, cap) { return this.capsFor(id).has(cap); }

  async waitFor(id, ms = 60000) {
    const t0 = Date.now();
    while (!this.connected(id)) {
      if (Date.now() - t0 > ms) throw new Error(`bridge ${id} never connected`);
      await sleep(500);
    }
  }

  // Raw round-trip: stamps a reqId, sends, resolves with the bridge's result msg.
  send(id, obj, ms = 15000) {
    const reqId = this.seq++;
    const ws = this.conns.get(id);
    if (!ws) return Promise.reject(new Error(`no bridge for ${id}`));
    return new Promise((resolve, reject) => {
      const to = setTimeout(() => {
        this.waiters.delete(reqId);
        reject(new Error("bridge req timeout"));
      }, ms);
      this.waiters.set(reqId, (m) => { clearTimeout(to); resolve(m); });
      ws.send(JSON.stringify({ ...obj, reqId }));
    });
  }

  // §3.3 action: executeCommand. Returns the raw result msg ({ok,error,...}).
  command(id, command, args = []) { return this.send(id, { type: "command", id: command, args }); }
  // §3.3 query: state snapshot. Default no-arg query → Snapshot in `.data`.
  query(id, extra = {}) { return this.send(id, { type: "query", ...extra }); }
  // Generic request for the rest of §3.3 (openFile/typeText/termSend/fileContent/…).
  request(id, msg, ms = 15000) { return this.send(id, msg, ms); }
  // Fire-and-forget: send without a reqId so the bridge runs it but does NOT reply.
  // For commands whose executeCommand promise doesn't resolve headlessly (e.g.
  // terminal.kill) — fire it, then verify the effect via observe().
  fire(id, msg) { const ws = this.conns.get(id); if (ws) ws.send(JSON.stringify(msg)); }

  close() { try { this.wss.close(); } catch {} }
}

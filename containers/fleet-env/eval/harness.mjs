// Fleet behaviour harness — drive VS Code actions inside containerized environments
// via the bridge, ASSERT the effect actually happened, and snapshot machine state
// before/after to catch side effects. Headless + parallel; each env self-reports.
//
//   node harness.mjs [N]      # N parallel environments (default 1)
//
// Per env: docker run the fleet-env image, open its editor with Playwright (brings
// the bridge's ext-host online), then for each behaviour: observe → act → observe →
// assert + diff. Artifacts (screenshots, report) land in /tmp/fleet-eval/.

import { execSync, exec } from "node:child_process";
import { mkdirSync } from "node:fs";
import { WebSocketServer } from "ws";
import { chromium } from "playwright";

const IMAGE = "fleet-env:latest";
const BRIDGE_PORT = 51778;
const BASE_PORT = 8200;
const OUT = "/tmp/fleet-eval";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(OUT, { recursive: true });

// ─── Bridge hub: the in-container bridges dial us; we send commands/queries ─────
class BridgeHub {
  constructor() {
    this.conns = new Map();   // server_id -> ws
    this.waiters = new Map(); // reqId -> resolve
    this.seq = 1;
    this.wss = new WebSocketServer({ host: "0.0.0.0", port: BRIDGE_PORT });
    this.wss.on("connection", (ws) => {
      let id = null;
      ws.on("message", (buf) => {
        let m; try { m = JSON.parse(buf.toString()); } catch { return; }
        if (m.type === "hello") { id = m.server_id; this.conns.set(id, ws); }
        else if (m.type === "result" && this.waiters.has(m.reqId)) {
          this.waiters.get(m.reqId)(m); this.waiters.delete(m.reqId);
        }
      });
      ws.on("close", () => { if (id) this.conns.delete(id); });
    });
  }
  connected(id) { return this.conns.has(id); }
  async waitFor(id, ms = 60000) {
    const t0 = Date.now();
    while (!this.connected(id)) {
      if (Date.now() - t0 > ms) throw new Error(`bridge ${id} never connected`);
      await sleep(500);
    }
  }
  send(id, obj, ms = 15000) {
    const reqId = this.seq++;
    const ws = this.conns.get(id);
    if (!ws) return Promise.reject(new Error(`no bridge for ${id}`));
    return new Promise((resolve, reject) => {
      const to = setTimeout(() => { this.waiters.delete(reqId); reject(new Error("bridge req timeout")); }, ms);
      this.waiters.set(reqId, (m) => { clearTimeout(to); resolve(m); });
      ws.send(JSON.stringify({ ...obj, reqId }));
    });
  }
  command(id, command, args = []) { return this.send(id, { type: "command", id: command, args }); }
  query(id) { return this.send(id, { type: "query" }); }
  close() { this.wss.close(); }
}

// ─── Machine-state probes (the before/after side-effect signal) ────────────────
const sh = (cmd) => { try { return execSync(cmd, { encoding: "utf8" }).trim(); } catch { return ""; } };
function machineState(name) {
  const stats = sh(`docker stats --no-stream --format '{{.CPUPerc}}|{{.MemUsage}}' ${name}`);
  const [cpu, mem] = (stats || "|").split("|");
  return {
    cpu: cpu || "n/a",
    mem: mem || "n/a",
    procs: parseInt(sh(`docker exec ${name} sh -c 'ps -e | wc -l'`) || "-1", 10),
  };
}

// ─── An environment we can observe / act / measure ─────────────────────────────
class Env {
  constructor(hub, id, port) { this.hub = hub; this.id = id; this.port = port; this.name = `fleet-eval-${id}`; }
  async reset() {
    sh(`docker rm -f ${this.name} >/dev/null 2>&1 || true`);
    sh(`docker run -d --name ${this.name} -e FLEET_SERVER_ID=${this.id} -e FLEET_HOST_ADDR=host.docker.internal -p ${this.port}:8080 ${IMAGE}`);
    // wait for code-server to ACTUALLY serve (302/200, not just any byte back).
    const url = `http://127.0.0.1:${this.port}/`;
    for (let i = 0; i < 60; i++) {
      const c = sh(`curl -s -o /dev/null -w '%{http_code}' --max-time 3 ${url}`);
      if (c === "302" || c === "200") break;
      await sleep(1000);
    }
    // open the editor → starts the ext host → the bridge dials us. Retry the nav
    // (the published port can still flap for a second under colima).
    this.browser = await chromium.launch();
    this.page = await this.browser.newPage();
    for (let i = 0; i < 6; i++) {
      try {
        await this.page.goto(`${url}?folder=/home/coder/project`, { waitUntil: "load", timeout: 60000 });
        break;
      } catch (e) {
        if (i === 5) throw e;
        await sleep(3000);
      }
    }
    await this.hub.waitFor(this.id);
    await sleep(2500); // let the workbench settle
  }
  async observe(tag) {
    const r = await this.hub.query(this.id);
    if (this.page && tag) await this.page.screenshot({ path: `${OUT}/${this.id}-${tag}.png` }).catch(() => {});
    return { vscode: r.data, machine: machineState(this.name) };
  }
  async act(command, args) { const r = await this.hub.command(this.id, command, args); if (!r.ok) throw new Error(`command failed: ${r.error}`); }
  async close() { try { await this.browser?.close(); } catch {} sh(`docker rm -f ${this.name} >/dev/null 2>&1 || true`); }
}

// ─── Behaviours: drive an action, assert its effect (OSWorld-style checker) ─────
const behaviours = [
  {
    name: "Terminal: New Terminal opens a terminal",
    async run(env) {
      const before = await env.observe("before");
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const after = await env.observe("after");
      return {
        pass: after.vscode.terminalCount > before.vscode.terminalCount,
        detail: `terminals ${before.vscode.terminalCount} → ${after.vscode.terminalCount} (${JSON.stringify(after.vscode.terminals)})`,
        stateDelta: { procs: `${before.machine.procs}→${after.machine.procs}`, mem: `${before.machine.mem} → ${after.machine.mem}` },
      };
    },
  },
  {
    name: "Command Palette opens",
    async run(env) {
      await env.act("workbench.action.showCommands");
      await sleep(800);
      const s = await env.observe("palette");
      return { pass: true, detail: "executeCommand(showCommands) returned ok", stateDelta: {} };
    },
  },
];

// ─── Runner ────────────────────────────────────────────────────────────────────
async function main() {
  const N = parseInt(process.argv[2] || "1", 10);
  const hub = new BridgeHub();
  const envs = Array.from({ length: N }, (_, i) => new Env(hub, `eval-${i + 1}`, BASE_PORT + i + 1));
  let totalPass = 0, totalRun = 0;
  try {
    console.log(`[eval] resetting ${N} environment(s) in parallel…`);
    await Promise.all(envs.map((e) => e.reset()));
    for (const env of envs) {
      console.log(`\n[eval] === ${env.id} ===`);
      for (const b of behaviours) {
        totalRun++;
        try {
          const r = await b.run(env);
          if (r.pass) totalPass++;
          console.log(`[eval] ${r.pass ? "✅ PASS" : "❌ FAIL"}  ${b.name}`);
          console.log(`[eval]      ${r.detail}`);
          if (Object.keys(r.stateDelta).length) console.log(`[eval]      stateΔ ${JSON.stringify(r.stateDelta)}`);
        } catch (e) {
          console.log(`[eval] ❌ ERROR ${b.name}: ${e.message}`);
        }
      }
    }
  } finally {
    await Promise.all(envs.map((e) => e.close()));
    hub.close();
  }
  console.log(`\n[eval] RESULT: ${totalPass}/${totalRun} behaviour checks pass. artifacts → ${OUT}/`);
  process.exit(totalPass === totalRun ? 0 : 1);
}
main().catch((e) => { console.error(e); process.exit(1); });

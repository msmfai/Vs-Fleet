// Env — the testable unit (§3.2). One container + one Playwright page + one bridge
// conn + machine probes. reset() boots the scenario's image, waits for code-server
// to actually serve (302/200), Playwright-opens the editor (which brings the
// ext-host online so the bridge dials the hub), then waits for the bridge. close()
// ALWAYS cleans up (browser + docker rm), even on a half-built env.
//
// Ported from harness.mjs's Env, generalized to honor a Scenario's image/docker opts
// and to expose the full §3.2 surface (request/exec/screenshot/supports).

import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";
import { machineState } from "./machine.mjs";

const DEFAULT_IMAGE = "fleet-env:latest";
export const OUT = process.env.FLEET_EVAL_OUT || "/tmp/fleet-eval";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd) => {
  try { return execSync(cmd, { encoding: "utf8" }).trim(); } catch { return ""; }
};
mkdirSync(OUT, { recursive: true });

export class Env {
  /**
   * @param {import("./bridgeHub.mjs").BridgeHub} hub
   * @param {string} id        unique server id (also the bridge handshake id)
   * @param {number} port      free host port mapped to container :8080
   * @param {import("../scenarios/_contract.mjs").Scenario} scenario
   */
  constructor(hub, id, port, scenario) {
    this.hub = hub;
    this.id = id;
    this.port = port;
    this.scenario = scenario || { id: "base", title: "Base" };
    this.name = `fleet-eval-${id}`;
    this.browser = null;
    this.page = null;
    this.bootError = null; // set when reset fails in an expected-failure scenario
  }

  // Build the `docker run` argv from the scenario's docker opts (§3.4).
  _dockerRunCmd() {
    const s = this.scenario;
    const image = s.image || DEFAULT_IMAGE;
    const d = s.docker || {};
    const parts = [
      "docker run -d",
      `--name ${this.name}`,
      `-e FLEET_SERVER_ID=${this.id}`,
      "-e FLEET_HOST_ADDR=host.docker.internal",
    ];
    if (d.env) for (const [k, v] of Object.entries(d.env)) parts.push(`-e ${k}=${JSON.stringify(v)}`);
    if (d.memory) parts.push(`--memory ${d.memory}`);
    if (d.cpus) parts.push(`--cpus ${d.cpus}`);
    if (d.network) parts.push(`--network ${d.network}`);
    // With `--network none` we cannot publish a port nor reach code-server over
    // http; the scenario owns that tradeoff (its behaviours assert via exec).
    if (d.network !== "none") parts.push(`-p ${this.port}:8080`);
    parts.push(image);
    return parts.join(" ");
  }

  async reset() {
    sh(`docker rm -f ${this.name} >/dev/null 2>&1 || true`);
    sh(this._dockerRunCmd());

    const noNet = this.scenario?.docker?.network === "none";
    if (!noNet) {
      // Wait for code-server to ACTUALLY serve (302/200, not just any byte). §8.
      const url = `http://127.0.0.1:${this.port}/`;
      let served = false;
      for (let i = 0; i < 60; i++) {
        const c = sh(`curl -s -o /dev/null -w '%{http_code}' --max-time 3 ${url}`);
        if (c === "302" || c === "200") { served = true; break; }
        await sleep(1000);
      }
      if (!served) throw new Error(`code-server never served 302/200 on :${this.port}`);

      // Open the editor → starts the ext host → the bridge dials the hub. Retry the
      // nav: the published port can flap for ~1s under colima (§8).
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
    }

    await this.hub.waitFor(this.id);
    await sleep(2500); // let the workbench settle

    // Scenario setup runs after the env is live (git clone / write files / inject).
    if (this.scenario?.setup) await this.scenario.setup(this);
  }

  supports(cap) { return this.hub.supports(this.id, cap); }

  async observe(tag) {
    const r = await this.hub.query(this.id);
    const obs = { vscode: r.data, machine: machineState(this.name) };
    if (tag) {
      const p = await this.screenshot(tag).catch(() => null);
      if (p) obs.screenshot = p;
    }
    return obs;
  }

  // §3.2: executeCommand; throws on !ok.
  async act(command, args) {
    const r = await this.hub.command(this.id, command, args);
    if (!r.ok) throw new Error(`command failed: ${r.error || "unknown"}`);
    return r;
  }

  // §3.2: raw bridge round-trip for §3.3 actions/queries. Throws on explicit !ok.
  async request(msg) {
    const r = await this.hub.request(this.id, msg);
    if (r && r.ok === false) throw new Error(`request failed: ${r.error || "unknown"}`);
    return r;
  }

  // §3.2: docker exec in the container; returns trimmed stdout ("" on failure).
  exec(shCmd) { return sh(`docker exec ${this.name} sh -lc ${JSON.stringify(shCmd)}`); }

  // §3.2: returns the screenshot path. No-op-safe when there is no page (no-net).
  async screenshot(tag) {
    const path = `${OUT}/${this.id}-${tag}.png`;
    if (!this.page) return path;
    await this.page.screenshot({ path });
    return path;
  }

  // §3.2: ALWAYS cleans up — browser then container — swallowing errors.
  async close() {
    try { await this.browser?.close(); } catch {}
    sh(`docker rm -f ${this.name} >/dev/null 2>&1 || true`);
  }
}

// Env — the testable unit (§3.2). One container + one Playwright page + one bridge
// conn + machine probes. reset() boots the scenario's image, waits for code-server
// to actually serve (302/200), Playwright-opens the editor (which brings the
// ext-host online so the bridge dials the hub), then waits for the bridge. close()
// ALWAYS cleans up (browser + docker rm), even on a half-built env.
//
// Ported from harness.mjs's Env, generalized to honor a Scenario's image/docker opts
// and to expose the full §3.2 surface (request/exec/screenshot/supports).

import { execSync, execFileSync } from "node:child_process";
import { mkdirSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { machineState } from "./machine.mjs";

const DEFAULT_IMAGE = "fleet-env:latest";
const EVAL_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const OUT = process.env.FLEET_EVAL_OUT || resolve(EVAL_ROOT, "artifacts");
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
    this.claudeAuthed = false; // true once the container's claude can authenticate
    this._screenshots = [];
  }

  // Inject claude auth into the running container so agent.* behaviours can run.
  // Two sources: ANTHROPIC_API_KEY (forwarded as a container env), or the host's
  // subscription OAuth from the macOS Keychain piped straight into the container's
  // ~/.claude/.credentials.json (the value never passes through the harness/logs).
  // Returns true if the container ends up authenticated.
  async _injectClaudeAuth() {
    if (process.env.ANTHROPIC_API_KEY) return true; // already passed via -e
    if (process.env.FLEET_CLAUDE_OAUTH === "0") return false;
    const pipe =
      `security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null | ` +
      `docker exec -i ${this.name} sh -c ` +
      `'mkdir -p ~/.claude && cat > ~/.claude/.credentials.json && chmod 600 ~/.claude/.credentials.json'`;
    for (let i = 0; i < 5; i++) {
      try {
        execSync(pipe, { stdio: ["ignore", "ignore", "ignore"], shell: "/bin/bash", timeout: 30000 });
        if (sh(`docker exec ${this.name} sh -c 'test -s ~/.claude/.credentials.json && echo ok'`).includes("ok")) {
          return true;
        }
      } catch { /* container not exec-able yet / no keychain access — retry/fall through */ }
      await sleep(1500);
    }
    return false;
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
    // Authenticate the container's claude (for the agent.* behaviours). Two paths:
    //  1) ANTHROPIC_API_KEY passthrough — portable, the recommended path. Set it in
    //     the host env and the harness forwards it.
    //  2) Mount host ~/.claude read-only — ONLY if it actually holds a creds FILE.
    //     macOS Keychain auth has no file to mount, and mounting a credless dir
    //     read-only just breaks claude's config writes — so we gate on the file.
    //     Opt out with FLEET_MOUNT_CLAUDE_AUTH=0.
    if (process.env.ANTHROPIC_API_KEY) parts.push("-e ANTHROPIC_API_KEY");
    const claudeDir = process.env.HOME ? `${process.env.HOME}/.claude` : null;
    if (
      process.env.FLEET_MOUNT_CLAUDE_AUTH !== "0" &&
      claudeDir &&
      existsSync(`${claudeDir}/.credentials.json`)
    ) {
      parts.push(`-v ${claudeDir}:/home/coder/.claude:ro`);
    }
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

    // Authenticate the container's claude (for agent.* behaviours) once it's live.
    this.claudeAuthed = await this._injectClaudeAuth();

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

  // Fire a command without awaiting a reply — for commands whose executeCommand
  // promise doesn't resolve headlessly. Verify the effect via observe().
  fire(command, args = []) { this.hub.fire(this.id, { type: "command", id: command, args }); }

  // §3.2: raw bridge round-trip for §3.3 actions/queries. Throws on explicit !ok.
  async request(msg) {
    const r = await this.hub.request(this.id, msg);
    if (r && r.ok === false) throw new Error(`request failed: ${r.error || "unknown"}`);
    return r;
  }

  // §3.2: docker exec in the container; returns trimmed stdout ("" on failure).
  // shCmd is passed as a DIRECT argv element (not through an outer host `sh -c`), so
  // the container's `sh -lc` is what expands its $vars / $(...). A string-built
  // `docker exec … sh -lc "<shCmd>"` would let the HOST shell expand them first
  // (to empty), silently corrupting any command that uses shell variables.
  exec(shCmd) {
    try {
      return execFileSync("docker", ["exec", this.name, "sh", "-lc", shCmd], { encoding: "utf8" }).trim();
    } catch {
      return "";
    }
  }

  // §3.2: returns the screenshot path. No-op-safe when there is no page (no-net).
  async screenshot(tag) {
    const path = `${OUT}/${this.id}-${tag}.png`;
    if (!this.page) return path;
    await this.page.screenshot({ path });
    this._screenshots.push(path);
    return path;
  }

  drainScreenshots() {
    const paths = this._screenshots;
    this._screenshots = [];
    return [...new Set(paths)];
  }

  // §3.2: ALWAYS cleans up — browser then container — swallowing errors.
  async close() {
    try { await this.browser?.close(); } catch {}
    sh(`docker rm -f ${this.name} >/dev/null 2>&1 || true`);
  }
}

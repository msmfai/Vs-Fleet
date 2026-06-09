// run.mjs — CLI orchestrator for the Fleet behaviour suite.
//
//   node run.mjs [flags]
//     --list                 print registered scenarios + behaviours and exit
//     --scenarios a,b         only these scenario ids (default: all)
//     --behaviours x,y        only these behaviour ids (default: all)
//     --tags t1,t2            only behaviours having ANY of these tags
//     --parallel N            bounded worker pool size (default 1)
//     --keep                  do not docker-rm envs after the run (debug)
//     --json <path>           also write the §3.5 JSON report there
//
// A run = the (scenario × behaviour) matrix. For each scenario we boot ONE env
// (free-port allocated) and run all its applicable behaviours against it
// ("shared" isolation); a behaviour marked isolation:"fresh" gets its own env.
// machineΔ + screenshots + timings are attached by the runner (not the behaviour).
// Cleanup is GUARANTEED via finally + a process-exit trap.

import { createServer, createConnection } from "node:net";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { loadRegistry } from "./registry.mjs";
import { BridgeHub } from "./lib/bridgeHub.mjs";
import { Env, OUT } from "./lib/env.mjs";
import { machineState, machineDelta } from "./lib/machine.mjs";
import { Reporter } from "./lib/report.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Hub lifecycle ────────────────────────────────────────────────────────────
// The agent.* behaviours need a Hub on 0.0.0.0:51777 (their env's reporter phones
// home there). Reuse one if already up, else spawn target/debug/fleet-hub for the
// run. If the binary is missing, agent behaviours runtime-SKIP cleanly.
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
function tcpOpen(port, host = "127.0.0.1", ms = 500) {
  return new Promise((res) => {
    const s = createConnection({ port, host });
    const done = (v) => { try { s.destroy(); } catch {} res(v); };
    s.once("connect", () => done(true));
    s.once("error", () => done(false));
    setTimeout(() => done(false), ms);
  });
}
async function startHub() {
  if (await tcpOpen(51777)) return { child: null, reused: true };
  const bin = resolve(REPO_ROOT, "target/debug/fleet-hub");
  if (!existsSync(bin)) { console.warn("[eval] fleet-hub not built — agent.* behaviours will SKIP"); return { child: null }; }
  const child = spawn(bin, [], { env: { ...process.env, FLEET_WS_ADDR: "0.0.0.0", RUST_LOG: "warn" }, stdio: "ignore" });
  for (let i = 0; i < 20; i++) { if (await tcpOpen(51777)) { console.log("[eval] Hub up on :51777"); return { child }; } await sleep(500); }
  console.warn("[eval] Hub did not come up on :51777");
  return { child };
}

// ─── CLI parsing ────────────────────────────────────────────────────────────
function parseArgs(argv) {
  const a = { parallel: 1, keep: false, list: false, json: null,
    scenarios: null, behaviours: null, tags: null };
  for (let i = 0; i < argv.length; i++) {
    const f = argv[i];
    const next = () => argv[++i];
    const csv = (s) => (s ? s.split(",").map((x) => x.trim()).filter(Boolean) : []);
    switch (f) {
      case "--list": a.list = true; break;
      case "--keep": a.keep = true; break;
      case "--parallel": a.parallel = Math.max(1, parseInt(next() || "1", 10)); break;
      case "--json": a.json = next(); break;
      case "--scenarios": a.scenarios = csv(next()); break;
      case "--behaviours": a.behaviours = csv(next()); break;
      case "--tags": a.tags = csv(next()); break;
      default:
        if (f.startsWith("--")) console.warn(`[eval] unknown flag: ${f}`);
    }
  }
  return a;
}

// ─── Free-port allocation (§3 / §4: no fixed 8200+i) ─────────────────────────
function freePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.unref();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

// ─── Matrix selection ────────────────────────────────────────────────────────
// A behaviour applies to a scenario when its `scenarios` list includes the id, OR
// (default) when it lists none and the scenario id starts with "base".
function behaviourAppliesTo(b, scenario) {
  if (Array.isArray(b.scenarios) && b.scenarios.length) return b.scenarios.includes(scenario.id);
  return scenario.id.startsWith("base");
}

function selectBehaviours(all, args) {
  let bs = all;
  if (args.behaviours) bs = bs.filter((b) => args.behaviours.includes(b.id));
  if (args.tags) bs = bs.filter((b) => (b.tags || []).some((t) => args.tags.includes(t)));
  return bs;
}

function selectScenarios(all, args) {
  if (args.scenarios) return all.filter((s) => args.scenarios.includes(s.id));
  return all;
}

// Capabilities a behaviour/scenario needs but the env's bridge doesn't advertise.
function missingCaps(env, needs) {
  if (!Array.isArray(needs)) return [];
  return needs.filter((c) => !env.supports(c));
}

// ─── Run one behaviour against an env, building the §3.5 result row ──────────
async function runBehaviour(env, scenario, behaviour) {
  const row = { scenario: scenario.id, behaviour: behaviour.id, pass: false, detail: "" };

  // Capability gate → SKIP cleanly (never a hard fail).
  const missing = missingCaps(env, behaviour.needs);
  if (missing.length) {
    row.skipped = `needs caps: ${missing.join(", ")}`;
    return row;
  }

  const tAct0 = Date.now();
  const before = machineState(env.name);
  try {
    const res = await behaviour.run(env);
    const effectMs = Date.now() - tAct0;
    const after = machineState(env.name);
    // A behaviour may decide AT RUNTIME that it can't run (e.g. a host-side Hub
    // dependency is absent) and return `skipped`. Honor it as a clean SKIP — the
    // reporter (console/JUnit/HTML/summary) branches on `row.skipped`, not pass.
    if (res.skipped) {
      row.skipped = res.skipped;
      row.detail = res.detail || "";
      if (res.evidence) row.evidence = res.evidence;
      row.timingsMs = { effect: effectMs };
      return row;
    }
    row.pass = !!res.pass;
    row.detail = res.detail || "";
    if (res.evidence) row.evidence = res.evidence;
    row.machineDelta = machineDelta(before, after);
    row.timingsMs = { effect: effectMs };
    // Screenshots the behaviour captured via observe(tag) land in OUT keyed by tag.
    const shot = await env.screenshot(`${behaviour.id}.result`).catch(() => null);
    if (shot) row.screenshots = [shot];
  } catch (e) {
    row.error = e?.message || String(e);
    row.timingsMs = { effect: Date.now() - tAct0 };
  }
  return row;
}

// ─── Run every applicable behaviour for ONE scenario in ONE env ──────────────
// (plus a fresh env per behaviour that asks for isolation:"fresh").
async function runScenario(hub, scenario, behaviours, reporter, args, idPrefix) {
  const applicable = behaviours.filter((b) => behaviourAppliesTo(b, scenario));
  if (!applicable.length) return;

  const shared = applicable.filter((b) => (b.isolation || "shared") !== "fresh");
  const fresh = applicable.filter((b) => (b.isolation || "shared") === "fresh");

  // Shared env: boot once, run all shared behaviours.
  if (shared.length) {
    const env = new Env(hub, `${idPrefix}-${scenario.id}`, await freePort(), scenario);
    try {
      await bootOrReport(env, scenario, shared, reporter);
      if (!env.bootError) {
        for (const b of shared) reporter.add(await runBehaviour(env, scenario, b));
      }
    } finally {
      if (!args.keep) await env.close(); else console.log(`[eval] --keep: left ${env.name}`);
    }
  }

  // Fresh envs: one per behaviour for isolation.
  let n = 0;
  for (const b of fresh) {
    const env = new Env(hub, `${idPrefix}-${scenario.id}-f${n++}`, await freePort(), scenario);
    try {
      await bootOrReport(env, scenario, [b], reporter);
      if (!env.bootError) reporter.add(await runBehaviour(env, scenario, b));
    } finally {
      if (!args.keep) await env.close(); else console.log(`[eval] --keep: left ${env.name}`);
    }
  }
}

// Boot an env, honoring expectBoot. A scenario declared expectBoot:"fail" that
// fails to boot is reported as an expected pass (not an error / hang).
async function bootOrReport(env, scenario, behaviours, reporter) {
  try {
    await env.reset();
  } catch (e) {
    env.bootError = e?.message || String(e);
    const expectFail = scenario.expectBoot === "fail";
    if (expectFail) {
      reporter.add({
        scenario: scenario.id, behaviour: "(boot)", pass: true,
        detail: `boot failed as expected (expectBoot:fail): ${env.bootError}`,
      });
    } else {
      // Report each intended behaviour as an error so the matrix stays accountable.
      for (const b of behaviours) {
        reporter.add({
          scenario: scenario.id, behaviour: b.id, pass: false,
          error: `env boot failed: ${env.bootError}`,
        });
      }
    }
  }
}

// ─── Bounded worker pool over scenarios ──────────────────────────────────────
async function pool(items, size, worker) {
  let idx = 0;
  const runners = Array.from({ length: Math.min(size, items.length) }, async () => {
    while (idx < items.length) {
      const i = idx++;
      await worker(items[i], i);
    }
  });
  await Promise.all(runners);
}

// ─── --list ──────────────────────────────────────────────────────────────────
function printList(scenarios, behaviours) {
  console.log(`\nScenarios (${scenarios.length}):`);
  for (const s of scenarios) {
    const img = s.image ? ` [${s.image}]` : "";
    const boot = s.expectBoot && s.expectBoot !== "ok" ? ` expectBoot:${s.expectBoot}` : "";
    console.log(`  ${s.id.padEnd(16)} ${s.title}${img}${boot}`);
  }
  console.log(`\nBehaviours (${behaviours.length}):`);
  for (const b of behaviours) {
    const tags = (b.tags || []).join(",");
    const needs = b.needs?.length ? ` needs:[${b.needs.join(",")}]` : "";
    const iso = b.isolation === "fresh" ? " fresh" : "";
    console.log(`  ${b.id.padEnd(20)} ${b.title}  (${tags})${needs}${iso}`);
  }
  console.log();
}

// ─── main ─────────────────────────────────────────────────────────────────────
async function main() {
  const args = parseArgs(process.argv.slice(2));
  const { behaviours: allB, scenarios: allS } = await loadRegistry();

  const scenarios = selectScenarios(allS, args);
  const behaviours = selectBehaviours(allB, args);

  if (args.list) { printList(scenarios, behaviours); return 0; }

  if (!scenarios.length) { console.error("[eval] no scenarios selected"); return 1; }
  if (!behaviours.length) { console.error("[eval] no behaviours selected"); return 1; }

  const hub = new BridgeHub();
  const fleetHub = await startHub();
  const reporter = new Reporter({
    image: scenarios[0]?.image || "fleet-env:latest",
    scenarios: scenarios.length,
    behaviours: behaviours.length,
  });

  const stopFleetHub = () => { try { fleetHub.child?.kill(); } catch {} };
  // Guaranteed cleanup trap: if the process is asked to die mid-run, still close hub.
  const onSignal = () => { try { hub.close(); } catch {} stopFleetHub(); process.exit(130); };
  process.on("SIGINT", onSignal);
  process.on("SIGTERM", onSignal);

  console.log(`[eval] ${scenarios.length} scenario(s) × ${behaviours.length} behaviour(s),` +
    ` parallel=${args.parallel}. artifacts → ${OUT}/`);

  try {
    await pool(scenarios, args.parallel, (scenario, i) =>
      runScenario(hub, scenario, behaviours, reporter, args, `r${i + 1}`));
  } finally {
    hub.close();
    stopFleetHub();
  }

  if (args.json) reporter.writeJSON(args.json);
  const ok = reporter.finish();
  return ok ? 0 : 1;
}

main()
  .then((code) => process.exit(code))
  .catch((e) => { console.error(e); process.exit(1); });

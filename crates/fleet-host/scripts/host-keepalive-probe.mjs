#!/usr/bin/env node
// Host-side Fleet keepalive probe.
//
// This exercises the actual Tauri host window, not only the container/code-server
// eval lane: launch Fleet with two autospawned servers, click between rail rows,
// capture Fleet-window screenshots, record RSS/log evidence, and write a report
// compatible with containers/fleet-env/eval/scripts/review-server.mjs.

import { execFileSync, spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { writeScreenshotMetadata } from "../../../containers/fleet-env/eval/lib/reviewContext.mjs";
import { analyzeWindowShots, attachVisualAnalysis } from "./analyze-window-shots.mjs";
import { captureMacWindow, findFleetWindow } from "./macos-window-shot.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const HOST_DIR = resolve(__dirname, "..");
const ROOT = resolve(HOST_DIR, "../..");
const DEFAULT_OUT = resolve(
  HOST_DIR,
  "artifacts",
  "keepalive",
  new Date().toISOString().replaceAll(":", "").replace(/\..+$/, "Z"),
);

const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
const FLEET_WINDOW_OWNERS = ["Fleet", "fleet-host"];

const RATIONALE = `
WHAT: Launches the real Fleet Tauri host with two autospawned VS Code serve-web
servers, then switches between the first and second rail rows while capturing
direct screenshots of the Fleet window itself, including the rail and embedded
editor pane.

WHY THIS IS THE EXPECTED OUTCOME: Fleet's cmux-like contract is that tab switching
preserves loaded VS Code clients. The host log should show two persistent editor
surfaces created, no persistent-editor creation failures, and no bridge
deregistration during switching. The screenshots provide human-visible evidence
that the rail and editor remain present rather than disappearing, cropping, or
turning black. They are captured by CoreGraphics window id, so the evidence does
not depend on Fleet being frontmost or uncovered.

WHY IT MATTERS: The container eval suite proves individual VS Code workbenches and
bridge commands, but it does not boot the desktop multiplexer. This probe covers
the missing host lane: Tauri child-webview creation, visibility switching, window
tiling, close cleanup for a managed server, and full-window screenshots for
review.`;

function parseArgs(argv) {
  const out = {
    out: DEFAULT_OUT,
    build: true,
    autospawn: 2,
    settleMs: 30000,
    switchDelayMs: 3500,
    controlPort: 51776,
    clickSwitch: false,
    closeCheck: true,
    appBundle: false,
    keep: false,
    allowBusyPorts: false,
    allowExistingManaged: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = () => argv[++i];
    if (arg === "--out") out.out = resolve(next());
    else if (arg === "--no-build") out.build = false;
    else if (arg === "--build") out.build = true;
    else if (arg === "--autospawn" || arg === "--autosspawn") out.autospawn = Number(next());
    else if (arg === "--settle-ms") out.settleMs = Number(next());
    else if (arg === "--switch-delay-ms") out.switchDelayMs = Number(next());
    else if (arg === "--control-port") out.controlPort = Number(next());
    else if (arg === "--click-switch") out.clickSwitch = true;
    else if (arg === "--skip-close-check") out.closeCheck = false;
    else if (arg === "--app-bundle") out.appBundle = true;
    else if (arg === "--keep") out.keep = true;
    else if (arg === "--allow-busy-ports") out.allowBusyPorts = true;
    else if (arg === "--allow-existing-managed") out.allowExistingManaged = true;
    else if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!Number.isFinite(out.autospawn) || out.autospawn < 2) {
    throw new Error("--autospawn must be at least 2");
  }
  if (out.clickSwitch) out.closeCheck = false;
  return out;
}

function usage() {
  console.log(`usage: node crates/fleet-host/scripts/host-keepalive-probe.mjs [options]

Options:
  --out DIR              Output directory. Default: crates/fleet-host/artifacts/keepalive/<timestamp>
  --build                Build fleet-host + fleet-reporter first. Default.
  --no-build             Reuse existing debug binaries.
  --autospawn N          Number of servers Fleet should spawn. Default: 2.
  --settle-ms MS         Wait after launch before first screenshot. Default: 30000.
  --switch-delay-ms MS   Wait after each rail click before screenshot. Default: 3500.
  --control-port PORT    Loopback probe-control port. Default: 51776.
  --click-switch         Switch with System Events clicks instead of probe control.
  --skip-close-check     Do not close server-2 and assert its managed processes exit.
  --app-bundle           Launch crates/fleet-host/Fleet.app/Contents/MacOS/fleet-host.
  --keep                 Leave Fleet running for manual debugging.
  --allow-busy-ports     Do not preflight fixed Fleet ports 51777/51778.
  --allow-existing-managed
                         Do not fail if old Fleet-managed server processes exist.

After the run, browse screenshots with:
  node containers/fleet-env/eval/scripts/review-server.mjs --json <out>/host-keepalive.json --dir <out>
`);
}

function run(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, {
    cwd: opts.cwd || ROOT,
    env: opts.env || process.env,
    encoding: "utf8",
    stdio: opts.capture ? "pipe" : "inherit",
  });
  if (result.status !== 0) {
    const suffix = opts.capture
      ? `\nstdout:\n${result.stdout || ""}\nstderr:\n${result.stderr || ""}`
      : "";
    throw new Error(`command failed: ${cmd} ${args.join(" ")}${suffix}`);
  }
  return result.stdout || "";
}

function output(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, {
    cwd: opts.cwd || ROOT,
    env: opts.env || process.env,
    encoding: "utf8",
    stdio: "pipe",
  });
  if (result.status !== 0) return "";
  return result.stdout || "";
}

function portInUse(port) {
  return output("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN"]).trim().length > 0;
}

function existingManagedProcesses() {
  return output("ps", ["-axo", "pid,ppid,command"])
    .split(/\r?\n/)
    .filter((line) => /fleet-reporter --serve|code serve-web|bin\/code-server|server-main\.js/.test(line))
    .filter((line) => /fleet-mux|\/\.fleet\/mux/.test(line));
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function managedProcessesForServer(id) {
  const escaped = escapeRegExp(id);
  const pattern = new RegExp(`reporter-${escaped}\\.sock|cs-userdata-${escaped}|ws-${escaped}`);
  return existingManagedProcesses().filter((line) => pattern.test(line));
}

async function waitForServerProcessesGone(id, ms) {
  const deadline = Date.now() + ms;
  let lines = managedProcessesForServer(id);
  while (lines.length && Date.now() < deadline) {
    await sleep(250);
    lines = managedProcessesForServer(id);
  }
  return lines;
}

function normalizeFleetWindow(window) {
  if (window && "x" in window && "width" in window) {
    return {
      id: Number(window.id),
      owner: window.owner,
      name: window.name,
      pid: Number(window.pid),
      layer: Number(window.layer),
      onscreen: Boolean(window.onscreen),
      x: Number(window.x || 0),
      y: Number(window.y || 0),
      width: Number(window.width || 0),
      height: Number(window.height || 0),
    };
  }
  const bounds = window.bounds || {};
  return {
    id: Number(window.id),
    owner: window.owner,
    name: window.name,
    pid: Number(window.pid),
    layer: Number(window.layer),
    onscreen: Number(window.onscreen) === 1,
    x: Number(bounds.X || 0),
    y: Number(bounds.Y || 0),
    width: Number(bounds.Width || 0),
    height: Number(bounds.Height || 0),
  };
}

function fleetWindowBounds(toolDir) {
  return normalizeFleetWindow(findFleetWindow({
    owner: FLEET_WINDOW_OWNERS,
    toolDir,
    minArea: 500000,
  }));
}

async function waitForWindow(ms, toolDir) {
  const deadline = Date.now() + ms;
  let last = null;
  while (Date.now() < deadline) {
    try {
      return fleetWindowBounds(toolDir);
    } catch (err) {
      last = err;
      await sleep(1000);
    }
  }
  throw new Error(`Fleet window did not appear: ${last?.message || last}`);
}

async function clickAt(pid, x, y) {
  const script = `
with timeout of 5 seconds
tell application "System Events"
  set frontmost of (first process whose unix id is ${Number(pid)}) to true
  click at {${Math.round(x)}, ${Math.round(y)}}
end tell
end timeout`;
  let last = null;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      execFileSync("osascript", ["-e", script], { stdio: "pipe", timeout: 7000 });
      return;
    } catch (err) {
      last = err;
      await sleep(500);
    }
  }
  throw last;
}

async function controlRequest(port, path) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 5000);
  try {
    const res = await fetch(`http://127.0.0.1:${port}${path}`, { signal: controller.signal });
    const text = await res.text();
    if (!res.ok) throw new Error(`probe control ${path} failed: ${res.status} ${text}`);
    return text ? JSON.parse(text) : {};
  } finally {
    clearTimeout(timer);
  }
}

async function waitForControl(port, ms) {
  const deadline = Date.now() + ms;
  let last = null;
  while (Date.now() < deadline) {
    try {
      return await controlRequest(port, "/healthz");
    } catch (err) {
      last = err;
      await sleep(500);
    }
  }
  throw new Error(`Fleet probe control did not become ready on port ${port}: ${last?.message || last}`);
}

async function waitForServers(port, minCount, ms) {
  const deadline = Date.now() + ms;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const body = await controlRequest(port, "/servers");
      const servers = Array.isArray(body.servers) ? body.servers : [];
      if (servers.length >= minCount) return servers;
      last = new Error(`only ${servers.length} server(s) visible`);
    } catch (err) {
      last = err;
    }
    await sleep(500);
  }
  throw new Error(`Fleet did not report ${minCount} servers: ${last?.message || last}`);
}

async function selectViaControl(port, id) {
  return controlRequest(port, `/select/${encodeURIComponent(id)}`);
}

async function closeViaControl(port, id) {
  return controlRequest(port, `/close/${encodeURIComponent(id)}`);
}

async function capture(path, window, toolDir) {
  let candidate = window;
  let last = null;
  for (let attempt = 0; attempt < 4; attempt++) {
    if (attempt > 0) {
      await sleep(500);
      candidate = fleetWindowBounds(toolDir);
    }
    try {
      const result = captureMacWindow({ out: path, window: candidate });
      return {
        ...result,
        window: normalizeFleetWindow(result.window),
        attempts: attempt + 1,
      };
    } catch (err) {
      last = err;
    }
  }
  throw last;
}

function logText(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
}

function countMatches(text, pattern) {
  return (text.match(pattern) || []).length;
}

function latestInboxConnected(text) {
  let latest = null;
  const re = /inbox → window: .*connected=(true|false)/g;
  for (const match of text.matchAll(re)) {
    latest = match[1] === "true";
  }
  return latest;
}

function parsePsRows() {
  const ps = output("ps", ["-axo", "pid,ppid,rss,command"]);
  return ps
    .split(/\r?\n/)
    .map((line) => {
      const parts = line.trim().split(/\s+/, 4);
      const pid = Number(parts[0]);
      const ppid = Number(parts[1]);
      const rss = Number(parts[2]);
      return {
        pid,
        ppid,
        rss,
        line,
      };
    })
    .filter((row) => Number.isInteger(row.pid) && Number.isInteger(row.ppid));
}

function rssSnapshot(rootPid) {
  const rows = parsePsRows();
  const descendants = new Set([rootPid]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      if (!descendants.has(row.pid) && descendants.has(row.ppid)) {
        descendants.add(row.pid);
        changed = true;
      }
    }
  }

  const lines = rows
    .filter((row) => descendants.has(row.pid))
    .map((row) => row.line);
  const rssKiB = lines.reduce((sum, line) => {
    const parts = line.trim().split(/\s+/);
    const rss = Number(parts[2]);
    return sum + (Number.isFinite(rss) ? rss : 0);
  }, 0);
  return {
    rootPid,
    rssKiB,
    rssMiB: Number((rssKiB / 1024).toFixed(1)),
    lines,
  };
}

async function waitForLog(logPath, pattern, minCount, ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const text = logText(logPath);
    if (countMatches(text, pattern) >= minCount) return true;
    await sleep(1000);
  }
  return false;
}

async function terminate(child) {
  if (!child || child.exitCode != null) return;
  child.kill("SIGTERM");
  for (let i = 0; i < 20; i++) {
    if (child.exitCode != null) return;
    await sleep(250);
  }
  child.kill("SIGKILL");
}

function cleanupManagedProcessGroups() {
  const pids = existingManagedProcesses()
    .map((line) => Number(line.trim().split(/\s+/)[0]))
    .filter((pid) => Number.isInteger(pid) && pid > 1);
  for (const pid of pids) {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {}
  }
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300);
  for (const pid of pids) {
    try {
      process.kill(-pid, "SIGKILL");
    } catch {}
  }
}

async function main() {
  if (process.platform !== "darwin") {
    throw new Error("host keepalive probe currently requires macOS for Tauri window screenshots");
  }

  const opts = parseArgs(process.argv.slice(2));
  const outDir = opts.out;
  const shotDir = resolve(outDir, "screenshots");
  const toolDir = resolve(ROOT, "target", "fleet-window-tools");
  mkdirSync(shotDir, { recursive: true });

  if (!opts.allowBusyPorts) {
    const preflightPorts = [51777, 51778, opts.controlPort];
    const busy = preflightPorts.filter(portInUse);
    if (busy.length) {
      throw new Error(`Fleet fixed port(s) already in use: ${busy.join(", ")}. Close Fleet or rerun with --allow-busy-ports.`);
    }
  }
  if (!opts.allowExistingManaged) {
    const existing = existingManagedProcesses();
    if (existing.length) {
      throw new Error(
        [
          "Fleet-managed server processes already exist. Close/kill them before probing, or rerun with --allow-existing-managed.",
          ...existing,
        ].join("\n"),
      );
    }
  }

  if (opts.build) {
    if (opts.appBundle) {
      run("./bundle.sh", ["debug"], { cwd: HOST_DIR });
    } else {
      run("cargo", ["build"], { cwd: HOST_DIR });
      run("cargo", ["build", "-p", "fleet-reporter"], { cwd: ROOT });
    }
  }

  const appBundle = resolve(HOST_DIR, "Fleet.app");
  const hostBin = opts.appBundle
    ? resolve(appBundle, "Contents/MacOS/fleet-host")
    : resolve(HOST_DIR, "target/debug/fleet-host");
  const reporterBin = opts.appBundle
    ? resolve(appBundle, "Contents/MacOS/fleet-reporter")
    : resolve(ROOT, "target/debug/fleet-reporter");
  const bridgeVsix = opts.appBundle
    ? resolve(appBundle, "Contents/Resources/fleet-bridge.vsix")
    : resolve(ROOT, "packages/fleet-bridge/fleet-bridge-0.2.0.vsix");
  if (!existsSync(hostBin)) throw new Error(`missing fleet-host binary: ${hostBin}`);
  if (!existsSync(reporterBin)) throw new Error(`missing fleet-reporter binary: ${reporterBin}`);
  if (!existsSync(bridgeVsix)) throw new Error(`missing fleet-bridge VSIX: ${bridgeVsix}`);

  const muxDir = resolve(outDir, "mux");
  const runtimeDir = resolve(outDir, "runtime");
  mkdirSync(muxDir, { recursive: true });
  mkdirSync(runtimeDir, { recursive: true });

  const logPath = resolve(outDir, "fleet-host.log");
  const logFd = openSync(logPath, "w");
  const startedAt = new Date().toISOString();
  const childEnv = {
    ...process.env,
    FLEET_AUTOSPAWN: String(opts.autospawn),
    FLEET_EDITOR_KEEPALIVE: "1",
    FLEET_MUX_DIR: muxDir,
    FLEET_RUNTIME_DIR: runtimeDir,
    ...(opts.appBundle ? {} : { FLEET_REPORTER_BIN: reporterBin, FLEET_BRIDGE_VSIX: bridgeVsix }),
    FLEET_PROBE_CONTROL_PORT: String(opts.controlPort),
    RUST_LOG: process.env.RUST_LOG || "info",
  };
  const child = spawn(hostBin, [], {
    cwd: opts.appBundle ? resolve(appBundle, "Contents/MacOS") : HOST_DIR,
    env: childEnv,
    stdio: ["ignore", logFd, logFd],
  });

  let report;
  const screenshots = [];
  const captures = [];
  try {
    child.on("exit", (code, signal) => {
      console.error(`[probe] fleet-host exited code=${code} signal=${signal}`);
    });

    let window = null;
    const take = async (file) => {
      const abs = resolve(shotDir, file);
      const shot = await capture(abs, window, toolDir);
      window = shot.window;
      const rel = relative(outDir, abs);
      screenshots.push(rel);
      captures.push({
        file: rel,
        window: shot.window,
        command: shot.command,
        attempts: shot.attempts,
      });
      await sleep(250);
    };

    window = await waitForWindow(20000, toolDir);
    await waitForControl(opts.controlPort, 10000);
    await waitForLog(logPath, /server registered \(phone-home\)/g, 2, opts.settleMs);
    const visibleServers = await waitForServers(opts.controlPort, 2, 10000);
    const orderedServers = [...visibleServers].sort((a, b) =>
      String(a.label || a.id).localeCompare(String(b.label || b.id)),
    );
    const server1 = orderedServers[0];
    const server2 = orderedServers[1];
    await sleep(Math.max(0, opts.settleMs - 2000));
    await take("01-initial-selected.png");

    const rowX = window.x + 82;
    const row1Y = window.y + 114;
    const row2Y = window.y + 174;
    const switchTo = opts.clickSwitch
      ? async (id, _rowY) => clickAt(child.pid, rowX, _rowY)
      : async (id) => selectViaControl(opts.controlPort, id);

    await switchTo(server1.id, row1Y);
    await sleep(opts.switchDelayMs);
    await take("02-server-1-selected.png");

    await switchTo(server2.id, row2Y);
    await sleep(opts.switchDelayMs);
    await take("03-server-2-selected.png");

    await switchTo(server1.id, row1Y);
    await sleep(opts.switchDelayMs);
    await take("04-server-1-returned.png");

    let closeResult = null;
    let closedServer2Processes = [];
    if (opts.closeCheck) {
      closeResult = await closeViaControl(opts.controlPort, server2.id);
      await sleep(1000);
      closedServer2Processes = await waitForServerProcessesGone(server2.id, 7000);
      await take("05-server-2-closed.png");
    }

    const rss = rssSnapshot(child.pid);
    writeFileSync(resolve(outDir, "rss.json"), JSON.stringify(rss, null, 2));
    writeFileSync(resolve(outDir, "rss.txt"), `${rss.lines.join("\n")}\n`);

    const log = logText(logPath);
    const evidence = {
      pid: child.pid,
      window,
      logPath: relative(outDir, logPath),
      rssMiB: rss.rssMiB,
      registeredCount: countMatches(log, /server registered \(phone-home\)/g),
      deregisteredCount: countMatches(log, /server deregistered \(bridge dropped\)/g),
      editorCreatedCount: countMatches(log, /created persistent editor surface/g),
      editorCreateFailures: countMatches(log, /persistent editor surface creation failed/g),
      persistentNavigations: countMatches(log, /navigating persistent editor surface/g),
      closeRequestedCount: countMatches(log, /close server requested/g),
      closeResult,
      closedServer2Processes,
      server1,
      server2,
      hubConnectedCount: countMatches(log, /host face connected to Hub; subscribed/g),
      hubLinkErrors: countMatches(log, /hub link error; retrying/g),
      inboxConnectedCount: countMatches(log, /inbox → window: .*connected=true/g),
      inboxDisconnectedCount: countMatches(log, /inbox → window: .*connected=false/g),
      latestInboxConnected: latestInboxConnected(log),
      server1SelectionCount: countMatches(log, new RegExp(`selected server server_id=${escapeRegExp(server1.id)}`, "g")),
      server2SelectionCount: countMatches(log, new RegExp(`selected server server_id=${escapeRegExp(server2.id)}`, "g")),
      switchMode: opts.clickSwitch ? "click" : "probe-control",
      closeCheck: opts.closeCheck,
      controlPort: opts.clickSwitch ? null : opts.controlPort,
      launchMode: opts.appBundle ? "app-bundle" : "debug-binary",
      hostBin,
      reporterBin,
      bridgeVsix,
      captures,
    };

    const pass =
      evidence.registeredCount >= 2 &&
      evidence.editorCreatedCount >= 2 &&
      evidence.editorCreateFailures === 0 &&
      evidence.deregisteredCount === 0 &&
      evidence.hubConnectedCount >= 1 &&
      evidence.inboxConnectedCount >= 1 &&
      evidence.latestInboxConnected === true &&
      evidence.server1SelectionCount >= 2 &&
      evidence.server2SelectionCount >= 2 &&
      (!opts.closeCheck ||
        (evidence.closeResult?.closed === true && evidence.closedServer2Processes.length === 0)) &&
      screenshots.length === (opts.closeCheck ? 5 : 4);

    report = {
      run: {
        startedAt,
        image: "fleet-host-local",
        command: process.argv.join(" "),
      },
      results: [
        {
          scenario: "host-keepalive",
          scenarioTitle: "Fleet host with two autospawned local serve-web servers",
          behaviour: "mux.keepaliveSwitch",
          title: "Host mux: switching keeps persistent editor clients visible",
          pass,
          detail: pass
            ? `registered=${evidence.registeredCount}, editors=${evidence.editorCreatedCount}, deregistered=${evidence.deregisteredCount}, closed=${evidence.closeResult?.closed ?? "skipped"}, rss=${rss.rssMiB} MiB`
            : `probe failed: ${JSON.stringify(evidence)}`,
          rationale: RATIONALE,
          evidence,
          screenshots,
        },
      ],
    };
  } catch (err) {
    report = {
      run: {
        startedAt,
        image: "fleet-host-local",
        command: process.argv.join(" "),
      },
      results: [
        {
          scenario: "host-keepalive",
          scenarioTitle: "Fleet host with two autospawned local serve-web servers",
          behaviour: "mux.keepaliveSwitch",
          title: "Host mux: switching keeps persistent editor clients visible",
          pass: false,
          error: err?.stack || String(err),
          detail: err?.message || String(err),
          rationale: RATIONALE,
          evidence: {
            pid: child.pid,
            logPath: relative(outDir, logPath),
            captures,
          },
          screenshots,
        },
      ],
    };
    process.exitCode = 1;
  } finally {
    if (!opts.keep) await terminate(child);
    if (!opts.keep) cleanupManagedProcessGroups();
    closeSync(logFd);
  }

  try {
    const visual = analyzeWindowShots({ baseDir: outDir, report, writeMasks: true });
    attachVisualAnalysis(report, visual, { baseDir: outDir });
    const row = report.results?.[0];
    const flags = visual.summary?.flags || {};
    const flagCount = Object.values(flags).reduce((sum, count) => sum + Number(count || 0), 0);
    if (row && flagCount > 0) {
      row.pass = false;
      row.detail = `${row.detail}; visual flags=${JSON.stringify(flags)}`;
      process.exitCode = 1;
    }
  } catch (err) {
    const row = report.results?.[0];
    row.evidence = row.evidence || {};
    row.evidence.visualAnalysisError = err?.stack || String(err);
  }

  const jsonPath = resolve(outDir, "host-keepalive.json");
  writeFileSync(jsonPath, JSON.stringify(report, null, 2));
  const metadata = writeScreenshotMetadata(report, { baseDir: outDir, quiet: true });

  console.log(`[probe] report: ${jsonPath}`);
  console.log(`[probe] screenshots tagged: ${metadata.tagged}${metadata.missing ? ` (${metadata.missing} missing)` : ""}`);
  console.log(`[probe] review: node containers/fleet-env/eval/scripts/review-server.mjs --json ${jsonPath} --dir ${outDir}`);

  if (!report.results[0]?.pass) process.exitCode = 1;
}

main().catch((err) => {
  console.error(`[probe] ${err?.stack || err}`);
  process.exit(1);
});

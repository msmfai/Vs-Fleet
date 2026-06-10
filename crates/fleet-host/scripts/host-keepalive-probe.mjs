#!/usr/bin/env node
// Host-side Fleet keepalive probe.
//
// This exercises the actual Tauri host window, not only the container/code-server
// eval lane: launch Fleet with two autospawned servers, click between rail rows,
// capture full-screen screenshots, record RSS/log evidence, and write a report
// compatible with containers/fleet-env/eval/scripts/review-server.mjs.

import { spawn, spawnSync, execFileSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { writeScreenshotMetadata } from "../../../containers/fleet-env/eval/lib/reviewContext.mjs";

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

const RATIONALE = `
WHAT: Launches the real Fleet Tauri host with two autospawned VS Code serve-web
servers, then switches between the first and second rail rows while capturing
full-screen screenshots that include the Fleet rail and embedded editor pane.

WHY THIS IS THE EXPECTED OUTCOME: Fleet's cmux-like contract is that tab switching
preserves loaded VS Code clients. The host log should show two persistent editor
surfaces created, no persistent-editor creation failures, and no bridge
deregistration during switching. The screenshots provide human-visible evidence
that the rail and editor remain present rather than disappearing, cropping, or
turning black.

WHY IT MATTERS: The container eval suite proves individual VS Code workbenches and
bridge commands, but it does not boot the desktop multiplexer. This probe covers
the missing host lane: Tauri child-webview creation, visibility switching, window
tiling, and full-window screenshots for review.`;

function parseArgs(argv) {
  const out = {
    out: DEFAULT_OUT,
    build: true,
    autospawn: 2,
    settleMs: 30000,
    switchDelayMs: 3500,
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
    .filter((line) => /fleet-reporter --serve|code serve-web/.test(line))
    .filter((line) => /fleet-mux|\/\.fleet\/mux/.test(line));
}

function fleetWindowBounds() {
  const script = `
set procNames to {"Fleet", "fleet-host"}
tell application "System Events"
  repeat with procName in procNames
    if exists process procName then
      tell process procName
        set frontmost to true
        if (count of windows) > 0 then
          set p to position of window 1
          set s to size of window 1
          return (procName as text) & "," & (item 1 of p) & "," & (item 2 of p) & "," & (item 1 of s) & "," & (item 2 of s)
        end if
      end tell
    end if
  end repeat
end tell
error "Fleet window not found"
`;
  const raw = execFileSync("osascript", ["-e", script], { encoding: "utf8" }).trim();
  const [processName, x, y, width, height] = raw.split(",");
  return {
    processName,
    x: Number(x),
    y: Number(y),
    width: Number(width),
    height: Number(height),
  };
}

async function waitForWindow(ms) {
  const deadline = Date.now() + ms;
  let last = null;
  while (Date.now() < deadline) {
    try {
      return fleetWindowBounds();
    } catch (err) {
      last = err;
      await sleep(1000);
    }
  }
  throw new Error(`Fleet window did not appear: ${last?.message || last}`);
}

function clickAt(x, y) {
  const script = `tell application "System Events" to click at {${Math.round(x)}, ${Math.round(y)}}`;
  execFileSync("osascript", ["-e", script], { stdio: "pipe" });
}

function capture(path) {
  run("screencapture", ["-x", path], { capture: true });
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

function rssSnapshot() {
  const ps = output("ps", ["-axo", "pid,ppid,rss,command"]);
  const lines = ps
    .split(/\r?\n/)
    .filter((line) => /fleet-host|fleet-reporter|code serve-web|Code Helper|WebKit/i.test(line));
  const rssKiB = lines.reduce((sum, line) => {
    const parts = line.trim().split(/\s+/);
    const rss = Number(parts[2]);
    return sum + (Number.isFinite(rss) ? rss : 0);
  }, 0);
  return {
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

async function main() {
  if (process.platform !== "darwin") {
    throw new Error("host keepalive probe currently requires macOS for Tauri window screenshots");
  }

  const opts = parseArgs(process.argv.slice(2));
  const outDir = opts.out;
  const shotDir = resolve(outDir, "screenshots");
  mkdirSync(shotDir, { recursive: true });

  if (!opts.allowBusyPorts) {
    const busy = [51777, 51778].filter(portInUse);
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
    run("cargo", ["build"], { cwd: HOST_DIR });
    run("cargo", ["build", "-p", "fleet-reporter"], { cwd: ROOT });
  }

  const hostBin = resolve(HOST_DIR, "target/debug/fleet-host");
  const reporterBin = resolve(ROOT, "target/debug/fleet-reporter");
  const bridgeVsix = resolve(ROOT, "packages/fleet-bridge/fleet-bridge-0.2.0.vsix");
  if (!existsSync(hostBin)) throw new Error(`missing fleet-host binary: ${hostBin}`);
  if (!existsSync(reporterBin)) throw new Error(`missing fleet-reporter binary: ${reporterBin}`);
  if (!existsSync(bridgeVsix)) throw new Error(`missing fleet-bridge VSIX: ${bridgeVsix}`);

  const logPath = resolve(outDir, "fleet-host.log");
  const logFd = openSync(logPath, "w");
  const startedAt = new Date().toISOString();
  const child = spawn(hostBin, [], {
    cwd: HOST_DIR,
    env: {
      ...process.env,
      FLEET_AUTOSPAWN: String(opts.autospawn),
      FLEET_EDITOR_KEEPALIVE: "1",
      FLEET_REPORTER_BIN: reporterBin,
      FLEET_BRIDGE_VSIX: bridgeVsix,
      RUST_LOG: process.env.RUST_LOG || "info",
    },
    stdio: ["ignore", logFd, logFd],
  });

  let report;
  try {
    child.on("exit", (code, signal) => {
      console.error(`[probe] fleet-host exited code=${code} signal=${signal}`);
    });

    const screenshots = [];
    const take = async (file) => {
      const abs = resolve(shotDir, file);
      capture(abs);
      screenshots.push(relative(outDir, abs));
      await sleep(250);
    };

    const window = await waitForWindow(20000);
    await waitForLog(logPath, /server registered \(phone-home\)/g, 2, opts.settleMs);
    await sleep(Math.max(0, opts.settleMs - 2000));
    await take("01-initial-selected.png");

    const rowX = window.x + 80;
    const row1Y = window.y + 88;
    const row2Y = window.y + 132;

    clickAt(rowX, row1Y);
    await sleep(opts.switchDelayMs);
    await take("02-server-1-selected.png");

    clickAt(rowX, row2Y);
    await sleep(opts.switchDelayMs);
    await take("03-server-2-selected.png");

    clickAt(rowX, row1Y);
    await sleep(opts.switchDelayMs);
    await take("04-server-1-returned.png");

    const rss = rssSnapshot();
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
    };

    const pass =
      evidence.registeredCount >= 2 &&
      evidence.editorCreatedCount >= 2 &&
      evidence.editorCreateFailures === 0 &&
      evidence.deregisteredCount === 0 &&
      screenshots.length === 4;

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
            ? `registered=${evidence.registeredCount}, editors=${evidence.editorCreatedCount}, deregistered=${evidence.deregisteredCount}, rss=${rss.rssMiB} MiB`
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
          },
          screenshots: [],
        },
      ],
    };
    process.exitCode = 1;
  } finally {
    if (!opts.keep) await terminate(child);
    closeSync(logFd);
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

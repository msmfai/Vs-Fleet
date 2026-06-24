#!/usr/bin/env node
// Cross-platform release smoke test for the Fleet host binary.
//
// What it proves (per the release plan): the built binary actually LAUNCHES on
// this OS, accepts one local VS Code-web-style session over the phone-home
// bridge, and shows it as a rail tab — not merely that it compiled.
//
//   1. launch the freshly built fleet-host with an isolated runtime dir and the
//      probe control endpoint enabled (FLEET_PROBE_CONTROL_PORT);
//   2. wait for /healthz — the Tauri window + event loop are up;
//   3. phone home one session over the bridge websocket (ws://127.0.0.1:51778)
//      exactly like the fleet-bridge VS Code extension does: a `hello` frame
//      carrying the launch token read from <runtime>/bridge.token. The session
//      URL points at a local stand-in page served by this script;
//   4. assert the session appears in /servers (the rail tab) and that
//      /select/<id> makes it the selected tab;
//   5. capture a screenshot artifact (best-effort; the protocol assertions are
//      the pass/fail signal) and write a JSON report.
//
// Usage: node scripts/release-smoke.mjs --bin <path-to-fleet-host> [--out <dir>]
// Requires Node >= 22 (global WebSocket client). No npm dependencies.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

const PROBE_PORT = 51776;
const BRIDGE_PORT = 51778;
const SERVER_ID = "smoke-session-1";
const SERVER_LABEL = "smoke session";

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}

const bin = arg("--bin");
const outDir = path.resolve(arg("--out", "smoke-out"));
if (!bin || !fs.existsSync(bin)) {
  console.error(`usage: release-smoke.mjs --bin <fleet-host binary> [--out <dir>]`);
  console.error(`binary not found: ${bin}`);
  process.exit(2);
}
fs.mkdirSync(outDir, { recursive: true });

const runtimeDir = fs.mkdtempSync(path.join(os.tmpdir(), "fleet-smoke-run-"));
const muxDir = fs.mkdtempSync(path.join(os.tmpdir(), "fleet-smoke-mux-"));
const logPath = path.join(outDir, "fleet-host.log");
const log = fs.createWriteStream(logPath);

const report = {
  platform: `${process.platform}-${process.arch}`,
  bin: path.resolve(bin),
  steps: {},
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function poll(label, timeoutMs, fn) {
  const deadline = Date.now() + timeoutMs;
  let lastErr;
  while (Date.now() < deadline) {
    try {
      const v = await fn();
      if (v !== undefined && v !== false) {
        report.steps[label] = "ok";
        return v;
      }
    } catch (e) {
      lastErr = e;
    }
    await sleep(250);
  }
  report.steps[label] = `timeout after ${timeoutMs}ms${lastErr ? `: ${lastErr}` : ""}`;
  throw new Error(`${label}: ${report.steps[label]}`);
}

async function probe(pathname) {
  const res = await fetch(`http://127.0.0.1:${PROBE_PORT}${pathname}`);
  if (!res.ok) throw new Error(`${pathname} -> HTTP ${res.status}`);
  return res.json();
}

// ── 1. a stand-in "editor" page for the session URL the rail tab embeds ──────
const editor = http.createServer((_req, res) => {
  res.setHeader("content-type", "text/html");
  res.end("<html><body style='background:#1e1e2e;color:#eee;font:24px sans-serif'><h1>fleet smoke editor stand-in</h1></body></html>");
});
await new Promise((r) => editor.listen(0, "127.0.0.1", r));
const editorUrl = `http://127.0.0.1:${editor.address().port}/`;

// ── 2. launch the host ───────────────────────────────────────────────────────
const child = spawn(bin, [], {
  env: {
    ...process.env,
    FLEET_RUNTIME_DIR: runtimeDir,
    FLEET_MUX_DIR: muxDir,
    FLEET_PROBE_CONTROL_PORT: String(PROBE_PORT),
    RUST_LOG: process.env.RUST_LOG ?? "info",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
child.stdout.pipe(log);
child.stderr.pipe(log);
let exited = null;
child.on("exit", (code, signal) => {
  exited = { code, signal };
});

let ws;
let failed = false;
try {
  // ── 3. the app is up: window created, event loop running ──────────────────
  await poll("healthz", 120_000, async () => (await probe("/healthz")).ok === true);

  // ── 4. phone home one session, like the fleet-bridge extension does ───────
  const tokenPath = path.join(runtimeDir, "bridge.token");
  const token = (
    await poll("bridge-token", 30_000, () =>
      fs.existsSync(tokenPath) ? fs.readFileSync(tokenPath, "utf8").trim() : false
    )
  ).toString();

  // The bridge listener binds asynchronously after startup — retry the connect.
  ws = await poll("bridge-connect", 60_000, async () => {
    const candidate = new WebSocket(`ws://127.0.0.1:${BRIDGE_PORT}`);
    return new Promise((resolve, reject) => {
      candidate.addEventListener("open", () => resolve(candidate), { once: true });
      candidate.addEventListener("error", (e) => reject(new Error(`bridge ws: ${e.message ?? e}`)), { once: true });
    });
  });
  ws.send(
    JSON.stringify({
      type: "hello",
      server_id: SERVER_ID,
      url: editorUrl,
      label: SERVER_LABEL,
      token,
    })
  );
  report.steps["bridge-hello"] = "ok";

  // ── 5. the tab appears and can be selected ─────────────────────────────────
  await poll("tab-appears", 45_000, async () => {
    const { servers } = await probe("/servers");
    return Array.isArray(servers) && servers.some((s) => s.id === SERVER_ID);
  });
  await probe(`/select/${SERVER_ID}`);
  await poll("tab-selected", 15_000, async () => (await probe("/selected")).selected === SERVER_ID);

  // ── 6. rename the session over the probe, then assert it sticks ────────────
  // Drive the State-mutating rename command and assert the rail tab shows the new
  // label — the live-window analogue of `apply_state_probe_command` (Layer E).
  const RENAMED_LABEL = "Renamed Session";
  const renameRes = await probe(`/rename/${SERVER_ID}?label=${encodeURIComponent(RENAMED_LABEL)}`);
  if (renameRes.renamed !== true) {
    throw new Error(`rename did not take: ${JSON.stringify(renameRes)}`);
  }
  await poll("tab-renamed", 15_000, async () => {
    const { servers } = await probe("/servers");
    return (
      Array.isArray(servers) &&
      servers.some((s) => s.id === SERVER_ID && s.label === RENAMED_LABEL)
    );
  });

  // Regression-lock: a reporter re-register (a FRESH phone-home, with the AUTO
  // label) must NOT clobber the user rename. The bridge only reads the first
  // hello per connection, so this opens a NEW socket — exactly how a reconnecting
  // reporter re-registers the same id — then asserts the renamed label still wins.
  const ws2 = await poll("bridge-reconnect", 30_000, async () => {
    const candidate = new WebSocket(`ws://127.0.0.1:${BRIDGE_PORT}`);
    return new Promise((resolve, reject) => {
      candidate.addEventListener("open", () => resolve(candidate), { once: true });
      candidate.addEventListener("error", (e) => reject(new Error(`bridge ws: ${e.message ?? e}`)), { once: true });
    });
  });
  ws2.send(
    JSON.stringify({
      type: "hello",
      server_id: SERVER_ID,
      url: editorUrl,
      label: SERVER_LABEL,
      token,
    })
  );
  await poll("rename-survives-reregister", 15_000, async () => {
    const { servers } = await probe("/servers");
    return (
      Array.isArray(servers) &&
      servers.some((s) => s.id === SERVER_ID && s.label === RENAMED_LABEL)
    );
  });
  try {
    ws2.close();
  } catch {}

  // Give the child webview a moment to load the stand-in page, then screenshot.
  await sleep(5_000);
  screenshot(path.join(outDir, `smoke-${process.platform}-${process.arch}.png`));

  if (exited) throw new Error(`fleet-host exited prematurely: ${JSON.stringify(exited)}`);
  report.ok = true;
  console.log(`SMOKE OK on ${report.platform}: session registered, tab appeared + selected.`);
} catch (e) {
  failed = true;
  report.ok = false;
  report.error = String(e);
  console.error(`SMOKE FAILED on ${report.platform}: ${e}`);
  try {
    const tail = fs.readFileSync(logPath, "utf8").split("\n").slice(-100).join("\n");
    console.error(`--- fleet-host.log (tail) ---\n${tail}`);
  } catch {}
} finally {
  try {
    ws?.close();
  } catch {}
  editor.close();
  child.kill("SIGTERM");
  await sleep(1_500);
  child.kill("SIGKILL");
  fs.writeFileSync(path.join(outDir, "smoke-report.json"), JSON.stringify(report, null, 2));
}
process.exit(failed ? 1 : 0);

/// Best-effort full-display screenshot (the assertions above are the gate; the
/// image is evidence). Window-targeted capture is deliberately avoided so the
/// test never depends on the Fleet window being focused.
function screenshot(out) {
  try {
    let r;
    if (process.platform === "darwin") {
      r = spawnSync("screencapture", ["-x", out], { timeout: 30_000 });
    } else if (process.platform === "linux") {
      r = spawnSync("import", ["-window", "root", out], { timeout: 30_000 });
    } else if (process.platform === "win32") {
      const ps = `
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('${out.replace(/\\/g, "\\\\")}', [System.Drawing.Imaging.ImageFormat]::Png)
`;
      r = spawnSync("powershell", ["-NoProfile", "-Command", ps], { timeout: 60_000 });
    }
    report.steps.screenshot = r && r.status === 0 && fs.existsSync(out) ? "ok" : `skipped (${r?.status})`;
  } catch (e) {
    report.steps.screenshot = `skipped (${e})`;
  }
}

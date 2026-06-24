// WebdriverIO config for Layer D — the real-UI E2E of the Fleet Tauri host.
//
// Structure mirrors the official Tauri "WebDriver / WebdriverIO" example:
//   - `onPrepare` spawns `tauri-driver`, which wraps the platform WebDriver
//     (WebKitWebGTKDriver on Linux) and exposes it on 127.0.0.1:4444;
//   - capabilities carry `tauri:options.application` (the built fleet-host binary)
//     and `tauri:options.env` (the bridge/probe/runtime env the app needs);
//   - `onComplete` kills tauri-driver.
//
// The app is launched in its FLEET_E2E_RAIL_ONLY composition (rail = the single
// top-level webview) so the WebDriver session can reach the rail's real DOM; the
// production layout puts the rail in an add_child child webview that tauri-driver
// cannot target. See crates/fleet-host/src/mux.rs::build_window_rail_only.
//
// We additionally stand up a tiny "editor stand-in" HTTP server here (the URL the
// phoned-home session points at), exactly like scripts/release-smoke.mjs, in an
// isolated runtime dir so the bridge token is sandboxed per run.
//
// LAUNCHER → WORKER HANDOFF (important): `onPrepare` runs in the wdio LAUNCHER
// process, but the spec's `before all` hook runs in a separate WORKER process.
// Anything stashed on `globalThis`/module state in onPrepare is therefore invisible
// to the spec. We hand off via a FILE written in onPrepare at a path BOTH processes
// derive identically; the spec READS it inside its hook (worker-safe). All values
// are fixed/derivable, so this is deterministic across the process boundary.
//
// CI/Linux only. Do not run locally.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const hostRoot = path.resolve(__dirname, "..");

// The built binary under test. Overridable so CI can point at the release build.
const BINARY = process.env.FLEET_HOST_BIN
  ? path.resolve(process.env.FLEET_HOST_BIN)
  : path.resolve(hostRoot, "target/release/fleet-host");

// Fixed loopback ports. BRIDGE_PORT is hard-coded in the host (mux::BRIDGE_PORT /
// main::BRIDGE_PORT = 51778); the probe control port is chosen by us. The editor
// stand-in uses a fixed port too so the launcher and worker (separate processes)
// agree without IPC.
const BRIDGE_PORT = 51778;
const PROBE_PORT = 51776;
const EDITOR_PORT = 51779;
const EDITOR_URL = `http://127.0.0.1:${EDITOR_PORT}/`;

// Isolated runtime dir → sandboxes <runtime>/bridge.token + hub state. Derived
// DETERMINISTICALLY (a fixed path under tmp, NOT mkdtemp) so the launcher, the
// worker, and the app's FLEET_RUNTIME_DIR all resolve the SAME dir without sharing
// in-memory state. (A per-process mkdtemp would give the worker a different dir
// than the one the app wrote bridge.token into.) Cleaned fresh in onPrepare.
const runtimeDir = path.join(os.tmpdir(), "fleet-e2e-run");

// The launcher→worker handoff file. Both processes compute this path identically
// from `runtimeDir`; onPrepare writes it, the spec reads it.
const configPath = path.join(runtimeDir, "e2e-config.json");

// Shared config both processes derive from the same constants/paths. Written to
// `configPath` for the worker; the spec reads the file (never module/global state).
const sharedConfig = {
  bridgePort: BRIDGE_PORT,
  probePort: PROBE_PORT,
  runtimeDir,
  editorUrl: EDITOR_URL,
  configPath,
};

let tauriDriver;
let editorServer;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  // tauri-driver does not implement the /status endpoint WebdriverIO polls by
  // default, so disable that readiness probe.
  path: "/",

  specs: ["./test/**/*.e2e.js"],
  maxInstances: 1,

  capabilities: [
    {
      // tauri-driver reads these to launch the app under the native WebDriver.
      "tauri:options": {
        application: BINARY,
        env: {
          // Build the rail as the single top-level webview (WebDriver-reachable).
          FLEET_E2E_RAIL_ONLY: "1",
          // Isolate runtime state + enable the probe control port (handshake/poke).
          FLEET_RUNTIME_DIR: runtimeDir,
          FLEET_PROBE_CONTROL_PORT: String(PROBE_PORT),
          RUST_LOG: process.env.RUST_LOG ?? "info",
          // WebKitGTK under Xvfb has no GPU: force software GL + disable compositing
          // to quiet the `libEGL/DRI3` warnings. The spec is DOM-driven and does NOT
          // depend on rendering, so this is noise reduction, not a correctness dep.
          LIBGL_ALWAYS_SOFTWARE: "1",
          WEBKIT_DISABLE_COMPOSITING_MODE: "1",
        },
      },
    },
  ],

  reporters: ["spec"],
  framework: "mocha",
  mochaOpts: {
    ui: "bdd",
    // Generous: includes the build-up to the Tauri window + bridge bind.
    timeout: 180000,
  },

  logLevel: "info",
  bail: 0,
  waitforTimeout: 20000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  // ── start the editor stand-in + tauri-driver before the session opens ───────
  onPrepare: async () => {
    if (!fs.existsSync(BINARY)) {
      throw new Error(
        `fleet-host binary not found at ${BINARY}. Build it first ` +
          `(cargo build --release) or set FLEET_HOST_BIN.`
      );
    }

    // Start each run from a clean runtime dir so a stale bridge.token from a
    // previous run can't be read before the app rewrites it.
    fs.rmSync(runtimeDir, { recursive: true, force: true });
    fs.mkdirSync(runtimeDir, { recursive: true });

    // The page the phoned-home session URL embeds. Harmless static HTML; in
    // rail-only mode nothing actually navigates to it, but the rail row carries
    // the URL and the bridge requires a well-formed one. Started in the LAUNCHER
    // and kept alive for the whole run, on a FIXED loopback port, so the worker
    // reaches it over 127.0.0.1 without sharing the server handle.
    editorServer = http.createServer((_req, res) => {
      res.setHeader("content-type", "text/html");
      res.end(
        "<!doctype html><html><body style='background:#1e1e2e;color:#eee'>" +
          "<h1>fleet e2e editor stand-in</h1></body></html>"
      );
    });
    await new Promise((resolve) =>
      editorServer.listen(EDITOR_PORT, "127.0.0.1", resolve)
    );

    // Hand off to the worker via a FILE (worker is a separate process; globalThis
    // does not cross). The spec reads `configPath`, which it derives identically.
    fs.writeFileSync(configPath, JSON.stringify(sharedConfig, null, 2));

    // Belt-and-suspenders for the app env: tauri-driver's `tauri:options.env`
    // propagation is version-dependent, and the previous run's failure ("bridge
    // token never appeared at <runtimeDir>/bridge.token") shows the app didn't
    // get FLEET_RUNTIME_DIR — so it wrote the token to its default dir. Set the
    // env on THIS launcher process before spawning tauri-driver below; tauri-driver
    // (and the app it launches as a descendant) inherit it. This is exactly how the
    // working release-smoke.mjs hands FLEET_RUNTIME_DIR to the binary.
    process.env.FLEET_E2E_RAIL_ONLY = "1";
    process.env.FLEET_RUNTIME_DIR = runtimeDir;
    process.env.FLEET_PROBE_CONTROL_PORT = String(PROBE_PORT);

    tauriDriver = spawn(tauriDriverBin(), [], {
      stdio: [null, process.stdout, process.stderr],
    });
    tauriDriver.on("error", (e) => {
      console.error("tauri-driver failed to start:", e);
      process.exit(1);
    });
  },

  onComplete: () => {
    if (editorServer) {
      try {
        editorServer.close();
      } catch {}
    }
    if (tauriDriver) {
      try {
        tauriDriver.kill();
      } catch {}
    }
  },
};

// Resolve the tauri-driver executable: prefer PATH, fall back to ~/.cargo/bin
// (where `cargo install tauri-driver` puts it on CI).
function tauriDriverBin() {
  const onPath = spawnSync(process.platform === "win32" ? "where" : "which", [
    "tauri-driver",
  ]);
  if (onPath.status === 0) return "tauri-driver";
  return path.resolve(os.homedir(), ".cargo", "bin", "tauri-driver");
}

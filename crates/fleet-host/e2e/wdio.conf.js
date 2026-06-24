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
// phoned-home session points at), exactly like scripts/release-smoke.mjs, and a
// fresh isolated runtime dir so the bridge token is sandboxed per run. These are
// published on `globalThis.__FLEET_E2E__` for the spec.
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

// Per-run isolated runtime dir → sandboxes <runtime>/bridge.token + hub state.
// Derived from an env var so the launcher and worker processes (which load this
// module separately) resolve the SAME dir; the launcher seeds it if unset.
if (!process.env.FLEET_E2E_RUNTIME_DIR) {
  process.env.FLEET_E2E_RUNTIME_DIR = fs.mkdtempSync(
    path.join(os.tmpdir(), "fleet-e2e-run-")
  );
}
const runtimeDir = process.env.FLEET_E2E_RUNTIME_DIR;

// Shared config both processes can rebuild from constants/env alone.
const sharedConfig = {
  bridgePort: BRIDGE_PORT,
  probePort: PROBE_PORT,
  runtimeDir,
  editorUrl: EDITOR_URL,
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

    // The page the phoned-home session URL embeds. Harmless static HTML; in
    // rail-only mode nothing actually navigates to it, but the rail row carries
    // the URL and the bridge requires a well-formed one.
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

    // Hand shared config to anything sharing this process.
    globalThis.__FLEET_E2E__ = sharedConfig;

    tauriDriver = spawn(tauriDriverBin(), [], {
      stdio: [null, process.stdout, process.stderr],
    });
    tauriDriver.on("error", (e) => {
      console.error("tauri-driver failed to start:", e);
      process.exit(1);
    });
  },

  // Re-publish the shared config into the worker process (onPrepare runs in the
  // launcher; before/specs run in the worker, a separate process). Both rebuild
  // `sharedConfig` from the same constants + inherited FLEET_E2E_RUNTIME_DIR, so
  // the values match across processes.
  before: () => {
    globalThis.__FLEET_E2E__ = sharedConfig;
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

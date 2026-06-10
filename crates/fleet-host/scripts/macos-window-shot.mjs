#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "../../..");
const HELPER_SOURCE = resolve(__dirname, "macos-window-info.m");
const DEFAULT_TOOL_DIR = resolve(ROOT, "target", "fleet-window-tools");

function run(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, {
    cwd: opts.cwd || __dirname,
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

export function ensureWindowInfoHelper(toolDir) {
  const dir = resolve(toolDir);
  mkdirSync(dir, { recursive: true });
  const helper = resolve(dir, "macos-window-info");

  const sourceMtime = statSync(HELPER_SOURCE).mtimeMs;
  const helperFresh = existsSync(helper) && statSync(helper).mtimeMs >= sourceMtime;
  if (helperFresh) return helper;

  run(
    "clang",
    [
      "-fobjc-arc",
      "-framework",
      "Foundation",
      "-framework",
      "CoreGraphics",
      HELPER_SOURCE,
      "-o",
      helper,
    ],
    { capture: true },
  );
  return helper;
}

export function listMacWindows({ owner = "", toolDir = DEFAULT_TOOL_DIR } = {}) {
  if (process.platform !== "darwin") {
    throw new Error("macOS window capture requires darwin");
  }
  const helper = ensureWindowInfoHelper(toolDir);
  const raw = run(helper, owner ? [owner] : [], { capture: true });
  return JSON.parse(raw);
}

function area(window) {
  const bounds = window.bounds || {};
  return Number(bounds.Width || 0) * Number(bounds.Height || 0);
}

export function findFleetWindow({ owner = "Fleet", toolDir, minArea = 120000 } = {}) {
  const owners = Array.isArray(owner)
    ? owner.filter(Boolean)
    : [owner].filter(Boolean);
  const windows = owners.length === 1
    ? listMacWindows({ owner: owners[0], toolDir })
    : listMacWindows({ toolDir });
  const ownerSet = new Set(owners);
  const candidates = windows
    .filter((window) => !ownerSet.size || ownerSet.has(window.owner))
    .filter((window) => Number(window.layer) === 0)
    .filter((window) => Number(window.onscreen) === 1)
    .filter((window) => area(window) > minArea)
    .sort((a, b) => area(b) - area(a));

  if (!candidates.length) {
    const summary = windows
      .slice(0, 8)
      .map((window) => {
        const bounds = window.bounds || {};
        return `${window.id}:${window.owner}:${window.name}:${window.layer}:${bounds.Width || 0}x${bounds.Height || 0}`;
      })
      .join(", ");
    const ownerLabel = owners.length ? owners.join("|") : "<any>";
    throw new Error(`Fleet window not found for owner ${ownerLabel}; windows: ${summary}`);
  }

  return candidates[0];
}

export function captureMacWindow({
  out,
  window,
} = {}) {
  if (!out) throw new Error("captureMacWindow requires an output path");
  if (!window?.id) throw new Error("captureMacWindow requires a CoreGraphics window");
  mkdirSync(dirname(resolve(out)), { recursive: true });
  run("screencapture", ["-x", "-l", String(window.id), resolve(out)], { capture: true });
  return {
    path: resolve(out),
    window,
    command: `screencapture -x -l ${window.id} ${resolve(out)}`,
  };
}

export function captureFleetWindow({
  out,
  owner = "Fleet",
  toolDir = DEFAULT_TOOL_DIR,
} = {}) {
  if (!out) throw new Error("captureFleetWindow requires an output path");
  const window = findFleetWindow({ owner, toolDir });
  return captureMacWindow({ out, window });
}

function parseArgs(argv) {
  const opts = {
    owner: "Fleet",
    out: "",
    toolDir: "",
    json: false,
    list: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = () => argv[++i];
    if (arg === "--owner") opts.owner = next();
    else if (arg === "--out") opts.out = next();
    else if (arg === "--tool-dir") opts.toolDir = next();
    else if (arg === "--json") opts.json = true;
    else if (arg === "--list") opts.list = true;
    else if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return opts;
}

function usage() {
  console.log(`usage: node crates/fleet-host/scripts/macos-window-shot.mjs [options]

Options:
  --out PATH      Capture the Fleet window to this PNG path.
  --owner NAME    Window owner name. Default: Fleet.
  --tool-dir DIR  Directory for the compiled CoreGraphics helper.
  --list          Print matching windows as JSON instead of capturing.
  --json          Print capture metadata as JSON.
`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const toolDir = opts.toolDir || DEFAULT_TOOL_DIR;

  if (opts.list || !opts.out) {
    const windows = listMacWindows({ owner: opts.owner, toolDir });
    console.log(JSON.stringify(windows, null, 2));
    return;
  }

  const result = captureFleetWindow({
    out: opts.out,
    owner: opts.owner,
    toolDir,
  });
  if (opts.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log(`[fleet-window-shot] ${result.command}`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    console.error(`[fleet-window-shot] ${err?.stack || err}`);
    process.exit(1);
  });
}

// Machine-state probes — the before/after side-effect signal (§3.2 MachineState).
//
// Track D (observation depth): the original Track-A stub measured procs + docker
// stats mem/cpu. This expands it — ADDITIVELY, signature unchanged — with:
//   • disk usage (container writable layer + project dir size)
//   • network I/O (rx/tx bytes from docker stats)
//   • fs-change count via `docker diff`
//   • per-action latency helpers (timeit / aTimeit)
// machineState(name) still returns {procs, mem, cpu}; every new field is optional
// so existing callers (env.mjs, run.mjs) and the §3.2 contract keep working.

import { execSync } from "node:child_process";

const sh = (cmd) => {
  try { return execSync(cmd, { encoding: "utf8" }).trim(); } catch { return ""; }
};

/**
 * Snapshot the container's machine state. Backward-compatible: {procs, mem, cpu}
 * are always present; Track-D fields ({rxBytes, txBytes, fsChanges, diskBytes,
 * projectBytes}) are added when cheaply obtainable (best-effort, never throws).
 *
 * @param {string} name   docker container name
 * @param {{ deep?: boolean }} [opts]  deep=true also sizes the project dir (slower)
 * @returns {import("../behaviours/_contract.mjs").MachineState}
 */
export function machineState(name, opts = {}) {
  // One docker-stats call carries cpu, mem AND net I/O — fetch them together so we
  // don't pay the (~1s) --no-stream cost twice.
  const stats = sh(
    `docker stats --no-stream --format '{{.CPUPerc}}|{{.MemUsage}}|{{.NetIO}}' ${name}`
  );
  const [cpu, mem, netio] = (stats || "||").split("|");
  const { rxBytes, txBytes } = parseNetIO(netio);

  const state = {
    cpu: cpu || "n/a",
    mem: mem || "n/a",
    procs: parseInt(sh(`docker exec ${name} sh -c 'ps -e | wc -l'`) || "-1", 10),
  };

  // ─── Track-D additive fields (best-effort; absent if the probe fails) ───
  if (rxBytes != null) state.rxBytes = rxBytes;
  if (txBytes != null) state.txBytes = txBytes;

  const fsChanges = fsChangeCount(name);
  if (fsChanges != null) state.fsChanges = fsChanges;

  const diskBytes = containerDiskBytes(name);
  if (diskBytes != null) state.diskBytes = diskBytes;

  if (opts.deep) {
    const pb = projectBytes(name);
    if (pb != null) state.projectBytes = pb;
  }

  return state;
}

// ─── fs-change count: how many paths the running container changed/added/deleted
// relative to its image layer. `docker diff` prints one line per path (A/C/D <p>).
/** @returns {number|null} */
export function fsChangeCount(name) {
  const out = sh(`docker diff ${name}`);
  if (out === "") return 0; // docker diff succeeded but nothing changed
  // Distinguish "no output" (0) from "command failed" (sh returns "" on throw).
  // Re-run with a sentinel so we know it ran: count non-empty lines.
  const n = out.split("\n").filter((l) => l.trim().length).length;
  return Number.isFinite(n) ? n : null;
}

// A breakdown of the fs changes by kind — useful evidence for file-write behaviours.
// Returns {added, changed, deleted, paths:[{kind,path}]} or null on failure.
export function fsDiff(name) {
  const out = sh(`docker diff ${name}`);
  if (out === "") return { added: 0, changed: 0, deleted: 0, paths: [] };
  const paths = [];
  let added = 0, changed = 0, deleted = 0;
  for (const line of out.split("\n")) {
    const m = line.match(/^([ACD])\s+(.*)$/);
    if (!m) continue;
    const kind = m[1] === "A" ? "added" : m[1] === "C" ? "changed" : "deleted";
    if (kind === "added") added++; else if (kind === "changed") changed++; else deleted++;
    paths.push({ kind, path: m[2] });
  }
  return { added, changed, deleted, paths };
}

// ─── disk: size of the container's writable layer (RootFS SizeRw). Cheap; reads
// docker inspect, no exec into the container.
/** @returns {number|null} bytes, or null */
export function containerDiskBytes(name) {
  const raw = sh(`docker inspect --size --format '{{.SizeRw}}' ${name}`);
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

// ─── project dir size (bytes) inside the container. Slower (du over the tree), so
// it's opt-in via machineState(name,{deep:true}) or called directly by behaviours.
/** @returns {number|null} bytes */
export function projectBytes(name, dir = "/home/coder/project") {
  const raw = sh(`docker exec ${name} sh -lc ${JSON.stringify(`du -sb ${dir} 2>/dev/null | cut -f1`)}`);
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

// ─── network: parse docker stats NetIO ("1.2kB / 800B") into rx/tx bytes.
/** @returns {{rxBytes:number|null, txBytes:number|null}} */
function parseNetIO(netio) {
  if (!netio || !netio.includes("/")) return { rxBytes: null, txBytes: null };
  const [rx, tx] = netio.split("/").map((s) => s.trim());
  return { rxBytes: parseBytes(rx), txBytes: parseBytes(tx) };
}

// "1.2kB" / "800B" / "3.4MB" / "1GiB" → bytes. docker uses SI (kB/MB/GB) for net.
function parseBytes(str) {
  if (!str) return null;
  const m = str.match(/([\d.]+)\s*([KMGT]?i?B)?/i);
  if (!m) return null;
  const v = parseFloat(m[1]);
  if (!Number.isFinite(v)) return null;
  const unit = (m[2] || "B").toLowerCase();
  const bin = unit.includes("i"); // KiB/MiB → 1024, kB/MB → 1000
  const base = bin ? 1024 : 1000;
  if (unit.startsWith("k")) return Math.round(v * base);
  if (unit.startsWith("m")) return Math.round(v * base ** 2);
  if (unit.startsWith("g")) return Math.round(v * base ** 3);
  if (unit.startsWith("t")) return Math.round(v * base ** 4);
  return Math.round(v); // bytes
}

// ─── Compact human-readable Δ between two MachineStates. Backward-compatible:
// always emits `procs`; emits memMiB/fsChanges/diskMiB/netBytes only when both
// snapshots carry the inputs (so old {procs,mem,cpu}-only states still work).
export function machineDelta(before, after) {
  const delta = { procs: `${before.procs}→${after.procs}` };

  const memMiB = parseMemDeltaMiB(before.mem, after.mem);
  if (memMiB != null) delta.memMiB = memMiB;

  // fsChanges: report the AFTER absolute count (it's a count vs the image, not a
  // before/after delta — that's what the §3.5 schema example shows: "fsChanges":7).
  if (after.fsChanges != null) delta.fsChanges = after.fsChanges;

  if (before.diskBytes != null && after.diskBytes != null) {
    delta.diskMiB = round1((after.diskBytes - before.diskBytes) / (1024 * 1024));
  }

  if (before.rxBytes != null && after.rxBytes != null) {
    const rx = after.rxBytes - before.rxBytes;
    const tx = (after.txBytes ?? 0) - (before.txBytes ?? 0);
    delta.netBytes = { rx, tx };
  }

  if (before.projectBytes != null && after.projectBytes != null) {
    delta.projectBytes = after.projectBytes - before.projectBytes;
  }

  return delta;
}

// ─── Per-action latency helpers ────────────────────────────────────────────────
// Wrap a sync or async unit of work and return {result, ms}. The runner already
// times the whole behaviour (timingsMs.effect); these let a behaviour time a
// specific sub-step (e.g. act-vs-effect) and stash it in evidence.

/** Time a synchronous fn. @returns {{result:any, ms:number}} */
export function timeit(fn) {
  const t0 = now();
  const result = fn();
  return { result, ms: round1(now() - t0) };
}

/** Time an async fn. @returns {Promise<{result:any, ms:number}>} */
export async function aTimeit(fn) {
  const t0 = now();
  const result = await fn();
  return { result, ms: round1(now() - t0) };
}

// A reusable stopwatch: const t = stopwatch(); … t() → elapsed ms.
export function stopwatch() {
  const t0 = now();
  return () => round1(now() - t0);
}

function now() {
  // Prefer the monotonic high-res clock; fall back to Date.now under odd runtimes.
  return typeof performance !== "undefined" && performance.now ? performance.now() : Date.now();
}

function parseMemDeltaMiB(beforeMem, afterMem) {
  const b = parseUsedMiB(beforeMem), a = parseUsedMiB(afterMem);
  if (b == null || a == null) return null;
  return Math.round(a - b);
}

// "123.4MiB / 512MiB" → 123.4 (in MiB). Handles KiB/MiB/GiB (binary, as docker
// reports container mem usage).
function parseUsedMiB(memStr) {
  if (!memStr) return null;
  const used = memStr.split("/")[0]?.trim();
  const m = used?.match(/([\d.]+)\s*([KMG]i?B)/i);
  if (!m) return null;
  const v = parseFloat(m[1]);
  const unit = m[2].toLowerCase();
  if (unit.startsWith("k")) return v / 1024;
  if (unit.startsWith("g")) return v * 1024;
  return v; // MiB
}

function round1(n) { return Math.round(n * 10) / 10; }

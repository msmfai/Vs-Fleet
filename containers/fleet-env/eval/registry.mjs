// Registry — auto-discovers behaviours and scenarios by globbing their dirs. Each
// module exports an array (`export const behaviours = [...]` / `export const
// scenarios = [...]`). Files starting with '_' (contracts, helpers) are ignored.
//
// This is the only "central" file behaviour/scenario authors touch — and they DON'T:
// they just drop a new `behaviours/foo.mjs` or `scenarios/bar.mjs` and it appears.

import { readdirSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, join, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// The last git commit + date that touched a test's source file. AUTO-DERIVED so a
// test's "last updated" provenance can never go stale — when a test breaks after a
// refactor, the report shows exactly when its definition last changed. Per-file
// (tests are grouped by file); cached.
const _gitCache = new Map();
export function gitInfo(file) {
  if (!file) return { commit: "?", date: "?", file: "?" };
  if (_gitCache.has(file)) return _gitCache.get(file);
  let info = { commit: "uncommitted", date: "—", file: relative(join(HERE, "../../.."), file) };
  try {
    const out = execSync(`git log -1 --format=%h|%cs -- "${file}"`, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    if (out) {
      const [commit, date] = out.split("|");
      info = { commit, date, file: info.file };
    }
  } catch {
    /* not a git repo / git absent — leave "uncommitted" */
  }
  _gitCache.set(file, info);
  return info;
}

function mjsFiles(dir) {
  let names;
  try { names = readdirSync(dir); } catch { return []; }
  return names
    .filter((n) => n.endsWith(".mjs") && !n.startsWith("_"))
    .sort()
    .map((n) => join(dir, n));
}

// Import every module in `dir`, flattening its named export `<key>` (an array).
async function loadAll(dir, key) {
  const out = [];
  for (const file of mjsFiles(dir)) {
    let mod;
    try {
      mod = await import(pathToFileURL(file).href);
    } catch (e) {
      console.warn(`[registry] failed to import ${file}: ${e.message}`);
      continue;
    }
    const arr = mod[key] || mod.default;
    if (!Array.isArray(arr)) {
      console.warn(`[registry] ${file} has no '${key}' array export — skipped`);
      continue;
    }
    // Tag each test with its source file so the harness can resolve its git
    // provenance (gitInfo) and surface it in --list + the report.
    for (const item of arr) { if (item && typeof item === "object") item.__file = file; out.push(item); }
  }
  return out;
}

export async function loadRegistry() {
  const behaviours = await loadAll(join(HERE, "behaviours"), "behaviours");
  const scenarios = await loadAll(join(HERE, "scenarios"), "scenarios");

  // Dedup-by-id guard (helps catch copy-paste collisions across files).
  dedupeWarn(behaviours, "behaviour");
  dedupeWarn(scenarios, "scenario");

  return { behaviours, scenarios };
}

function dedupeWarn(items, kind) {
  const seen = new Set();
  for (const it of items) {
    if (!it || !it.id) { console.warn(`[registry] a ${kind} is missing an id`); continue; }
    if (seen.has(it.id)) console.warn(`[registry] duplicate ${kind} id: ${it.id}`);
    seen.add(it.id);
  }
}

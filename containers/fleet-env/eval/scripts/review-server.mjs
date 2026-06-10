#!/usr/bin/env node
import { createServer } from "node:http";
import { createReadStream, existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  FLEET_CONTEXT_KEY,
  FLEET_WHY_KEY,
  readScreenshotMetadata,
  reportBaseDir,
  resolveScreenshotPath,
  shotContextsFromReport,
} from "../lib/reviewContext.mjs";

const EVAL_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const args = parseArgs(process.argv.slice(2));
const jsonPath = resolve(args.json || resolve(EVAL_ROOT, "artifacts/eval.json"));
const artifactsDir = resolve(args.dir || dirname(jsonPath));
const host = args.host || process.env.HOST || "127.0.0.1";
const port = Number(args.port || process.env.PORT || process.env.FLEET_EVAL_REVIEW_PORT || 51779);

let cached = null;
let cachedAt = 0;

const server = createServer((req, res) => {
  try {
    const url = new URL(req.url, `http://${req.headers.host || "localhost"}`);
    if (url.pathname === "/") return sendHTML(res);
    if (url.pathname === "/healthz") return sendJSON(res, { ok: true });
    if (url.pathname === "/api/shots") return sendJSON(res, publicShots(loadShots()));

    const match = url.pathname.match(/^\/shot\/(\d+)\.png$/);
    if (match) return sendShot(res, Number(match[1]));

    res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    res.end("not found\n");
  } catch (err) {
    res.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
    res.end(`${err?.stack || err}\n`);
  }
});

server.listen(port, host, () => {
  const addr = server.address();
  console.log(`[review] serving ${loadShots().length} screenshot(s) at http://${addr.address}:${addr.port}/`);
});

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = () => argv[++i];
    if (arg === "--json") out.json = next();
    else if (arg === "--dir") out.dir = next();
    else if (arg === "--host") out.host = next();
    else if (arg === "--port") out.port = next();
    else if (arg === "--help" || arg === "-h") {
      console.log("usage: node scripts/review-server.mjs [--json artifacts/eval.json] [--dir artifacts] [--host 127.0.0.1] [--port 51779]");
      process.exit(0);
    }
  }
  return out;
}

function loadShots() {
  const now = Date.now();
  if (cached && now - cachedAt < 1000) return cached;
  cached = existsSync(jsonPath) ? loadFromReport(jsonPath) : loadFromMetadataDir(artifactsDir);
  cachedAt = now;
  return cached;
}

function loadFromReport(path) {
  const report = JSON.parse(readFileSync(path, "utf8"));
  const baseDir = reportBaseDir(path);
  return shotContextsFromReport(report, { baseDir }).map((shot, index) => enrichShot(shot, index, baseDir));
}

function loadFromMetadataDir(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((file) => extname(file).toLowerCase() === ".png")
    .sort()
    .map((file, index) => {
      const imagePath = resolve(dir, file);
      const metadata = safeMetadata(imagePath);
      const context = metadata.context || {
        schema: "fleet-eval-screenshot-context/v1",
        screenshot: { path: file, file, rowIndex: 0, rowCount: 1, exists: true },
        scenario: "",
        scenarioTitle: "",
        behaviour: "",
        title: file,
        status: "UNKNOWN",
        detail: "",
        rationale: metadata.why || "",
      };
      return normalizeShot(context, index, imagePath, metadata);
    });
}

function enrichShot(shot, index, baseDir) {
  const imagePath = resolveScreenshotPath(shot.screenshot.path, baseDir);
  const metadata = existsSync(imagePath) ? safeMetadata(imagePath) : { text: {} };
  return normalizeShot(metadata.context || shot, index, imagePath, metadata);
}

function normalizeShot(context, index, imagePath, metadata) {
  const file = basename(imagePath);
  return {
    id: index,
    url: `/shot/${index}.png`,
    file,
    path: context.screenshot?.path || file,
    scenario: context.scenario || "",
    scenarioTitle: context.scenarioTitle || "",
    behaviour: context.behaviour || "",
    title: context.title || context.behaviour || file,
    status: context.status || statusFromContext(context),
    detail: context.detail || "",
    rationale: context.rationale || metadata.why || "",
    provenance: context.provenance || null,
    evidence: context.evidence || null,
    machineDelta: context.machineDelta || null,
    timingsMs: context.timingsMs || null,
    run: context.run || null,
    hasPngContext: Boolean(metadata.text?.[FLEET_CONTEXT_KEY]),
    hasPngWhy: Boolean(metadata.text?.[FLEET_WHY_KEY]),
    imagePath,
  };
}

function safeMetadata(imagePath) {
  try { return readScreenshotMetadata(imagePath); }
  catch { return { context: null, why: "", text: {} }; }
}

function statusFromContext(context) {
  if (context.skipped) return "SKIP";
  if (context.error) return "ERROR";
  if (context.pass === false) return "FAIL";
  if (context.pass === true) return "PASS";
  return "UNKNOWN";
}

function publicShots(shots) {
  return shots.map(({ imagePath, ...shot }) => shot);
}

function sendShot(res, index) {
  const shot = loadShots()[index];
  if (!shot || !existsSync(shot.imagePath)) {
    res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    res.end("missing screenshot\n");
    return;
  }
  res.writeHead(200, {
    "content-type": "image/png",
    "cache-control": "no-store",
    "x-fleet-shot": shot.file,
  });
  createReadStream(shot.imagePath).pipe(res);
}

function sendJSON(res, data) {
  res.writeHead(200, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(JSON.stringify(data));
}

function sendHTML(res) {
  res.writeHead(200, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(`<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fleet screenshot review</title>
<style>
:root {
  color-scheme: dark;
  --bg: #111315;
  --panel: #191d21;
  --panel2: #20262b;
  --line: #343b43;
  --text: #edf0f2;
  --muted: #aab3bc;
  --green: #55c77a;
  --red: #f36b6b;
  --yellow: #dfbd55;
  --orange: #ee9854;
  --blue: #70a7ff;
}
* { box-sizing: border-box; }
html, body { height: 100%; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}
button, input, select {
  font: inherit;
  color: var(--text);
  background: var(--panel2);
  border: 1px solid var(--line);
  border-radius: 6px;
}
button { min-height: 32px; padding: 0 10px; cursor: pointer; }
button:hover { border-color: var(--blue); }
input { width: min(34vw, 420px); min-height: 32px; padding: 0 10px; }
.app {
  height: 100%;
  display: grid;
  grid-template-rows: auto 1fr;
}
.top {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--line);
  background: var(--panel);
}
.brand { font-weight: 700; white-space: nowrap; }
.count { color: var(--muted); white-space: nowrap; }
.spacer { flex: 1; }
.main {
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 390px;
}
.stage {
  min-width: 0;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 14px;
  background: #080a0c;
}
.stage img {
  max-width: 100%;
  max-height: calc(100vh - 82px);
  object-fit: contain;
  border: 1px solid var(--line);
  background: #000;
}
.side {
  min-width: 0;
  overflow: auto;
  border-left: 1px solid var(--line);
  background: var(--panel);
  padding: 14px;
}
.kicker, .file, .muted { color: var(--muted); }
.file { overflow-wrap: anywhere; }
h1 { margin: 6px 0 4px; font-size: 18px; line-height: 1.25; }
.id { color: var(--muted); overflow-wrap: anywhere; }
.status {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  padding: 2px 8px;
  margin: 10px 0;
  border: 1px solid var(--line);
  border-radius: 999px;
  font-weight: 700;
  font-size: 12px;
}
.PASS { color: var(--green); }
.FAIL, .ERROR { color: var(--red); }
.SKIP { color: var(--yellow); }
.UNKNOWN { color: var(--orange); }
dl {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  gap: 7px 10px;
  margin: 12px 0 0;
}
dt { color: var(--muted); }
dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
pre {
  margin: 0;
  padding: 10px;
  white-space: pre-wrap;
  overflow: auto;
  background: #0c0f12;
  border: 1px solid #252b31;
  border-radius: 6px;
  font-size: 12px;
}
.ok { color: var(--green); }
.bad { color: var(--red); }
@media (max-width: 920px) {
  .top { flex-wrap: wrap; }
  input { width: min(100%, 520px); flex: 1 1 220px; }
  .main { grid-template-columns: 1fr; grid-template-rows: minmax(0, 62vh) minmax(0, 1fr); }
  .side { border-left: 0; border-top: 1px solid var(--line); }
  .stage img { max-height: 58vh; }
}
</style>
</head>
<body>
<div class="app">
  <header class="top">
    <div class="brand">Fleet screenshots</div>
    <button id="prev" type="button">Prev</button>
    <button id="next" type="button">Next</button>
    <div class="count" id="count"></div>
    <input id="filter" autocomplete="off" placeholder="Filter">
    <div class="spacer"></div>
    <label class="muted"><input id="failOnly" type="checkbox"> failing</label>
  </header>
  <main class="main">
    <section class="stage"><img id="image" alt=""></section>
    <aside class="side">
      <div class="kicker" id="kicker"></div>
      <h1 id="title"></h1>
      <div class="id" id="ids"></div>
      <div class="status" id="status"></div>
      <div class="file" id="file"></div>
      <dl id="details"></dl>
    </aside>
  </main>
</div>
<script>
let shots = [];
let visible = [];
let cursor = 0;

const els = {
  image: document.getElementById("image"),
  count: document.getElementById("count"),
  filter: document.getElementById("filter"),
  failOnly: document.getElementById("failOnly"),
  prev: document.getElementById("prev"),
  next: document.getElementById("next"),
  kicker: document.getElementById("kicker"),
  title: document.getElementById("title"),
  ids: document.getElementById("ids"),
  status: document.getElementById("status"),
  file: document.getElementById("file"),
  details: document.getElementById("details"),
};

fetch("/api/shots")
  .then((res) => res.json())
  .then((data) => {
    shots = data;
    const hash = Number(location.hash.replace("#", ""));
    cursor = Number.isFinite(hash) && hash > 0 ? hash - 1 : 0;
    applyFilter();
  });

function applyFilter() {
  const q = els.filter.value.trim().toLowerCase();
  const failOnly = els.failOnly.checked;
  visible = shots.filter((shot) => {
    if (failOnly && !["FAIL", "ERROR"].includes(shot.status)) return false;
    if (!q) return true;
    return [shot.file, shot.path, shot.scenario, shot.scenarioTitle, shot.behaviour, shot.title, shot.detail, shot.status]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(q);
  }).map((shot) => shot.id);
  if (!visible.includes(cursor)) cursor = visible[0] ?? 0;
  render();
}

function render() {
  const shot = shots[cursor];
  if (!shot) {
    els.count.textContent = "0 / 0";
    els.title.textContent = "No screenshots";
    return;
  }
  const pos = Math.max(0, visible.indexOf(cursor));
  els.count.textContent = (pos + 1) + " / " + visible.length + " visible, " + shots.length + " total";
  els.image.src = shot.url;
  els.image.alt = shot.file;
  els.kicker.textContent = shot.run?.startedAt ? shot.run.startedAt : "";
  els.title.textContent = shot.title || shot.behaviour || shot.file;
  els.ids.textContent = [shot.scenarioTitle || shot.scenario, shot.behaviour].filter(Boolean).join(" / ");
  els.status.textContent = shot.status;
  els.status.className = "status " + shot.status;
  els.file.textContent = shot.file + (shot.hasPngWhy ? " / PNG metadata" : "");
  els.details.innerHTML = "";
  addDetail("detail", shot.detail);
  addDetail("why", shot.rationale, true);
  addDetail("changed", shot.provenance ? "[" + (shot.provenance.commit || "?") + " " + (shot.provenance.date || "?") + "] " + (shot.provenance.file || "") : "");
  addDetail("timing", shot.timingsMs);
  addDetail("machine", shot.machineDelta);
  addDetail("evidence", shot.evidence, true);
  history.replaceState(null, "", "#" + (cursor + 1));
  preload(pos + 1);
  preload(pos + 2);
  preload(pos - 1);
}

function addDetail(label, value, pre = false) {
  if (value == null || value === "") return;
  const dt = document.createElement("dt");
  const dd = document.createElement("dd");
  dt.textContent = label;
  if (typeof value === "object" || pre) {
    const node = document.createElement("pre");
    node.textContent = typeof value === "object" ? JSON.stringify(value, null, 2) : String(value).trim();
    dd.appendChild(node);
  } else {
    dd.textContent = String(value);
  }
  els.details.append(dt, dd);
}

function move(delta) {
  if (!visible.length) return;
  const at = Math.max(0, visible.indexOf(cursor));
  const next = Math.min(visible.length - 1, Math.max(0, at + delta));
  cursor = visible[next];
  render();
}

function preload(visibleIndex) {
  const id = visible[visibleIndex];
  if (id == null || !shots[id]) return;
  const img = new Image();
  img.src = shots[id].url;
}

els.prev.addEventListener("click", () => move(-1));
els.next.addEventListener("click", () => move(1));
els.filter.addEventListener("input", applyFilter);
els.failOnly.addEventListener("change", applyFilter);
window.addEventListener("keydown", (ev) => {
  if (ev.target === els.filter) return;
  if (["ArrowRight", "PageDown", " ", "j"].includes(ev.key)) { ev.preventDefault(); move(1); }
  else if (["ArrowLeft", "PageUp", "k"].includes(ev.key)) { ev.preventDefault(); move(-1); }
  else if (ev.key === "Home" || ev.key === "g") { cursor = visible[0] ?? 0; render(); }
  else if (ev.key === "End" || ev.key === "G") { cursor = visible[visible.length - 1] ?? 0; render(); }
  else if (ev.key === "/") { ev.preventDefault(); els.filter.focus(); }
});
</script>
</body>
</html>`);
}

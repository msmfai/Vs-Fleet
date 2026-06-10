#!/usr/bin/env node
// Classic CV pass for Fleet host window screenshots.
//
// The goal is not to "understand" VS Code semantically. It gives us stable,
// reviewable geometry evidence: alpha/window bounds, rail/editor split, titlebar
// band, dark/edge-heavy strips in the editor top chrome, and mask PNGs that make
// those detections visible.

import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, extname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { writeScreenshotMetadata } from "../../../containers/fleet-env/eval/lib/reviewContext.mjs";
import { decodePNG, encodePNG } from "../../../containers/fleet-env/eval/lib/visual.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const HOST_DIR = resolve(__dirname, "..");
const DEFAULT_DIR = resolve(HOST_DIR, "artifacts", "keepalive-reviewed", "2026-06-10");
const SCHEMA = "fleet-host-window-visual-analysis/v1";
const EXPECTED_RAIL_W = 248;
const TOP_SCAN_PX = 180;

function parseArgs(argv) {
  const opts = {
    dir: DEFAULT_DIR,
    json: "",
    out: "",
    masks: true,
    updateReport: false,
    tagMetadata: true,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = () => argv[++i];
    if (arg === "--dir") opts.dir = resolve(next());
    else if (arg === "--json") opts.json = resolve(next());
    else if (arg === "--out") opts.out = resolve(next());
    else if (arg === "--no-masks") opts.masks = false;
    else if (arg === "--update-report") opts.updateReport = true;
    else if (arg === "--no-tag-metadata") opts.tagMetadata = false;
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
  console.log(`usage: node crates/fleet-host/scripts/analyze-window-shots.mjs [options]

Options:
  --dir DIR          Artifact directory. Default: ${relative(process.cwd(), DEFAULT_DIR)}
  --json PATH       Review report JSON. Default: <dir>/host-keepalive.json if present.
  --out PATH        Analysis JSON. Default: <dir>/visual-analysis.json.
  --no-masks        Do not write cv/*.top-mask.png files.
  --update-report   Attach visual-analysis summary to the review report JSON.
  --no-tag-metadata Do not rewrite PNG screenshot metadata after --update-report.
`);
}

export function analyzeWindowShots({
  baseDir,
  report = null,
  screenshots = null,
  outPath = null,
  writeMasks = true,
} = {}) {
  const dir = resolve(baseDir || process.cwd());
  const shots = collectScreenshots({ baseDir: dir, report, screenshots });
  const analysis = {
    schema: SCHEMA,
    generatedAt: new Date().toISOString(),
    baseDir: dir,
    screenshotCount: shots.length,
    screenshots: [],
    comparisons: [],
    summary: {},
  };

  const maskDir = resolve(dir, "cv");
  if (writeMasks) mkdirSync(maskDir, { recursive: true });

  for (const shot of shots) {
    const result = analyzeOneScreenshot({
      baseDir: dir,
      shot,
      maskDir,
      writeMasks,
    });
    analysis.screenshots.push(result);
  }

  analysis.comparisons = compareGeometry(analysis.screenshots);
  analysis.summary = summarize(analysis);

  const jsonPath = outPath || resolve(dir, "visual-analysis.json");
  mkdirSync(dirname(jsonPath), { recursive: true });
  const persisted = {
    ...analysis,
    baseDir: relative(process.cwd(), analysis.baseDir) || ".",
  };
  writeFileSync(jsonPath, `${JSON.stringify(persisted, null, 2)}\n`);
  analysis.path = jsonPath;
  analysis.relativePath = relative(dir, jsonPath);
  return analysis;
}

export function attachVisualAnalysis(report, analysis, { baseDir = process.cwd() } = {}) {
  if (!report || !Array.isArray(report.results)) return report;
  const visualAnalysis = {
    path: relative(baseDir, analysis.path),
    screenshotCount: analysis.screenshotCount,
    summary: analysis.summary,
    masks: analysis.screenshots
      .map((shot) => shot.maskPath)
      .filter(Boolean),
  };

  for (const row of report.results) {
    row.evidence = row.evidence || {};
    row.evidence.visualAnalysis = visualAnalysis;
  }
  return report;
}

function collectScreenshots({ baseDir, report, screenshots }) {
  if (Array.isArray(screenshots) && screenshots.length) {
    return screenshots.map((screenshot) => normalizeShot(screenshot, baseDir));
  }

  const fromReport = [];
  for (const row of Array.isArray(report?.results) ? report.results : []) {
    for (const screenshot of Array.isArray(row.screenshots) ? row.screenshots : []) {
      fromReport.push(normalizeShot(screenshot, baseDir));
    }
  }
  if (fromReport.length) return fromReport;

  const dirs = [resolve(baseDir, "screenshots"), baseDir].filter(existsSync);
  const found = [];
  for (const dir of dirs) {
    for (const name of readdirSync(dir).sort()) {
      if (extname(name).toLowerCase() === ".png") {
        found.push(normalizeShot(relative(baseDir, resolve(dir, name)), baseDir));
      }
    }
    if (found.length) return found;
  }
  return found;
}

function normalizeShot(screenshot, baseDir) {
  const rel = typeof screenshot === "string" ? screenshot : screenshot?.file || screenshot?.path;
  const absPath = isAbsolute(rel) ? rel : resolve(baseDir, rel);
  return {
    file: isAbsolute(rel) ? relative(baseDir, rel) : rel,
    absPath,
  };
}

function analyzeOneScreenshot({ baseDir, shot, maskDir, writeMasks }) {
  const base = {
    file: shot.file,
    path: shot.file,
    decoded: false,
  };
  if (!existsSync(shot.absPath)) {
    return { ...base, error: "missing screenshot file" };
  }

  let image = null;
  try {
    image = decodePNG(readFileSync(shot.absPath));
  } catch (err) {
    return { ...base, error: err?.message || String(err) };
  }
  if (!image) return { ...base, error: "unsupported PNG variant" };

  const alphaBbox = bboxForAlpha(image, 8);
  const contentBbox = bboxForAlpha(image, 200) || alphaBbox || {
    x: 0,
    y: 0,
    width: image.width,
    height: image.height,
  };
  const railRightX = estimateRailRightX(image, contentBbox);
  const editorPane = {
    x: railRightX,
    y: contentBbox.y,
    width: Math.max(1, contentBbox.x + contentBbox.width - railRightX),
    height: contentBbox.height,
  };
  const titlebar = estimateTitlebar(image, contentBbox, editorPane);
  const topScan = scanEditorTop(image, editorPane, titlebar);
  const railStatus = analyzeRailStatus(image, contentBbox, titlebar);
  const flags = flagsFor({ contentBbox, railRightX, titlebar, topScan, railStatus });

  let maskPath = null;
  if (writeMasks) {
    const out = resolve(maskDir, `${basename(shot.file, ".png")}.top-mask.png`);
    writeFileSync(out, makeTopMask(image, contentBbox, editorPane, titlebar, railStatus));
    maskPath = relative(baseDir, out);
  }

  return {
    ...base,
    decoded: true,
    width: image.width,
    height: image.height,
    alpha: alphaStats(image, alphaBbox),
    geometry: {
      alphaBbox,
      contentBbox,
      estimatedRailRightX: railRightX,
      expectedRailRightX: contentBbox.x + EXPECTED_RAIL_W,
      railSplitDeltaPx: round2(railRightX - (contentBbox.x + EXPECTED_RAIL_W)),
      editorPane,
      titlebar,
    },
    railStatus,
    topScan,
    flags,
    maskPath,
  };
}

function bboxForAlpha(image, threshold) {
  let minX = image.width;
  let minY = image.height;
  let maxX = -1;
  let maxY = -1;
  for (let y = 0; y < image.height; y++) {
    for (let x = 0; x < image.width; x++) {
      const a = image.rgba[(y * image.width + x) * 4 + 3];
      if (a <= threshold) continue;
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
    }
  }
  if (maxX < minX || maxY < minY) return null;
  return {
    x: minX,
    y: minY,
    width: maxX - minX + 1,
    height: maxY - minY + 1,
  };
}

function alphaStats(image, bbox) {
  let transparent = 0;
  let translucent = 0;
  let opaque = 0;
  for (let i = 3; i < image.rgba.length; i += 4) {
    const a = image.rgba[i];
    if (a === 0) transparent++;
    else if (a === 255) opaque++;
    else translucent++;
  }
  return {
    transparent,
    translucent,
    opaque,
    bbox,
  };
}

function estimateRailRightX(image, box) {
  const minX = Math.max(0, box.x + 120);
  const maxX = Math.min(image.width - 3, box.x + 380);
  const y0 = Math.max(0, box.y + 68);
  const y1 = Math.min(image.height, box.y + Math.min(260, box.height - 20));

  let best = box.x + EXPECTED_RAIL_W;
  let bestScore = -Infinity;
  for (let x = minX; x <= maxX; x++) {
    const left = columnMean(image, x - 3, y0, y1);
    const right = columnMean(image, x + 3, y0, y1);
    if (!left.coverage || !right.coverage) continue;
    const score = right.mean - left.mean;
    if (score > bestScore) {
      bestScore = score;
      best = x;
    }
  }
  return clamp(Math.round(best), box.x, box.x + box.width - 80);
}

function columnMean(image, x, y0, y1) {
  let sum = 0;
  let count = 0;
  const xx = clamp(Math.round(x), 0, image.width - 1);
  for (let y = y0; y < y1; y++) {
    const p = pixel(image, xx, y);
    if (p.a <= 8) continue;
    sum += p.l;
    count++;
  }
  return { mean: count ? sum / count : 0, coverage: count / Math.max(1, y1 - y0) };
}

function estimateTitlebar(image, contentBbox, editorPane) {
  const x0 = clamp(editorPane.x + 16, 0, image.width - 1);
  const x1 = clamp(contentBbox.x + contentBbox.width - 16, x0 + 1, image.width);
  const yTop = contentBbox.y;
  const yMax = Math.min(image.height, contentBbox.y + 90);
  const rows = [];
  for (let y = yTop; y < yMax; y++) {
    rows.push({ y, ...rowStats(image, y, x0, x1) });
  }

  const firstLight = rows.find((row) => row.coverage > 0.75 && row.meanLuma > 160);
  const titlebarBottomY = firstLight ? firstLight.y : yTop;
  const topRows = rows.slice(0, Math.min(12, rows.length));
  const topMean = topRows.length
    ? topRows.reduce((sum, row) => sum + row.meanLuma, 0) / topRows.length
    : 0;

  return {
    y: yTop,
    bottomY: titlebarBottomY,
    heightPx: titlebarBottomY - yTop,
    editorTopMeanLuma: round2(topMean),
    editorPaintsUnderTitlebar: titlebarBottomY - yTop <= 4 && topMean > 140,
  };
}

function scanEditorTop(image, editorPane, titlebar) {
  const x0 = clamp(editorPane.x + 8, 0, image.width - 1);
  const x1 = clamp(editorPane.x + editorPane.width - 8, x0 + 1, image.width);
  const y0 = editorPane.y;
  const y1 = Math.min(image.height, editorPane.y + TOP_SCAN_PX);
  const rows = [];
  for (let y = y0; y < y1; y++) {
    rows.push({ y, relY: y - y0, ...rowStats(image, y, x0, x1) });
  }

  const verticalEdges = [];
  for (let i = 1; i < rows.length; i++) {
    const delta = Math.abs(rows[i].meanLuma - rows[i - 1].meanLuma);
    if (delta >= 5) {
      verticalEdges.push({
        y: rows[i].y,
        relY: rows[i].relY,
        deltaMeanLuma: round2(delta),
      });
    }
  }

  const stripRows = rows.filter((row) => {
    if (row.y < titlebar.bottomY) return false;
    return row.darkRatio >= 0.025 || row.edgeXMean >= 6 || row.stdLuma >= 18;
  });
  const strips = mergeRowRuns(stripRows)
    .map((run) => describeRun(run, rows))
    .sort((a, b) => b.score - a.score)
    .slice(0, 12);

  return {
    region: { x: x0, y: y0, width: x1 - x0, height: y1 - y0 },
    verticalEdges: verticalEdges.slice(0, 24),
    strips,
  };
}

function rowStats(image, y, x0, x1) {
  let count = 0;
  let sum = 0;
  let sum2 = 0;
  let dark = 0;
  let nonWhite = 0;
  let edgeX = 0;
  let edgeXCount = 0;
  let prev = null;

  for (let x = x0; x < x1; x++) {
    const p = pixel(image, x, y);
    if (p.a <= 8) {
      prev = null;
      continue;
    }
    count++;
    sum += p.l;
    sum2 += p.l * p.l;
    if (p.l < 85) dark++;
    if (p.l < 245) nonWhite++;
    if (prev != null) {
      edgeX += Math.abs(p.l - prev);
      edgeXCount++;
    }
    prev = p.l;
  }

  const width = Math.max(1, x1 - x0);
  const mean = count ? sum / count : 0;
  const variance = count ? Math.max(0, sum2 / count - mean * mean) : 0;
  return {
    coverage: round3(count / width),
    meanLuma: round2(mean),
    stdLuma: round2(Math.sqrt(variance)),
    darkRatio: round4(count ? dark / count : 0),
    nonWhiteRatio: round4(count ? nonWhite / count : 0),
    edgeXMean: round2(edgeXCount ? edgeX / edgeXCount : 0),
  };
}

function mergeRowRuns(rows) {
  const runs = [];
  let current = [];
  for (const row of rows) {
    if (!current.length || row.y <= current[current.length - 1].y + 1) {
      current.push(row);
    } else {
      runs.push(current);
      current = [row];
    }
  }
  if (current.length) runs.push(current);
  return runs.filter((run) => run.length >= 2);
}

function describeRun(run) {
  const avg = (key) => run.reduce((sum, row) => sum + row[key], 0) / run.length;
  const score = avg("darkRatio") * 100 + avg("edgeXMean") + avg("stdLuma") * 0.4;
  return {
    y: run[0].y,
    relY: run[0].relY,
    heightPx: run.length,
    score: round2(score),
    meanLuma: round2(avg("meanLuma")),
    stdLuma: round2(avg("stdLuma")),
    darkRatio: round4(avg("darkRatio")),
    nonWhiteRatio: round4(avg("nonWhiteRatio")),
    edgeXMean: round2(avg("edgeXMean")),
  };
}

function analyzeRailStatus(image, contentBbox, titlebar) {
  const region = {
    x: clamp(contentBbox.x + 45, 0, image.width - 1),
    y: clamp(titlebar.bottomY + 6, 0, image.height - 1),
    width: Math.min(160, Math.max(1, contentBbox.width - 65)),
    height: 30,
  };
  region.width = Math.min(region.width, image.width - region.x);
  region.height = Math.min(region.height, image.height - region.y);

  const counts = { green: 0, amber: 0, red: 0, colored: 0, sampled: 0 };
  let sumX = 0;
  let sumY = 0;
  for (let y = region.y; y < region.y + region.height; y++) {
    for (let x = region.x; x < region.x + region.width; x++) {
      const p = pixel(image, x, y);
      if (p.a <= 8) continue;
      counts.sampled++;
      if (isStatusGreen(p)) {
        counts.green++;
        counts.colored++;
        sumX += x;
        sumY += y;
      } else if (isStatusAmber(p)) {
        counts.amber++;
        counts.colored++;
        sumX += x;
        sumY += y;
      } else if (isStatusRed(p)) {
        counts.red++;
        counts.colored++;
        sumX += x;
        sumY += y;
      }
    }
  }

  const ranked = [
    ["connected", counts.green],
    ["waiting", counts.amber],
    ["red", counts.red],
  ].sort((a, b) => b[1] - a[1]);
  const [state, count] = ranked[0];
  const classified = count >= 8 ? state : "unknown";
  return {
    region,
    state: classified,
    counts,
    confidence: round3(count / Math.max(1, counts.colored || 49)),
    coloredCenter: counts.colored
      ? { x: round2(sumX / counts.colored), y: round2(sumY / counts.colored) }
      : null,
  };
}

function isStatusGreen(p) {
  return p.g > 130 && p.r < 100 && p.b < 140 && p.g > p.r * 1.7 && p.g > p.b * 1.25;
}

function isStatusAmber(p) {
  return p.r > 170 && p.g > 100 && p.b < 100 && p.r > p.b * 2 && p.g > p.b * 1.6;
}

function isStatusRed(p) {
  return p.r > 150 && p.g < 130 && p.b < 140 && p.r > p.g * 1.25 && p.r > p.b * 1.25;
}

function flagsFor({ contentBbox, railRightX, titlebar, topScan, railStatus }) {
  const flags = [];
  if (Math.abs(railRightX - (contentBbox.x + EXPECTED_RAIL_W)) > 12) {
    flags.push("rail-split-deviation");
  }
  if (titlebar.editorPaintsUnderTitlebar) {
    flags.push("editor-paints-in-titlebar-band");
  }
  if (railStatus.state === "red") {
    flags.push("rail-status-red");
  }
  const topDarkStrip = topScan.strips.find(
    (strip) => strip.relY >= titlebar.heightPx && strip.relY <= titlebar.heightPx + 90 && strip.darkRatio >= 0.05,
  );
  if (topDarkStrip) flags.push("dark-editor-top-strip");
  return flags;
}

function makeTopMask(image, contentBbox, editorPane, titlebar, railStatus) {
  const region = {
    x: contentBbox.x,
    y: contentBbox.y,
    width: contentBbox.width,
    height: Math.min(TOP_SCAN_PX, contentBbox.height),
  };
  const rgba = Buffer.alloc(region.width * region.height * 4, 0);

  for (let y = 0; y < region.height; y++) {
    for (let x = 0; x < region.width; x++) {
      const ax = region.x + x;
      const ay = region.y + y;
      const p = pixel(image, ax, ay);
      const o = (y * region.width + x) * 4;
      rgba[o + 3] = 255;

      if (p.a <= 8) {
        rgba[o + 3] = 0;
        continue;
      }

      const inEditor = ax >= editorPane.x;
      const edge = localEdge(image, ax, ay);
      if (Math.abs(ax - editorPane.x) <= 1) {
        rgba[o] = 0;
        rgba[o + 1] = 110;
        rgba[o + 2] = 255;
      } else if (Math.abs(ay - titlebar.bottomY) <= 1) {
        rgba[o] = 0;
        rgba[o + 1] = 220;
        rgba[o + 2] = 80;
      } else if (inside(ax, ay, railStatus.region) && (isStatusGreen(p) || isStatusAmber(p) || isStatusRed(p))) {
        rgba[o] = p.r;
        rgba[o + 1] = p.g;
        rgba[o + 2] = p.b;
      } else if (inEditor && p.l < 85) {
        rgba[o] = 255;
        rgba[o + 1] = 72;
        rgba[o + 2] = 72;
      } else if (inEditor && edge > 34) {
        rgba[o] = 255;
        rgba[o + 1] = 0;
        rgba[o + 2] = 255;
      } else {
        const v = Math.round(clamp(p.l * 0.22, 8, 64));
        rgba[o] = v;
        rgba[o + 1] = v;
        rgba[o + 2] = v;
      }
    }
  }

  return encodePNG(region.width, region.height, rgba);
}

function inside(x, y, region) {
  return x >= region.x && x < region.x + region.width && y >= region.y && y < region.y + region.height;
}

function localEdge(image, x, y) {
  const here = pixel(image, x, y);
  const right = x + 1 < image.width ? pixel(image, x + 1, y) : here;
  const down = y + 1 < image.height ? pixel(image, x, y + 1) : here;
  return Math.max(Math.abs(here.l - right.l), Math.abs(here.l - down.l));
}

function compareGeometry(shots) {
  const out = [];
  for (let i = 1; i < shots.length; i++) {
    const prev = shots[i - 1];
    const next = shots[i];
    if (!prev.decoded || !next.decoded) continue;
    out.push({
      from: prev.file,
      to: next.file,
      railSplitDeltaPx: next.geometry.estimatedRailRightX - prev.geometry.estimatedRailRightX,
      titlebarBottomDeltaPx: next.geometry.titlebar.bottomY - prev.geometry.titlebar.bottomY,
      flagChanges: {
        added: next.flags.filter((flag) => !prev.flags.includes(flag)),
        removed: prev.flags.filter((flag) => !next.flags.includes(flag)),
      },
    });
  }
  return out;
}

function summarize(analysis) {
  const decoded = analysis.screenshots.filter((shot) => shot.decoded);
  const flags = new Map();
  for (const shot of decoded) {
    for (const flag of shot.flags) flags.set(flag, (flags.get(flag) || 0) + 1);
  }
  const railDeltas = decoded.map((shot) => Math.abs(shot.geometry.railSplitDeltaPx));
  const titlebarHeights = decoded.map((shot) => shot.geometry.titlebar.heightPx);
  const railStatus = new Map();
  for (const shot of decoded) {
    const status = shot.railStatus?.state || "unknown";
    railStatus.set(status, (railStatus.get(status) || 0) + 1);
  }
  return {
    decodedCount: decoded.length,
    allDecoded: decoded.length === analysis.screenshotCount,
    flags: Object.fromEntries([...flags.entries()].sort()),
    railStatus: Object.fromEntries([...railStatus.entries()].sort()),
    maxRailSplitDeltaPx: railDeltas.length ? Math.max(...railDeltas) : 0,
    titlebarHeightPx: titlebarHeights.length
      ? {
          min: Math.min(...titlebarHeights),
          max: Math.max(...titlebarHeights),
        }
      : null,
  };
}

function pixel(image, x, y) {
  const xx = clamp(Math.round(x), 0, image.width - 1);
  const yy = clamp(Math.round(y), 0, image.height - 1);
  const o = (yy * image.width + xx) * 4;
  const r = image.rgba[o];
  const g = image.rgba[o + 1];
  const b = image.rgba[o + 2];
  const a = image.rgba[o + 3];
  return {
    r,
    g,
    b,
    a,
    l: 0.2126 * r + 0.7152 * g + 0.0722 * b,
  };
}

function clamp(n, min, max) {
  return Math.min(max, Math.max(min, n));
}

function round2(n) {
  return Math.round(n * 100) / 100;
}

function round3(n) {
  return Math.round(n * 1000) / 1000;
}

function round4(n) {
  return Math.round(n * 10000) / 10000;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const baseDir = resolve(opts.dir);
  const jsonPath = opts.json || resolve(baseDir, "host-keepalive.json");
  const report = existsSync(jsonPath) ? JSON.parse(readFileSync(jsonPath, "utf8")) : null;
  const analysis = analyzeWindowShots({
    baseDir,
    report,
    outPath: opts.out || resolve(baseDir, "visual-analysis.json"),
    writeMasks: opts.masks,
  });

  if (opts.updateReport) {
    if (!report) throw new Error(`--update-report requires a report JSON: ${jsonPath}`);
    attachVisualAnalysis(report, analysis, { baseDir });
    writeFileSync(jsonPath, `${JSON.stringify(report, null, 2)}\n`);
    if (opts.tagMetadata) writeScreenshotMetadata(report, { baseDir, quiet: true });
  }

  console.log(`[visual-analysis] report: ${analysis.path}`);
  console.log(`[visual-analysis] decoded=${analysis.summary.decodedCount}/${analysis.screenshotCount} flags=${JSON.stringify(analysis.summary.flags)}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    console.error(`[visual-analysis] ${err?.stack || err}`);
    process.exit(1);
  });
}

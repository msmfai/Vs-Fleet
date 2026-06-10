import { existsSync } from "node:fs";
import { basename, dirname, isAbsolute, resolve } from "node:path";
import { readPngText, upsertPngTexts } from "./pngMetadata.mjs";

export const FLEET_CONTEXT_KEY = "fleet.eval.context";
export const FLEET_WHY_KEY = "fleet.eval.why";

export function reportBaseDir(jsonPath) {
  const dir = dirname(resolve(jsonPath));
  return basename(dir) === "artifacts" ? dirname(dir) : dir;
}

export function shotContextsFromReport(report, { baseDir = process.cwd() } = {}) {
  const rows = Array.isArray(report?.results) ? report.results : [];
  const shots = [];
  for (const row of rows) {
    const paths = Array.isArray(row.screenshots) ? row.screenshots : [];
    for (let i = 0; i < paths.length; i++) {
      const screenshot = paths[i];
      const absPath = resolveScreenshotPath(screenshot, baseDir);
      shots.push({
        schema: "fleet-eval-screenshot-context/v1",
        run: {
          startedAt: report?.run?.startedAt || null,
          image: report?.run?.image || null,
        },
        screenshot: {
          path: screenshot,
          file: basename(screenshot),
          rowIndex: i,
          rowCount: paths.length,
          absPath,
          exists: absPath ? existsSync(absPath) : false,
        },
        scenario: row.scenario || "",
        scenarioTitle: row.scenarioTitle || "",
        behaviour: row.behaviour || "",
        title: row.title || row.behaviour || "",
        status: statusLabel(row),
        pass: row.pass === true,
        skipped: row.skipped || null,
        error: row.error || null,
        detail: row.detail || "",
        rationale: row.rationale || "",
        provenance: row.provenance || null,
        evidence: row.evidence || null,
        machineDelta: row.machineDelta || null,
        timingsMs: row.timingsMs || null,
      });
    }
  }
  return shots;
}

export function writeScreenshotMetadata(report, { baseDir = process.cwd(), quiet = false } = {}) {
  let tagged = 0;
  let missing = 0;
  for (const shot of shotContextsFromReport(report, { baseDir })) {
    if (!shot.screenshot.absPath || !existsSync(shot.screenshot.absPath)) {
      missing++;
      continue;
    }
    const context = { ...shot, screenshot: { ...shot.screenshot, absPath: undefined } };
    upsertPngTexts(shot.screenshot.absPath, [
      { keyword: FLEET_CONTEXT_KEY, text: JSON.stringify(context, null, 2) },
      { keyword: FLEET_WHY_KEY, text: context.rationale || context.title || context.detail || "" },
    ]);
    tagged++;
  }
  if (!quiet) {
    const suffix = missing ? ` (${missing} missing)` : "";
    console.log(`[eval] tagged screenshot metadata → ${tagged} PNG(s)${suffix}`);
  }
  return { tagged, missing };
}

export function readScreenshotMetadata(path) {
  const text = readPngText(path);
  let context = null;
  if (text[FLEET_CONTEXT_KEY]) {
    try { context = JSON.parse(text[FLEET_CONTEXT_KEY]); } catch {}
  }
  return {
    context,
    why: text[FLEET_WHY_KEY] || context?.rationale || "",
    text,
  };
}

export function resolveScreenshotPath(p, baseDir = process.cwd()) {
  const raw = String(p || "");
  if (!raw) return null;
  if (isAbsolute(raw)) return raw;
  const candidates = [
    resolve(process.cwd(), raw),
    resolve(baseDir, raw),
    resolve(dirname(baseDir), raw),
  ];
  return candidates.find((candidate) => existsSync(candidate)) || candidates[1];
}

function statusLabel(row) {
  if (row.skipped) return "SKIP";
  if (row.error) return "ERROR";
  return row.pass ? "PASS" : "FAIL";
}

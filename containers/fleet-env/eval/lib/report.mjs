// Reporter — console + JSON (§3.5 frozen schema) + JUnit XML + linked HTML
// reports + PNG screenshot metadata. Track A shipped the console+JSON stub;
// Track F (this file) ADDS the XML/HTML emitters. The JSON shape is unchanged so all
// emitters consume the same Report object. PNG metadata feeds the hosted screenshot
// review UI; eval.html remains the row-oriented CI report.
//
//   { run: {startedAt,image,scenarios:N,behaviours:M},
//     results: [ {scenario,behaviour,pass,detail,evidence,machineDelta,
//                 timingsMs,screenshots,skipped?,error?} ],
//     summary: {pass,fail,skipped,durationMs} }

import { existsSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { reportBaseDir, writeScreenshotMetadata as tagScreenshotMetadata } from "./reviewContext.mjs";

export class Reporter {
  /** @param {{image?:string, scenarios?:number, behaviours?:number}} runMeta */
  constructor(runMeta = {}) {
    this.run = {
      startedAt: new Date().toISOString(),
      image: runMeta.image || "fleet-env:latest",
      scenarios: runMeta.scenarios ?? 0,
      behaviours: runMeta.behaviours ?? 0,
    };
    this.results = [];
    this._t0 = Date.now();
  }

  // Record one (scenario × behaviour) cell. `result` carries the §3.5 row fields.
  add(result) {
    this.results.push(result);
    this._line(result);
  }

  _line(r) {
    let mark;
    if (r.skipped) mark = "⏭️  SKIP";
    else if (r.error) mark = "💥 ERROR";
    else mark = r.pass ? "✅ PASS" : "❌ FAIL";
    console.log(`[eval] ${mark}  ${r.scenario} × ${r.behaviour}`);
    if (r.detail) console.log(`[eval]      ${r.detail}`);
    if (r.skipped) console.log(`[eval]      skipped: ${r.skipped}`);
    if (r.error) console.log(`[eval]      error: ${r.error}`);
    // For an unexpected/skip row, surface what/why/when right where it happened:
    // provenance ([commit·date]) + the test's rationale, so a break is interrogable.
    if (r.skipped || r.error || !r.pass) {
      const p = provLabel(r.provenance);
      if (p) console.log(`[eval]      provenance: ${p}`);
      if (r.rationale) for (const ln of rationaleLines(r.rationale)) console.log(`[eval]      why: ${ln}`);
    }
    if (r.machineDelta && Object.keys(r.machineDelta).length) {
      console.log(`[eval]      machineΔ ${JSON.stringify(r.machineDelta)}`);
    }
    if (r.timingsMs && Object.keys(r.timingsMs).length) {
      console.log(`[eval]      timingsMs ${JSON.stringify(r.timingsMs)}`);
    }
  }

  summary() {
    const pass = this.results.filter((r) => !r.skipped && !r.error && r.pass).length;
    const skipped = this.results.filter((r) => r.skipped).length;
    const fail = this.results.length - pass - skipped; // includes errors
    return { pass, fail, skipped, durationMs: Date.now() - this._t0 };
  }

  toJSON() {
    return { run: this.run, results: this.results, summary: this.summary() };
  }

  writeJSON(path) {
    writeFileSync(path, JSON.stringify(this.toJSON(), null, 2));
    console.log(`[eval] wrote JSON report → ${path}`);
  }

  // ─── JUnit XML (consumed by CI: GitLab/GitHub/Jenkins) ──────────────────────
  // One <testsuite> per scenario; one <testcase> per result row. Classnames are
  // the scenario id; testcase names are the behaviour id. fail → <failure>,
  // error → <error>, skip → <skipped>. Durations come from timingsMs.effect.
  toJUnitXML() {
    const bySuite = new Map();
    for (const r of this.results) {
      const k = r.scenario || "(unknown)";
      if (!bySuite.has(k)) bySuite.set(k, []);
      bySuite.get(k).push(r);
    }
    const esc = xmlEscapeAttr;
    const lines = ['<?xml version="1.0" encoding="UTF-8"?>'];
    const s = this.summary();
    lines.push(`<testsuites name="fleet-eval" tests="${this.results.length}" ` +
      `failures="${s.fail}" skipped="${s.skipped}" time="${(s.durationMs / 1000).toFixed(3)}">`);
    for (const [suite, rows] of bySuite) {
      const fails = rows.filter((r) => !r.skipped && (r.error || !r.pass)).length;
      const skips = rows.filter((r) => r.skipped).length;
      const time = rows.reduce((acc, r) => acc + (r.timingsMs?.effect || 0), 0) / 1000;
      lines.push(`  <testsuite name="${esc(suite)}" tests="${rows.length}" ` +
        `failures="${fails}" skipped="${skips}" time="${time.toFixed(3)}">`);
      for (const r of rows) {
        const t = ((r.timingsMs?.effect || 0) / 1000).toFixed(3);
        lines.push(`    <testcase classname="${esc(suite)}" ` +
          `name="${esc(r.behaviour || "(unnamed)")}" time="${t}">`);
        if (r.skipped) {
          lines.push(`      <skipped message="${esc(String(r.skipped))}"/>`);
        } else if (r.error) {
          lines.push(`      <error message="${esc(String(r.error))}">${xmlEscapeText(String(r.error))}</error>`);
        } else if (!r.pass) {
          lines.push(`      <failure message="${esc(r.detail || "behaviour assertion failed")}">` +
            `${xmlEscapeText(r.detail || "")}</failure>`);
        }
        if (r.detail) lines.push(`      <system-out>${xmlEscapeText(r.detail)}</system-out>`);
        lines.push(`    </testcase>`);
      }
      lines.push(`  </testsuite>`);
    }
    lines.push(`</testsuites>`);
    return lines.join("\n") + "\n";
  }

  writeJUnit(path) {
    writeFileSync(path, this.toJUnitXML());
    console.log(`[eval] wrote JUnit XML → ${path}`);
  }

  // ─── HTML report with linked screenshot PNGs ───────────────────────────────
  // Screenshots stay as normal files so large visual runs do not create huge
  // base64 documents or force the browser to parse every image as one string.
  toHTML({ assetBase = process.cwd() } = {}) {
    const s = this.summary();
    const rows = this.results.map((r, i) => this._htmlRow(r, i, assetBase)).join("\n");
    const passPct = this.results.length
      ? Math.round((s.pass / this.results.length) * 100) : 0;
    return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fleet eval — ${escHtml(this.run.startedAt)}</title>
<style>
  :root { color-scheme: dark light; }
  body { font: 14px/1.5 -apple-system, system-ui, sans-serif; margin: 0; background: #14161a; color: #e6e6e6; }
  header { padding: 20px 24px; background: #1c1f26; border-bottom: 1px solid #2a2e38; }
  h1 { margin: 0 0 6px; font-size: 18px; }
  .meta { color: #9aa0aa; font-size: 13px; }
  .bar { height: 8px; border-radius: 4px; background: #c0392b; overflow: hidden; margin-top: 12px; }
  .bar > span { display: block; height: 100%; background: #27ae60; width: ${passPct}%; }
  .counts { display: flex; gap: 16px; margin-top: 12px; flex-wrap: wrap; }
  .pill { padding: 4px 10px; border-radius: 12px; font-weight: 600; font-size: 12px; }
  .pill.pass { background: #16341f; color: #57d98a; }
  .pill.fail { background: #3a1414; color: #ff6b6b; }
  .pill.skip { background: #2a2a14; color: #d9c357; }
  main { padding: 16px 24px 48px; }
  details { border: 1px solid #2a2e38; border-radius: 8px; margin: 8px 0; background: #1a1d24; overflow: hidden; }
  summary { padding: 10px 14px; cursor: pointer; display: flex; gap: 10px; align-items: center; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  .status { width: 14px; height: 14px; border-radius: 50%; flex: 0 0 auto; }
  .status.pass { background: #27ae60; } .status.fail { background: #c0392b; }
  .status.skip { background: #d9c357; } .status.error { background: #e67e22; }
  .name { font-weight: 600; }
  .scn { color: #9aa0aa; font-weight: 400; }
  .detail { margin-left: auto; color: #9aa0aa; font-size: 12px; max-width: 50%; text-align: right;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .body { padding: 0 14px 14px; border-top: 1px solid #2a2e38; }
  .kv { display: grid; grid-template-columns: 120px 1fr; gap: 4px 12px; margin: 12px 0; font-size: 13px; }
  .kv dt { color: #9aa0aa; } .kv dd { margin: 0; }
  pre { background: #0f1116; padding: 10px; border-radius: 6px; overflow: auto; font-size: 12px; }
  .shots { display: flex; gap: 12px; flex-wrap: wrap; margin-top: 12px; }
  .shots figure { margin: 0; } .shots img { max-width: 380px; border: 1px solid #2a2e38; border-radius: 6px; display: block; }
  .shots figcaption { color: #9aa0aa; font-size: 11px; margin-top: 4px; }
  .err { color: #ff6b6b; }
  code { background: #0f1116; padding: 1px 5px; border-radius: 4px; }
</style></head>
<body>
<header>
  <h1>Fleet behaviour eval</h1>
  <div class="meta">image <code>${escHtml(this.run.image)}</code> · started ${escHtml(this.run.startedAt)} ·
    ${this.run.scenarios} scenarios × ${this.run.behaviours} behaviours · ${(s.durationMs / 1000).toFixed(1)}s</div>
  <div class="bar"><span></span></div>
  <div class="counts">
    <span class="pill pass">${s.pass} pass</span>
    <span class="pill fail">${s.fail} fail</span>
    <span class="pill skip">${s.skipped} skip</span>
  </div>
</header>
<main>
${rows}
</main>
</body></html>
`;
  }

  _htmlRow(r, i, assetBase) {
    let cls, label;
    if (r.skipped) { cls = "skip"; label = "SKIP"; }
    else if (r.error) { cls = "error"; label = "ERROR"; }
    else if (r.pass) { cls = "pass"; label = "PASS"; }
    else { cls = "fail"; label = "FAIL"; }
    const open = (cls === "fail" || cls === "error") ? " open" : "";

    const kv = [];
    if (r.detail) kv.push(`<dt>detail</dt><dd>${escHtml(r.detail)}</dd>`);
    if (r.skipped) kv.push(`<dt>skipped</dt><dd>${escHtml(String(r.skipped))}</dd>`);
    if (r.error) kv.push(`<dt>error</dt><dd class="err">${escHtml(String(r.error))}</dd>`);
    // For unexpected/skip rows, surface the what/why/when so a break is interrogable
    // straight from the HTML: provenance ([commit·date], links to file) + rationale.
    if (r.skipped || r.error || !r.pass) {
      const p = provLabel(r.provenance);
      if (p) {
        const f = r.provenance?.file;
        kv.push(`<dt>provenance</dt><dd><code>${escHtml(p)}</code>${f ? ` <span class="scn">${escHtml(f)}</span>` : ""}</dd>`);
      }
      if (r.rationale) kv.push(`<dt>rationale</dt><dd><pre>${escHtml(String(r.rationale).trim())}</pre></dd>`);
    }
    if (r.machineDelta && Object.keys(r.machineDelta).length) {
      kv.push(`<dt>machineΔ</dt><dd><code>${escHtml(JSON.stringify(r.machineDelta))}</code></dd>`);
    }
    if (r.timingsMs && Object.keys(r.timingsMs).length) {
      kv.push(`<dt>timingsMs</dt><dd><code>${escHtml(JSON.stringify(r.timingsMs))}</code></dd>`);
    }

    const body = [`<div class="kv">${kv.join("")}</div>`];
    if (r.evidence && Object.keys(r.evidence).length) {
      body.push(`<div class="kv"><dt>evidence</dt><dd><pre>${escHtml(
        JSON.stringify(r.evidence, null, 2))}</pre></dd></div>`);
    }

    const paths = Array.isArray(r.screenshots) ? r.screenshots : [];
    if (paths.length) {
      const figs = paths.map((p) => {
        const src = imgSrc(p, assetBase);
        const cap = escHtml(baseName(p));
        return src
          ? `<figure><img src="${escHtml(src)}" alt="${cap}" loading="lazy" decoding="async"><figcaption>${cap}</figcaption></figure>`
          : `<figure><figcaption class="err">missing: ${cap}</figcaption></figure>`;
      }).join("");
      body.push(`<div class="shots">${figs}</div>`);
    }

    return `<details${open}>
  <summary>
    <span class="status ${cls}"></span>
    <span class="name">${escHtml(r.behaviour || "(unnamed)")}</span>
    <span class="scn">${escHtml(r.scenario || "")}</span>
    <span class="detail" title="${escHtml(r.detail || "")}">${label}${r.detail ? " — " + escHtml(r.detail) : ""}</span>
  </summary>
  <div class="body">
    ${body.join("\n    ")}
  </div>
</details>`;
  }

  writeHTML(path) {
    writeFileSync(path, this.toHTML({ assetBase: resolve(dirname(path)) }));
    console.log(`[eval] wrote HTML report → ${path}`);
  }

  // ─── Screenshot review page ────────────────────────────────────────────────
  // One page for flicking through captured screens with the row's
  // detail, rationale, provenance, and evidence beside the image.
  toReviewHTML({ assetBase = process.cwd() } = {}) {
    const s = this.summary();
    const shots = [];
    for (const r of this.results) {
      for (const p of Array.isArray(r.screenshots) ? r.screenshots : []) {
        shots.push({ row: r, path: p });
      }
    }
    const cards = shots.map((it, i) => this._reviewShot(it.row, it.path, i, shots.length, assetBase)).join("\n");
    const empty = shots.length ? "" : `<section class="empty">
  <h2>No screenshots captured</h2>
  <p>This run produced result rows, but none of them had a readable screenshot path.</p>
</section>`;

    return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fleet screenshot review — ${escHtml(this.run.startedAt)}</title>
<style>
  :root { color-scheme: dark; --bg: #101214; --panel: #171b20; --line: #303741; --text: #eceff1; --muted: #aeb7c2; --ok: #49c172; --bad: #f05b5b; --warn: #e4bd4f; }
  * { box-sizing: border-box; }
  html { scroll-behavior: smooth; }
  body { margin: 0; background: var(--bg); color: var(--text); font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
  header { position: sticky; top: 0; z-index: 2; display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 12px 18px; background: rgba(16, 18, 20, 0.96); border-bottom: 1px solid var(--line); }
  h1 { margin: 0; font-size: 16px; font-weight: 700; }
  .meta { color: var(--muted); font-size: 12px; }
  .counts { display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end; }
  .pill { border: 1px solid var(--line); border-radius: 999px; padding: 3px 8px; font-size: 12px; color: var(--muted); }
  .pill.pass { color: var(--ok); } .pill.fail { color: var(--bad); } .pill.skip { color: var(--warn); }
  main { scroll-snap-type: y proximity; }
  .shot { min-height: calc(100vh - 49px); scroll-snap-align: start; display: grid; grid-template-columns: minmax(0, 1fr) 360px; gap: 18px; padding: 18px; border-bottom: 1px solid var(--line); }
  .frame { min-width: 0; display: flex; align-items: center; justify-content: center; background: #0a0c0e; border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
  .frame img { display: block; max-width: 100%; max-height: calc(100vh - 88px); object-fit: contain; }
  .context { align-self: start; max-height: calc(100vh - 88px); overflow: auto; background: var(--panel); border: 1px solid var(--line); border-radius: 6px; padding: 14px; }
  .kicker { color: var(--muted); font-size: 12px; margin-bottom: 8px; }
  h2 { margin: 0 0 6px; font-size: 17px; line-height: 1.25; }
  .id { color: var(--muted); overflow-wrap: anywhere; }
  .status { display: inline-block; margin: 10px 0; border-radius: 999px; padding: 3px 8px; font-weight: 700; font-size: 12px; border: 1px solid var(--line); }
  .status.pass { color: var(--ok); } .status.fail, .status.error { color: var(--bad); } .status.skip { color: var(--warn); }
  dl { display: grid; grid-template-columns: 96px 1fr; gap: 6px 10px; margin: 10px 0 0; }
  dt { color: var(--muted); }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  pre { margin: 0; padding: 10px; background: #0c0f12; border-radius: 4px; overflow: auto; white-space: pre-wrap; font-size: 12px; }
  code { background: #0c0f12; border-radius: 4px; padding: 1px 4px; }
  .empty { padding: 32px; color: var(--muted); }
  @media (max-width: 900px) {
    header { position: static; align-items: flex-start; flex-direction: column; }
    .shot { grid-template-columns: 1fr; }
    .context { max-height: none; }
    .frame img { max-height: 70vh; }
  }
</style></head>
<body>
<header>
  <div>
    <h1>Fleet screenshot review</h1>
    <div class="meta">${shots.length} screenshot(s) · image <code>${escHtml(this.run.image)}</code> · started ${escHtml(this.run.startedAt)} · ${(s.durationMs / 1000).toFixed(1)}s</div>
  </div>
  <div class="counts">
    <span class="pill pass">${s.pass} pass</span>
    <span class="pill fail">${s.fail} fail</span>
    <span class="pill skip">${s.skipped} skip</span>
  </div>
</header>
<main>
${cards}
${empty}
</main>
<script>
(() => {
  const shots = [...document.querySelectorAll(".shot")];
  const nearest = () => shots.reduce((best, el) => {
    const d = Math.abs(el.getBoundingClientRect().top);
    return !best || d < best.d ? { el, d } : best;
  }, null)?.el;
  window.addEventListener("keydown", (ev) => {
    const keys = ["ArrowDown", "ArrowRight", "PageDown", " ", "j", "ArrowUp", "ArrowLeft", "PageUp", "k"];
    if (!keys.includes(ev.key)) return;
    const at = Math.max(0, shots.indexOf(nearest()));
    const dir = ["ArrowUp", "ArrowLeft", "PageUp", "k"].includes(ev.key) ? -1 : 1;
    const next = shots[Math.min(shots.length - 1, Math.max(0, at + dir))];
    if (next) { ev.preventDefault(); next.scrollIntoView({ block: "start" }); }
  });
})();
</script>
</body></html>
`;
  }

  _reviewShot(r, path, index, total, assetBase) {
    const src = imgSrc(path, assetBase);
    const cls = statusClass(r);
    const label = statusLabel(r);
    const p = provLabel(r.provenance);
    const dl = [];
    if (r.detail) dl.push(`<dt>happened</dt><dd>${escHtml(r.detail)}</dd>`);
    if (r.error) dl.push(`<dt>error</dt><dd>${escHtml(String(r.error))}</dd>`);
    if (r.skipped) dl.push(`<dt>skipped</dt><dd>${escHtml(String(r.skipped))}</dd>`);
    if (p) {
      dl.push(`<dt>changed</dt><dd><code>${escHtml(p)}</code>${r.provenance?.file ? ` ${escHtml(r.provenance.file)}` : ""}</dd>`);
    }
    if (r.timingsMs) dl.push(`<dt>timing</dt><dd><code>${escHtml(JSON.stringify(r.timingsMs))}</code></dd>`);
    if (r.machineDelta && Object.keys(r.machineDelta).length) {
      dl.push(`<dt>machine</dt><dd><code>${escHtml(JSON.stringify(r.machineDelta))}</code></dd>`);
    }
    if (r.rationale) dl.push(`<dt>look for</dt><dd><pre>${escHtml(String(r.rationale).trim())}</pre></dd>`);
    if (r.evidence && Object.keys(r.evidence).length) {
      dl.push(`<dt>evidence</dt><dd><pre>${escHtml(JSON.stringify(r.evidence, null, 2))}</pre></dd>`);
    }
    const image = src
      ? `<img src="${escHtml(src)}" alt="${escHtml(baseName(path))}" loading="lazy" decoding="async">`
      : `<p class="id">missing screenshot: ${escHtml(path)}</p>`;
    return `<section class="shot" id="shot-${index + 1}">
  <div class="frame">${image}</div>
  <aside class="context">
    <div class="kicker">${index + 1} / ${total} · ${escHtml(baseName(path))}</div>
    <h2>${escHtml(r.title || r.behaviour || "(unnamed)")}</h2>
    <div class="id">${escHtml(r.scenario || "")}${r.scenarioTitle ? " · " + escHtml(r.scenarioTitle) : ""}</div>
    <div class="id">${escHtml(r.behaviour || "")}</div>
    <div class="status ${cls}">${label}</div>
    <dl>
      ${dl.join("\n      ")}
    </dl>
  </aside>
</section>`;
  }

  writeReviewHTML(path) {
    writeFileSync(path, this.toReviewHTML({ assetBase: resolve(dirname(path)) }));
    console.log(`[eval] wrote screenshot review → ${path}`);
  }

  writeScreenshotMetadata({ baseDir = process.cwd() } = {}) {
    return tagScreenshotMetadata(this.toJSON(), { baseDir });
  }

  // Write all configured artifacts at once. Paths default off; pass what you want.
  writeAll({ json, junit, html, review } = {}) {
    if (json) this.writeJSON(json);
    if (junit) this.writeJUnit(junit);
    if (html) this.writeHTML(html);
    if (review) this.writeReviewHTML(review);
  }

  // Console epilogue + exit-code signal. Unexpected = fail or error (not skip).
  // Also tags PNG metadata and auto-emits JUnit/HTML when FLEET_EVAL_JUNIT /
  // FLEET_EVAL_HTML are set, so the Makefile/CI gets artifacts without run.mjs
  // having to know about each emitter (--json stays the explicit JSON path).
  finish() {
    const s = this.summary();
    const baseDir = process.env.FLEET_EVAL_JSON ? reportBaseDir(process.env.FLEET_EVAL_JSON) : process.cwd();
    this.writeScreenshotMetadata({ baseDir });
    if (process.env.FLEET_EVAL_JSON) this.writeJSON(process.env.FLEET_EVAL_JSON);
    if (process.env.FLEET_EVAL_JUNIT) this.writeJUnit(process.env.FLEET_EVAL_JUNIT);
    if (process.env.FLEET_EVAL_HTML) this.writeHTML(process.env.FLEET_EVAL_HTML);
    if (process.env.FLEET_EVAL_REVIEW) this.writeReviewHTML(process.env.FLEET_EVAL_REVIEW);
    this._summarizeUnexpected();
    console.log(`\n[eval] RESULT: ${s.pass} pass, ${s.fail} fail, ${s.skipped} skipped` +
      ` (${s.durationMs}ms)`);
    return s.fail === 0; // true ⇒ exit 0
  }

  // Collate every FAIL/ERROR/SKIP row at the end of the run, each annotated with its
  // provenance ([commit·date]) and rationale, so the "what failed / why / when it
  // last changed" is one glance away without scrolling the streamed log.
  _summarizeUnexpected() {
    const fails = this.results.filter((r) => !r.skipped && (r.error || !r.pass));
    const skips = this.results.filter((r) => r.skipped);
    if (!fails.length && !skips.length) return;
    const emit = (heading, rows, reasonOf) => {
      if (!rows.length) return;
      console.log(`\n[eval] ${heading} (${rows.length}):`);
      for (const r of rows) {
        const p = provLabel(r.provenance);
        console.log(`[eval]   ${r.scenario} × ${r.behaviour}${p ? "  " + p : ""}`);
        const reason = reasonOf(r);
        if (reason) console.log(`[eval]       ${reason}`);
        if (r.rationale) for (const ln of rationaleLines(r.rationale)) console.log(`[eval]       why: ${ln}`);
      }
    };
    emit("FAILURES", fails, (r) => r.error ? `error: ${r.error}` : (r.detail || "assertion failed"));
    emit("SKIPS", skips, (r) => `skipped: ${r.skipped}`);
  }
}

// ─── helpers ──────────────────────────────────────────────────────────────────
function escHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

// XML attribute values: escape &,<,>,",' — and strip control chars XML 1.0 forbids.
function xmlEscapeAttr(s) {
  return String(s)
    .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F]/g, "")
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&apos;");
}
function xmlEscapeText(s) {
  return String(s)
    .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F]/g, "")
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function baseName(p) { return String(p).split("/").pop(); }

function statusClass(r) {
  if (r.skipped) return "skip";
  if (r.error) return "error";
  return r.pass ? "pass" : "fail";
}

function statusLabel(r) {
  if (r.skipped) return "SKIP";
  if (r.error) return "ERROR";
  return r.pass ? "PASS" : "FAIL";
}

// "[commit·date]" provenance label (registry stamps {commit,date,file} per test).
function provLabel(p) {
  if (!p || (!p.commit && !p.date)) return "";
  return `[${p.commit || "?"}·${p.date || "?"}]`;
}

// Rationale can be multi-line prose; yield trimmed non-empty lines for the console.
function rationaleLines(r) {
  return String(r).split("\n").map((l) => l.trim()).filter(Boolean);
}

function imgSrc(p, assetBase) {
  const abs = findImagePath(p, assetBase);
  if (!abs) return null;
  return relative(assetBase, abs).split(sep).join("/") || baseName(abs);
}

function findImagePath(p, assetBase) {
  const raw = String(p);
  if (isAbsolute(raw)) return existsSync(raw) ? raw : null;
  const bases = [
    process.cwd(),
    assetBase,
    dirname(assetBase),
  ];
  for (const base of bases) {
    const abs = resolve(base, raw);
    if (existsSync(abs)) return abs;
  }
  return null;
}

// Visual smoke behaviours: assertions over the real Playwright screenshot, not
// just bridge state. These guard blank/black workbench renders and missing editor
// chrome after startup.

import { readFileSync } from "node:fs";

import { decodePNG } from "../lib/visual.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function imageStats(path) {
  const png = decodePNG(readFileSync(path));
  if (!png) return { decoded: false };

  const total = png.width * png.height;
  let blackish = 0;
  let light = 0;
  const buckets = new Set();
  for (let i = 0; i < total; i++) {
    const o = i * 4;
    const r = png.rgba[o];
    const g = png.rgba[o + 1];
    const b = png.rgba[o + 2];
    const lum = (r + g + b) / 3;
    if (lum < 12) blackish++;
    if (lum > 180) light++;
    buckets.add(`${r >> 4},${g >> 4},${b >> 4}`);
  }

  return {
    decoded: true,
    width: png.width,
    height: png.height,
    blackishRatio: Number((blackish / total).toFixed(4)),
    lightRatio: Number((light / total).toFixed(4)),
    colorBuckets: buckets.size,
  };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "visual.workbenchTabSmoke",
    title: "Visual: workbench renders with an editor tab and nonblank screenshot",
    tags: ["visual", "smoke"],
    needs: ["writeFile", "openFile", "query"],
    rationale: `
WHAT: Boots one Fleet environment, writes and opens a real file so VS Code must
show an editor tab, then uses Playwright DOM checks plus a decoded screenshot to
assert that the workbench chrome is present and the captured page is not blank or
all-black.

WHY THIS IS THE EXPECTED OUTCOME: A healthy VS Code / code-server workbench with
one opened file has a .monaco-workbench root, an editor part, a tab row, an
activity bar, and a status bar. Its screenshot contains many colour buckets and
not an overwhelming fraction of near-black pixels. Those are stable visual facts
that do not depend on exact theme colours or text rendering.

WHY IT MATTERS: Bridge command/query tests can pass while the user-facing page is
visually broken. This catches the class of regressions where the editor starts
and phones home but the browser surface is blank, black, missing tabs, or missing
core workbench chrome. It is intentionally a coarse smoke test: exact pixel
matching would be brittle across VS Code/theme/browser versions.`,
    async run(env) {
      if (!env.page) {
        return {
          pass: false,
          detail: "no Playwright page available for visual smoke",
        };
      }

      const file = "/home/coder/project/visual-smoke.txt";
      await env.request({
        type: "writeFile",
        path: file,
        content: "Fleet visual smoke\n",
      });
      await env.request({ type: "openFile", path: file });
      await sleep(1000);

      await env.page.waitForSelector(".monaco-workbench", { timeout: 15000 });
      await env.page.waitForSelector(".monaco-workbench .part.editor", { timeout: 15000 });
      await env.page.waitForSelector(".monaco-workbench .part.statusbar", { timeout: 15000 });

      const tabCount = await env.page
        .locator(".monaco-workbench .part.editor .tabs-container .tab")
        .count();
      const activityBarCount = await env.page
        .locator(".monaco-workbench .part.activitybar")
        .count();
      const editorCount = await env.page
        .locator(".monaco-workbench .part.editor")
        .count();
      const statusBarCount = await env.page
        .locator(".monaco-workbench .part.statusbar")
        .count();

      const shot = await env.screenshot("visual.workbenchTabSmoke.full");
      const stats = imageStats(shot);

      const pass =
        tabCount >= 1 &&
        activityBarCount >= 1 &&
        editorCount >= 1 &&
        statusBarCount >= 1 &&
        stats.decoded &&
        stats.width >= 800 &&
        stats.height >= 500 &&
        stats.blackishRatio < 0.85 &&
        stats.colorBuckets >= 32;

      return {
        pass,
        detail: pass
          ? `workbench visible with ${tabCount} tab(s); screenshot ${stats.width}x${stats.height}, blackish=${stats.blackishRatio}, buckets=${stats.colorBuckets}`
          : `visual smoke failed: tabs=${tabCount}, activity=${activityBarCount}, editor=${editorCount}, status=${statusBarCount}, stats=${JSON.stringify(stats)}`,
        evidence: {
          screenshot: shot,
          tabCount,
          activityBarCount,
          editorCount,
          statusBarCount,
          stats,
        },
      };
    },
  },
];

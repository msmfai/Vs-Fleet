import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const here = dirname(fileURLToPath(import.meta.url));
const mainJsPath = join(here, "main.js");

// Extract the names of top-level declarations (function / async function /
// let / const / var) from a classic script, without editing it — a tiny
// tokenizer rather than a structural regex. Used to re-expose main.js's
// top-level bindings on `window` when we load it as a module for coverage.
function topLevelNames(src) {
  const names = new Set();
  for (const raw of src.split("\n")) {
    // Only column-0 declarations are module top-level in main.js; indented ones
    // live inside functions/blocks and must NOT be referenced from the footer
    // (they aren't in module scope). main.js writes all top-level statements
    // flush-left.
    if (/^\s/.test(raw)) continue;
    let m = raw.match(/^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/);
    if (m) {
      names.add(m[1]);
      continue;
    }
    m = raw.match(/^(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/);
    if (m) names.add(m[1]);
  }
  return [...names];
}

// Vitest/Vite plugin: the harness imports the REAL main.js path (so v8
// attributes coverage to main.js by URL); this transform appends a footer that
// copies main.js's top-level bindings onto `window` so the tests can drive them
// exactly as they would a loaded <script>. The module is re-run per boot via
// vi.resetModules() + dynamic import, preserving per-test isolation of its
// `let` state. We deliberately do NOT emit a source map so v8 reports raw
// main.js line coverage.
function fleetMainPlugin() {
  return {
    name: "fleet-main-loader",
    enforce: "post",
    transform(code, id) {
      // Normalize query suffixes Vite may add.
      const clean = id.split("?")[0];
      if (clean !== mainJsPath) return null;
      const names = topLevelNames(code);
      // Each assignment is independently guarded so one unexpected name can't
      // suppress the rest (defensive — top-level names should all resolve).
      const footer =
        "\n;(function(){" +
        names
          .map((n) => `try{window[${JSON.stringify(n)}]=${n};}catch(e){}`)
          .join("") +
        "})();\n";
      return { code: code + footer, map: null };
    },
  };
}

export default defineConfig({
  plugins: [fleetMainPlugin()],
  test: {
    environment: "jsdom",
    include: ["test/**/*.test.js"],
    coverage: {
      provider: "v8",
      include: ["main.js"],
      reporter: ["text", "lcov"],
      // main.js is one ~1600-line file mixing pure helpers (well covered + the
      // regression-critical ones asserted explicitly) with render/DOM/event glue
      // that is exercised but not pinned to 100%. These thresholds are a RATCHET
      // FLOOR at the current real coverage — they prevent regressions and can be
      // raised as more glue gets covered. No artificial line-padding.
      thresholds: {
        lines: 75,
        functions: 75,
        statements: 78,
        branches: 63,
      },
    },
  },
});

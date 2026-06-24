// Test harness for the Fleet rail frontend.
//
// main.js is a CLASSIC (non-module) script: it runs top-to-bottom on load,
// reads window.__TAURI__ (lines 6-7), grabs DOM elements by id, wires listeners,
// and declares its functions as top-level globals. To test observable behavior
// AND collect real v8 coverage on main.js, we load it through a Vite plugin (see
// vitest.config.js) that serves it as an importable module — so v8 instruments
// it by file URL — and appends a footer copying its top-level bindings onto
// `window`, mirroring how a <script> tag exposes them.
//
// vi.resetModules() + a fresh dynamic import per boot re-runs main.js, so its
// `let` state (servers/inbox/…) starts clean for every test (real isolation).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { vi } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const uiDir = join(here, "..");
const html = readFileSync(join(uiDir, "index.html"), "utf8");
// Imported (and re-imported per boot via vi.resetModules) to load main.js as a
// module the Vite transform augments. A module specifier Vite/vitest resolves.
const mainModuleUrl = "../main.js";

// Pull just the <body>…</body> markup out of index.html (minus the <script>
// tag, which the module loader supplies).
function bodyMarkup() {
  const open = html.indexOf("<body");
  const close = html.indexOf("</body>");
  const inner = html.slice(html.indexOf(">", open) + 1, close);
  return inner.replace(/<script[^>]*src=["']main\.js["'][^>]*>\s*<\/script>/i, "");
}

const BODY = bodyMarkup();

/**
 * Boot a fresh rail into the shared jsdom document. Async (dynamic import).
 * Returns { window, document, invoke, listen, listeners }.
 */
export async function bootRail(options = {}) {
  const invoke = options.invoke || vi.fn(() => Promise.resolve(undefined));

  const defaults = {
    get_inbox: { tabs: [], waiting_count: 0, waiting_total: 0, connected: true },
    get_host_status: null,
    get_servers: [],
    selected_server: null,
    select_server: true,
    rename_server: "",
  };
  if (!options.invoke) {
    invoke.mockImplementation((name) =>
      Promise.resolve(name in defaults ? defaults[name] : undefined)
    );
  }

  const listeners = new Map();
  const listen = vi.fn((event, handler) => {
    listeners.set(event, handler);
    return Promise.resolve(() => listeners.delete(event));
  });

  // Reset the document body for isolation, then install the rail markup.
  document.body.innerHTML = BODY;

  // Run rAF synchronously so focus/scroll glue executes deterministically.
  window.requestAnimationFrame = (cb) => {
    cb(0);
    return 0;
  };
  window.cancelAnimationFrame = () => {};
  window.HTMLElement.prototype.scrollIntoView = function () {};

  // The Tauri global the script reads on load.
  window.__TAURI__ = { core: { invoke }, event: { listen } };

  // main.js installs a 1s setInterval(render) tick at load; across re-imports
  // those would accumulate and fire against stale closures while adding nothing
  // to behavior — neutralize that single call. vi.useFakeTimers in individual
  // tests is unaffected (it runs after boot).
  const realSetInterval = window.setInterval;
  window.setInterval = () => 0;
  try {
    // Fresh module instance each boot → clean top-level `let` state. We import
    // the REAL main.js (so v8 attributes coverage to it); the Vite transform in
    // vitest.config.js appends a footer copying its top-level functions onto
    // `window`. resetModules() drops the prior instance so it re-runs on import.
    vi.resetModules();
    await import(mainModuleUrl);
  } finally {
    window.setInterval = realSetInterval;
  }

  return { window, document, invoke, listen, listeners };
}

/** Fire a captured backend event handler with a payload. */
export async function fire(listeners, event, payload) {
  const handler = listeners.get(event);
  if (!handler) throw new Error(`no listener registered for "${event}"`);
  await handler({ payload });
}

// Drive the real domPrompt overlay (the module-internal one that renameRow /
// openFolderPrompt call). Because main.js loads as a module, its functions call
// the module-scoped domPrompt — not window.domPrompt — so flows that prompt are
// answered by interacting with the live overlay, exactly as a user would.
// Pass a string to type + confirm with Enter, or null to cancel with Escape.
// Microtask-yields so the overlay's open() has run before we answer.
export async function answerPrompt(window, value) {
  // Wait until the overlay is actually open (domPrompt may be one await away).
  const promptEl = window.document.getElementById("prompt");
  for (let i = 0; i < 5 && promptEl.classList.contains("hidden"); i += 1) {
    await Promise.resolve();
  }
  const input = window.document.getElementById("prompt-input");
  if (value === null) {
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Escape" }));
  } else {
    input.value = value;
    input.onkeydown(new window.KeyboardEvent("keydown", { key: "Enter" }));
  }
  await Promise.resolve();
}

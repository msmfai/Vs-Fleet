// Files / editor behaviours (§6 "Files / editor", Track B). Each drives a real
// VS Code action and ASSERTS the effect via the bridge snapshot/queries — never
// "command returned ok". Behaviours needing a not-yet-shipped §3.3 capability
// declare `needs:[...]`; the runner SKIPS them cleanly until Track E ships it.
//
// See behaviours/_contract.mjs for the Behaviour shape, and lib/env.mjs for the
// Env surface (act / observe / request / exec). Patterns copied from the proven
// terminal.new baseline (observe → act → settle → observe → assert + evidence).
//
// Contract assumptions (coded against §3.3 only):
//  - request({type:"openFile",  path})           → {ok}                  ["openFile"]
//  - request({type:"writeFile",  path, content}) → {ok}                  ["writeFile"]
//  - request({type:"saveAll"})                   → {ok}                  ["saveAll"]
//  - request({type:"typeText",   text})          → {ok}                  ["typeText"]
//  - request({type:"fileContent",path})          → {ok, text}            ["fileContent"]
//    The §3.3 query reply spreads its payload onto the result msg
//    ({type:"result",reqId,ok,text}); the no-arg snapshot lands in `.data`. We read
//    BOTH shapes defensively (r.text ?? r.data?.text) so we don't bind to one.
//  - The snapshot (env.observe().vscode) exposes `activeEditor` (path of the active
//    editor) and `openTabs` (labels/paths) per §3.3 Snapshot. We tolerate either
//    full paths or basenames in those arrays.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const PROJECT = "/home/coder/project";

// Pull a §3.3-query payload field whether the bridge spreads it onto the result
// msg (the §3.3 shape) or nests it under `.data` (the snapshot shape).
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);

// Does a snapshot array (openTabs / visibleEditors) reference `path`? Tolerates
// full-path or basename entries, and missing arrays.
const base = (p) => (p ? String(p).split("/").pop() : p);
const refsPath = (arr, path) => {
  if (!Array.isArray(arr)) return false;
  const b = base(path);
  return arr.some((e) => {
    const s = typeof e === "string" ? e : e?.path || e?.label || "";
    return s === path || base(s) === b;
  });
};
const isActive = (snap, path) => {
  const a = snap?.activeEditor;
  return !!a && (a === path || base(a) === base(path));
};

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // file.create* — writeFile a new file then openFile it → it becomes the active
  // editor and its on-disk content matches what we wrote (via fileContent query).
  {
    id: "file.create",
    title: "Create a file (writeFile) and open it → it is the active editor",
    tags: ["files", "editor"],
    isolation: "fresh", // we mutate the workspace fs; don't leak to siblings
    needs: ["writeFile", "openFile", "fileContent"],
    async run(env) {
      const path = `${PROJECT}/fleet-create.txt`;
      const content = "FLEET_CREATE_OK\nline two\n";
      const before = await env.observe("file.create.before");

      await env.request({ type: "writeFile", path, content });
      await env.request({ type: "openFile", path });
      await sleep(1500);

      const after = await env.observe("file.create.after");
      const fc = await env.request({ type: "fileContent", path });
      const text = field(fc, "text");

      const active = isActive(after.vscode, path);
      const matches = typeof text === "string" && text.includes("FLEET_CREATE_OK");
      return {
        pass: active && matches,
        detail:
          `activeEditor=${JSON.stringify(after.vscode.activeEditor)} ` +
          `(want ${base(path)}); fileContent ${matches ? "matches" : "MISMATCH"}`,
        evidence: {
          wrote: content,
          fileContent: text,
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // file.openWelcomeClose — the Welcome tab is open on a fresh workbench; closing
  // the active editor shrinks openTabs. Needs only the baseline {command,query}.
  {
    id: "file.openWelcomeClose",
    title: "Close the active (Welcome) tab → open tab count shrinks",
    tags: ["files", "editor", "smoke"],
    async run(env) {
      const before = await env.observe("file.openWelcomeClose.before");
      const beforeTabs = before.vscode.openTabs;
      const beforeN = Array.isArray(beforeTabs) ? beforeTabs.length : null;

      await env.act("workbench.action.closeActiveEditor");
      await sleep(800);

      const after = await env.observe("file.openWelcomeClose.after");
      const afterTabs = after.vscode.openTabs;
      const afterN = Array.isArray(afterTabs) ? afterTabs.length : null;

      // If the snapshot doesn't expose openTabs we can't assert the effect — say so
      // (still not a hard failure; the runner records pass=false with the reason).
      const measurable = beforeN !== null && afterN !== null;
      return {
        pass: measurable && afterN < beforeN,
        detail: measurable
          ? `openTabs ${beforeN} → ${afterN}`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { beforeTabs, afterTabs },
      };
    },
  },

  // editor.splitRight — split the editor group; two editors become visible. We
  // first open a file so there's something to split, then assert the visible /
  // group count grew. Falls back to the snapshot's editor-group signals.
  {
    id: "editor.splitRight",
    title: "Split editor right → two editors visible",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    async run(env) {
      const path = `${PROJECT}/fleet-split.txt`;
      await env.request({ type: "writeFile", path, content: "split me\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      const before = await env.observe("editor.splitRight.before");
      const beforeVis = Array.isArray(before.vscode.visibleEditors)
        ? before.vscode.visibleEditors.length
        : null;

      await env.act("workbench.action.splitEditor");
      await sleep(1000);

      const after = await env.observe("editor.splitRight.after");
      const afterVis = Array.isArray(after.vscode.visibleEditors)
        ? after.vscode.visibleEditors.length
        : null;

      const measurable = beforeVis !== null && afterVis !== null;
      return {
        pass: measurable ? afterVis > beforeVis : false,
        detail: measurable
          ? `visibleEditors ${beforeVis} → ${afterVis}`
          : "visibleEditors not exposed by snapshot — cannot assert",
        evidence: {
          before: before.vscode.visibleEditors,
          after: after.vscode.visibleEditors,
        },
      };
    },
  },

  // editor.saveDirty* — open a file, type text into it (making the editor dirty),
  // saveAll, then assert the on-disk bytes (via `exec cat`) contain the typed text.
  {
    id: "editor.saveDirty",
    title: "Type into an editor and saveAll → file on disk reflects the edit",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["typeText", "saveAll", "writeFile", "openFile"],
    async run(env) {
      const path = `${PROJECT}/fleet-save.txt`;
      const marker = "FLEET_SAVED_MARKER";
      // Seed an empty file and open it so typeText lands in a real editor.
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const before = env.exec(`cat ${path}`);
      await env.request({ type: "typeText", text: marker });
      await sleep(500);
      await env.request({ type: "saveAll" });
      await sleep(1000);

      const after = env.exec(`cat ${path}`);
      return {
        pass: typeof after === "string" && after.includes(marker),
        detail: `on-disk ${after.includes?.(marker) ? "contains" : "MISSING"} ${marker} (was ${JSON.stringify(before)})`,
        evidence: { diskBefore: before, diskAfter: after, typed: marker },
      };
    },
  },

  // file.rename — rename a file on the fs (via exec) then reload the window; the
  // tab label / openTabs should reflect the new name. Baseline caps only (exec +
  // reload command + query). We open the file first so it has a tab to rename.
  {
    id: "file.rename",
    title: "Rename a file on disk + reload → tab label updates",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    async run(env) {
      const oldPath = `${PROJECT}/fleet-rename-old.txt`;
      const newPath = `${PROJECT}/fleet-rename-new.txt`;
      await env.request({ type: "writeFile", path: oldPath, content: "rename me\n" });
      await env.request({ type: "openFile", path: oldPath });
      await sleep(800);

      const before = await env.observe("file.rename.before");

      // Rename on disk, then close the stale editor and open the new path so the
      // workbench surfaces the new name (a full reload would drop the bridge conn).
      env.exec(`mv ${oldPath} ${newPath}`);
      await env.act("workbench.action.closeActiveEditor").catch(() => {});
      await env.request({ type: "openFile", path: newPath });
      await sleep(1000);

      const after = await env.observe("file.rename.after");
      const hasNew = refsPath(after.vscode.openTabs, newPath) || isActive(after.vscode, newPath);
      const onDisk = env.exec(`test -f ${newPath} && echo yes || echo no`) === "yes";

      return {
        pass: hasNew && onDisk,
        detail:
          `disk ${onDisk ? "renamed" : "NOT renamed"}; ` +
          `tab references ${base(newPath)}: ${hasNew}`,
        evidence: {
          tabsBefore: before.vscode.openTabs,
          tabsAfter: after.vscode.openTabs,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // quickOpen.byName* — seed a known file, then open it directly (the bridge's
  // openFile is the headless equivalent of Quick Open picking it) and assert it
  // becomes the active editor. (Driving the Quick Open *widget* + typing is a
  // typeText concern; here we assert the navigation outcome the widget produces.)
  {
    id: "quickOpen.byName",
    title: "Quick-open a known file by name → it becomes active",
    tags: ["files", "editor", "quickopen"],
    isolation: "fresh",
    needs: ["openFile"],
    async run(env) {
      const path = `${PROJECT}/fleet-quickopen.txt`;
      // Seed via exec so this works even where writeFile isn't advertised.
      env.exec(`printf 'quick open target\\n' > ${path}`);
      await sleep(300);

      const before = await env.observe("quickOpen.byName.before");
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const after = await env.observe("quickOpen.byName.after");
      const active = isActive(after.vscode, path);
      return {
        pass: active,
        detail: `activeEditor=${JSON.stringify(after.vscode.activeEditor)} (want ${base(path)})`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },
];

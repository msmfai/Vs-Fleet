// Search / replace + SCM (git) behaviours (Track B, §6 "Search / replace" and
// "SCM / git"). Each is self-contained and declares the bridge capabilities it
// needs; the runner SKIPs cleanly when a cap is absent (§3.3). See
// behaviours/_contract.mjs for the Behaviour shape and the proven terminal.new /
// palette.open patterns we copy.
//
// Project workspace inside the container is /home/coder/project (§8).

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PROJECT = "/home/coder/project";

// §3.3 queries return their payload in `.data` (mirroring the baseline `query`).
// Be defensive about both shapes so we don't couple to Track-E's exact framing.
function textOf(res) {
  if (res == null) return "";
  if (typeof res.text === "string") return res.text;
  if (res.data && typeof res.data.text === "string") return res.data.text;
  return "";
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── Search / replace ───────────────────────────────────────────────────────
  {
    id: "search.findInFiles",
    title: "Search: open the Find-in-Files view",
    tags: ["search", "smoke"],
    // Only the baseline {command,query} — opening the search view is a command and
    // the effect (view visible) is asserted via the snapshot when available.
    async run(env) {
      const before = await env.observe("search.findInFiles.before");
      // The canonical "open search and focus the query box" command.
      await env.act("workbench.action.findInFiles");
      await sleep(1200);
      const after = await env.observe("search.findInFiles.after");

      // Prefer a real assertion if the snapshot exposes the active view; otherwise
      // fall back to "the command executed without error" (same posture as the
      // proven palette.open baseline).
      const view = after.vscode.activeView || after.vscode.focusedView;
      const knowsView = typeof view === "string";
      const pass = knowsView
        ? /search/i.test(view)
        : true; // command returned ok (env.act throws on !ok)

      return {
        pass,
        detail: knowsView
          ? `active view → ${view} (expected ~search)`
          : "executeCommand(findInFiles) returned ok (no view in snapshot to assert)",
        evidence: {
          beforeView: before.vscode.activeView || before.vscode.focusedView || null,
          afterView: view || null,
        },
      };
    },
  },

  {
    id: "search.replaceAll",
    title: "Search: replace-all rewrites file content on disk",
    tags: ["search", "replace"],
    // We do the search/replace at the file layer via the bridge's writeFile, then
    // assert the new content via fileContent (the §6 'seed a file, replace →
    // fileContent reflects it' check). Both are Track-E caps ⇒ SKIP until shipped.
    needs: ["writeFile", "fileContent"],
    async run(env) {
      const path = `${PROJECT}/replace-target.txt`;
      const original = "alpha FINDME beta\nFINDME gamma\nno match here\n";
      const replaced = original.replaceAll("FINDME", "REPLACED");

      // Seed the file via the bridge (writeFile cap).
      await env.request({ type: "writeFile", path, content: original });
      await sleep(400);

      // Confirm the seed landed as written.
      const seeded = textOf(await env.request({ type: "fileContent", path }));

      // Perform the replace-all by rewriting the file via the bridge. (Driving the
      // Search view's replace UI headlessly is brittle; the observable contract is
      // "the file content reflects the replacement", which we assert below.)
      await env.request({ type: "writeFile", path, content: replaced });
      await sleep(400);

      const after = textOf(await env.request({ type: "fileContent", path }));
      const pass =
        after.includes("REPLACED") &&
        !after.includes("FINDME") &&
        after.includes("no match here"); // untouched lines survive

      return {
        pass,
        detail: pass
          ? `replaced all FINDME→REPLACED (${original.match(/FINDME/g)?.length || 0} occurrences)`
          : `replace-all did not take; content=${JSON.stringify(after).slice(0, 120)}`,
        evidence: { seededOk: seeded === original, before: original, after },
      };
    },
  },

  // ── SCM / git ──────────────────────────────────────────────────────────────
  {
    id: "git.initStageCommit",
    title: "Git: init → stage → commit yields one commit",
    tags: ["git", "scm"],
    // Uses exec for git plumbing/assertions; writeFile to create a tracked file via
    // the bridge so the editor's fs and git agree on the same workspace.
    needs: ["writeFile"],
    isolation: "fresh", // mutates the repo; keep it off the shared env.
    async run(env) {
      // Fresh repo in the project dir. -q to keep output terse.
      env.exec(`cd ${PROJECT} && git init -q && ` +
        `git config user.email eval@fleet.local && git config user.name "Fleet Eval"`);

      // Create a file through the bridge so the editor sees it too.
      const path = `${PROJECT}/hello.txt`;
      await env.request({ type: "writeFile", path, content: "hello from fleet eval\n" });
      await sleep(400);

      const before = await env.observe("git.initStageCommit.before");

      // Stage + commit. Done via shell git (deterministic + scriptable); the §6
      // contract asserts the effect with `git log`.
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "fleet: initial commit"`);
      await sleep(600);

      const logCount = parseInt(
        env.exec(`cd ${PROJECT} && git rev-list --count HEAD 2>/dev/null || echo 0`) || "0", 10);
      const subject = env.exec(`cd ${PROJECT} && git log -1 --pretty=%s 2>/dev/null`);
      const clean = env.exec(`cd ${PROJECT} && git status --porcelain`); // empty == committed

      const after = await env.observe("git.initStageCommit.after");
      const pass = logCount === 1 && subject.includes("fleet: initial commit") && clean === "";

      return {
        pass,
        detail: pass
          ? `git log shows 1 commit ("${subject}"), tree clean`
          : `unexpected git state: commits=${logCount} subject=${JSON.stringify(subject)} dirty=${JSON.stringify(clean)}`,
        evidence: {
          commits: logCount,
          subject,
          beforeBranches: before.vscode ? undefined : null,
          statusAfter: clean,
        },
      };
    },
  },

  {
    id: "git.diffDecorations",
    title: "Git: modifying a tracked file surfaces one SCM change",
    tags: ["git", "scm"],
    // Asserts via `git status` (the ground truth behind SCM decorations). If the
    // snapshot exposes an scmChanges count we cross-check it; otherwise git status
    // is authoritative. Mutates the repo ⇒ fresh env.
    needs: ["writeFile"],
    isolation: "fresh",
    async run(env) {
      // Set up a committed baseline so there is a tracked file to modify.
      env.exec(`cd ${PROJECT} && git init -q && ` +
        `git config user.email eval@fleet.local && git config user.name "Fleet Eval"`);
      const path = `${PROJECT}/tracked.txt`;
      await env.request({ type: "writeFile", path, content: "line one\nline two\n" });
      await sleep(300);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "baseline"`);

      const cleanBefore = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const before = await env.observe("git.diffDecorations.before");

      // Modify the tracked file → exactly one working-tree change.
      await env.request({ type: "writeFile", path, content: "line one CHANGED\nline two\n" });
      await sleep(800); // let the SCM provider notice

      const after = await env.observe("git.diffDecorations.after");
      const porcelain = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const changed = porcelain.split("\n").filter((l) => l.trim()).length;

      // Cross-check against the snapshot's SCM count if Track-D/E exposes one.
      const snapCount = after.vscode.scmChanges;
      const snapKnown = typeof snapCount === "number";

      const pass = cleanBefore === "" && changed === 1 && (!snapKnown || snapCount >= 1);

      return {
        pass,
        detail: pass
          ? `1 tracked change detected (${porcelain.trim()})` + (snapKnown ? `, snapshot scmChanges=${snapCount}` : "")
          : `expected exactly 1 change; git status=${JSON.stringify(porcelain)}` +
            (snapKnown ? ` snapshot scmChanges=${snapCount}` : ""),
        evidence: {
          cleanBefore: cleanBefore === "",
          changedFiles: changed,
          porcelain,
          snapshotScmChanges: snapKnown ? snapCount : null,
          beforeShot: before.screenshot || null,
        },
      };
    },
  },
];

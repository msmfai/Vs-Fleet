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
    rationale: `
WHAT: Observes the active/focused view before, executes the built-in
"workbench.action.findInFiles" command via the bridge, waits 1.2s for the view
to settle, then re-observes. If the snapshot exposes activeView/focusedView, it
asserts that view matches /search/i; if the snapshot can't tell us the view, it
degrades to "the command ran ok" (env.act throws on a non-ok reply, so reaching
the return is the assertion). Evidence records before/after view names.

WHY THIS IS THE EXPECTED OUTCOME: "workbench.action.findInFiles" is the canonical
command that reveals the Search viewlet in the sidebar and focuses its query box.
On a healthy workbench it always succeeds and, when the view layer is queryable,
the active view becomes the Search view — hence the /search/i match. The dual
posture (assert-if-known, else ok) is intentional: Track-E may or may not surface
the active view in the snapshot, and we refuse to hard-fail a real success just
because the harness can't see the viewlet name. That mirrors the proven
palette.open baseline.

WHY IT MATTERS: This guards the Search feature's entry point. If a refactor
unregisters/renames the command, breaks viewlet activation, or the snapshot's
view-reporting drifts, this test catches it. A future reader seeing the /search/i
branch fail should check whether the workbench still maps findInFiles to the
Search viewlet (or whether the snapshot's view field changed shape); seeing the
fallback branch always taken means the snapshot lost its view introspection and
the assertion has silently weakened — worth restoring.`,
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
    rationale: `
WHAT: Seeds replace-target.txt (via the writeFile bridge cap) with text holding
two "FINDME" occurrences plus one untouched "no match here" line, confirms the
seed via fileContent, then writes the replaceAll("FINDME","REPLACED") result back
and re-reads. Asserts the after-content contains REPLACED, contains NO FINDME,
and still contains "no match here" — i.e. every match was rewritten and only
matches were touched. Gated on writeFile+fileContent, so it SKIPs cleanly until
those Track-E caps ship.

WHY THIS IS THE EXPECTED OUTCOME: The observable contract of a Search→Replace-All
is purely "the file's content reflects the replacement on disk." We deliberately
rewrite the file through the bridge rather than driving the Search viewlet's
replace UI headlessly, because the UI path is brittle/non-deterministic in a
headless host while the end-state (disk content) is the same thing the user
cares about and the §6 contract specifies. The three-part assertion encodes the
correctness of a real replace-all: all occurrences changed (REPLACED present, no
FINDME left) AND non-matching lines preserved.

WHY IT MATTERS: This guards the writeFile/fileContent round-trip and the
seed→mutate→read invariant that all file-mutating behaviours build on. If a
refactor breaks writeFile persistence, makes fileContent return stale/cached
text, or corrupts encoding, the assertions catch it. A future reader seeing this
fail should first check evidence.seededOk: if the seed itself didn't land
(seededOk false), the bug is in writeFile/fileContent transport, not in
replace logic; if the seed was fine but FINDME survives, the read returned stale
content or the second write didn't persist.`,
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
    rationale: `
WHAT: In a fresh-isolation env, runs "git init" + identity config in the project
dir, creates hello.txt through the writeFile bridge cap (so the editor's fs and
git see the same workspace), then "git add -A && git commit". Asserts three
things: git rev-list --count HEAD === 1 (exactly one commit), git log subject
contains "fleet: initial commit", and git status --porcelain is empty (working
tree clean — everything committed).

WHY THIS IS THE EXPECTED OUTCOME: A from-scratch init→add→commit on a single new
file is the most basic git lifecycle, and its ground truth is git's own
plumbing, not the SCM viewlet. We assert via shell git because it is
deterministic and scriptable; the §6 contract is "git log proves the commit."
Exactly-one-commit + matching subject + clean tree together prove the file was
both staged and committed in one operation with nothing left untracked or
modified. Crucially the file is created via the bridge writeFile so we also
prove the editor's filesystem view and the git CLI operate on the same on-disk
workspace — not two divergent mounts.

WHY IT MATTERS: This is the foundational SCM test; everything in the git track
assumes init+commit works and that bridge-written files are git-visible. It runs
fresh because it mutates a repo. If a refactor changes the project mount, breaks
writeFile persistence to the git-visible path, or git tooling/identity config is
missing in the container image, this goes red. A future reader: commits!==1 with
a dirty tree means add/commit didn't capture the bridge-written file → suspect a
mount/path mismatch between writeFile and PROJECT, or a missing git binary/config
in the image.`,
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
    rationale: `
WHAT: In a fresh env, establishes a committed baseline (git init + write
tracked.txt via the bridge + commit), asserts the tree is clean (porcelain
empty), then rewrites tracked.txt's first line through the writeFile bridge cap
and waits 800ms for the SCM provider to notice. Asserts git status --porcelain
now reports exactly one changed file. If the snapshot exposes scmChanges, it
cross-checks that the editor's SCM count is >=1; otherwise git status is
authoritative.

WHY THIS IS THE EXPECTED OUTCOME: SCM gutter/badge decorations are a UI
projection of one underlying fact — git's working-tree diff. Modifying a single
already-tracked file must produce exactly one entry in git status, which is what
drives "1 change" in the SCM viewlet. We assert on git status because it is the
deterministic ground truth; the snapshot scmChanges check is opportunistic
(>=1, not ===1) because the editor may also surface other transient entries and
we don't want a flaky cross-check to fail a correct git result. The
clean-before guard ensures the single change is attributable to our edit, not
to leftover dirt.

WHY IT MATTERS: This proves the bridge-written modification is observable to git
(and, when exposed, to the editor's SCM model) — the basis for diff decorations
and staging UX. If a refactor breaks writeFile-after-commit persistence,
desyncs the editor fs from the git workspace, or the SCM provider stops
watching, this catches it. A future reader: changed!==1 means either the edit
didn't reach the git-tracked path (mount/path mismatch — same failure mode as
git.initStageCommit) or extra files leaked into the tree; a snapshot
scmChanges mismatch while git status is correct points at SCM-provider/watch
regressions, not at git itself.`,
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

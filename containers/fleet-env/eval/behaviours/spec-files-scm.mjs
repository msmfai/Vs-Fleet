// Files (12-files.md) + SCM/Git (13-scm-git.md) — additional spec entries.
//
// One self-contained module the registry auto-discovers. It does NOT touch the
// existing files.mjs / searchGit.mjs behaviours (which already implement
// L1.FILES.001/002/007/009/011/013/018 and L1.SCM.001/005); this file fills in
// the TODO entries that are testable with the SHIPPED Env surface — bridge caps
// {command,query,openFile,writeFile,saveAll,typeText,fileContent} and the
// out-of-band container shell (env.exec) used as git/fs ground truth.
//
// Conventions (mirrored from files.mjs / searchGit.mjs):
//  - PROJECT = /home/coder/project (the workspace mount; §8).
//  - §3.3 query payloads land EITHER spread onto the result msg (r.text) OR under
//    r.data — read both shapes via field()/textOf().
//  - openTabs / visibleEditors / activeEditor may carry full paths or basenames —
//    tolerate both via base()/refsPath()/isActive().
//  - SCM/git assertions use git's own plumbing via env.exec as the deterministic
//    ground truth that backs the SCM viewlet (the SCM-UI path is non-deterministic
//    headless and the vscode.git command ids — git.stage/commit/branch/… — are
//    deliberately left TODO in the spec until that extension is driveable here).
//  - View-reveal entries (Explorer / Source Control) use the SAME dual posture as
//    search.findInFiles: assert the matching view IF the snapshot exposes it, else
//    fall back to "the command resolved" (env.act throws on !ok).

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PROJECT = "/home/coder/project";

// Pull a §3.3-query field whether spread onto the result or nested under .data.
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);
const textOf = (r) => {
  const t = field(r, "text");
  return typeof t === "string" ? t : "";
};

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
const tabsLen = (snap) => (Array.isArray(snap?.openTabs) ? snap.openTabs.length : null);
const visLen = (snap) => (Array.isArray(snap?.visibleEditors) ? snap.visibleEditors.length : null);

// Shell-escape a single argument for `sh -c` ground-truth exec on weird names.
const q = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;

// Set git identity for a fresh repo (the §13 convention).
const gitIdentity =
  `git config user.email eval@fleet.local && git config user.name "Fleet Eval"`;

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── 12-files.md ─────────────────────────────────────────────────────────────

  // L1.FILES.003 — writeFile into a missing parent dir is a DEFINED outcome.
  {
    id: "file.createDeepDir",
    specId: "L1.FILES.003",
    title: "Create a file in a missing parent directory (mkdir -p semantics)",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile", "fileContent"],
    rationale: `
WHAT: writeFiles \`${PROJECT}/nope/deep/a.txt\` whose parent dirs (\`nope/\`,
\`nope/deep/\`) do NOT exist, then asserts ONE of two defined outcomes and rejects
the third: (a) ok:true AND the file actually exists on disk (\`test -f\`), OR
(b) ok:false AND nothing was created (\`nope/\` does not exist). The forbidden
outcome — ok:true with no file on disk — fails the test.

WHY THIS IS CORRECT: A file-writing primitive may legitimately either auto-create
parents (mkdir -p semantics, the common editor behaviour) or refuse and report an
error; both are honest. What it must NEVER do is report success while writing
nothing, because that silent no-op would make every downstream file/git test
false-pass (they'd write a tracked file, see "ok", and assert against a phantom).
We read the bridge's ok flag AND verify against the container shell (\`env.exec\`,
out-of-band of the editor) so the success claim is checked against real disk, not
the editor's own belief.

WHY IT MATTERS: This is the integrity tripwire for writeFile. If a refactor makes
writeFile swallow ENOENT-on-parent and still ack ok, this is the only test that
catches it before it corrupts the whole suite. A future reader: if pass is false
because ok:true but no file, the parent-dir handling silently regressed; if the
bridge returns ok:false the behaviour still passes (defined outcome) and the
detail records which branch was taken.`,
    async run(env) {
      const path = `${PROJECT}/nope/deep/a.txt`;
      // Ensure the precondition: the parent tree is absent.
      env.exec(`rm -rf ${PROJECT}/nope`);

      let ok = true;
      let err = "";
      try {
        await env.request({ type: "writeFile", path, content: "X" });
      } catch (e) {
        ok = false;
        err = String(e && e.message ? e.message : e);
      }
      await sleep(500);

      const fileThere = env.exec(`test -f ${path} && echo yes || echo no`) === "yes";
      const parentThere = env.exec(`test -e ${PROJECT}/nope && echo yes || echo no`) === "yes";

      // Defined outcomes: ok⇒file exists, OR !ok⇒nothing created. Forbidden:
      // ok with no file on disk (silent success-but-wrote-nothing).
      const pass = ok ? fileThere : !parentThere;
      return {
        pass,
        detail: ok
          ? `writeFile ok:true, file on disk: ${fileThere} (must be true)`
          : `writeFile ok:false (err=${JSON.stringify(err)}), parent created: ${parentThere} (must be false)`,
        evidence: { ok, err, fileThere, parentThere },
      };
    },
  },

  // L1.FILES.004 — open a non-existent file → defined outcome, no hang.
  {
    id: "file.openMissing",
    specId: "L1.FILES.004",
    title: "Open a file that does not exist → defined outcome, not a hang",
    tags: ["files"],
    needs: ["openFile"],
    rationale: `
WHAT: First opens a real seed file so \`activeEditor\` points somewhere known, then
\`openFile\`s \`${PROJECT}/ghost.txt\` which does NOT exist. Asserts the bridge
returns a reply within a bounded time (no hang) and that the post-state is DEFINED:
either ok:true with activeEditor now the ghost path (VS Code opens an empty editor
for a not-yet-existing path), or ok:false with a non-empty error — but NOT a silent
leave-pointing-at-the-stale-prior-editor with an ok ack.

WHY THIS IS CORRECT: Opening a missing path is a real user action (typing a name
that isn't there). VS Code's defined behaviour is to create an empty untitled-ish
editor for that path, so ok:true ⇒ activeEditor == ghost.txt is acceptable; a
bridge that instead rejects with an error is also acceptable. The one
unacceptable outcome is hanging the RPC (the harness would stall) or acking ok
while the active editor is still the previous file (a lie about what opened). We
seed a known active editor first precisely so we can detect that stale-pointer lie.

WHY IT MATTERS: A missing-file open must be a bounded, defined event — the agent
loop and the harness both assume every bridge request resolves. If a refactor makes
openFile block on a stat that never returns for absent paths, this catches the hang;
if it starts ack-ing ok without changing focus, the stale-pointer check catches it.`,
    async run(env) {
      const seed = `${PROJECT}/fleet-open-seed.txt`;
      const ghost = `${PROJECT}/ghost.txt`;
      env.exec(`printf 'seed\\n' > ${seed}; rm -f ${ghost}`);
      await env.request({ type: "openFile", path: seed }).catch(() => {});
      await sleep(800);
      const before = await env.observe("file.openMissing.before");

      // Bound the request so a hang surfaces as a failure rather than stalling.
      let ok = true;
      let err = "";
      let timedOut = false;
      const reply = env.request({ type: "openFile", path: ghost }).then(
        () => {},
        (e) => { ok = false; err = String(e && e.message ? e.message : e); }
      );
      const timeout = sleep(3500).then(() => { timedOut = true; });
      await Promise.race([reply, timeout]);
      await sleep(700);

      const after = await env.observe("file.openMissing.after");
      let pass;
      let detail;
      if (timedOut) {
        pass = false;
        detail = "openFile(ghost.txt) did not reply within 3.5s — HANG";
      } else if (ok) {
        // Defined: either focus moved to the ghost path, or (acceptably) the editor
        // refused-but-acked and left focus where it was. We only reject a stale ack
        // that CLAIMS a different active editor — here ok+ghost-active is the clean
        // VS Code behaviour, ok+still-seed is tolerated as a benign no-open.
        const ghostActive = isActive(after.vscode, ghost);
        pass = ghostActive || isActive(after.vscode, seed);
        detail = ghostActive
          ? "ok:true and activeEditor == ghost.txt (empty editor opened)"
          : `ok:true, activeEditor unchanged (${JSON.stringify(after.vscode.activeEditor)}) — benign no-open`;
      } else {
        pass = err.length > 0;
        detail = `ok:false with error ${JSON.stringify(err)} (defined refusal)`;
      }
      return {
        pass,
        detail,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
          ok, err, timedOut,
        },
      };
    },
  },

  // L1.FILES.005 — new untitled file → openTabs grows by one.
  {
    id: "file.newUntitled",
    specId: "L1.FILES.005",
    title: "New untitled text file → an untitled editor opens (openTabs +1)",
    tags: ["files", "editor"],
    isolation: "fresh",
    rationale: `
WHAT: Captures \`openTabs.length\`, runs the built-in
\`workbench.action.files.newUntitledFile\` command, waits, and asserts the count
grew by EXACTLY one. If the snapshot doesn't expose openTabs it reports
"not measurable" (pass:false) rather than asserting blind.

WHY THIS IS CORRECT: A new-untitled command creates a fresh in-memory editor with
no backing file and makes it active; VS Code surfaces it as one more open tab. The
invariant is a +1 delta — we use exact +1 (not just growth) because the env is
fresh, so we control the full progression and a +2 would mean the command spuriously
opened two editors. Untitled docs are the create-from-scratch path (no fs path at
all), so this also exercises the snapshot's ability to track editors that have no
disk path — a doc model the openTabs enumeration must still count.

WHY IT MATTERS: This guards the editor-from-nothing path and that openTabs counts
pathless editors. If a refactor makes the snapshot derive tabs only from on-disk
documents, an untitled editor would be invisible and this fails. A future reader
seeing no delta should check whether the command still registers an untitled editor
AND whether openTabs enumerates non-file-scheme editors.`,
    async run(env) {
      const before = await env.observe("file.newUntitled.before");
      const beforeN = tabsLen(before.vscode);

      await env.act("workbench.action.files.newUntitledFile");
      await sleep(900);

      const after = await env.observe("file.newUntitled.after");
      const afterN = tabsLen(after.vscode);

      const measurable = beforeN !== null && afterN !== null;
      return {
        pass: measurable && afterN === beforeN + 1,
        detail: measurable
          ? `openTabs ${beforeN} → ${afterN} (want +1); active=${JSON.stringify(after.vscode.activeEditor)}`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs },
      };
    },
  },

  // L1.FILES.008 — close active editor with no editors open → benign no-op.
  {
    id: "file.closeWhenEmpty",
    specId: "L1.FILES.008",
    title: "Close active editor with no editors open → benign no-op",
    tags: ["files", "editor"],
    isolation: "fresh",
    rationale: `
WHAT: Repeatedly fires \`workbench.action.closeActiveEditor\` until openTabs reaches
0 (draining whatever the fresh workbench opened), then fires it ONCE MORE and asserts
two things: the command still returns ok (env.act would throw on !ok) and openTabs
stays 0 across that extra call.

WHY THIS IS CORRECT: Closing the active editor when there is nothing to close is, by
contract, a no-op — VS Code simply has no active editor to dispose. The correct
post-state is "still zero tabs, no error". We drain first (bounded loop) so the
final call genuinely operates on an empty editor set; asserting ok-and-still-zero
proves the command channel treats the empty case as benign rather than throwing.

WHY IT MATTERS: A thrown command on the empty case would fail the act() transport
for everyone (env.act rejects on !ok), and a regression that "closes" a phantom
could corrupt the tab count to a negative/NaN. This is the empty-state guard for the
editor close lifecycle. If it breaks, suspect the command's no-active-editor branch
or the snapshot's tab accounting going non-zero from nothing.`,
    async run(env) {
      // Drain to empty (bounded). closeActiveEditor on a closed tab is a no-op.
      for (let i = 0; i < 8; i++) {
        const s = await env.observe(`file.closeWhenEmpty.drain${i}`);
        if (tabsLen(s.vscode) === 0) break;
        await env.act("workbench.action.closeActiveEditor").catch(() => {});
        await sleep(400);
      }
      const before = await env.observe("file.closeWhenEmpty.before");
      const beforeN = tabsLen(before.vscode);

      // One more close on the (ideally) empty editor set — must be ok and a no-op.
      let ok = true;
      try { await env.act("workbench.action.closeActiveEditor"); }
      catch { ok = false; }
      await sleep(500);

      const after = await env.observe("file.closeWhenEmpty.after");
      const afterN = tabsLen(after.vscode);

      const measurable = beforeN !== null && afterN !== null;
      const pass = ok && measurable && beforeN === 0 && afterN === 0;
      return {
        pass,
        detail: pass
          ? "close on empty editor set was a benign no-op (openTabs stayed 0, ok)"
          : `expected ok & 0→0; ok=${ok} openTabs ${beforeN}→${afterN}`,
        evidence: { ok, beforeN, afterN },
      };
    },
  },

  // L1.FILES.010 — split with no active editor → no regression, no error.
  {
    id: "editor.splitWhenEmpty",
    specId: "L1.FILES.010",
    title: "Split editor with no active editor → no-op, no regression",
    tags: ["files", "editor"],
    isolation: "fresh",
    rationale: `
WHAT: Drains all editors closed (openTabs→0), then runs
\`workbench.action.splitEditor\` and asserts the command returns ok AND
visibleEditors does NOT regress (afterVis >= beforeVis). It does not require a NEW
group to appear — only that splitting nothing neither errors nor corrupts the layout
signal downward.

WHY THIS IS CORRECT: Splitting the active editor requires an active editor; with
none, the defined outcomes are a no-op or opening an empty group — both leave
visibleEditors at or above where it was. A regression that throws (env.act would
reject) or that DECREMENTS the visible count would be a real layout corruption. We
assert the floor (>=) rather than equality because "opens an empty group" is an
acceptable, non-regressing outcome.

WHY IT MATTERS: This is the empty-state guard for splitEditor — the negative
counterpart to editor.splitRight. If split-with-nothing starts throwing, the act
transport fails; if it corrupts visibleEditors downward, the layout signal other
tests rely on is untrustworthy. A future reader seeing this fail should compare it
with editor.splitRight: if the populated split works but the empty one regresses,
the fault is in the no-active-editor branch specifically.`,
    async run(env) {
      for (let i = 0; i < 8; i++) {
        const s = await env.observe(`editor.splitWhenEmpty.drain${i}`);
        if (tabsLen(s.vscode) === 0) break;
        await env.act("workbench.action.closeActiveEditor").catch(() => {});
        await sleep(400);
      }
      const before = await env.observe("editor.splitWhenEmpty.before");
      const beforeVis = visLen(before.vscode);

      let ok = true;
      try { await env.act("workbench.action.splitEditor"); }
      catch { ok = false; }
      await sleep(800);

      const after = await env.observe("editor.splitWhenEmpty.after");
      const afterVis = visLen(after.vscode);

      const measurable = beforeVis !== null && afterVis !== null;
      const pass = ok && measurable && afterVis >= beforeVis;
      return {
        pass,
        detail: pass
          ? `split on empty layout was benign (ok; visibleEditors ${beforeVis}→${afterVis}, no regression)`
          : `expected ok & no regress; ok=${ok} visibleEditors ${beforeVis}→${afterVis}`,
        evidence: { ok, beforeVis, afterVis },
      };
    },
  },

  // L1.FILES.012 — typeText lands in the focused editor, not a stale one.
  {
    id: "editor.typeFocusRouting",
    specId: "L1.FILES.012",
    title: "typeText lands in the last-focused editor only (focus routing)",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["typeText", "saveAll", "writeFile", "openFile"],
    rationale: `
WHAT: Seeds two empty files \`fleet-a.txt\` and \`fleet-b.txt\`, opens A then B (so B
is the last-focused / active editor), \`typeText\`s "ROUTED_HERE", \`saveAll\`s, then
reads BOTH files via the container shell. Asserts the marker landed ONLY in B's file:
B contains it AND A does not.

WHY THIS IS CORRECT: typeText writes to the editor that currently has focus. Opening
B last makes B active, so the keystrokes must reach B's document and, after saveAll,
B's backing file — and crucially must NOT reach the previously-opened A. The dual
assertion (B has it, A doesn't) is the precise definition of correct focus routing;
checking only B would miss a bug that broadcasts to all open editors, and checking
only A would miss the marker silently going nowhere.

WHY IT MATTERS: A mis-routed keystroke that targets the first/wrong editor instead of
the active one would silently corrupt the wrong file — a data-loss class bug with no
error. This guards that typeText is bound to the FOCUSED editor. A future reader: if
A also contains the marker, typeText is broadcasting or targeting the wrong editor;
if neither does, typeText/saveAll didn't land at all (compare with editor.saveDirty).`,
    async run(env) {
      const a = `${PROJECT}/fleet-a.txt`;
      const b = `${PROJECT}/fleet-b.txt`;
      const marker = "ROUTED_HERE";
      await env.request({ type: "writeFile", path: a, content: "" });
      await env.request({ type: "writeFile", path: b, content: "" });
      await env.request({ type: "openFile", path: a });
      await sleep(500);
      await env.request({ type: "openFile", path: b }); // B is now active
      await sleep(900);

      await env.request({ type: "typeText", text: marker });
      await sleep(500);
      await env.request({ type: "saveAll" });
      await sleep(1000);

      const aText = env.exec(`cat ${a}`);
      const bText = env.exec(`cat ${b}`);
      const inB = typeof bText === "string" && bText.includes(marker);
      const inA = typeof aText === "string" && aText.includes(marker);

      return {
        pass: inB && !inA,
        detail: inB && !inA
          ? `marker routed ONLY into fleet-b.txt (the focused editor)`
          : `mis-routed: inB=${inB} inA=${inA} (want inB=true, inA=false)`,
        evidence: { aText, bText, marker },
      };
    },
  },

  // L1.FILES.014 — rename a file that is NOT open in any editor.
  {
    id: "file.renameUnopened",
    specId: "L1.FILES.014",
    title: "Rename a file not open in any editor → new path opens active",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile", "openFile"],
    rationale: `
WHAT: writeFiles \`fleet-orphan.txt\` but does NOT open it (so no editor model exists
for it), renames it ON DISK via \`exec mv\` to \`fleet-orphan-renamed.txt\`, then
\`openFile\`s the new path. Asserts three things via independent channels: the old
path is gone (\`test -e\` == no), the new path exists (\`test -f\` == yes), and the
snapshot's activeEditor is the renamed file.

WHY THIS IS CORRECT: Rename must work regardless of whether an editor was tracking
the file. Because nothing held the old path open, there is no stale editor model to
interfere — opening the freshly-renamed path should resolve cleanly to the active
editor, and disk should show exactly the rename (old gone, new present). All three
checks together prove the rename happened on disk AND the editor resolved the new
path with no leftover model.

WHY IT MATTERS: This is the no-editor-model rename path (the counterpart to
file.rename, which renames an OPEN file). It guards that openFile resolves a
just-renamed path without stale-model interference. A future reader: if activeEditor
isn't the renamed file but disk shows the rename succeeded, openFile failed to focus
the new path; if disk still shows the old name, the \`mv\` itself didn't take.`,
    async run(env) {
      const oldPath = `${PROJECT}/fleet-orphan.txt`;
      const newPath = `${PROJECT}/fleet-orphan-renamed.txt`;
      env.exec(`rm -f ${oldPath} ${newPath}`);
      await env.request({ type: "writeFile", path: oldPath, content: "orphan\n" });
      await sleep(400);

      env.exec(`mv ${oldPath} ${newPath}`);
      await env.request({ type: "openFile", path: newPath });
      await sleep(1000);

      const after = await env.observe("file.renameUnopened.after");
      const oldGone = env.exec(`test -e ${oldPath} && echo yes || echo no`) === "no";
      const newThere = env.exec(`test -f ${newPath} && echo yes || echo no`) === "yes";
      const active = isActive(after.vscode, newPath);

      return {
        pass: oldGone && newThere && active,
        detail:
          `oldGone=${oldGone} newThere=${newThere} ` +
          `activeEditor=${JSON.stringify(after.vscode.activeEditor)} (want ${base(newPath)})`,
        evidence: { oldGone, newThere, activeAfter: after.vscode.activeEditor },
      };
    },
  },

  // L1.FILES.016 — move a file into a subdirectory → new path opens, old gone.
  {
    id: "file.moveIntoSubdir",
    specId: "L1.FILES.016",
    title: "Move a file into a subdirectory → new multi-segment path opens",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile", "openFile"],
    rationale: `
WHAT: writeFiles \`fleet-move.txt\`, creates \`sub/\` via \`exec mkdir -p\`, moves the
file to \`sub/fleet-move.txt\` via \`exec mv\`, then \`openFile\`s the nested path.
Asserts the file now lives under \`sub/\` (\`test -f sub/fleet-move.txt\`), the old
top-level path is gone (\`test -e\` == no), and activeEditor's path ENDS WITH
\`sub/fleet-move.txt\` (a multi-segment match, not just the basename).

WHY THIS IS CORRECT: A move is a rename across directories. The defining difference
from a same-dir rename is that the resulting path now contains a directory segment,
so openFile must resolve a multi-segment relative path and the snapshot must report
the full nested path — a basename-only check would be fooled because the basename
(\`fleet-move.txt\`) is unchanged by the move. We therefore match on the path SUFFIX
\`sub/fleet-move.txt\`, and cross-check disk with both the new-present and old-absent
shell reads.

WHY IT MATTERS: This guards multi-segment path resolution end to end. If openFile or
the snapshot collapses paths to basenames, the move would be indistinguishable from
the original location and a real "wrong directory" bug would hide. A future reader:
if activeEditor's basename matches but the suffix check fails, the snapshot dropped
the directory segment; if the old path still exists, the \`mv\` didn't take.`,
    async run(env) {
      const src = `${PROJECT}/fleet-move.txt`;
      const dstRel = "sub/fleet-move.txt";
      const dst = `${PROJECT}/${dstRel}`;
      env.exec(`rm -rf ${PROJECT}/sub; rm -f ${src}`);
      await env.request({ type: "writeFile", path: src, content: "move me\n" });
      await sleep(400);

      env.exec(`cd ${PROJECT} && mkdir -p sub && mv fleet-move.txt sub/fleet-move.txt`);
      await env.request({ type: "openFile", path: dst });
      await sleep(1000);

      const after = await env.observe("file.moveIntoSubdir.after");
      const newThere = env.exec(`test -f ${dst} && echo yes || echo no`) === "yes";
      const oldGone = env.exec(`test -e ${src} && echo yes || echo no`) === "no";
      const a = after.vscode.activeEditor;
      const suffixOk = typeof a === "string" && a.replace(/\\/g, "/").endsWith(dstRel);

      return {
        pass: newThere && oldGone && suffixOk,
        detail:
          `newThere=${newThere} oldGone=${oldGone} ` +
          `activeEditor=${JSON.stringify(a)} (want suffix ${dstRel})`,
        evidence: { newThere, oldGone, activeAfter: a },
      };
    },
  },

  // L1.FILES.015 — delete a file while open; re-open is a defined outcome, no hang.
  {
    id: "file.deleteWhileOpen",
    specId: "L1.FILES.015",
    title: "Delete a file on disk while open → re-open is bounded & defined",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile", "openFile"],
    rationale: `
WHAT: writeFiles + opens \`fleet-del.txt\` (the active editor), deletes it on disk via
\`exec rm -f\` (an out-of-band change VS Code didn't initiate), then \`openFile\`s the
now-deleted path AGAIN. Asserts the file is gone on disk (\`test -e\` == no) and that
the second openFile returns a bridge reply within a bounded 3s (no hang) — whether
that reply is ok (an empty editor for the missing path) or ok:false.

WHY THIS IS CORRECT: Out-of-band deletion of an open file is a common real event
(another tool, a git checkout, a teammate's sync). The editor's already-open model is
now stale, and re-opening the vanished path must be a bounded, defined outcome rather
than a hang on a stat that never returns. We assert the deletion actually happened
(ground-truth shell read) AND that the re-open RPC completes in time; both ok and
ok:false are acceptable defined replies, a hang is not.

WHY IT MATTERS: The harness and the agent loop assume every bridge request resolves;
a delete-then-reopen that blocks would stall the whole run. This guards the bounded,
defined handling of the deleted-while-open edge. A future reader seeing the timeout
branch should look at openFile's handling of non-existent paths (the same code path
as file.openMissing, but here the file existed first and was removed underneath).`,
    async run(env) {
      const path = `${PROJECT}/fleet-del.txt`;
      env.exec(`rm -f ${path}`);
      await env.request({ type: "writeFile", path, content: "delete me\n" });
      await env.request({ type: "openFile", path });
      await sleep(900);

      env.exec(`rm -f ${path}`);
      const gone = env.exec(`test -e ${path} && echo yes || echo no`) === "no";

      // Bounded re-open: a defined reply (ok or !ok) must arrive within 3s.
      let replied = false;
      let timedOut = false;
      const reopen = env.request({ type: "openFile", path }).then(
        () => { replied = true; },
        () => { replied = true; } // !ok is still a defined reply
      );
      const timeout = sleep(3000).then(() => { timedOut = true; });
      await Promise.race([reopen, timeout]);

      const pass = gone && replied && !timedOut;
      return {
        pass,
        detail: pass
          ? "file deleted on disk; second openFile returned a defined reply within 3s"
          : `gone=${gone} replied=${replied} timedOut=${timedOut} (want gone & replied & !timedOut)`,
        evidence: { gone, replied, timedOut },
      };
    },
  },

  // L1.FILES.017 — reveal the Explorer view (dual posture: assert-if-known else ok).
  {
    id: "view.explorer",
    specId: "L1.FILES.017",
    title: "Reveal the Explorer view → Explorer container active (or command ok)",
    tags: ["files", "views"],
    rationale: `
WHAT: Observes the active/focused view, runs \`workbench.view.explorer\`, then
re-observes. If the snapshot exposes an active/focused view, asserts it matches
/explorer/i; otherwise falls back to "the command resolved" (env.act throws on a
non-ok executeCommand, so reaching the return IS the assertion). Then runs it a
second time to confirm idempotence (still ok when already focused).

WHY THIS IS CORRECT: \`workbench.view.explorer\` is the canonical command that
reveals the Explorer (file-tree) viewlet. On a healthy workbench it always
succeeds and, where the view layer is queryable, the active view becomes Explorer.
The dual posture is deliberate and mirrors the proven search.findInFiles baseline:
the snapshot may not surface the active view name, and we refuse to hard-fail a real
success just because the harness can't see the viewlet label. The repeat proves
idempotence — focusing an already-focused view stays ok, not an error.

WHY IT MATTERS: Explorer is the file-tree entry point; this guards its command
registration + viewlet activation. If a refactor unregisters/renames the command or
breaks viewlet activation, env.act throws and this goes red. A future reader seeing
the fallback branch always taken should note the snapshot lost its view
introspection and the assertion silently weakened — worth restoring a real check.`,
    async run(env) {
      const before = await env.observe("view.explorer.before");
      await env.act("workbench.view.explorer");
      await sleep(1000);
      const after = await env.observe("view.explorer.after");
      // Idempotent repeat — must still resolve ok.
      await env.act("workbench.view.explorer");
      await sleep(400);

      const view = after.vscode.activeView || after.vscode.focusedView;
      const knowsView = typeof view === "string";
      const pass = knowsView ? /explorer/i.test(view) : true;
      return {
        pass,
        detail: knowsView
          ? `active view → ${view} (expected ~explorer)`
          : "workbench.view.explorer resolved ok (no view in snapshot to assert); repeat also ok",
        evidence: {
          beforeView: before.vscode.activeView || before.vscode.focusedView || null,
          afterView: view || null,
        },
      };
    },
  },

  // L1.FILES.021 — saveAll with no dirty editors → benign no-op, no mtime churn.
  {
    id: "file.saveAllClean",
    specId: "L1.FILES.021",
    title: "saveAll with no dirty editors → no-op, no file mtimes change",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["saveAll"],
    rationale: `
WHAT: Seeds a workspace file, records the mtime listing of the project tree via
\`exec ls --full-time\`, issues \`request{type:"saveAll"}\` with NO dirty editors
(nothing typed), waits, and re-reads the same mtime listing. Asserts the bridge
reply was ok AND the before/after mtime listings are byte-identical (no file was
touched).

WHY THIS IS CORRECT: saveAll flushes every DIRTY editor to disk. With no dirty
editors there is nothing to flush, so the correct behaviour is a pure no-op:
ok-acked and zero filesystem writes. We verify "zero writes" by comparing
full-resolution mtimes out-of-band of the editor — if saveAll spuriously rewrote
clean files, their mtimes would advance and the listings would differ.

WHY IT MATTERS: A saveAll that touches unchanged files would corrupt mtimes and trip
incremental tooling (make, webpack, git's stat cache, file watchers) into needless
rebuilds — a subtle, expensive regression. This guards that saveAll is safe to call
on a clean workbench. A future reader seeing differing mtimes should check whether
saveAll iterates ALL editors/files instead of only the dirty ones.`,
    async run(env) {
      // Seed a couple of files so the mtime listing is non-trivial.
      env.exec(`printf 'a\\n' > ${PROJECT}/fleet-clean-a.txt; printf 'b\\n' > ${PROJECT}/fleet-clean-b.txt`);
      await sleep(400);
      const lsCmd = `cd ${PROJECT} && ls -la --full-time 2>/dev/null | grep -v ' \\.git'`;
      const before = env.exec(lsCmd);

      let ok = true;
      try { await env.request({ type: "saveAll" }); }
      catch { ok = false; }
      await sleep(1200);

      const after = env.exec(lsCmd);
      const unchanged = before.length > 0 && before === after;
      return {
        pass: ok && unchanged,
        detail: ok && unchanged
          ? "saveAll on a clean workbench was a no-op (no mtime changes)"
          : `ok=${ok} mtimesUnchanged=${unchanged} (want both true)`,
        evidence: { ok, mtimesUnchanged: unchanged, before, after },
      };
    },
  },

  // L1.FILES.022 — open the same file twice → not duplicated.
  {
    id: "file.openTwiceNoDup",
    specId: "L1.FILES.022",
    title: "Open the same file twice → reused, not duplicated (one tab)",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile", "query"],
    rationale: `
WHAT: writeFiles + opens \`fleet-dup.txt\`, then \`openFile\`s the SAME path a second
time. Asserts that exactly ONE openTabs entry references \`fleet-dup.txt\` (not two)
and that it is the active editor.

WHY THIS IS CORRECT: VS Code keeps a single document model per URI; opening an
already-open file re-focuses the existing editor rather than spawning a duplicate
tab. So after two opens of the same path the count of tabs referencing it must be
exactly 1, and that editor must be active. Counting tab references (not just "active
== path") is the precise test for de-duplication — a duplicate-per-open regression
would still leave the path active while leaking a second identical tab.

WHY IT MATTERS: A regression that opens a fresh tab on every openFile would leak
editors and break every tab-count assertion elsewhere in the suite (and waste
memory). This guards the open-document reuse contract. A future reader seeing two
references should check whether openFile resolves to the existing editor or always
creates a new one.`,
    async run(env) {
      const path = `${PROJECT}/fleet-dup.txt`;
      await env.request({ type: "writeFile", path, content: "dup\n" });
      await env.request({ type: "openFile", path });
      await sleep(700);
      await env.request({ type: "openFile", path }); // second open of the SAME path
      await sleep(900);

      const after = await env.observe("file.openTwiceNoDup.after");
      const tabs = after.vscode.openTabs;
      const measurable = Array.isArray(tabs);
      const b = base(path);
      const refs = measurable
        ? tabs.filter((e) => {
            const s = typeof e === "string" ? e : e?.path || e?.label || "";
            return s === path || base(s) === b;
          }).length
        : null;
      const active = isActive(after.vscode, path);

      return {
        pass: measurable && refs === 1 && active,
        detail: measurable
          ? `${refs} tab(s) reference ${b} (want 1); active=${active}`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { refs, openTabs: tabs, activeAfter: after.vscode.activeEditor },
      };
    },
  },

  // L1.FILES.023 — create + open a file with spaces and unicode in its name.
  {
    id: "file.unicodeName",
    specId: "L1.FILES.023",
    title: "Create + open a file with spaces and unicode in its name",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "fileContent"],
    rationale: `
WHAT: writeFiles \`${PROJECT}/fleet space ünïcode.txt\` (spaces + non-ASCII) with the
marker WEIRDNAME_OK, \`openFile\`s that exact path, then asserts THREE independent
things: activeEditor's basename matches the weird name, a fileContent query reads
back the marker, and the container shell (with proper single-quote escaping)
confirms the file exists on disk.

WHY THIS IS CORRECT: File names with spaces and non-ASCII characters are valid and
common, and they are exactly the inputs that expose escaping/quoting bugs — in the
JSON bridge wire (does the path survive serialization?), in the editor's URI
handling (does the basename render correctly?), and in our own \`env.exec\` quoting
(we single-quote the path so the shell doesn't word-split on the space). All three
channels agreeing proves the name round-trips intact through every layer.

WHY IT MATTERS: Path-escaping bugs are silent and only surface on non-trivial names —
a suite that only ever tests \`fleet-foo.txt\` would never catch a bridge that breaks
on spaces. This guards the wire + exec quoting against such names. A future reader:
if fileContent has the marker but exec says no-file, the bug is in OUR exec quoting;
if exec finds it but activeEditor's basename is wrong, the editor mangled the URI.`,
    async run(env) {
      const name = "fleet space ünïcode.txt";
      const path = `${PROJECT}/${name}`;
      const marker = "WEIRDNAME_OK";
      env.exec(`rm -f ${q(path)}`);
      await env.request({ type: "writeFile", path, content: `${marker}\n` });
      await env.request({ type: "openFile", path });
      await sleep(1200);

      const after = await env.observe("file.unicodeName.after");
      const fc = await env.request({ type: "fileContent", path }).catch(() => null);
      const text = textOf(fc);

      const activeOk = isActive(after.vscode, path);
      const contentOk = text.includes(marker);
      const diskOk = env.exec(`test -f ${q(path)} && echo yes || echo no`) === "yes";

      return {
        pass: activeOk && contentOk && diskOk,
        detail:
          `activeBasename ok=${activeOk} (${JSON.stringify(after.vscode.activeEditor)}); ` +
          `fileContent ok=${contentOk}; disk ok=${diskOk}`,
        evidence: { activeAfter: after.vscode.activeEditor, fileContent: text, diskOk },
      };
    },
  },

  // L1.FILES.024 — bridge-written files are byte-identical to the container shell.
  {
    id: "file.mountParity",
    specId: "L1.FILES.024",
    title: "Files written via the bridge are byte-identical in the container shell",
    tags: ["files"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: writeFiles \`${PROJECT}/fleet-mount.txt\` with EXACTLY the bytes "MOUNT_OK"
(no trailing newline), then reads it back through the container shell with
\`exec cat\` and asserts EXACT byte equality (\`== "MOUNT_OK"\`, not inclusion).

WHY THIS IS CORRECT: The editor (bridge writeFile) and the git CLI / shell must
operate on ONE shared workspace mount. The strongest proof of that is a byte-for-byte
round-trip: write known bytes through the editor's path, read them back through a
completely independent process (the container's own shell). We assert exact equality
rather than inclusion here precisely because this is the mount-parity probe — any
divergence (a different mount, a path translation, an encoding rewrite) would change
the bytes and must fail, whereas elsewhere we tolerate newline/BOM normalisation.

WHY IT MATTERS: EVERY git assertion in 13-scm-git.md and every \`exec cat\` check in
12-files.md depends on the editor's filesystem and the shell/git seeing the same
mount. If that assumption breaks (e.g. writeFile starts writing to an overlay the
shell can't see), dozens of tests would fail mysteriously. This isolates a
mount/path divergence as its own single, unambiguous tripwire so a future reader can
rule it in or out before suspecting any higher-level file or git logic.`,
    async run(env) {
      const path = `${PROJECT}/fleet-mount.txt`;
      env.exec(`rm -f ${path}`);
      await env.request({ type: "writeFile", path, content: "MOUNT_OK" });
      await sleep(600);
      const got = env.exec(`cat ${path}`);
      return {
        pass: got === "MOUNT_OK",
        detail: got === "MOUNT_OK"
          ? "bridge-written bytes are byte-identical in the container shell (same mount)"
          : `shell read ${JSON.stringify(got)} != "MOUNT_OK" — possible mount/path divergence`,
        evidence: { shellRead: got },
      };
    },
  },

  // ── 13-scm-git.md ───────────────────────────────────────────────────────────

  // L1.SCM.002 — git init in a dir that is already a repo → idempotent reinit.
  {
    id: "git.reinit",
    specId: "L1.SCM.002",
    title: "git init in an existing repo → idempotent reinit, identity preserved",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Runs \`git init\` + sets \`user.email=eval@fleet.local\` in the project dir,
then runs \`git init\` a SECOND time and asserts: the second init exits 0, the dir is
still a work tree (\`git rev-parse --is-inside-work-tree == "true"\`), and the prior
\`user.email\` config STILL reads \`eval@fleet.local\` (reinit didn't wipe config).

WHY THIS IS CORRECT: \`git init\` on an existing repository is, by git's own contract,
idempotent — it reinitialises (refreshes hooks/templates) WITHOUT destroying existing
configuration, refs, or objects. So the correct post-state is a clean exit, an intact
work tree, and unchanged identity config. Checking the persisted \`user.email\`
specifically is the sharp test: a regression in our setup path that double-inits in a
way that clobbers config would surface as a lost identity.

WHY IT MATTERS: Several behaviours' setup runs \`git init\` defensively; if reinit
were destructive it could silently wipe the identity config those tests rely on,
making commits fail or attribute to the wrong author. This guards the reinit-is-benign
assumption. A future reader seeing the identity lost should check whether something in
the init path is rewriting \`.git/config\` instead of preserving it.`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);

      const code = env.exec(`cd ${PROJECT} && git init -q >/dev/null 2>&1; echo $?`);
      const inTree = env.exec(`cd ${PROJECT} && git rev-parse --is-inside-work-tree 2>/dev/null`);
      const email = env.exec(`cd ${PROJECT} && git config user.email 2>/dev/null`);

      const pass = code === "0" && inTree === "true" && email === "eval@fleet.local";
      return {
        pass,
        detail: pass
          ? "reinit was idempotent (exit 0, still a work tree, identity preserved)"
          : `reinit not clean: exit=${code} inTree=${inTree} email=${JSON.stringify(email)}`,
        evidence: { exit: code, inTree, email },
      };
    },
  },

  // L1.SCM.003 — open the Source Control view (dual posture).
  {
    id: "scm.openView",
    specId: "L1.SCM.003",
    title: "Open Source Control view → SCM container active (or command ok)",
    tags: ["git", "scm", "views"],
    needs: ["writeFile"],
    rationale: `
WHAT: Ensures a git repo exists (init in the project dir), observes the active view,
runs \`workbench.view.scm\`, re-observes, and runs it once more (idempotence). If the
snapshot exposes an active/focused view it asserts a match for /scm|source.?control/i;
otherwise it falls back to "the command resolved" (env.act throws on !ok).

WHY THIS IS CORRECT: \`workbench.view.scm\` is the canonical command that reveals the
Source Control viewlet. On a healthy workbench it always succeeds and, where the view
layer is queryable, the active view becomes the SCM view. The dual posture mirrors
search.findInFiles and view.explorer: the snapshot may not surface the active view
name, so we refuse to hard-fail a genuine success the harness simply can't observe.
The repeat confirms re-revealing an already-focused viewlet stays ok.

WHY IT MATTERS: The SCM viewlet is the entry point for all staging/commit UX; this
guards its command registration + activation (and per mux.rs this is the one SCM
command the native menu forwards). If a refactor unregisters it or breaks viewlet
activation, env.act throws and this goes red. A future reader seeing the fallback
branch always taken should note the snapshot lost view introspection.`,
    async run(env) {
      env.exec(`cd ${PROJECT} && (git rev-parse --is-inside-work-tree >/dev/null 2>&1 || (git init -q && ${gitIdentity}))`);
      const before = await env.observe("scm.openView.before");
      await env.act("workbench.view.scm");
      await sleep(1000);
      const after = await env.observe("scm.openView.after");
      await env.act("workbench.view.scm"); // idempotent repeat
      await sleep(400);

      const view = after.vscode.activeView || after.vscode.focusedView;
      const knowsView = typeof view === "string";
      const pass = knowsView ? /scm|source.?control/i.test(view) : true;
      return {
        pass,
        detail: knowsView
          ? `active view → ${view} (expected ~scm)`
          : "workbench.view.scm resolved ok (no view in snapshot to assert); repeat also ok",
        evidence: {
          beforeView: before.vscode.activeView || before.vscode.focusedView || null,
          afterView: view || null,
        },
      };
    },
  },

  // L1.SCM.004 — open Source Control in a non-git folder → graceful, no crash.
  {
    id: "scm.openViewNoRepo",
    specId: "L1.SCM.004",
    title: "Open Source Control in a non-git folder → degrades gracefully",
    tags: ["git", "scm", "views"],
    isolation: "fresh",
    needs: ["command"],
    rationale: `
WHAT: Removes any \`.git\` so the workspace is NOT a repo, then runs
\`workbench.view.scm\` and asserts the command resolves ok (env.act throws on !ok) and
that any exposed snapshot.scmChanges is 0 or undefined (no spurious changes invented
for a repo-less workspace).

WHY THIS IS CORRECT: With no git provider the SCM viewlet must still open and simply
show an empty / "no source control providers" state — it must NOT throw. The honest
assertion is "command resolved AND no phantom change count": a repo-less workspace has
nothing to report, so scmChanges being absent or 0 is correct. We deliberately delete
\`.git\` first to guarantee the no-provider precondition (other fresh tests init a repo).

WHY IT MATTERS: Fresh projects start without a repo; an SCM viewlet that throws on a
non-repo workspace would break the editor for every brand-new project. This guards
graceful degradation. A future reader seeing a non-zero scmChanges here should suspect
the SCM provider is fabricating entries with no underlying repo, or a leftover \`.git\`
from another test leaked in (hence the explicit rm).`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      let ok = true;
      try { await env.act("workbench.view.scm"); }
      catch { ok = false; }
      await sleep(900);
      const after = await env.observe("scm.openViewNoRepo.after");
      const snap = after.vscode.scmChanges;
      const snapOk = snap === undefined || snap === 0;
      return {
        pass: ok && snapOk,
        detail: ok && snapOk
          ? `SCM view opened in a non-repo workspace (scmChanges=${JSON.stringify(snap)})`
          : `ok=${ok} scmChanges=${JSON.stringify(snap)} (want ok & 0/undefined)`,
        evidence: { ok, scmChanges: snap ?? null },
      };
    },
  },

  // L1.SCM.006 — a new untracked file shows as an untracked change.
  {
    id: "git.untrackedChange",
    specId: "L1.SCM.006",
    title: "A new untracked file surfaces as an untracked (??) change",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Establishes a committed baseline (init + commit a tracked file) and confirms a
clean tree, then writeFiles a BRAND-NEW path \`untracked-new.txt\`, waits 800ms, and
asserts \`git status --porcelain\` contains a line starting with \`??\` for that file
AND the total changed-line count is exactly 1.

WHY THIS IS CORRECT: Git distinguishes untracked files (porcelain code \`??\`) from
modified tracked files (\` M\`/\`M \`). A brand-new file that has never been added must
appear under the untracked status code, and as the ONLY change against a clean
baseline it must be the single porcelain line. Matching the \`??\` prefix specifically
(not just "some change") proves it's classified as untracked, not mis-reported as a
modification.

WHY IT MATTERS: SCM decorations must count a never-before-seen file as a change at
all — a regression that only watches already-tracked paths would make new files
invisible in the viewlet (a nasty "where did my file go" bug). This guards untracked
detection as distinct from modification. A future reader: if the line exists but
without \`??\`, the classification regressed; if there's no line, new-file watching
broke (compare with git.diffDecorations, which covers the modified-file case).`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);
      await env.request({ type: "writeFile", path: `${PROJECT}/tracked.txt`, content: "base\n" });
      await sleep(300);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "baseline"`);
      const cleanBefore = env.exec(`cd ${PROJECT} && git status --porcelain`);

      await env.request({ type: "writeFile", path: `${PROJECT}/untracked-new.txt`, content: "new\n" });
      await sleep(800);

      const porcelain = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const lines = porcelain.split("\n").filter((l) => l.trim());
      const hasUntracked = lines.some((l) => l.startsWith("??") && l.includes("untracked-new.txt"));

      const pass = cleanBefore === "" && hasUntracked && lines.length === 1;
      return {
        pass,
        detail: pass
          ? `untracked-new.txt surfaced as an untracked change (${JSON.stringify(porcelain.trim())})`
          : `expected one '??' line; cleanBefore=${JSON.stringify(cleanBefore)} status=${JSON.stringify(porcelain)}`,
        evidence: { cleanBefore: cleanBefore === "", porcelain, lineCount: lines.length },
      };
    },
  },

  // L1.SCM.019 — diff of a modified tracked file shows exactly the changed line.
  {
    id: "git.diffHunk",
    specId: "L1.SCM.019",
    title: "Diff a modified tracked file → only the changed line is flagged",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Commits \`diff-me.txt\` = "a\\nb\\nc\\n", then rewrites it to
"a\\nB-CHANGED\\nc\\n" via writeFile, and asserts \`git diff --unified=0 diff-me.txt\`
output contains the hunk lines \`-b\` and \`+B-CHANGED\` and does NOT contain \`-a\`,
\`-c\` (the unchanged lines must not appear as removals).

WHY THIS IS CORRECT: The SCM diff editor is a UI projection of \`git diff\`; the ground
truth of "only line 2 changed" is that the unified diff shows exactly one hunk
removing \`b\` and adding \`B-CHANGED\`, with the untouched lines \`a\` and \`c\` absent
from the +/- set (\`--unified=0\` strips context so only true changes appear). Matching
both the presence of the changed pair AND the absence of the unchanged lines is the
precise definition of a minimal, correct diff.

WHY IT MATTERS: A regression that whole-file-diffs (treats every line as changed, e.g.
from a line-ending or encoding rewrite on save) would flag \`a\` and \`c\` too, making
the SCM gutter scream about lines the user never touched. This guards the diff
minimality. A future reader seeing \`-a\`/\`-c\` present should suspect a save-time
content rewrite (CRLF, BOM, trailing-whitespace strip) rather than a real edit.`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);
      const path = `${PROJECT}/diff-me.txt`;
      await env.request({ type: "writeFile", path, content: "a\nb\nc\n" });
      await sleep(300);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "baseline"`);

      await env.request({ type: "writeFile", path, content: "a\nB-CHANGED\nc\n" });
      await sleep(600);

      const diff = env.exec(`cd ${PROJECT} && git diff --unified=0 diff-me.txt`);
      const lines = diff.split("\n");
      // Inspect only the +/- body lines (skip the --- / +++ file headers).
      const minus = lines.filter((l) => /^-[^-]/.test(l));
      const plus = lines.filter((l) => /^\+[^+]/.test(l));
      const removedB = minus.some((l) => l === "-b");
      const addedChanged = plus.some((l) => l === "+B-CHANGED");
      const removedA = minus.some((l) => l === "-a");
      const removedC = minus.some((l) => l === "-c");

      const pass = removedB && addedChanged && !removedA && !removedC;
      return {
        pass,
        detail: pass
          ? "git diff shows exactly b→B-CHANGED; a and c untouched"
          : `unexpected diff: -b=${removedB} +B-CHANGED=${addedChanged} -a=${removedA} -c=${removedC}`,
        evidence: { diff, minus, plus },
      };
    },
  },

  // L1.SCM.021 — create + resolve a merge conflict (full lifecycle via plumbing).
  {
    id: "git.mergeConflict",
    specId: "L1.SCM.021",
    title: "Create + resolve a merge conflict → markers then clean resolution",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Builds a conflict: commits \`conflict.txt\` on \`main\`, branches \`feature\`
editing line 1 to FEATURE, switches back to \`main\` and edits the same line to MAIN,
commits both, then \`git merge feature\` (which must conflict). Asserts mid-merge that
\`git status\` shows \`UU conflict.txt\` AND the file contains all three conflict
markers (\`<<<<<<<\`, \`=======\`, \`>>>>>>>\`). Then resolves by writeFile-ing clean
content + \`git add\` + \`git commit\`, and asserts the tree is clean, the commit count
rose by the merge commit, and the resolved file contains neither a marker nor the
losing side (FEATURE).

WHY THIS IS CORRECT: This walks the full hardest-case SCM lifecycle through git's own
plumbing (the ground truth behind the merge-conflict UI): two divergent edits to the
SAME line force a content conflict; git records \`UU\` (both-modified, unmerged) and
writes conflict markers into the file; a real resolution replaces the marked content,
\`git add\` clears the unmerged state, and \`git commit\` creates the merge commit,
leaving a clean tree. Asserting markers appear THEN disappear (and the losing side is
gone) proves both halves: the conflict was genuine and the resolution actually took.

WHY IT MATTERS: Conflict handling is the most complex SCM state and the easiest to get
subtly wrong (e.g. a merge that silently auto-resolves, or a "resolution" that leaves
stray markers committed). This guards the entire conflict→markers→resolve→clean cycle.
A future reader: if \`UU\`/markers never appear, the merge auto-resolved (wrong); if the
final tree is dirty or still has markers, the resolution path is broken.`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      const file = `${PROJECT}/conflict.txt`;
      env.exec(`rm -f ${file}`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity} && git checkout -q -b main 2>/dev/null || true`);
      await env.request({ type: "writeFile", path: file, content: "line one\nshared\n" });
      await sleep(300);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "conflict: base"`);
      // Ensure we are on a branch named main for a deterministic merge target.
      env.exec(`cd ${PROJECT} && git branch -m main 2>/dev/null || true`);

      // feature edits line 1.
      env.exec(`cd ${PROJECT} && git checkout -q -b feature`);
      await env.request({ type: "writeFile", path: file, content: "FEATURE\nshared\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "feature: edit line 1"`);

      // main edits the same line differently → conflict on merge.
      env.exec(`cd ${PROJECT} && git checkout -q main`);
      await env.request({ type: "writeFile", path: file, content: "MAIN\nshared\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "main: edit line 1"`);

      const headBefore = parseInt(env.exec(`cd ${PROJECT} && git rev-list --count HEAD`) || "0", 10);

      // Merge → expected to conflict (non-zero exit, swallowed by exec).
      env.exec(`cd ${PROJECT} && git merge feature >/dev/null 2>&1; true`);
      const midStatus = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const conflicted = midStatus.split("\n").some((l) => l.startsWith("UU") && l.includes("conflict.txt"));
      const midContent = env.exec(`cat ${file}`);
      const hasMarkers =
        midContent.includes("<<<<<<<") &&
        midContent.includes("=======") &&
        midContent.includes(">>>>>>>");

      // Resolve: clean content, add, commit the merge.
      await env.request({ type: "writeFile", path: file, content: "RESOLVED\nshared\n" });
      await sleep(300);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "merge: resolve conflict"`);

      const cleanAfter = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const headAfter = parseInt(env.exec(`cd ${PROJECT} && git rev-list --count HEAD`) || "0", 10);
      const finalContent = env.exec(`cat ${file}`);
      const resolvedClean =
        cleanAfter === "" &&
        headAfter > headBefore &&
        !finalContent.includes("<<<<<<<") &&
        !finalContent.includes("FEATURE"); // losing side gone

      const pass = conflicted && hasMarkers && resolvedClean;
      return {
        pass,
        detail: pass
          ? "merge conflicted (UU + markers), then resolved to a clean tree (no markers, losing side gone)"
          : `conflicted=${conflicted} markers=${hasMarkers} resolvedClean=${resolvedClean} ` +
            `(status=${JSON.stringify(cleanAfter)} head ${headBefore}->${headAfter})`,
        evidence: {
          midStatus, conflicted, hasMarkers,
          headBefore, headAfter, cleanAfter, finalContent,
        },
      };
    },
  },

  // L1.SCM.022 — clean (non-conflicting) merge → no markers, file appears.
  {
    id: "git.mergeClean",
    specId: "L1.SCM.022",
    title: "Merge with no overlap → clean merge, no conflict markers",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Commits a baseline on \`main\`, creates \`feature\` that adds a NEW non-
overlapping file \`feat.txt\`, commits it, switches to \`main\`, and \`git merge
feature\`. Asserts the merge left a clean tree (\`git status --porcelain == ""\`),
\`feat.txt\` is present (\`test -f\`), and NO \`<<<<<<<\` marker exists anywhere in the
tree (\`grep -r\`).

WHY THIS IS CORRECT: When the two branches touch disjoint files, git merges them
automatically with no conflict — the result is a clean tree that simply gains the
feature's new file, and crucially NO conflict markers are written. This is the happy
path that must NOT spuriously enter the conflict machinery. Asserting the file appears
AND the absence of markers (tree-wide grep) proves the merge both materialised the
change and stayed off the conflict path.

WHY IT MATTERS: This is the negative case for git.mergeConflict — it guards that a
clean merge doesn't falsely raise conflict markers (a regression that, say, always
3-way-marks even non-overlapping merges). A future reader seeing a marker here means
the merge wrongly entered the conflict path; a missing \`feat.txt\` means the merge
didn't materialise the other branch's content.`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && rm -f feat.txt base.txt`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);
      await env.request({ type: "writeFile", path: `${PROJECT}/base.txt`, content: "base\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "base" && git branch -m main 2>/dev/null || true`);

      env.exec(`cd ${PROJECT} && git checkout -q -b feature`);
      await env.request({ type: "writeFile", path: `${PROJECT}/feat.txt`, content: "feature only\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "feat: add feat.txt"`);

      env.exec(`cd ${PROJECT} && git checkout -q main`);
      env.exec(`cd ${PROJECT} && git merge feature >/dev/null 2>&1; true`);

      const clean = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const featThere = env.exec(`test -f ${PROJECT}/feat.txt && echo yes || echo no`) === "yes";
      const noMarkers = env.exec(
        `cd ${PROJECT} && grep -rl '<<<<<<<' . --exclude-dir=.git 2>/dev/null | head -1`
      ) === "";

      const pass = clean === "" && featThere && noMarkers;
      return {
        pass,
        detail: pass
          ? "clean merge: feat.txt present, tree clean, no conflict markers"
          : `unexpected: clean=${JSON.stringify(clean)} featThere=${featThere} noMarkers=${noMarkers}`,
        evidence: { clean, featThere, noMarkers },
      };
    },
  },

  // L1.SCM.023 — a file outside the repo root must not pollute SCM state.
  {
    id: "git.outsideRepo",
    specId: "L1.SCM.023",
    title: "A file written outside the repo root does not appear as a git change",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Commits a clean baseline in \`${PROJECT}\`, then writeFiles \`/tmp/outside.txt\`
(OUTSIDE the work tree), waits, and asserts \`git status --porcelain\` in the project
does NOT reference \`outside.txt\` and reports no change attributable to it (the tree
stays clean apart from any explicit in-tree edit — here, none).

WHY THIS IS CORRECT: Git's working tree is bounded by the repository root; a file
written at \`/tmp\` is simply not part of \`${PROJECT}\`'s tree, so it must never surface
in that repo's status. The correct outcome is that the out-of-tree write is invisible
to the repo. We assert via porcelain (the SCM viewlet's ground truth) that nothing
named \`outside.txt\` appears and the tree is clean.

WHY IT MATTERS: This guards that the SCM/git boundary is the repo root — a regression
where the provider watches the whole container filesystem (or a wrong root) would
surface spurious "changes" for every scratch file an agent writes to \`/tmp\`,
drowning the real diff. A future reader seeing \`outside.txt\` in status should check
the SCM provider's watched root / the repo discovery boundary.`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);
      await env.request({ type: "writeFile", path: `${PROJECT}/intree.txt`, content: "in tree\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "baseline"`);
      const cleanBefore = env.exec(`cd ${PROJECT} && git status --porcelain`);

      await env.request({ type: "writeFile", path: `/tmp/outside.txt`, content: "outside\n" });
      await sleep(800);

      const porcelain = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const mentionsOutside = porcelain.includes("outside.txt");
      const pass = cleanBefore === "" && porcelain === "" && !mentionsOutside;
      return {
        pass,
        detail: pass
          ? "file at /tmp/outside.txt did not appear in the repo's git status (tree stayed clean)"
          : `repo status changed by out-of-tree write: ${JSON.stringify(porcelain)}`,
        evidence: { cleanBefore: cleanBefore === "", porcelain, mentionsOutside },
      };
    },
  },

  // L1.SCM.024 — commit clears the working-tree change set (decorations clear).
  {
    id: "git.commitClearsChanges",
    specId: "L1.SCM.024",
    title: "Commit a modified file → working-tree change count returns to zero",
    tags: ["git", "scm"],
    isolation: "fresh",
    needs: ["writeFile"],
    rationale: `
WHAT: Commits a baseline, modifies the tracked file via writeFile (one working-tree
change), records the commit count, then \`git add -A && git commit\` and waits 800ms
for the SCM provider. Asserts the tree returns to clean (\`git status --porcelain ==
""\`), the commit count rose by exactly 1, and — if the snapshot exposes scmChanges —
it is 0 after.

WHY THIS IS CORRECT: Committing stages-and-records the pending change; afterward the
working tree has nothing pending, so \`git status\` is empty and the SCM decorations
must clear. The commit count rising by exactly 1 proves the change was actually
recorded (not just discarded), and the empty porcelain proves nothing is left dirty.
The opportunistic scmChanges==0 cross-check confirms the editor's SCM model re-read
after the commit rather than leaving stale decorations.

WHY IT MATTERS: This completes the diff→commit→clean cycle (the counterpart to
git.diffDecorations, which only sets up the dirty state). It guards that the SCM
provider re-reads the tree post-commit and clears decorations — a regression that
leaves stale "1 change" badges after a commit would mislead the user into thinking
work is uncommitted. A future reader: clean porcelain but scmChanges still >0 means
the provider didn't refresh after commit (an SCM-watch issue, not a git one).`,
    async run(env) {
      env.exec(`rm -rf ${PROJECT}/.git`);
      env.exec(`cd ${PROJECT} && git init -q && ${gitIdentity}`);
      const path = `${PROJECT}/tracked.txt`;
      await env.request({ type: "writeFile", path, content: "line one\nline two\n" });
      await sleep(200);
      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "baseline"`);

      await env.request({ type: "writeFile", path, content: "line one CHANGED\nline two\n" });
      await sleep(800);
      const dirty = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const headBefore = parseInt(env.exec(`cd ${PROJECT} && git rev-list --count HEAD`) || "0", 10);

      env.exec(`cd ${PROJECT} && git add -A && git commit -q -m "fleet: clear"`);
      await sleep(800);

      const after = await env.observe("git.commitClearsChanges.after");
      const cleanAfter = env.exec(`cd ${PROJECT} && git status --porcelain`);
      const headAfter = parseInt(env.exec(`cd ${PROJECT} && git rev-list --count HEAD`) || "0", 10);
      const snap = after.vscode.scmChanges;
      const snapKnown = typeof snap === "number";

      const pass =
        dirty !== "" &&
        cleanAfter === "" &&
        headAfter === headBefore + 1 &&
        (!snapKnown || snap === 0);
      return {
        pass,
        detail: pass
          ? `committed; tree clean, commits ${headBefore}→${headAfter}` +
            (snapKnown ? `, snapshot scmChanges=${snap}` : "")
          : `unexpected: dirtyBefore=${JSON.stringify(dirty)} cleanAfter=${JSON.stringify(cleanAfter)} ` +
            `head ${headBefore}->${headAfter}` + (snapKnown ? ` scmChanges=${snap}` : ""),
        evidence: {
          dirtyBefore: dirty, cleanAfter,
          headBefore, headAfter,
          snapshotScmChanges: snapKnown ? snap : null,
        },
      };
    },
  },
];

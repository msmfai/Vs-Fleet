// SPEC area 11-terminal.md — additional L1.TERM behaviours.
//
// One self-contained file the registry auto-discovers. It does NOT touch the
// existing terminal.mjs / terminal_more.mjs files (which already implement
// L1.TERM.001/010/020/030/040). Every behaviour here drives a real action and
// ASSERTS the effect via the snapshot (terminalCount / terminals), the
// terminalText / fileContent queries, or container exec — never "command ok".
//
// Bridge contract used here (see packages/fleet-bridge/src/extension.ts):
//  - command / query are always present.
//  - termSend {name?, text} → reply {terminal:<name>}; appends a newline so the
//    command actually RUNS, and records "$ <text>\n" into the per-terminal buffer.
//  - terminalText {name?} → {text, source} where source∈{"buffer",""}; NEVER errors.
//  - fileContent {path} → {text}; reads the in-memory doc or disk.
//  - kill / killAll executeCommand promises do NOT resolve headlessly → env.fire().
//
// Most behaviours use isolation:"fresh" because terminalCount is process-global
// within an env, so exact-count asserts need a clean 0 start.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Pull a §3.3-query payload field whether the bridge spreads it onto the result
// msg or nests it under `.data` (both shapes seen across queries).
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);

// Poll fileContent until it contains `needle`. Returns the last text seen.
async function waitForFile(env, path, needle, { tries = 14, gap = 500 } = {}) {
  let text = "";
  for (let i = 0; i < tries; i++) {
    await sleep(gap);
    const r = await env.request({ type: "fileContent", path }).catch(() => null);
    text = field(r, "text") || "";
    if (text.includes(needle)) return { hit: true, text };
  }
  return { hit: false, text };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L1.TERM.002 — fresh env: New yields EXACTLY terminalCount 1 ──────────────
  {
    id: "terminal.newExactlyOne",
    specId: "L1.TERM.002",
    title: "Terminal: New from a fresh env yields exactly terminalCount 1",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a FRESH env (terminalCount 0, terminals == []), fires the single command
'workbench.action.terminal.new', waits 2s, and asserts terminalCount == 1 AND
terminals.length == 1 — an EXACT count, not merely monotonic growth.

WHY THIS IS THE EXPECTED OUTCOME: a fresh env controls the full 0→1 transition, so
exactly one New must register exactly one Terminal object in window.terminals. The
monotonic-only terminal.new (shared isolation) deliberately cannot pin the value
because it inherits whatever terminals earlier behaviours left behind; here the
clean start lets us demand the precise 1. We read terminalCount and terminals.length
together because they must agree (the array and the count are two views of the same
window.terminals).

WHY IT MATTERS: a regression that fires the create twice (a duplicate-dispatch bug),
or that mis-counts (counting a disposed terminal, or off-by-one in the snapshot),
would still pass the monotonic terminal.new but is caught here. The split/kill
behaviours all build their +1 / −1 arithmetic on top of this exact 0→1 baseline, so
pinning it guards the count semantics the rest of the terminal suite depends on.`,
    async run(env) {
      const before = await env.observe("term.newExactlyOne.before");
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const after = await env.observe("term.newExactlyOne.after");
      const tc = after.vscode.terminalCount;
      const len = Array.isArray(after.vscode.terminals) ? after.vscode.terminals.length : -1;
      return {
        pass: before.vscode.terminalCount === 0 && tc === 1 && len === 1,
        detail: `fresh ${before.vscode.terminalCount} → count=${tc}, terminals.length=${len} (want 1/1)`,
        evidence: {
          before: before.vscode.terminalCount,
          after: { terminalCount: tc, terminals: after.vscode.terminals },
        },
      };
    },
  },

  // ── L1.TERM.003 — Repeated New accumulates distinct terminals (→3) ───────────
  {
    id: "terminal.newAccumulates",
    specId: "L1.TERM.003",
    title: "Terminal: repeated New accumulates to terminalCount 3",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env (count 0), fires 'workbench.action.terminal.new' three times
with 1.5s gaps and asserts terminalCount == 3 with terminals.length == 3.

WHY THIS IS THE EXPECTED OUTCOME: New always ADDS a terminal — it never reuses or
focuses an existing one — so three independent creates must yield three distinct
Terminal objects. VS Code disambiguates duplicate default names internally, but the
count is the invariant: 0→1→2→3. We gap the creates by 1.5s so each pty registers in
window.terminals before the next fires, making the final count deterministic.

WHY IT MATTERS: a subtle regression where New focuses/activates an existing terminal
instead of creating a fresh one would silently cap the count (e.g. stay at 1) while
still "succeeding" — invisible to a single-create test. Only repeated creates expose
it. This guards the additive semantics agents rely on when opening several shells
(build + server + scratch).`,
    async run(env) {
      const before = await env.observe("term.newAccumulates.before");
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.terminal.new");
        await sleep(1500);
      }
      const after = await env.observe("term.newAccumulates.after");
      const tc = after.vscode.terminalCount;
      const len = Array.isArray(after.vscode.terminals) ? after.vscode.terminals.length : -1;
      return {
        pass: before.vscode.terminalCount === 0 && tc === 3 && len === 3,
        detail: `fresh 0 → count=${tc}, terminals.length=${len} after 3×New (want 3/3)`,
        evidence: { after: { terminalCount: tc, terminals: after.vscode.terminals } },
      };
    },
  },

  // ── L1.TERM.011 — Split with NO active terminal is a graceful no-op ──────────
  {
    id: "terminal.splitFromEmpty",
    specId: "L1.TERM.011",
    title: "Terminal: Split with no active terminal degrades gracefully (no throw/hang)",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env (count 0, NO terminal to split), invokes
'workbench.action.terminal.split' via act() and asserts the command RESOLVES (act()
throws on !ok / a hang would never resolve) — then records the resulting terminalCount
as evidence. Empirically code-server treats split-with-nothing-to-split as a NO-OP:
the command resolves ok and terminalCount stays 0 (it does NOT auto-create a terminal).

WHY THIS IS THE EXPECTED OUTCOME: the load-bearing invariant is graceful DEGRADATION,
not a particular count. With no active terminal there is nothing to split, so the
correct, safe behaviour is to resolve cleanly without throwing or hanging the bridge.
Whether the editor chooses to create a first terminal (count 1) or no-op (count 0) is
an editor-version detail; code-server no-ops. We assert only the part that must hold —
the command came back ok and did not wedge the env — and surface the count as evidence
so a future change (e.g. an editor that starts auto-creating) is visible, not a failure.

WHY IT MATTERS: agents blind-fire split without first checking for an active terminal;
if the no-active branch threw or hung, it would dead-end the env. This pins the
graceful-degrade contract for the empty-workbench path on any refactor of
terminal-group handling.`,
    async run(env) {
      const before = await env.observe("term.splitFromEmpty.before");
      let ok = true;
      try {
        await env.act("workbench.action.terminal.split");
      } catch {
        ok = false;
      }
      await sleep(2000);
      const after = await env.observe("term.splitFromEmpty.after");
      // Pass on graceful resolution from an empty start; the count is evidence, not the
      // assertion (code-server no-ops → stays 0; another editor might create one).
      return {
        pass: before.vscode.terminalCount === 0 && ok,
        detail: `fresh 0 → split resolved ok=${ok}; count=${after.vscode.terminalCount} (graceful no-op, no throw/hang)`,
        evidence: { commandOk: ok, after: after.vscode.terminalCount },
      };
    },
  },

  // ── L1.TERM.012 — Splitting a split deepens the group (→3) ───────────────────
  {
    id: "terminal.splitTwice",
    specId: "L1.TERM.012",
    title: "Terminal: New + Split + Split yields terminalCount 3",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env, runs New (1), Split (2), Split again (3) with 2s settles and
asserts the final terminalCount == 3.

WHY THIS IS THE EXPECTED OUTCOME: each split adds exactly one pane, and VS Code models
every pane — including a split of a split — as its own Terminal object. So the
progression is 1→2→3; the second split must add to the group, not collapse the panes
into one object. The snapshot exposes only flat names/count (no group membership), so
the assertable invariant is the count delta, which must be +1 per split.

WHY IT MATTERS: a regression where later splits plateau (each split after the first
silently no-ops, or multiple panes coalesce into one Terminal object in the snapshot)
would leave the count stuck at 2. Only a split-of-a-split exposes it. This guards the
"always +1, never collapse" shape of split that terminal.split (a single split)
cannot.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.act("workbench.action.terminal.split");
      await sleep(2000);
      await env.act("workbench.action.terminal.split");
      await sleep(2000);
      const after = await env.observe("term.splitTwice.after");
      return {
        pass: after.vscode.terminalCount === 3,
        detail: `count=${after.vscode.terminalCount} after New+Split+Split (want 3)`,
        evidence: { terminalCount: after.vscode.terminalCount, terminals: after.vscode.terminals },
      };
    },
  },

  // ── L1.TERM.021 — Kill with no terminals is a clean no-op ────────────────────
  {
    id: "terminal.killEmpty",
    specId: "L1.TERM.021",
    title: "Terminal: Kill with no terminals open is a clean no-op",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env (count 0), FIRES 'workbench.action.terminal.kill' (kill's
executeCommand doesn't resolve headlessly, so fire — never await), waits 2.5s, then
asserts terminalCount is 0 both before AND after, and proves the bridge stays
responsive by round-tripping a follow-up query.

WHY THIS IS THE EXPECTED OUTCOME: with nothing to dispose, kill must be a no-op — no
terminal created, none removed (it was already 0), and crucially the bridge must not
wedge. We FIRE rather than act() because kill's RPC reply hangs even in the
non-empty case; the empty case must not behave differently. A successful follow-up
query is the proof the fire path didn't leave the bridge waiting on a never-resolving
disposal.

WHY IT MATTERS: agents (or panic-recovery code) may fire kill defensively without
checking for an open terminal. If the no-active branch threw, hung the bridge, or
desynced the snapshot, that defensive call would brick the env. This guards the
empty-state kill branch and proves the fire path doesn't wedge when there is nothing
to dispose.`,
    async run(env) {
      const before = await env.observe("term.killEmpty.before");
      env.fire("workbench.action.terminal.kill");
      await sleep(2500);
      const after = await env.observe("term.killEmpty.after");
      // follow-up query proves the bridge still round-trips (not wedged).
      let responsive = false;
      try {
        const r = await env.request({ type: "query" });
        responsive = !!(r && (r.ok !== false));
      } catch { responsive = false; }
      return {
        pass: before.vscode.terminalCount === 0 && after.vscode.terminalCount === 0 && responsive,
        detail: `count ${before.vscode.terminalCount}→${after.vscode.terminalCount} after kill; bridge responsive=${responsive}`,
        evidence: { before: before.vscode.terminalCount, after: after.vscode.terminalCount, responsive },
      };
    },
  },

  // ── L1.TERM.022 — Kill one of several leaves the rest (3→2) ───────────────────
  {
    id: "terminal.killOneOfThree",
    specId: "L1.TERM.022",
    title: "Terminal: Kill one of three leaves two alive",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env, creates three terminals (count 3), FIRES
'workbench.action.terminal.kill' ONCE, waits 2.5s, and asserts terminalCount == 2 —
exactly one removed.

WHY THIS IS THE EXPECTED OUTCOME: kill targets only the ACTIVE terminal, disposing it
and its backing pty, leaving the other two untouched. So a single kill against three
terminals must leave two. We FIRE because kill's executeCommand promise doesn't
resolve headlessly. We assert == 2 (not "< 3") because the whole point is that kill
removes exactly one, not all.

WHY IT MATTERS: a regression where kill disposes the entire terminal group, or all
terminals, would silently destroy an agent's other live shells (its build or server
pty) when it only meant to close the active one — a data-loss-class bug that a
single-terminal kill test (1→0) cannot distinguish from correct behaviour. Three
terminals make "exactly one" observable.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.terminal.new");
        await sleep(1500);
      }
      const before = await env.observe("term.killOneOfThree.before");
      env.fire("workbench.action.terminal.kill");
      await sleep(2500);
      const after = await env.observe("term.killOneOfThree.after");
      return {
        pass: before.vscode.terminalCount === 3 && after.vscode.terminalCount === 2,
        detail: `count ${before.vscode.terminalCount}→${after.vscode.terminalCount} after one kill (want 3→2)`,
        evidence: { before: before.vscode.terminalCount, after: after.vscode.terminalCount },
      };
    },
  },

  // ── L1.TERM.023 — Kill All disposes every terminal (→0) ──────────────────────
  {
    id: "terminal.killAll",
    specId: "L1.TERM.023",
    title: "Terminal: Kill All Terminals disposes every terminal",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env, creates three terminals (count 3), FIRES
'workbench.action.terminal.killAll', waits 2.5s, and asserts terminalCount == 0.

WHY THIS IS THE EXPECTED OUTCOME: killAll is the bulk-teardown command — it disposes
every Terminal object and every backing pty, not just the active one. So from three
terminals the count must fall to 0. We FIRE (not act) because, like single kill, the
disposal leaves the executeCommand reply hanging headlessly; observation after a
settle is the source of truth.

WHY IT MATTERS: resource hygiene across a soak run depends on bulk teardown actually
reaping all ptys. A regression where killAll disposes only the active terminal (or
none) would leak shells round after round, inflating the container's process and
memory footprint. Pre-seeding three terminals proves killAll clears ALL of them, not
just one.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.terminal.new");
        await sleep(1500);
      }
      const before = await env.observe("term.killAll.before");
      env.fire("workbench.action.terminal.killAll");
      await sleep(2500);
      const after = await env.observe("term.killAll.after");
      return {
        pass: before.vscode.terminalCount === 3 && after.vscode.terminalCount === 0,
        detail: `count ${before.vscode.terminalCount}→${after.vscode.terminalCount} after killAll (want 3→0)`,
        evidence: { before: before.vscode.terminalCount, after: after.vscode.terminalCount },
      };
    },
  },

  // ── L1.TERM.031 — runCommand output captured to a file, read back ────────────
  {
    id: "terminal.runToFile",
    specId: "L1.TERM.031",
    title: "Terminal: command output redirected to a file reads back deterministically",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a terminal, sends "printf 'fleet-run-%s\\n' OK > /tmp/fleet-run.txt" via
termSend (which appends a newline so the line runs), then polls fileContent for
/tmp/fleet-run.txt until it contains "fleet-run-OK".

WHY THIS IS THE EXPECTED OUTCOME: redirecting command output to a file and reading it
through fileContent is the DETERMINISTIC capture path. terminalText scraping depends
on shell-integration buffer-render timing and is racy; a file the shell wrote to disk
is unambiguous. A match proves termSend delivered the complete command line (not a
truncated prefix) and the shell ran it to completion, flushing the marker to disk.

WHY IT MATTERS: this is the reliable substrate the racier terminalText assertions
back-stop. If termSend ever started truncating long command lines, or dropped the
appended newline (typed-but-never-run), the file would be absent or empty — a
distinguishable evidence state. Guards the keystroke-delivery + shell-execution path
that every file-based terminal assertion relies on.`,
    async run(env) {
      const path = "/tmp/fleet-run.txt";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.request({ type: "termSend", text: `printf 'fleet-run-%s\\n' OK > ${path}` });
      const { hit, text } = await waitForFile(env, path, "fleet-run-OK");
      await env.observe("term.runToFile.after");
      return {
        pass: hit,
        detail: hit ? `file contains fleet-run-OK` : `file lacked marker (got ${JSON.stringify(text.trim().slice(-120))})`,
        evidence: { path, got: text.trim() },
      };
    },
  },

  // ── L1.TERM.032 — a failing command's nonzero exit is observable ─────────────
  {
    id: "terminal.exitCode",
    specId: "L1.TERM.032",
    title: "Terminal: a failing command's nonzero exit ($?) is captured",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a terminal, sends "false; echo rc=$? > /tmp/fleet-rc.txt" via termSend,
then polls fileContent for /tmp/fleet-rc.txt until it contains "rc=1".

WHY THIS IS THE EXPECTED OUTCOME: 'false' exits with status 1, so $? immediately after
is 1, and the redirect writes "rc=1" to the file. Capturing $? proves the shell
actually EXECUTED the failing command — a command that never ran would leave $? at the
previous (likely 0) value or the file absent entirely. The ';' sequences the two
commands so the echo runs after false regardless of its exit code.

WHY IT MATTERS: agents must distinguish "ran and failed" from "never ran" — they react
very differently (retry the command vs fix the harness). If termSend silently dropped
the command, the file would be absent (a distinguishable state), not show rc=0. This
guards that a nonzero-exit command still runs to completion and its status is
readable, the foundation of agent error-handling over the terminal.`,
    async run(env) {
      const path = "/tmp/fleet-rc.txt";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.request({ type: "termSend", text: `false; echo rc=$? > ${path}` });
      const { hit, text } = await waitForFile(env, path, "rc=1");
      await env.observe("term.exitCode.after");
      return {
        pass: hit,
        detail: hit ? `file contains rc=1` : `file lacked rc=1 (got ${JSON.stringify(text.trim().slice(-120))})`,
        evidence: { path, got: text.trim() },
      };
    },
  },

  // ── L1.TERM.033 — chained && command runs as one shell line ──────────────────
  {
    id: "terminal.chainedCommand",
    specId: "L1.TERM.033",
    title: "Terminal: an &&-chained command line runs intact and in order",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a terminal, sends
"cd /tmp && pwd > /tmp/fleet-chain.txt && echo done >> /tmp/fleet-chain.txt" via
termSend, then polls fileContent for /tmp/fleet-chain.txt and asserts it contains
BOTH "/tmp" and "done".

WHY THIS IS THE EXPECTED OUTCOME: the whole &&-chain is one command line; the shell
runs it left-to-right, each step gated on the prior's success. 'cd /tmp' makes pwd
print /tmp, then 'echo done' appends 'done'. Both tokens present means the entire
chain was delivered intact and executed sequentially — not split at the first '&&' or
at a space.

WHY IT MATTERS: termSend must deliver a full command line verbatim. A regression that
truncates at '&&', a space, or a special char would leave only the first segment's
output ("/tmp" without "done" — a distinguishable partial-evidence state) and silently
break every agent shell pipeline that chains commands. Asserting both tokens pins
in-order, intact delivery.`,
    async run(env) {
      const path = "/tmp/fleet-chain.txt";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.request({
        type: "termSend",
        text: `cd /tmp && pwd > ${path} && echo done >> ${path}`,
      });
      // poll until both tokens present
      let text = "";
      let both = false;
      for (let i = 0; i < 14; i++) {
        await sleep(500);
        const r = await env.request({ type: "fileContent", path }).catch(() => null);
        text = field(r, "text") || "";
        if (text.includes("/tmp") && text.includes("done")) { both = true; break; }
      }
      await env.observe("term.chainedCommand.after");
      return {
        pass: both,
        detail: both ? `file has /tmp and done` : `chain incomplete (got ${JSON.stringify(text.trim().slice(-120))})`,
        evidence: { path, got: text.trim() },
      };
    },
  },

  // ── L1.TERM.034 — terminalText on a never-run terminal returns empty, not error
  {
    id: "terminal.textEmptyClean",
    specId: "L1.TERM.034",
    title: "Terminal: terminalText on an empty terminal returns ok+empty, never errors",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["terminalText"],
    rationale: `
WHAT: Opens a terminal and IMMEDIATELY (no command sent) issues a terminalText query,
asserting the reply does not throw (env.request throws on ok:false — reaching the
assert means ok:true) and that source ∈ {"", "buffer"} with text being a string.

WHY THIS IS THE EXPECTED OUTCOME: a freshly created terminal whose buffer has captured
nothing yet must return cleanly — ok:true, empty (or prompt-only) text, source "".
The bridge's terminalText handler reads the per-terminal buffer Map, defaulting to ""
when the key is absent, and reports source "" when empty — by construction it never
errors on an empty buffer. We accept source "buffer" too, because shell-integration
may emit a prompt line before we query; the contract is "no error", not "exactly
empty".

WHY IT MATTERS: every waitForTerminalText poll calls terminalText repeatedly while the
buffer is still empty; if that returned an error reply, the polling helper would
fail-fast and falsely report "broken" instead of "not yet". This guards the empty-
buffer path so the suite's polling assertions can distinguish "no output yet" from a
genuine bridge fault.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      let ok = false;
      let text = null;
      let source = null;
      try {
        const r = await env.request({ type: "terminalText" });
        ok = !!(r && r.ok !== false);
        text = field(r, "text");
        source = field(r, "source");
      } catch {
        ok = false;
      }
      const textOk = typeof text === "string";
      const sourceOk = source === "" || source === "buffer" || source === undefined;
      return {
        pass: ok && textOk && sourceOk,
        detail: `terminalText ok=${ok} source=${JSON.stringify(source)} textLen=${typeof text === "string" ? text.length : "n/a"}`,
        evidence: { ok, source, sample: typeof text === "string" ? text.slice(0, 120) : text },
      };
    },
  },

  // ── L1.TERM.051 — Snapshot lists every open terminal name (len==count==3) ─────
  {
    id: "terminal.snapshotNames",
    specId: "L1.TERM.051",
    title: "Terminal: terminals.length == terminalCount == 3 with non-empty names",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["query"],
    rationale: `
WHAT: In a fresh env, creates three terminals and asserts the snapshot invariant
terminals.length == terminalCount == 3, AND that every entry in terminals is a
non-empty string.

WHY THIS IS THE EXPECTED OUTCOME: terminals and terminalCount are two views of the
same window.terminals — the array is the names, the count is its length — so they must
never drift. Three creates yield three named Terminal objects; VS Code always assigns
a non-empty name (defaulting to the shell, e.g. "bash"), so no entry may be empty.

WHY IT MATTERS: behaviours that route termSend/terminalText by name read the names
from this array; if the array and count ever desync (count includes a disposed
terminal, or a name drops to ""), every name-routed assertion silently mis-targets a
terminal — failing for the wrong reason or, worse, passing against the wrong shell.
Pinning length == count and non-empty names guards that addressing substrate.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.terminal.new");
        await sleep(1500);
      }
      const snap = (await env.observe("term.snapshotNames.after")).vscode;
      const names = Array.isArray(snap.terminals) ? snap.terminals : [];
      const allNonEmpty = names.length > 0 && names.every((n) => typeof n === "string" && n.length > 0);
      return {
        pass: snap.terminalCount === 3 && names.length === 3 && allNonEmpty,
        detail: `terminalCount=${snap.terminalCount} terminals.length=${names.length} names=${JSON.stringify(names)}`,
        evidence: { terminalCount: snap.terminalCount, terminals: names },
      };
    },
  },

  // ── L1.TERM.061 — Clear does not dispose the terminal (count stays 1) ─────────
  {
    id: "terminal.clearKeepsCount",
    specId: "L1.TERM.061",
    title: "Terminal: Clear does not dispose the terminal (count unchanged)",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env, creates one terminal (count 1), runs
'workbench.action.terminal.clear', waits 1.5s, and asserts terminalCount is 1 both
before AND after — clear changes the buffer, not the lifecycle.

WHY THIS IS THE EXPECTED OUTCOME: clear scrubs the visible scrollback of the active
terminal but keeps the Terminal object and its backing pty alive. So the count must be
invariant across clear (1 → 1). This deliberately separates clear from kill, two
superficially similar "make the terminal empty" operations with opposite lifecycle
semantics.

WHY IT MATTERS: an agent that clears to reduce output noise mid-session must not lose
its shell. A regression where clear disposed the pty (or where the command id got
mis-wired to a kill) would surprise the agent with a dead terminal. The count-
invariant is the assertable contract that clear is a buffer op, never a lifecycle op.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const before = await env.observe("term.clearKeepsCount.before");
      await env.act("workbench.action.terminal.clear");
      await sleep(1500);
      const after = await env.observe("term.clearKeepsCount.after");
      return {
        pass: before.vscode.terminalCount === 1 && after.vscode.terminalCount === 1,
        detail: `count ${before.vscode.terminalCount}→${after.vscode.terminalCount} after clear (want 1→1)`,
        evidence: { before: before.vscode.terminalCount, after: after.vscode.terminalCount },
      };
    },
  },

  // ── L1.TERM.070 — Focus Terminal: echo into the active terminal lands ────────
  {
    id: "terminal.focusActiveEcho",
    specId: "L1.TERM.070",
    title: "Terminal: after focus, a name-less termSend lands in the active terminal",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "termSend", "terminalText"],
    rationale: `
WHAT: In a fresh env, opens a terminal, runs 'workbench.action.terminal.focus' (asserts
it resolves ok via act), then sends "echo FLEET_FOCUSED" via termSend WITHOUT a name —
which targets the ACTIVE terminal — and polls terminalText (by the termSend reply's
terminal name) until the buffer contains FLEET_FOCUSED.

WHY THIS IS THE EXPECTED OUTCOME: the Snapshot exposes no focus/active-terminal field,
so focus is asserted INDIRECTLY: a name-less termSend goes to whatever terminal is
active, and the bridge records the sent line into that terminal's buffer; terminalText
reading the marker back proves the just-focused terminal is the active one receiving
input. The marker FLEET_FOCUSED is chosen so a match means our send genuinely landed,
not that the word pre-existed.

WHY IT MATTERS: focus is a real workbench op agents use to direct subsequent input; if
focus errored, or if name-less termSend stopped routing to the active terminal, input
would land in the wrong (or no) shell. This is the best available proxy until the
Snapshot grows an activeTerminal field — the rationale records that Track-E gap so the
indirection is understood, not mistaken for the real assertion.`,
    async run(env) {
      const MARKER = "FLEET_FOCUSED";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      let focusOk = true;
      try {
        await env.act("workbench.action.terminal.focus");
      } catch {
        focusOk = false;
      }
      await sleep(800);
      const sent = await env.request({ type: "termSend", text: `echo ${MARKER}` });
      const name = sent && sent.terminal;
      let text = "";
      let hit = false;
      for (let i = 0; i < 15; i++) {
        await sleep(800);
        const r = await env.request({ type: "terminalText", ...(name ? { name } : {}) }).catch(() => null);
        text = field(r, "text") || "";
        if (text.includes(MARKER)) { hit = true; break; }
      }
      await env.observe("term.focusActiveEcho.after");
      return {
        pass: focusOk && hit,
        detail: focusOk
          ? (hit ? `marker landed in active terminal ${JSON.stringify(name)}` : `marker never appeared (tail ${JSON.stringify(text.slice(-120))})`)
          : `focus command failed`,
        evidence: { focusOk, terminal: name, tail: text.slice(-200) },
      };
    },
  },

  // ── L1.TERM.072 — Toggle Terminal panel: reversible, never disposes ──────────
  {
    id: "terminal.togglePanel",
    specId: "L1.TERM.072",
    title: "Terminal: Toggle Terminal panel twice keeps the terminal (count invariant)",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env, opens one terminal, then runs
'workbench.action.terminal.toggleTerminal' TWICE (asserting each resolves ok via act),
and asserts terminalCount == 1 throughout (before, between is implied, after).

WHY THIS IS THE EXPECTED OUTCOME: toggling the terminal PANEL is a view-state op
(show ↔ hide), not a lifecycle op. Two toggles return the panel to its starting
visibility and must never spawn or dispose a pty — so the terminal count is invariant
at 1. The Snapshot has no panel-visibility field, so the assertable contract is
exactly "both commands ok AND count unchanged"; the rationale records that Track-E
gap.

WHY IT MATTERS: a regression where toggle mis-wired to new/kill, or where hiding the
panel disposed its terminals, would silently change the terminal lifecycle on a pure
visibility action — surprising any agent that toggles the panel for layout. The count-
invariant guards that toggle touches only the view, never the ptys.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const before = await env.observe("term.togglePanel.before");
      let ok = true;
      try {
        await env.act("workbench.action.terminal.toggleTerminal");
        await sleep(800);
        await env.act("workbench.action.terminal.toggleTerminal");
        await sleep(800);
      } catch {
        ok = false;
      }
      const after = await env.observe("term.togglePanel.after");
      return {
        pass: ok && before.vscode.terminalCount === 1 && after.vscode.terminalCount === 1,
        detail: `toggles ok=${ok}; count ${before.vscode.terminalCount}→${after.vscode.terminalCount} (want 1→1)`,
        evidence: { commandsOk: ok, before: before.vscode.terminalCount, after: after.vscode.terminalCount },
      };
    },
  },

  // ── L1.TERM.080 — Default profile spawns bash ────────────────────────────────
  {
    id: "terminal.defaultProfileBash",
    specId: "L1.TERM.080",
    title: "Terminal: the default terminal profile is bash",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a terminal via the default 'workbench.action.terminal.new', sends
'echo "$0-$BASH_VERSION" > /tmp/fleet-shell.txt' via termSend, then polls fileContent
and asserts the file contains "bash" AND a version digit (\\d).

WHY THIS IS THE EXPECTED OUTCOME: the proven container baseline (terminal.new
evidence) shows the default integrated terminal is bash. In bash, $BASH_VERSION is a
non-empty version string (e.g. "5.1.16(1)-release"), so the captured line contains
"bash" and digits. A dash/sh shell would leave $BASH_VERSION empty and $0 would not be
bash — a distinguishable miss. We read via a redirected file (deterministic) rather
than the racy terminalText buffer.

WHY IT MATTERS: nearly every terminal behaviour assumes bash semantics — &&-chaining,
redirections, $BASH_VERSION, $?. If an image change swapped the default profile to
sh/dash, those behaviours would break in confusing ways downstream. Pinning the
default profile to bash here makes profile drift a single, clear failure instead of a
cascade of cryptic ones.`,
    async run(env) {
      const path = "/tmp/fleet-shell.txt";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.request({ type: "termSend", text: `echo "$0-$BASH_VERSION" > ${path}` });
      const { text } = await waitForFile(env, path, "bash");
      await env.observe("term.defaultProfileBash.after");
      const hasBash = /bash/.test(text);
      const hasDigit = /\d/.test(text);
      return {
        pass: hasBash && hasDigit,
        detail: hasBash && hasDigit
          ? `default shell is bash (${JSON.stringify(text.trim().slice(-80))})`
          : `not clearly bash (got ${JSON.stringify(text.trim().slice(-120))})`,
        evidence: { path, got: text.trim() },
      };
    },
  },

  // ── L1.TERM.082 — newWithProfile with a bogus name fails cleanly (bounded) ────
  {
    id: "terminal.badProfileBounded",
    specId: "L1.TERM.082",
    title: "Terminal: newWithProfile with an unknown profile is bounded, never hangs",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env (count 0), FIRES
'workbench.action.terminal.newWithProfile' with a bogus profile name
(["no-such-profile"]), waits, then proves the bridge stays responsive by round-
tripping a query, and asserts the resulting terminalCount ∈ {0, 1}.

WHY THIS IS THE EXPECTED OUTCOME: a bad profile name must surface as a BOUNDED result —
either no terminal is created (count stays 0) or the command falls back to the default
profile (count 1) — but the bridge must NOT wedge waiting on a never-resolving
command. We FIRE rather than act() precisely because this command can otherwise block
awaiting a profile picker; firing decouples us from its reply, and the follow-up query
is the proof the bridge didn't hang. count ∈ {0,1} accepts both legitimate outcomes
(no-op or default fallback).

WHY IT MATTERS: a typo'd or stale profile name (from a config or an agent guess) must
not brick the env by hanging the bridge on an open picker. This guards the command-arg
error path: the outcome is bounded and the bridge stays alive, regardless of which of
the two acceptable terminal-count outcomes occurs.`,
    async run(env) {
      const before = await env.observe("term.badProfileBounded.before");
      env.fire("workbench.action.terminal.newWithProfile", ["no-such-profile"]);
      await sleep(3000);
      // bridge must still answer a query (not wedged on a picker).
      let responsive = false;
      let count = -1;
      try {
        const r = await env.request({ type: "query" });
        responsive = !!(r && r.ok !== false);
        count = field(r, "data")?.terminalCount;
        if (count === undefined) count = (await env.observe("term.badProfileBounded.after")).vscode.terminalCount;
      } catch {
        responsive = false;
      }
      return {
        pass: before.vscode.terminalCount === 0 && responsive && (count === 0 || count === 1),
        detail: `responsive=${responsive}; terminalCount=${count} (want 0 or 1)`,
        evidence: { responsive, count },
      };
    },
  },

  // ── L1.TERM.090 — Run Build Task with no tasks.json: bounded, count unchanged ─
  {
    id: "terminal.buildNoTasks",
    specId: "L1.TERM.090",
    title: "Terminal: Run Build Task with no tasks.json returns, no phantom terminal",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env with no .vscode/tasks.json, FIRES
'workbench.action.tasks.build' (the native Terminal-menu id), waits, then proves the
bridge stays responsive via a follow-up query and asserts terminalCount is unchanged
(no phantom task terminal spawned).

WHY THIS IS THE EXPECTED OUTCOME: with no build task configured, the command must be a
bounded no-op — it surfaces a "no build task" notification and does NOT spawn a
terminal or block awaiting a task picker. We FIRE rather than act() because the task
command can otherwise hang on a picker/notification round-trip headlessly; the follow-
up query proves the bridge didn't wedge. The Snapshot exposes no task/notification
field, so the assertable observable is "bridge responsive AND terminalCount
unchanged" — the rationale records that Track-E gap.

WHY IT MATTERS: a misclick on a task menu entry in an unconfigured workspace must not
wedge the env on an open picker or litter it with phantom terminals. This guards the
empty-tasks branch of the task surface so the menu's build/run entries degrade
gracefully rather than hanging the bridge.`,
    async run(env) {
      const before = await env.observe("term.buildNoTasks.before");
      env.fire("workbench.action.tasks.build");
      await sleep(3000);
      let responsive = false;
      let count = -1;
      try {
        const r = await env.request({ type: "query" });
        responsive = !!(r && r.ok !== false);
        count = field(r, "data")?.terminalCount;
        if (count === undefined) count = (await env.observe("term.buildNoTasks.after")).vscode.terminalCount;
      } catch {
        responsive = false;
      }
      return {
        pass: responsive && count === before.vscode.terminalCount,
        detail: `responsive=${responsive}; terminalCount ${before.vscode.terminalCount}→${count} (want unchanged)`,
        evidence: { responsive, before: before.vscode.terminalCount, after: count },
      };
    },
  },

  // ── L1.TERM.103 — Kill then recreate reuses a clean slot (count 1) ───────────
  {
    id: "terminal.killThenRecreate",
    specId: "L1.TERM.103",
    title: "Terminal: New → Kill → New settles to terminalCount 1 (no residue)",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env, runs New (count 1), FIRES kill (count → 0), waits 2.5s, then runs
New again, and asserts the final terminalCount == 1 — not 2 (the dead one resurrected)
and not 0 (the second create lost).

WHY THIS IS THE EXPECTED OUTCOME: kill fully disposes the first terminal and its pty
before the second New runs, so window.terminals ends with exactly the one live
terminal. A count of 2 would mean the killed terminal was still counted (not reaped);
a count of 0 would mean the second create failed. We FIRE the kill (its executeCommand
doesn't resolve headlessly) and settle 2.5s so disposal completes before the recreate.

WHY IT MATTERS: a dispose/recreate cycle that left the dead terminal in the snapshot
would inflate the count across a soak and confuse name-routing (a stale name lingering
in the terminals array). This guards lifecycle cleanliness — dispose-then-recreate
must leave no residue in the snapshot, the count returning to exactly 1.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const opened = await env.observe("term.killThenRecreate.opened");
      env.fire("workbench.action.terminal.kill");
      await sleep(2500);
      const killed = await env.observe("term.killThenRecreate.killed");
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const after = await env.observe("term.killThenRecreate.after");
      return {
        pass:
          opened.vscode.terminalCount === 1 &&
          killed.vscode.terminalCount === 0 &&
          after.vscode.terminalCount === 1,
        detail: `count 1→${killed.vscode.terminalCount}(kill)→${after.vscode.terminalCount}(recreate) (want 1→0→1)`,
        evidence: {
          opened: opened.vscode.terminalCount,
          killed: killed.vscode.terminalCount,
          after: after.vscode.terminalCount,
        },
      };
    },
  },
];

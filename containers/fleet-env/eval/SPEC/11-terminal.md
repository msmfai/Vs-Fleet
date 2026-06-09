# L1.TERM — Integrated terminals

In-env terminal surface: create / split / kill / runCommand / cwd / multiple /
output-capture / clear / focus / profiles / tasks. Driven through the bridge
`command` action (built-in `workbench.action.terminal.*` ids, sourced from the
native menu in `crates/fleet-host/src/mux.rs` and the behaviours in
`behaviours/terminal.mjs` + `behaviours/terminal_more.mjs`). Asserted via the
`query` Snapshot (`terminals:string[]`, `terminalCount:number`) and the
`terminalText` / `fileContent` queries, plus container `exec` for the backing pty
processes.

Capability tokens used here (from the bridge `hello.caps`): `command`, `query`,
`termSend`, `terminalText`, `fileContent`, `writeFile`. Behaviours that list a
cap in `needs:` SKIP cleanly if the bridge does not advertise it.

Workspace root in this image: `/home/coder/project`. The Snapshot exposes only a
flat `terminals` name list + `terminalCount` — it does NOT expose group/pane
membership, focus, active-terminal, profile, or shell-process pid. Where a test
needs those, it asserts a proxy observable (a file written by the shell, a
`terminalText` buffer, or a container `exec` of `pgrep`/`ps`) and the `why`
records that the direct field is missing (drives Track-E Snapshot extensions).

---

## Create / new

### L1.TERM.001 — New Terminal creates one integrated terminal
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [command, query]
- precondition: bridge connected and answering `query`; any starting terminalCount N (shared env)
- action: `command` `workbench.action.terminal.new`
- expected: terminalCount strictly increases (N → >N); a new pty/shell is registered
- assert: `query` Snapshot `terminalCount`(after) > `terminalCount`(before)
- machine-state: container procs +1..+2 (one bash pty); mem Δ ≈ +50..60 MiB (proven baseline: 10→12 procs, +58 MiB)
- why: canonical terminal-creation command must spawn a REAL backing pty registered in `window.terminals`; this is the canary for the whole act→effect→observe bridge round-trip — a green here narrows any richer terminal failure to that behaviour's own specifics.
- status: implemented (behaviour `terminal.new`)

### L1.TERM.002 — New Terminal from a fresh env yields exactly terminalCount 1
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env, terminalCount == 0 (Snapshot `terminals` == [])
- action: `command` `workbench.action.terminal.new`; wait 2s
- expected: terminalCount == 1, `terminals.length` == 1
- assert: `query` `terminalCount` == 1 (exact, not just monotonic — fresh env controls the 0→1)
- why: the monotonic-only `terminal.new` can't catch a regression that creates TWO terminals (e.g. a duplicate-fire bug) or that mis-counts; a fresh env pins the exact 0→1 and guards the count arithmetic the split/kill tests depend on.
- status: TODO

### L1.TERM.003 — Repeated New Terminal accumulates distinct terminals
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env, terminalCount == 0
- action: fire `workbench.action.terminal.new` three times with 1.5s gaps
- expected: terminalCount == 3; `terminals` has 3 entries (default names e.g. "bash"/"bash", VS Code disambiguates)
- assert: `query` `terminalCount` == 3 after the third create
- edges: repeat / accumulation edge for create — no command dedupes or replaces
- machine-state: procs +3 (three ptys); mem grows ~linearly per terminal
- why: New must always ADD, never reuse the active terminal; a regression that focuses an existing terminal instead of creating one would silently cap the count — only repeated creates expose it.
- status: TODO

---

## Split

### L1.TERM.010 — Split Terminal adds a second pane to the active group
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; one terminal created first (`workbench.action.terminal.new`, count 1)
- action: `command` `workbench.action.terminal.split`; wait 2s
- expected: terminalCount == before+1 (1 → 2); the new pane shares the active terminal group
- assert: `query` `terminalCount`(after) == `terminalCount`(before) + 1 (exact, fresh env)
- machine-state: procs +1 (split pane is its own pty)
- why: split is distinct from New at the workbench level (joins a group vs standalone) but must still surface as a new Terminal object to the observer; a regress to count-stays-1 means split degraded to a no-op or the pane stopped registering. Snapshot exposes no group membership, so grouping is noted in detail only — flags a Track-E gap.
- status: implemented (behaviour `terminal.split`)

### L1.TERM.011 — Split with NO active terminal first creates one (0→1, not error)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env, terminalCount == 0 (no terminal to split)
- action: `command` `workbench.action.terminal.split` directly (no prior New)
- expected: command does not error; terminalCount becomes 1 (split with nothing to split creates the first terminal)
- assert: `command` reply `ok` == true; `query` `terminalCount` transitions 0 → 1
- edges: missing-precondition edge for split (empty terminal state)
- why: split on an empty workbench must degrade to create-one, not throw or hang the bridge; guards the "no active terminal" branch so agents that blind-fire split don't dead-end the env.
- status: TODO

### L1.TERM.012 — Splitting a split deepens the group (3 panes, one group)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; New then Split applied (terminalCount 2, one group)
- action: `command` `workbench.action.terminal.split` again; wait 2s
- expected: terminalCount == 3
- assert: `query` `terminalCount` == 3
- edges: repeat edge for split — each split adds exactly one pane
- machine-state: procs +1 per split
- why: split must be idempotent-in-shape (always +1), never collapse multiple panes into one group object; a count that plateaus would mean later splits silently no-op.
- status: TODO

---

## Kill / dispose

### L1.TERM.020 — Kill Terminal disposes the active terminal
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; one terminal opened (count >= 1)
- action: FIRE (not await) `workbench.action.terminal.kill` via `env.fire`; wait 2.5s
- expected: terminalCount drops below the opened count (1 → 0)
- assert: `query` `terminalCount`(after) < `terminalCount`(opened) and opened >= 1
- machine-state: procs −1 (the backing pty is terminated, not orphaned)
- why: teardown half of the lifecycle and the ONLY proof a terminal can be disposed (not just created) — leaked ptys accumulate. NON-OBVIOUS: kill's `executeCommand` promise does NOT resolve headlessly (disposal leaves the RPC reply hanging), so it must be FIRED not awaited; a hang here points at the fire-vs-act distinction, a count-unchanged points at kill itself.
- status: implemented (behaviour `terminal.kill`)

### L1.TERM.021 — Kill with no terminals open is a clean no-op
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env, terminalCount == 0
- action: FIRE `workbench.action.terminal.kill`; wait 2.5s
- expected: terminalCount stays 0; bridge stays responsive (subsequent `query` still answers)
- assert: `query` `terminalCount` == 0 before AND after; a follow-up `query` round-trips ok
- edges: empty-state edge for kill — nothing to dispose
- why: kill against an empty workbench must not throw, must not hang the bridge, and must not desync the snapshot; guards the no-active-terminal branch and proves the fire path doesn't wedge when there is no disposal to do.
- status: TODO

### L1.TERM.022 — Kill one of several leaves the rest alive
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; three terminals created (count 3)
- action: FIRE `workbench.action.terminal.kill` once; wait 2.5s
- expected: terminalCount == 2 (only the active one disposed)
- assert: `query` `terminalCount` == 2 (exactly one removed)
- machine-state: procs −1 (exactly one pty terminated, two remain — `exec` `pgrep -c bash` drops by 1)
- edges: concurrent-state edge — kill targets only the active terminal, not all
- why: kill must dispose exactly the active terminal, never the whole group or all terminals; a regress that drops count to 0 would silently destroy an agent's other live shells.
- status: TODO

### L1.TERM.023 — Kill All Terminals disposes every terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; three terminals created (count 3)
- action: FIRE `workbench.action.terminal.killAll`; wait 2.5s
- expected: terminalCount == 0
- assert: `query` `terminalCount` == 0; `exec` `pgrep -c bash` returns 0 pty shells
- machine-state: procs back to the pre-terminal baseline (all ptys reaped)
- why: bulk teardown for resource hygiene — verifies killAll reaps every pty, not just the active one, so a soak run doesn't leak shells across rounds.
- status: TODO

---

## Run command / output capture

### L1.TERM.030 — echo marker round-trips through the terminal (termSend → terminalText)
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [termSend, terminalText]
- precondition: a terminal open (created in-test); shell integration available
- action: `termSend` `echo FLEET_OK` (termSend appends `\n` so it actually runs); poll `terminalText` (15×800ms)
- expected: the terminal's text buffer contains `FLEET_OK`
- assert: `terminalText` `{text}` (named to the `termSend` reply's `terminal`) `.includes("FLEET_OK")`
- why: first proof the terminal is a LIVE interactive shell (runs a command and returns output), not just a Terminal object. FLEET_OK is a marker the prompt itself won't contain, so a match means the echo truly executed and emitted stdout — not merely that we typed the word. Breakage with `terminal.new` green isolates the Track-E I/O path (keystroke delivery / newline-append / output-stream capture).
- status: implemented (behaviour `terminal.runEcho`)

### L1.TERM.031 — runCommand output is captured to a file and read back deterministically
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; one terminal open
- action: `termSend` `printf 'fleet-run-%s\n' OK > /tmp/fleet-run.txt`; poll `fileContent` `/tmp/fleet-run.txt` (12×500ms)
- expected: the file contains `fleet-run-OK`
- assert: `fileContent` `{text}` `.includes("fleet-run-OK")`
- why: terminalText output capture depends on shell-integration buffer-render timing and is RACY; redirecting to a file read via `fileContent` is the deterministic capture path. Guards that termSend delivers a complete command line (not truncated) and the shell runs it to completion — the reliable substrate the racy buffer assertions back-stop.
- status: TODO

### L1.TERM.032 — A failing command's nonzero exit is observable
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; one terminal open
- action: `termSend` `false; echo rc=$? > /tmp/fleet-rc.txt`; poll `fileContent` `/tmp/fleet-rc.txt`
- expected: the file contains `rc=1`
- assert: `fileContent` `{text}` `.includes("rc=1")`
- edges: failure-mode edge for runCommand — a command that exits nonzero still runs and its status is readable
- why: agents must distinguish a command that ran-and-failed from one that never ran; capturing `$?` proves the shell executed the failing command rather than the termSend silently dropping it (which would leave the file absent, a distinguishable evidence state).
- status: TODO

### L1.TERM.033 — Multi-line / chained command runs as one shell line
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; one terminal open
- action: `termSend` `cd /tmp && pwd > /tmp/fleet-chain.txt && echo done >> /tmp/fleet-chain.txt`
- expected: file contains both `/tmp` and `done` (the chain ran in order)
- assert: `fileContent` `{text}` `.includes("/tmp")` AND `.includes("done")`
- edges: complex-input edge — `&&`-chained command line is delivered intact and executes sequentially
- why: termSend must deliver a full chained command line without splitting at `&&`/spaces; a regress that truncates at the first separator would leave only `/tmp` (a distinguishable partial-evidence state) and silently break agent shell pipelines.
- status: TODO

### L1.TERM.034 — terminalText on an empty/never-run terminal returns empty, not error
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [terminalText]
- precondition: fresh env; one terminal open, no command sent
- action: `terminalText` query immediately after create
- expected: reply `ok` == true with `text` == "" (or prompt-only) and `source` == "" (empty) — no throw
- assert: `terminalText` reply `ok` == true; `source` ∈ {"", "buffer"}; never an error reply
- edges: empty-state edge for output capture — buffer queried before any output exists
- why: querying a fresh terminal's buffer must return cleanly (empty) so a polling assertion can distinguish "not yet" from "broken"; an error reply here would make every `waitForTerminalText` poll falsely fail-fast.
- status: TODO

---

## CWD

### L1.TERM.040 — New terminal opens in the workspace project root
- layer: L1
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; one terminal open
- action: `termSend` `pwd > /tmp/fleet-cwd.txt`; poll `fileContent` `/tmp/fleet-cwd.txt` (12×500ms)
- expected: file text contains `/home/coder/project`
- assert: `fileContent` `{text}` `.includes("/home/coder/project")`
- why: VS Code opens integrated terminals at the folder root by default; if the cwd default regresses (workspace-layout change, `terminal.integrated.cwd` override, or a `.bashrc` `cd`), an agent silently runs builds/git/edits in the wrong directory with NO error. Reading via a redirected file (not the racy output buffer) makes the path unambiguous; empty file → termSend/write path broken, wrong path → workspace/container config.
- status: implemented (behaviour `terminal.cwd`)

### L1.TERM.041 — Split pane inherits the parent terminal's cwd
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; terminal created, `termSend` `cd /tmp` into it, then split
- action: into the SPLIT pane `termSend` `pwd > /tmp/fleet-split-cwd.txt`; poll `fileContent`
- expected: file contains `/tmp` (split inherits the active pane's cwd, not the workspace root)
- assert: `fileContent` `{text}` `.includes("/tmp")` and NOT the project root
- edges: state-derived edge — split cwd derives from the source pane, not a fresh root
- why: VS Code's split semantics inherit the parent cwd; a regress to root-cwd would surprise agents that `cd` then split expecting to stay put. Targeting the split pane also exercises the `termSend` `name?` routing (active vs named terminal).
- status: TODO

### L1.TERM.042 — no-folder scenario: terminal cwd falls back to home, not a missing folder
- layer: L1
- scenarios: [no-folder]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: env booted with NO `?folder` (no workspace root)
- action: open a terminal; `termSend` `pwd > /tmp/fleet-nofolder-cwd.txt`; poll `fileContent`
- expected: file contains a valid existing path (e.g. `/home/coder`), NOT a nonexistent project dir, and the shell did not error
- assert: `fileContent` `{text}` matches `^/home/coder` ; `exec` `test -d "$(cat /tmp/fleet-nofolder-cwd.txt)"` exits 0
- edges: missing-workspace edge for cwd — no folder root to anchor to
- why: with no workspace folder the terminal must still open at a real directory (home), not crash on an undefined root; guards the no-folder boot path so terminal behaviours degrade rather than fail there.
- status: TODO

---

## Multiple terminals / naming

### L1.TERM.050 — Two named terminals are independently addressable
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent, query]
- precondition: fresh env; two terminals created (count 2)
- action: `termSend` `{name:<first>, text:"echo A > /tmp/fleet-a.txt"}` then `{name:<second>, text:"echo B > /tmp/fleet-b.txt"}` (names from Snapshot `terminals`)
- expected: `/tmp/fleet-a.txt` == "A" and `/tmp/fleet-b.txt` == "B" — each command ran in its addressed terminal
- assert: `fileContent` `/tmp/fleet-a.txt` `.includes("A")` AND `/tmp/fleet-b.txt` `.includes("B")`; both files exist
- machine-state: `exec` `pgrep -c bash` == 2 (two live ptys)
- edges: concurrent edge — multiple terminals coexist and route independently by name
- why: `termSend` `name?` must route to the addressed terminal, not always the active one; agents juggling a build shell + a server shell rely on this. If both writes land in one file, name routing collapsed to active-only.
- status: TODO

### L1.TERM.051 — Snapshot lists every open terminal name
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [query]
- precondition: fresh env; three terminals created
- action: `query`
- expected: `terminals.length` == 3 == `terminalCount`; names are non-empty strings
- assert: Snapshot `terminals.length` == `terminalCount` == 3; every entry is a non-empty string
- edges: invariant edge — `terminals` array length and `terminalCount` never drift
- why: behaviours address terminals by the names in `terminals`; if the array and count desync (count counts disposed terminals, or names drop) every name-routed assertion silently mis-targets. Pins the array↔count invariant.
- status: TODO

---

## Clear

### L1.TERM.060 — Clear Terminal empties the visible buffer
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, termSend, terminalText]
- precondition: fresh env; one terminal that has run `echo FLEET_BEFORE_CLEAR` (buffer contains the marker)
- action: confirm `terminalText` contains the marker, then `command` `workbench.action.terminal.clear`; wait 1.5s; re-query `terminalText`
- expected: after clear the buffer no longer contains `FLEET_BEFORE_CLEAR`
- assert: `terminalText`(before) `.includes("FLEET_BEFORE_CLEAR")` == true; `terminalText`(after) `.includes("FLEET_BEFORE_CLEAR")` == false
- why: clear must scrub the scrollback the shell-integration buffer tracks; an agent that clears to reduce noise before reading output relies on stale lines being gone. If the marker survives, clear no-opped or the buffer capture isn't honoring the clear sequence.
- status: TODO

### L1.TERM.061 — Clear does not dispose the terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; one terminal (count 1)
- action: `command` `workbench.action.terminal.clear`; wait 1.5s
- expected: terminalCount stays 1 (the terminal is cleared, not killed)
- assert: `query` `terminalCount` == 1 before AND after
- edges: distinguishing edge — clear vs kill (clear keeps the pty)
- why: clear must affect only the buffer, never the lifecycle; a regress that disposes on clear would surprise agents mid-session. Separates the two superficially-similar "make the terminal empty" operations.
- status: TODO

---

## Focus

### L1.TERM.070 — Focus Terminal reveals/focuses the terminal panel
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query, termSend, terminalText]
- precondition: fresh env; one terminal open; panel may be hidden
- action: `command` `workbench.action.terminal.focus`; then `termSend` (no `name`) `echo FLEET_FOCUSED` to the ACTIVE terminal; poll `terminalText`
- expected: the echo lands in the focused terminal's buffer (proxy for "this terminal is active/focused")
- assert: `terminalText` (active) `.includes("FLEET_FOCUSED")`; `command` reply `ok` == true
- why: Snapshot exposes no focus/active-terminal field, so focus is asserted indirectly — `termSend` without a name targets the ACTIVE terminal, so a successful echo into the just-focused terminal is the observable proxy. Flags a Track-E gap (Snapshot needs an `activeTerminal` field for a direct assertion).
- status: TODO

### L1.TERM.071 — Focus Next Terminal moves the active terminal
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query, termSend, fileContent]
- precondition: fresh env; two terminals created; active is the second
- action: `command` `workbench.action.terminal.focusNext`; `termSend` (no name, active) `echo $$ > /tmp/fleet-active-pid.txt`; compare against each terminal's known pid
- expected: the active terminal changed (the pid written matches the OTHER terminal than before focusNext)
- assert: `fileContent` `/tmp/fleet-active-pid.txt` pid differs from the pre-focusNext active pid (captured the same way)
- edges: navigation edge — focus cycles between terminals
- why: focusNext must actually move the active selection; without a Snapshot focus field, the shell's `$$` pid is the only ground-truth of which terminal is active. A regress that no-ops focusNext leaves the same pid (distinguishable). Flags the same Track-E `activeTerminal` gap.
- status: TODO

### L1.TERM.072 — Toggle Terminal panel shows then hides the panel
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; one terminal open
- action: `command` `workbench.action.terminal.toggleTerminal` twice (the id wired in the native menu, mux.rs:378)
- expected: both invocations return `ok` (toggle visible→hidden→visible) without disposing the terminal
- assert: `command` reply `ok` == true on both calls; `query` `terminalCount` == 1 throughout (toggle is visibility, not lifecycle)
- edges: repeat/visibility edge — toggle is reversible and never kills the terminal
- why: panel visibility is a view-state op, not a terminal-lifecycle op; toggling must never spawn or dispose a pty. Snapshot has no panel-visibility field, so the assertable invariant is "count unchanged, command ok" — flags a Track-E panel-visibility gap.
- status: partial(no Snapshot panel-visibility field; only count-invariant + ok asserted)

---

## Profiles

### L1.TERM.080 — Default profile spawns bash
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [termSend, fileContent]
- precondition: fresh env; one terminal open via default `workbench.action.terminal.new`
- action: `termSend` `echo "$0-$BASH_VERSION" > /tmp/fleet-shell.txt`; poll `fileContent`
- expected: file shows a bash shell (contains `bash` and a nonempty `BASH_VERSION`)
- assert: `fileContent` `/tmp/fleet-shell.txt` `.includes("bash")` and matches a version digit
- why: the proven baseline shows the default terminal is bash (`terminal.new` evidence); profile drift (image change to sh/dash) would break every behaviour assuming bash semantics (`$BASH_VERSION`, `&&`, redirections). Pins the default profile to bash.
- status: TODO

### L1.TERM.081 — Selecting a non-default profile spawns that shell
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, termSend, fileContent]
- precondition: fresh env; image provides at least two terminal profiles (e.g. bash + sh)
- action: `command` `workbench.action.terminal.newWithProfile` with the profile name arg (e.g. `["sh"]`); `termSend` `echo $0 > /tmp/fleet-prof.txt`
- expected: the spawned shell matches the selected profile (file shows `sh`, not bash)
- assert: `fileContent` `/tmp/fleet-prof.txt` reflects the chosen profile's shell; `query` `terminalCount` +1
- edges: alternate-profile edge — profile selection honored, not ignored to default
- why: agents/users may pick a specific shell; the profile arg must be honored. If the file shows bash regardless, profile selection collapsed to default. SKIPs cleanly if the image ships only one profile (a distinguishable degraded outcome, recorded in detail).
- status: TODO

### L1.TERM.082 — newWithProfile with an unknown profile name fails cleanly
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env, terminalCount == 0
- action: `command` `workbench.action.terminal.newWithProfile` with a bogus name (e.g. `["no-such-profile"]`)
- expected: either no terminal is created (count stays 0) OR it falls back to default (count 1) — but the bridge does NOT hang and stays responsive
- assert: `command` reply returns (ok or `ok:false`+error) within timeout; a follow-up `query` round-trips; `terminalCount` ∈ {0,1}
- edges: failure-mode edge for profiles — invalid profile arg
- why: a bad profile name must surface as a bounded result (error or default fallback), never wedge the bridge waiting on a never-resolving command; guards the command-arg error path so a typo'd profile doesn't hang the env.
- status: TODO

---

## Tasks (Terminal menu — run-task surface)

### L1.TERM.090 — Run Build Task with no tasks.json reports "no build task", does not hang
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; empty workspace (no `.vscode/tasks.json`)
- action: `command` `workbench.action.tasks.build` (the id wired in the native Terminal menu, mux.rs:426)
- expected: command returns (no build task configured → no-op/notification), bridge stays responsive; no terminal spawned
- assert: `command` reply returns within timeout; follow-up `query` round-trips; `terminalCount` unchanged
- edges: empty-state edge for tasks — no tasks configured
- why: invoking a task command in a workspace with no tasks must not hang awaiting a picker or spawn a phantom terminal; guards the menu's task entries so a misclick can't wedge the env. Snapshot has no task/notification field — flags a Track-E gap (the assertable observable is "returns + count unchanged").
- status: partial(no Snapshot task/notification field; only responsiveness + count-invariant asserted)

### L1.TERM.091 — Run Task with a defined task spawns a task terminal and runs it
- layer: L1
- scenarios: [small-repo]
- isolation: fresh
- needs: [command, writeFile, query, fileContent]
- precondition: fresh env; `writeFile` `.vscode/tasks.json` defining a shell task `echo FLEET_TASK > /tmp/fleet-task.txt`
- action: `command` `workbench.action.tasks.runTask` with the task label arg (the id wired at mux.rs:425)
- expected: a task terminal spawns (terminalCount +1) and the task's command writes the marker
- assert: `query` `terminalCount` +1; poll `fileContent` `/tmp/fleet-task.txt` `.includes("FLEET_TASK")`
- machine-state: procs +1..+2 (task shell)
- why: the run-task path is the menu surface for builds/scripts; it must actually launch the configured task in a real terminal and run it to completion. A missing output file means the task was picked but not executed (picker/launch break); a missing terminal means runTask no-opped.
- status: TODO

---

## Resilience / cross-cutting edges

### L1.TERM.100 — Terminals survive across a bridge reconnect (count preserved)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; two terminals created (count 2); bridge connection then dropped + redialed
- action: drop the bridge WS, let it reconnect (`hello` re-sent), then `query`
- expected: terminalCount still 2 — terminals are owned by the ext-host, not the bridge connection
- assert: `query` `terminalCount` == 2 after reconnect; the same `terminals` names present
- edges: failure/recovery edge — bridge transport churn must not desync the terminal snapshot
- why: the cmux model reattaches terminals across connection churn (see mux.rs header); a reconnect must not lose, double-count, or duplicate terminals. Guards that the Snapshot reads live ext-host state, not bridge-cached state.
- status: TODO

### L1.TERM.101 — no-network scenario: terminals still create and run commands
- layer: L1
- scenarios: [no-network]
- isolation: fresh
- needs: [command, termSend, fileContent]
- precondition: env booted `--network none` (reporter can't reach Hub; editor still drivable)
- action: open a terminal; `termSend` `echo offline > /tmp/fleet-offline.txt`; poll `fileContent`
- expected: terminal creates and the command runs (file contains `offline`) despite no network
- assert: `query` `terminalCount` +1; `fileContent` `/tmp/fleet-offline.txt` `.includes("offline")`
- edges: degraded-environment edge — terminal surface is local and must work offline
- why: the terminal/pty is purely local; phone-home failing must NOT impair terminal create/run. Pairs with the no-network reporter assertion (phone-home FAILS but commands WORK) to prove the failure is contained to networking.
- status: TODO

### L1.TERM.102 — Concurrent New + Split + termSend don't desync the count
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query, termSend]
- precondition: fresh env, terminalCount == 0
- action: fire `workbench.action.terminal.new`, `workbench.action.terminal.split`, and a `termSend` in quick succession (no inter-op settle), then settle 3s
- expected: terminalCount settles to a consistent value (2) and the Snapshot equals `terminals.length`
- assert: after settle, `query` `terminalCount` == `terminals.length` == 2 (no off-by-one, no orphaned entry)
- edges: concurrency edge — overlapping terminal ops must converge to a consistent snapshot
- why: agents fire terminal ops back-to-back; the snapshot must converge (no double-count from a race between create and split, no lost terminal). Guards the count/array invariant under overlap, the scenario most likely to expose a snapshot race.
- status: TODO

### L1.TERM.103 — Killed-then-recreated reuses a clean slot (no zombie pty)
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [command, query]
- precondition: fresh env; create a terminal, kill it (count 0), create another
- action: New → fire Kill → wait 2.5s → New → wait 2s
- expected: terminalCount == 1 (not 2, not 0); the killed pty is fully reaped before the new one
- assert: `query` `terminalCount` == 1 after the second New; `exec` `pgrep -c bash` == 1 (no zombie shell from the killed terminal)
- edges: lifecycle-cycle edge — dispose then recreate leaves no residue
- why: a kill that doesn't reap its pty would leave a zombie counted by `exec`, inflating procs across a soak; the second create must not resurrect the dead terminal's slot. Guards dispose/recreate cleanliness via both the Snapshot and the container process table.
- status: TODO

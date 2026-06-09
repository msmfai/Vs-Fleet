# Fleet VM Test Suite — Hyper-Specific Specification

The authoritative, exhaustive specification of Fleet's container ("VM") test suite.
Every entry here is a precise, individually-checkable test; the suite under
`containers/fleet-env/eval/` is the *implementation* of this spec. Each spec id maps
1:1 to a test id, and §traceability tracks coverage.

> Runtime: **fleet-env containers via Docker + colima** (the proven model — what
> "these VMs" has meant; Apple `container build` is unreliable here, see PLAN §8).
> The harness already exists (`run.mjs` + `lib/` + the bridge observe/act channel);
> this spec drives its expansion.

## Two layers

- **L1 — In-env behaviour surface.** What runs *inside* one environment: the VS Code
  editor/agent surface, driven via the bridge and asserted via the bridge snapshot /
  queries / machine-state. Exhaustive over commands, state transitions, and edges.
- **L2 — Fleet-stack E2E.** Fleet *itself* around the envs: env lifecycle, the Hub's
  state/rollup/persistence/reap, the reporter adapters + correlation + inference, the
  multiplexer (rail/bridge/menu/embed), agent-state flow, deploy/spawn/close, and
  networking. Spans the Rust crates + the harness, exercised through real containers.

## The test-entry format (every spec item MUST use this)

Each test is a numbered entry. Ids are `L{1,2}.<AREA>.<NNN>` (zero-padded, stable —
never renumber; append). Example:

```
### L1.TERM.003 — Split terminal spawns a second shell in the workspace root
- layer: L1
- scenarios: [base, small-repo]            # where it runs (smoke → all)
- isolation: fresh                          # fresh | shared
- needs: [termSend, terminalText]           # bridge caps (skip if absent)
- precondition: exactly one terminal open, cwd = /home/coder/project
- action: executeCommand "workbench.action.terminal.split"
- expected: terminalCount +1 (→2); the new pane's pwd == /home/coder/project
- assert: snapshot.terminalCount delta == +1; termSend `pwd > f`, fileContent(f) == path
- machine-state: procs +1..+3 (a shell process per pane); mem Δ < 80 MiB
- edges: split with NO active terminal → the command first creates one (0→1, not error)
- why: split must launch a REAL shell rooted in the workspace, not inherit a stale cwd
       or silently no-op; guards terminal-group + pane-shell spawning on refactor.
- status: implemented (behaviour `terminal.split`)   # implemented(<id>) | TODO | partial(<gap>)
```

Required fields: `layer, scenarios, precondition, action, expected, assert, why,
status`. Optional but encouraged: `isolation, needs, machine-state, edges`.

Rules for entries:
- **Hyper-specific.** State the EXACT command id / API / wire frame, the EXACT
  pre/post, and the EXACT assertion mechanism (which snapshot field / query / exec /
  Hub query). No "verify it works" — name the observable.
- **One observable per `expected`.** If an action has several effects, write several
  entries (or list each effect as a separate assert line).
- **Edges are first-class.** Every command has at least one edge entry (empty state,
  missing precondition, repeat, concurrent, failure mode).
- **Honest status.** `TODO` for unimplemented, `partial(<what's missing>)` for stubs.
  The implementation phase turns every entry into a behaviour/scenario.

## Conventions inherited from the suite

- **Provenance + rationale.** Each implemented test carries an auto-stamped git
  commit/date (its file's last change) and a full written rationale (the `why`
  expanded). See `behaviours/_contract.mjs`. The spec `why` is the seed of that.
- **Capabilities.** `needs:[…]` gates a test on bridge caps (see §3.3 / the bridge
  `hello.caps`); absent → clean SKIP. New asserts may require new caps — flag them.
- **Scenarios / expectBoot.** A test's `scenarios` lists where it runs (smoke → all).
  Failure scenarios (`expectBoot: fail|degraded`) assert the *boot outcome*; their
  behaviours record expected, never hang (bounded boot wait).
- **Determinism over realism where they conflict.** Prefer a controlled, reliable
  trigger that exercises the real pipeline (e.g. agent.waitingState injects a
  controlled `PreToolUse`-without-`Stop` to the real reporter socket) over a flaky
  real-world trigger — but say so in `why`.

## Area index

L1 (in-env):
- `10-editor.md`        — editors: open/close/split/tabs/save/dirty/diff/peek/format
- `11-terminal.md`      — terminals: new/split/kill/run/cwd/multiple/output/profiles
- `12-files.md`         — files: create/open/rename/delete/move/explorer/quickopen
- `13-scm-git.md`       — git: init/stage/commit/branch/diff/decorations/conflict
- `14-search.md`        — find/replace in files + in editor; symbol/quick search
- `15-diagnostics.md`   — language diagnostics, problems, code actions (needs +lang)
- `16-views-panels.md`  — views/panels/sidebar/palette/quick-open/statusbar/layout
- `17-settings.md`      — settings read/write/toggle; per-editor vs config-backed
- `18-extensions.md`    — installed list, activation, the fleet-bridge itself
- `19-agent-in-env.md`  — claude lifecycle → Hub: working/idle/done/waiting; multi-turn
- `1a-input.md`         — typeText/selection/keystroke into editor + terminal

L2 (Fleet-stack):
- `20-env-lifecycle.md` — spawn → phone-home → embed-reachable → close → cleanup
- `21-hub-state.md`     — session/run merge, rollup precedence, ephemeral vs persist, reap/TTL
- `22-reporter.md`      — claude/codex adapters, frame parse, correlation, S16 inference
- `23-multiplexer.md`   — rail, bridge registration, native-menu forward, embed, switch
- `24-agent-state-flow.md` — env claude → reporter → Hub → rail badge/ping end-to-end
- `25-deploy-spawn.md`  — supervisor spawn modes (process/container), env wiring, GC
- `26-networking.md`    — host↔container binds (FLEET_WS_ADDR/BRIDGE_ADDR), host.docker.internal

## Traceability

`run.mjs --list` shows each implemented test's id + provenance. A spec entry is
"covered" when a behaviour/scenario with a matching `specId: "<id>"` exists and is
green (or expected-skip). The implementation phase adds `specId` to each test and a
`--coverage` mode reports spec entries with no green test.
